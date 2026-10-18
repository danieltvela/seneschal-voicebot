#!/usr/bin/env bash
# Descarga los pesos de NVIDIA Nemotron 3.5 ASR Streaming 0.6B (q8_0) desde
# Hugging Face usando el CLI del propio proyecto (nemo-speech pull nemotron-3.5),
# que verifica tamaño y SHA-256. Alimenta el servicio ASR independiente
# `nemotron-asr` (Plan A); el TTS lo sirve magpie-tts (magpie-tts/download-weights.sh).
#
# Requiere haber construido antes la imagen builder: magpie-tts/build-image.sh
set -euo pipefail

BUILDER_IMAGE="${BUILDER_IMAGE:-magpie-tts-builder:latest}"
MODELS_DIR="${NEMOTRON_ASR_MODEL_DIR:-${MODELS_DIR:-}}"
if [ -z "$MODELS_DIR" ]; then
  echo "error: set NEMOTRON_ASR_MODEL_DIR (or MODELS_DIR); see docker/.env.example" >&2
  exit 1
fi

mkdir -p "$MODELS_DIR"

echo ">> Descargando pesos ASR (Nemotron 3.5) a ${MODELS_DIR}"
docker run --rm \
  --gpus all \
  -u "$(id -u):$(id -g)" \
  -v "$MODELS_DIR":/models \
  -e NEMO_SPEECH_MODEL_DIR=/models \
  "$BUILDER_IMAGE" /work/build/bin/nemo-speech pull nemotron-3.5

# Enlace estable (relativo) para no fijar el hash de revision en asr.yaml.
ASR_GGUF="$(find "$MODELS_DIR" -name 'nemotron-3.5-asr-streaming-0.6b.q8_0.gguf' -type f | head -1)"
ln -sfn "${ASR_GGUF#"$MODELS_DIR"/}" "$MODELS_DIR/asr.gguf"

echo ">> Contenido:"
find "$MODELS_DIR" -maxdepth 3 -type f -printf '%p\t%s bytes\n' | sort
