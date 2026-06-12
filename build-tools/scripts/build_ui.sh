#!/usr/bin/env bash
set -e
# Resolve project root (two levels up)
PROJECT_ROOT=$(realpath "$(dirname "$0")/../..")
cd "$PROJECT_ROOT/ui/osaima-shell"
# Install Node.js dependencies (use npm install as lockfile missing)
npm install
# Ensure Tauri CLI is available
cargo install --locked tauri-cli || true
# Build the Tauri UI in release mode
cargo tauri build --release
# Verify binary
if [[ -f src-tauri/target/release/osaima-shell ]]; then
  echo "UI binary built successfully."
else
  echo "Failed to build UI binary." && exit 1
fi

