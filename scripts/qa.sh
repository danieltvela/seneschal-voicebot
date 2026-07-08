#!/usr/bin/env bash
# scripts/qa.sh — Voicebot Quality Assurance harness.
#
# Public API:
#   bash scripts/qa.sh [MODE|STAGE]
#
# Modes:  fast (default) | full | all
# Stages: fmt | lint | test | test-ci | test-e2e | test-stt | test-llm
#         | build | audit | coverage
#
# Env overrides:
#   QA_NO_COLOR=1     disable ANSI colors
#   QA_SKIP=<csv>     stages to skip (e.g. "audit,coverage")
#   QA_KEEP_GOING=1   continue after a stage failure (default: abort)
#   CARGO=cargo       override the cargo binary
#
# Exit codes:
#   0   every requested stage ran to completion (or was explicitly skipped)
#   1   at least one stage failed
#   2   invalid mode/stage argument
#
# Each stage prints [PASS] / [FAIL] / [SKIP] with a one-line reason on failure.

set -uo pipefail

CARGO="${CARGO:-cargo}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
MODE="${1:-fast}"

if [[ -t 1 && -z "${QA_NO_COLOR:-}" ]]; then
    C_RED=$'\033[0;31m'
    C_GREEN=$'\033[0;32m'
    C_YELLOW=$'\033[1;33m'
    C_BLUE=$'\033[0;34m'
    C_BOLD=$'\033[1m'
    C_RESET=$'\033[0m'
else
    C_RED="" C_GREEN="" C_YELLOW="" C_BLUE="" C_BOLD="" C_RESET=""
fi

pass=0
fail=0
skip=0
declare -a STAGE_NAMES
declare -a STAGE_RESULTS

banner() {
    printf '\n%s%s=== %s ===%s\n' "$C_BOLD" "$C_BLUE" "$1" "$C_RESET"
}

stage_ok() {
    pass=$((pass + 1))
    STAGE_NAMES+=("$1")
    STAGE_RESULTS+=("ok")
    printf '%s[PASS]%s %s\n' "$C_GREEN" "$C_RESET" "$1"
}

stage_fail() {
    fail=$((fail + 1))
    STAGE_NAMES+=("$1")
    STAGE_RESULTS+=("fail")
    printf '%s[FAIL]%s %s — %s\n' "$C_RED" "$C_RESET" "$1" "$2"
}

stage_skip() {
    skip=$((skip + 1))
    STAGE_NAMES+=("$1")
    STAGE_RESULTS+=("skip")
    printf '%s[SKIP]%s %s — %s\n' "$C_YELLOW" "$C_RESET" "$1" "$2"
}

should_skip() {
    [[ -n "${QA_SKIP:-}" && ",${QA_SKIP}," == *",${1},"* ]]
}

stage_fmt() {
    banner "Stage: fmt  (cargo fmt --check)"
    if (cd "$PROJECT_ROOT" && $CARGO fmt --check --all); then
        stage_ok "fmt"
    else
        stage_fail "fmt" "formatting issues — run 'cargo fmt' to fix"
        return 1
    fi
}

stage_lint() {
    banner "Stage: lint  (cargo clippy --all-targets -- -D warnings)"
    if (cd "$PROJECT_ROOT" && $CARGO clippy --all-targets --no-deps -- -D warnings 2>&1 \
            | tail -n 80); then
        stage_ok "lint"
    else
        stage_fail "lint" "clippy reported warnings/errors"
        return 1
    fi
}

stage_test() {
    banner "Stage: test  (cargo test, default features)"
    if (cd "$PROJECT_ROOT" && $CARGO test --quiet 2>&1 | tail -n 50); then
        stage_ok "test"
    else
        stage_fail "test" "unit tests failed"
        return 1
    fi
}

stage_test_ci() {
    banner "Stage: test-ci  (cargo test --features tui,remote,control)"
    if (cd "$PROJECT_ROOT" && $CARGO test --features "tui,remote,control" --quiet 2>&1 \
            | tail -n 50); then
        stage_ok "test-ci"
    else
        stage_fail "test-ci" "feature-gated tests failed"
        return 1
    fi
}

stage_test_e2e() {
    banner "Stage: test-e2e  (cargo test e2e -- --ignored, wiremock-based)"
    if (cd "$PROJECT_ROOT" && $CARGO test --features "tui,remote,control" --quiet e2e -- --ignored 2>&1 | tail -n 80); then
        stage_ok "test-e2e"
    else
        stage_fail "test-e2e" "e2e tests failed"
        return 1
    fi
}

stage_test_stt() {
    banner "Stage: test-stt  (real Whisper model + audio fixtures)"
    if [[ ! -f "${WHISPER_MODEL:-models/ggml-large-v3-turbo.bin}" ]]; then
        stage_skip "test-stt" "WHISPER_MODEL not set and models/ggml-large-v3-turbo.bin not found"
        return 0
    fi
    if (cd "$PROJECT_ROOT" && $CARGO test --quiet -- --ignored stt 2>&1 | tail -n 50); then
        stage_ok "test-stt"
    else
        stage_fail "test-stt" "STT integration tests failed"
        return 1
    fi
}

