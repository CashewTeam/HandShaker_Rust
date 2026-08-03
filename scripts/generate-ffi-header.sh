#!/bin/sh
# Verify the handshaker-ffi C header stays in sync with the Rust exports and
# stage the canonical header/modulemap under dist/apple/.
#
# Zero-dependency check: every `#[unsafe(no_mangle)] pub unsafe extern "C" fn
# hs_*` in crates/handshaker-ffi/src/lib.rs must have a prototype in
# crates/handshaker-ffi/include/handshaker_ffi.h.
set -eu
cd "$(dirname "$0")/.."

LIB=crates/handshaker-ffi/src/lib.rs
HDR=crates/handshaker-ffi/include/handshaker_ffi.h

# Extract exported symbol names (fn name on the no_mangle line's following
# `pub unsafe extern "C" fn NAME`).
missing=0
while IFS= read -r sym; do
    [ -z "$sym" ] && continue
    if ! grep -q "${sym}(" "$HDR"; then
        echo "missing header prototype: $sym" >&2
        missing=$((missing + 1))
    fi
done <<EOF
$(grep -A1 'no_mangle' "$LIB" | sed -n 's/.*extern "C" fn \([a-z0-9_]*\).*/\1/p')
EOF

if [ "$missing" -ne 0 ]; then
    echo "header sync FAILED: $missing symbol(s) missing from $HDR" >&2
    exit 1
fi

# Stage canonical header + modulemap alongside any built libraries.
DIST=dist/apple
mkdir -p "$DIST"
cp "$HDR" "$DIST/handshaker_ffi.h"
cp crates/handshaker-ffi/include/module.modulemap "$DIST/module.modulemap"
echo "header sync ok ($(grep -c 'no_mangle' "$LIB") exports; staged $DIST/handshaker_ffi.h)"
