#!/bin/sh
# Run the C and Swift smoke tests against the release FFI library.
set -eu
cd "$(dirname "$0")/.."

cargo build -p handshaker-ffi --release

# C smoke test.
# The release archive carries undefined _libusb_* references when
# pkg-config found Homebrew libusb at build time (CI runners); on local
# builds the archive is self-contained, so -lusb-1.0 is a no-op.
clang -I crates/handshaker-ffi/include scripts/ffi_smoke.c \
    -L target/release -lhandshaker_ffi -o /tmp/ffi_smoke_c \
    -L/opt/homebrew/lib -lusb-1.0 \
    -Wl,-rpath,"$PWD/target/release"
/tmp/ffi_smoke_c
rm -f /tmp/ffi_smoke_c
echo "C smoke test passed"

# Swift smoke test.
swiftc -I crates/handshaker-ffi/include \
    -L target/release -lhandshaker_ffi \
    -L/opt/homebrew/lib -lusb-1.0 \
    -Xlinker -rpath -Xlinker "$PWD/target/release" \
    scripts/ffi_smoke.swift -o /tmp/ffi_smoke_swift
/tmp/ffi_smoke_swift
rm -f /tmp/ffi_smoke_swift
echo "Swift smoke test passed"
