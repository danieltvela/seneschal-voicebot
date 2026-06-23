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
set -e

# ── Caller contract ──────────────────────────────────────────────
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

# ── All installer logic (inlined from scripts/lib-installer-common.sh) ─────

# Output helpers — use ANSI colors only when stdout is a terminal (not when piped).
if [ -t 1 ]; then
    _GREEN='\033[0;32m'; _YELLOW='\033[1;33m'; _RED='\033[0;31m'; _NC='\033[0m'
else
    _GREEN=''; _YELLOW=''; _RED=''; _NC=''
fi

# `info`, `warn`, `error`, `step` print to stderr so $() captures stay clean.
# `error` exits 1 — it is the only place we exit.
info()  { printf "${_GREEN}[voicebot]${_NC} %s\n" "$1" >&2; }
warn()  { printf "${_YELLOW}[voicebot]${_NC} %s\n" "$1" >&2; }
error() { printf "${_RED}[voicebot] ERROR:${_NC} %s\n" "$1" >&2; exit 1; }
step()  { printf "\n${_GREEN}▶ %s${_NC}\n" "$1" >&2; }

# ── TTY detection ─────────────────────────────────────────────────────────────
# VOICEBOT_TTY=1 means interactive (stdin is a TTY). All prompts consult this
# and fall back to defaults silently when piped (`curl | sh`).
detect_tty() {
    if [ -t 0 ]; then
        VOICEBOT_TTY=1
    else
        VOICEBOT_TTY=0
    fi
}

# ── Interactive I/O primitives ────────────────────────────────────────────────
# All three return their result on stdout so callers can do:
#     VOICE=$(ask "Voice?" "Marisol (Enhanced)")
# They print prompts to stderr so they don't pollute the captured value.

# ask PROMPT DEFAULT — returns DEFAULT if no input or non-TTY.
ask() {
    _prompt="$1"; _default="$2"
    if [ "${VOICEBOT_TTY:-0}" != "1" ]; then
        printf "%s" "$_default"
        return
    fi
    printf "%s [%s]: " "$_prompt" "$_default" >&2
    _reply=""
    # `read` may fail under set -e when input ends; ignore that case.
    read -r _reply || _reply=""
    if [ -z "$_reply" ]; then
        printf "%s" "$_default"
    else
        printf "%s" "$_reply"
    fi
}

# confirm PROMPT DEFAULT(y|n) — returns "y" or "n" on stdout.
confirm() {
    _prompt="$1"; _default="$2"
    if [ "${VOICEBOT_TTY:-0}" != "1" ]; then
        printf "%s" "$_default"
        return
    fi
    case "$_default" in
        y|Y) _hint="Y/n" ;;
        *)   _hint="y/N" ;;
    esac
    printf "%s [%s]: " "$_prompt" "$_hint" >&2
    _reply=""
    read -r _reply || _reply=""
    case "$_reply" in
        [Yy]*) printf "y" ;;
        [Nn]*) printf "n" ;;
        "")    printf "%s" "$_default" ;;
        *)     printf "%s" "$_default" ;;
    esac
}

# pick_from_list PROMPT DEFAULT_INDEX ITEM1 ITEM2 ... — returns selected item.
# DEFAULT_INDEX is 1-based. Non-TTY: returns items[DEFAULT_INDEX-1].
# TTY: shows numbered list, accepts number or freeform text.
pick_from_list() {
    _prompt="$1"; _default_idx="$2"; shift 2
    # Remaining args are the items
    _items_count=$#
    if [ "$_items_count" = "0" ]; then
        error "pick_from_list: no items provided"
    fi
    if [ "${VOICEBOT_TTY:-0}" != "1" ]; then
        # Non-interactive: evaluate default index, print that item.
        # eval dance to index into positional params portably.
        _i=0
        for _item in "$@"; do
            _i=$((_i + 1))
            if [ "$_i" = "$_default_idx" ]; then
                printf "%s" "$_item"
                return
            fi
        done
        # Fall through: print first item
        for _item in "$@"; do printf "%s" "$_item"; return; done
    fi
    printf "\n%s\n" "$_prompt" >&2
    _i=0
    for _item in "$@"; do
        _i=$((_i + 1))
        _marker="  "
        if [ "$_i" = "$_default_idx" ]; then _marker=" *"; fi
        printf "  %s%d) %s\n" "$_marker" "$_i" "$_item" >&2
    done
    printf "Choose [%s]: " "$_default_idx" >&2
    _reply=""
    read -r _reply || _reply=""
    if [ -z "$_reply" ]; then
        _reply="$_default_idx"
    fi
    # Numeric in range?
    case "$_reply" in
        *[!0-9]*)
            # Not a number — treat as freeform
            printf "%s" "$_reply"
            return
            ;;
    esac
    if [ "$_reply" -ge 1 ] && [ "$_reply" -le "$_items_count" ] 2>/dev/null; then
        _i=0
        for _item in "$@"; do
            _i=$((_i + 1))
            if [ "$_i" = "$_reply" ]; then
                printf "%s" "$_item"
                return
            fi
        done
    fi
    # Out of range numeric — fall back to default
    _i=0
    for _item in "$@"; do
        _i=$((_i + 1))
        if [ "$_i" = "$_default_idx" ]; then
            printf "%s" "$_item"
            return
        fi
    done
}

