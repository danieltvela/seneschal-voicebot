#!/usr/bin/env python3
"""
test-tools-dynamic.py — Tool-overhead benchmark for LLM models

For each model in config.yaml, runs two types of probe requests:
  1. Bare prompt (no tools)     — ~128 tokens of user text
  2. Full-tools prompt           — same text + all Seneschal tool definitions
  
Measures TTFT (ms), TG (tokens/sec), PP (prompt t/s) for each variant,
computing the overhead incurred by carrying the full tool catalogue.

Each probe runs BENCH_TRIALS times for statistical stability.

Usage:
  python3 scripts/test-tools-dynamic.py [config.yaml]

  Default config path: scripts/config.yaml (next to this script)

Env vars:
  BENCH_TRIALS    hot measurement trials    (default 3)
  BENCH_GEN       tokens to generate        (default 80)
"""

import http.client
import json
import math
import os
import sys
import time
import yaml
from statistics import mean, stdev

# ═══════════════════════════════════════════════════════════════════════════════
# Constants
# ═══════════════════════════════════════════════════════════════════════════════

TRIALS = int(os.environ.get("BENCH_TRIALS", "3"))
GEN_TOKENS = int(os.environ.get("BENCH_GEN", "80"))

# System prompt that the real Seneschal uses (kept minimal for benchmark purity)
# but the tools test does NOT embed this in the prompt — tools are sent via the
# OpenAI `tools` parameter so the server handles the function-definition routing.

SYSTEM_PROMPT = (
    "Eres un asistente de voz llamado seneschal. Responde de forma concisa y natural, "
    "en español, con 2-3 frases como máximo. No uses markdown ni listas."
)

# ── Test prompt (~128 tokens) ─────────────────────────────────────────────────
# A single-user-turn prompt asking for a concise explanation.
# Approx 128 * 3.5 ≈ 448 chars; the actual token count depends on the model.

TEST_PROMPT = (
    "Explica brevemente qué es el aprendizaje automático, también conocido como "
    "machine learning, mencionando los tres paradigmas principales: aprendizaje "
    "supervisado, aprendizaje no supervisado y aprendizaje por refuerzo. Describe "
    "un ejemplo cotidiano de cada uno de estos paradigmas y explica cómo se "
    "diferencian entre sí. Sé conciso pero completo en tu respuesta."
)

# ── Tool definitions — mirror of src/tools/ (static tools) ────────────────────
# These are the EXACT function-calling definitions used in the Seneschal
# binary. Dynamic tools (NoopTool, McpToolProxy) are excluded because their
# descriptions/parameters depend on runtime configuration.
# RunAgentTool is included with the conventional "run_agent_async" name.

