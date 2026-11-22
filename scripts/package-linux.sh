#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMMIT_COUNT="$(git -C "$ROOT_DIR" rev-list --count HEAD 2>/dev/null || echo 1)"
VERSION="${PKG_VERSION:-0.1.${COMMIT_COUNT}}"
if [[ ! "$VERSION" =~ ^[0-9A-Za-z.+~-]+$ ]]; then
  echo "ERROR: invalid PKG_VERSION '$VERSION'" >&2
  exit 1
fi
TUI_BIN_PATH="$ROOT_DIR/target/release/rust-audio-tui"
GUI_BIN_PATH="$ROOT_DIR/target/release/rust-audio-gui"
DIST_DIR="$ROOT_DIR/dist"

if [[ ! -x "$TUI_BIN_PATH" ]] || [[ ! -x "$GUI_BIN_PATH" ]]; then
  echo "Building release binaries (TUI + GUI)..."
  (cd "$ROOT_DIR" && cargo build --release -p rust-audio-tui -p rust-audio-gui)
fi

rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR"

# Optional LAN Jackett discovery config (opt-in local builds only; skipped on GitHub Actions CI)
LAN_JACKETT_TMP="$DIST_DIR/lan-jackett.json"
JACKETT_CFG="${JACKETT_CONFIG:-${HOME}/media-stack/config/jackett/Jackett/ServerConfig.json}"
if [[ "${INCLUDE_LAN_JACKETT:-0}" == "1" ]] && [[ -z "${GITHUB_ACTIONS:-}" ]] && [[ -f "$JACKETT_CFG" ]]; then
  LAN_IP="$(hostname -I 2>/dev/null | awk '{print $1}')"
  python3 - "$JACKETT_CFG" "$LAN_IP" "$LAN_JACKETT_TMP" << 'PYEOF'
import json, sys
cfg_path, lan_ip, out_path = sys.argv[1], sys.argv[2], sys.argv[3]
cfg = json.load(open(cfg_path))
key = cfg.get("APIKey", "").strip()
port = cfg.get("Port", 9117)
if key:
    urls = []
    if lan_ip:
        urls.append(f"http://{lan_ip}:{port}")
    urls.append(f"http://localhost:{port}")
    with open(out_path, "w") as f:
        json.dump({"urls": urls, "api_key": key}, f, indent=2)
PYEOF
  if [[ -f "$LAN_JACKETT_TMP" ]]; then
    echo "WARNING: INCLUDE_LAN_JACKETT=1 embeds your local Jackett API key into the package; do NOT distribute publicly." >&2
  fi
fi

# ─── 1. Build Debian/Ubuntu/Kubuntu .deb package (with built-in APT repo) ────
DEB_STAGE="$DIST_DIR/pkg-deb"
mkdir -p "$DEB_STAGE/DEBIAN" \
         "$DEB_STAGE/etc/apt/sources.list.d" \
         "$DEB_STAGE/usr/bin" \
         "$DEB_STAGE/usr/share/applications" \
         "$DEB_STAGE/usr/share/doc/rust-audio-p2p" \
         "$DEB_STAGE/usr/share/rust-audio-p2p"

install -m 0755 "$TUI_BIN_PATH" "$DEB_STAGE/usr/bin/rust-audio-tui"
ln -sf rust-audio-tui "$DEB_STAGE/usr/bin/localplayer"
install -m 0755 "$GUI_BIN_PATH" "$DEB_STAGE/usr/bin/rust-audio-gui"
ln -sf rust-audio-gui "$DEB_STAGE/usr/bin/localplayer-gui"
install -m 0644 "$ROOT_DIR/LICENSE" "$DEB_STAGE/usr/share/doc/rust-audio-p2p/copyright"
if [[ -f "$LAN_JACKETT_TMP" ]]; then
  install -m 0644 "$LAN_JACKETT_TMP" "$DEB_STAGE/usr/share/rust-audio-p2p/lan-jackett.json"
fi

