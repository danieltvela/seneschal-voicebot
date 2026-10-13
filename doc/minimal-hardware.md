# Hardware Mínimo Viable — Seneschal

> Cálculo del hardware mínimo necesario para ejecutar el pipeline completo de voz
> (STT → LLM → TTS) con el escenario de **5 modelos mínimos**: VAD, STT, TTS, un
> único LLM (voz rápida + deep work con thinking conmutable) y el proyector de
> visión del propio LLM.
>
> **Fecha:** octubre 2026
> **Audiencia:** selección/compra de hardware (Apple Silicon o NVIDIA) y puesta a
> punto de un despliegue mono-usuario completo.
> **Complementa:** [`LLM-requirements.md`](LLM-requirements.md) (contrato del LLM),
> [`llm-providers.md`](llm-providers.md) (servidores por hardware), [`deep-worker.md`](deep-worker.md)
> (thinking conmutable, mismo servidor), [`build-features.md`](build-features.md)
> (features y multi-GPU), [`env-vars.md`](env-vars.md).

---

## Tabla de contenidos

1. [Resumen ejecutivo](#1-resumen-ejecutivo)
2. [Supuestos del escenario mínimo](#2-supuestos-del-escenario-mínimo)
3. [Los 5 modelos](#3-los-5-modelos)
4. [El LLM único con thinking conmutable](#4-el-llm-único-con-thinking-conmutable)
5. [Presupuesto de memoria](#5-presupuesto-de-memoria)
6. [Hardware mínimo — comparativa por plataforma](#6-hardware-mínimo--comparativa-por-plataforma)
7. [Verificación de latencia](#7-verificación-de-latencia)
8. [Comandos de referencia](#8-comandos-de-referencia)
9. [Qué queda fuera del mínimo](#9-qué-queda-fuera-del-mínimo)
10. [Fuentes](#10-fuentes)

---

## 1. Resumen ejecutivo

| Nivel | LLM (uno para todo) | Memoria LLM | Apple Silicon | NVIDIA | Disco de modelos |
|-------|---------------------|-------------|---------------|--------|------------------|
| **A — mínimo viable** | Qwen3.5-9B (Q4_K_M + mmproj) | ~7–8 GB | **Mac 24 GB** (16 GB solo con STT/TTS del sistema) | **GPU 8–12 GB** (RTX 3060 12GB / 4060 Ti 16GB) + 32 GB RAM | ~20 GB |
| **B — mínimo recomendado** | Qwen3.5-35B-A3B (Q4_K_M + mmproj) | ~23–25 GB | **Mac 48 GB** (M4 Pro/Max) | **GPU 24 GB** (RTX 3090/4090) + 64 GB RAM; **32 GB** (RTX 5090) holgado | ~50 GB |

- Los modelos de voz (VAD + STT + TTS) pesan **~3 GB en total** y corren en CPU:
  no son el factor que decide el hardware. **El LLM decide.**
- Para "un modelo que haga de todo con latencia mínima", la forma óptima es un
  **MoE con pocos parámetros activos (clase -A3B)**: velocidad de un denso pequeño
  en la ruta de voz (~100 t/s en RTX 3090) y calidad de modelo grande con el
  thinking activado para deep work.
- CPU-only **no es viable** para el LLM (objetivo TG > 30 t/s; un 9B Q4 en CPU
  rinde 5–15 t/s). Se necesita GPU o Apple Silicon.

---

## 2. Supuestos del escenario mínimo

1. **Despliegue mono-usuario, todo local** en una máquina (sin servicios remotos).
2. **Un único LLM** sirve la voz y el deep work. El thinking se conmuta por
   request: OFF en conversación, ON en tareas profundas. Esto es exactamente la
   *"política recomendada: voz rápida, worker profundo, mismo servidor"* de
   [`deep-worker.md`](deep-worker.md) — `LLM_URL` y `DEEP_WORKER_URL` apuntan al
   mismo servidor.
3. **Visión incluida** (`take_screenshot`): el código enruta las imágenes al
   **provider LLM primario** vía `OpenAIClient::complete_multimodal`
   (`crates/seneschal-core/src/llm/client.rs:591-617`), así que el LLM elegido
   debe ser **nativamente multimodal** (el "modelo 5" es su proyector `mmproj`,
   no un modelo independiente).
4. **Idioma:** español nativo + switching EN/ES en toda la cadena.
5. Los extras ligeros (verificación de hablante, clasificador) no forman parte
   del mínimo (ver [§9](#9-qué-queda-fuera-del-mínimo)).

---

## 3. Los 5 modelos

| # | Rol | Modelo mínimo viable | Disco | Memoria en uso | Dónde corre | Config |
|---|-----|----------------------|-------|----------------|-------------|--------|
| 1 | **VAD** | Silero v5.1.2 (`ggml-silero-v5.1.2.bin`, GGML) | ~10 MB | ~50 MB | CPU (basta) | `VAD_MODEL` |
| 2 | **STT** | **Parakeet TDT 0.6B v3** (ONNX) — o Whisper large-v3-turbo si hay GPU | 2.4 GB / 1.6 GB | ~1–2.5 GB | CPU o GPU | `STT_PROVIDER=parakeet` / `WHISPER_MODEL` |
| 3 | **TTS** | **Kokoro 82M** (`kokoro-v1.0.onnx` + `voices-v1.0.bin`) | ~0.35 GB | ~0.5 GB | CPU (basta) | `TTS_PROVIDER=kokoro` |
| 4 | **LLM voz + deep work** | **Qwen3.5-9B** Q4_K_M (piso) · **Qwen3.5-35B-A3B** Q4_K_M (recomendado) | 5.8 GB / 21.5 GB | 6.5–8 GB / 22–24 GB | GPU / Apple ANE-Metal | `LLM_MODEL`, `DEEP_WORKER_MODEL` |
| 5 | **Visión** (`take_screenshot`) | `mmproj` del propio LLM | ~0.9–2 GB | ~0.5–1 GB | junto al LLM | (implícito en `LLM_MODEL`) |

### 3.1 VAD — Silero v5.1.2

- GGML (~10 MB), descargado de `ggml-org/silero-v5.1.2` por `install.sh`.
- Corre dentro de `whisper-cpp-plus` (`WhisperVadProcessor`) envuelto por
  `SileroDetector` en el `VadSegmenter` compartido (`crates/seneschal-core/src/stt/segmenter.rs`):
  sondas de 200 ms, confirmación 2-de-3, pre-roll 300 ms, post-roll 200 ms.
- Coste de hardware: **despreciable**. CPU suficiente.
- ⚠️ Nota de instalación: el default de `VAD_MODEL` apunta a `models/ggml-silero-vad.bin`
  pero el fichero en disco es `models/ggml-silero-v5.1.2.bin` — en una instalación
  limpia conviene fijar `VAD_MODEL` en `.env` (bug conocido, ver
  `tasks/stt-audit-report-2026-08-25.md`).

### 3.2 STT — Parakeet TDT 0.6B v3 (default del instalador)

- ONNX Runtime (`parakeet-rs`, `ParakeetTDT::from_pretrained`); directorio
  `parakeet-tdt-0.6b-v3-onnx` con 4 ficheros (`install.sh:572-599`):
  `encoder-model.onnx` (~40 MB) + `encoder-model.onnx.data` (~2.3 GB) +
  `decoder_joint-model.onnx` (~69 MB) + `vocab.txt`.
- 25 idiomas; streaming en vivo + transcripción final (contrato P1–P6 de
  [`stt-contract.md`](stt-contract.md)); EP CUDA opcional con `--features cuda`.
- **Es la apuesta para hardware mínimo** (0.6B corre holgado en CPU).
- **Alternativa con GPU/Metal:** Whisper `large-v3-turbo` (GGML 1.6 GB, 99 idiomas,
  default `WHISPER_MODEL`). El audit del proyecto mide **RTF 0.03–0.11 en M4 Pro
  con Metal**; en CPU su RTF sube a ~0.3–0.6 (aceptable, menos ágil que Parakeet).
  Gate de regresión del proyecto: RTF steady-state < 0.5.

### 3.3 TTS — Kokoro 82M

- `kokoro-v1.0.onnx` (~305 MB) + `voices-v1.0.bin` (embeddings de voz), vía
  `kokorox`; salida mono f32 24 kHz; fonemización con `espeak-ng` (voces en español);
  asignación GPU opcional (`TTS_GPU`/`GPU_AUTO`), si no corre en CPU con RTF ≪ 1.
- Trivial para cualquier CPU moderna; **no influye en el cálculo de GPU**.
- Sustituto sin modelo en macOS: AVSpeechSynthesizer (`--features avspeech`).

### 3.4 LLM — Qwen3.5-9B (piso) / Qwen3.5-35B-A3B (recomendado)

Ver [§4](#4-el-llm-único-con-thinking-conmutable).

### 3.5 Visión — el `mmproj` del mismo LLM

- Para llama.cpp los GGUF de Qwen3.5 incluyen un fichero aparte de proyector
  (`mmproj-f16.gguf`, ~0.9 GB para el 9B; ~1–2 GB para el 35B). Se pasa con
  `--mmproj`; llama.cpp lo baja automáticamente en algunos repos cuantizados.
- Ollama **no** soporta `mmproj` separado — para visión usar llama.cpp, LM Studio
  o vLLM (con el encoder incluido en el checkpoint HF).

---

## 4. El LLM único con thinking conmutable

### 4.1 Contrato que el proyecto impone (de `LLM-requirements.md` §3 y §10)

| Requisito | Detalle |
|-----------|---------|
| API | OpenAI-compatible `/v1/chat/completions` con SSE + `/v1/models` |
| Tool calling | `delta.tool_calls` + `finish_reason: "tool_calls"` nativos |
| Contexto | ≥ 8192 tokens (configurable, `LLM_CONTEXT_TOKENS`) |
| Thinking | `chat_template_kwargs: {"enable_thinking": bool}` — canal `delta.reasoning_content` nunca se locuta |
| Multimodal | `image_url` + texto (`take_screenshot`) |
| Sampling por request | `temperature`, `top_p`, `top_k`, `repetition_penalty`, `min_p` |
| Latencia | TTFT warm < 500 ms · TG > 30 t/s · prefill > 200 t/s · KV-cache speedup ≥ 3× |
| Idioma/formato | Español nativo, texto "speakable", 1–3 frases, sin markdown |

### 4.2 Política de un solo servidor (ya implementada)

Del [`deep-worker.md`](deep-worker.md) — voz y worker comparten servidor y modelo:

```ini
# Voz: thinking siempre OFF, también junto a tools (anula el default del servidor)
LLM_THINKING=false
LLM_THINKING_WITH_TOOLS=true

# Deep work: mismo servidor/modelo, thinking ON en todas las pasadas
DEEP_WORKER_ENABLED=1
DEEP_WORKER_URL=http://127.0.0.1:8000     # = LLM_URL
DEEP_WORKER_MODEL=<mismo que LLM_MODEL>
DEEP_WORKER_THINKING=true
DEEP_WORKER_THINKING_MODE=inline          # servidores con reasoning parser: external|inline
```

El estilo del flag lo resuelve `LLM_THINKING_STYLE=auto` por nombre de modelo
(`client.rs` `ThinkingStyle`: `kwargs` para Qwen, `effort` para o-series/gpt-5,
`ollama`, `none`). En Qwen3.5/3.6 el mecanismo oficial es
`chat_template_kwargs` — exactamente lo que el cliente ya envía.

### 4.3 Por qué MoE (clase -A3B) y no un denso

| Cualidad | Dense 27B (Q4 ≈ 17 GB) | **MoE 35B-A3B** (Q4 ≈ 22 GB) | Dense 9B (Q4 ≈ 6 GB) |
|---|---|---|---|
| Decode en voz | ~34 t/s (RTX 3090) — en el límite del objetivo | **~100–111 t/s (RTX 3090)**; ~85 t/s MLX | ~30–50 t/s Mac / >50 t/s GPU |
| Deep work con thinking | Bueno | **Muy bueno** (MMLU-Pro 85.3 · GPQA 84.2 · SWE-bench 69.2; 3.6: 73.4) | Funcional, limitado |
| Visión nativa | Sí (Qwen3.5) | Sí (Qwen3.5/3.6, `mmproj`) | Sí (Qwen3.5-9B, early fusion) |
| Thinking conmutable | Sí (`chat_template_kwargs`) | Sí (mecanismo oficial en 3.6) | Sí |
| KV cache @8K | mayor (más layers con atención) | **~0.2 GB** (solo 10/40 layers atienden) | ~0.13 GB (8/32 layers) |
| Contexto nativo | 262K | 262K (extensible a 1M) | 262K |

Paradójicamente el MoE de 35B es **más rápido en la ruta de voz** que el denso de
27B: el coste por token sigue a los parámetros activos (3B), mientras que el
denso paga sus 28B completos en cada paso. En 24 GB de VRAM el MoE gana en los
dos ejes (latencia y capacidad).

### 4.4 Candidatos concretos

| Modelo | Q4 en memoria (con mmproj) | Datos clave | Cuándo elegirlo |
|--------|----------------------------|-------------|-----------------|
| **Qwen3.5-9B** (Q4_K_M 5.75 GB + mmproj ~0.9 GB) | ~7–8 GB | BFCL-V4 66.1% · TAU2 79.1% · 201 idiomas · builds MTP | **Tier A** — piso real del sistema |
| **Qwen3.5-35B-A3B** (Q4_K_M ~21.3 GB + mmproj) | ~23–25 GB | 3B activos · ~111 t/s en 3090 · MMLU-Pro 85.3 | **Tier B** — recomendado |
| **Qwen3.6-35B-A3B** (Q4_K_M ~21.4 GB) | ~23–25 GB | SWE-bench 73.4 · Terminal-Bench 51.5 · ~101 t/s · thinking oficial vía `chat_template_kwargs` | Tier B orientado a agentes/código |
| **Gemma-4-26b-a4b** (Q4_K_M ~14–16 GB) | ~15–18 GB | MoE 26B/4B activos · MTP GGUF (unsloth) · referencia de `llm-providers.md` | Término medio — **verificar soporte de visión/mmproj antes de comprometerse** |
| Qwen3.5-27B (dense, Q4 ~17 GB) | ~18–19 GB | ~34 t/s en 3090 | Solo si se prefiere un denso "predecible" en 32 GB Mac |

⚠️ En 8–9B el tool calling (el proyecto elige entre 16+ tools) es el punto débil.
Validar siempre con `scripts/bench-models.py` + `scripts/fixtures.json` (35 tests,
8 grupos: `tool_use`, `voice_format`, `brevity`, …) antes de dar por viable un
modelo pequeño.

---

## 5. Presupuesto de memoria

Todos los componentes vivos simultáneamente (pipeline mono-proceso + servidor LLM):

| Componente | Apple Silicon (memoria unificada) | NVIDIA |
|------------|-----------------------------------|--------|
| LLM Qwen3.5-9B Q4 + mmproj + KV 8–16K | ~7.5 GB | ~7.5 GB VRAM |
| LLM Qwen3.5-35B-A3B Q4 + mmproj + KV | ~24 GB | ~24 GB VRAM |
| STT Parakeet (encoder 2.3 GB) | ~2.5 GB | ~2.5 GB (GPU o CPU) |
| STT Whisper large-v3-turbo (alt.) | ~1.6 GB | ~1.6 GB (GPU o CPU) |
| TTS Kokoro 82M | ~0.5 GB | ~0.5 GB (GPU o CPU) |
| VAD Silero | ~50 MB | ~50 MB |
| Runtime/overhead | ~1.5 GB | ~1.5 GB VRAM |
| Sistema operativo | ~6–8 GB (macOS) | — (va a RAM) |

Reparto GPU en builds CUDA: `GPU_AUTO=1` (default) coloca Silero VAD, Parakeet y
Kokoro round-robin en la GPU con más VRAM libre; `VAD_GPU`/`PARAKEET_GPU`/`TTS_GPU`
los fijan o los mandan a `cpu` (ver [`build-features.md`](build-features.md) §multi-GPU).
El servidor LLM se separa con `CUDA_VISIBLE_DEVICES` — en hardware de una sola
GPU, congelar la voz en CPU cuando la VRAM no alcance.

---

## 6. Hardware mínimo — comparativa por plataforma

### Tier A — mínimo viable (voz ágil, deep work básico)

| | Apple Silicon | NVIDIA (Linux) |
|---|---|---|
| **Mínimo real** | **Mac 24 GB** (p. ej. Mac mini M4 24 GB). Con 16 GB solo es viable usando STT/TTS del sistema (`--features speech` + `avspeech`, cero modelos) y al límite de swap | **GPU 12 GB** (RTX 3060 12GB) con voz en CPU; **16 GB** (4060 Ti) con todo en GPU. 8 GB: solo LLM (`PARAKEET_GPU=cpu`, `TTS_GPU=cpu`), justo |
| RAM sistema | incluida (unificada) | 32 GB (16 GB mínimo) |
| CPU | M4 base basta | 6–8 núcleos (`--threads 8`) |
| Disco | ~20 GB de modelos | ~20 GB |
| Servidor LLM | `llama-server` (Metal) — recomendado para M4 y anteriores | `llama-server` o vLLM |

### Tier B — mínimo recomendado (un modelo serio para voz *y* deep work)

| | Apple Silicon | NVIDIA (Linux) |
|---|---|---|
| **Mínimo recomendado** | **Mac 48 GB** (M4 Pro/Max). 32 GB solo con cuantización Q3 (~18 GB) o STT/TTS del sistema — muy justo | **GPU 24 GB** (RTX 3090/4090) con voz fijada a CPU; **32 GB** (RTX 5090) holgado con todo en GPU |
| RAM sistema | incluida | 64 GB recomendado |
| Disco | ~50 GB | ~50 GB |
| Servidor LLM | `llama-server` (M4−) / `vllm-mlx` (M5 ≥32 GB, prompt cache 10–30×) | **vLLM con `--reasoning-parser qwen3`** — thinking por canal separado, tool calling maduro |

Reglas de decisión:

- **24 GB de VRAM con Q4_K_M del 35B-A3B es el punto dulce**: cabe a contexto
  8–32K (KV casi plano por la atención híbrida), pero no deja hueco para los
  modelos de voz en GPU → fijarlos a CPU no cuesta latencia (RTF ≪ 1 igualmente).
- **Memoria unificada de Mac**: el sistema operativo + apps consumen 6–8 GB;
  sumar siempre ese margen antes de decidir 32 vs 48 GB.
- **CPU-only no viable** para el LLM (ver resumen ejecutivo). El resto del
  pipeline sí puede ir en CPU.

---

## 7. Verificación de latencia

Ruta de voz con thinking OFF (Tier B, régimen cálido):

1. **VAD** — barge-in inmediato (sondas de 200 ms): sin coste.
2. **STT** — transcripción en vivo mientras el usuario habla; gate RTF < 0.5 en
   la transcripción final. Parakeet/Whisper lo cumplen con margen.
3. **LLM** — TTFT warm ~100–300 ms con prefix cache (el system prompt de 4–8K
   tokens se cachea: `--cache-prompt` en llama.cpp, prompt cache persistente en
   vllm-mlx/vLLM → 10–30× de reducción desde el turno 2). Decode ~100 t/s: una
   respuesta de 1–3 frases (máx. 300 tokens) en < 1 s.
4. **TTS** — Kokoro sintetiza por frase en cuanto llega el primer punto.

**Total: primera palabra hablada en ~0.5–1 s en régimen cálido.** Cumple el
objetivo del proyecto (SpeechStart → primer audio < 500 ms después del prefill
cacheado). El Tier A cumple también, con menos margen en decode.

Deep work (thinking ON): latencia no crítica — las tareas del worker están
desacopladas del barge-in (`DEEP_WORKER_TIMEOUT_MS` 300 s, máx. 2048 tokens).

---

## 8. Comandos de referencia

### Tier A — llama-server (Mac o NVIDIA)

```bash
llama-server -m Qwen3.5-9B-Q4_K_M.gguf \
  --mmproj Qwen3.5-9B.mmproj-f16.gguf \
  --host 127.0.0.1 --port 8000 \
  --ctx-size 8192 --threads 8 \
  --cache-prompt --load-mode mmap+mlock
```

### Tier B — llama-server con MTP (MoE)

```bash
llama-server -m Qwen3.5-35B-A3B-Q4_K_M.gguf \
  --mmproj Qwen3.5-35B-A3B.mmproj-f16.gguf \
  --host 127.0.0.1 --port 8000 \
  --ctx-size 8192 --threads 8 --n-gpu-layers 99 \
  --cache-prompt --load-mode mmap+mlock \
  --spec-type draft-mtp --spec-draft-n-max 1
```

### Tier B — vLLM en NVIDIA (thinking en canal separado)

```bash
vllm serve Qwen/Qwen3.5-35B-A3B \
  --port 8000 --max-model-len 32768 \
  --reasoning-parser qwen3
```

### Seneschal (`.env`)

```ini
LLM_URL=http://127.0.0.1:8000
LLM_MODEL=qwen3.5-35b-a3b          # o qwen3.5-9b en Tier A
LLM_THINKING=false
LLM_THINKING_WITH_TOOLS=true

DEEP_WORKER_ENABLED=1
DEEP_WORKER_URL=http://127.0.0.1:8000
DEEP_WORKER_MODEL=qwen3.5-35b-a3b
DEEP_WORKER_THINKING=true
DEEP_WORKER_THINKING_MODE=inline

# Voz: parakeet + kokoro en CPU en el hardware justo
STT_PROVIDER=parakeet
TTS_PROVIDER=kokoro
# PARAKEET_GPU=cpu
# TTS_GPU=cpu
```

Validación de calidad/latencia del modelo antes de dar por cerrado el hardware:
`python3 scripts/bench-models.py` (velocidad + 35 fixtures) y
`python3 scripts/bench-server.py` (escenario hot-cache).

---

## 9. Qué queda fuera del mínimo

- **Despliegue "actual" completo** (defaults del repo): voz `qwen3.8-27b` +
  deep worker como modelo *aparte* (`qwen3-dense`) + servicios Qwen3-ASR-1.7B
  (vLLM, `STT_PROVIDER=openai`) y Qwen3-TTS-1.7B (vLLM-Omni, `openai-tts`).
  Es el escenario *pro* documentado en [`tts-openai.md`](tts-openai.md)
  (GPU compartida con ~22 GB libres) — excede con creces el mínimo.
- **Verificación de hablante**: `models/speaker_embedding.onnx` (sherpa-rs,
  `--features speaker`). Ligero (~50 MB) pero prescindible.
- **Clasificador**: solo JSON de centroides/pesos — sin modelo ML real
  ([`classifier.md`](classifier.md): niveles 2/3 son stubs muertos).
- **CPU-only para el LLM**: latencia de voz inservible.
- **Cuanta < Q3 en el LLM**: degradación fuerte de vision + razonamiento; en
  hardware muy justo preferir 9B Q4 antes que 35B Q2.

---

## 10. Fuentes

- **Del proyecto:** `doc/LLM-requirements.md` (contrato + targets), `doc/llm-providers.md`
  (servidores por hardware, benchmarks Metal/MLX), `doc/deep-worker.md` (política
  de un servidor, thinking por request), `doc/build-features.md` (multi-GPU
  `GPU_AUTO`), `install.sh` (tamaños y URLs de los modelos de voz),
  `tasks/stt-audit-report-2026-08-25.md` (RTF de Whisper en Metal),
  `doc/tts-openai.md` (escenario pro con Qwen3-TTS).
- **Externas (octubre 2026):** tablas de VRAM/quant de Qwen3.5-9B y 35B-A3B
  (canitrun.dev, quantized.uk, modelfit.io — GGUF bartowski/jc-builds/prithivMLmods),
  benchmarks de generación (RTX 3090/5090, Apple Silicon), guía de selección
  Qwen3.5 por VRAM (BSWEN, insiderllm).