TOOL_DEFINITIONS = [
    # -- apple_events (Calendar + Reminders)
    {
        "type": "function",
        "function": {
            "name": "apple_events",
            "description": (
                "Accesses Apple Calendar and Reminders on macOS via AppleScript. "
                "Use for scheduling, events, appointments, reminders, and tasks."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": [
                            "list_calendars", "list_events", "create_event", "delete_event",
                            "list_reminder_lists", "list_reminders", "create_reminder",
                            "complete_reminder", "delete_reminder",
                        ],
                        "description": "The operation to perform",
                    },
                    "calendar": {
                        "type": "string",
                        "description": "Calendar name (e.g. 'Work', 'Home', 'Calendar')",
                    },
                    "title": {
                        "type": "string",
                        "description": "Event or reminder title",
                    },
                    "start": {
                        "type": "string",
                        "description": "Start date/time in ISO 8601, e.g. '2024-01-01T10:00:00'",
                    },
                    "end": {
                        "type": "string",
                        "description": "End date/time in ISO 8601",
                    },
                    "location": {
                        "type": "string",
                        "description": "Event location (optional)",
                    },
                    "notes": {
                        "type": "string",
                        "description": "Notes or description (optional)",
                    },
                    "list": {
                        "type": "string",
                        "description": "Reminders list name (e.g. 'Work', 'Shopping') or smart folder: 'Today', 'Scheduled', 'Flagged', 'All'",
                    },
                    "from": {
                        "type": "string",
                        "description": "Start of date range for listing events (ISO 8601)",
                    },
                    "to": {
                        "type": "string",
                        "description": "End of date range for listing events (ISO 8601)",
                    },
                    "due_date": {
                        "type": "string",
                        "description": "Due date in ISO 8601 (for reminders)",
                    },
                    "show_completed": {
                        "type": "boolean",
                        "description": "If true, list_reminders includes completed reminders (default: false, only incomplete)",
                    },
                },
                "required": ["operation"],
            },
        },
    },
    # -- read_clipboard
    {
        "type": "function",
        "function": {
            "name": "read_clipboard",
            "description": (
                "Returns the current text content of the clipboard. "
                "Use when the user says 'lo que tengo copiado', 'el portapapeles', "
                "'lo que acabo de copiar', or similar."
            ),
            "parameters": {"type": "object", "properties": {}},
        },
    },
    # -- set_clipboard
    {
        "type": "function",
        "function": {
            "name": "set_clipboard",
            "description": (
                "Writes the given text to the clipboard. "
                "Use when the user asks to copy something, save something to "
                "the clipboard, or 'pon esto en el portapapeles'."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "The text to copy to the clipboard.",
                    }
                },
                "required": ["text"],
            },
        },
    },
    # -- set_conversation_mode
    {
        "type": "function",
        "function": {
            "name": "set_conversation_mode",
            "description": (
                "Cambia el modo de escucha del asistente entre Active y Ambient. "
                "mode='ambient': silencio, modo espera, duerme, go to sleep. "
                "mode='active': conversación, despierta, wake up. "
                "SIEMPRE llama a esta herramienta inmediatamente — nunca simules el cambio."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "mode": {
                        "type": "string",
                        "enum": ["active", "ambient"],
                        "description": "'ambient' to go quiet, 'active' to resume normal listening",
                    }
                },
                "required": ["mode"],
            },
        },
    },
    # -- current_time
    {
        "type": "function",
        "function": {
            "name": "current_time",
            "description": (
                "Returns the current local date and time. MUST be called EVERY TIME "
                "the user explicitly asks for the current time, date, day or hour. "
                "Do not answer from memory, cached context or general knowledge; "
                "always call this tool."
            ),
            "parameters": {"type": "object", "properties": {}},
        },
    },
    # -- deep_research
    {
        "type": "function",
        "function": {
            "name": "deep_research",
            "description": (
                "Investigación profunda y síntesis compleja de información. "
                "Úsala cuando el usuario pida: análisis comparativo, investigación "
                "exhaustiva, resumen de múltiples fuentes, informes detallados, "
                "o tareas que requieran razonamiento extendido con acceso a herramientas. "
                "NO la uses para consultas factuales simples (usa quick_search). "
                "Esta herramienta tarda más pero da resultados más completos."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The research query or task description",
                    }
                },
                "required": ["query"],
            },
        },
    },
    # -- open_app
    {
        "type": "function",
        "function": {
            "name": "open_app",
            "description": (
                "Opens a macOS application by name. Use when the user asks to open, "
                "launch, or start an application. Examples: 'abre Cursor', "
                "'lanza Safari', 'abre la terminal'."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The application name as it appears in /Applications, e.g. 'Cursor', 'Safari', 'Terminal', 'Finder'.",
                    }
                },
                "required": ["name"],
            },
        },
    },
    # -- open_terminal
    {
        "type": "function",
        "function": {
            "name": "open_terminal",
            "description": "Abre OpenCode en una terminal para que el usuario vea el progreso",
            "parameters": {"type": "object", "properties": {}},
        },
    },
    # -- set_prompt_build
    {
        "type": "function",
        "function": {
            "name": "set_prompt_build",
            "description": (
                "Control the prompt-build mode. Actions: start (activate mode), "
                "update (replace the prompt text with a new version), cancel "
                "(deactivate mode). While active, all user messages are instructions "
                "to modify the prompt — call update after each refinement. "
                "IMPORTANT: Always call cancel after the prompt has been saved, "
                "copied, sent to another tool/agent, or otherwise dispatched."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["start", "update", "cancel"],
                        "description": "The action to perform: start activates the mode, update modifies the prompt text, cancel deactivates the mode",
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The prompt text (required for 'update' action, ignored for 'start' and 'cancel')",
                    },
                },
                "required": ["action"],
                "additionalProperties": False,
            },
        },
    },
    # -- quick_search
    {
        "type": "function",
        "function": {
            "name": "quick_search",
            "description": (
                "Búsqueda web rápida para consultas factuales cortas. "
                "Úsala cuando el usuario pregunte por información actual, "
                "noticias, eventos recientes, datos concretos, definiciones, "
                "o cualquier cosa que se pueda responder con una búsqueda simple. "
                "NO la uses para investigación profunda o síntesis compleja "
                "(usa deep_research para eso). Respuesta en 1-3 segundos."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "The search query"},
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default 5)",
                    },
                },
                "required": ["query"],
            },
        },
    },
    # -- read_file
    {
        "type": "function",
        "function": {
            "name": "read_file",
            "description": (
                "Reads the text content of a file at the given path and returns it. "
                "Use when the user asks to read, show, check, or review a file. "
                "Output is capped at 16 KB; binary files are rejected."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute or home-relative (~) path to the file.",
                    }
                },
                "required": ["path"],
            },
        },
    },
    # -- recover_historical_context
    {
        "type": "function",
        "function": {
            "name": "recover_historical_context",
            "description": (
                "Busca mensajes históricos en el archivo L2 (conversaciones antiguas consolidadas). "
                "Útil cuando el usuario pregunta sobre algo que se habló en el pasado lejano. "
                "Recibe un texto de búsqueda y opcionalmente un límite de resultados y "
                "un session_id para acotar la búsqueda a una sesión específica."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Texto de búsqueda en el archivo histórico",
                    },
                    "session_id": {
                        "type": "string",
                        "description": "Opcional: ID de sesión para acotar la búsqueda",
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Número máximo de resultados (por defecto 10)",
                    },
                },
                "required": ["query"],
            },
        },
    },
    # -- run_agent (conventional name "run_agent_async")
    {
        "type": "function",
        "function": {
            "name": "run_agent_async",
            "description": (
                "Delega una tarea al agente externo. Consulta la sección AGENTES EXTERNOS DISPONIBLES "
                "del system prompt para saber qué agentes están disponibles y cuándo usar cada uno. "
                "IMPORTANTE: DEBES llamar a esta función para delegar tareas. Nunca describas "
                "verbalmente que 'has enviado al agente' sin haber llamado primero a run_agent. "
                "El resultado llega de forma proactiva cuando el agente termina. "
                "Para cancelar una tarea en curso usa task='cancel'. "
                "Para consultar el estado usa task='status'."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "Descripción breve de la tarea a delegar, o 'cancel' para cancelar la tarea en curso, o 'status' para consultar el estado.",
                    }
                },
                "required": ["task"],
            },
        },
    },
    # -- run_shell
    {
        "type": "function",
        "function": {
            "name": "run_shell",
            "description": (
                "Execute a shell command and return its output (stdout + stderr + exit code). "
                "Use for compiling code, reading files, searching the filesystem, checking "
                "system state, running scripts, git operations, etc. "
                "Always say what you are about to do before calling this tool. "
                "Do NOT run destructive commands (delete, overwrite, format) without "
                "explicit user confirmation."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute",
                    }
                },
                "required": ["command"],
            },
        },
    },
    # -- list_tasks
    {
        "type": "function",
        "function": {
            "name": "list_tasks",
            "description": (
                "Lista las tareas en segundo plano que se están ejecutando o que han "
                "terminado recientemente. Úsala cuando el usuario pregunte qué estás "
                "haciendo, el estado de una tarea, o si algo terminó."
            ),
            "parameters": {"type": "object", "properties": {}},
        },
    },
    # -- switch_plugin
    {
        "type": "function",
        "function": {
            "name": "switch_plugin",
            "description": "Activa o cambia el plugin activo. Los plugins disponibles son: ",
            "parameters": {
                "type": "object",
                "properties": {
                    "plugin_name": {
                        "type": "string",
                        "description": "Nombre del plugin a activar",
                    }
                },
                "required": ["plugin_name"],
            },
        },
    },
    # -- take_screenshot
    {
        "type": "function",
        "function": {
            "name": "take_screenshot",
            "description": (
                "Captures the current screen and returns a text description of what is "
                "visible. Use when the user asks about what is on the screen, an "
                "application window, a document, code, or any visual element on the display."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "Optional question or focus for the vision analysis. If omitted, a general description is returned.",
                    }
                },
                "required": [],
            },
        },
    },
    # -- web_search
    {
        "type": "function",
        "function": {
            "name": "web_search",
            "description": (
                "Busca información en internet. Usa esta herramienta cuando el usuario "
                "pregunte sobre información actual, noticias, eventos recientes, datos "
                "que no conoces, o necesites verificar algo. Devuelve los primeros "
                "resultados con título, fragmento y URL."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "The search query"},
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default 5)",
                    },
                },
                "required": ["query"],
            },
        },
    },
]