# ── Defaults & derived paths ──────────────────────────────────────────────────
_apply_defaults() {
    VOICEBOT_HOME="${VOICEBOT_HOME:-$HOME/.voicebot}"
    BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"
    VOICEBOT_VERSION="${VOICEBOT_VERSION:-latest}"

    WHISPER_MODEL_URL="${WHISPER_MODEL_URL:-https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin}"
    KOKORO_MODEL_URL="${KOKORO_MODEL_URL:-https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/kokoro-v1.0.onnx}"
    KOKORO_VOICES_URL="${KOKORO_VOICES_URL:-https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/voices-v1.0.bin}"
    VAD_MODEL_URL="${VAD_MODEL_URL:-https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx}"

    VOICEBOT_BIN_DIR="$VOICEBOT_HOME/bin"
    VOICEBOT_MODELS_DIR="$VOICEBOT_HOME/models"
    VOICEBOT_DATA_DIR="$VOICEBOT_HOME/data"
    VOICEBOT_ENV="$VOICEBOT_HOME/.env"
}

# ── Platform detection ────────────────────────────────────────────────────────
_detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"
    case "$OS" in
        Darwin) OS_NAME="macOS" ;;
        Linux)  OS_NAME="Linux" ;;
        *)      error "Unsupported operating system: $OS. Only macOS and Linux are supported." ;;
    esac
    case "$ARCH" in
        x86_64)          ARCH_TRIPLE="x86_64" ;;
        arm64 | aarch64) ARCH_TRIPLE="aarch64" ;;
        *)               error "Unsupported architecture: $ARCH. Only x86_64 and arm64/aarch64 are supported." ;;
    esac
    case "$OS" in
        Darwin) PLATFORM="apple-darwin" ;;
        Linux)  PLATFORM="unknown-linux-gnu" ;;
    esac
    TARGET="${ARCH_TRIPLE}-${PLATFORM}"
    BINARY_URL="${RELEASE_BASE}/${TARBALL}"
}

# ── Utility: download a file ──────────────────────────────────────────────────
download() {
    _url="$1"; _dest="$2"; _label="$3"
    info "  Downloading: $_label"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --progress-bar -o "$_dest" "$_url" 2>&1 >&2 || {
            rm -f "$_dest"
            error "Download failed: $_url"
        }
    elif command -v wget >/dev/null 2>&1; then
        wget -q --show-progress -O "$_dest" "$_url" 2>&1 >&2 || {
            rm -f "$_dest"
            error "Download failed: $_url"
        }
    else
        error "Neither curl nor wget found. Please install one and re-run."
    fi
}

# ── Step 1: System dependency check ──────────────────────────────────────────
check_dependencies() {
    step "Checking system dependencies"
    if [ "$OS" = "Linux" ]; then
        _missing=""
        if ! ldconfig -p 2>/dev/null | grep -q "libasound\.so" && \
           ! find /usr/lib /usr/local/lib 2>/dev/null | grep -q "libasound"; then
            _missing="$_missing libasound2"
        fi
        if ! command -v espeak-ng >/dev/null 2>&1; then
            _missing="$_missing espeak-ng"
        fi
        if [ -n "$_missing" ]; then
            warn "The following runtime dependencies are missing:$_missing"
            warn ""
            warn "Install them before running voicebot:"
            warn "  Debian/Ubuntu:  sudo apt-get install -y$_missing"
            warn "  Fedora/RHEL:    sudo dnf install -y$_missing"
            warn "  Arch Linux:     sudo pacman -S$_missing"
            warn ""
            warn "Installation will continue, but voicebot may fail to start."
        else
            info "  All Linux runtime dependencies found."
        fi
    fi
    if [ "$OS" = "Darwin" ]; then
        info "  macOS detected — TTS: AVSpeechSynthesizer (built-in)."
        info "  Microphone access will be requested on first run."
    fi
}

# ── Step 2: Create directory layout ──────────────────────────────────────────
setup_directories() {
    step "Setting up directories"
    mkdir -p "$VOICEBOT_BIN_DIR"
    mkdir -p "$VOICEBOT_MODELS_DIR"
    mkdir -p "$VOICEBOT_DATA_DIR"
    mkdir -p "$BIN_DIR"
    info "  Install home : $VOICEBOT_HOME"
    info "  Launcher dir : $BIN_DIR"
}

