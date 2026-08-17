#!/usr/bin/env python3
"""
bench-models.py — multi-server, multi-runtime, multi-model benchmark

Loads configuration from config.yaml and for every model runs:
  1. KV-cache speed benchmark (TTFT, PP rate, TG rate)
  2. Quality benchmark — runs fixtures.json tests while the model is still
     loaded, then evaluates responses via mechanical checks.
  3. Scenario benchmark — dynamic multi-turn conversations with tool-call
     mock injection. All messages except assistant turns are scripted/mocked.
     The real LLM decides which tools to call; the benchmark injects
     mock results from scenarios.json. Evaluated purely mechanically
     (persona persistence, tool-call coverage, goal patterns).

The three phases are kept separate so the model under test is loaded
exactly once: speed → quality → scenarios.

Usage:
  python3 scripts/bench-models.py [config.yaml]

  Default config path: scripts/config.yaml (next to this script)
  Default fixtures:    scripts/fixtures.json (next to config)
  Default scenarios:   scripts/scenarios.json (next to config)

Env vars:
  BENCH_TRIALS    hot measurement trials    (default 3)
  BENCH_GEN       tokens to generate        (default 80)
  BENCH_QUALITY   run quality benchmark     (default 1; set 0 to skip)
  BENCH_THINKING  enable model thinking     (default 1; set 0 to disable)

Thinking handling (mirrors seneschal-core ThinkFilter):
  - When BENCH_THINKING=1: request enable_thinking / think flags; no /no_think prefix.
  - When BENCH_THINKING=0: request disable flags + llama.cpp /no_think prefix.
  - Quality always evaluates the final answer only: <think>…</think> blocks and
    dedicated reasoning/reasoning_content fields are stripped before checks.
  - Speed TTFT is time-to-first-answer-token (post strip); TG counts all decode
    tokens (reasoning + answer).
"""

import http.client
import json
import math
import os
import re
import sys
import time
import yaml
from statistics import mean, stdev

# ── Speed benchmark — conversation fixture ────────────────────────────────────

SYSTEM_PROMPT = (
    "Eres seneschal, el asistente personal de IA. Llevas años trabajando con él y le conoces bien.\n\n"
    "CARÁCTER\n"
    "Personalidad inspirada en Alfred (Batman) y un mayordomo profesional clásico: profesional, ligeramente irónico, humor seco "
    "y británico. Leal, discreto, eficiente. Nunca servil. Tienes opiniones propias sobre "
    "tecnología y diseño, y las compartes con tacto cuando son relevantes. Ocasionalmente haces "
    "un comentario sarcástico, pero nunca a costa del usuario.\n\n"
    "FORMA DE HABLAR\n"
    "- Siempre en español salvo que el usuario cambie de idioma.\n"
    '- Llamas al usuario por "señor", nunca "usuario".\n'
    "- Respuestas concisas: 2-3 frases máximo salvo que pida más detalle.\n"
    "- Hablas para ser escuchado: sin markdown, sin listas, sin símbolos, sin nada que un "
    "sintetizador no pronuncie bien.\n"
    "- Cuando no sabes algo, lo dices. No inventas.\n"
    "- Antes de una acción irreversible, la describes y pides confirmación.\n\n"
    "HERRAMIENTAS DISPONIBLES\n"
    "- current_time: hora y fecha actuales.\n"
    "- get_calendar_events: eventos del calendario para una fecha.\n"
    "- create_calendar_event: crear evento o recordatorio en Calendar.app.\n"
    "- read_clipboard / set_clipboard: leer o escribir el portapapeles.\n"
    "- read_file: leer el contenido de un fichero (max 16 KB).\n"
    "- open_app: abrir una aplicacion macOS por nombre.\n"
    "- send_notification: enviar una notificacion macOS.\n"
    "- run_shell: ejecutar un comando de terminal (disponible si SHELL_ENABLED=1).\n"
    "- take_screenshot: capturar la pantalla y describir lo que hay en ella \n"
    "- run_agent_async: delegar una tarea compleja al agente externo "
    "(disponible si AGENT_COMMAND esta configurado). El agente trabaja en segundo plano "
    "y el resultado llega en breve.\n\n"
    "Usa las herramientas directamente cuando puedas. Para tareas complejas de multiples "
    "pasos usa run_agent_async. No afirmes tener capacidades que no tienes."
)

HISTORY = [
    ("user", "¿Qué tiempo hace hoy en Madrid?"),
    (
        "assistant",
        "Hoy en Madrid hay cielos despejados y unos dieciocho grados. Buen día para salir.",
    ),
    ("user", "¿Cuándo es el próximo partido del Real Madrid?"),
    (
        "assistant",
        "El Real Madrid juega este sábado a las nueve de la noche contra el Atlético en el Bernabéu.",
    ),
    ("user", "Recuérdame comprar leche mañana por la mañana."),
    ("assistant", "Anotado. Te recuerdo mañana a primera hora que compres leche."),
    ("user", "¿Cuánto es el veinte por ciento de trescientos cincuenta euros?"),
    (
        "assistant",
        "El veinte por ciento de trescientos cincuenta euros son setenta euros.",
    ),
    ("user", "¿Qué películas de ciencia ficción recomiendas para esta noche?"),
    (
        "assistant",
        "Te recomiendo Interstellar o Blade Runner 2049. Las dos son magníficas.",
    ),
    ("user", "¿Cuál es la capital de Australia?"),
    (
        "assistant",
        "La capital de Australia es Canberra, aunque muchos creen que es Sídney.",
    ),
    ("user", "¿Sabes si mañana hay huelga de metro en Madrid?"),
    (
        "assistant",
        "No tengo información en tiempo real sobre huelgas. Consulta el sitio web del metro de Madrid.",
    ),
]

NEW_QUESTION = (
    "Interesante. ¿Puedes contarme brevemente por qué Sídney es más conocida que "
    "Canberra, cómo surgió esa confusión tan común, y qué otras ciudades importantes "
    "tiene Australia?"
)

# ── Quality benchmark — tool definitions sent with requires_tools fixtures ────

