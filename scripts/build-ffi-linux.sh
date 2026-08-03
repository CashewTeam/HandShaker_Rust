#!/bin/sh
# Build handshaker-ffi for Linux and stage artifacts under dist/linux/.
# Mirrors scripts/build-ffi-macos.sh (aarch64/x86_64 native build).
set -eu
cd "$(dirname "$0")/.."

cargo build -p handshaker-ffi --release

DIST=dist/linux
mkdir -p "$DIST"
cp target/release/libhandshaker_ffi.a "$DIST/libhandshaker_ffi.a"
if [ -f target/release/libhandshaker_ffi.so ]; then
    cp target/release/libhandshaker_ffi.so "$DIST/libhandshaker_ffi.so"
fi
cp crates/handshaker-ffi/include/handshaker_ffi.h "$DIST/handshaker_ffi.h"
cp crates/handshaker-ffi/include/module.modulemap "$DIST/module.modulemap"
echo "staged $DIST:"
ls -la "$DIST"