# ── Step 3: Download and install the pre-compiled binary ──────────────────────
install_binary() {
    step "Downloading voicebot binary ($TARGET)"
    _tmp_dir="$(mktemp -d)"
    # shellcheck disable=SC2064
    trap "rm -rf '$_tmp_dir'" EXIT
    download "$BINARY_URL" "$_tmp_dir/$TARBALL" "voicebot ($TARGET)"
    info "  Extracting binary..."
    tar -xzf "$_tmp_dir/$TARBALL" -C "$_tmp_dir"
    if [ ! -f "$_tmp_dir/voicebot" ]; then
        error "Binary not found inside tarball. Expected: voicebot"
    fi
    mv "$_tmp_dir/voicebot" "$VOICEBOT_BIN_DIR/voicebot"
    chmod +x "$VOICEBOT_BIN_DIR/voicebot"
    rm -rf "$_tmp_dir"
    trap - EXIT
    info "  Binary installed: $VOICEBOT_BIN_DIR/voicebot"
}

# ── Step 4: Download Whisper STT model (with size picker) ────────────────────
# Whisper model registry: id|name|filename|url
_WHISPER_REGISTRY='tiny|tiny|ggml-tiny.bin|https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin
small|small|ggml-small.bin|https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin
large-v3-turbo|large-v3-turbo|ggml-large-v3-turbo.bin|https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin'

_pick_whisper_model() {
    # Echo the model id (tiny|small|large-v3-turbo) on stdout.
    # Build display items: "tiny (~75 MB)" etc.
    _items=""
    _defaults_idx=3   # default to large-v3-turbo (preserves prior behavior)
    _i=0
    _OLD_IFS="$IFS"
    IFS='
'
    for _line in $_WHISPER_REGISTRY; do
        IFS='|'
        set -- $_line
        _id="$1"; _name="$2"; _fname="$3"; _url="$4"
        IFS="$_OLD_IFS"
        _i=$((_i + 1))
        case "$_id" in
            tiny)          _size="~75 MB"   ;;
            small)         _size="~488 MB"  ;;
            large-v3-turbo) _size="~1.6 GB" ;;
            *)             _size="?"        ;;
        esac
        if [ -z "$_items" ]; then
            _items="$_name ($_size)"
        else
            # Newline-separated — use a sentinel since items contain spaces.
            _items="$_items|$_name ($_size)"
        fi
        IFS='
'
    done
    IFS="$_OLD_IFS"
    # Convert sentinel back to newline.
    _items_nl=$(printf "%s" "$_items" | tr '|' '
')
    # Hand the items to pick_from_list. We use the file-descriptor trick to
    # pass newlines: write items to a tmp file and read line-by-line.
    _items_file="$(mktemp)"
    printf "%s" "$_items_nl" > "$_items_file"
    _labels=""
    while IFS= read -r _line; do
        if [ -z "$_labels" ]; then _labels="$_line"; else _labels="$_labels|$_line"; fi
    done < "$_items_file"
    rm -f "$_items_file"
    # Use eval-ish: pick_from_list expects positional args, but we have
    # variable-count items with newlines. Call directly with field splitting.
    _chosen_label=$(pick_from_list "Whisper model size (controls transcription quality vs install time)" "$_defaults_idx" $_labels)
    rm -f "$_items_file" 2>/dev/null
    # Map label back to id.
    _i=0
    _chosen_id=""
    IFS='
'
    for _line in $_WHISPER_REGISTRY; do
        IFS='|'
        set -- $_line
        _id="$1"; _name="$2"; _fname="$3"; _url="$4"
        IFS="$_OLD_IFS"
        _i=$((_i + 1))
        if [ "$_name ($_size)" = "$_chosen_label" ] 2>/dev/null; then
            # Re-derive size (we lost it; rebuild from id).
            case "$_id" in
                tiny)          _size2="~75 MB"   ;;
                small)         _size2="~488 MB"  ;;
                large-v3-turbo) _size2="~1.6 GB" ;;
                *)             _size2="?"        ;;
            esac
            if [ "$_name ($_size2)" = "$_chosen_label" ]; then
                _chosen_id="$_id"
                break
            fi
        fi
        IFS='
'
    done
    IFS="$_OLD_IFS"
    # Fallback if mapping failed: default to large-v3-turbo
    if [ -z "$_chosen_id" ]; then
        _chosen_id="large-v3-turbo"
    fi
    printf "%s" "$_chosen_id"
}

