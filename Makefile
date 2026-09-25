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

BINDIR   = $(DESTDIR)$(PREFIX)/bin
DATADIR  = $(DESTDIR)$(PREFIX)/share
ICONDIR  = $(DATADIR)/icons/hicolor

.PHONY: all build install uninstall check clean

all: build

build:
	$(CARGO) build --release --locked

check:
	$(CARGO) test --locked
	desktop-file-validate data/$(APPID).desktop
	appstreamcli validate --no-net data/$(APPID).metainfo.xml

install: build
	install -Dm755 target/release/glance $(BINDIR)/glance
	install -Dm644 data/$(APPID).desktop $(DATADIR)/applications/$(APPID).desktop
	install -Dm644 data/$(APPID).metainfo.xml $(DATADIR)/metainfo/$(APPID).metainfo.xml
	install -Dm644 data/icons/hicolor/scalable/apps/$(APPID).svg \
		$(ICONDIR)/scalable/apps/$(APPID).svg
	install -Dm644 data/icons/hicolor/symbolic/apps/$(APPID)-symbolic.svg \
		$(ICONDIR)/symbolic/apps/$(APPID)-symbolic.svg
	install -Dm644 LICENSE $(DATADIR)/licenses/glance/LICENSE
# Only when installing for real. A packaging tool staging into DESTDIR runs
# these itself, at the right moment, for the whole package at once.
ifeq ($(DESTDIR),)
	-gtk-update-icon-cache -qtf $(ICONDIR)
	-update-desktop-database -q $(DATADIR)/applications
endif

uninstall:
	rm -f $(BINDIR)/glance
	rm -f $(DATADIR)/applications/$(APPID).desktop
	rm -f $(DATADIR)/metainfo/$(APPID).metainfo.xml
	rm -f $(ICONDIR)/scalable/apps/$(APPID).svg
	rm -f $(ICONDIR)/symbolic/apps/$(APPID)-symbolic.svg
	rm -f $(DATADIR)/licenses/glance/LICENSE
ifeq ($(DESTDIR),)
	-gtk-update-icon-cache -qtf $(ICONDIR)
	-update-desktop-database -q $(DATADIR)/applications
endif

clean:
	$(CARGO) clean
