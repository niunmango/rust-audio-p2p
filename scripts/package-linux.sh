#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="0.1.0"
BIN_PATH="$ROOT_DIR/target/release/rust-audio-tui"
DIST_DIR="$ROOT_DIR/dist"

if [[ ! -x "$BIN_PATH" ]]; then
  echo "Building release binary..."
  (cd "$ROOT_DIR" && cargo build --release -p rust-audio-tui)
fi

rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR"

# ─── 1. Build Debian/Ubuntu/Kubuntu .deb package ─────────────────────────────
DEB_STAGE="$DIST_DIR/pkg-deb"
mkdir -p "$DEB_STAGE/DEBIAN" \
         "$DEB_STAGE/usr/bin" \
         "$DEB_STAGE/usr/share/applications" \
         "$DEB_STAGE/usr/share/doc/rust-audio-p2p"

install -m 0755 "$BIN_PATH" "$DEB_STAGE/usr/bin/rust-audio-tui"
ln -sf rust-audio-tui "$DEB_STAGE/usr/bin/localplayer"
install -m 0644 "$ROOT_DIR/LICENSE" "$DEB_STAGE/usr/share/doc/rust-audio-p2p/copyright"

cat << 'EOF' > "$DEB_STAGE/usr/share/applications/rust-audio-p2p.desktop"
[Desktop Entry]
Type=Application
Name=rust-audio-p2p
GenericName=Audio Player
Comment=Distributed Rust Audio Player & BitTorrent/P2P TUI
Exec=/usr/bin/rust-audio-tui
Icon=audio-x-generic
Terminal=true
Categories=AudioVideo;Audio;Player;
EOF
chmod 0644 "$DEB_STAGE/usr/share/applications/rust-audio-p2p.desktop"

INSTALLED_KB=$(du -sk "$DEB_STAGE/usr" | awk '{print $1}')
cat << EOF > "$DEB_STAGE/DEBIAN/control"
Package: rust-audio-p2p
Version: $VERSION
Section: sound
Priority: optional
Architecture: amd64
Installed-Size: $INSTALLED_KB
Depends: libc6, libasound2t64 | libasound2
Maintainer: niunmango <niunmango@yahoo.com>
Homepage: https://github.com/niunmango/rust-audio-p2p
Description: Distributed Audio Player & P2P Network in Rust (TUI)
 Unified audio player supporting automatic \$HOME music discovery,
 SQLite FTS5 indexing, BitTorrent streaming (librqbit), LRU cache,
 offline travel pins, and bilingual ES/EN interface.
EOF

DEB_OUT="$DIST_DIR/rust-audio-p2p_${VERSION}_amd64.deb"
fakeroot dpkg-deb --build "$DEB_STAGE" "$DEB_OUT"
rm -rf "$DEB_STAGE"
echo "Built .deb: $DEB_OUT"

# ─── 2. Build portable self-contained .AppImage bundle ────────────────────────
APPDIR="$DIST_DIR/rust-audio-p2p.AppDir"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/scalable/apps"

install -m 0755 "$BIN_PATH" "$APPDIR/usr/bin/rust-audio-tui"
ln -sf rust-audio-tui "$APPDIR/usr/bin/localplayer"

cat << 'EOF' > "$APPDIR/rust-audio-p2p.desktop"
[Desktop Entry]
Type=Application
Name=rust-audio-p2p
Exec=rust-audio-tui
Icon=rust-audio-p2p
Terminal=true
Categories=AudioVideo;Audio;Player;
EOF
cp "$APPDIR/rust-audio-p2p.desktop" "$APPDIR/usr/share/applications/rust-audio-p2p.desktop"

cat << 'EOF' > "$APPDIR/rust-audio-p2p.svg"
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
  <rect width="64" height="64" rx="12" fill="#181825"/>
  <circle cx="32" cy="32" r="20" fill="none" stroke="#89dceb" stroke-width="4"/>
  <polygon points="27,22 45,32 27,42" fill="#a6e3a1"/>
</svg>
EOF
cp "$APPDIR/rust-audio-p2p.svg" "$APPDIR/usr/share/icons/hicolor/scalable/apps/rust-audio-p2p.svg"

cat << 'EOF' > "$APPDIR/AppRun"
#!/usr/bin/env bash
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec "$HERE/usr/bin/rust-audio-tui" "$@"
EOF
chmod 0755 "$APPDIR/AppRun"

PAYLOAD_TGZ="$DIST_DIR/appdir.tar.gz"
tar -C "$DIST_DIR" -czf "$PAYLOAD_TGZ" "rust-audio-p2p.AppDir"

APPIMAGE_OUT="$DIST_DIR/rust-audio-p2p-${VERSION}-x86_64.AppImage"
cat << 'EOF' > "$APPIMAGE_OUT"
#!/usr/bin/env bash
set -euo pipefail
CACHE_ROOT="${XDG_CACHE_HOME:-$HOME/.cache}/rust-audio-p2p-appimage/0.1.0"
APPDIR_PATH="$CACHE_ROOT/rust-audio-p2p.AppDir"
MARKER="$CACHE_ROOT/.extracted_ok"
SELF_BIN="$(readlink -f "$0")"
SELF_STAMP="$(stat -c '%Y:%s' "$SELF_BIN" 2>/dev/null || echo 1)"

if [[ ! -f "$MARKER" ]] || [[ "$(cat "$MARKER" 2>/dev/null || true)" != "$SELF_STAMP" ]]; then
  rm -rf "$CACHE_ROOT"
  mkdir -p "$CACHE_ROOT"
  ARCHIVE_LINE=$(awk '/^__APPIMAGE_ARCHIVE_BELOW__$/ {print NR + 1; exit 0;}' "$SELF_BIN")
  tail -n +"$ARCHIVE_LINE" "$SELF_BIN" | tar -xz -C "$CACHE_ROOT"
  echo "$SELF_STAMP" > "$MARKER"
fi

exec "$APPDIR_PATH/AppRun" "$@"
__APPIMAGE_ARCHIVE_BELOW__
EOF
cat "$PAYLOAD_TGZ" >> "$APPIMAGE_OUT"
chmod 0755 "$APPIMAGE_OUT"
rm -f "$PAYLOAD_TGZ"
rm -rf "$APPDIR"

echo "Built portable AppImage: $APPIMAGE_OUT"
ls -lh "$DIST_DIR"
