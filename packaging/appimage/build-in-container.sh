#!/bin/bash
# SPDX-FileCopyrightText: 2026 Abhilesh Singh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Builds the AppImage. Run by scripts/build-packages.sh inside ubuntu:24.04,
# the oldest system it is meant to run on: an AppImage needs a glibc at least
# as new as the one it was built against, so building on Ubuntu 24.04 (2.39)
# makes one that runs on 24.04 and everything newer.
#
# /src is the source tree (read-only), /tools holds linuxdeploy and its GTK
# plugin, /out receives Glance-x86_64.AppImage. JOBS keeps memory in check.
set -euxo pipefail
export DEBIAN_FRONTEND=noninteractive
APPID=io.github.abhilesh1412.Glance
JOBS=${JOBS:-4}

apt-get update
apt-get install -y --no-install-recommends \
  build-essential ca-certificates curl git pkg-config cmake ninja-build file patchelf \
  libgtk-4-dev libadwaita-1-dev libheif-dev libsoup-3.0-dev libglib2.0-dev libglib2.0-dev-bin \
  libcairo2-dev libfreetype-dev libfontconfig-dev libjpeg-dev libpng-dev libtiff-dev \
  libopenjp2-7-dev liblcms2-dev zlib1g-dev \
  libheif-plugin-libde265 libheif-plugin-dav1d libheif-plugin-x265 libheif-plugin-aomenc \
  librsvg2-common libgdk-pixbuf2.0-bin shared-mime-info adwaita-icon-theme xz-utils

# Poppler and qpdf as new as the Flatpak's: Ubuntu's Poppler is too old for
# drawing on PDFs.
mkdir -p /build && cd /build
curl -sSLO https://poppler.freedesktop.org/poppler-26.09.0.tar.xz
echo "8059eadb6805340768f138c465b57f8164c92b4a0773c37ef031ea6c0d987b2e  poppler-26.09.0.tar.xz" | sha256sum -c
tar xf poppler-26.09.0.tar.xz
cmake -S poppler-26.09.0 -B poppler-build -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr/local \
  -DCMAKE_INSTALL_LIBDIR=lib -DENABLE_GLIB=ON -DENABLE_BOOST=OFF -DENABLE_CPP=OFF -DENABLE_QT5=OFF -DENABLE_QT6=OFF \
  -DENABLE_UTILS=OFF -DENABLE_LIBCURL=OFF -DENABLE_NSS3=OFF -DENABLE_GPGME=OFF -DENABLE_GOBJECT_INTROSPECTION=OFF \
  -DBUILD_GTK_TESTS=OFF -DBUILD_CPP_TESTS=OFF -DBUILD_QT5_TESTS=OFF -DBUILD_QT6_TESTS=OFF -DBUILD_MANUAL_TESTS=OFF
cmake --build poppler-build -j $JOBS && cmake --install poppler-build

git clone --depth 1 --branch v12.4.2 https://github.com/qpdf/qpdf.git
cmake -S qpdf -B qpdf-build -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr/local -DCMAKE_INSTALL_LIBDIR=lib \
  -DUSE_IMPLICIT_CRYPTO=OFF -DREQUIRE_CRYPTO_NATIVE=ON -DBUILD_STATIC_LIBS=OFF -DBUILD_DOC=OFF \
  -DINSTALL_EXAMPLES=OFF -DINSTALL_MANUAL=OFF
cmake --build qpdf-build -j $JOBS && cmake --install qpdf-build
ldconfig
export PKG_CONFIG_PATH=/usr/local/lib/pkgconfig

curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
. "$HOME/.cargo/env"
export CARGO_BUILD_JOBS=$JOBS

cp -a /src /work && cd /work
rm -rf target
cargo build --release --locked

rm -rf AppDir
make install DESTDIR=/work/AppDir PREFIX=/usr

# libheif finds its decoders and encoders as plugins, by a fixed path that
# would be the host's: bundle them, and point libheif at them when it runs.
mkdir -p AppDir/usr/lib/libheif/plugins AppDir/apprun-hooks
cp /usr/lib/x86_64-linux-gnu/libheif/plugins/*.so AppDir/usr/lib/libheif/plugins/
cat > AppDir/apprun-hooks/libheif-plugins.sh <<'EOF'
export LIBHEIF_PLUGIN_PATH="$APPDIR/usr/lib/libheif/plugins"
EOF

export APPIMAGE_EXTRACT_AND_RUN=1 DEPLOY_GTK_VERSION=4 ARCH=x86_64
export LDAI_OUTPUT="/out/Glance-x86_64.AppImage"
export LD_LIBRARY_PATH=/usr/local/lib
cp /tools/linuxdeploy-x86_64.AppImage /tools/linuxdeploy-plugin-gtk.sh /build/
chmod +x /build/linuxdeploy-x86_64.AppImage /build/linuxdeploy-plugin-gtk.sh
# libadwaita draws its own light and dark styles and follows Glance's setting;
# the plugin's forced theme and X11 would override both, so they go.
sed -i -e '/^export GTK_THEME=/d' -e '/^export GDK_BACKEND=x11/d' /build/linuxdeploy-plugin-gtk.sh
if grep -q "GDK_BACKEND=x11\|^export GTK_THEME" /build/linuxdeploy-plugin-gtk.sh; then exit 1; fi
export PATH="/build:$PATH"
/build/linuxdeploy-x86_64.AppImage --appdir AppDir \
  --executable AppDir/usr/bin/glance \
  --executable AppDir/usr/libexec/glance/glance-ocr \
  --deploy-deps-only AppDir/usr/lib/libheif/plugins \
  --desktop-file AppDir/usr/share/applications/$APPID.desktop \
  --icon-file AppDir/usr/share/icons/hicolor/scalable/apps/$APPID.svg \
  --plugin gtk \
  --output appimage
chown -R "$HOST_UID:$HOST_GID" /out
ls -la /out
