#!/usr/bin/env bash
# Build an example for Windows, natively or cross from macOS/Linux:
#   bash scripts/build-example.sh launcher-poc            -> dist/launcher-poc.exe
# Cross builds need cargo-xwin and LLVM (clang-cl, lld-link): `cargo install cargo-xwin`,
# `brew install llvm lld`. TISH names the compiler (default: `tish` on PATH).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
NAME="${1:?example name}"
SRC="$ROOT/crates/tish-windows/examples/$NAME/src/main.tish"
TISH="${TISH:-tish}"
TARGET=x86_64-pc-windows-msvc
mkdir -p "$ROOT/dist"
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) ;;
  *)
    export PATH="/opt/homebrew/opt/llvm/bin:$PATH"
    # cargo-xwin's C compiler, linker and SDK paths, so the compiler's own `cargo build` cross-links.
    eval "$(cargo xwin env --target "$TARGET")"
    ;;
esac
export TISH_NATIVE_CARGO_TARGET="$TARGET"
export TISH_NATIVE_TARGET_DIR="${TISH_NATIVE_TARGET_DIR:-$ROOT/target/tish-native}"
"$TISH" build "$SRC" --target native --native-backend rust -o "$ROOT/dist/$NAME.exe"
ls -la "$ROOT/dist/$NAME.exe"
