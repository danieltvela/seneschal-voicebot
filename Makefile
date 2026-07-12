# Makefile — Seneschal Quality Assurance targets.
#
# Public surface (discoverable via `make help`):
#   make qa         — full fast suite (default goal)
#   make qa-fast    — alias for `qa`
#   make qa-full    — fast + audit + coverage
#   make fmt        — cargo fmt --check
#   make lint       — cargo clippy --all-targets -- -D warnings
#   make test       — cargo test (default features)
#   make test-ci    — cargo test --features tui,remote,control
#   make test-e2e   — cargo test e2e -- --ignored (wiremock e2e)
#   make test-stt   — ignored STT tests (needs WHISPER_MODEL)
#   make test-llm   — ignored LLM tests (needs running LLM server)
#   make build      — cargo build --features tui,remote,control (+ speech,avspeech on macOS)
#   make audit      — cargo audit (needs cargo-audit)
#   make coverage   — cargo llvm-cov summary (needs cargo-llvm-cov)
#   make help       — print this list
#
# Everything is a thin wrapper around scripts/qa.sh so behavior stays in
# one place. Pass-through args: e.g. `make qa QA_SKIP=audit`.

SHELL := /usr/bin/env bash
QA    := bash scripts/qa.sh

.PHONY: help qa qa-fast qa-full fmt lint test test-ci test-e2e test-stt test-llm build audit coverage

help:
	@printf 'Seneschal QA targets:\n'
	@printf '  make qa         full fast suite (default goal)\n'
	@printf '  make qa-fast    alias for qa\n'
	@printf '  make qa-full    fast + audit + coverage\n'
	@printf '  make fmt        cargo fmt --check\n'
	@printf '  make lint       cargo clippy --all-targets -- -D warnings\n'
	@printf '  make test       cargo test (default features)\n'
	@printf '  make test-ci    cargo test --features tui,remote,control\n'
	@printf '  make test-e2e   cargo test e2e -- --ignored (wiremock e2e)\n'
	@printf '  make test-stt   ignored STT tests (needs WHISPER_MODEL)\n'
	@printf '  make test-llm   ignored LLM tests (needs running LLM server)\n'
	@printf '  make build      cargo build --features tui,remote,control (+ speech,avspeech on macOS)\n'
	@printf '  make audit      cargo audit (needs cargo-audit)\n'
	@printf '  make coverage   cargo llvm-cov summary (needs cargo-llvm-cov)\n'
	@printf '  make help       this list\n'
	@printf '\nPass-through: make qa QA_SKIP=audit,coverage\n'

qa qa-fast:
	@$(QA) fast

qa-full:
	@$(QA) full

fmt:        ; @$(QA) fmt
lint:       ; @$(QA) lint
test:       ; @$(QA) test
test-ci:    ; @$(QA) test-ci
test-e2e:   ; @$(QA) test-e2e
test-stt:   ; @$(QA) test-stt
test-llm:   ; @$(QA) test-llm
build:      ; @$(QA) build
audit:      ; @$(QA) audit
coverage:   ; @$(QA) coverage