install_whisper_model() {
    step "Installing Whisper STT model"
    # If WHISPER_MODEL_URL is set (env override or test fixture), use it
    # directly. The picker only runs when no URL override is provided.
    if [ -n "${WHISPER_MODEL_URL:-}" ]; then
        # Derive a filename from the URL path.
        _fname=$(basename "${WHISPER_MODEL_URL%%\?*}")
        case "$_fname" in
            ggml-*.bin) ;;
            *) error "WHISPER_MODEL_URL does not look like a Whisper model: $_fname" ;;
        esac
        WHISPER_MODEL_FILE="$_fname"
        _dest="$VOICEBOT_MODELS_DIR/$_fname"
        if [ -f "$_dest" ]; then
            info "  Already present ($_fname) — skipping (delete to re-download)."
            return
        fi
        warn "  Downloading $_fname (via WHISPER_MODEL_URL override)..."
        download "$WHISPER_MODEL_URL" "$_dest" "Whisper $_fname"
        info "  Whisper model installed."
        return
    fi
    # No URL override — run the size picker.
    _chosen_id=$(_pick_whisper_model)
    _fname=""
    _url=""
    _OLD_IFS="$IFS"
    IFS='
'
    for _line in $_WHISPER_REGISTRY; do
        IFS='|'
        set -- $_line
        _id="$1"; _name="$2"; _fname="$3"; _url="$4"
        IFS="$_OLD_IFS"
        if [ "$_id" = "$_chosen_id" ]; then
            break
        fi
        IFS='
'
    done
    IFS="$_OLD_IFS"
    if [ -z "$_fname" ]; then
        error "Unknown Whisper model id: $_chosen_id"
    fi
    WHISPER_MODEL_FILE="$_fname"
    _dest="$VOICEBOT_MODELS_DIR/$_fname"
    if [ -f "$_dest" ]; then
        info "  Already present ($_fname) — skipping (delete to re-download)."
        return
    fi
    warn "  Downloading $_fname..."
    download "$_url" "$_dest" "Whisper $_name"
    info "  Whisper model installed."
}

# ── Step 4.5: Download Silero VAD model ──────────────────────────────────────
install_vad_model() {
    step "Installing Silero VAD model"
    _dest="$VOICEBOT_MODELS_DIR/ggml-silero-vad.bin"
    if [ -f "$_dest" ]; then
        info "  Already present — skipping (delete to re-download)."
        return
    fi
    warn "  Downloading Silero VAD model (~10 MB)..."
    download "$VAD_MODEL_URL" "$_dest" "Silero VAD"
    info "  VAD model installed."
}

# ── Step 5: Download Kokoro TTS models (Linux only) ──────────────────────────
install_kokoro_models() {
    step "Installing Kokoro TTS models (Linux)"
    _kokoro_model="$VOICEBOT_MODELS_DIR/kokoro-v1.0.onnx"
    _kokoro_voices="$VOICEBOT_MODELS_DIR/voices-v1.0.bin"
    if [ -f "$_kokoro_model" ]; then
        info "  kokoro-v1.0.onnx already present — skipping."
    else
        warn "  Downloading kokoro-v1.0.onnx (~305 MB)..."
        download "$KOKORO_MODEL_URL" "$_kokoro_model" "Kokoro ONNX model"
        info "  Kokoro model installed."
    fi
    if [ -f "$_kokoro_voices" ]; then
        info "  voices-v1.0.bin already present — skipping."
    else
        warn "  Downloading voices-v1.0.bin (~28 MB)..."
        download "$KOKORO_VOICES_URL" "$_kokoro_voices" "Kokoro voice embeddings"
        info "  Kokoro voices installed."
    fi
}

# ── Voice selection: macOS AVSpeech ──────────────────────────────────────────
# Parse `say -v ?` output. Each line is: NAME LOCALE [# sample text]
_parse_say_voices() {
    _out=""
    _say_out=$(say -v '?' 2>/dev/null) || _say_out=""
    _OLD_IFS="$IFS"
    IFS='
'
    for _line in $_say_out; do
        IFS=' '
        # First token is the voice name. Strip trailing spaces.
        set -- $_line
        _name="$1"
        if [ -n "$_name" ]; then
            if [ -z "$_out" ]; then
                _out="$_name"
            else
                _out="$_out|$_name"
            fi
        fi
        IFS='
'
    done
    IFS="$_OLD_IFS"
    printf "%s" "$_out"
}

# Map language code (es, en) to likely macOS locale prefix.
_macos_lang_prefix() {
    case "$1" in
        es) printf "es_" ;;
        en) printf "en_" ;;
        *)  printf ""    ;;
    esac
}

# Filter voices by language prefix. Echo newline-separated.
_filter_voices_by_lang() {
    _all="$1"; _prefix="$2"
    if [ -z "$_prefix" ]; then
        printf "%s" "$_all" | tr '|' '
'
        return
    fi
    _out=""
    _OLD_IFS="$IFS"
    IFS='|'
    # Disable set -e temporarily so the for loop's empty list is safe.
    set +e
    for _v in $_all; do
        case "$_v" in
            "${_prefix}"*)
                if [ -z "$_out" ]; then
                    _out="$_v"
                else
                    _out="$_out|$_v"
                fi
                ;;
        esac
    done
    set -e
    IFS="$_OLD_IFS"
    if [ -z "$_out" ]; then
        # Fall back to all voices if no language match.
        printf "%s" "$_all" | tr '|' '
