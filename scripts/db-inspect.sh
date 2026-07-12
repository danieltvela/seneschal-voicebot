#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$SCRIPT_DIR/.."
DB_INSPECT="$PROJECT_DIR/db-inspect"

case "${1:-help}" in
  run)
    DB_PATH="${2:-../data/seneschal.db}"
    cd "$DB_INSPECT"
    exec cargo run -- --db "$DB_PATH"
    ;;
  test)
    cd "$DB_INSPECT"
    exec cargo test
    ;;
  lint)
    cd "$DB_INSPECT"
    exec cargo clippy --all-targets -- -D warnings
    ;;
  fmt)
    cd "$DB_INSPECT"
    exec cargo fmt --check
    ;;
  *)
    echo "Usage: $0 {run [db-path]|test|lint|fmt}"
    exit 1
    ;;
esac
