# Fedora keeps `glance` free — only `glances`, the monitoring tool, is taken —
# so this one gets the name the user expects to type.
%global appid io.github.abhilesh1412.Glance

Name:           glance
Version:        1.0.0
Release:        %autorelease
Summary:        A fast, native image viewer for Linux

License:        GPL-3.0-or-later
URL:            https://github.com/ABHILESH1412/glance
Source0:        %{url}/archive/refs/tags/v%{version}/%{name}-%{version}.tar.gz
# Fedora builds with the network off, so every crate has to be in the sources.
# Made with:  cargo vendor && tar caf glance-%%{version}-vendor.tar.xz vendor/
Source1:        %{name}-%{version}-vendor.tar.xz

BuildRequires:  cargo
BuildRequires:  rust >= 1.75
BuildRequires:  make
# For glib-compile-resources, which build.rs uses to put the icons in the binary.
BuildRequires:  glib2-devel
BuildRequires:  pkgconfig(gtk4) >= 4.14
BuildRequires:  pkgconfig(libadwaita-1) >= 1.5
BuildRequires:  pkgconfig(libheif)
BuildRequires:  pkgconfig(poppler-glib)
BuildRequires:  desktop-file-utils
BuildRequires:  libappstream-glib

# The AUR has an unrelated `glance` dashboard and Debian has OpenStack's; on
# Fedora neither is packaged, so /usr/bin/glance is ours without a fight.

%description
Glance opens an image and gets out of the way. Arrow keys walk through the
folder, the scroll wheel zooms, two fingers on a touchpad pan, and the picture
is on screen before you have finished letting go of the mouse.

When you do need to change something, the editor is one button away: crop,
resize, rotate to any angle, adjust brightness, contrast and saturation, draw
and annotate, add text, convert between formats, or compress a file to a size
you name.

It reads PNG, JPEG, WebP, TIFF, GIF, BMP, ICO, QOI, PNM and TGA; HEIC, HEIF and
AVIF; SVG; and camera raw from most makers. Embedded ICC profiles are honoured.

%prep
%autosetup -n %{name}-%{version} -a 1
# Point Cargo at the vendored crates rather than the network.
mkdir -p .cargo
cat > .cargo/config.toml <<'CARGOEOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
CARGOEOF

%build
cargo build --release --locked --offline

%install
%make_install PREFIX=%{_prefix}

%check
cargo test --release --locked --offline
desktop-file-validate %{buildroot}%{_datadir}/applications/%{appid}.desktop
appstream-util validate-relax --nonet %{buildroot}%{_metainfodir}/%{appid}.metainfo.xml

%files
%license %{_datadir}/licenses/%{name}/LICENSE
%doc README.md
%{_bindir}/%{name}
%{_datadir}/applications/%{appid}.desktop
%{_metainfodir}/%{appid}.metainfo.xml
%{_datadir}/icons/hicolor/scalable/apps/%{appid}.svg
%{_datadir}/icons/hicolor/symbolic/apps/%{appid}-symbolic.svg

%changelog
%autochangelog
