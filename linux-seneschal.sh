#!/usr/bin/env bash
# Lanzador de seneschal en Linux (contraparte de mac-seneschal.sh).
#
# Diferencias frente a macOS:
#   - Los features `speech` (SFSpeechRecognizer) y `avspeech` (AVSpeechSynthesizer)
#     son exclusivos de macOS y NO se compilan aquí.
#   - STT: Parakeet (ONNX Runtime, CPU)  -> --features parakeet + STT_PROVIDER=parakeet
#   - TTS: Kokoro (ONNX + espeak-ng embebido en el binario) -> --features kokoro + TTS_PROVIDER=kokoro
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

# ── Overrides específicos de Linux (el .env apunta a dispositivos/voices de macOS) ──
# STT: Parakeet TDT vía ONNX. El modelo ya está en models/parakeet-tdt-0.6b-v3-onnx.
# export STT_PROVIDER=parakeet
# export PARAKEET_MODEL_DIR="${PARAKEET_MODEL_DIR:-models/parakeet-tdt-0.6b-v3-onnx}"

# STT:
export STT_PROVIDER=openai
export STT_URL=http://localhost:8024
export STT_MODEL=qwen3-asr
export STT_TIMEOUT_MS=10000


# TTS (issue #20): contenedor NeMo-Speech.cpp sirviendo MagpieTTS Multilingual
# + NanoCodec, consumido por el proveedor OpenAI-compatible (TTS_PROVIDER=openai).
# Si el contenedor responde en :8025 lo usamos; si no, caemos a Kokoro (ONNX
# local), que ya viene en FEATURES. En Linux no existe AVSpeechSynthesizer, así
# que si el .env hereda TTS_PROVIDER=avspeech el binario abortaría al arrancar.
OPENAI_TTS_URL="${OPENAI_TTS_URL:-http://localhost:8025}"
if command -v curl >/dev/null 2>&1 && curl -fsS -m 2 "${OPENAI_TTS_URL}/v1/models" >/dev/null 2>&1; then
    export TTS_PROVIDER=openai
    export OPENAI_TTS_URL
    # Voces anunciadas por GET /v1/models: John, Sofia, Aria, Jason, Leo.
    # There is currently an open bug in nemo: voices doesn't correspond with the ones listed. Sofia is Jason
    export OPENAI_TTS_VOICE="${OPENAI_TTS_VOICE:-Sofia}"
    # MagpieTTS exige un language_code real: "auto" -> "Auto" devuelve 400.
    # Códigos soportados: en-US, es-ES, de-DE, fr-FR, it-IT, vi-VN, hi-IN,
    # ar-AE, ar-SA, ar-MSA, ko-KR, pt-BR. El prompt del sistema responde en
    # español, así que por defecto es-ES (sin code-switching dentro de frase);
    # exporta OPENAI_TTS_LANGUAGE=en-US (o cámbialo aquí) para voz en inglés.
    export OPENAI_TTS_LANGUAGE="${OPENAI_TTS_LANGUAGE:-es-ES}"
    # NanoCodec emite 22050 Hz. Sin esto el audio suena ~9% rápido, porque el
    # proveedor asumía los 24000 Hz de Qwen3-TTS.
    export OPENAI_TTS_SAMPLE_RATE="${OPENAI_TTS_SAMPLE_RATE:-22050}"
    # NeMo-Speech.cpp hace su propio warmup; no hace falta el de ~90 s del NIM.
    export OPENAI_TTS_WARMUP="${OPENAI_TTS_WARMUP:-0}"
    export OPENAI_TTS_TIMEOUT_MS="${OPENAI_TTS_TIMEOUT_MS:-30000}"
    echo "TTS: NeMo-Speech.cpp (MagpieTTS) en ${OPENAI_TTS_URL} (voice=${OPENAI_TTS_VOICE} lang=${OPENAI_TTS_LANGUAGE} rate=${OPENAI_TTS_SAMPLE_RATE})"
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
