#!/bin/sh
# Build handshaker-ffi for macOS (arm64 + x86_64, static libusb) and stage
# Apple artifacts under dist/apple/.
#
# Static libusb (P1-10): LIBUSB_STATIC=1 makes libusb1-sys link the static
# libusb (Homebrew .a on arm64; the crate falls back to compiling a vendored
# copy when pkg-config has no static lib, e.g. x86_64). The staged .a/.dylib
# and the XCFramework therefore have no libusb dynamic dependency — required
# for App Store / notarized delivery.
#
# x86_64 std: requires `rustup target add x86_64-apple-darwin`. On machines
# where rustup cannot install targets (e.g. a read-only ~/.rustup), point
# HS_X86_CARGO at a cargo that already has the target, e.g.:
#   HS_X86_CARGO="env LIBUSB_STATIC=1 RUSTC=$HOME/.cargo/hs-x86-toolchain/bin/rustc $HOME/.cargo/hs-x86-toolchain/bin/cargo"
set -eu
cd "$(dirname "$0")/.."

LIBUSB_STATIC=1 cargo build -p handshaker-ffi --release

if [ -n "${HS_X86_CARGO:-}" ]; then
    # shellcheck disable=SC2086
    $HS_X86_CARGO build -p handshaker-ffi --release --target x86_64-apple-darwin
else
    LIBUSB_STATIC=1 cargo build -p handshaker-ffi --release --target x86_64-apple-darwin
fi

DIST=dist/apple
mkdir -p "$DIST"
lipo -create \
    target/release/libhandshaker_ffi.a \
    target/x86_64-apple-darwin/release/libhandshaker_ffi.a \
    -output "$DIST/libhandshaker_ffi.a"
lipo -create \
    target/release/libhandshaker_ffi.dylib \
    target/x86_64-apple-darwin/release/libhandshaker_ffi.dylib \
    -output "$DIST/libhandshaker_ffi.dylib"
cp crates/handshaker-ffi/include/handshaker_ffi.h "$DIST/handshaker_ffi.h"
cp crates/handshaker-ffi/include/module.modulemap "$DIST/module.modulemap"
echo "staged $DIST:"
lipo -info "$DIST/libhandshaker_ffi.a" "$DIST/libhandshaker_ffi.dylib"
ls -la "$DIST"

# Static XCFramework for the Swift Package (platform/macos binaryTarget).
XCFRAMEWORK=platform/macos/Artifacts/HandShakerFFI.xcframework
rm -rf "$XCFRAMEWORK"
xcodebuild -create-xcframework \
    -library "$DIST/libhandshaker_ffi.a" \
    -headers "$DIST" \
    -output "$XCFRAMEWORK" >/dev/null
echo "staged $XCFRAMEWORK:"
ls "$XCFRAMEWORK"
