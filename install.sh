#!/bin/sh
# Voicebot installer — GitHub release edition
# Usage: curl -fsSL https://raw.githubusercontent.com/danieltvela/voicebot/main/install.sh | sh
#
# Environment overrides:
#   GITHUB_REPO       — GitHub owner/repo (default: danieltvela/voicebot)
#   VOICEBOT_HOME     — where models/data/config live (default: ~/.voicebot)
#   BIN_DIR           — where to place the `voicebot` launcher (default: ~/.local/bin)
#   VOICEBOT_VERSION  — pin a release tag, e.g. v1.2.0 (default: latest)
#
# All installer logic (download, dependencies, model selection, voice picker,
# LLM probe, smoke test) lives in scripts/lib-installer-common.sh.
set -e

# ── Caller contract for the common lib ──────────────────────────────────────
GITHUB_REPO="${GITHUB_REPO:-danieltvela/voicebot}"
VOICEBOT_VERSION="${VOICEBOT_VERSION:-latest}"

if [ "$VOICEBOT_VERSION" = "latest" ]; then
    RELEASE_BASE="https://github.com/${GITHUB_REPO}/releases/latest/download"
else
    RELEASE_BASE="https://github.com/${GITHUB_REPO}/releases/download/${VOICEBOT_VERSION}"
fi

case "$(uname -s)" in
    Darwin) PLATFORM="apple-darwin" ;;
    Linux)  PLATFORM="unknown-linux-gnu" ;;
    *)      PLATFORM="apple-darwin" ;;
esac
case "$(uname -m)" in
    x86_64)          ARCH_TRIPLE="x86_64" ;;
    arm64 | aarch64) ARCH_TRIPLE="aarch64" ;;
    *)               ARCH_TRIPLE="aarch64" ;;
esac
TARBALL="voicebot-${ARCH_TRIPLE}-${PLATFORM}.tar.gz"

# TTS defaults — overridden at runtime by select_voice_macos / select_voice_kokoro
DEFAULT_TTS_PROVIDER="avspeech"
DEFAULT_TTS_VOICE="Marisol (Enhanced)"
DEFAULT_KOKORO_VOICE="es_xb"
DEFAULT_KOKORO_LANG="es"

# Source the common library. The lib provides main() which runs the full flow.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=scripts/lib-installer-common.sh
. "$SCRIPT_DIR/scripts/lib-installer-common.sh"

main "$@"
