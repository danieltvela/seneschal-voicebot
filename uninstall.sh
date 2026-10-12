#!/bin/sh
# Seneschal uninstaller
# Reverses everything created by install.
#
# Usage:
#   ./uninstall.sh           # interactive confirmation
#   ./uninstall.sh --yes     # non-interactive / CI
#
# Environment overrides (same names as the installer):
#   SENESCHAL_HOME  — home directory to remove (default: ~/.seneschal)
#   BIN_DIR        — launcher directory (default: ~/.local/bin)

set -e

# ── Argument parsing ─────────────────────────────────────────────────────────
FORCE_YES=0
for _arg in "$@"; do
    case "$_arg" in
        -y|--yes) FORCE_YES=1 ;;
        -h|--help)
            printf 'Usage: %s [--yes]\n' "$0"
            printf '  --yes   Skip confirmation prompts\n'
            exit 0
            ;;
        *)
            printf 'Unknown option: %s\n' "$_arg" >&2
            printf 'Usage: %s [--yes]\n' "$0" >&2
            exit 1
            ;;
    esac
done

# ── Output helpers ──────────────────────────────────────────────────────────
if [ -t 1 ]; then
    _GREEN='\033[0;32m'; _YELLOW='\033[1;33m'; _RED='\033[0;31m'; _NC='\033[0m'
else
    _GREEN=''; _YELLOW=''; _RED=''; _NC=''
fi

info()  { printf "${_GREEN}[seneschal]${_NC} %s\n" "$1" >&2; }
warn()  { printf "${_YELLOW}[seneschal]${_NC} %s\n" "$1" >&2; }
error() { printf "${_RED}[seneschal] ERROR:${_NC} %s\n" "$1" >&2; exit 1; }
step()  { printf "\n${_GREEN}▶ %s${_NC}\n" "$1" >&2; }

# ── Defaults ────────────────────────────────────────────────────────────────
SENESCHAL_HOME="${SENESCHAL_HOME:-$HOME/.seneschal}"
BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"
LAUNCHER="$BIN_DIR/seneschal"

# ── Safety guards ─────────────────────────────────────────────────────────────
# Refuse to delete paths that are obviously dangerous.
_is_dangerous_path() {
    _path="$1"
    case "$_path" in
        /|/bin|/boot|/dev|/etc|/home|/lib|/lib64|/opt|/sbin|/usr|/var|"$HOME")
            return 0
            ;;
        "")
            return 0
            ;;
    esac
    return 1
}

_validate_path() {
    _label="$1"; _path="$2"
    if [ -z "$2" ]; then
        error "$_label is empty — refusing to proceed."
    fi
    if _is_dangerous_path "$2"; then
        error "Refusing to delete protected path: $2 ($_label)"
    fi
}

_validate_path "SENESCHAL_HOME" "$SENESCHAL_HOME"
_validate_path "BIN_DIR"      "$BIN_DIR"

# ── Detect what exists ──────────────────────────────────────────────────────
HOME_EXISTS=0
LAUNCHER_EXISTS=0
if [ -d "$SENESCHAL_HOME" ]; then
    HOME_EXISTS=1
fi
if [ -f "$LAUNCHER" ]; then
    LAUNCHER_EXISTS=1
fi

if [ "$HOME_EXISTS" -eq 0 ] && [ "$LAUNCHER_EXISTS" -eq 0 ]; then
    info "Nothing to uninstall."
    info "  SENESCHAL_HOME: $SENESCHAL_HOME (not found)"
    info "  Launcher:      $LAUNCHER (not found)"
    exit 0
fi

# ── Show what will be removed ────────────────────────────────────────────────
step "Review uninstall targets"
if [ "$LAUNCHER_EXISTS" -eq 1 ]; then
    warn "  Launcher script: $LAUNCHER"
fi
if [ "$HOME_EXISTS" -eq 1 ]; then
    warn "  Home directory:  $SENESCHAL_HOME"
    # Show a quick inventory so the user knows what's inside.
    _size=""
    if command -v du >/dev/null 2>&1; then
        _size=$(du -sh "$SENESCHAL_HOME" 2>/dev/null | awk '{print $1}')
    fi
    if [ -n "$_size" ]; then
        warn "    Size: $_size"
    fi
    # List top-level entries.
    for _entry in "$SENESCHAL_HOME"/* "$SENESCHAL_HOME"/.*; do
        [ -e "$_entry" ] || continue
        _base=$(basename "$_entry")
        case "$_base" in
            .|..) continue ;;
        esac
        warn "    └── $_base"
    done
fi
warn ""
warn "  System dependencies (espeak-ng, libasound2, etc.) are NOT removed."
warn "  Remove them manually with your package manager if desired."

# ── Confirmation ──────────────────────────────────────────────────────────────
if [ "$FORCE_YES" -eq 0 ]; then
    printf "\n${_YELLOW}Proceed with uninstall? [y/N]: ${_NC}" >&2
    _reply=""
    read -r _reply || _reply=""
    case "$_reply" in
        [Yy]*) ;;
        *) info "Uninstall cancelled."; exit 0 ;;
    esac
fi

# ── Remove launcher ───────────────────────────────────────────────────────────
if [ "$LAUNCHER_EXISTS" -eq 1 ]; then
    step "Removing launcher"
    rm -f "$LAUNCHER"
    info "  Removed: $LAUNCHER"
fi

# ── Remove home directory ─────────────────────────────────────────────────────
if [ "$HOME_EXISTS" -eq 1 ]; then
    step "Removing home directory"
    rm -rf "$SENESCHAL_HOME"
    info "  Removed: $SENESCHAL_HOME"
fi

# ── PATH reminder ─────────────────────────────────────────────────────────────
step "Uninstall complete"
info "seneschal has been removed from this system."
if [ "$FORCE_YES" -eq 0 ]; then
    case ":$PATH:" in
        *":$BIN_DIR:"*)
            warn ""
            warn "  $BIN_DIR is still in your PATH."
            warn "  You can leave it there (harmless) or remove it from your shell config."
            ;;
    esac
fi