if [[ -n "${APT_GPG_PUBLIC_KEY_FILE:-}" ]] && [[ -f "$APT_GPG_PUBLIC_KEY_FILE" ]]; then
  mkdir -p "$DEB_STAGE/usr/share/keyrings"
  install -m 0644 "$APT_GPG_PUBLIC_KEY_FILE" "$DEB_STAGE/usr/share/keyrings/rust-audio-p2p.gpg"
  cat << 'EOF' > "$DEB_STAGE/etc/apt/sources.list.d/rust-audio-p2p.list"
deb [arch=amd64 signed-by=/usr/share/keyrings/rust-audio-p2p.gpg] https://niunmango.github.io/rust-audio-p2p ./
EOF
else
  cat << 'EOF' > "$DEB_STAGE/etc/apt/sources.list.d/rust-audio-p2p.list"
deb [arch=amd64 trusted=yes] https://niunmango.github.io/rust-audio-p2p ./
EOF
fi
chmod 0644 "$DEB_STAGE/etc/apt/sources.list.d/rust-audio-p2p.list"

cat << 'EOF' > "$DEB_STAGE/DEBIAN/conffiles"
/etc/apt/sources.list.d/rust-audio-p2p.list
EOF

cat << 'EOF' > "$DEB_STAGE/usr/share/applications/rust-audio-p2p.desktop"
[Desktop Entry]
Type=Application
Name=rust-audio-p2p (TUI)
GenericName=Audio Player
Comment=Distributed Rust Audio Player & BitTorrent/P2P TUI
Exec=/usr/bin/rust-audio-tui
Icon=audio-x-generic
Terminal=true
Categories=AudioVideo;Audio;Player;
EOF
chmod 0644 "$DEB_STAGE/usr/share/applications/rust-audio-p2p.desktop"

cat << 'EOF' > "$DEB_STAGE/usr/share/applications/rust-audio-p2p-gui.desktop"
[Desktop Entry]
Type=Application
Name=rust-audio-p2p
GenericName=Audio Player
Comment=Distributed Rust Audio Player & BitTorrent/P2P Desktop GUI
Exec=/usr/bin/rust-audio-gui
Icon=audio-x-generic
Terminal=false
Categories=AudioVideo;Audio;Player;
EOF
chmod 0644 "$DEB_STAGE/usr/share/applications/rust-audio-p2p-gui.desktop"

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
Description: Distributed Audio Player & P2P Network in Rust (TUI + GUI + Web)
 Unified audio player supporting automatic \$HOME music discovery,
 SQLite FTS5 indexing, BitTorrent streaming (librqbit), LRU cache,
 offline travel pins, LAN/Token P2P + Web UI/PWA, and bilingual ES/EN TUI + Desktop GUI.
EOF

DEB_OUT="$DIST_DIR/rust-audio-p2p_${VERSION}_amd64.deb"
fakeroot dpkg-deb --build "$DEB_STAGE" "$DEB_OUT"
cp "$DEB_OUT" "$DIST_DIR/rust-audio-p2p_latest_amd64.deb"
rm -rf "$DEB_STAGE"
echo "Built .deb: $DEB_OUT"

# ─── 2. Build portable self-contained .AppImage bundle ────────────────────────
APPDIR="$DIST_DIR/rust-audio-p2p.AppDir"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/scalable/apps" "$APPDIR/usr/share/rust-audio-p2p"

install -m 0755 "$TUI_BIN_PATH" "$APPDIR/usr/bin/rust-audio-tui"
ln -sf rust-audio-tui "$APPDIR/usr/bin/localplayer"
install -m 0755 "$GUI_BIN_PATH" "$APPDIR/usr/bin/rust-audio-gui"
ln -sf rust-audio-gui "$APPDIR/usr/bin/localplayer-gui"
if [[ -f "$LAN_JACKETT_TMP" ]]; then
  install -m 0644 "$LAN_JACKETT_TMP" "$APPDIR/usr/share/rust-audio-p2p/lan-jackett.json"
