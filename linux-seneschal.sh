#!/usr/bin/env bash
# Lanzador de seneschal en Linux (contraparte de mac-seneschal.sh).
#
# Diferencias frente a macOS:
#   - Los features `speech` (SFSpeechRecognizer) y `avspeech` (AVSpeechSynthesizer)
#     son exclusivos de macOS y NO se compilan aquí.
#   - STT: provider `openai` contra un servicio remoto OpenAI-compatible. Se
#     prefiere nemotron-asr (NeMo-Speech.cpp, Nemotron 3.5 ASR, :8026; issue #21)
#     y se cae a seneschal-asr (vLLM Qwen3-ASR, :8024) si el primero no responde.
#   - TTS: provider `openai` contra magpie-tts (NeMo-Speech.cpp, Magpie, :8025;
#     issue #20); si no responde, Kokoro local (ONNX + espeak-ng embebido).
#
# Requisitos de build en Debian/Ubuntu (una sola vez):
#   sudo apt-get install -y pkg-config libssl-dev libasound2-dev libclang-dev cmake build-essential

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Load environment variables
if [[ -f "$SCRIPT_DIR/.env" ]]; then
    set -a
    # shellcheck source=.env
    source "$SCRIPT_DIR/.env"
    set +a
fi

# ── Preflight: dependencias nativas de build en Linux ────────────────────────
# (si falta alguna, cargo fallaría a mitad de la compilación con errores opacos)
missing=()
command -v cargo >/dev/null 2>&1 || missing+=("cargo (rustup: https://rustup.rs)")
command -v cmake >/dev/null 2>&1 || missing+=("cmake")
command -v g++   >/dev/null 2>&1 || missing+=("g++ (build-essential)")

PKGCONFIG="$(command -v pkg-config || command -v pkgconf || true)"
if [[ -z "$PKGCONFIG" ]]; then
    missing+=("pkg-config")
else
    # openssl-sys (sqlx/reqwest native-tls) y alsa-sys (cpal) se localizan vía pkg-config
    "$PKGCONFIG" --exists openssl 2>/dev/null || missing+=("libssl-dev")
    "$PKGCONFIG" --exists alsa    2>/dev/null || missing+=("libasound2-dev")
fi

