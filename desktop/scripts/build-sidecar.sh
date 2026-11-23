#!/usr/bin/env bash
# Build the real Seneschal binary and stage it as a Tauri sidecar.
#
# Tauri appends the host target triple to every entry in `bundle.externalBin`.
# Override backend features on platforms that need them, for example:
#   SENESCHAL_SIDECAR_FEATURES="control,avspeech" npm run sidecar:build
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${DESKTOP_DIR}/.." && pwd)"
BINARIES_DIR="${DESKTOP_DIR}/src-tauri/binaries"

if [[ -n "${SENESCHAL_SIDECAR_FEATURES:-}" ]]; then
    FEATURES="${SENESCHAL_SIDECAR_FEATURES}"
elif [[ "$(uname -s)" = "Darwin" ]]; then
    FEATURES="control,avspeech"
else
    FEATURES="control,kokoro"
fi
TARGET_TRIPLE="$(rustc -vV | awk '/host:/ {print $2}')"
if [[ -z "${TARGET_TRIPLE}" ]]; then
    echo "could not determine the rustc host target triple" >&2
    exit 1
fi

mkdir -p "${BINARIES_DIR}"

echo "Building Seneschal sidecar (features: ${FEATURES})..."
(
    cd "${REPO_ROOT}"
    cargo build --release --features "${FEATURES}"
)

SIDECAR="${BINARIES_DIR}/seneschal-${TARGET_TRIPLE}"
cp "${REPO_ROOT}/target/release/seneschal" "${SIDECAR}"
chmod +x "${SIDECAR}"
echo "Sidecar ready: ${SIDECAR}"