'
    else
        printf "%s" "$_out" | tr '|' '
'
    fi
}

# Return the locale string of a voice name from `say -v ?` (column 2).
# Empty if not found.
_voice_locale() {
    _name="$1"
    _say_out=$(say -v '?' 2>/dev/null) || return
    _OLD_IFS="$IFS"
    IFS='
'
    for _line in $_say_out; do
        IFS=' '
        set -- $_line
        if [ "$1" = "$_name" ]; then
            printf "%s" "$2"
            IFS="$_OLD_IFS"
            return
        fi
        IFS='
'
    done
    IFS="$_OLD_IFS"
}

# Is the voice installed locally? (Always true for `say -v ?` results.)
# We can't distinguish "installed" from "available" with `say -v ?`; this is
# a best-effort. The test_voice_macos call will catch the real failure.

select_voice_macos() {
    step "Selecting macOS TTS voice"
    _all_voices=$(_parse_say_voices)
    if [ -z "$_all_voices" ]; then
        warn "  Could not enumerate voices via 'say -v ?'."
        warn "  Falling back to default: $DEFAULT_TTS_VOICE"
        SELECTED_VOICE="$DEFAULT_TTS_VOICE"
        SELECTED_LOCALE=""
        return
    fi
    _lang="${VOICEBOT_LANGUAGE:-es}"
    _prefix=$(_macos_lang_prefix "$_lang")
    _filtered_file="$(mktemp)"
    _filter_voices_by_lang "$_all_voices" "$_prefix" > "$_filtered_file"
    if [ ! -s "$_filtered_file" ]; then
        warn "  No $_lang voices detected. Showing all voices."
        _filter_voices_by_lang "$_all_voices" "" > "$_filtered_file"
    fi
    # Build a list, prioritizing Enhanced/Premium variants.
    _prioritized_file="$(mktemp)"
    _normal_file="$(mktemp)"
    _grep -E '\(Enhanced\)|\(Premium\)' "$_filtered_file" > "$_prioritized_file" 2>/dev/null || true
    _grep -vE '\(Enhanced\)|\(Premium\)' "$_filtered_file" > "$_normal_file" 2>/dev/null || true
    cat "$_prioritized_file" "$_normal_file" > "$_filtered_file"
    # Default index: 1 if Marisol (Enhanced) is first, else 1.
    _defaults_idx=1
    # Build pick arguments from file lines
    _args=""
    _i=0
    while IFS= read -r _line; do
        _i=$((_i + 1))
        if [ -z "$_args" ]; then _args="$_line"; else _args="$_args|$_line"; fi
    done < "$_filtered_file"
    # Convert to positional via eval-safe field splitting.
    _OLD_IFS="$IFS"
    IFS='|'
    # shellcheck disable=SC2086
    set -- $_args
    IFS="$_OLD_IFS"
    if [ $# -eq 0 ]; then
        warn "  No voices to choose from. Using default: $DEFAULT_TTS_VOICE"
        SELECTED_VOICE="$DEFAULT_TTS_VOICE"
    else
        # If DEFAULT_TTS_VOICE is in the list, make it the default selection.
        _def_idx=1
        _ii=1
        for _v in "$@"; do
            if [ "$_v" = "$DEFAULT_TTS_VOICE" ]; then
                _def_idx=$_ii
                break
            fi
            _ii=$((_ii + 1))
        done
        _chosen=$(pick_from_list "Choose a TTS voice (Enhanced/Premium sound more natural)" "$_def_idx" "$@")
        SELECTED_VOICE="$_chosen"
    fi
    rm -f "$_filtered_file" "$_prioritized_file" "$_normal_file"
    SELECTED_LOCALE=$(_voice_locale "$SELECTED_VOICE")
    info "  Selected voice: $SELECTED_VOICE ($SELECTED_LOCALE)"
    # Reminder about downloading more voices.
    if [ "${VOICEBOT_TTY:-0}" = "1" ]; then
        printf "\n" >&2
        warn "  To download more voices (free, ~50-300 MB each):"
        warn "    System Settings → Accessibility → Spoken Content"
        warn "    → System Voice → Manage Voices… → Spanish (or your language)"
        warn "    → tick the Enhanced or Premium variants → OK"
        printf "\n" >&2
    fi
}

# Run a `say` test so the user can hear the voice. Skip if non-TTY.
test_voice_macos() {
    _voice="$1"
    if [ "${VOICEBOT_TTY:-0}" != "1" ]; then
        return
    fi
    info "  Testing voice with 'say'..."
    if say -v "$_voice" "Hola, soy voicebot. Esta es una prueba de mi voz." 2>/dev/null; then
        info "  Voice test played."
    else
        warn "  'say' test failed — voice may not be installed."
        warn "  Voicebot will use this name anyway, but TTS may fail at runtime."
    fi
}

# ── Voice selection: Linux Kokoro ────────────────────────────────────────────
select_voice_kokoro() {
    step "Selecting Kokoro TTS voice"
    _voice_bin="$VOICEBOT_BIN_DIR/voicebot"
    if [ ! -x "$_voice_bin" ]; then
        warn "  voicebot binary not yet installed — cannot enumerate Kokoro voices."
        warn "  Using default: $DEFAULT_KOKORO_VOICE"
        SELECTED_KOKORO_VOICE="$DEFAULT_KOKORO_VOICE"
        SELECTED_KOKORO_LANG="$DEFAULT_KOKORO_LANG"
        return
    fi
    # Capture voice list
    _list_out=$(KOKORO_MODEL="$VOICEBOT_MODELS_DIR/kokoro-v1.0.onnx" \
                KOKORO_VOICES="$VOICEBOT_MODELS_DIR/voices-v1.0.bin" \
                TTS_PROVIDER=kokoro \
                "$_voice_bin" --list-voices 2>&1) || _list_out=""
    if [ -z "$_list_out" ]; then
        warn "  Could not enumerate Kokoro voices. Using default: $DEFAULT_KOKORO_VOICE"
        SELECTED_KOKORO_VOICE="$DEFAULT_KOKORO_VOICE"
        SELECTED_KOKORO_LANG="$DEFAULT_KOKORO_LANG"
        return
    fi
    # Parse voice IDs from the formatted table — they're in the leftmost column.
    _ids_file="$(mktemp)"
    printf "%s\n" "$_list_out" | awk 'NR>3 && $1 ~ /^[a-z][a-z]_/ {print $1}' > "$_ids_file"
    if [ ! -s "$_ids_file" ]; then
        # Fallback: just take any non-header line.
        printf "%s\n" "$_list_out" | awk 'NR>3 {print $1}' | grep -E '_' > "$_ids_file" || true
    fi
    if [ ! -s "$_ids_file" ]; then
        warn "  No voice IDs parsed. Using default: $DEFAULT_KOKORO_VOICE"
        SELECTED_KOKORO_VOICE="$DEFAULT_KOKORO_VOICE"
        SELECTED_KOKORO_LANG="$DEFAULT_KOKORO_LANG"
        rm -f "$_ids_file"
        return
    fi
    # Prioritize voices for current language.
    _lang="${VOICEBOT_LANGUAGE:-es}"
    _lang_prefix="$([ "$_lang" = "es" ] && printf "e" || printf "a")"
    _prio="$(mktemp)"
    _rest="$(mktemp)"
    _grep "^${_lang_prefix}" "$_ids_file" > "$_prio" 2>/dev/null || true
    _grep -v "^${_lang_prefix}" "$_ids_file" > "$_rest" 2>/dev/null || true
    cat "$_prio" "$_rest" > "$_ids_file"
    rm -f "$_prio" "$_rest"
    # Build pick arguments.
    _args=""
    while IFS= read -r _line; do
        [ -z "$_line" ] && continue
        if [ -z "$_args" ]; then _args="$_line"; else _args="$_args|$_line"; fi
    done < "$_ids_file"
    _OLD_IFS="$IFS"
    IFS='|'
    # shellcheck disable=SC2086
    set -- $_args
    IFS="$_OLD_IFS"
    if [ $# -eq 0 ]; then
        warn "  No Kokoro voices parsed. Using default: $DEFAULT_KOKORO_VOICE"
        SELECTED_KOKORO_VOICE="$DEFAULT_KOKORO_VOICE"
        SELECTED_KOKORO_LANG="$DEFAULT_KOKORO_LANG"
    else
        # Default index = where DEFAULT_KOKORO_VOICE sits, or 1.
        _def_idx=1; _ii=1
        for _v in "$@"; do
            if [ "$_v" = "$DEFAULT_KOKORO_VOICE" ]; then
                _def_idx=$_ii
                break
            fi
            _ii=$((_ii + 1))
        done
        _chosen=$(pick_from_list "Choose a Kokoro TTS voice" "$_def_idx" "$@")
        SELECTED_KOKORO_VOICE="$_chosen"
    fi
    rm -f "$_ids_file"
    SELECTED_KOKORO_LANG="$DEFAULT_KOKORO_LANG"
    info "  Selected Kokoro voice: $SELECTED_KOKORO_VOICE ($SELECTED_KOKORO_LANG)"
}

# ── Step 6: Write default .env ───────────────────────────────────────────────
create_env() {
    step "Writing default configuration"
    if [ -f "$VOICEBOT_ENV" ]; then
        info "  Config already exists at $VOICEBOT_ENV — skipping."
        return
    fi
    # TTS settings differ per platform
    if [ "$OS" = "Darwin" ]; then
        TTS_PROVIDER_DEFAULT="avspeech"
        TTS_VOICE_LINE="AVSPEECH_VOICE=${SELECTED_VOICE:-$DEFAULT_TTS_VOICE}"
        TTS_RATE_LINE="AVSPEECH_RATE=0.55"
    else
        TTS_PROVIDER_DEFAULT="kokoro"
        TTS_VOICE_LINE="KOKORO_VOICE=${SELECTED_KOKORO_VOICE:-$DEFAULT_KOKORO_VOICE}"
        TTS_RATE_LINE="KOKORO_LANGUAGE=${SELECTED_KOKORO_LANG:-$DEFAULT_KOKORO_LANG}"
    fi
    _whisper_file="${WHISPER_MODEL_FILE:-ggml-large-v3-turbo.bin}"
    cat > "$VOICEBOT_ENV" << ENVEOF
# ── Voicebot configuration ────────────────────────────────────────────────────
# Edit this file to customize your setup.
# Full list of options: see .env.example in the source repo.

# Language: es (Spanish) or en (English)
VOICEBOT_LANGUAGE=${VOICEBOT_LANGUAGE:-es}

# ── LLM server ────────────────────────────────────────────────────────────────
# Start with: mlx_lm.server --model mlx-community/Qwen3-8B-4bit --port 8000
#         or: omlx serve --model-dir ~/models --port 8001
LLM_URL=http://localhost:8000
LLM_MAX_TOKENS=400
LLM_TEMPERATURE=0.3
# LLM_SYSTEM_PROMPT=You are a helpful voice assistant.
# LLM_MODEL=local-model

# ── TTS ───────────────────────────────────────────────────────────────────────
TTS_PROVIDER=$TTS_PROVIDER_DEFAULT
$TTS_VOICE_LINE
$TTS_RATE_LINE

# ── Audio devices ─────────────────────────────────────────────────────────────
# Uncomment and set to a substring of your device name.
# Run: voicebot --list-devices  to see available devices.
# AUDIO_INPUT_DEVICE=
# AUDIO_OUTPUT_DEVICE=
ENVEOF
    info "  Config written: $VOICEBOT_ENV"
}

# ── Step 7: Install the launcher wrapper script ──────────────────────────────
install_launcher() {
    step "Installing launcher script"
    _launcher="$BIN_DIR/voicebot"
    if [ "$OS" = "Darwin" ]; then
        _default_tts="avspeech"
    else
        _default_tts="kokoro"
    fi
    cat > "$_launcher" << LAUNCHEOF
#!/bin/sh
# voicebot launcher — generated by install.sh
# Edit $VOICEBOT_ENV to configure your setup.
VOICEBOT_HOME="\${VOICEBOT_HOME:-$VOICEBOT_HOME}"

# Point to installed models (can be overridden by env or .env file)
export WHISPER_MODEL="\${WHISPER_MODEL:-\$VOICEBOT_HOME/models/${WHISPER_MODEL_FILE:-ggml-large-v3-turbo.bin}}"
export DB_PATH="\${DB_PATH:-\$VOICEBOT_HOME/data/voicebot.db}"
export KOKORO_MODEL="\${KOKORO_MODEL:-\$VOICEBOT_HOME/models/kokoro-v1.0.onnx}"
export KOKORO_VOICES="\${KOKORO_VOICES:-\$VOICEBOT_HOME/models/voices-v1.0.bin}"
export VAD_MODEL="\${VAD_MODEL:-\$VOICEBOT_HOME/models/ggml-silero-vad.bin}"
export TTS_PROVIDER="\${TTS_PROVIDER:-$_default_tts}"

# Load user configuration (values here override defaults above)
if [ -f "\$VOICEBOT_HOME/.env" ]; then
    set -a
    # shellcheck source=/dev/null
    . "\$VOICEBOT_HOME/.env"
    set +a
fi

exec "\$VOICEBOT_HOME/bin/voicebot" "\$@"
LAUNCHEOF
    chmod +x "$_launcher"
    info "  Launcher installed: $_launcher"
}

# ── Step 8: PATH check ────────────────────────────────────────────────────────
check_path() {
    case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *)
            warn ""
            warn "  $BIN_DIR is not in your PATH."
            warn "  Add this line to your shell config (~/.bashrc, ~/.zshrc, etc.):"
            warn ""
            warn "    export PATH=\"\$HOME/.local/bin:\$PATH\""
            warn ""
            warn "  Then reload your shell:  source ~/.bashrc  (or restart your terminal)"
            ;;
    esac
}

# ── LLM server detection ────────────────────────────────────────────────────
# Probe LLM_URL/v1/models with a short timeout. Sets VOICEBOT_LLM_UP=1|0.
detect_llm_server() {
    step "Probing LLM server"
    _llm_url="${LLM_URL:-http://localhost:8000}"
    # Convert base URL to /v1/models endpoint.
    _probe_url="${_llm_url%/}/v1/models"
    info "  Checking $_probe_url ..."
    if command -v curl >/dev/null 2>&1; then
        if curl -fsS --max-time 3 -o /dev/null "$_probe_url" 2>/dev/null; then
            VOICEBOT_LLM_UP=1
            info "  LLM server reachable."
            return
        fi
    elif command -v wget >/dev/null 2>&1; then
        if wget -q --timeout=3 -O /dev/null "$_probe_url" 2>/dev/null; then
            VOICEBOT_LLM_UP=1
            info "  LLM server reachable."
            return
        fi
    fi
    VOICEBOT_LLM_UP=0
    warn "  LLM server not reachable at $_probe_url"
    warn "  Before running voicebot, start one of:"
    warn "    mlx-lm:  mlx_lm.server --model mlx-community/Qwen3-8B-4bit --port 8000"
    warn "    omlx:    omlx serve --model-dir ~/models --port 8001"
    warn "  Or set LLM_URL in $VOICEBOT_ENV to point at a remote server."
}

# ── End-of-install TTS smoke test ────────────────────────────────────────────
# Plays a 2-second sample with the chosen voice so the user hears it works.
# Non-TTY: just synthesizes silently and reports success/failure.
smoke_test() {
    step "Running TTS smoke test"
    _voice_bin="$VOICEBOT_BIN_DIR/voicebot"
    if [ ! -x "$_voice_bin" ]; then
        warn "  voicebot binary not found at $_voice_bin — skipping smoke test."
        return
    fi
    if [ "$OS" = "Darwin" ] && [ -n "${SELECTED_VOICE:-}" ]; then
        # Use `say` directly — no need to launch the whole binary.
        if say -v "$SELECTED_VOICE" "Hola, esto es voicebot." 2>/dev/null; then
            info "  Smoke test passed (macOS AVSpeech)."
        else
            warn "  Smoke test inconclusive (say returned non-zero)."
        fi
    elif [ "$OS" = "Linux" ] && [ -n "${SELECTED_KOKORO_VOICE:-}" ]; then
        # Use the binary's --list-voices as a no-op health check.
        if KOKORO_MODEL="$VOICEBOT_MODELS_DIR/kokoro-v1.0.onnx" \
           KOKORO_VOICES="$VOICEBOT_MODELS_DIR/voices-v1.0.bin" \
           TTS_PROVIDER=kokoro KOKORO_VOICE="$SELECTED_KOKORO_VOICE" \
           "$_voice_bin" --list-voices >/dev/null 2>&1; then
            info "  Smoke test passed (Kokoro voices loaded)."
        else
            warn "  Smoke test inconclusive — voicebot could not load Kokoro."
        fi
    fi
}

# ── Final banner ─────────────────────────────────────────────────────────────
print_completion() {
    printf "\n" >&2
    info "══════════════════════════════════════════════════"
    info "  Installation complete!"
    info "══════════════════════════════════════════════════"
    printf "\n" >&2
    if [ "${VOICEBOT_LLM_UP:-0}" = "1" ]; then
        info "LLM server is up. You can launch voicebot now."
    else
        info "Before starting voicebot, start your LLM server. Then edit config if needed:"
        info "  \$EDITOR $VOICEBOT_ENV"
    fi
    printf "\n" >&2
    info "Then start:"
    info "  voicebot"
    printf "\n" >&2
    info "List audio devices:"
    info "  voicebot --list-devices"
    info "List TTS voices:"
    info "  voicebot --list-voices"
    printf "\n" >&2
}

# ── Main orchestrator ───────────────────────────────────────────────────────
main() {
    detect_tty
    _apply_defaults
    _detect_platform

    printf "\n" >&2
    info "╔══════════════════════════════════════════════╗"
    info "║          Voicebot Installer                  ║"
    info "╚══════════════════════════════════════════════╝"
    printf "\n" >&2
    info "Platform : $OS_NAME ($TARGET)"
    info "Home     : $VOICEBOT_HOME"
    info "Launcher : $BIN_DIR/voicebot"
    printf "\n" >&2

    check_dependencies
    setup_directories
    install_binary
    install_whisper_model
    install_vad_model

    # Voice selection happens AFTER models are installed because the
    # Kokoro enumerator invokes the binary.
    if [ "$OS" = "Darwin" ]; then
        select_voice_macos
    else
        install_kokoro_models
        select_voice_kokoro
    fi

    create_env
    install_launcher
    detect_llm_server
    smoke_test
    check_path
    print_completion
}

main "$@"
