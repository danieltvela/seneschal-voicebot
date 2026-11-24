#!/usr/bin/env bash
# Build the real Seneschal binary and stage it as a Tauri sidecar.
#
# Tauri appends the host target triple to every entry in `bundle.externalBin`.
# An explicit target is useful for native CI runners; cross-compilation is not
# assumed by packaging:
#   npm run sidecar:build -- aarch64-apple-darwin
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
    # Native STT/TTS baseline for first-run onboarding (#36), plus OpenAI TTS
    # so provider switching stays first-class in Settings (#33).
    FEATURES="control,avspeech,speech,openai-tts"
else
    FEATURES="control,kokoro,openai-tts"
fi
TARGET_TRIPLE="${1:-${SENESCHAL_SIDECAR_TARGET:-}}"
if [[ -z "${TARGET_TRIPLE}" ]]; then
    TARGET_TRIPLE="$(rustc -vV | awk '/host:/ {print $2}')"
fi
if [[ -z "${TARGET_TRIPLE}" ]]; then
    echo "could not determine the rustc host target triple" >&2
    exit 1
fi

mkdir -p "${BINARIES_DIR}"

# espeak-rs-sys has a fixed internal path buffer. Keep Cargo's target directory
# short in CI with CARGO_TARGET_DIR to avoid truncated phoneme source paths.
TARGET_DIR="${CARGO_TARGET_DIR:-${REPO_ROOT}/target}"

echo "Building Seneschal sidecar for ${TARGET_TRIPLE} (features: ${FEATURES})..."
(
    cd "${REPO_ROOT}"
    cargo build --release --locked --target "${TARGET_TRIPLE}" --target-dir "${TARGET_DIR}" --features "${FEATURES}"
)

BUILT_SIDECAR="${TARGET_DIR}/${TARGET_TRIPLE}/release/seneschal"
SIDECAR="${BINARIES_DIR}/seneschal-${TARGET_TRIPLE}"
if [[ ! -f "${BUILT_SIDECAR}" ]]; then
    echo "sidecar binary was not produced at ${BUILT_SIDECAR}" >&2
    exit 1
fi
cp "${BUILT_SIDECAR}" "${SIDECAR}"
chmod +x "${SIDECAR}"

# Kokoro phonemization needs the espeak-ng data generated beside the sidecar.
# The Linux bundle resource root is injected as PIPER_ESPEAKNG_DATA_DIRECTORY.
if [[ "${FEATURES}" == *kokoro* ]]; then
    ESPEAK_DATA_SOURCE="$(
        find "${TARGET_DIR}/${TARGET_TRIPLE}/release/build" \
            -type d -path '*/out/share/espeak-ng-data' -printf '%T@ %p\n' 2>/dev/null |
            sort -n | tail -n 1 | cut -d' ' -f2-
    )"
    if [[ -n "${ESPEAK_DATA_SOURCE}" ]]; then
        rm -rf "${DESKTOP_DIR}/src-tauri/espeak-ng-data"
        cp -a "${ESPEAK_DATA_SOURCE}" "${DESKTOP_DIR}/src-tauri/espeak-ng-data"
        echo "espeak-ng-data staged: ${DESKTOP_DIR}/src-tauri/espeak-ng-data"
    else
        echo "warning: espeak-ng-data was not found for ${TARGET_TRIPLE}" >&2
    fi
fi

if command -v file >/dev/null 2>&1; then
    file "${SIDECAR}"
fi
echo "Sidecar ready: ${SIDECAR}"
