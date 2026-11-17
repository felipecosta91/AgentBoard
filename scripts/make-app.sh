#!/bin/bash
# Assembles AgentBoard.app from a compiled binary.
# Usage: scripts/make-app.sh <binary> <output-dir> [version]
set -euo pipefail
BIN="$1"; OUT="$2"; VERSION="${3:-0.1.0}"
cd "$(dirname "$0")/.."

APP="$OUT/AgentBoard.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/AgentBoard"
cp assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>AgentBoard</string>
  <key>CFBundleDisplayName</key><string>AgentBoard</string>
  <key>CFBundleIdentifier</key><string>io.github.agentboard</string>
  <key>CFBundleExecutable</key><string>AgentBoard</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSAppTransportSecurity</key><dict><key>NSAllowsLocalNetworking</key><true/></dict>
</dict></plist>
PLIST
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true
echo "$APP"
