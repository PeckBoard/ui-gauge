#!/usr/bin/env bash
# Build the Peckboard ui-gauge plugin to a WASM module.
#
# Output: target/wasm32-unknown-unknown/release/peckboard_ui_gauge_plugin.wasm
# Drop that file into <dataDir>/plugins/ (rename to ui-gauge.wasm — the
# plugin's config key is its file stem) and (re)start Peckboard, or install it
# via the plugin registry.
set -euo pipefail

cd "$(dirname "$0")"

rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true

cargo build --target wasm32-unknown-unknown --release

WASM="target/wasm32-unknown-unknown/release/peckboard_ui_gauge_plugin.wasm"
echo "Built: $WASM"
ls -lh "$WASM"
