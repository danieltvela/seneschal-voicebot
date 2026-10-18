#!/usr/bin/env bash
# Descarga los pesos de Magpie TTS Multilingual (357M) + NanoCodec desde
# Hugging Face usando el propio CLI del proyecto (nemo-speech pull magpie),
# que ademas extrae el tokenizer del archivo .nemo y verifica SHA-256.
#
# Requiere haber construido antes la imagen builder: magpie-tts/build-image.sh
# No usa NGC.
set -euo pipefail

BUILDER_IMAGE="${BUILDER_IMAGE:-magpie-tts-builder:latest}"
MODELS_DIR="${MAGPIE_TTS_MODEL_DIR:-${MODELS_DIR:-}}"
if [ -z "$MODELS_DIR" ]; then
  echo "error: set MAGPIE_TTS_MODEL_DIR (or MODELS_DIR); see docker/.env.example" >&2
  exit 1
fi

mkdir -p "$MODELS_DIR"

echo ">> Descargando pesos a ${MODELS_DIR}"
docker run --rm \
  --gpus all \
  -u "$(id -u):$(id -g)" \
  -v "$MODELS_DIR":/models \
  -e NEMO_SPEECH_MODEL_DIR=/models \
  "$BUILDER_IMAGE" /work/build/bin/nemo-speech pull magpie

# Enlaces estables (relativos) para no fijar el hash de revision en el compose.
MAGPIE_GGUF="$(find "$MODELS_DIR" -name '*magpie_tts_multilingual*.f16.gguf' -type f | head -1)"
CODEC_GGUF="$(find "$MODELS_DIR" -name '*decoder.f16.gguf' -type f | head -1)"
TOKENIZER_DIR="$(find "$MODELS_DIR" -type d -name tokenizer | head -1)"
ln -sfn "${MAGPIE_GGUF#"$MODELS_DIR"/}" "$MODELS_DIR/magpie.gguf"
ln -sfn "${CODEC_GGUF#"$MODELS_DIR"/}" "$MODELS_DIR/codec.gguf"
ln -sfn "${TOKENIZER_DIR#"$MODELS_DIR"/}" "$MODELS_DIR/tokenizer"

echo ">> Contenido:"
find "$MODELS_DIR" -maxdepth 3 -type f -printf '%p\t%s bytes\n' | sort