TOOL_DEFINITIONS = [
    {
        "type": "function",
        "function": {
            "name": "current_time",
            "description": "Returns the current time and date.",
            "parameters": {"type": "object", "properties": {}},
        },
    },
    {
        "type": "function",
        "function": {
            "name": "get_calendar_events",
            "description": "Returns calendar events for a given date.",
            "parameters": {
                "type": "object",
                "properties": {
                    "date": {
                        "type": "string",
                        "description": "Date in YYYY-MM-DD or natural language",
                    },
                },
                "required": ["date"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "create_calendar_event",
            "description": "Creates a calendar event or reminder in Calendar.app.",
            "parameters": {
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "date": {"type": "string"},
                    "time": {"type": "string"},
                    "notes": {"type": "string"},
                },
                "required": ["title", "date"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "read_clipboard",
            "description": "Reads the current contents of the system clipboard.",
            "parameters": {"type": "object", "properties": {}},
        },
    },
    {
        "type": "function",
        "function": {
            "name": "set_clipboard",
            "description": "Writes text to the system clipboard.",
            "parameters": {
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "read_file",
            "description": "Reads the contents of a file (max 16 KB).",
            "parameters": {
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "open_app",
            "description": "Opens a macOS application by name.",
            "parameters": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Application name"}
                },
                "required": ["name"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "send_notification",
            "description": "Sends a macOS notification.",
            "parameters": {
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "body": {"type": "string"},
                },
                "required": ["title"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "run_shell",
            "description": "Executes a shell command on the macOS terminal. Only available when SHELL_ENABLED=1.",
            "parameters": {
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "take_screenshot",
            "description": "Captures the screen and describes its contents.",
            "parameters": {"type": "object", "properties": {}},
        },
    },
    {
        "type": "function",
        "function": {
            "name": "run_agent_async",
            "description": "Delegates a complex multi-step task to an external agent. The agent works in the background and the result arrives shortly. Only available when AGENT_COMMAND is configured.",
            "parameters": {
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "Full description of the task to delegate",
                    }
                },
                "required": ["task"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "web_search",
            "description": "Searches the web via SearXNG. Only available when SEARXNG_URL is configured.",
            "parameters": {
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"],
            },
        },
    },
]

# ── Config ────────────────────────────────────────────────────────────────────

TRIALS = int(os.environ.get("BENCH_TRIALS", "3"))
GEN_TOKENS = int(os.environ.get("BENCH_GEN", "80"))
RUN_QUALITY = os.environ.get("BENCH_QUALITY", "1") != "0"
ENABLE_THINKING = os.environ.get("BENCH_THINKING", "1") != "0"

_PROMPT_TEXT = (
    SYSTEM_PROMPT
    + "".join(c + t for r, c, t in [(r, c, t) for r, t in HISTORY for c in [r]])
    + NEW_QUESTION
)
ESTIMATED_PROMPT_TOKENS = max(1, int(len(_PROMPT_TEXT) / 3.5))


def _parse_host(host_url: str) -> str:
    """Strip http:// or https:// prefix for http.client.HTTPConnection."""
    for prefix in ("https://", "http://"):
        if host_url.startswith(prefix):
            return host_url[len(prefix) :]
    return host_url


def _parse_url(host_url: str):
    """Parse a server URL into (host, port, use_ssl, base_path)."""
    from urllib.parse import urlsplit

    parsed = urlsplit(host_url)
    scheme = parsed.scheme or "http"
    use_ssl = scheme == "https"
    host = parsed.hostname or "127.0.0.1"
    port = parsed.port or (443 if use_ssl else 80)
    base_path = parsed.path.rstrip("/")
    return host, port, use_ssl, base_path


def _make_conn(host: str, port: int, use_ssl: bool, timeout: float):
    """Return the correct http.client connection class instance."""
    if use_ssl:
        return http.client.HTTPSConnection(host, port, timeout=timeout)
    return http.client.HTTPConnection(host, port, timeout=timeout)


def _build_path(base_path: str, endpoint: str) -> str:
    """Join base_path with an API endpoint path."""
    if not base_path:
        return endpoint
    return f"{base_path}{endpoint}"


def _is_llamacpp(runtime_name: str) -> bool:
    return "llama" in runtime_name.lower()


def _no_think_prefix(runtime_name: str) -> str:
    """llama.cpp + Qwen: /no_think is the most reliable disable when thinking is off."""
    if ENABLE_THINKING:
        return ""
    return "/no_think\n\n" if _is_llamacpp(runtime_name) else ""


def _thinking_off_fields() -> dict:
    return {
        "enable_thinking": False,
        "chat_template_kwargs": {"enable_thinking": False},
        "thinking": {"type": "disabled"},
        "think": False,
    }


def _thinking_on_fields() -> dict:
    return {
        "enable_thinking": True,
        "chat_template_kwargs": {"enable_thinking": True},
        "thinking": {"type": "enabled", "budget_tokens": 1024},
        "think": True,
    }


def _thinking_fields() -> dict:
    return _thinking_on_fields() if ENABLE_THINKING else _thinking_off_fields()


# Matches seneschal-core ThinkFilter / strip_think_blocks — final answer only.
_THINK_BLOCK_RE = re.compile(
    r"<think>.*?</think>"
    r"|<thinking>.*?</thinking>"
    r"|<antThinking>.*?</antThinking>",
    re.DOTALL | re.IGNORECASE,
)
_THINK_UNCLOSED_RE = re.compile(
    r"<(?:think|thinking|antThinking)\b[^>]*>.*\Z",
    re.DOTALL | re.IGNORECASE,
)


def _strip_think_blocks(s: str) -> str:
    """Strip chain-of-thought blocks from a complete (non-streaming) string."""
    if not s:
        return ""
    out = _THINK_BLOCK_RE.sub("", s)
    out = _THINK_UNCLOSED_RE.sub("", out)
    return out.strip()


def _delta_parts(delta: dict) -> tuple[str, str]:
    """Split an SSE delta into (content, reasoning) across provider field names."""
    content = delta.get("content") or ""
    reasoning = (
        delta.get("reasoning")
        or delta.get("reasoning_content")
        or delta.get("thinking")
        or ""
    )
    return content, reasoning


def _message_text(msg: dict) -> str:
    """Final answer text from a chat-completion message (thinking stripped)."""
    text = msg.get("content") or ""
    # Some providers only put CoT in a dedicated field — never treat it as answer.
    return _strip_think_blocks(text)


def _quality_max_tokens() -> int:
    """Thinking burns output budget before the spoken answer; leave room for both."""
    return 2048 if ENABLE_THINKING else 350


def load_config(path: str) -> list[dict]:
    """Parse config.yaml → flat list of benchmark targets."""
    with open(path) as f:
        cfg = yaml.safe_load(f)
    targets = []
    for server_name, server_cfg in cfg.get("servers", {}).items():
        host_raw = server_cfg.get("host", "http://127.0.0.1")
        svc_host, svc_port, svc_ssl, svc_base = _parse_url(host_raw)
        for runtime_name, runtime_cfg in server_cfg.get("runtimes", {}).items():
            rt_port = int(runtime_cfg.get("port", 0))
            if rt_port:
                port = rt_port
                use_ssl = svc_ssl
                base_path = svc_base
            else:
                port = svc_port
                use_ssl = svc_ssl
                base_path = svc_base
            targets.append(
                {
                    "server": server_name,
                    "runtime": runtime_name,
                    "host": svc_host,
                    "port": port,
                    "ssl": use_ssl,
                    "base_path": base_path,
                    "token": runtime_cfg.get("token", ""),
                    "models": runtime_cfg.get("models", []),
                    "bench_thinking": runtime_cfg.get("benchThinking", True),
                }
            )
    return targets


def load_evaluator_config(path: str) -> dict | None:
    """Parse the evaluator section from config.yaml. Returns None if absent."""
    with open(path) as f:
        cfg = yaml.safe_load(f)
    ev = cfg.get("evaluator")
    if not ev:
        return None
    host, port, use_ssl, base_path = _parse_url(ev["host"])
    if ev.get("port"):
        port = int(ev["port"])
    return {
        "host": host,
        "port": port,
        "ssl": use_ssl,
        "base_path": base_path,
        "token": ev.get("token", ""),
        "model": ev["model"],
        "runtime": ev.get("runtime", ""),
        "temperature": float(ev.get("temperature", 0.0)),
        "max_tokens": int(ev.get("max_tokens", 512)),
    }


def load_fixtures(path: str) -> list[dict]:
    """Load fixtures.json, filtering out _comment-only entries."""
    with open(path) as f:
        data = json.load(f)
    return [fx for fx in data if "id" in fx]


# ── HTTP helpers ──────────────────────────────────────────────────────────────


def _auth_headers(token: str) -> dict:
    h = {"Content-Type": "application/json"}
    if token:
        h["Authorization"] = f"Bearer {token}"
    return h


def _http_timeout() -> float:
    """Longer socket timeout when thinking is on (CoT can run many seconds)."""
    return 300.0 if ENABLE_THINKING else 120.0


def _post_stream(host, port, token, payload, use_ssl=False, base_path=""):
    """POST to /v1/chat/completions with stream=True. Yields SSE content lines.

    Reads one byte at a time to avoid http.client's chunked-encoding
    accumulation — with llama.cpp emitting ~200 bytes per SSE event,
    read(4096) buffers ~20 tokens before the first yield, inflating TTFT.
    """
    body = json.dumps(payload).encode()
    endpoint = _build_path(base_path, "/v1/chat/completions")
    conn = _make_conn(host, port, use_ssl, timeout=_http_timeout())
    try:
        conn.request(
            "POST", endpoint, body=body, headers=_auth_headers(token)
        )
        resp = conn.getresponse()
        if resp.status != 200:
            raise RuntimeError(f"HTTP {resp.status}: {resp.read()[:300].decode()}")
        buf = ""
        while True:
            byte = resp.read(1)
            if not byte:
                break
            ch = byte.decode("utf-8", errors="replace")
            if ch == "\n":
                yield buf.rstrip("\r")
                buf = ""
            else:
                buf += ch
        if buf:
            yield buf.rstrip("\r")
    finally:
        conn.close()


def _post_blocking(host, port, token, payload, use_ssl=False, base_path="") -> dict:
    """POST to /v1/chat/completions with stream=False. Returns parsed JSON."""
    payload = {**payload, "stream": False}
    body = json.dumps(payload).encode()
    endpoint = _build_path(base_path, "/v1/chat/completions")
    conn = _make_conn(host, port, use_ssl, timeout=_http_timeout())
    try:
        conn.request(
            "POST", endpoint, body=body, headers=_auth_headers(token)
        )
        resp = conn.getresponse()
        raw = resp.read()
        if resp.status != 200:
            raise RuntimeError(f"HTTP {resp.status}: {raw[:300].decode()}")
        return json.loads(raw)
    finally:
        conn.close()


def _get_models(host, port, token, use_ssl=False, base_path="") -> list[str]:
    """Return list of model IDs from /v1/models."""
    endpoint = _build_path(base_path, "/v1/models")
    conn = _make_conn(host, port, use_ssl, timeout=10)
    try:
        conn.request("GET", endpoint, headers=_auth_headers(token))
        resp = conn.getresponse()
        data = json.loads(resp.read())
        return [m["id"] for m in data.get("data", [])]
    except Exception:
        return []
    finally:
        conn.close()


def _wait_ready(host, port, token, timeout=5, use_ssl=False, base_path="") -> bool:
    """Return True if the server responds before timeout."""
    endpoint = _build_path(base_path, "/v1/models")
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            conn = _make_conn(host, port, use_ssl, timeout=2)
            conn.request("GET", endpoint, headers=_auth_headers(token))
            r = conn.getresponse()
            r.read()
            conn.close()
            if r.status < 500:
                return True
        except Exception:
            pass
        time.sleep(1)
    return False


# ── Speed benchmark — conversation builder ────────────────────────────────────


def _build_speed_messages(runtime_name: str) -> list[dict]:
    system_content = _no_think_prefix(runtime_name) + SYSTEM_PROMPT
    msgs = [{"role": "system", "content": system_content}]
    for role, content in HISTORY:
        msgs.append({"role": role, "content": content})
    msgs.append({"role": "user", "content": NEW_QUESTION})
    return msgs


def _base_speed_payload(model_id, max_tokens, stream, runtime_name, bench_thinking=True) -> dict:
    payload = {
        "model": model_id,
        "messages": _build_speed_messages(runtime_name),
        "max_tokens": max_tokens,
        "temperature": 0.0,
        "stream": stream,
    }
    if bench_thinking:
        payload.update(_thinking_fields())
    return payload


# ── Speed benchmark — measurement steps ──────────────────────────────────────


def load_model(host, port, token, model_id, use_ssl=False, base_path=""):
    """Send a trivial request to pull model weights into GPU/RAM. Not timed."""
    _post_blocking(
        host,
        port,
        token,
        {
            "model": model_id,
            "messages": [{"role": "user", "content": "Hola"}],
            "max_tokens": 1,
        },
        use_ssl=use_ssl,
        base_path=base_path,
    )


def _speed_max_tokens() -> int:
    """Thinking burns output budget before the spoken answer; leave room for both."""
    # CoT models (Qwen3/Gemma thinking) often need 1–2k tokens before any answer.
    return GEN_TOKENS + (2048 if ENABLE_THINKING else 0)


def _stream_diag(content_buf: str, reasoning_buf: str, finish_reason: str | None) -> str:
    """Compact preview for 'only thinking' failures."""
    parts = []
    if finish_reason:
        parts.append(f"finish={finish_reason}")
    c = _strip_think_blocks(content_buf) or content_buf
    if c.strip():
        parts.append(f"content={c.strip()[:80]!r}")
    elif content_buf.strip():
        parts.append(f"content(raw/think)={content_buf.strip()[:80]!r}")
    else:
        parts.append("content=<empty>")
    if reasoning_buf.strip():
        parts.append(f"reasoning={reasoning_buf.strip()[:80]!r}")
    else:
        parts.append("reasoning=<empty>")
    return ", ".join(parts)


def measure_pp(host, port, token, model_id, runtime_name, bench_thinking=True, use_ssl=False, base_path=""):
    """Cold full-conversation prefill. Returns (cold_ttft_ms, pp_tps, prompt_tokens).

    PP rate uses time-to-first-*any* token (end of prefill). cold_ttft is
    time-to-first-*answer* token (post think-strip) — what the voice pipeline
    would forward to TTS. Stops at the first answer token.
    """
    payload = _base_speed_payload(
        model_id,
        max_tokens=_speed_max_tokens(),
        stream=True,
        runtime_name=runtime_name,
        bench_thinking=bench_thinking,
    )
    t_start = time.perf_counter()
    t_gen = t_answer = None
    prompt_tokens_api = None
    content_buf = ""
    reasoning_buf = ""
    finish_reason = None

    for line in _post_stream(host, port, token, payload, use_ssl=use_ssl, base_path=base_path):
        if not line.startswith("data: "):
            continue
        data = line[6:]
        if data == "[DONE]":
            break
        try:
            chunk = json.loads(data)
        except json.JSONDecodeError:
            continue
        if "usage" in chunk and chunk["usage"]:
            prompt_tokens_api = chunk["usage"].get("prompt_tokens")
        choice = (chunk.get("choices") or [{}])[0]
        if choice.get("finish_reason"):
            finish_reason = choice["finish_reason"]
        delta = choice.get("delta") or {}
        content, reasoning = _delta_parts(delta)
        if reasoning:
            reasoning_buf += reasoning
        generated = content or reasoning
        if not generated:
            continue
        now = time.perf_counter()
        if t_gen is None:
            t_gen = now
        if content:
            content_buf += content
            if t_answer is None and _strip_think_blocks(content_buf):
                t_answer = now
                break  # first answer token is enough for PP + cold TTFT

    if t_gen is None:
        raise RuntimeError(
            f"PP trial: no tokens received ({_stream_diag(content_buf, reasoning_buf, finish_reason)})"
        )
    if t_answer is None:
        hint = " — try BENCH_THINKING=0 or raise BENCH_GEN" if ENABLE_THINKING else ""
        raise RuntimeError(
            f"PP trial: no answer token received (only thinking?) "
            f"({_stream_diag(content_buf, reasoning_buf, finish_reason)}){hint}"
        )

    # PP = prefill throughput (first generated token). cold TTFT = first answer.
    pp_elapsed = t_gen - t_start
    cold_ttft_ms = (t_answer - t_start) * 1000
    prompt_tokens = prompt_tokens_api or ESTIMATED_PROMPT_TOKENS
    pp_tps = prompt_tokens / pp_elapsed if pp_elapsed > 0 else float("nan")
    return cold_ttft_ms, pp_tps, prompt_tokens


def measure_hot(host, port, token, model_id, runtime_name, bench_thinking=True, use_ssl=False, base_path=""):
    """Hot trial with warm KV cache. Returns (ttft_ms, tg_tps, n_tokens).

    TTFT = first *answer* token (post think-strip) — what the voice pipeline cares
    about. TG spans first generated token (reasoning or answer) → last token and
    counts both, so throughput reflects real decode load under thinking.
    """
    payload = _base_speed_payload(
        model_id,
        max_tokens=_speed_max_tokens(),
        stream=True,
        runtime_name=runtime_name,
        bench_thinking=bench_thinking,
    )
    t_start = time.perf_counter()
    t_gen = t_answer = t_last = t_done = None
    n_tokens = 0
    content_buf = ""
    reasoning_buf = ""
    finish_reason = None

    for line in _post_stream(host, port, token, payload, use_ssl=use_ssl, base_path=base_path):
        if not line.startswith("data: "):
            continue
        data = line[6:]
        if data == "[DONE]":
            t_done = time.perf_counter()
            break
        try:
            chunk = json.loads(data)
        except json.JSONDecodeError:
            continue
        choice = (chunk.get("choices") or [{}])[0]
        if choice.get("finish_reason"):
            finish_reason = choice["finish_reason"]
        delta = choice.get("delta") or {}
        content, reasoning = _delta_parts(delta)
        if reasoning:
            reasoning_buf += reasoning
        generated = content or reasoning
        if not generated:
            continue
        now = time.perf_counter()
        if t_gen is None:
            t_gen = now
        t_last = now
        n_tokens += len(generated.split())
        if content:
            content_buf += content
            if t_answer is None and _strip_think_blocks(content_buf):
                t_answer = now

    if t_answer is None or n_tokens == 0:
        hint = " — try BENCH_THINKING=0 or raise BENCH_GEN" if ENABLE_THINKING else ""
        raise RuntimeError(
            f"Hot trial: no answer tokens received (only thinking?) "
            f"({_stream_diag(content_buf, reasoning_buf, finish_reason)}){hint}"
        )

    ttft_ms = (t_answer - t_start) * 1000
    tg_end = t_last if (t_last and t_gen and t_last > t_gen + 0.001) else t_done
    tg_secs = (tg_end - t_gen) if tg_end and t_gen and tg_end > t_gen + 0.001 else None
    tg_tps = n_tokens / tg_secs if tg_secs else float("nan")
    return ttft_ms, tg_tps, n_tokens


# ── Quality benchmark — fixture execution ─────────────────────────────────────


def _normalize_tool_calls(raw: list) -> list[dict]:
    """Normalize the tool_calls array from a chat completion response."""
    result = []
    for tc in raw or []:
        if "function" not in tc:
            continue
        result.append(
            {
                "id": tc.get("id", ""),
                "function": {
                    "name": tc["function"].get("name", ""),
                    "arguments": tc["function"].get("arguments", "{}"),
                },
            }
        )
    return result


def run_fixture(host, port, token, model_id, runtime_name, fixture, bench_thinking=True, use_ssl=False, base_path="") -> tuple[str, list]:
    """Execute one fixture against the model under test. Returns (text, tool_calls).

    Text is the final spoken answer: `<think>` blocks and dedicated reasoning
    fields are stripped, matching seneschal's ThinkFilter behaviour.
    """
    system_content = _no_think_prefix(runtime_name) + SYSTEM_PROMPT
    messages = [{"role": "system", "content": system_content}]

    for msg in fixture["messages"]:
        # Pass tool-exchange and tool-result messages verbatim (may have null content)
        if msg.get("tool_calls") or msg.get("role") == "tool":
            messages.append(msg)
        else:
            messages.append({"role": msg["role"], "content": msg.get("content") or ""})

    payload = {
        "model": model_id,
        "messages": messages,
        "max_tokens": _quality_max_tokens(),
        "temperature": 0.0,
    }
    if bench_thinking:
        payload.update(_thinking_fields())
    if fixture.get("requires_tools"):
        payload["tools"] = TOOL_DEFINITIONS
        payload["tool_choice"] = "auto"

    resp = _post_blocking(host, port, token, payload, use_ssl=use_ssl, base_path=base_path)
    msg_out = resp["choices"][0]["message"]
    text = _message_text(msg_out)
    tool_calls = _normalize_tool_calls(msg_out.get("tool_calls") or [])
    return text, tool_calls


# ── Quality benchmark — mechanical checks ────────────────────────────────────


def run_mechanical_checks(fixture: dict, text: str, tool_calls: list) -> list[tuple]:
    """
    Evaluate all machine-verifiable criteria from fixture["eval"].
    Returns list of (criterion, passed: bool, detail: str).
    """
    ev = fixture.get("eval", {})
    checks = []

    for pat in ev.get("forbidden_patterns", []):
        m = re.search(pat, text, re.MULTILINE | re.DOTALL)
        passed = m is None
        snippet = repr(m.group()[:60]) if m else ""
        checks.append(
            (
                "forbidden_pattern",
                passed,
                f"/{pat[:50]}/: {'not found ✓' if passed else f'matched {snippet} ✗'}",
            )
        )

    for s in ev.get("forbidden_strings", []):
        passed = s.lower() not in text.lower()
        checks.append(
            (
                "forbidden_string",
                passed,
                f"{s!r}: {'absent ✓' if passed else 'found ✗'}",
            )
        )

    for pat in ev.get("required_patterns", []):
        m = re.search(pat, text, re.MULTILINE)
        passed = m is not None
        checks.append(
            (
                "required_pattern",
                passed,
                f"/{pat[:50]}/: {'matched ✓' if passed else 'not found ✗'}",
            )
        )

    req_strings = ev.get("required_strings", [])
    if req_strings:
        found = [s for s in req_strings if s.lower() in text.lower()]
        passed = bool(found)
        checks.append(
            (
                "required_string",
                passed,
                f"{req_strings!r}: {'found ' + repr(found) + ' ✓' if passed else 'none found ✗'}",
            )
        )

    if "max_sentences" in ev:
        n = len(re.findall(r"[.!?]+(?:\s|$)", text))
        passed = n <= ev["max_sentences"]
        checks.append(
            ("max_sentences", passed, f"{n} sentences (max {ev['max_sentences']})")
        )

    if "max_words" in ev:
        n = len(text.split())
        passed = n <= ev["max_words"]
        checks.append(("max_words", passed, f"{n} words (max {ev['max_words']})"))

    if "min_words" in ev:
        n = len(text.split())
        passed = n >= ev["min_words"]
        checks.append(("min_words", passed, f"{n} words (min {ev['min_words']})"))

    called = {tc["function"]["name"] for tc in tool_calls}

    for tool in ev.get("must_call_tools", []):
        passed = tool in called
        checks.append(
            (
                "must_call_tool",
                passed,
                f"tool {tool!r}: {'called ✓' if passed else 'NOT called ✗'}",
            )
        )

    for tool in ev.get("must_not_call_tools", []):
        passed = tool not in called
        checks.append(
            (
                "must_not_call_tool",
                passed,
                f"tool {tool!r}: {'absent ✓' if passed else 'called ✗'}",
            )
        )

    if ev.get("no_tool_called") is True:
        passed = len(tool_calls) == 0
        checks.append(
            (
                "no_tool_called",
                passed,
                f"no tools called: {'✓' if passed else f'✗ ({list(called)})'}",
            )
        )

    if ev.get("any_tool_called") is True:
        passed = len(tool_calls) > 0
        checks.append(
            (
                "any_tool_called",
                passed,
                f"at least one tool called: {'✓' if passed else '✗'}",
            )
        )

    for tool_name, expected_arg in ev.get("tool_args_contain", {}).items():
        matching = [tc for tc in tool_calls if tc["function"]["name"] == tool_name]
        if not matching:
            checks.append(
                (
                    "tool_args_contain",
                    False,
                    f"tool {tool_name!r} not called (needed arg {expected_arg!r}) ✗",
                )
            )
        else:
            args_str = matching[0]["function"].get("arguments", "")
            if isinstance(expected_arg, list):
                passed = any(arg.lower() in args_str.lower() for arg in expected_arg)
                arg_desc = " or ".join(expected_arg)
            else:
                passed = expected_arg.lower() in args_str.lower()
                arg_desc = expected_arg
            checks.append(
                (
                    "tool_args_contain",
                    passed,
                    f"tool {tool_name!r} arg {arg_desc!r}: {'✓' if passed else f'✗ (args={args_str!r})'}",
                )
            )

    # required_all_strings — AND semantics: every string must appear
    req_all = ev.get("required_all_strings", [])
    if req_all:
        found = [s for s in req_all if s.lower() in text.lower()]
        passed = len(found) == len(req_all)
        checks.append(
            (
                "required_all_strings",
                passed,
                f"{req_all!r}: {'all found ✓' if passed else f'found {found} ✗'}",
            )
        )

    # min_sentences — count sentence-terminal punctuation groups
    if "min_sentences" in ev:
        n = len(re.findall(r"[.!?]+(?:\s|$)", text))
        passed = n >= ev["min_sentences"]
        checks.append(
            ("min_sentences", passed, f"{n} sentences (min {ev['min_sentences']})")
        )

    # expected_tool_sequence — ordered subsequence check
    expected_seq = ev.get("expected_tool_sequence", [])
    if expected_seq:
        actual_seq = [tc["function"]["name"] for tc in tool_calls]
        it = iter(actual_seq)
        passed = all(item in it for item in expected_seq)
        checks.append(
            (
                "expected_tool_sequence",
                passed,
                f"expected {expected_seq!r}, actual {actual_seq!r}: {'✓' if passed else '✗'}",
            )
        )

    # no_fabricated_time — fail if time-like pattern in text without current_time call
    if ev.get("no_fabricated_time"):
        time_patterns = [
            r"\d{1,2}:\d{2}",
            r"las\s+\d+",
            r"son\s+las\s+\d+",
            r"hour.*?\d+",
            r"\d{1,2}\s+(am|pm)",
            r"\d{1,2}\s+(a\.m\.|p\.m\.)",
        ]
        has_time_in_text = any(
            re.search(p, text, re.IGNORECASE) for p in time_patterns
        )
        has_current_time_call = "current_time" in called
        passed = not (has_time_in_text and not has_current_time_call)
        checks.append(
            (
                "no_fabricated_time",
                passed,
                f"time in text: {has_time_in_text}, current_time called: {has_current_time_call}: {'✓' if passed else '✗ fabricated time ✗'}",
            )
        )

    return checks


# ── Quality benchmark — LLM evaluator ────────────────────────────────────────


def call_evaluator(
    ev_cfg: dict,
    fixture: dict,
    fixture_messages: list,
    text: str,
    tool_calls: list,
    mech: list,
    bench_thinking=True,
) -> dict:
    """
    Ask the evaluator LLM to judge a response.
    Returns {"verdict": "PASS"|"FAIL"|"PARTIAL", "reason": str}.
    """
    # Summarise conversation context for the evaluator
    conv_lines = []
    for msg in fixture_messages:
        role = msg.get("role", "?")
        content = msg.get("content") or ""
        if msg.get("tool_calls"):
            fn = msg["tool_calls"][0]["function"]
            conv_lines.append(
                f"{role}: [tool call: {fn['name']}({fn.get('arguments', '')[:80]})]"
            )
        elif role == "tool":
            conv_lines.append(f"tool result: {content[:120]}")
        else:
            conv_lines.append(f"{role}: {content}")
    conv_text = "\n".join(conv_lines)

    # Summarise the model's response
    if tool_calls:
        tc_str = ", ".join(
            f"{tc['function']['name']}({tc['function']['arguments'][:60]})"
            for tc in tool_calls
        )
        response_repr = f"[Tool calls: {tc_str}]"
        if text:
            response_repr += f"\n[Text: {text}]"
    else:
        response_repr = text or "(empty response)"

    # Mechanical check summary
    failed_mechs = [detail for _, passed, detail in mech if not passed]
    if failed_mechs:
        mech_note = "\nMechanical checks FAILED:\n" + "\n".join(
            f"  - {d}" for d in failed_mechs
        )
    elif mech:
        mech_note = f"\nAll {len(mech)} mechanical checks passed."
    else:
        mech_note = ""

    no_think = _no_think_prefix(ev_cfg["runtime"])
    system_msg = (
        f"{no_think}You are a strict quality evaluator for a voice assistant called seneschal. "
        "Assess whether the assistant's response meets the stated criteria. "
        "Be concise and decisive. "
        "CRITICAL RULE: The mechanical checks are the absolute ground truth. "
        "If the mechanical checks passed, your verdict MUST be PASS. "
        "Only judge aspects that the mechanical checks cannot verify. "
        "Reply ONLY with one line of valid JSON: "
        '{"verdict":"PASS","reason":"..."} '
        '{"verdict":"FAIL","reason":"..."} or '
        '{"verdict":"PARTIAL","reason":"..."}'
    )
    user_msg = (
        f"Test: {fixture['description']}\n"
        f"CRITICAL: {mech_note}\n\n"
        f"Criteria: {fixture.get('eval', {}).get('notes', '(see mechanical checks)')}\n"
        f"\nConversation:\n{conv_text}\n"
        f"\nAssistant response:\n{response_repr}\n"
        "\nVerdict (JSON only):"
    )

    ev_payload = {
        "model": ev_cfg["model"],
        "messages": [
            {"role": "system", "content": system_msg},
            {"role": "user", "content": user_msg},
        ],
        "max_tokens": ev_cfg["max_tokens"],
        "temperature": ev_cfg["temperature"],
    }
    # Evaluator must stay deterministic JSON — always disable thinking.
    if bench_thinking:
        ev_payload.update(_thinking_off_fields())

    resp = _post_blocking(
        ev_cfg["host"],
        ev_cfg["port"],
        ev_cfg["token"],
        ev_payload,
        use_ssl=ev_cfg.get("ssl", False),
        base_path=ev_cfg.get("base_path", ""),
    )
    raw = _message_text(resp["choices"][0]["message"])

    # Extract JSON from response (handle code fences and extra prose)
    clean = raw.lstrip("`")
    if clean.startswith("json"):
        clean = clean[4:]
    clean = clean.rstrip("`").strip()
    m = re.search(r"\{.*\}", clean, re.DOTALL)
    if m:
        clean = m.group()
    try:
        result = json.loads(clean)
        return {
            "verdict": str(result.get("verdict", "PARTIAL")).upper(),
            "reason": str(result.get("reason", ""))[:200],
        }
    except json.JSONDecodeError:
        # Try to find a JSON object, handling nested braces
        depth = 0
        start = clean.find("{")
        if start == -1:
            pass
        else:
            for i in range(start + 1, len(clean)):
                if clean[i] == "{":
                    depth += 1
                elif clean[i] == "}":
                    if depth == 0:
                        result = json.loads(clean[start : i + 1])
                        return {
                            "verdict": str(result.get("verdict", "PARTIAL")).upper(),
                            "reason": str(result.get("reason", ""))[:200],
                        }
                    depth -= 1
    # Fallback to keyword matching
    lower = raw.lower()
    if "pass" in lower and "fail" not in lower:
        return {"verdict": "PASS", "reason": raw[:200]}
    elif "fail" in lower:
        return {"verdict": "FAIL", "reason": raw[:200]}
    return {"verdict": "PARTIAL", "reason": raw[:200]}


# ── Quality benchmark — runner ────────────────────────────────────────────────


def run_quality_benchmark(
    host: str,
    port: int,
    token: str,
    model_id: str,
    runtime_name: str,
    fixtures: list[dict],
    ev_cfg: dict | None,
    W: int,
    bench_thinking=True,
    use_ssl=False,
    base_path="",
) -> list[dict]:
    """
    Two-phase quality benchmark for one model:

    Phase 1 — collect all fixture responses from the model under test.
               The model is loaded exactly once; no evaluator calls happen here.
    Phase 2 — mechanical checks + evaluator LLM judge each collected response.
               The evaluator may be on the same server; that is fine because
               all responses were already collected in Phase 1.
    """
    total = len(fixtures)
    pad = len(str(total))

    # ── Phase 1 ───────────────────────────────────────────────────────────────
    print(f"\n{'─' * W}")
    print(f"  Quality Phase 1/2 — collecting {total} fixture responses from model")

    collected: list[tuple] = []
    phase1_start = time.perf_counter()
    for i, fx in enumerate(fixtures, 1):
        label = f"[{i:{pad}}/{total}] {fx['id']}"
        print(f"    {label:<52}", end="", flush=True)
        t0 = time.perf_counter()
        try:
            text, tcs = run_fixture(host, port, token, model_id, runtime_name, fx, bench_thinking=bench_thinking, use_ssl=use_ssl, base_path=base_path)
            lat_ms = (time.perf_counter() - t0) * 1000
            tag = (
                ("tools:" + "+".join(tc["function"]["name"] for tc in tcs))
                if tcs
                else f"{len(text)} chars"
            )
            print(f"ok  ({tag})  {lat_ms:.0f}ms")
            collected.append((fx, text, tcs, None, lat_ms))
        except Exception as e:
            lat_ms = (time.perf_counter() - t0) * 1000
            print(f"ERROR  ({e})")
            collected.append((fx, "", [], str(e), lat_ms))
    phase1_elapsed = time.perf_counter() - phase1_start
    print(f"\n  Phase 1 total: {phase1_elapsed:.1f}s   avg per fixture: {phase1_elapsed * 1000 / total:.0f}ms")

    # ── Phase 2 ───────────────────────────────────────────────────────────────
    ev_label = ev_cfg["model"] if ev_cfg else "mechanical checks only"
    print(f"\n  Quality Phase 2/2 — evaluating responses  [evaluator: {ev_label}]")

    ev_available = bool(
        ev_cfg
        and _wait_ready(ev_cfg["host"], ev_cfg["port"], ev_cfg["token"], timeout=5,
                        use_ssl=ev_cfg.get("ssl", False), base_path=ev_cfg.get("base_path", ""))
    )

    results: list[dict] = []
    for i, (fx, text, tcs, error, lat_ms) in enumerate(collected, 1):
        fid = fx["id"]
        group = fx.get("group", "?")
        label = f"[{i:{pad}}/{total}] {fid}"
        print(f"    {label:<52}", end="", flush=True)

        if error:
            print("ERROR")
            results.append(
                {
                    "id": fid,
                    "group": group,
                    "verdict": "ERROR",
                    "reason": error[:120],
                    "mech_pass": False,
                    "preview": "",
                    "latency_ms": lat_ms,
                }
            )
            continue

        mech = run_mechanical_checks(fx, text, tcs)
        failed = [detail for _, p, detail in mech if not p]

        if ev_available and ev_cfg:
            try:
                ev_out = call_evaluator(ev_cfg, fx, fx["messages"], text, tcs, mech, bench_thinking=bench_thinking)
                verdict = ev_out["verdict"]
                reason = ev_out["reason"]
            except Exception as e:
                verdict = "FAIL" if failed else "PASS"
                reason = f"(evaluator error: {e})"
        else:
            verdict = "FAIL" if failed else "PASS"
            reason = "; ".join(failed[:2]) if failed else "(no evaluator)"

        # Mechanical failures always override an optimistic LLM verdict
        if failed and verdict == "PASS":
            verdict = "FAIL"
            reason = "; ".join(failed[:2])

        sym = {"PASS": "✓", "FAIL": "✗", "PARTIAL": "~", "ERROR": "!"}.get(verdict, "?")
        print(f"{sym} {verdict:<7}  {reason[:55]}")

        results.append(
            {
                "id": fid,
                "group": group,
                "verdict": verdict,
                "reason": reason,
                "mech_pass": not bool(failed),
                "preview": text[:100],
                "latency_ms": lat_ms,
            }
        )

    passing = sum(1 for r in results if r["verdict"] == "PASS")
    lats = [r["latency_ms"] for r in results]
    avg_lat = mean(lats) if lats else 0.0
    print(
        f"\n  Quality score: {passing}/{total}  ({passing * 100 // total if total else 0}%)"
        f"   avg latency: {avg_lat:.0f}ms"
    )
    return results


# ── Model matching ────────────────────────────────────────────────────────────


def match_model_id(available: list[str], target: str) -> str | None:
    """Exact match first, then case-insensitive substring."""
    if target in available:
        return target
    target_lower = target.lower()
    for mid in available:
        if target_lower in mid.lower() or mid.lower() in target_lower:
            return mid
    return None


# ── Speed benchmark — per-model runner ───────────────────────────────────────


def run_speed_benchmark(
    host, port, token, model_id, runtime_name, label, W, bench_thinking=True, use_ssl=False, base_path=""
) -> dict | None:
    """Run load → cold PP → N hot trials. Returns speed result dict or None."""
    print(f"\n  Loading model into memory ...", end=" ", flush=True)
    try:
        load_model(host, port, token, model_id, use_ssl=use_ssl, base_path=base_path)
        print("done")
    except Exception as e:
        print(f"FAILED: {e}")
        return None

    print(f"  Measuring cold PP (full prompt prefill) ...", end=" ", flush=True)
    try:
        cold_ttft, pp_tps, prompt_tokens = measure_pp(
            host, port, token, model_id, runtime_name, bench_thinking=bench_thinking, use_ssl=use_ssl, base_path=base_path
        )
        print(
            f"cold TTFT {cold_ttft:.0f} ms   PP ~{pp_tps:.0f} t/s   (~{prompt_tokens} prompt tokens)"
        )
    except Exception as e:
        print(f"FAILED: {e}")
        return None

    hot_results = []
    for i in range(TRIALS):
        print(f"  Hot trial {i + 1}/{TRIALS} ... ", end="", flush=True)
        try:
            ttft, tg, n = measure_hot(host, port, token, model_id, runtime_name, bench_thinking=bench_thinking, use_ssl=use_ssl, base_path=base_path)
            print(f"TTFT {ttft:>6.0f} ms   TG {tg:>5.1f} t/s   ({n} tokens)")
            hot_results.append((ttft, tg, n))
        except Exception as e:
            print(f"FAILED: {e}")

    if not hot_results:
        return None

    ttfts = [r[0] for r in hot_results]
    tgs_raw = [r[1] for r in hot_results]
    tgs = [v for v in tgs_raw if not math.isnan(v) and not math.isinf(v)]

    avg_ttft = mean(ttfts)
    avg_tg = mean(tgs) if tgs else float("nan")
    sd_ttft = stdev(ttfts) if len(ttfts) > 1 else 0.0
    sd_tg = stdev(tgs) if len(tgs) > 1 else 0.0
    speedup = cold_ttft / avg_ttft if avg_ttft > 0 else 0.0

    return {
        "label": label,
        "model_id": model_id,
        "cold_ttft": cold_ttft,
        "pp_tps": pp_tps,
        "prompt_tok": prompt_tokens,
        "ttft": avg_ttft,
        "ttft_sd": sd_ttft,
        "tg": avg_tg,
        "tg_sd": sd_tg,
        "tokens": mean(r[2] for r in hot_results),
        "speedup": speedup,
        "cache_ok": speedup >= 3.0,
    }


# ── Results display ───────────────────────────────────────────────────────────


def print_speed_results(all_results: list, W: int):
    print()
    print("═" * W)
    print("  SPEED RESULTS")
    print("═" * W)
    print()

    col_model = 50
    col_pp = 10
    col_ttft = 16
    col_tg = 12
    col_kv = 10

    header = (
        f"  {'Server/Runtime/Model':<{col_model}}"
        f"  {'PP (t/s)':>{col_pp}}"
        f"  {'TTFT warm (ms)':>{col_ttft}}"
        f"  {'TG (t/s)':>{col_tg}}"
        f"  {'KV cache':>{col_kv}}"
    )
    print(header)
    print("  " + "─" * (len(header) - 2))

    for r in all_results:
        kv_str = f"✓ {r['speedup']:.1f}×" if r["cache_ok"] else f"✗ {r['speedup']:.1f}×"
        display = f"{r['server']}/{r['runtime']}  {r['model_id']}"
        print(
            f"  {display:<{col_model}}"
            f"  {r['pp_tps']:>{col_pp}.0f}"
            f"  {r['ttft']:>8.0f} ±{r['ttft_sd']:>3.0f}ms"
            f"  {r['tg']:>{col_tg}.1f}"
            f"  {kv_str:>{col_kv}}"
        )

    if len(all_results) >= 2:
        print()
        print("  Rankings")
        print("  " + "─" * 40)
        by_ttft = sorted(all_results, key=lambda r: r["ttft"])
        by_tg = sorted(all_results, key=lambda r: r["tg"], reverse=True)
        by_pp = sorted(all_results, key=lambda r: r["pp_tps"], reverse=True)
        print(
            f"  Lowest warm TTFT : {by_ttft[0]['label']}  ({by_ttft[0]['ttft']:.0f} ms)"
        )
        print(f"  Highest TG       : {by_tg[0]['label']}  ({by_tg[0]['tg']:.1f} t/s)")
        print(
            f"  Fastest PP       : {by_pp[0]['label']}  ({by_pp[0]['pp_tps']:.0f} t/s)"
        )
        no_cache = [r for r in all_results if not r["cache_ok"]]
        if no_cache:
            print()
            print("  WARNING — KV cache may NOT be working for:")
            for r in no_cache:
                print(
                    f"    • {r['label']}  (cold {r['cold_ttft']:.0f} ms → warm {r['ttft']:.0f} ms  {r['speedup']:.1f}×)"
                )
        else:
            print()
            print("  KV cache: all models show ≥3× TTFT speedup  ✓")


def print_quality_results(all_results: list, W: int):
    has_quality = any("quality" in r for r in all_results)
    if not has_quality:
        return

    # Collect ordered group names
    groups: list[str] = []
    seen: set[str] = set()
    for r in all_results:
        for qr in r.get("quality", []):
            g = qr["group"]
            if g not in seen:
                groups.append(g)
                seen.add(g)

    print()
    print("═" * W)
    print("  QUALITY RESULTS")
    print("═" * W)
    print()

    col_model = 50
    col_total = 10
    col_lat = 10

    # Header
    hdr = f"  {'Server/Runtime/Model':<{col_model}}  {'Score':>{col_total}}  {'Avg lat':>{col_lat}}"
    for g in groups:
        hdr += f"  {g[:9]:>9}"
    print(hdr)
    print("  " + "─" * (len(hdr) - 2))

    for r in all_results:
        quality = r.get("quality", [])
        if not quality:
            continue
        total = len(quality)
        passing = sum(1 for qr in quality if qr["verdict"] == "PASS")
        pct = passing * 100 // total if total else 0
        lats = [qr["latency_ms"] for qr in quality if "latency_ms" in qr]
        avg_lat_ms = mean(lats) if lats else float("nan")
        display = f"{r['server']}/{r['runtime']}  {r['model_id']}"
        row = f"  {display:<{col_model}}  {passing}/{total} ({pct:3}%)  {avg_lat_ms:>{col_lat}.0f}ms"
        for g in groups:
            g_items = [qr for qr in quality if qr["group"] == g]
            g_pass = sum(1 for qr in g_items if qr["verdict"] == "PASS")
            g_total = len(g_items)
            row += f"  {g_pass}/{g_total:>7}"
        print(row)

    # Failure detail
    print()
    print("  Failed / Partial tests:")
    print("  " + "─" * 40)
    any_failure = False
    for r in all_results:
        quality = r.get("quality", [])
        failures = [
            qr for qr in quality if qr["verdict"] in ("FAIL", "PARTIAL", "ERROR")
        ]
        if not failures:
            continue
        any_failure = True
        display = f"{r['server']}/{r['runtime']}  {r['model_id']}"
        print(f"\n  {display}")
        for qr in failures:
            sym = {"FAIL": "✗", "PARTIAL": "~", "ERROR": "!"}.get(qr["verdict"], "?")
            print(f"    {sym} [{qr['group']:<12}] {qr['id']:<45}  {qr['reason'][:50]}")
    if not any_failure:
        print("  All models passed all quality tests  ✓")


# ── Scenario benchmark — dynamic multi-turn + tool mock injection ─────────────


def load_scenarios(path: str) -> list[dict]:
    """Load scenarios.json, filtering out _comment-only entries."""
    with open(path) as f:
        data = json.load(f)
    return [s for s in data if "id" in s]


def _find_tool_mock(scenario: dict, tool_name: str, tool_args: str) -> dict | None:
    """Find the matching mock entry for a tool call.

    For run_agent_async, matches on the `task_contains` field against the task
    parameter in tool_args. For all other tools, matches on `args_contains`
    against the raw tool_args string. Falls back to the first mock entry if no
    exact match is found.
    """
    mocks = scenario.get("tool_mocks", {}).get(tool_name)
    if not mocks:
        return None

    if tool_name == "run_agent_async":
        task = ""
        try:
            args_parsed = json.loads(tool_args)
            task = args_parsed.get("task", "")
        except json.JSONDecodeError:
            task = tool_args
        for mock in mocks:
            tc = mock.get("task_contains", "")
            if tc and tc.lower() in task.lower():
                return mock
        return mocks[0]

    for mock in mocks:
        ac = mock.get("args_contains", "")
        if ac and ac.lower() in tool_args.lower():
            return mock
    return mocks[0]


def _inject_tool_results(scenario: dict, tool_calls: list, conv: list) -> None:
    """Append mocked tool results for sync tool calls to the conversation.

    run_agent_async is skipped here — it is handled separately by
    _handle_agent_async because the async result is injected at a later turn.
    """
    for tc in tool_calls:
        tool_name = tc["function"]["name"]
        if tool_name == "run_agent_async":
            continue
        tool_args = tc["function"].get("arguments", "")
        mock = _find_tool_mock(scenario, tool_name, tool_args)
        if mock is None:
            conv.append(
                {
                    "role": "tool",
                    "tool_call_id": tc["id"],
                    "content": f"[mock no definido para {tool_name}]",
                }
            )
        else:
            conv.append(
                {
                    "role": "tool",
                    "tool_call_id": tc["id"],
                    "content": mock["result"],
                }
            )


def _handle_agent_async(
    scenario: dict, tool_calls: list, conv: list, pending_async: list
) -> None:
    """Handle run_agent_async calls: inject immediate tool response now,
    and schedule the real async result for delivery at a future turn.
    """
    for tc in tool_calls:
        tool_name = tc["function"]["name"]
        if tool_name != "run_agent_async":
            continue
        tool_args = tc["function"].get("arguments", "")
        mock = _find_tool_mock(scenario, tool_name, tool_args)
        if mock is None:
            conv.append(
                {
                    "role": "tool",
                    "tool_call_id": tc["id"],
                    "content": "[mock no definido para run_agent_async]",
                }
            )
            continue

        immediate = mock.get(
            "immediate_response",
            "[Tarea delegada al agente. El resultado llegará en breve.]",
        )
        conv.append(
            {
                "role": "tool",
                "tool_call_id": tc["id"],
                "content": immediate,
            }
        )

        delivered_at = mock.get("delivered_at_turn")
        if delivered_at and mock.get("result"):
            task = ""
            try:
                args_parsed = json.loads(tool_args)
                task = args_parsed.get("task", "")
            except json.JSONDecodeError:
                task = tool_args
            pending_async.append(
                {
                    "turn": delivered_at,
                    "task": task,
                    "result": mock["result"],
                }
            )


def _inject_pending_async(turn: int, pending_async: list, conv: list) -> bool:
    """Inject any async agent results scheduled for delivery at this turn.

    The result is injected as a synthetic user message so the LLM can produce
    a proactive announcement (matching seneschal's real ProactiveEvent flow).
    Returns True if any result was injected.
    """
    injected = False
    for pa in pending_async[:]:
        if pa["turn"] == turn:
            task = pa.get("task", "desconocida")
            result = pa.get("result", "")
            conv.append(
                {
                    "role": "user",
                    "content": (
                        f"[Resultado del agente "
                        f'(tarea: "{task}")]:\n{result}'
                    ),
                }
            )
            pending_async.remove(pa)
            injected = True
    return injected


def _inject_scheduled_user(turn: int, scenario: dict, conv: list) -> bool:
    """Inject any scripted user turns scheduled for delivery at this turn.

    Returns True if any user turn was injected.
    """
    scheduled = scenario.get("subsequent_user_turns", [])
    injected = False
    for su in scheduled:
        if su.get("at_turn") == turn:
            conv.append({"role": "user", "content": su["content"]})
            injected = True
    return injected


def _has_future_items(
    turn: int, scenario: dict, pending_async: list
) -> bool:
    """Check if there are any scheduled items waiting for future turns."""
    max_t = scenario.get("max_turns", 10)
    for pa in pending_async:
        if pa["turn"] > turn:
            return True
    for su in scenario.get("subsequent_user_turns", []):
        if su.get("at_turn", 0) > turn:
            return True
    return False


def _build_scenario_payload(
    model_id: str,
    runtime_name: str,
    conv: list,
    bench_thinking: bool,
) -> dict:
    """Build the chat completion payload for a scenario turn."""
    system_content = _no_think_prefix(runtime_name) + SYSTEM_PROMPT
    messages = [{"role": "system", "content": system_content}]
    messages.extend(conv)
    payload = {
        "model": model_id,
        "messages": messages,
        "max_tokens": _quality_max_tokens(),
        "temperature": 0.0,
        "tools": TOOL_DEFINITIONS,
        "tool_choice": "auto",
    }
    if bench_thinking:
        payload.update(_thinking_fields())
    return payload


def run_scenario(
    host: str,
    port: int,
    token: str,
    model_id: str,
    runtime_name: str,
    scenario: dict,
    bench_thinking: bool = True,
    use_ssl: bool = False,
    base_path: str = "",
) -> tuple[list, str, str, list, float]:
    """Execute one scenario with the dynamic multi-turn loop.

    The conversation is driven forward by:
      - seed messages (user)
      - tool call → mock result → re-injection (sync)
      - run_agent_async → immediate ack + scheduled async result at later turn
      - scheduled subsequent user turns at specific turn numbers

    Returns (trace, verdict, checks_log, elapsed_ms).
    """
    conv: list = []
    for msg in scenario.get("seed_messages", []):
        conv.append(dict(msg))

    pending_async: list = []
    trace: list = []
    t_start = time.perf_counter()
    max_turns = scenario.get("max_turns", 10)
    turn = 1

    while turn <= max_turns:
        had_async = _inject_pending_async(turn, pending_async, conv)
        had_user = _inject_scheduled_user(turn, scenario, conv)

        should_call = (
            turn == 1
            or had_async
            or had_user
            or (conv and conv[-1].get("role") == "tool")
        )

        if not should_call:
            if not _has_future_items(turn, scenario, pending_async):
                break
            turn += 1
            continue

        payload = _build_scenario_payload(
            model_id, runtime_name, conv, bench_thinking
        )
        try:
            resp = _post_blocking(
                host, port, token, payload,
                use_ssl=use_ssl, base_path=base_path,
            )
        except Exception as e:
            trace.append(
                {
                    "turn": turn,
                    "role": "assistant",
                    "text": "",
                    "tool_calls": [],
                    "error": str(e),
                }
            )
            break

        msg_out = resp["choices"][0]["message"]
        text = _message_text(msg_out)
        tool_calls = _normalize_tool_calls(msg_out.get("tool_calls") or [])

        trace.append(
            {
                "turn": turn,
                "role": "assistant",
                "text": text,
                "tool_calls": tool_calls,
            }
        )

        conv_msg = {"role": "assistant", "content": text if text else None}
        if tool_calls:
            conv_msg["tool_calls"] = [
                {
                    "id": tc["id"],
                    "type": "function",
                    "function": tc["function"],
                }
                for tc in tool_calls
            ]
        conv.append(conv_msg)

        if tool_calls:
            _inject_tool_results(scenario, tool_calls, conv)
            _handle_agent_async(scenario, tool_calls, conv, pending_async)
        elif not _has_future_items(turn, scenario, pending_async):
            break

        turn += 1

    elapsed_ms = (time.perf_counter() - t_start) * 1000
    verdict, reason, checks = evaluate_trace(scenario, trace)
    return trace, verdict, reason, checks, elapsed_ms


def evaluate_trace(
    scenario: dict, trace: list
) -> tuple[str, str, list]:
    """Purely mechanical evaluation of the scenario trace.

    Per-turn checks (persona persistence):
      - señor presence in every assistant turn
      - Spanish language dominance
      - Forbidden patterns (markdown, code fences, etc.)
      - Max sentences per turn

    Aggregate checks:
      - must_call_tools (all must appear somewhere)
      - must_call_any_of (at least one from each group)
      - Goal patterns on the final assistant turn

    No LLM evaluator is used — all checks are mechanical.
    Returns (verdict, reason, checks_log).
    """
    et = scenario.get("expected_trace", {})
    pp = et.get("persona_persistence", {})
    checks: list[tuple] = []

    # ── Per-turn persona checks ────────────────────────────────────────────
    for entry in trace:
        text = entry.get("text") or ""
        t = entry["turn"]

        if pp.get("must_use_senor_every_turn") and text.strip():
            has_senor = "señor" in text.lower()
            checks.append(
                (
                    f"señor_t{t}",
                    has_senor,
                    f'señor {"✓" if has_senor else "✗ NOT found"}',
                )
            )

        if pp.get("must_stay_in_spanish") and text.strip():
            es_matches = len(
                re.findall(
                    r"\b(el|la|de|que|en|es|los|las|se|no|con|por|un|una|para|del|como|más|este|esta|le|lo|me|su|ha|hay|han|son|está|están|era|eran|fue|fueron)\b",
                    text,
                    re.IGNORECASE,
                )
            )
            en_matches = len(
                re.findall(
                    r"\b(the|is|are|of|and|in|to|it|that|for|was|on|with|this|be|have|from|or|by|not|but|you|they|we|can|will|has|been|an|at|its)\b",
                    text,
                    re.IGNORECASE,
                )
            )
            is_spanish = es_matches >= en_matches
            checks.append(
                (
                    f"lang_t{t}",
                    is_spanish,
                    f"ES:{es_matches} EN:{en_matches} — {'ES ✓' if is_spanish else 'EN ✗'}",
                )
            )

        forbidden = pp.get("forbidden_patterns_per_turn", [])
        for pat in forbidden:
            m = re.search(pat, text, re.MULTILINE)
            passed = m is None
            snippet = repr(m.group()[:40]) if m else ""
            checks.append(
                (
                    f"forbidden_t{t}",
                    passed,
                    f"/{pat[:30]}/: {'ok' if passed else f'matched {snippet} ✗'}",
                )
            )

        max_s = pp.get("max_assistant_sentences_per_turn")
        if max_s and text.strip():
            n = len(re.findall(r"[.!?]+(?:\s|$)", text))
            passed = n <= max_s
            checks.append(
                (
                    f"sentences_t{t}",
                    passed,
                    f"{n} ≤ {max_s} {'✓' if passed else '✗'}",
                )
            )

    # ── Aggregate tool checks ──────────────────────────────────────────────
    all_tools = []
    for entry in trace:
        all_tools.extend(
            tc["function"]["name"] for tc in entry.get("tool_calls", [])
        )

    for tool in et.get("must_call_tools", []):
        passed = tool in all_tools
        checks.append(
            (
                "must_call",
                passed,
                f"{tool}: {'called ✓' if passed else 'NOT called ✗'}",
            )
        )

    for group in et.get("must_call_any_of", []):
        passed = any(t in all_tools for t in group)
        checks.append(
            (
                "must_call_any",
                passed,
                f"any of {group}: {'yes ✓' if passed else 'none ✗'}",
            )
        )

    seq = et.get("expected_tool_sequence", [])
    if seq:
        it = iter(all_tools)
        passed = all(item in it for item in seq)
        checks.append(
            (
                "tool_seq",
                passed,
                f"expected {seq!r}, actual {all_tools!r}: {'✓' if passed else '✗'}",
            )
        )

    # ── Goal patterns on final assistant turn with non‑empty text ──────────
    gp = et.get("goal_patterns", {})
    if gp:
        final_text = ""
        for entry in reversed(trace):
            if entry.get("text"):
                final_text = entry["text"]
                break

        for pat in gp.get("required_patterns", []):
            m = re.search(pat, final_text, re.MULTILINE | re.IGNORECASE)
            passed = m is not None
            checks.append(
                (
                    "goal_pat",
                    passed,
                    f"/{pat[:45]}/: {'matched ✓' if passed else 'not found ✗'}",
                )
            )

        for s in gp.get("required_strings", []):
            passed = s.lower() in final_text.lower()
            checks.append(
                (
                    "goal_str",
                    passed,
                    f"{s!r}: {'found ✓' if passed else 'missing ✗'}",
                )
            )

        req_all = gp.get("required_all_strings", [])
        if req_all:
            found = [
                s for s in req_all if s.lower() in final_text.lower()
            ]
            passed = len(found) == len(req_all)
            checks.append(
                (
                    "goal_all",
                    passed,
                    f"all of {req_all}: {'✓' if passed else f'only {found} ✗'}",
                )
            )

    failed = [
        (name, detail) for name, passed, detail in checks if not passed
    ]
    if failed:
        reason = "; ".join(
            f"{n}: {d}" for n, d in failed[:5]
        )
        if len(failed) > 5:
            reason += f" (+{len(failed) - 5} more)"
        return "FAIL", reason, checks
    return "PASS", f"all {len(checks)} checks passed", checks


# ── Scenario benchmark — per‑model runner ─────────────────────────────────────


def run_scenarios_benchmark(
    host: str,
    port: int,
    token: str,
    model_id: str,
    runtime_name: str,
    scenarios: list[dict],
    W: int,
    bench_thinking: bool = True,
    use_ssl: bool = False,
    base_path: str = "",
) -> list[dict]:
    """Run all scenarios against one model and return result list."""
    total = len(scenarios)
    pad = len(str(total))
    print(f"\n{'─' * W}")
    print(f"  Scenarios — {total} multi-turn dynamic scenarios")

    results: list[dict] = []
    for i, sc in enumerate(scenarios, 1):
        sid = sc["id"]
        label = f"[{i:{pad}}/{total}] {sid}"
        print(f"    {label:<52}", end="", flush=True)
        try:
            trace, verdict, reason, checks, elapsed_ms = run_scenario(
                host, port, token, model_id, runtime_name, sc,
                bench_thinking=bench_thinking,
                use_ssl=use_ssl, base_path=base_path,
            )
            sym = {"PASS": "✓", "FAIL": "✗"}.get(verdict, "?")
            turns = len(trace)
            tools = []
            for entry in trace:
                tools.extend(
                    tc["function"]["name"]
                    for tc in entry.get("tool_calls", [])
                )
            tools_str = (
                "→".join(tools[:4]) + ("…" if len(tools) > 4 else "")
                if tools
                else "no tools"
            )
            print(
                f"{sym} {verdict:<4}  {turns}t  {elapsed_ms:.0f}ms"
                f"  [{tools_str}]"
            )
            results.append(
                {
                    "id": sid,
                    "group": sc.get("group", "scenarios"),
                    "verdict": verdict,
                    "reason": reason,
                    "turns": turns,
                    "max_turns": sc.get("max_turns", 10),
                    "tools_used": tools,
                    "checks": checks,
                    "trace_texts": [
                        e.get("text", "") for e in trace
                    ],
                    "latency_ms": elapsed_ms,
                }
            )
        except Exception as e:
            print(f"ERROR  ({e})")
            results.append(
                {
                    "id": sid,
                    "group": sc.get("group", "scenarios"),
                    "verdict": "ERROR",
                    "reason": str(e)[:120],
                    "turns": 0,
                    "max_turns": sc.get("max_turns", 10),
                    "tools_used": [],
                    "checks": [],
                    "trace_texts": [],
                    "latency_ms": 0,
                }
            )

    passing = sum(1 for r in results if r["verdict"] == "PASS")
    lats = [r["latency_ms"] for r in results if r["latency_ms"]]
    avg_lat = mean(lats) if lats else 0.0
    print(
        f"\n  Scenario score: {passing}/{total}"
        f"  ({passing * 100 // total if total else 0}%)"
        f"   avg latency: {avg_lat:.0f}ms"
    )
    return results


# ── Scenario results display ──────────────────────────────────────────────────


def print_scenarios_results(all_results: list, W: int):
    has_scenarios = any("scenarios" in r for r in all_results)
    if not has_scenarios:
        return

    print()
    print("═" * W)
    print("  SCENARIO RESULTS  (multi-turn dynamic, no LLM evaluator)")
    print("═" * W)
    print()

    col_model = 50
    col_score = 10
    col_lat = 10

    hdr = (
        f"  {'Server/Runtime/Model':<{col_model}}"
        f"  {'Score':>{col_score}}"
        f"  {'Avg lat':>{col_lat}}"
    )
    print(hdr)
    print("  " + "─" * (len(hdr) - 2))

    for r in all_results:
        sc_results = r.get("scenarios", [])
        if not sc_results:
            continue
        total = len(sc_results)
        passing = sum(1 for sr in sc_results if sr["verdict"] == "PASS")
        pct = passing * 100 // total if total else 0
        lats = [
            sr["latency_ms"]
            for sr in sc_results
            if sr["latency_ms"]
        ]
        avg_lat_ms = mean(lats) if lats else float("nan")
        display = (
            f"{r.get('server','?')}/{r.get('runtime','?')}"
            f"  {r.get('model_id','?')}"
        )
        print(
            f"  {display:<{col_model}}"
            f"  {passing}/{total} ({pct:3}%)"
            f"  {avg_lat_ms:>{col_lat}.0f}ms"
        )

    # Failure detail
    print()
    print("  Failed scenarios:")
    print("  " + "─" * 40)
    any_failure = False
    for r in all_results:
        sc_results = r.get("scenarios", [])
        failures = [sr for sr in sc_results if sr["verdict"] == "FAIL"]
        if not failures:
            continue
        any_failure = True
        display = (
            f"{r.get('server','?')}/{r.get('runtime','?')}"
            f"  {r.get('model_id','?')}"
        )
        print(f"\n  {display}")
        for sr in failures:
            tools_used = "→".join(sr["tools_used"][:4]) or "none"
            print(
                f"    ✗ {sr['id']:<42}"
                f"  {sr['turns']}/{sr['max_turns']} turns"
                f"  [{tools_used}]"
            )
            # Show reason in next line for readability
            print(f"      {sr['reason'][:100]}")
    if not any_failure:
        print("  All models passed all scenarios  ✓")


# ── Main ──────────────────────────────────────────────────────────────────────


def main():
    script_dir = os.path.dirname(os.path.abspath(__file__))
    config_path = (
        sys.argv[1] if len(sys.argv) > 1 else os.path.join(script_dir, "config.yaml")
    )
    fixtures_path = os.path.join(
        os.path.dirname(os.path.abspath(config_path)), "fixtures.json"
    )
    scenarios_path = os.path.join(
        os.path.dirname(os.path.abspath(config_path)), "scenarios.json"
    )

    if not os.path.isfile(config_path):
        sys.exit(f"Error: config file not found: {config_path}")

    targets = load_config(config_path)
    if not targets:
        sys.exit("Error: no servers/runtimes defined in config.yaml")

    total_models = sum(len(t["models"]) for t in targets)

    # Quality benchmark setup
    fixtures: list[dict] = []
    if RUN_QUALITY and os.path.isfile(fixtures_path):
        fixtures = load_fixtures(fixtures_path)

    # Scenario benchmark setup
    scenarios: list[dict] = []
    if RUN_QUALITY and os.path.isfile(scenarios_path):
        scenarios = load_scenarios(scenarios_path)

    ev_cfg: dict | None = None
    if RUN_QUALITY and fixtures:
        ev_cfg = load_evaluator_config(config_path)

    W = 82
    print()
    print("═" * W)
    print(f"  Multi-Server Benchmark  (speed + quality + scenarios)")
    print("═" * W)
    print(f"  Config      : {config_path}")
    print(f"  Targets     : {len(targets)} runtime(s)   {total_models} model(s) total")
    print(f"  Speed       : {len(HISTORY)} turns → warm KV cache → new question")
    print(f"              : {GEN_TOKENS} tokens/response   {TRIALS} hot trials")
    print(
        f"  Thinking    : {'ON  (enable_thinking + strip <think>)' if ENABLE_THINKING else 'OFF (disable flags + strip leaks)'}"
    )
    print(
        f"  Quality     : {len(fixtures)} fixtures"
        if fixtures
        else "  Quality     : disabled"
    )
    print(
        f"  Scenarios   : {len(scenarios)} dynamic multi-turn scenarios"
        if scenarios
        else "  Scenarios   : disabled"
    )
    if ev_cfg:
        ev_ready = _wait_ready(
            ev_cfg["host"], ev_cfg["port"], ev_cfg["token"], timeout=3,
            use_ssl=ev_cfg.get("ssl", False), base_path=ev_cfg.get("base_path", "")
        )
        ev_status = f"{ev_cfg['model']}  @ {ev_cfg['host']}:{ev_cfg['port']}"
        ev_status += (
            "  ✓"
            if ev_ready
            else "  ✗ unreachable (will fall back to mechanical checks)"
        )
        print(f"  Evaluator   : {ev_status}")
    elif fixtures:
        print(f"  Evaluator   : not configured — mechanical checks only")
    print(f"  Est. prompt : ~{ESTIMATED_PROMPT_TOKENS} tokens")

    all_results: list[dict] = []

    for tgt in targets:
        server_name = tgt["server"]
        runtime_name = tgt["runtime"]
        host = tgt["host"]
        port = tgt["port"]
        token = tgt["token"]
        use_ssl = tgt.get("ssl", False)
        base_path = tgt.get("base_path", "")
        bench_thinking = tgt.get("bench_thinking", True)
        models = tgt["models"]
        prefix = f"{server_name}/{runtime_name}"

        print()
        print("═" * W)
        print(f"  Server: {server_name}   Runtime: {runtime_name}   ({host}:{port})")
        if use_ssl:
            print(f"  SSL: enabled   Base path: {base_path or '(none)'}")
        print("═" * W)

        if not _wait_ready(host, port, token, timeout=5, use_ssl=use_ssl, base_path=base_path):
            print(f"  SKIP — {host}:{port} not reachable.")
            continue

        available = _get_models(host, port, token, use_ssl=use_ssl, base_path=base_path)
        print(f"  Available models ({len(available)}):")
        for mid in available:
            print(f"    • {mid}")

        for i, target_model in enumerate(models, 1):
            model_id = match_model_id(available, target_model) or target_model
            label = f"{prefix}  {target_model}"

            print(f"\n{'─' * W}")
            print(f"  [{i}/{len(models)}] {target_model}")
            if model_id != target_model:
                print(f"  Matched model ID: {model_id}")

            # ── Speed benchmark ──────────────────────────────────────────────
            result = run_speed_benchmark(
                host, port, token, model_id, runtime_name, label, W, bench_thinking=bench_thinking, use_ssl=use_ssl, base_path=base_path
            )
            if not result:
                continue

            result["server"] = server_name
            result["runtime"] = runtime_name

            # ── Quality benchmark (model still loaded from speed benchmark) ──
            if fixtures:
                result["quality"] = run_quality_benchmark(
                    host,
                    port,
                    token,
                    model_id,
                    runtime_name,
                    fixtures,
                    ev_cfg,
                    W,
                    bench_thinking=bench_thinking,
                    use_ssl=use_ssl,
                    base_path=base_path,
                )

            # ── Scenario benchmark (model still loaded) ────────────────────
            if scenarios:
                result["scenarios"] = run_scenarios_benchmark(
                    host,
                    port,
                    token,
                    model_id,
                    runtime_name,
                    scenarios,
                    W,
                    bench_thinking=bench_thinking,
                    use_ssl=use_ssl,
                    base_path=base_path,
                )

            all_results.append(result)

    # ── Final results ─────────────────────────────────────────────────────────
    print_speed_results(all_results, W)
    print_quality_results(all_results, W)
    print_scenarios_results(all_results, W)

    print()
    print("═" * W)
    print()


if __name__ == "__main__":
    main()