stage_test_llm() {
    banner "Stage: test-llm  (real LLM server, e.g. mlx-lm)"
    if [[ -z "${LLM_URL:-}" ]] && ! curl -fs --max-time 1 "${LLM_URL:-http://127.0.0.1:8000}/v1/models" >/dev/null 2>&1; then
        stage_skip "test-llm" "no LLM server reachable on http://127.0.0.1:8000"
        return 0
    fi
    if (cd "$PROJECT_ROOT" && $CARGO test --quiet -- --ignored llm 2>&1 | tail -n 50); then
        stage_ok "test-llm"
    else
        stage_fail "test-llm" "real-LLM tests failed"
        return 1
    fi
}

stage_build() {
    _build_features="tui,remote,control"
    if [ "$(uname -s)" = "Darwin" ]; then
        _build_features="$_build_features,speech,avspeech"
    fi
    banner "Stage: build  (cargo build --features $_build_features)"
    if (cd "$PROJECT_ROOT" && $CARGO build --features "$_build_features" --quiet 2>&1 \
            | tail -n 20); then
        stage_ok "build"
    else
        stage_fail "build" "build failed"
        return 1
    fi
}

stage_audit() {
    banner "Stage: audit  (cargo audit)"
    if ! command -v cargo-audit >/dev/null 2>&1; then
        stage_skip "audit" "cargo-audit not installed (cargo install cargo-audit)"
        return 0
    fi
    if (cd "$PROJECT_ROOT" && cargo audit 2>&1 | tail -n 30); then
        stage_ok "audit"
    else
        stage_fail "audit" "cargo audit found vulnerabilities"
        return 1
    fi
}

stage_coverage() {
    banner "Stage: coverage  (cargo llvm-cov)"
    if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
        stage_skip "coverage" "cargo-llvm-cov not installed (cargo install cargo-llvm-cov)"
        return 0
    fi
    if (cd "$PROJECT_ROOT" && cargo llvm-cov --quiet --workspace --summary-only 2>&1 \
            | tail -n 20); then
        stage_ok "coverage"
    else
        stage_fail "coverage" "coverage run failed"
        return 1
    fi
}

run_stage() {
    if should_skip "$1"; then
        stage_skip "$1" "skipped via QA_SKIP"
        return 0
    fi
    "stage_$1" || true
}

run_fast() {
    stage_fmt      || return 1
    stage_lint     || return 1
    stage_test     || return 1
    stage_test_ci  || return 1
    stage_test_e2e || return 1
    stage_build    || return 1
}

run_full() {
    run_fast || return 1
    stage_test_stt  || true
    stage_test_llm  || true
    stage_audit     || true
    stage_coverage  || true
}

print_summary() {
    banner "Summary"
    local i
    for i in "${!STAGE_NAMES[@]}"; do
        local name="${STAGE_NAMES[$i]}"
        local res="${STAGE_RESULTS[$i]}"
        case "$res" in
            ok)   printf '  %s✓%s %s\n' "$C_GREEN"  "$C_RESET" "$name" ;;
            fail) printf '  %s✗%s %s\n' "$C_RED"    "$C_RESET" "$name" ;;
            skip) printf '  %s~%s %s\n' "$C_YELLOW" "$C_RESET" "$name" ;;
        esac
    done
    printf '\n  %s%d passed%s · %s%d failed%s · %s%d skipped%s\n' \
        "$C_GREEN"  "$pass" "$C_RESET" \
        "$C_RED"    "$fail" "$C_RESET" \
        "$C_YELLOW" "$skip" "$C_RESET"
}

main() {
    banner "Voicebot QA harness — mode: $MODE"
    printf '  project : %s\n' "$PROJECT_ROOT"
    printf '  cargo   : %s (%s)\n' "$CARGO" "$($CARGO --version 2>/dev/null || echo 'not found')"
    printf '  rustc   : %s\n' "$(rustc --version 2>/dev/null || echo 'not found')"

    case "$MODE" in
        fast)    run_fast    ;;
        full)    run_full    ;;
        fmt|\
        lint|\
        test|\
        test-ci|\
        test-e2e|\
        test-stt|\
        test-llm|\
        build|\
        audit|\
        coverage) run_stage "$MODE" ;;
        all)     run_full    ;;
        *)
            printf '%sUnknown mode/stage: %s%s\n' "$C_RED" "$MODE" "$C_RESET" >&2
            printf 'Valid modes: fast, full, all\n' >&2
            printf 'Valid stages: fmt, lint, test, test-ci, test-e2e, test-stt, test-llm, build, audit, coverage\n' >&2
            exit 2
            ;;
    esac

    print_summary

    if [[ "$fail" -gt 0 ]]; then
        printf '\n%sQA FAILED%s — %d stage(s) failed\n' "$C_RED" "$C_RESET" "$fail"
        exit 1
    fi

    printf '\n%sQA PASSED%s\n' "$C_GREEN" "$C_RESET"
    exit 0
}

main
