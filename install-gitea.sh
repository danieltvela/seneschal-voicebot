#!/bin/sh
# Voicebot installer — Gitea (local) edition
# Usage: curl -fsSL http://tesla.local:3000/danielvela/voicebot/releases/latest/download/install-gitea.sh | sh
#
# Environment overrides:
#   GITEA_URL         — Gitea base URL      (default: http://tesla.local:3000)
#   GITEA_REPO        — owner/repo          (default: danielvela/voicebot)
#   VOICEBOT_HOME     — install home dir    (default: ~/.voicebot)
#   BIN_DIR           — launcher directory  (default: ~/.local/bin)
#   VOICEBOT_VERSION  — pin a release tag   (default: latest)
#
# All installer logic (download, dependencies, model selection, voice picker,
# LLM probe, smoke test) lives in scripts/lib-installer-common.sh.
set -e

GITEA_URL="${GITEA_URL:-http://tesla.local:3000}"
GITEA_REPO="${GITEA_REPO:-danielvela/voicebot}"
VOICEBOT_VERSION="${VOICEBOT_VERSION:-latest}"

if [ -z "$VOICEBOT_VERSION" ]; then
    # Resolve latest via the Gitea API; curl mock in tests serves a fixture here.
    VOICEBOT_VERSION=$(curl -fsSL "${GITEA_URL}/api/v1/repos/${GITEA_REPO}/releases/latest" \
        | sed -n 's/.*"tag_name":"\([^"]*\)".*/\1/p')
fi

RELEASE_BASE="${GITEA_URL}/${GITEA_REPO}/releases/download/${VOICEBOT_VERSION}"

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

DEFAULT_TTS_PROVIDER="avspeech"
DEFAULT_TTS_VOICE="Marisol (Enhanced)"
DEFAULT_KOKORO_VOICE="es_xb"
DEFAULT_KOKORO_LANG="es"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=scripts/lib-installer-common.sh
. "$SCRIPT_DIR/scripts/lib-installer-common.sh"

main "$@"
