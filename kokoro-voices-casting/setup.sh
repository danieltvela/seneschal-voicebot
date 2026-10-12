#!/usr/bin/env bash
# Instalación completa: uv -> venv Python 3.12 -> deps -> modelo + voces.
# Uso:
#   ./setup.sh              # todo (modelo 327 MB + las 54 voces ~27 MB)
#   ./setup.sh --no-model   # solo entorno Python
#   ./setup.sh --langs e    # modelo + solo voces de lengua(s) dadas (p.ej. "e a")
set -euo pipefail
cd "$(dirname "$0")"

./install_uv.sh
UV=$(command -v uv || echo ./bin/uv)

echo "==> Creando entorno Python 3.12 (uv lo descarga si hace falta)"
"$UV" venv --python 3.12 .venv

echo "==> Instalando dependencias (kokoro 0.9.4, torch CPU, misaki[en], soundfile)"
"$UV" sync

echo "==> Verificando import"
"$UV" run python - <<'EOF'
import torch, kokoro, misaki, soundfile
from kokoro import KPipeline, KModel
print("kokoro OK | torch", torch.__version__, "| cuda disponible:", torch.cuda.is_available())
EOF

ARGS=("$@")
if [[ "${1:-}" == "--no-model" ]]; then
  echo "==> Sin modelo (--no-model). Ya puedes usar ./casting.sh."
  exit 0
fi

echo "==> Descargando modelo Kokoro-82M v1.0 (kokoro-v1_0.pth, ~327 MB)"
"$UV" run python download_voices.py --model

if [[ "${1:-}" == "--langs" ]]; then
  shift 1
  echo "==> Descargando voces de lenguas: $*"
  "$UV" run python download_voices.py --langs "$@"
else
  echo "==> Descargando las 54 voces (~27 MB en total)"
  "$UV" run python download_voices.py --all
fi

echo
echo "Instalación completa. Prueba:"
echo "  ./casting.sh list"
echo "  ./casting.sh say ef_dora"
