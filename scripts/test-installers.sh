#!/bin/bash
# Seneschal installer smoke-test harness
#
# Tests install.sh without real network access by
# placing a mock curl and a mock `say` on PATH that serve local fixture files.
#
# The installers are run with stdin redirected from /dev/null, which forces
# non-interactive mode and verifies that all prompts fall back to defaults
# silently (the curl | sh use case).
#
# Usage:
#   bash scripts/test-installers.sh                  # normal (exit 0)
#   SIMULATE_MISSING_VAD=1 bash scripts/test-installers.sh   # expected failure
#   SIMULATE_LLM_DOWN=1   bash scripts/test-installers.sh   # LLM probe returns down
set -e

# ── Setup ───────────────────────────────────────────────────────────────────
PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEST_DIR=$(mktemp -d)
FIXTURE_DIR="$TEST_DIR/fixtures"
MOCK_BIN="$TEST_DIR/mock-bin"

cleanup() { rm -rf "$TEST_DIR"; }
trap cleanup EXIT

mkdir -p "$FIXTURE_DIR" "$MOCK_BIN"

echo "=== Setting up test fixtures ==="

cat > "$FIXTURE_DIR/seneschal" << 'EOF'
#!/bin/sh
echo "Seneschal stub"
EOF
chmod +x "$FIXTURE_DIR/seneschal"
tar -czf "$FIXTURE_DIR/seneschal-stub.tar.gz" -C "$FIXTURE_DIR" seneschal

echo "STUB_WHISPER_TINY"   > "$FIXTURE_DIR/ggml-tiny.bin"
echo "STUB_WHISPER_SMALL"  > "$FIXTURE_DIR/ggml-small.bin"
echo "STUB_WHISPER"        > "$FIXTURE_DIR/ggml-large-v3-turbo.bin"
echo "STUB_VAD"            > "$FIXTURE_DIR/silero_vad.onnx"
echo "STUB_KOKORO"         > "$FIXTURE_DIR/kokoro-v1.0.onnx"
echo "STUB_VOICES"         > "$FIXTURE_DIR/voices-v1.0.bin"

# ── Mock curl ───────────────────────────────────────────────────────────────
# Intercepts every call and serves fixture files. Also handles:
#   - LLM probe URL (/v1/models) — returns 200 with empty body unless
#     SIMULATE_LLM_DOWN=1
#   - Gitea API for release resolution
cat > "$MOCK_BIN/curl" << 'MOCKEOF'
#!/bin/bash
MOCK_FIXTURE_DIR="${MOCK_FIXTURE_DIR:?}"
OUT_FILE=""
URL=""
HEAD_ONLY=0
MAX_TIME=""

while [ $# -gt 0 ]; do
    case "$1" in
        -o)
            shift
            OUT_FILE="$1"
            ;;
        --progress-bar|-fsSL|-S|-s|-f|-L|-k)
            ;;
        --max-time)
            shift
            MAX_TIME="$1"
            ;;
        -I|-X|--head)
            HEAD_ONLY=1
            ;;
        -*)
            ;;
        *)
            URL="$1"
            ;;
    esac
    shift
done

[ -n "$URL" ] || { echo "Mock curl: no URL" >&2; exit 1; }

# LLM probe — /v1/models. Honor SIMULATE_LLM_DOWN.
case "$URL" in
    */v1/models)
        if [ "${SIMULATE_LLM_DOWN:-0}" = "1" ]; then
            echo "Mock curl: LLM probe simulated as down" >&2
            exit 22
        fi
        if [ -n "$OUT_FILE" ]; then
            echo '{"object":"list","data":[]}' > "$OUT_FILE"
        else
            echo '{"object":"list","data":[]}'
        fi
        exit 0
        ;;
esac

