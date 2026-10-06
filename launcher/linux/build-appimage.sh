#!/bin/sh
# Builds the Linux launcher as an AppImage: target/appimage/WowCraft-x86_64.AppImage.
#
#   linux/build-appimage.sh            build in a Debian 11 container (glibc 2.31: runs on most distros)
#   linux/build-appimage.sh --native   build with this machine's Rust (runs only where glibc is as new)
#
# Needs Docker (or Podman) for the container build, curl the first time (to fetch appimagetool).
# WOWCRAFT_UPDATE_REPO (owner/name) is passed on, as for the Windows build.
set -eu

cd "$(dirname "$0")/.."
LAUNCHER="$(pwd)"
OUT="$LAUNCHER/target/appimage"
TOOLS="$LAUNCHER/target/appimage-tools"
mkdir -p "$OUT" "$TOOLS"

if [ "${1:-}" = "--native" ]; then
    cargo build --release
    BIN="$LAUNCHER/target/release/wowcraft-launcher"
else
    ENGINE="$(command -v docker || command -v podman || true)"
    if [ -z "$ENGINE" ]; then
        echo "Docker or Podman is needed (or pass --native)." >&2
        exit 1
    fi
    # Its own target and Cargo folders, so the host's build isn't mixed with the container's.
    "$ENGINE" run --rm \
        -u "$(id -u):$(id -g)" \
        -e CARGO_HOME=/src/target/docker-cargo \
        -e CARGO_TARGET_DIR=/src/target/docker \
        -e WOWCRAFT_UPDATE_REPO="${WOWCRAFT_UPDATE_REPO:-}" \
        -v "$LAUNCHER:/src" -w /src \
        docker.io/library/rust:1-bullseye \
        cargo build --release
    BIN="$LAUNCHER/target/docker/release/wowcraft-launcher"
fi

APPDIR="$OUT/WowCraft.AppDir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/256x256/apps"
cp "$BIN" "$APPDIR/usr/bin/wowcraft-launcher"
cp linux/AppRun "$APPDIR/AppRun"
chmod +x "$APPDIR/AppRun"
cp linux/wowcraft.desktop "$APPDIR/wowcraft.desktop"
cp linux/wowcraft.desktop "$APPDIR/usr/share/applications/wowcraft.desktop"
cp assets/icon.png "$APPDIR/wowcraft.png"
cp assets/icon.png "$APPDIR/usr/share/icons/hicolor/256x256/apps/wowcraft.png"

TOOL="$TOOLS/appimagetool-x86_64.AppImage"
if [ ! -x "$TOOL" ]; then
    curl -fL -o "$TOOL" https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x "$TOOL"
fi
# Extract-and-run: no FUSE needed to build.
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 "$TOOL" --no-appstream "$APPDIR" "$OUT/WowCraft-x86_64.AppImage"
echo "Built $OUT/WowCraft-x86_64.AppImage"
