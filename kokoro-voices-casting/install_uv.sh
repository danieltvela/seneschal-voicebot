#!/usr/bin/env bash
# Descarga uv (gestor de paquetes Python pedido por Daniel) DENTRO del proyecto.
# No toca nada fuera de este directorio salvo la caché estándar de uv.
set -euo pipefail
cd "$(dirname "$0")"

if command -v uv >/dev/null 2>&1; then
  echo "uv ya está en el PATH: $(command -v uv)"
  exit 0
fi

if [[ -x bin/uv ]]; then
  echo "uv local ya existe: $(pwd)/bin/uv"
  exit 0
fi

ARCH=$(uname -m)
case "$ARCH" in
  x86_64)  TARGET="x86_64-unknown-linux-gnu" ;;
  aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
  *) echo "Arquitectura no soportada: $ARCH" >&2; exit 1 ;;
esac

URL="https://github.com/astral-sh/uv/releases/latest/download/uv-${TARGET}.tar.gz"
echo "Descargando uv -> bin/  ($URL)"
mkdir -p bin
TMP=$(mktemp -d)
curl -fsSL "$URL" -o "$TMP/uv.tar.gz"
tar -xzf "$TMP/uv.tar.gz" -C "$TMP"
cp "$TMP/uv-${TARGET}/uv" bin/uv
chmod +x bin/uv
rm -rf "$TMP"
echo "OK: $(./bin/uv --version)"
echo 'Añade este directorio al PATH o usa ./bin/uv (casting.sh lo hace solo).'
