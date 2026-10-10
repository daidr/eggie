#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
output="${1:-target/theme-palette-audit.html}"
mkdir -p "$(dirname "$output")"
if [[ "$output" != /* ]]; then
  output="$PWD/$output"
fi
EGGIE_THEME_AUDIT_HTML="$output" cargo test -p eggie-themes --no-default-features ui_palette_audit -- --nocapture
printf 'Palette preview: %s\n' "$output"
