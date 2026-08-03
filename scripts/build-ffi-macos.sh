#!/bin/sh
# Build handshaker-ffi for macOS and stage Apple artifacts under dist/apple/.
set -eu
cd "$(dirname "$0")/.."

cargo build -p handshaker-ffi --release

DIST=dist/apple
mkdir -p "$DIST"
cp target/release/libhandshaker_ffi.a "$DIST/libhandshaker_ffi.a"
cp target/release/libhandshaker_ffi.dylib "$DIST/libhandshaker_ffi.dylib"
cp crates/handshaker-ffi/include/handshaker_ffi.h "$DIST/handshaker_ffi.h"
cp crates/handshaker-ffi/include/module.modulemap "$DIST/module.modulemap"
echo "staged $DIST:"
ls -la "$DIST"

# Static XCFramework for the Swift Package (HandShakerCore binaryTarget).
XCFRAMEWORK=HandShakerCore/Artifacts/HandShakerFFI.xcframework
rm -rf "$XCFRAMEWORK"
xcodebuild -create-xcframework \
    -library "$DIST/libhandshaker_ffi.a" \
    -headers "$DIST" \
    -output "$XCFRAMEWORK" >/dev/null
echo "staged $XCFRAMEWORK:"
ls "$XCFRAMEWORK"
