#!/bin/bash
# Build a Linux .deb package and portable AppImage-style tarball for Voxlink.
# Usage: ./installer/build-linux.sh
#
# Prerequisites:
#   - Rust toolchain (cargo)
#   - dpkg-deb (for .deb package)
#
# Output:
#   installer/dist/voxlink-<version>-linux-amd64.deb
#   installer/dist/voxlink-<version>-linux-x86_64.tar.gz

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DIST_DIR="$SCRIPT_DIR/dist"

VERSION=$(grep '^version' "$ROOT_DIR/crates/app_desktop/Cargo.toml" | head -1 | sed 's/.*"\(.*\)"/\1/')
BINARY_NAME="app_desktop"
ARCH="amd64"

echo "Building Voxlink v$VERSION for Linux..."

# Build the release binary
cd "$ROOT_DIR"
cargo build --release --bin "$BINARY_NAME"
BINARY="$ROOT_DIR/target/release/$BINARY_NAME"

if [ ! -f "$BINARY" ]; then
    echo "ERROR: Binary not found at $BINARY"
    exit 1
fi

mkdir -p "$DIST_DIR"

# ─── .deb package ───

DEB_DIR="$DIST_DIR/deb-staging"
rm -rf "$DEB_DIR"
mkdir -p "$DEB_DIR/DEBIAN"
mkdir -p "$DEB_DIR/usr/bin"
mkdir -p "$DEB_DIR/usr/share/applications"

# Copy binary
cp "$BINARY" "$DEB_DIR/usr/bin/voxlink"
chmod 755 "$DEB_DIR/usr/bin/voxlink"

# Control file
cat > "$DEB_DIR/DEBIAN/control" << CTRL
Package: voxlink
Version: $VERSION
Section: sound
Priority: optional
Architecture: $ARCH
Depends: libasound2, libssl3
Maintainer: Voxlink <dev@voxlink.app>
Description: Voice without limits
 Low-latency desktop voice chat with UDP transport,
 adaptive noise gating, and Opus audio codec.
CTRL

# Desktop entry
cat > "$DEB_DIR/usr/share/applications/voxlink.desktop" << DESKTOP
[Desktop Entry]
Name=Voxlink
Comment=Voice without limits
Exec=voxlink
Terminal=false
Type=Application
Categories=Network;Audio;
DESKTOP

# Build .deb
DEB_NAME="voxlink-${VERSION}-linux-${ARCH}.deb"
DEB_PATH="$DIST_DIR/$DEB_NAME"
if command -v dpkg-deb &> /dev/null; then
    dpkg-deb --build "$DEB_DIR" "$DEB_PATH"
    echo "Created: $DEB_PATH"
else
    echo "SKIP: dpkg-deb not found, skipping .deb package"
fi

rm -rf "$DEB_DIR"

# ─── Portable tarball ───

TAR_DIR="$DIST_DIR/voxlink-$VERSION"
rm -rf "$TAR_DIR"
mkdir -p "$TAR_DIR"
cp "$BINARY" "$TAR_DIR/voxlink"
chmod 755 "$TAR_DIR/voxlink"

# Include a simple launcher script
cat > "$TAR_DIR/run.sh" << 'LAUNCHER'
#!/bin/bash
DIR="$(cd "$(dirname "$0")" && pwd)"
exec "$DIR/voxlink" "$@"
LAUNCHER
chmod 755 "$TAR_DIR/run.sh"

TAR_NAME="voxlink-${VERSION}-linux-x86_64.tar.gz"
TAR_PATH="$DIST_DIR/$TAR_NAME"
cd "$DIST_DIR"
tar czf "$TAR_NAME" "voxlink-$VERSION/"
rm -rf "$TAR_DIR"

echo "Created: $TAR_PATH"
echo ""
echo "Done."
