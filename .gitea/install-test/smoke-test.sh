#!/bin/sh
# Smoke test for voicebot install (issue #31).

set +e

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

pass=0
fail=0
skip=0

banner() {
    printf '\n%s=== %s ===%s\n' "$YELLOW" "$1" "$NC"
}

report_pass() {
    printf '%s[PASS]%s %s\n' "$GREEN" "$NC" "$1"
    pass=$((pass + 1))
}

report_fail() {
    printf '%s[FAIL]%s %s\n' "$RED" "$NC" "$1"
    fail=$((fail + 1))
}

report_skip() {
    printf '%s[SKIP]%s %s\n' "$YELLOW" "$NC" "$1"
    skip=$((skip + 1))
}

banner "voicebot install smoke test"
printf 'voicebot: %s\n' "$(command -v voicebot || echo 'NOT FOUND')"
printf 'uname:    %s %s\n' "$(uname -s)" "$(uname -m)"

if ! command -v voicebot >/dev/null 2>&1; then
    report_fail "voicebot binary not found in PATH"
    printf '\n%sResult: %d passed, %d failed, %d skipped%s\n' \
        "$RED" "$pass" "$fail" "$skip" "$NC"
    exit 1
fi

banner "Check 1: --list-devices (CPAL / libasound)"
out="$(voicebot --list-devices 2>&1)"
rc=$?
if [ "$rc" -eq 0 ]; then
    report_pass "voicebot --list-devices exited 0"
    printf '%s\n' "$out" | sed 's/^/    /'
else
    report_fail "voicebot --list-devices exited $rc"
    printf '%s\n' "$out" | sed 's/^/    /'
fi

banner "Check 2: --list-voices with TTS_PROVIDER=kokoro"
if [ -f "$KOKORO_MODEL" ] && [ -f "$KOKORO_VOICES" ] \
    && [ -s "$KOKORO_MODEL" ] && [ -s "$KOKORO_VOICES" ]; then
    out="$(TTS_PROVIDER=kokoro voicebot --list-voices 2>&1)"
    rc=$?
    if [ "$rc" -eq 0 ]; then
        report_pass "voicebot --list-voices (kokoro) exited 0 and loaded ONNX models"
        printf '%s\n' "$out" | sed 's/^/    /' | head -20
    elif printf '%s' "$out" | grep -q "requires the 'kokoro' feature"; then
        report_skip "binary built without 'kokoro' feature — TTS check skipped"
    else
        report_fail "voicebot --list-voices (kokoro) exited $rc"
        printf '%s\n' "$out" | sed 's/^/    /'
    fi
else
    report_skip "Kokoro model files not present (KOKORO_MODEL=$KOKORO_MODEL, KOKORO_VOICES=$KOKORO_VOICES)"
fi

banner "Check 3: Silero VAD model file"
if [ -f "$VAD_MODEL" ] && [ -s "$VAD_MODEL" ]; then
    size_kb=$(($(stat -c%s "$VAD_MODEL") / 1024))
    report_pass "Silero VAD model present (${size_kb} KB): $VAD_MODEL"
else
    report_skip "Silero VAD model missing or empty: $VAD_MODEL"
fi

banner "Check 4: uninstall.sh removes install artifacts"
_uninstall_pass=0
_uninstall_fail=0

# Set up fake install tree in a temp location so we don't clobber the real binary.
_test_home="$(mktemp -d)"
_test_bin="$(mktemp -d)"
mkdir -p "$_test_home/bin" "$_test_home/models" "$_test_home/data"
printf '#!/bin/sh\nexec true\n' > "$_test_bin/voicebot"
chmod +x "$_test_bin/voicebot"
printf 'dummy model' > "$_test_home/models/dummy.bin"
printf 'dummy db' > "$_test_home/data/voicebot.db"
printf 'VOICEBOT_LANGUAGE=es\n' > "$_test_home/.env"

# Run uninstall.sh --yes with overridden paths.
_out="$(VOICEBOT_HOME="$_test_home" BIN_DIR="$_test_bin" /app/uninstall.sh --yes 2>&1)"
_rc=$?

if [ "$_rc" -eq 0 ]; then
    report_pass "uninstall.sh exited 0"
else
    report_fail "uninstall.sh exited $_rc"
    printf '%s\n' "$_out" | sed 's/^/    /'
    _uninstall_fail=$((_uninstall_fail + 1))
fi

if [ ! -d "$_test_home" ]; then
    report_pass "VOICEBOT_HOME removed"
else
    report_fail "VOICEBOT_HOME still exists: $_test_home"
    _uninstall_fail=$((_uninstall_fail + 1))
fi

if [ ! -f "$_test_bin/voicebot" ]; then
    report_pass "launcher removed"
else
    report_fail "launcher still exists: $_test_bin/voicebot"
    _uninstall_fail=$((_uninstall_fail + 1))
fi

# Clean up temp dirs (should already be gone, but be safe).
rm -rf "$_test_home" "$_test_bin"

if [ "$_uninstall_fail" -gt 0 ]; then
    fail=$((fail + _uninstall_fail))
fi
pass=$((pass + 3 - _uninstall_fail))

printf '\n%s=== Summary ===%s\n' "$YELLOW" "$NC"
printf '  passed: %s%d%s\n' "$GREEN" "$pass" "$NC"
printf '  failed: %s%d%s\n' "$RED" "$fail" "$NC"
printf '  skipped: %s%d%s\n' "$YELLOW" "$skip" "$NC"

if [ "$fail" -gt 0 ]; then
    exit 1
fi
exit 0