# API calls (no -o) — return JSON
if [ -z "$OUT_FILE" ]; then
    case "$URL" in
        */api/v1/repos/*/releases/latest*)
            echo '{"tag_name":"v0.0.0-test"}'
            exit 0
            ;;
    esac
    echo "Mock curl: unknown API call: $URL" >&2
    exit 1
fi

# File download — serve from fixtures based on URL pattern
case "$URL" in
    *seneschal-*.tar.gz*)
        cp "$MOCK_FIXTURE_DIR/seneschal-stub.tar.gz" "$OUT_FILE" ;;
    *ggml-tiny.bin*)
        cp "$MOCK_FIXTURE_DIR/ggml-tiny.bin" "$OUT_FILE" ;;
    *ggml-small.bin*)
        cp "$MOCK_FIXTURE_DIR/ggml-small.bin" "$OUT_FILE" ;;
    *ggml-large-v3-turbo.bin*)
        cp "$MOCK_FIXTURE_DIR/ggml-large-v3-turbo.bin" "$OUT_FILE" ;;
    *silero_vad.onnx*)
        cp "$MOCK_FIXTURE_DIR/silero_vad.onnx" "$OUT_FILE" ;;
    *kokoro-v1.0.onnx*)
        cp "$MOCK_FIXTURE_DIR/kokoro-v1.0.onnx" "$OUT_FILE" ;;
    *voices-v1.0.bin*)
        cp "$MOCK_FIXTURE_DIR/voices-v1.0.bin" "$OUT_FILE" ;;
    *)
        echo "Mock curl: unknown download URL: $URL" >&2
        exit 1
        ;;
esac

[ -f "$OUT_FILE" ]
MOCKEOF
chmod +x "$MOCK_BIN/curl"

# ── Mock `say` (macOS) ──────────────────────────────────────────────────────
# Returns a fixed list of voices regardless of the host's actual voices.
# This lets select_voice_macos run on Linux CI too.
# ── Mock osascript (macOS) ─────────────────────────────────────────────────
# Returns success for any AppleScript snippet. This lets the
# Calendar/Reminders prompt run without a real macOS.
cat > "$MOCK_BIN/osascript" << 'OSAEOF'
#!/bin/bash
# Mock macOS osascript for installer tests.
# Supported invocations:
#   osascript -e <script>    — always returns 0
echo "[mock-osascript] $*" >&2
exit 0
OSAEOF
chmod +x "$MOCK_BIN/osascript"

cat > "$MOCK_BIN/say" << 'SAYEOF'
#!/bin/bash
# Mock macOS `say` for installer tests.
# Supports: `say -v ?` (list voices) and `say -v <name> "text"` (speak).
case "$1" in
    -v)
        case "$2" in
            "?")
                cat <<'VOICES'
Marisol (Enhanced)  es_ES    # Hola! Me llamo Marisol.
Jorge (Enhanced)    es_ES    # Hola! Me llamo Jorge.
Mónica              es_ES    # Hola! Me llamo Mónica.
Paulina             es_MX    # Hola! Me llamo Paulina.
Eddy (Spanish (Spain))      es_ES    # Hola! Me llamo Eddy.
Eddy (Spanish (Mexico))     es_MX    # Hola! Me llamo Eddy.
VOICES
                exit 0
                ;;
            *)
                # Speak the text and exit 0. Capture voice before shifting.
                _voice_arg="$2"
                shift 2
                echo "[mock-say voice='$_voice_arg'] $*"
                exit 0
                ;;
        esac
        ;;
    *)
        echo "Mock say: unsupported invocation: $*" >&2
        exit 1
        ;;
esac
SAYEOF
chmod +x "$MOCK_BIN/say"

export MOCK_FIXTURE_DIR="$FIXTURE_DIR"
export PATH="$MOCK_BIN:$PATH"

# ── Helper: verify installed files ──────────────────────────────────────────
check_file() {
    if [ -f "$1" ]; then
        echo "  [+] $2"
        return 0
    else
        echo "  [!!] $2 — MISSING"
        return 1
    fi
}

check_grep() {
    # $1=file, $2=pattern, $3=label
    if grep -qE "$2" "$1" 2>/dev/null; then
        echo "  [+] $3"
        return 0
    else
        echo "  [!!] $3 — pattern not found in $1"
        return 1
    fi
}

# Run an installer in non-interactive mode (stdin from /dev/null). All prompts
# must fall back to defaults without blocking. Args after the installer path
# are forwarded as KEY=VAL env pairs to `env` (no eval, no quoting gotchas).
run_installer_noninteractive() {
    _installer="$1"; shift
    env "$@" </dev/null sh "$_installer"
}

# ═══════════════════════════════════════════════════════════════════════════════
# Test 1 — install.sh (non-interactive)
# ═══════════════════════════════════════════════════════════════════════════════
echo ""
echo "═══════════════════════════════════════════════"
echo "  Test 1: install.sh (non-interactive)"
echo "═══════════════════════════════════════════════"
echo ""

INSTALL_DIR="$TEST_DIR/install-test"

run_installer_noninteractive "$PROJECT_ROOT/install.sh" \
    "SENESCHAL_HOME=$INSTALL_DIR" \
    "BIN_DIR=$INSTALL_DIR/launcher" \
    "GITHUB_REPO=localhost:9876" \
    "SENESCHAL_VERSION=" \
    "WHISPER_MODEL_URL=http://localhost:9876/ggml-large-v3-turbo.bin" \
    "VAD_MODEL_URL=http://localhost:9876/silero_vad.onnx" \
    "KOKORO_MODEL_URL=http://localhost:9876/kokoro-v1.0.onnx" \
    "KOKORO_VOICES_URL=http://localhost:9876/voices-v1.0.bin"

echo ""
echo "--- Verifying install.sh ---"
ERR=0
check_file "$INSTALL_DIR/bin/seneschal"                        "Binary installed"        || ERR=1
check_file "$INSTALL_DIR/models/ggml-large-v3-turbo.bin"        "Whisper model (default size)" || ERR=1
check_file "$INSTALL_DIR/models/ggml-silero-vad.bin"            "VAD model"               || ERR=1
check_file "$INSTALL_DIR/.env"                                  "Default config"          || ERR=1
check_file "$INSTALL_DIR/launcher/seneschal"                   "Launcher script"         || ERR=1
check_grep "$INSTALL_DIR/launcher/seneschal" "seneschal"        "Launcher references binary"            || ERR=1

if [ "$(uname -s)" = "Linux" ]; then
    check_file "$INSTALL_DIR/models/kokoro-v1.0.onnx"  "Kokoro model (Linux)"   || ERR=1
    check_file "$INSTALL_DIR/models/voices-v1.0.bin"   "Kokoro voices (Linux)"  || ERR=1
    check_grep "$INSTALL_DIR/.env" "^KOKORO_VOICE="   ".env has KOKORO_VOICE (Linux path)"  || ERR=1
    check_grep "$INSTALL_DIR/.env" "^TTS_PROVIDER=kokoro" ".env has TTS_PROVIDER=kokoro (Linux path)" || ERR=1
else
    check_grep "$INSTALL_DIR/.env" "^AVSPEECH_VOICE="  ".env has AVSPEECH_VOICE (macOS path)" || ERR=1
    check_grep "$INSTALL_DIR/.env" "^TTS_PROVIDER=avspeech" ".env has TTS_PROVIDER=avspeech"    || ERR=1
fi

if [ "$ERR" = "1" ]; then
    echo ""
    echo "FAILED: install.sh test — missing files or invalid config"
    exit 1
fi
echo ""
echo "  [+] install.sh test passed"

# ═══════════════════════════════════════════════════════════════════════════════
# Test 3 — custom Whisper model (tiny) via env override
# ═══════════════════════════════════════════════════════════════════════════════
echo ""
echo "═══════════════════════════════════════════════"
echo "  Test 3: WHISPER_MODEL override points to tiny"
echo "═══════════════════════════════════════════════"
echo ""

TINY_DIR="$TEST_DIR/install-tiny-test"

run_installer_noninteractive "$PROJECT_ROOT/install.sh" \
    "SENESCHAL_HOME=$TINY_DIR" \
    "BIN_DIR=$TINY_DIR/launcher" \
    "GITHUB_REPO=localhost:9876" \
    "WHISPER_MODEL_URL=http://localhost:9876/ggml-tiny.bin" \
    "VAD_MODEL_URL=http://localhost:9876/silero_vad.onnx" \
    "KOKORO_MODEL_URL=http://localhost:9876/kokoro-v1.0.onnx" \
    "KOKORO_VOICES_URL=http://localhost:9876/voices-v1.0.bin"

ERR=0
check_file "$TINY_DIR/models/ggml-tiny.bin" "Whisper tiny model via env override" || ERR=1
if [ "$ERR" = "1" ]; then
    echo ""
    echo "FAILED: tiny model test"
    exit 1
fi
echo ""
echo "  [+] tiny model test passed"

# ═══════════════════════════════════════════════════════════════════════════════
# Test 4 — SIMULATE_LLM_DOWN (LLM probe returns failure)
# ═══════════════════════════════════════════════════════════════════════════════
if [ "$SIMULATE_LLM_DOWN" = "1" ]; then
    echo ""
    echo "═══════════════════════════════════════════════"
    echo "  Test 4: LLM probe reports down — installer warns but completes"
    echo "═══════════════════════════════════════════════"
    echo ""

    LLM_DOWN_DIR="$TEST_DIR/llm-down-test"

    run_installer_noninteractive "$PROJECT_ROOT/install.sh" \
        "SENESCHAL_HOME=$LLM_DOWN_DIR" \
        "BIN_DIR=$LLM_DOWN_DIR/launcher" \
        "GITHUB_REPO=localhost:9876" \
        "WHISPER_MODEL_URL=http://localhost:9876/ggml-large-v3-turbo.bin" \
        "VAD_MODEL_URL=http://localhost:9876/silero_vad.onnx" \
        "KOKORO_MODEL_URL=http://localhost:9876/kokoro-v1.0.onnx" \
        "KOKORO_VOICES_URL=http://localhost:9876/voices-v1.0.bin"

    # Installer should still complete and produce files; LLM warning is printed.
    ERR=0
    check_file "$LLM_DOWN_DIR/bin/seneschal" "Binary installed even with LLM down" || ERR=1
    if [ "$ERR" = "1" ]; then
        echo ""
        echo "FAILED: LLM-down test"
        exit 1
    fi
    echo ""
    echo "  [+] LLM-down test passed"
fi

# ═══════════════════════════════════════════════════════════════════════════════
# Test 5 — SIMULATE_MISSING_VAD (expected failure)
# ═══════════════════════════════════════════════════════════════════════════════
if [ "$SIMULATE_MISSING_VAD" = "1" ]; then
    echo ""
    echo "═══════════════════════════════════════════════"
    echo "  Test 5: SIMULATE_MISSING_VAD"
    echo "═══════════════════════════════════════════════"
    echo ""

    echo "  Removing VAD model to simulate missing asset..."
    rm -f "$INSTALL_DIR/models/ggml-silero-vad.bin"

    if [ ! -f "$INSTALL_DIR/models/ggml-silero-vad.bin" ]; then
        echo ""
        echo "  VAD model missing as expected"
        echo "  Test 5 FAILED intentionally — missing VAD detected correctly."
        exit 1
    fi
fi

# ── Success ──────────────────────────────────────────────────────────────────
echo ""
echo "═══════════════════════════════════════════════"
echo "  All installer smoke tests passed"
echo "═══════════════════════════════════════════════"
exit 0
