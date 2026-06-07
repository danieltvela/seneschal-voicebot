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

printf '\n%s=== Summary ===%s\n' "$YELLOW" "$NC"
printf '  passed: %s%d%s\n' "$GREEN" "$pass" "$NC"
printf '  failed: %s%d%s\n' "$RED" "$fail" "$NC"
printf '  skipped: %s%d%s\n' "$YELLOW" "$skip" "$NC"

if [ "$fail" -gt 0 ]; then
    exit 1
fi
exit 0
