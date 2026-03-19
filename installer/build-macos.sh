#!/bin/bash
# Build a macOS .app bundle and .dmg disk image for Voxlink.
# Usage: ./installer/build-macos.sh [--release]
#
# Prerequisites:
#   - Rust toolchain (cargo)
#   - hdiutil (ships with macOS)
#
# Output: installer/dist/Voxlink-<version>-macos.dmg

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DIST_DIR="$SCRIPT_DIR/dist"

VERSION=$(grep '^version' "$ROOT_DIR/crates/app_desktop/Cargo.toml" | head -1 | sed 's/.*"\(.*\)"/\1/')
APP_NAME="Voxlink"
BUNDLE_ID="com.voxlink.Voxlink"
BINARY_NAME="app_desktop"

echo "Building $APP_NAME v$VERSION for macOS..."

# Build the release binary
cd "$ROOT_DIR"
cargo build --release --bin "$BINARY_NAME"
BINARY="$ROOT_DIR/target/release/$BINARY_NAME"

if [ ! -f "$BINARY" ]; then
    echo "ERROR: Binary not found at $BINARY"
    exit 1
fi

# Create .app bundle structure
APP_DIR="$DIST_DIR/$APP_NAME.app"
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS"
mkdir -p "$APP_DIR/Contents/Resources"

# Copy binary
cp "$BINARY" "$APP_DIR/Contents/MacOS/$APP_NAME"

# Create Info.plist
cat > "$APP_DIR/Contents/Info.plist" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>$APP_NAME</string>
    <key>CFBundleDisplayName</key>
    <string>$APP_NAME</string>
    <key>CFBundleIdentifier</key>
    <string>$BUNDLE_ID</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundleExecutable</key>
    <string>$APP_NAME</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSMicrophoneUsageDescription</key>
    <string>Voxlink needs microphone access for voice chat.</string>
</dict>
</plist>
PLIST

echo "Created $APP_DIR"

# Create DMG
DMG_NAME="$APP_NAME-$VERSION-macos.dmg"
DMG_PATH="$DIST_DIR/$DMG_NAME"
rm -f "$DMG_PATH"

# Create a temporary directory for DMG contents
DMG_STAGING="$DIST_DIR/dmg-staging"
rm -rf "$DMG_STAGING"
mkdir -p "$DMG_STAGING"
cp -R "$APP_DIR" "$DMG_STAGING/"
ln -s /Applications "$DMG_STAGING/Applications"

hdiutil create -volname "$APP_NAME" \
    -srcfolder "$DMG_STAGING" \
    -ov -format UDZO \
    "$DMG_PATH"

rm -rf "$DMG_STAGING"

echo ""
echo "Done: $DMG_PATH"
echo "Size: $(du -h "$DMG_PATH" | cut -f1)"
