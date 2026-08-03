#!/bin/sh
# Verify the handshaker-ffi C header stays in sync with the Rust exports and
# stage the canonical header/modulemap under dist/apple/.
#
# Single source of truth (M8.1 Phase A):
#   crates/handshaker-ffi/src/lib.rs            ABI_VERSION_* constants + exports
#   crates/handshaker-ffi/include/handshaker_ffi.h   prototypes + version comment
#   docs/ffi-abi-snapshot.md                    generated snapshot (committed)
#   scripts/check-ffi-abi.py                    comparison logic
#
# Checks (default mode):
#   - every exported `fn hs_*` in lib.rs has a header prototype;
#   - header prototypes match Rust signatures (param count + type categories);
#   - ABI_VERSION_* constants match the header top comment;
#   - docs/ffi-abi-snapshot.md matches the current exports.
#
# Usage:
#   scripts/generate-ffi-header.sh              verify + stage
#   scripts/generate-ffi-header.sh --update     regenerate snapshot, then stage
set -eu
cd "$(dirname "$0")/.."

LIB=crates/handshaker-ffi/src/lib.rs
HDR=crates/handshaker-ffi/include/handshaker_ffi.h
SNAPSHOT=docs/ffi-abi-snapshot.md

if [ "${1:-}" = "--update" ]; then
    python3 scripts/check-ffi-abi.py --lib "$LIB" --header "$HDR" \
        --snapshot "$SNAPSHOT"
else
    python3 scripts/check-ffi-abi.py --lib "$LIB" --header "$HDR" \
        --check-snapshot "$SNAPSHOT"
fi

# Stage canonical header + modulemap alongside any built libraries.
DIST=dist/apple
mkdir -p "$DIST"
cp "$HDR" "$DIST/handshaker_ffi.h"
cp crates/handshaker-ffi/include/module.modulemap "$DIST/module.modulemap"
echo "staged $DIST/handshaker_ffi.h (+ modulemap)"
