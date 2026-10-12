#!/usr/bin/env bash
# Frontend del casting: ./casting.sh <list|say|battle|mix|best|all> [...]
# Usa uv local si existe, si no el del PATH; nunca el pip del sistema.
set -euo pipefail
cd "$(dirname "$0")"

if command -v uv >/dev/null 2>&1; then
  UV=uv
elif [[ -x bin/uv ]]; then
  UV=./bin/uv
else
  echo "Falta uv. Ejecuta primero: ./setup.sh" >&2
  exit 1
fi

if [[ ! -d .venv ]]; then
  echo "Falta el entorno. Ejecuta primero: ./setup.sh" >&2
  exit 1
fi

exec "$UV" run python cast.py "$@"
