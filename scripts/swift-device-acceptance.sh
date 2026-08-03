#!/bin/sh
# Real-device acceptance for the Swift wrapper (Phase F / goal 3a).
# Opt-in: requires a device attached via adb; skips otherwise.
# Run: scripts/swift-device-acceptance.sh
set -eu
cd "$(dirname "$0")/../platform/macos"

export HS_ACCEPTANCE=1
swift test --disable-sandbox --filter DeviceAcceptanceTests