fi

cat << 'EOF' > "$APPDIR/rust-audio-p2p.desktop"
[Desktop Entry]
Type=Application
Name=rust-audio-p2p
Exec=rust-audio-gui
Icon=rust-audio-p2p
Terminal=false
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
if [[ "${1:-}" == "--gui" ]]; then
  shift
  exec "$HERE/usr/bin/rust-audio-gui" "$@"
elif [[ "${ARGV0:-}" == *gui* ]]; then
  exec "$HERE/usr/bin/rust-audio-gui" "$@"
fi
exec "$HERE/usr/bin/rust-audio-tui" "$@"
EOF
chmod 0755 "$APPDIR/AppRun"

PAYLOAD_TGZ="$DIST_DIR/appdir.tar.gz"
tar -C "$DIST_DIR" -czf "$PAYLOAD_TGZ" "rust-audio-p2p.AppDir"

APPIMAGE_OUT="$DIST_DIR/rust-audio-p2p-${VERSION}-x86_64.AppImage"
cat << EOF > "$APPIMAGE_OUT"
#!/usr/bin/env bash
set -euo pipefail
CACHE_ROOT="\${XDG_CACHE_HOME:-\$HOME/.cache}/rust-audio-p2p-appimage/${VERSION}"
APPDIR_PATH="\$CACHE_ROOT/rust-audio-p2p.AppDir"
MARKER="\$CACHE_ROOT/.extracted_ok"
SELF_BIN="\$(readlink -f "\$0")"
SELF_STAMP="\$(stat -c '%Y:%s' "\$SELF_BIN" 2>/dev/null || echo 1)"

if [[ -e "\$CACHE_ROOT" ]] && { [[ -L "\$CACHE_ROOT" ]] || [[ ! -O "\$CACHE_ROOT" ]]; }; then
  echo "ERROR: unsafe AppImage cache directory \$CACHE_ROOT" >&2
  exit 1
fi

if [[ ! -f "\$MARKER" ]] || [[ "\$(cat "\$MARKER" 2>/dev/null || true)" != "\$SELF_STAMP" ]]; then
  rm -rf "\$CACHE_ROOT"
  mkdir -p "\$CACHE_ROOT"
  chmod 0700 "\$CACHE_ROOT"
  ARCHIVE_LINE=\$(awk '/^__APPIMAGE_ARCHIVE_BELOW__$/ {print NR + 1; exit 0;}' "\$SELF_BIN")
  tail -n +"\$ARCHIVE_LINE" "\$SELF_BIN" | tar --no-same-owner -xz -C "\$CACHE_ROOT"
  echo "\$SELF_STAMP" > "\$MARKER"
fi

if [[ -L "\$CACHE_ROOT" ]] || [[ ! -O "\$CACHE_ROOT" ]]; then
  echo "ERROR: unsafe AppImage cache directory \$CACHE_ROOT" >&2
  exit 1
fi

export ARGV0="\${ARGV0:-\$0}"
exec "\$APPDIR_PATH/AppRun" "\$@"
__APPIMAGE_ARCHIVE_BELOW__
EOF
cat "$PAYLOAD_TGZ" >> "$APPIMAGE_OUT"
chmod 0755 "$APPIMAGE_OUT"
cp "$APPIMAGE_OUT" "$DIST_DIR/rust-audio-p2p-latest-x86_64.AppImage"
rm -f "$PAYLOAD_TGZ" "$LAN_JACKETT_TMP"
rm -rf "$APPDIR"

(
  cd "$DIST_DIR"
  sha256sum \
    "rust-audio-p2p_${VERSION}_amd64.deb" \
    "rust-audio-p2p_latest_amd64.deb" \
    "rust-audio-p2p-${VERSION}-x86_64.AppImage" \
    "rust-audio-p2p-latest-x86_64.AppImage" > SHA256SUMS.txt
)

echo "Built portable AppImage: $APPIMAGE_OUT"
ls -lh "$DIST_DIR"
