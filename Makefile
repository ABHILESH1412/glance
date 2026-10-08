# Building and installing Glance.
#
# cargo builds the program; this puts it, and the five files a desktop needs in
# order to know it exists, where they belong. Without them the program runs but
# never appears in a launcher, is never offered for a JPEG, and shows up with a
# blank icon.

PREFIX  ?= /usr/local
DESTDIR ?=
APPID    = io.github.abhilesh1412.Glance
CARGO   ?= cargo
# The distribution's name for the package, which is not always "glance": the
# AUR and Debian both already have an unrelated one, so their builds pass
# PKGNAME=glance-image-viewer and the licence lands where their tools look.
PKGNAME ?= glance

BINDIR   = $(DESTDIR)$(PREFIX)/bin
# Live Text's helper is started by Glance, not by people, so it lives out of
# the way, where Glance looks for it.
HELPERDIR = $(DESTDIR)$(PREFIX)/libexec/glance
DATADIR  = $(DESTDIR)$(PREFIX)/share
ICONDIR  = $(DATADIR)/icons/hicolor

.PHONY: all build install uninstall check clean release release-minor release-major packages

all: build

build:
	$(CARGO) build --release --locked

check:
	$(CARGO) test --locked
	desktop-file-validate data/$(APPID).desktop
	appstreamcli validate --no-net data/$(APPID).metainfo.xml

# Deliberately not dependent on `build`: this is the target run under sudo, and
# cargo run as root builds into root's home and leaves root-owned files behind.
# Build as yourself, install as root.
install:
	@test -x target/release/glance || { \
		echo "target/release/glance is missing — run 'make' first, as your own user."; \
		exit 1; }
	install -Dm755 target/release/glance $(BINDIR)/glance
	install -Dm755 target/release/glance-ocr $(HELPERDIR)/glance-ocr
	install -Dm644 data/$(APPID).desktop $(DATADIR)/applications/$(APPID).desktop
	install -Dm644 data/$(APPID).metainfo.xml $(DATADIR)/metainfo/$(APPID).metainfo.xml
	install -Dm644 data/icons/hicolor/scalable/apps/$(APPID).svg \
		$(ICONDIR)/scalable/apps/$(APPID).svg
	install -Dm644 data/icons/hicolor/symbolic/apps/$(APPID)-symbolic.svg \
		$(ICONDIR)/symbolic/apps/$(APPID)-symbolic.svg
	install -Dm644 LICENSE $(DATADIR)/licenses/$(PKGNAME)/LICENSE
# Only when installing for real. A packaging tool staging into DESTDIR runs
# these itself, at the right moment, for the whole package at once.
ifeq ($(DESTDIR),)
	-gtk-update-icon-cache -qtf $(ICONDIR)
	-update-desktop-database -q $(DATADIR)/applications
endif

uninstall:
	rm -f $(BINDIR)/glance
	rm -f $(HELPERDIR)/glance-ocr
	-rmdir $(HELPERDIR)
	rm -f $(DATADIR)/applications/$(APPID).desktop
	rm -f $(DATADIR)/metainfo/$(APPID).metainfo.xml
	rm -f $(ICONDIR)/scalable/apps/$(APPID).svg
	rm -f $(ICONDIR)/symbolic/apps/$(APPID)-symbolic.svg
	rm -f $(DATADIR)/licenses/$(PKGNAME)/LICENSE
ifeq ($(DESTDIR),)
	-gtk-update-icon-cache -qtf $(ICONDIR)
	-update-desktop-database -q $(DATADIR)/applications
endif

clean:
	$(CARGO) clean

# Releasing, once your work is committed: version bumped, tested, tagged,
# built, pushed and published on GitHub. See scripts/release.sh.
release:
	scripts/release.sh patch

release-minor:
	scripts/release.sh minor

release-major:
	scripts/release.sh major

# The three packages for the commit checked out, without releasing them.
packages:
	scripts/build-packages.sh target/packaging/release/local
