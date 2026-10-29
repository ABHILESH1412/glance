#!/bin/bash
# SPDX-FileCopyrightText: 2026 Abhilesh Singh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Build the three release packages from the commit checked out, one after
# another (never two at once: a release build with LTO takes gigabytes):
#
#   glance-image-viewer-x86_64.pkg.tar.zst   Arch, with makepkg
#   Glance-x86_64.flatpak                    Flatpak bundle, with flatpak-builder
#   Glance-x86_64.AppImage                   AppImage, built inside ubuntu:24.04
#
# and a SHA256SUMS listing them. The names carry no version, so the README's
# .../releases/latest/download/<name> links, and Glance's own updater, always
# find the newest. The version is inside each package.
#
#   scripts/build-packages.sh <output folder> [arch] [flatpak] [appimage]
#
# Naming packages builds just those: GitHub Actions builds each on a machine
# of its own. SHA256SUMS is only written when all three are built here.
#
# JOBS=n sets how many compile jobs run at once (default 4).

set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT=${1:?usage: scripts/build-packages.sh <output folder> [arch] [flatpak] [appimage]}
shift
wanted=" ${*:-arch flatpak appimage} "
want() { [[ "$wanted" == *" $1 "* ]]; }
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
WORK="$ROOT/target/packaging"
JOBS=${JOBS:-4}
APPID=io.github.abhilesh1412.Glance
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)

say() { printf '\n\033[1m== %s\033[0m\n' "$*"; }
mkdir -p "$WORK"
cd "$ROOT"

# --- Arch -------------------------------------------------------------------
if want arch; then
say "Arch package (glance-image-viewer $VERSION)"
rm -rf "$WORK/arch" && mkdir -p "$WORK/arch"
# Exactly the commit, nothing uncommitted.
git archive --format=tar.gz --prefix="glance-$VERSION/" -o "$WORK/arch/glance-image-viewer-$VERSION.tar.gz" HEAD
sed "s|^source=.*|source=(\"glance-image-viewer-$VERSION.tar.gz\")|" packaging/aur/PKGBUILD > "$WORK/arch/PKGBUILD"
(cd "$WORK/arch" && CARGO_BUILD_JOBS=$JOBS PKGDEST="$WORK/arch/out" BUILDDIR="$WORK/arch/work" \
    makepkg -f --noconfirm > makepkg.log 2>&1) || { tail -30 "$WORK/arch/makepkg.log"; exit 1; }
cp "$WORK/arch/out/glance-image-viewer-$VERSION"-*-x86_64.pkg.tar.zst "$OUT/glance-image-viewer-x86_64.pkg.tar.zst"
fi

# --- Flatpak ----------------------------------------------------------------
if want flatpak; then
say "Flatpak"
if command -v flatpak-builder > /dev/null; then
    builder=(flatpak-builder)
else
    # The Flathub build of flatpak-builder. It hands its own data folder to
    # the host's Flatpak, which would then look for the SDK in the wrong
    # place: the real one is named outright.
    builder=(flatpak run --env=FLATPAK_USER_DIR="$HOME/.local/share/flatpak" --command=flatpak-builder org.flatpak.Builder)
fi
"${builder[@]}" --user --jobs="$JOBS" --force-clean --disable-rofiles-fuse \
    --state-dir=.flatpak-builder --repo=.flatpak-builder/repo \
    build-dir build-aux/$APPID.yaml > "$WORK/flatpak.log" 2>&1 || { tail -30 "$WORK/flatpak.log"; exit 1; }
flatpak build-bundle .flatpak-builder/repo "$OUT/Glance-x86_64.flatpak" $APPID \
    --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo
fi

# --- AppImage ---------------------------------------------------------------
if want appimage; then
say "AppImage"
TOOLS="$WORK/appimage-tools"
mkdir -p "$TOOLS"
for url in \
    https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage \
    https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh; do
    [ -s "$TOOLS/$(basename "$url")" ] || curl -fsSL -o "$TOOLS/$(basename "$url")" "$url"
done
chmod +x "$TOOLS"/*
rm -rf "$WORK/appimage" && mkdir -p "$WORK/appimage/src" "$WORK/appimage/out"
git archive HEAD | tar -x -C "$WORK/appimage/src"
# The host's network, and the host's way of finding names on it: Docker's
# own bridge often cannot resolve names, and DNS servers set in Docker's
# daemon.json may not be reachable from every network this computer is on.
docker run --rm --network host -v /etc/resolv.conf:/etc/resolv.conf:ro \
    --memory=5g -e JOBS="$JOBS" -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
    -v "$WORK/appimage/src:/src:ro" -v "$TOOLS:/tools:ro" -v "$WORK/appimage/out:/out" \
    -v "$ROOT/packaging/appimage/build-in-container.sh:/build.sh:ro" \
    ubuntu:24.04 bash /build.sh > "$WORK/appimage.log" 2>&1 || { tail -30 "$WORK/appimage.log"; exit 1; }
cp "$WORK/appimage/out/Glance-x86_64.AppImage" "$OUT/"
fi

# --- Checksums --------------------------------------------------------------
cd "$OUT"
if want arch && want flatpak && want appimage; then
    sha256sum glance-image-viewer-x86_64.pkg.tar.zst Glance-x86_64.flatpak Glance-x86_64.AppImage > SHA256SUMS
fi
say "Built Glance $VERSION in $OUT"
ls -la "$OUT"
