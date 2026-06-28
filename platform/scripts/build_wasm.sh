#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLATFORM_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
ROOT_DIR="$(cd "$PLATFORM_DIR/.." && pwd)"

PKG="${SENA_CARGO_PKG:-pal-vm}"
RUST_LIB_NAME="${RUST_LIB_NAME:-pal_vm}"
WASM_BINDGEN_TARGET="${WASM_BINDGEN_TARGET:-web}"
DIST="$ROOT_DIR/dist/wasm"
PKG_DIR="$DIST/pkg"

command -v cargo >/dev/null 2>&1 || { echo "ERROR: cargo not found" >&2; exit 1; }
command -v rustup >/dev/null 2>&1 || { echo "ERROR: rustup not found" >&2; exit 1; }
command -v wasm-bindgen >/dev/null 2>&1 || {
  echo "ERROR: wasm-bindgen not found. Install wasm-bindgen-cli before running the experimental build." >&2
  exit 1
}

mkdir -p "$DIST" "$PKG_DIR"
rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true

echo "[wasm] Building ${PKG}"
(cd "$ROOT_DIR" && cargo build --release -p "$PKG" --lib --target wasm32-unknown-unknown)

WASM_PATH="$ROOT_DIR/target/wasm32-unknown-unknown/release/${RUST_LIB_NAME}.wasm"
[[ -f "$WASM_PATH" ]] || {
  echo "ERROR: Missing wasm output: $WASM_PATH" >&2
  exit 1
}

rm -rf "$PKG_DIR"
mkdir -p "$PKG_DIR"

wasm-bindgen "$WASM_PATH" \
  --target "$WASM_BINDGEN_TARGET" \
  --out-dir "$PKG_DIR"

cp -f "$PLATFORM_DIR/wasm/index.html" "$DIST/index.html"
cp -f "$PLATFORM_DIR/wasm/main.js" "$DIST/main.js"
cp -f "$PLATFORM_DIR/wasm/style.css" "$DIST/style.css"

(
  cd "$DIST"
  rm -f sena-wasm.zip
  zip -r sena-wasm.zip index.html main.js style.css pkg
)

echo "OK: $DIST/sena-wasm.zip"
