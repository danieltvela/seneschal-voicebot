# LLM Server Providers — Apple Silicon

> **Date:** July 2026
> **Goal:** choose the inference backend (server) with the lowest TTFT for Seneschal on an Apple Silicon Mac (M4 / M5), respecting the project's API requirements.
> **Scope:** comparison of LLM server runtimes (mlx-lm, llama.cpp, vllm-mlx, Ollama, MLC-LLM). Does not cover model selection or Rust client configuration.
>
> **Complements:** [llm-provider.md](llm-provider.md) (`LlmProvider` trait in Rust) and [LLM-requirements.md](LLM-requirements.md) (model requirements and quality benchmarks).

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Requirements That Drive Server Choice](#2-requirements-that-drive-server-choice)
3. [Server Comparison](#3-server-comparison)
4. [Hardware-Conditional Recommendation](#4-hardware-conditional-recommendation)
5. [Discarded Options and Why](#5-discarded-options-and-why)
6. [Micro-Benchmark Plan](#6-micro-benchmark-plan)
7. [References](#7-references)

---

## 1. Executive Summary

Seneschal is a single-user voice assistant with a streaming **STT → LLM → TTS** pipeline. The LLM runs locally on an Apple Silicon Mac and communicates via an OpenAI-compatible API with SSE. Latency (TTFT) is critical for the voice experience.

**Primary recommendation for M4 and earlier:** `llama-server` (llama.cpp, Metal backend) with GGUF model. Faster prefill on Q4 8B-class, mature OpenAI tool-calling, automatic prefix cache.

**Recommendation for M5 with ≥32 GB:** `vllm-mlx` (excps fork) with `--tool-call-parser`. The M5's Neural Accelerators give MLX a ~4× TTFT advantage, and the persistent prompt cache reduces multi-turn TTFT by 10–30×.

**The current server (official mlx-lm) has critical limitations** in tool-calling and prefix cache that make it unreliable for this project.

---

## 2. Requirements That Drive Server Choice

### 2.1 Must-Haves

| Requirement | Detail | Impact if not met |
|-------------|--------|-------------------|
| **OpenAI-compatible SSE** | `POST /v1/chat/completions` with `stream: true`, format `data: {choices:[...]}\n\n`, termination `data: [DONE]` | Incompatible with the Rust client |
| **Native tool-calling** | `delta.tool_calls[]` with `function.name` + `function.arguments` in SSE, `finish_reason: "tool_calls"` | *Complex* turns break — the assistant cannot act |
| **Message roles** | `system`, `user`, `assistant`, `tool` (with `tool_call_id`) | Invalid multi-turn history |
| **Per-request sampling** | `temperature`, `top_p`, `top_k`, `repetition_penalty` | Unpredictable behavior |
| **Streaming + non-streaming** | Stream for conversation, non-stream for summarization/extraction | Broken pipeline |

**Code references:**
- SSE parsing: `src/llm/client.rs:245-364` (stream), `src/llm/client.rs:370-406` (complete)
- Tool definitions: `src/tools/mod.rs:131-154`
- System prompt: `src/pipeline/consolidation.rs:71-102`

### 2.2 Critical for Latency

| Factor | Why it matters | Lever |
|--------|---------------|-------|
| **Prefix cache** (stable system prompt) | The system prompt (~4-8K tokens) does not change between turns. With prefix cache, only the first turn precomputes it. | Reduces TTFT from turn 2+ by 3–30× |
| **Prefill rate** | TTFT = prefill + decode to first token. System prompt prefill dominates TTFT on the first turn. | >200 tok/s on M4/M5 |
| **Decode rate** | Short generations (`max_tokens=400`), 1 request at a time. | >30 tok/s for smooth streaming |
| **Single-stream** | Single-user → continuous batching adds little. Single-stream latency matters more than throughput. | — |

### 2.3 Nice-to-Have

| Factor | Detail |
|--------|--------|
| `chat_template_kwargs` | For Qwen3 thinking control. The client already sends it (`client.rs:237`) but it's not critical: `ThinkFilter` strips `<think>` client-side regardless. |
| Multimodal (`image_url`) | Required by `take_screenshot` and vision tools. Uses the primary LLM provider. |
| `repetition_penalty` per request | The current mlx-lm requires it; the client always sends it. |
| Optional Bearer auth | For remote servers. Not used locally. |

---

## 3. Server Comparison

### 3.1 Comparison Table (Apple Silicon, July 2026)

| Server | Runtime | Prefill / TTFT | Decode | OpenAI Tool-Calling | Multi-turn Prefix Cache | OpenAI SSE | Notes |
|--------|---------|---------------|--------|---------------------|-------------------------|------------|-------|
| **mlx-lm official** (current) | MLX (ANE on M5) | Good; M5: ~4× TTFT via Neural Accelerators vs Metal | Very high (<14B: +20–87% vs llama.cpp) | ❌ Weak. The team acknowledges: "fast moving target... aims to support common cases". Reported Qwen3 tool-calling failures. | Only basic "rotating cache", no automatic reuse between requests. | ✅ | **Current project server.** Example: `mlx_lm.server --model google/gemma-4-26b-a4b` |
| **llama-server** (llama.cpp, Metal) | llama.cpp (Metal + CPU) | Faster prefill than MLX on M4: 1,420 vs 1,180 tok/s (8B-Q4). M5: MLX leads via ANE. | High; MTP/speculative wins on MoE (~100 tok/s with Qwen3.6-35B-A3B vs MLX 85). | ✅ Mature, battle-tested. Works with Gemma, Qwen, Llama. | ✅ `--cache-prompt` on by default (same slot reuses KV for identical prefixes). | ✅ | **Recommended for M4 and earlier.** GGUF (unsloth Q4_K_M). MTP/speculative decoding support. |
| **Ollama** | MLX (since 03/2026 on Mac) | +57% prefill vs old llama.cpp in Ollama. TTFT degrades significantly at >100K context. | ~112 tok/s (2× vs before). Requires ≥32 GB for MLX path. | ✅ Acceptable for common cases. | ✅ By default. | ✅ | Convenience layer over llama.cpp/MLX. Less fine control. Weak TTFT at long context. |
| **MLC-LLM** | Apache TVM (Metal) | Lowest TTFT on prompts ≤16K (arXiv study, M2 Ultra). | ~190 tok/s. | ⚠️ Less proven. Partial support via chat template. | Paged KV (memory-efficient). | ✅ | MLC weight format (limited). Tool-calling poorly documented. Compatibility risk. |
| **vllm-mlx** (excps fork) | MLX (fork of vllm-mlx) | Strong (MLX). Persistent prompt cache reduces multi-turn TTFT 10–30×. | 43–74 tok/s without draft (M3 Ultra); faster with `--draft-model`. | ✅ 17 tool-calling parsers (hermès, llama3_json, mistral, etc.) + auto-recovery of degraded tool calls. | ✅ Always on. Hash-based prefix cache persistent between requests. | ✅ | **Recommended for M5.** Speculative decoding via external draft model (`--draft-model`), not MTP. |

### 3.2 Detail per Server

#### mlx-lm (official) — the current server

- **Advantages:** Native MLX on Apple Silicon, Neural Accelerators support (M5), native `chat_template_kwargs`, MLX weight format (models on HuggingFace `mlx-community`).
- **Critical limitations for this project:**
  1. **Tool-calling:** the MLX team acknowledges that "tool calling is a fast moving target and the server aims to support common cases". Users report Qwen3 tool-calling failures compared to GGUF/llama.cpp. The project depends on `delta.tool_calls` + `finish_reason: "tool_calls"` — if it fails, *Complex* turns break.
  2. **Prefix cache:** only offers a "rotating cache" with some reuse, not automatic prefix caching between requests like llama.cpp or vllm-mlx. The system prompt (~4-8K tokens) is re-prefilled every turn.
  3. **No speculative decoding** (before mlx-lm 0.21, May 2026). Version 0.21 added production speculative decoding, but without native MTP (multi-token prediction) like llama.cpp.
- **When to use it:** only for quick prototyping or when tool-calling is not critical. For this project, **not recommended** in production.

#### llama-server (llama.cpp, Metal backend)

- **Advantages:**
  1. **Mature tool-calling:** OpenAI function calling, battle-tested, compatible with Gemma, Qwen, Llama. Supports `tool_choice`, `tool_calls` in SSE, `finish_reason: "tool_calls"`.
  2. **Automatic prefix cache:** `--cache-prompt` (on by default) reuses the KV cache of the system prompt between turns. The first request precomputes the prompt; subsequent ones only add the new user message.
  3. **MTP/speculative decoding:** GGUF models with MTP (e.g. `unsloth/gemma-4-26b-a4b-MTP-GGUF`) use multi-token prediction to accelerate decode. On MoE (-A4B/A3B), the gain is ~+12% in decode.
  4. **Fast prefill on M4:** Contra Collective benchmarks (2026) show 1,420 tok/s on 8B-Q4 with Metal vs 1,180 tok/s on MLX, thanks to Metal prefill optimizations.
  5. **Model availability:** GGUF is the most widespread format. Gemma-4, Qwen3.5/3.6, Llama 3.x, DeepSeek-V4-Flash — all available in Q4_K_M/Q5_K_M via unsloth, bartowski, lmstudio-community.
- **Disadvantages:**
  1. On M5, MLX's Neural Accelerator gives ~4× TTFT advantage that Metal cannot match.
  2. `chat_template_kwargs` is ignored (llama.cpp uses `--reasoning-budget` and the GGUF's Jinja2 chat template). Not critical: `ThinkFilter` strips `<think>` client-side.
- **Base command (without speculative decoding):**
  ```bash
  llama-server -m gemma-4-26b-a4b-Q4_K_M.gguf \
    --host 127.0.0.1 --port 8000 \
    --ctx-size 8192 --threads 8 --n-gpu-layers 99 \
    --cache-prompt --load-mode mmap+mlock
  ```

  **With MTP speculative decoding (MTP-GGUF models such as unsloth):**
  ```bash
  llama-server -m gemma-4-26b-a4b-MTP-Q4_K_M.gguf \
    --host 127.0.0.1 --port 8000 \
    --ctx-size 8192 --threads 8 --n-gpu-layers 99 \
    --cache-prompt --load-mode mmap+mlock \
    --spec-type draft-mtp --spec-draft-n-max 1
  ```
  - `--spec-type draft-mtp` enables the MTP heads baked into the GGUF (no separate draft model needed).
  - `--spec-draft-n-max 1` limits to 1 draft token per step (typical value for MTP; adjustable).
  - Without `--spec-type`, the default is `none` → **no speculative decoding**.

#### vllm-mlx (excps fork)

- **Advantages:**
  1. **Native MLX** with all Apple Silicon benefits (Neural Accelerators on M5, fused operations, efficient 4-bit quantization).
  2. **Persistent hash-based prefix cache:** the biggest multi-turn TTFT lever. Always on in SimpleEngine (default mode) — no flags needed. The system prompt is cached by hash; subsequent turns skip full prefill. **10–30× faster TTFT from turn 2+**.
  3. **Tool-calling with recovery:** 17 tool-calling parsers (hermès, llama3_json, mistral, functionary, etc.) + auto-recovery of malformed tool calls. If the model outputs invalid JSON, the server repairs it.
  4. **Continuous batching:** though single-user, helps overlap prefill and decode for chained tool calls.
  5. **KV cache quantization:** reduces memory, allows larger models.
  6. **Speculative decoding with draft model:** `--draft-model` loading a small independent model (≠ baked-in MTP). Draft token control with `--num-draft-tokens` (default 4).
- **Disadvantages:**
  1. Community project (forks of `waybarrios/vllm-mlx` and `excps/vllm-mlx`). Less battle-tested than llama.cpp.
  2. Slower decode than llama.cpp on M4 without draft (43–74 tok/s on M3 Ultra). With `--draft-model` the gap narrows.
  3. Requires MLX-format weights (available on `mlx-community`).
- **Base command (without speculative decoding):**
  ```bash
  vllm-mlx serve mlx-community/gemma-4-26b-a4b-4bit \
    --host 127.0.0.1 --port 8000 \
    --tool-call-parser hermes
  ```
  Prompt cache is always on in SimpleEngine (default mode).

  **With speculative decoding (external draft model):**
  ```bash
  vllm-mlx serve mlx-community/gemma-4-26b-a4b-4bit \
    --host 127.0.0.1 --port 8000 \
    --tool-call-parser hermes \
    --draft-model mlx-community/gemma-4-1b-4bit \
    --num-draft-tokens 4
  ```
  - `--draft-model` loads a small independent model as a draft predictor (≠ MTP).
  - `--num-draft-tokens` controls how many speculative tokens are generated per step (default 4).
  - The draft model must share the same tokenizer and vocabulary as the main model.

#### Ollama (MLX mode)

- **Advantages:** easy to install, simple interface, MLX since March 2026, good for prototyping.
- **Disadvantages:**
  1. Less control over sampling, cache, and tool-calling.
  2. TTFT degrades significantly at >100K context (arXiv study).
  3. Abstracts decisions this project needs to control (prefix cache, tool-call format, MTP).
- **When to use it:** only if simplicity is preferred over fine control. Not recommended for production in this project.

#### MLC-LLM

- **Advantages:** lowest TTFT on prompts ≤16K (arXiv, M2 Ultra). Efficient paged KV cache.
- **Disadvantages:**
  1. MLC weight format (requires conversion from HuggingFace/GGUF). Limited catalog.
  2. Tool-calling poorly documented and untested with this project.
  3. Smaller community, fewer troubleshooting resources.
- **When to use it:** only if real-machine benchmarks show a decisive TTFT advantage and tool-calling is confirmed to work with the chosen model.

---

## 4. Hardware-Conditional Recommendation

### 4.1 Mac M4 or Earlier (M1/M2/M3) → llama-server (llama.cpp)

**Recommended server:** `llama-server` with GGUF (Metal backend).

**Rationale:**

1. **Faster prefill on M4:** 1,420 vs 1,180 tok/s for 8B-Q4 (Contra Collective, 2026). Without Neural Accelerators, Metal wins on raw prefill.
2. **Mature tool-calling:** battle-tested with Gemma, Qwen, Llama. No risk of broken tool calls.
3. **Automatic prefix cache:** `--cache-prompt` reuses KV of the system prompt. Turn 2+: near-zero prefill.
4. **MTP/speculative decoding** accelerates decode on MoE (-A4B/A3B).
5. **Universal GGUF:** all models available.

**Reference command (with MTP):**
```bash
llama-server \
  -m gemma-4-26b-a4b-MTP-Q4_K_M.gguf \
  --host 127.0.0.1 --port 8000 \
  --ctx-size 8192 \
  --threads 8 \
  --n-gpu-layers 99 \
  --cache-prompt \
  --load-mode mmap+mlock \
  --spec-type draft-mtp \
  --spec-draft-n-max 1
```

### 4.2 Mac M5 with ≥32 GB → vllm-mlx (excps fork)

**Recommended server:** `vllm-mlx` with `--tool-call-parser hermes` and optionally `--draft-model` for speculative decoding. Prompt cache always on by default.

**Rationale:**

1. **M5 Neural Accelerators:** ~4× TTFT advantage for MLX over Metal on prefill (Apple, WWDC 2025). This advantage only exists on M5.
2. **Hash-based prefix cache:** 10–30× TTFT reduction from turn 2+. The biggest latency lever for a multi-turn voice assistant.
3. **Tool-call auto-recovery:** mitigates tool-calling risk on MLX. If the model outputs invalid JSON, the server repairs it.
4. **≥32 GB unified memory** needed for MLX to efficiently use the Neural Engine and 4-bit quantization without swapping.

**Reference command:**
```bash
vllm-mlx serve mlx-community/gemma-4-26b-a4b-4bit \
  --host 127.0.0.1 --port 8000 \
  --tool-call-parser hermes \
  --draft-model mlx-community/gemma-4-1b-4bit \
  --num-draft-tokens 4
```
- `--draft-model`: small independent model (≠ MTP) that accelerates decode.
- Prompt cache: always on in SimpleEngine mode, no flags needed.

---

## 5. Discarded Options and Why

| Server | Primary reason |
|--------|---------------|
| **mlx-lm official** (current) | Unreliable tool-calling (acknowledged by the MLX team). No automatic prefix cache between requests. **Migration recommended.** |
| **Ollama** | Convenience layer that abstracts critical decisions (cache, tool-calling, MTP). Weak TTFT at long context. Less fine control. |
| **MLC-LLM** | Tool-calling untested with this project. Limited weight format. High compatibility risk. |
| **vLLM upstream** (NVIDIA/CUDA) | Does not support Apple Silicon. CUDA/ROCm only. |
| **llama.cpp without Metal** (CPU-only) | Prefill and decode ~3–5× slower than with Metal. Unfeasible for real-time voice. |

---

## 6. Micro-Benchmark Plan

Since the project runs on two different machines (M5 and M4), and TTFT crossovers between runtimes change with each release and model, the final decision must be based on real measurements, not just public benchmarks.

### 6.1 Setup

**This benchmark is external to the project** — it does not modify Seneschal's code or configuration. Run with `curl` or a minimal Python script against the candidate servers.

**Prerequisites:**
- Server running on `http://127.0.0.1:8000` (each candidate separately).
- Same model in compatible format (GGUF for llama-server, MLX for vllm-mlx/mlx-lm). Example: Gemma-4-26B-A4B in Q4_K_M / MLX 4-bit.
- Same system prompt (~4-8K tokens, copied from `seneschal.pro.toml` or the actual base prompt).

### 6.2 Scenarios

| Scenario | What it measures | Method |
|----------|-----------------|--------|
| **A. Cold turn** | TTFT with system prompt + 8 history turns + new question. Full prefill. | Send request with simulated history. Measure time from POST to first `delta.content` in SSE. Repeat 3×. |
| **B. Turn 2 (cache hit)** | TTFT with the same system prompt as cold turn + new message. Evaluates prefix cache. | Second request to the same server (same session/slot). The system prompt should be cached. Measure TTFT. Repeat 3×. |
| **C. Turn with tool-call** | Verify the server correctly emits `delta.tool_calls` + `finish_reason: "tool_calls"`. | Send a message that forces a tool call (e.g. "What time is it?" → `current_time`). Check SSE. |
| **D. Non-streaming** (summarization) | Latency of `complete()` (non-stream, 512 tokens). | POST with `stream: false`. Measure total time. |

### 6.3 Example Script

```python
#!/usr/bin/env python3
"""Micro-benchmark: TTFT for Seneschal on Apple Silicon.
Usage: python bench-ttft.py http://127.0.0.1:8000 gemma-4-26b-a4b
"""

import json, sys, time
import requests

URL = sys.argv[1]
MODEL = sys.argv[2]

SYSTEM_PROMPT = """You are Jarvis, a digital butler..."""  # Copy from seneschal.pro.toml

HISTORY = [
    {"role": "system", "content": SYSTEM_PROMPT},
    {"role": "user", "content": "Hello Jarvis"},
    {"role": "assistant", "content": "Good morning, sir. How can I help you?"},
    {"role": "user", "content": "What's the weather like today?"},
    {"role": "assistant", "content": "I don't have access to weather data right now, sir."},
]

def measure_ttft(messages, label: str, n: int = 3):
    times = []
    for i in range(n):
        t0 = time.monotonic()
        resp = requests.post(
            f"{URL}/v1/chat/completions",
            json={
                "model": MODEL,
                "messages": messages,
                "max_tokens": 300,
                "temperature": 0.5,
                "top_p": 0.9,
                "stream": True,
            },
            stream=True,
            timeout=60,
        )
        first_token = None
        for line in resp.iter_lines(decode_unicode=True):
            if line.startswith("data: ") and not line.startswith("data: [DONE]"):
                chunk = json.loads(line[6:])
                delta = chunk["choices"][0].get("delta", {})
                if "content" in delta:
                    first_token = time.monotonic()
                    break
        if first_token:
            ttft = first_token - t0
            times.append(ttft)
            print(f"  {label} run {i+1}: {ttft*1000:.0f} ms")
        else:
            print(f"  {label} run {i+1}: NO CONTENT")
    if times:
        print(f"  {label} mean: {sum(times)/len(times)*1000:.0f} ms")
    return times

def test_tool_call():
    t0 = time.monotonic()
    resp = requests.post(
        f"{URL}/v1/chat/completions",
        json={
            "model": MODEL,
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": "What time is it?"},
            ],
            "max_tokens": 100,
            "temperature": 0.1,
            "stream": True,
            "tools": [{
                "type": "function",
                "function": {
                    "name": "current_time",
                    "description": "Gets the current date and time",
                    "parameters": {"type": "object", "properties": {}}
                }
            }],
            "tool_choice": "auto",
        },
        stream=True,
        timeout=60,
    )
    has_tool_call = False
    finish = None
    for line in resp.iter_lines(decode_unicode=True):
        if line.startswith("data: ") and not line.startswith("data: [DONE]"):
            chunk = json.loads(line[6:])
            choice = chunk["choices"][0]
            if choice.get("delta", {}).get("tool_calls"):
                has_tool_call = True
            if choice.get("finish_reason"):
                finish = choice["finish_reason"]
    status = "PASS" if has_tool_call and finish == "tool_calls" else "FAIL"
    print(f"  Tool-call: {status} (has_tool_call={has_tool_call}, finish={finish})")
    return has_tool_call

print(f"Benchmark: {URL} model={MODEL}")
print()

print("=== Cold turn (system + history + new question) ===")
cold = HISTORY + [{"role": "user", "content": "What is the capital of France?"}]
measure_ttft(cold, "COLD")

print("\n=== Turn 2 (cache hit) ===")
warm = HISTORY + [{"role": "user", "content": "And what about Italy?"}]
measure_ttft(warm, "WARM")

print("\n=== Tool-call ===")
test_tool_call()
```

### 6.4 Decision Matrix

Run the benchmark on **each Mac** (M5 and M4) with **each candidate server** (llama-server, vllm-mlx, mlx-lm official). Fill in:

| Server | Mac | TTFT cold (ms) | TTFT warm (ms) | Cache speedup | Tool-call OK? |
|--------|-----|----------------|----------------|---------------|---------------|
| llama-server + GGUF | M4 | ___ | ___ | ___× | ___ |
| vllm-mlx | M4 | ___ | ___ | ___× | ___ |
| mlx-lm official | M4 | ___ | ___ | ___× | ___ |
| llama-server + GGUF | M5 | ___ | ___ | ___× | ___ |
| vllm-mlx | M5 | ___ | ___ | ___× | ___ |
| mlx-lm official | M5 | ___ | ___ | ___× | ___ |

**Decision criteria:**
1. Tool-call must pass (must-have requirement).
2. Lowest average warm TTFT (turn 2+) — the most frequent case in a conversation.
3. If tied, lowest cold TTFT.

### 6.5 Notes

- For `llama-server`: if using a GGUF with baked-in MTP (unsloth MTP-GGUF), add `--spec-type draft-mtp --spec-draft-n-max 1` to measure real latency with speculative decoding. Without these flags, decode will be slower and the benchmark won't reflect production performance.
- For `vllm-mlx`: prompt cache is always on (SimpleEngine). To measure with speculative decoding, add `--draft-model <small_model> --num-draft-tokens 4` (external draft model, ≠ MTP).
- The benchmark must use the actual model the project will use (Gemma-4-26B-A4B, Qwen3.5-35B-A3B, or whichever is chosen).
- Ensure the benchmark's system prompt matches the project's actual prompt (size and content).
- If the server uses slots/workers (e.g. llama.cpp `--parallel`), disable or set to 1 to reflect single-user usage.
- Scenario order matters: run the cold turn first, then the warm turn in the same session so the cache is populated.

---

## 7. References

### Performance Benchmarks

- Contra Collective (2026). "llama.cpp vs MLX: Benchmarking LLM Inference on Apple Silicon". [Web](https://presenc.ai/research/mlx-vs-llama-cpp-throughput-benchmarks-2026) — prefill 1,420 vs 1,180 tok/s on M4 8B-Q4.
- arXiv (2025). "Comparative Study of LLM Inference on Apple Silicon" — MLC-LLM lowest TTFT on prompts ≤16K (M2 Ultra); MLX requires full prefill before emitting.
- Apple WWDC 2025. "MLX with Neural Accelerators on M5" — ~4× TTFT advantage vs Metal.

### Tool-Calling and Servers

- mlx-lm discussions #371. "Tool calling / function calling for mlx_lm.server". [GitHub](https://github.com/ml-explore/mlx-lm/discussions/371) — acknowledged limitations.
- excps/vllm-mlx. Fork with 17 tool-call parsers + auto-recovery. [GitHub](https://github.com/excps/vllm-mlx).
- llama.cpp `llama-server`. OpenAI-compatible API docs. [GitHub](https://github.com/ggml-org/llama.cpp/tree/master/examples/server).

### Project Internals

- `doc/llm-provider.md` — `LlmProvider` trait architecture (Rust).
- `doc/LLM-requirements.md` — Model requirements and quality benchmarks.
- `doc/RECOMMENDED_LLM_PARAMS.md` — Recommended sampling parameters.
- `src/llm/client.rs` — OpenAI SSE client (`OpenAIClient`).
- `src/llm/provider.rs` — `LlmProvider` trait and factory.
- `src/pipeline/llm_task.rs` — Request logic, classifier, TTFT metrics.
- `src/tools/mod.rs` — Tool registry and `tool_definitions()`.