NUM_TOOLS = len(TOOL_DEFINITIONS)

# ═══════════════════════════════════════════════════════════════════════════════
# Config helpers
# ═══════════════════════════════════════════════════════════════════════════════

def _parse_host(host_url: str) -> str:
    """Strip http:// or https:// prefix for http.client.HTTPConnection."""
    for prefix in ("https://", "http://"):
        if host_url.startswith(prefix):
            return host_url[len(prefix):]
    return host_url


def load_config(path: str) -> list[dict]:
    """Parse config.yaml → flat list of benchmark targets."""
    with open(path) as f:
        cfg = yaml.safe_load(f)
    targets = []
    for server_name, server_cfg in cfg.get("servers", {}).items():
        host = _parse_host(server_cfg.get("host", "http://127.0.0.1"))
        for runtime_name, runtime_cfg in server_cfg.get("runtimes", {}).items():
            models = runtime_cfg.get("models") or []
            if not models:
                continue
            targets.append(
                {
                    "server": server_name,
                    "runtime": runtime_name,
                    "host": host,
                    "port": int(runtime_cfg.get("port", 8000)),
                    "token": runtime_cfg.get("token", ""),
                    "models": models,
                }
            )
    return targets


# ═══════════════════════════════════════════════════════════════════════════════
# HTTP helpers
# ═══════════════════════════════════════════════════════════════════════════════