# bindgen (espeak-rs-sys, que compila espeak-ng embebido para Kokoro) necesita libclang
# Ojo: no usar `ls glob1 glob2 | grep` con pipefail: ls sale con error si algún
# glob no casa y eso contaminaría el exit status del pipeline aunque hubiera hits.
libclang_found=0
for p in /usr/lib/libclang.so* /usr/lib/*/libclang.so* /usr/lib/llvm-*/lib/libclang.so* \
         /usr/lib/*/libclang-*.so*; do
    if [[ -e "$p" ]]; then libclang_found=1; break; fi
done
(( libclang_found )) || missing+=("libclang-dev")

if ((${#missing[@]} > 0)); then
    echo "Faltan dependencias de build para Linux:" >&2
    printf '  - %s\n' "${missing[@]}" >&2
    echo >&2
    echo "Instálalas con:" >&2
    echo "  sudo apt-get install -y pkg-config libssl-dev libasound2-dev libclang-dev cmake build-essential" >&2
    exit 1
fi

# ── Voz (STT + TTS) — DOS servicios NeMo independientes ──────────────────────
#   STT: nemotron-asr (:8026) -> NVIDIA Nemotron 3.5 ASR vía NeMo-Speech.cpp
#   TTS: magpie-tts    (:8025) -> NVIDIA Magpie TTS vía NeMo-Speech.cpp
# El STT usa el WebSocket realtime del proveedor: VAD-driven endpointing y
# parciales/finales ocurren en el servicio. Si no responde, el proveedor
# reconecta; no hay fallback a Qwen3-ASR (no implementa este protocolo).
# En Linux no existe AVSpeechSynthesizer: si el .env hereda TTS_PROVIDER=avspeech
# el binario abortaría al arrancar, así que siempre lo sobreescribimos aquí.
#
# STT_PROVIDER es configuración legacy: el ejecutable ya no la usa para
# seleccionar Whisper/Parakeet/OpenAI.

# ── STT: Nemotron 3.5 ASR (nemotron-asr, :8026) ──────────────────────────────
STT_SERVICE_URL="${STT_URL:-http://localhost:8026}"
export STT_URL="$STT_SERVICE_URL"
export STT_MODEL="${STT_MODEL:-nemotron-3.5}"
export STT_TIMEOUT_MS="${STT_TIMEOUT_MS:-15000}"
if command -v curl >/dev/null 2>&1 && curl -fsS -m 2 "${STT_SERVICE_URL}/ready" >/dev/null 2>&1; then
    echo "STT: Nemotron 3.5 ASR en ${STT_URL} (${STT_MODEL})"
else
    # Arrancamos igualmente: el proveedor reconecta y el fallo se ve como
    # error de red por sesión, no como crash de configuración.
    echo "STT: nemotron-asr no responde en ${STT_SERVICE_URL}; se reintentará en el runtime" >&2
    echo "STT: revisa 'docker compose --profile nvidia logs nemotron-asr' (si aborta" >&2
    echo "     con 'VAD model file does not exist', falta el GGUF del Silero VAD:" >&2
    echo "     ejecuta docker/nemotron-asr/download-weights.sh)" >&2
fi

# ── TTS: Magpie (servicio TTS independiente) ─────────────────────────────────
OPENAI_TTS_URL="${OPENAI_TTS_URL:-http://localhost:8025}"
if command -v curl >/dev/null 2>&1 && curl -fsS -m 2 "${OPENAI_TTS_URL}/v1/models" >/dev/null 2>&1; then
    export TTS_PROVIDER=openai
    export OPENAI_TTS_URL
    # OJO bug abierto en nemo: los nombres de voz no corresponden con las voces
    # reales (p.ej. "Sofia" suena como el hablante Jason). Ver issue.
    export OPENAI_TTS_VOICE="${OPENAI_TTS_VOICE:-Sofia}"
    # MagpieTTS exige un language_code real: "auto" -> "Auto" devuelve 400.
    export OPENAI_TTS_LANGUAGE="${OPENAI_TTS_LANGUAGE:-es-ES}"
    # NanoCodec emite 22050 Hz; sin esto el audio suena ~9% rápido.
    export OPENAI_TTS_SAMPLE_RATE="${OPENAI_TTS_SAMPLE_RATE:-22050}"
    # NeMo-Speech.cpp hace su propio warmup; no hace falta el de ~90 s del NIM.
    export OPENAI_TTS_WARMUP="${OPENAI_TTS_WARMUP:-0}"
    export OPENAI_TTS_TIMEOUT_MS="${OPENAI_TTS_TIMEOUT_MS:-30000}"
    echo "TTS: Magpie en ${OPENAI_TTS_URL} (voice=${OPENAI_TTS_VOICE} lang=${OPENAI_TTS_LANGUAGE} rate=${OPENAI_TTS_SAMPLE_RATE})"
else
    echo "TTS: NeMo-Speech.cpp no responde en ${OPENAI_TTS_URL}; usando Kokoro" >&2
    export TTS_PROVIDER=kokoro
fi



# ── GPU (issue #14): CUDA opt-in ─────────────────────────────────────────────
# Con SENECHAL_CUDA=1 se compila con CUDA (whisper.cpp + ONNX Runtime GPU) y,
# por defecto, GPU_AUTO reparte VAD/Parakeet/Kokoro entre las GPUs visibles.
# Requiere toolkit CUDA instalado (nvcc) en el host. Requiere GPU_AUTO=0 o
# overrides VAD_GPU/PARAKEET_GPU/TTS_GPU para un reparto manual.
FEATURES="control,parakeet,kokoro,tui,remote,openai-tts,openai-stt"
if [[ "${SENECHAL_CUDA:-0}" == "1" ]]; then
    FEATURES="$FEATURES,cuda"
fi
echo "Building with features: $FEATURES"

RUST_LOG=debug exec cargo run --bin seneschal --features "$FEATURES" --release -- "$@" 2> >(grep -vE "^(whisper_|ggml_)" >&2)

## Tools and agent debugging
# RUST_LOG=pipeline=debug,llm=debug,tools=debug,agent=debug exec cargo run --bin seneschal --features control,parakeet,kokoro,tui,remote --release -- "$@" 2> >(grep -vE "^(whisper_|ggml_)" >&2)
