#!/bin/sh
# Dawnroom installer for Linux (x86_64). Per user, no sudo, no telemetry.
#   curl -fsSL https://dawnroom.dawnlightlabs.com/install.sh | sh
#   curl -fsSL https://dawnroom.dawnlightlabs.com/install.sh | sh -s -- --cli   (engine + CLI only)
# It downloads the latest release from GitHub (DawnlightLabs/dawnroom), checks each file against the
# release's SHA256SUMS, and installs:
#   ~/.local/lib/dawnroom/        the engine (dawnroom) and its model list
#   ~/.local/bin/dawnroom         a link to the engine, so `dawnroom` works in a terminal
#   ~/.local/lib/dawnroom/Dawnroom.AppImage + a menu entry   (Studio; not with --cli)
# Your models and images live in ~/.local/share/dawnroom and are never touched. Re-run to update.
set -eu

RELEASES="${DAWNROOM_RELEASES:-https://api.github.com/repos/DawnlightLabs/dawnroom/releases/latest}"
LIB="$HOME/.local/lib/dawnroom"
BIN="$HOME/.local/bin"
CLI_ONLY=0
[ "${1:-}" = "--cli" ] && CLI_ONLY=1

fail() { echo "Dawnroom wasn't installed: $*" >&2; exit 1; }

[ "$(uname -s)" = "Linux" ] || fail "this script is for Linux. On Windows, use install.ps1 or the installer from dawnroom.dawnlightlabs.com/download."
[ "$(uname -m)" = "x86_64" ] || fail "only x86_64 is built so far ($(uname -m) isn't yet)."
command -v curl >/dev/null || fail "curl is needed."
if command -v sha256sum >/dev/null; then SHA="sha256sum"; elif command -v shasum >/dev/null; then SHA="shasum -a 256"; else fail "sha256sum is needed."; fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# The latest release: its version and the download link of each file, from GitHub's JSON.
curl -fsSL "$RELEASES" -o "$TMP/release.json" || fail "couldn't reach $RELEASES."
VERSION="$(sed -n 's/.*"tag_name": *"v\{0,1\}\([^"]*\)".*/\1/p' "$TMP/release.json" | head -n 1)"
[ -n "$VERSION" ] || fail "couldn't read the latest version."
url() { grep -o "\"browser_download_url\": *\"[^\"]*/$1\"" "$TMP/release.json" | sed 's/.*"\(http[^"]*\)"/\1/' | head -n 1; }

get() { # get <file name>: download it and check it against SHA256SUMS
  u="$(url "$1")"; [ -n "$u" ] || fail "the release has no $1."
  curl -fSL --progress-bar "$u" -o "$TMP/$1" || fail "the download of $1 broke off."
  want="$(grep " \*\{0,1\}$1\$" "$TMP/SHA256SUMS" | cut -d ' ' -f 1)"
  [ -n "$want" ] || fail "$1 isn't listed in SHA256SUMS."
  have="$($SHA "$TMP/$1" | cut -d ' ' -f 1)"
  [ "$want" = "$have" ] || fail "$1 doesn't match its checksum (download damaged or tampered with). Nothing was changed."
}

u="$(url SHA256SUMS)"; [ -n "$u" ] || fail "the release has no SHA256SUMS."
curl -fsSL "$u" -o "$TMP/SHA256SUMS" || fail "couldn't download SHA256SUMS."

CLI_FILE="dawnroom-$VERSION-linux-x64.tar.gz"
APP_FILE="Dawnroom_${VERSION}_amd64.AppImage"
get "$CLI_FILE"
[ "$CLI_ONLY" = 1 ] || get "$APP_FILE"

# Everything is downloaded and checked; only now does anything on disk change.
mkdir -p "$TMP/new" && tar -xzf "$TMP/$CLI_FILE" -C "$TMP/new" || fail "couldn't unpack $CLI_FILE."
[ -x "$TMP/new/dawnroom" ] || fail "$CLI_FILE has no dawnroom binary."
"$BIN/dawnroom" stop >/dev/null 2>&1 || true   # a running engine lets go of its files
mkdir -p "$LIB" "$BIN"
rm -rf "$LIB/registry.new" && mv "$TMP/new/registry" "$LIB/registry.new"
mv "$TMP/new/dawnroom" "$LIB/dawnroom.new"
mv -f "$LIB/dawnroom.new" "$LIB/dawnroom"
rm -rf "$LIB/registry" && mv "$LIB/registry.new" "$LIB/registry"
ln -sf "$LIB/dawnroom" "$BIN/dawnroom"

if [ "$CLI_ONLY" = 0 ]; then
  chmod +x "$TMP/$APP_FILE" && mv -f "$TMP/$APP_FILE" "$LIB/Dawnroom.AppImage"
  mkdir -p "$HOME/.local/share/applications"
  cat > "$HOME/.local/share/applications/dawnroom.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Dawnroom
Comment=Local image generation on your own GPU
Exec=$LIB/Dawnroom.AppImage
Categories=Graphics;
Terminal=false
EOF
fi

where="Run \`dawnroom\`"
[ "$CLI_ONLY" = 0 ] && where="$where or open Dawnroom from your apps menu"
echo "Dawnroom $VERSION installed. $where."
case ":$PATH:" in *":$BIN:"*) ;; *) echo "Add $BIN to your PATH first: echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.profile, then open a new terminal." ;; esac
