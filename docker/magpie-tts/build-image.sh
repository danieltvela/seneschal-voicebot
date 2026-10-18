#!/usr/bin/env bash
# Build a local image of NVIDIA/NeMo-Speech.cpp (Apache-2.0) that serves
# Magpie TTS Multilingual over the OpenAI-compatible HTTP API (/v1/audio/speech).
#
# Sin NGC: la imagen base CUDA se toma de Docker Hub (nvidia/cuda) y los pesos
# de Hugging Face. El Dockerfile oficial del proyecto se usa tal cual; solo se
# reescribe la base nvcr.io/nvidia/cuda -> nvidia/cuda.
#
# Uso:
#   magpie-tts/build-image.sh              # construye y etiqueta en local
#   PUSH=1 magpie-tts/build-image.sh       # ademas publica en el registro local
#
# Variables:
#   REF        rama/tag de NeMo-Speech.cpp (def. main)
#   BUILD_DIR  directorio donde clonar el proyecto (def. ${XDG_CACHE_HOME:-$HOME/.cache}/nemo-speech-cpp)
#   IMAGE      nombre:tag de la imagen      (def. magpie-tts:latest)
#   REGISTRY   registro local si PUSH=1     (def. localhost:5000)
set -euo pipefail

REPO_URL="${REPO_URL:-https://github.com/NVIDIA/NeMo-Speech.cpp.git}"
REF="${REF:-main}"
BUILD_DIR="${BUILD_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/nemo-speech-cpp}"
IMAGE="${IMAGE:-magpie-tts:latest}"
BUILDER_IMAGE="${BUILDER_IMAGE:-magpie-tts-builder:latest}"
CUDA_VERSION="${CUDA_VERSION:-13.0.3}"
UBUNTU_VERSION="${UBUNTU_VERSION:-24.04}"
CUDA_ARCH="${CUDA_ARCH:-120}"       # RTX PRO 6000 Blackwell => sm_120 (SASS nativo)
REGISTRY="${REGISTRY:-localhost:5000}"

echo ">> Clonando/cargando ${REPO_URL} (${REF}) en ${BUILD_DIR}"
mkdir -p "$(dirname "$BUILD_DIR")"
if [ ! -d "$BUILD_DIR/.git" ]; then
  git clone --branch "$REF" --depth 1 "$REPO_URL" "$BUILD_DIR"
else
  git -C "$BUILD_DIR" fetch --depth 1 origin "$REF"
  git -C "$BUILD_DIR" checkout -q FETCH_HEAD
fi
git -C "$BUILD_DIR" submodule update --init --depth 1 llama.cpp third_party/cpp-httplib
# El Dockerfile upstream instala las licencias de flashlight-text y kenlm de
# forma incondicional (aunque ENABLE_FLASHLIGHT=OFF): inicializa los submodulos
# para que existan esos LICENSE/COPYING.
git -C "$BUILD_DIR" submodule update --init --depth 1 third_party/flashlight-text third_party/kenlm

echo ">> Base CUDA desde Docker Hub (sin nvcr.io)"
sed -i 's#nvcr.io/nvidia/cuda#nvidia/cuda#g' "$BUILD_DIR/docker/Dockerfile"

# Solo TTS + HTTP: sin gRPC/normalizacion/Flashlight/NMT/VoiceChat (menos tiempo
# de build). TTS se compila por defecto (NEMO_SPEECH_BUILD_TTS=ON).
BUILD_ARGS=(
  --build-arg "CUDA_VERSION=${CUDA_VERSION}"
  --build-arg "UBUNTU_VERSION=${UBUNTU_VERSION}"
  --build-arg "CUDA_ARCH=${CUDA_ARCH}"
  --build-arg "JOBS=$(nproc)"
  --build-arg "ENABLE_GRPC=OFF"
  --build-arg "ENABLE_NORM=OFF"
  --build-arg "ENABLE_FLASHLIGHT=OFF"
  --build-arg "ENABLE_NMT=OFF"
  --build-arg "ENABLE_S2S=OFF"
)

echo ">> Construyendo imagen runtime ${IMAGE} (puede tardar varios minutos)"
docker build -f "$BUILD_DIR/docker/Dockerfile" --target runtime \
  "${BUILD_ARGS[@]}" -t "$IMAGE" "$BUILD_DIR"

echo ">> Construyendo imagen builder ${BUILDER_IMAGE} (para descargar pesos)"
docker build -f "$BUILD_DIR/docker/Dockerfile" --target builder \
  "${BUILD_ARGS[@]}" -t "$BUILDER_IMAGE" "$BUILD_DIR"

if [ "${PUSH:-0}" = "1" ]; then
  echo ">> Publicando en ${REGISTRY}/$(basename "${IMAGE%%:*}")"
  docker tag "$IMAGE" "${REGISTRY}/$(basename "${IMAGE%%:*}")${IMAGE##*:}"
  docker push "${REGISTRY}/$(basename "${IMAGE%%:*}")${IMAGE##*:}"
fi

echo ">> Listo: $IMAGE"
