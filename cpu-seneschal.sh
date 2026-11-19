#!/usr/bin/env bash
# Lanzador CPU-only de seneschal en Linux (derivado de linux-seneschal.sh).
#
# Frente a la ruta GPU de linux-seneschal.sh:
#   - Sin servicios NeMo: nada de nemotron-asr (:8026) ni magpie-tts (:8025).
#   - STT: Parakeet TDT 0.6b v3 local vía ONNX (STT_PROVIDER=parakeet). El
#     binario despacha los valores legacy de STT_PROVIDER a la fábrica de
#     seneschal-core (ver src/main.rs). Requiere la feature `parakeet` y el
#     modelo en PARAKEET_MODEL_DIR.
#   - TTS: Kokoro ONNX local (TTS_PROVIDER=kokoro, espeak-ng embebido).
#   - CPU estricto: sin feature `cuda` y con GPU_AUTO=0 + pins *_GPU=cpu para
#     Silero VAD, Parakeet y Kokoro (issue #14).
#   - LLM: servidor OpenAI-compatible en :8005 (tesla-model); el deep worker
#     apunta al mismo endpoint. Sampling y flags de thinking se heredan del
#     .env sin cambios (mismo servidor SGLang que la ruta GPU).
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

# ── LLM: tesla-model en :8005 ────────────────────────────────────────────────
# Sobreescribe a propósito LLM_URL/LLM_MODEL del .env (que apuntan a la ruta
# NVIDIA :8007). Sampling (LLM_TEMPERATURE/TOP_P/TOP_K) y thinking se heredan
# del .env: mismo servidor SGLang.
export LLM_URL="http://127.0.0.1:8005"
export LLM_MODEL="tesla-model"
# Deep worker (issue #236): mismo endpoint que el LLM de voz.
export DEEP_WORKER_URL="$LLM_URL"
export DEEP_WORKER_MODEL="$LLM_MODEL"
if command -v curl >/dev/null 2>&1 && curl -fsS -m 2 "${LLM_URL}/v1/models" >/dev/null 2>&1; then
    echo "LLM: ${LLM_MODEL} en ${LLM_URL}"
else
    # Arrancamos igualmente: el fallo se verá como error de red por petición.
    echo "LLM: el servidor no responde en ${LLM_URL}/v1/models; se reintentará en el runtime" >&2
fi

# ── STT: Parakeet local (ONNX, CPU) ──────────────────────────────────────────
# STT_PROVIDER vuelve a ser efectivo para los valores legacy; nemotron (default
# del .env/TOML) sigue siendo la ruta realtime de linux-seneschal.sh.
export STT_PROVIDER=parakeet
if [[ -z "${PARAKEET_MODEL_DIR:-}" || ! -d "$PARAKEET_MODEL_DIR" ]]; then
    echo "PARAKEET_MODEL_DIR no existe: '${PARAKEET_MODEL_DIR:-<unset>}'" >&2
    echo "Descarga el ONNX de https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx" >&2
    exit 1
fi
# Partiales "live" más espaciados en CPU: re-decodificar cada 1 s satura el
# host junto a Kokoro (doc/env-vars.md: "Raise on CPU-only hosts").
export STT_LIVE_INTERVAL_MS="${STT_LIVE_INTERVAL_MS:-2500}"
echo "STT: Parakeet local en CPU (${PARAKEET_MODEL_DIR})"

# ── TTS: Kokoro local (ONNX + espeak-ng embebido) ────────────────────────────
# El .env trae TTS_PROVIDER=avspeech (default macOS; aquí abortaría): forzamos
# kokoro. Voz/velocidad/idioma vienen del .env (KOKORO_VOICE/SPEED/LANGUAGE).
export TTS_PROVIDER=kokoro
echo "TTS: Kokoro local en CPU (${KOKORO_MODEL:-models/kokoro-v1.0.onnx})"

# ── CPU estricto (issue #14) ─────────────────────────────────────────────────
# Sin feature `cuda` el runtime ONNX ya es CPU, pero fijamos pins explícitos
# (ganan sobre GPU_AUTO) para que el resumen de asignación no sugiera GPU.
export GPU_AUTO=0
export VAD_GPU=cpu
export PARAKEET_GPU=cpu
export TTS_GPU=cpu

# ── Features ─────────────────────────────────────────────────────────────────
# Sin openai-tts/openai-stt (no hay servicios de voz remotos) y sin cuda.
FEATURES="control,parakeet,kokoro,tui,remote"
echo "Building with features: $FEATURES"

RUST_LOG=debug exec cargo run --bin seneschal --features "$FEATURES" --release -- "$@" 2> >(grep -vE "^(whisper_|ggml_)" >&2)

## Tools and agent debugging
# RUST_LOG=pipeline=debug,llm=debug,tools=debug,agent=debug exec cargo run --bin seneschal --features control,parakeet,kokoro,tui,remote --release -- "$@" 2> >(grep -vE "^(whisper_|ggml_)" >&2)