def _auth_headers(token: str) -> dict:
    h = {"Content-Type": "application/json"}
    if token:
        h["Authorization"] = f"Bearer {token}"
    return h


def _post_stream(host, port, token, payload):
    """POST to /v1/chat/completions with stream=True.  Yields SSE content lines.

    Reads one byte at a time to avoid http.client's chunked-encoding
    accumulation — with many runtimes emitting small SSE chunks, read(4096)
    would buffer several tokens before the first yield, inflating TTFT.
    """
    body = json.dumps(payload).encode()
    conn = http.client.HTTPConnection(host, port, timeout=120)
    try:
        conn.request(
            "POST", "/v1/chat/completions", body=body, headers=_auth_headers(token)
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


def _post_blocking(host, port, token, payload) -> dict:
    """POST to /v1/chat/completions with stream=False. Returns parsed JSON."""
    payload = {**payload, "stream": False}
    body = json.dumps(payload).encode()
    conn = http.client.HTTPConnection(host, port, timeout=120)
    try:
        conn.request(
            "POST", "/v1/chat/completions", body=body, headers=_auth_headers(token)
        )
        resp = conn.getresponse()
        raw = resp.read()
        if resp.status != 200:
            raise RuntimeError(f"HTTP {resp.status}: {raw[:300].decode()}")
        return json.loads(raw)
    finally:
        conn.close()


def _get_models(host, port, token) -> list[str]:
    """Return list of model IDs from /v1/models."""
    conn = http.client.HTTPConnection(host, port, timeout=10)
    try:
        conn.request("GET", "/v1/models", headers=_auth_headers(token))
        resp = conn.getresponse()
        data = json.loads(resp.read())
        return [m["id"] for m in data.get("data", [])]
    except Exception:
        return []
    finally:
        conn.close()


def _wait_ready(host, port, token, timeout=5) -> bool:
    """Return True if the server responds before timeout."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            conn = http.client.HTTPConnection(host, port, timeout=2)
            conn.request("GET", "/v1/models", headers=_auth_headers(token))
            r = conn.getresponse()
            r.read()
            conn.close()
            if r.status < 500:
                return True
        except Exception:
            pass
        time.sleep(1)
    return False


# ═══════════════════════════════════════════════════════════════════════════════
# Model matching
# ═══════════════════════════════════════════════════════════════════════════════

def match_model_id(available: list[str], target: str) -> str | None:
    """Exact match first, then case-insensitive substring."""
    if target in available:
        return target
    target_lower = target.lower()
    for mid in available:
        if target_lower in mid.lower() or mid.lower() in target_lower:
            return mid
    return None


# ═══════════════════════════════════════════════════════════════════════════════
# Payload builders
# ═══════════════════════════════════════════════════════════════════════════════

def _base_messages() -> list[dict]:
    """Returns the conversation messages used for every probe."""
    return [
        {"role": "system", "content": SYSTEM_PROMPT},
        {"role": "user", "content": TEST_PROMPT},
    ]


def _bare_payload(model_id: str, stream: bool) -> dict:
    """Payload WITHOUT tool definitions."""
    return {
        "model": model_id,
        "messages": _base_messages(),
        "max_tokens": GEN_TOKENS,
        "temperature": 0.0,
        "stream": stream,
    }


def _tools_payload(model_id: str, stream: bool) -> dict:
    """Payload WITH the full Seneschal tool catalogue."""
    return {
        "model": model_id,
        "messages": _base_messages(),
        "max_tokens": GEN_TOKENS,
        "temperature": 0.0,
        "stream": stream,
        "tools": TOOL_DEFINITIONS,
        "tool_choice": "auto",
    }


# ═══════════════════════════════════════════════════════════════════════════════
# Single-trial measurement
# ═══════════════════════════════════════════════════════════════════════════════

def measure_one_trial(host, port, token, payload) -> dict:
    """Run one streaming probe and return TTFT, TG, prompt_tokens, generated_tokens."""
    t_start = time.perf_counter()
    t_first = None
    t_last = None
    n_gen_tokens = 0
    prompt_tokens_api = None

    for line in _post_stream(host, port, token, payload):
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
        delta = (chunk.get("choices") or [{}])[0].get("delta", {})
        content = delta.get("content") or ""
        if content:
            now = time.perf_counter()
            if t_first is None:
                t_first = now
            t_last = now
            n_gen_tokens += len(content.split())

    if t_first is None:
        raise RuntimeError("No content token received")

    ttft_ms = (t_first - t_start) * 1000

    # Generation time: from first token to last token
    if t_last and t_last > t_first + 0.001:
        tg_secs = t_last - t_first
        tg_tps = n_gen_tokens / tg_secs
    else:
        tg_tps = float("nan")

    return {
        "ttft_ms": ttft_ms,
        "tg_tps": tg_tps,
        "prompt_tokens": prompt_tokens_api,
        "generated_tokens": n_gen_tokens,
    }


# ═══════════════════════════════════════════════════════════════════════════════
# Per-model runner
# ═══════════════════════════════════════════════════════════════════════════════

def run_tool_benchmark(host, port, token, model_id, server, runtime, W):
    """Run bare + tools trials for one model. Returns combined result dict."""

    label = f"{server}/{runtime}  {model_id}"
    print(f"\n{'─' * W}")
    print(f"  Model: {model_id}  ({server}/{runtime})")

    # ── Warm-up ────────────────────────────────────────────────────────────────
    print(f"  Warming up ...", end=" ", flush=True)
    try:
        _post_blocking(
            host, port, token,
            {
                "model": model_id,
                "messages": [{"role": "user", "content": "Hola"}],
                "max_tokens": 1,
            },
        )
        print("done")
    except Exception as e:
        print(f"FAILED: {e}")
        return None

    # ── Bare trials ────────────────────────────────────────────────────────────
    bare_trials = []
    print(f"\n  ── Bare prompt ({TRIALS} trials) ──")
    for i in range(TRIALS):
        print(f"    [{i+1}/{TRIALS}] ", end="", flush=True)
        try:
            r = measure_one_trial(host, port, token, _bare_payload(model_id, stream=True))
            bare_trials.append(r)
            pt = r["prompt_tokens"] or "?"
            print(f"TTFT {r['ttft_ms']:>7.0f} ms   TG {r['tg_tps']:>5.1f} t/s   (prompt_tok={pt})")
        except Exception as e:
            print(f"FAILED: {e}")

    if not bare_trials:
        print(f"  ERROR: no bare trials succeeded")
        return None

    # ── Full-tools trials ──────────────────────────────────────────────────────
    tools_trials = []
    print(f"\n  ── Full tools ({NUM_TOOLS} tools, {TRIALS} trials) ──")
    for i in range(TRIALS):
        print(f"    [{i+1}/{TRIALS}] ", end="", flush=True)
        try:
            r = measure_one_trial(host, port, token, _tools_payload(model_id, stream=True))
            tools_trials.append(r)
            pt = r["prompt_tokens"] or "?"
            print(f"TTFT {r['ttft_ms']:>7.0f} ms   TG {r['tg_tps']:>5.1f} t/s   (prompt_tok={pt})")
        except Exception as e:
            print(f"FAILED: {e}")

    if not tools_trials:
        print(f"  ERROR: no tools trials succeeded")
        return None

    # ── Aggregate statistics ───────────────────────────────────────────────────
    def aggregate(trials):
        ttfts = [t["ttft_ms"] for t in trials]
        tgs_raw = [t["tg_tps"] for t in trials]
        tgs = [v for v in tgs_raw if not (math.isnan(v) or math.isinf(v))]
        pts = [t["prompt_tokens"] for t in trials if t["prompt_tokens"]]

        avg_ttft = mean(ttfts)
        sd_ttft = stdev(ttfts) if len(ttfts) > 1 else 0.0
        avg_tg = mean(tgs) if tgs else float("nan")
        sd_tg = stdev(tgs) if len(tgs) > 1 else 0.0
        avg_pt = mean(pts) if pts else None

        # PP = prompt_tokens / TTFT_seconds
        pp_vals = []
        for t in trials:
            pt = t["prompt_tokens"]
            ttft_s = t["ttft_ms"] / 1000
            if pt and ttft_s > 0:
                pp_vals.append(pt / ttft_s)
        avg_pp = mean(pp_vals) if pp_vals else float("nan")
        sd_pp = stdev(pp_vals) if len(pp_vals) > 1 else 0.0

        return {
            "ttft_ms": avg_ttft,
            "ttft_sd": sd_ttft,
            "tg_tps": avg_tg,
            "tg_sd": sd_tg,
            "prompt_tokens": avg_pt,
            "pp_tps": avg_pp,
            "pp_sd": sd_pp,
            "n_trials": len(trials),
        }

    bare = aggregate(bare_trials)
    tools = aggregate(tools_trials)

    # ── Overhead ratios ────────────────────────────────────────────────────────
    overhead_ttft = (tools["ttft_ms"] / bare["ttft_ms"]) if bare["ttft_ms"] > 0 else float("nan")
    overhead_pp = (tools["pp_tps"] / bare["pp_tps"]) if bare["pp_tps"] > 0 else float("nan")
    delta_tg = tools["tg_tps"] - bare["tg_tps"]

    print(f"\n  ── Summary ──")
    print(f"    Bare   : TTFT={bare['ttft_ms']:.0f} ±{bare['ttft_sd']:.0f} ms   "
          f"TG={bare['tg_tps']:.1f} t/s   PP={bare['pp_tps']:.0f} t/s   "
          f"prompt_tok≈{bare['prompt_tokens'] or '?'}")
    print(f"    Tools  : TTFT={tools['ttft_ms']:.0f} ±{tools['ttft_sd']:.0f} ms   "
          f"TG={tools['tg_tps']:.1f} t/s   PP={tools['pp_tps']:.0f} t/s   "
          f"prompt_tok≈{tools['prompt_tokens'] or '?'}")
    print(f"    Overhead: TTFT {overhead_ttft:.2f}×   PP {overhead_pp:.2f}×   "
          f"ΔTG {delta_tg:+.1f} t/s")

    return {
        "server": server,
        "runtime": runtime,
        "model_id": model_id,
        "bare": bare,
        "tools": tools,
        "overhead_ttft": overhead_ttft,
        "overhead_pp": overhead_pp,
        "delta_tg": delta_tg,
    }


# ═══════════════════════════════════════════════════════════════════════════════
# Results display
# ═══════════════════════════════════════════════════════════════════════════════

def print_results(all_results: list, W: int):
    print()
    print("═" * W)
    print("  TOOL OVERHEAD RESULTS")
    print("═" * W)
    print()

    col_label = 52
    col_t = 12

    # Header
    hdr = (
        f"  {'Model':<{col_label}}"
        f"  {'TTFT bare':>{col_t}}"
        f"  {'TTFT tools':>{col_t}}"
        f"  {'TTFT ×':>8}"
        f"  {'TG bare':>{col_t}}"
        f"  {'TG tools':>{col_t}}"
        f"  {'ΔTG':>8}"
        f"  {'PP bare':>{col_t}}"
        f"  {'PP tools':>{col_t}}"
        f"  {'PP ×':>8}"
    )
    print(hdr)
    print("  " + "─" * (len(hdr) - 2))

    for r in all_results:
        b = r["bare"]
        t = r["tools"]
        label = f"{r['server']}/{r['runtime']}  {r['model_id']}"
        print(
            f"  {label:<{col_label}}"
            f"  {b['ttft_ms']:>{col_t}.0f}"
            f"  {t['ttft_ms']:>{col_t}.0f}"
            f"  {r['overhead_ttft']:>8.2f}"
            f"  {b['tg_tps']:>{col_t}.1f}"
            f"  {t['tg_tps']:>{col_t}.1f}"
            f"  {r['delta_tg']:>+8.1f}"
            f"  {b['pp_tps']:>{col_t}.0f}"
            f"  {t['pp_tps']:>{col_t}.0f}"
            f"  {r['overhead_pp']:>8.2f}"
        )

    # Rankings
    if len(all_results) >= 2:
        print()
        print("  Rankings  (lower overhead = better)")
        print("  " + "─" * 50)
        by_ttft = sorted(all_results, key=lambda r: r["overhead_ttft"])
        by_pp = sorted(all_results, key=lambda r: r["overhead_pp"], reverse=True)
        by_dtg = sorted(all_results, key=lambda r: r["delta_tg"], reverse=True)

        label_fn = lambda r: f"{r['server']}/{r['runtime']} {r['model_id']}"

        print(f"  Lowest TTFT overhead : {label_fn(by_ttft[0])}  ({by_ttft[0]['overhead_ttft']:.2f}×)")
        print(f"  Best PP with tools   : {label_fn(by_pp[0])}  ({by_pp[0]['overhead_pp']:.2f}×)")
        print(f"  Least TG degradation : {label_fn(by_dtg[0])}  ({by_dtg[0]['delta_tg']:+.1f} t/s)")

        # Highlight worst
        print(f"  Highest TTFT overhead: {label_fn(by_ttft[-1])}  ({by_ttft[-1]['overhead_ttft']:.2f}×)")


# ═══════════════════════════════════════════════════════════════════════════════
# main
# ═══════════════════════════════════════════════════════════════════════════════

def main():
    script_dir = os.path.dirname(os.path.abspath(__file__))
    config_path = (
        sys.argv[1] if len(sys.argv) > 1 else os.path.join(script_dir, "config.yaml")
    )

    if not os.path.isfile(config_path):
        sys.exit(f"Error: config file not found: {config_path}")

    targets = load_config(config_path)
    if not targets:
        sys.exit("Error: no servers/runtimes with models defined in config.yaml")

    total_models = sum(len(t["models"]) for t in targets)

    W = 105
    print()
    print("═" * W)
    print("  test-tools-dynamic.py — Tool Overhead Benchmark")
    print("═" * W)
    print(f"  Config      : {config_path}")
    print(f"  Targets     : {len(targets)} runtime(s)   {total_models} model(s) total")
    print(f"  Tools       : {NUM_TOOLS} tools loaded from src/tools/ definitions")
    print(f"  Trials      : {TRIALS} per variant")
    print(f"  Gen tokens  : {GEN_TOKENS}")
    print(f"  Test prompt : {len(TEST_PROMPT)} chars (~{max(1, int(len(TEST_PROMPT) / 3.5))} tokens)")
    print()
    print(f"  Each model runs {TRIALS}× bare + {TRIALS}× full-tools = {TRIALS * 2} probes")
    print()

    all_results = []

    for tgt in targets:
        server_name = tgt["server"]
        runtime_name = tgt["runtime"]
        host = tgt["host"]
        port = tgt["port"]
        token = tgt["token"]
        models = tgt["models"]

        print()
        print("═" * W)
        print(f"  Server: {server_name}   Runtime: {runtime_name}   ({host}:{port})")
        print("═" * W)

        if not _wait_ready(host, port, token, timeout=5):
            print(f"  SKIP — {host}:{port} not reachable.")
            continue

        available = _get_models(host, port, token)
        print(f"  Available models ({len(available)}):")
        for mid in available:
            print(f"    • {mid}")

        for i, target_model in enumerate(models, 1):
            model_id = match_model_id(available, target_model) or target_model

            print(f"\n{'─' * W}")
            print(f"  [{i}/{len(models)}] {target_model}")
            if model_id != target_model:
                print(f"  Matched model ID: {model_id}")

            result = run_tool_benchmark(
                host, port, token, model_id, server_name, runtime_name, W
            )
            if result:
                all_results.append(result)

    # ── Final results ─────────────────────────────────────────────────────────
    if all_results:
        print_results(all_results, W)
    else:
        print("\n  No results collected.")

    print()
    print("═" * W)
    print()


if __name__ == "__main__":
    main()