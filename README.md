<div align="center">

<img src="data/compare/glance.png" width="96" alt="Glance">

# Glance

**A fast, native image and PDF viewer for Linux.**

It opens a file, shows it properly, and gets out of the way.

</div>

![Glance showing a photograph, with the rest of the folder along the bottom](data/screenshots/viewing.png)

## What it is

An image viewer first: photographs, screenshots, vector art and camera raw all open in
the same window, and browsing a folder is a matter of pressing an arrow key. PDFs open
in the same window too, and can be searched, highlighted, annotated, drawn and written
on, with every change saved into the file where any PDF reader will see it.

When you do need to change something, the editor is one button away — crop, resize,
rotate, adjust the colour, draw on it, add text, convert it to another format, or squeeze
it down to a file size someone asked for.

## Why

Most Linux image viewers are either fast but dated, or modern-looking but sluggish for
what is fundamentally a pixel-blitting problem. Glance aims at both: a GNOME-native
window that starts instantly and stays smooth on a 50-megapixel photograph.

Three decisions follow from that:

- **Rust**, because an image viewer is a parser for untrusted binary data. Image decoders
  are a classic source of memory-corruption bugs — libjpeg, libpng and WebP have all had
  serious ones. Rust removes that whole class of defect from the decoding path.
- **The GPU does the drawing.** Zooming and panning are transforms, not re-renders. Where
  an SVG maps onto GTK's own render nodes it is translated into them once at load, so
  zooming never re-rasterises anything. On one logo with three nested drop shadows,
  zooming to 23× cost 571 MB and 11.6 s through a rasteriser, and 0 MB and 1.0 s this way.
- **Colour is handled once, at the door.** Every decoder converts to 8-bit sRGB before
  the picture reaches anything else, so a pixel means the same thing in the tone sliders,
  under the text and drawings, and on the way back out to a file.

Decoding never runs on the main thread, so a huge file cannot freeze the window.

## Features

**Viewing** — fit to window, 100%, free zoom to 32×. Zoom anchors on the pointer, so the
pixel under the cursor stays under it. Two fingers on a touchpad pan and the wheel zooms —
the hardware says which is which, so there is no modifier to remember. Pinch works too.
Animated GIF and WebP play, and keep playing while you zoom or rotate; `F9` lists every
frame down the side with how long it shows, to pause on one or step through them; Copy
copies the frame on screen. Right-click the picture, or a frame, to copy it.

**Browsing** — a filmstrip of the folder along the bottom, arrow keys to step through it,
and the folder is watched so files added elsewhere show up. `F5` plays the folder as a
slideshow, full screen, every 2 to 30 seconds; Space pauses it.

**Image Info** — `Ctrl+I` opens a panel beside the picture with the file's details (kind,
size, dates, dimensions, print resolution, frames) and what the camera wrote into it:
camera, lens, when it was taken, shutter speed, aperture, ISO, focal length, flash, and
where, if the photo has GPS. It reads EXIF from JPEG, PNG, WebP, TIFF, HEIF and camera raw,
and follows along as you browse.

**PDFs** — every page in one scrolling column, one page at a time, or two side by side
like an open book, fitted to the window. Only the pages near the screen are drawn and
held in memory, so a long document costs about what a short one does. Zooming redraws
the pages sharp once it stops moving. The header stays quiet — pages, open, the page
box, search and the menu — and the menu opens on a row of zoom, rotate and fullscreen
buttons, which leave it open so you can press them again, and a row of highlight
colours.

*Reading*

- A **sidebar** (F9, or the button at the top left) with three tabs, and it remembers
  the last one you used:
  - **Pages**: every page as a thumbnail, drawn only while it can be seen. Click one to
    go there; it follows along as you scroll
  - **Contents**: the document's own table of contents, opened as far as its author
    left it. Clicking a heading takes you to the heading itself, not just the top of its
    page, and the section you are reading is picked out as you go
  - **Bookmarks**: pages you marked to come back to, with Ctrl+D or from the menu. Each
    is named after the section it is in, and can be renamed or removed from its row's
    menu; a removal can be undone. Bookmarks are kept with Glance's settings, not
    written into the PDF, and they follow a file that is moved or renamed
- A **page number box** in the header: type a number and press Enter to go there
- **Search** with Ctrl+F. Every match is marked, the current one in orange, and the
  count grows while a long document is still being searched — it never freezes the
  window. Case and accents do not matter, and a phrase broken across two lines is still
  found, as one match. The first match shown is the first from the page you are on
- **Select text** by dragging, a word with a double-click, a line with a triple-click,
  across pages if you like, and copy it with Ctrl+C
- **Night mode**: black pages and white text, with colours keeping their hue, so a red
  heading stays red
- **Document info** (Ctrl+I): title, author, dates, the program that made it, PDF
  version, page sizes by name (A4, Letter…), fonts, restrictions and more
- **Rotate** the pages a quarter turn at a time — for reading, the file is left alone

*Marking up* — every change is an ordinary PDF annotation, saved straight into the file,
so every other PDF reader shows it too. Ctrl+Z and Ctrl+Shift+Z undo and redo all of
it, and a read-only file is left untouched.

- **Highlight, underline or strike through** selected text with Ctrl+H, Ctrl+U or
  Ctrl+Shift+X, or from the right-click menu. Highlights come in six colours from the
  menu, or any other; marking text the same way again takes the mark off
- **Notes and speech bubbles**: right-click where one should go. A note is a
  sticky-note icon that opens when clicked; a speech bubble writes its words on the
  page, with a tail pointing at the spot. Click either to edit or delete it, and drag it
  to move it
- **Edit** (Ctrl+E), a panel of three sections: **Draw**, with the image editor's pen,
  highlighter, line, arrow, rectangle and ellipse, in any colour and thickness; **Text**,
  for text boxes in any font, size, colour and background, bold or italic — click one to
  change it, drag it to move it; and **Redact** (see below). Opening another PDF puts the
  panel away

*Protecting and shrinking*

- **Password-protected PDFs** open after asking for the password, right in the window.
  What their author does not allow — copying text, marking up — Glance does not do
  either, unless the document was opened with its permissions password
- **Password** (in the menu): require a password to open the document, or take it
  off. Saved with AES-256, the strongest encryption PDF has. A document someone else
  restricted asks for its permissions password before any of it changes
- **Reduce File Size** (in the menu): repack the document without changing anything you
  can see, or also scale its photos down to 150 or 96 dots per inch. The new size is
  worked out first, so you see it before deciding. Drawings, charts and screenshots are
  left sharp, and nothing is saved that came out bigger

Both save over the file or as a copy beside it, and Glance reopens the document at the
page you were on.

## Combine into PDF

Pages from any number of PDFs and pictures, put together as one new PDF. Start it from
the window Glance opens with, from **Combine into PDF…** in the menu (with what is open
already in), or by opening or dropping several files at once.

- **Add files** with the button, Ctrl+O, or by dropping them in — between two pages to
  put them there. A PDF of more than one page asks which pages to take: all of them, or
  a list such as `2-5, 9`. A protected PDF asks for its password.
- **Every page in a grid**, with its number, the file it came from, and a stripe in that
  file's colour, so after any amount of shuffling it is still plain where a page came
  from. A slider sets how big the pages are drawn.
- **Arrange**: drag pages into order. Click to pick a page, Ctrl+click to pick more,
  Shift+click for a run; then drag them together, turn them with [ and ], or take them
  out with Delete. Ctrl+← and Ctrl+→ move them along. Pointing at a page shows buttons to
  turn it either way or take it out. Everything can be undone.
- **Look at a page** across the whole window with a double-click or Enter; the arrow keys
  step through, and Escape goes back to the grid where you were.
- **Save as PDF…** writes a new file — the files the pages came from are never changed.
  PDF pages are copied as they are, text, links and notes included. Pictures go on A4,
  Letter, or a page their own size: Glance asks which, and remembers the answer for next
  time. A JPEG goes in exactly as it was, not compressed again.

## Redacting

A black box drawn over a document hides nothing. The words under it are still in the
file, and any program can copy them out or lift the box off. Glance's redaction removes
what is under the box for good, in pictures and PDFs alike.

It happens in two steps, so nothing goes by accident:

1. **Mark** what should go, in the **Redact** section of the Edit panel: drag a box over
   anything (**Area**), or select text and have it marked as you let go (**Text**). Or
   select text anywhere and press Ctrl+Shift+R, or right-click → Redact. Marked areas show
   dark but see-through, edged in red, so you can check what each one covers. Undo, or
   right-click → Remove Redaction Mark, takes one back. Nothing has changed yet.
2. **Apply**: a bar says how many areas are marked, and its Apply button asks what to do.
   **Save Redacted Copy…** (the default) leaves the original as it was; **Redact
   Original** changes the file itself. Saving an edited picture with marked areas asks the
   same question.

How it is made unrecoverable:

- **Pictures**: the marked pixels are replaced with solid black before the file is
  written, one pixel past the edge all round. Glance writes only pixels, so no embedded
  thumbnail or metadata keeps a copy of the original.
- **PDFs**: every page with a redaction is replaced with a picture of itself, the boxes
  painted in, so none of its text, fonts, drawings, hidden layers, annotations or form
  values survive. The words outside the boxes go back as invisible text, so the page can
  still be searched and copied from; a word touched anywhere by a box goes whole. The
  file is then written afresh, which drops earlier versions of the page that incremental
  saves keep. Finally Glance opens the result and checks that no text it removed is left.
- Either way, the thumbnails desktop programs keep of the file are thrown away, so they
  do not go on showing the original.

## Printing

Ctrl+P, or Print in the menu, for pictures and PDFs alike, through the system's print
dialog. A PDF prints page by page, each page fitted to the paper and turned to match it.
A picture prints as it is on screen, edits included, at a natural size, shrunk to fit
the paper when it is larger. A PDF whose author does not allow printing is not printed.

A PDF is read on its own: the Edit and Delete bar and the filmstrip are for images, and
arrow keys do not step to the next file. PDFs do not appear in an image folder's
filmstrip either.

**Keyboard shortcuts** — the menus stay short, without a shortcut written beside every
item; instead **Keyboard Shortcuts** in the menu, or Ctrl+?, opens one window listing
them all, for pictures and for PDFs, with a search box.

**Colours** are picked from GTK's own palette and colour editor, in a dialog that grows
and shrinks to fit whichever is showing.

**Editing** — one panel, one section at a time:

| Section | What it does |
|---|---|
| **Rotate & Flip** | Quarter turns, or any angle typed into the box. A separate header button turns the picture 90° just to look at it, without changing the file |
| **Crop** | Drag a rectangle, pull any of its eight handles, or pick an aspect preset |
| **Resize** | Handles on the picture or numbers in the panel, kept in step, with an optional aspect lock. Sizes in pixels, percent, inches or centimetres, and the resolution in pixels per inch — resampled to keep the printed size, or not, to keep every pixel and change only the print size. The resolution is read from the file and saved with it |
| **Adjust** | Light: exposure, brightness, contrast, highlights, shadows. Colour: saturation, temperature, tint, sepia. Detail: sharpness, or softness below zero. Whatever is a colour matrix shows instantly on the GPU; the rest is worked out on a screen-sized copy while the slider moves and on the whole picture once it rests. Saving bakes exactly what was shown |
| **Levels** | The histogram, with handles for the black point, midtones and white point, per channel or all together, and **Auto Levels**, which also takes out a colour cast |
| **Draw** | Pen, highlighter, line, arrow, rectangle, ellipse — with the buttons drawing their own shapes |
| **Text** | Font, size, colour, background, bold, italic, underline, dragged anywhere |
| **Export** | Any format the picture can honestly become, with a quality dial — or a file size to aim for |

That last one is the unusual bit: give it a number and it will compress *or* inflate the
file to hit it, which is what people normally hand to a shady upload site to get done.

Nothing is written until you say so, and saving over the original asks first.

**Formats**

| Family | Formats |
|---|---|
| Raster | PNG, JPEG, GIF, WebP, TIFF, BMP, ICO, QOI, TGA, PNM |
| Modern | HEIC / HEIF, AVIF |
| Vector | SVG, SVGZ |
| Camera raw | CR2, CR3, CRW, NEF, NRW, ARW, DNG, RAF, ORF, RW2, PEF, SRW, and 14 more |

EXIF orientation is honoured, so phone photos are upright. So are ICC profiles: an Adobe
RGB or Display P3 photograph is converted to sRGB on the way in rather than shown as
though its numbers meant something else. About 27 ms on a 12-megapixel photograph;
recognising an already-sRGB profile and skipping it costs 2 µs.

## How it compares

Against the image viewers people actually use on Linux. Compiled from each project's own
documentation in September 2026 — features move, so check upstream if one matters to you.

<table>
<tr>
<th align="left">Speed and rendering</th>
<th align="center"><img src="data/compare/glance.png" width="34" alt="Glance"><br>Glance</th>
<th align="center"><img src="data/compare/loupe.png" width="34" alt="Loupe"><br>Loupe</th>
<th align="center"><img src="data/compare/gthumb.png" width="34" alt="gThumb"><br>gThumb</th>
<th align="center"><img src="data/compare/gwenview.png" width="34" alt="Gwenview"><br>Gwenview</th>
<th align="center"><img src="data/compare/qview.png" width="34" alt="qView"><br>qView</th>
<th align="center"><img src="data/compare/nomacs.png" width="34" alt="nomacs"><br>nomacs</th>
</tr>
<tr><td>GPU-accelerated rendering</td><td align="center">✅</td><td align="center">✅</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td></tr>
<tr><td>SVG zoom with no re-rasterising</td><td align="center">✅</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td></tr>
<tr><td>Memory-safe decoders (Rust)</td><td align="center">✅</td><td align="center">✅</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td></tr>
<tr><td>Two-finger pan and pinch zoom</td><td align="center">✅</td><td align="center">✅</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td></tr>
<tr><td>Modern toolkit (GTK 4 / libadwaita)</td><td align="center">✅</td><td align="center">✅</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td></tr>
</table>

<table>
<tr>
<th align="left">Editing</th>
<th align="center"><img src="data/compare/glance.png" width="34" alt="Glance"><br>Glance</th>
<th align="center"><img src="data/compare/loupe.png" width="34" alt="Loupe"><br>Loupe</th>
<th align="center"><img src="data/compare/gthumb.png" width="34" alt="gThumb"><br>gThumb</th>
<th align="center"><img src="data/compare/gwenview.png" width="34" alt="Gwenview"><br>Gwenview</th>
<th align="center"><img src="data/compare/qview.png" width="34" alt="qView"><br>qView</th>
<th align="center"><img src="data/compare/nomacs.png" width="34" alt="nomacs"><br>nomacs</th>
</tr>
<tr><td>Rotate and flip</td><td align="center">✅</td><td align="center">✅</td><td align="center">✅</td><td align="center">✅</td><td align="center">✅</td><td align="center">✅</td></tr>
<tr><td>Rotate to any angle</td><td align="center">✅</td><td align="center">❌</td><td align="center">✅</td><td align="center">❌</td><td align="center">❌</td><td align="center">✅</td></tr>
<tr><td>Crop</td><td align="center">✅</td><td align="center">✅</td><td align="center">✅</td><td align="center">✅</td><td align="center">❌</td><td align="center">✅</td></tr>
<tr><td>Resize to exact pixels</td><td align="center">✅</td><td align="center">❌</td><td align="center">✅</td><td align="center">✅</td><td align="center">❌</td><td align="center">✅</td></tr>
<tr><td>Brightness / contrast / saturation</td><td align="center">✅</td><td align="center">❌</td><td align="center">✅</td><td align="center">✅</td><td align="center">❌</td><td align="center">✅</td></tr>
<tr><td>Draw, shapes and text on the image</td><td align="center">✅</td><td align="center">❌</td><td align="center">❌</td><td align="center">✅</td><td align="center">❌</td><td align="center">✅</td></tr>
<tr><td>Convert format on save, with a quality dial</td><td align="center">✅</td><td align="center">❌</td><td align="center">✅</td><td align="center">✅</td><td align="center">❌</td><td align="center">✅</td></tr>
<tr><td><strong>Compress or inflate to a target file size</strong></td><td align="center">✅</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td></tr>
</table>

<table>
<tr>
<th align="left">Browsing and extras</th>
<th align="center"><img src="data/compare/glance.png" width="34" alt="Glance"><br>Glance</th>
<th align="center"><img src="data/compare/loupe.png" width="34" alt="Loupe"><br>Loupe</th>
<th align="center"><img src="data/compare/gthumb.png" width="34" alt="gThumb"><br>gThumb</th>
<th align="center"><img src="data/compare/gwenview.png" width="34" alt="Gwenview"><br>Gwenview</th>
<th align="center"><img src="data/compare/qview.png" width="34" alt="qView"><br>qView</th>
<th align="center"><img src="data/compare/nomacs.png" width="34" alt="nomacs"><br>nomacs</th>
</tr>
<tr><td>Camera raw, HEIC / AVIF and SVG</td><td align="center">✅</td><td align="center">✅</td><td align="center">✅</td><td align="center">✅</td><td align="center">✅</td><td align="center">✅</td></tr>
<tr><td>ICC colour management</td><td align="center">✅</td><td align="center">✅</td><td align="center">✅</td><td align="center">✅</td><td align="center">✅</td><td align="center">❌</td></tr>
<tr><td>Filmstrip of the folder</td><td align="center">✅</td><td align="center">❌</td><td align="center">✅</td><td align="center">✅</td><td align="center">❌</td><td align="center">✅</td></tr>
<tr><td>Thumbnail browser / file manager</td><td align="center">❌</td><td align="center">❌</td><td align="center">✅</td><td align="center">✅</td><td align="center">❌</td><td align="center">✅</td></tr>
<tr><td>EXIF / metadata panel</td><td align="center">❌</td><td align="center">✅</td><td align="center">✅</td><td align="center">✅</td><td align="center">❌</td><td align="center">✅</td></tr>
<tr><td>Slideshow</td><td align="center">❌</td><td align="center">❌</td><td align="center">✅</td><td align="center">✅</td><td align="center">✅</td><td align="center">✅</td></tr>
<tr><td>Batch processing</td><td align="center">❌</td><td align="center">❌</td><td align="center">✅</td><td align="center">❌</td><td align="center">❌</td><td align="center">✅</td></tr>
<tr><td>Tags, catalogs, albums</td><td align="center">❌</td><td align="center">❌</td><td align="center">✅</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td></tr>
<tr><td>Read, search and mark up PDFs</td><td align="center">✅</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td></tr>
<tr><td>Windows and macOS builds</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td><td align="center">✅</td><td align="center">✅</td></tr>
</table>

Icons are each project's own, from their Flathub listings, used to identify them.

**The short version.** Glance is the fastest and the most capable editor of the six, and
the only one that can hit a file size on request — or open a PDF and mark it up. It is not a photo library: if you want
tags, catalogs, batch jobs or a camera import wizard, gThumb is still the answer, and
nomacs if you need Windows too. Loupe is the closest in spirit — same toolkit, same
language — but stops at crop and rotate.

## Installing

Two ways in. **From source** is the short one if your distribution is recent enough;
**Flatpak** works anywhere, because GTK, libadwaita and libheif come from the GNOME
runtime instead of from your system.

### From source

**1. Install what it builds against.** GTK 4.14 and libadwaita 1.5 or newer — the
versions in Ubuntu 24.04 LTS, so anything that recent will do — plus libheif for HEIC and
AVIF, Poppler's GLib library and qpdf for PDFs, Rust, `make`, and glib's resource
compiler, which the build uses to put the icons inside the binary. Drawing on PDFs needs Poppler 25.06,
and text boxes in a chosen font 24.12; with an older Poppler, PDFs open and the rest works,
and those two tools say what they need. Everything else is pure Rust and comes in through Cargo.

```bash
# Arch
sudo pacman -S gtk4 libadwaita libheif poppler-glib qpdf glib2 rust make
# Fedora
sudo dnf install gtk4-devel libadwaita-devel libheif-devel poppler-glib-devel qpdf-devel glib2-devel cargo make
# Debian, Ubuntu
sudo apt install libgtk-4-dev libadwaita-1-dev libheif-dev libpoppler-glib-dev libqpdf-dev libglib2.0-dev cargo make
```

**2. Get the source.**

```bash
git clone https://github.com/ABHILESH1412/glance.git
cd glance
```

**3. Build and install.** The first build takes a few minutes; it is compiling every
dependency once.

```bash
make                       # cargo build --release
sudo make install          # into /usr/local
```

Use `sudo make install PREFIX=/usr` if you would rather it went where your package
manager puts things. `make install` also places the desktop entry, the icons and the
AppStream metainfo — without those the program runs but never appears in a launcher or as
a handler for a JPEG. Packagers: `DESTDIR` works as usual and the icon and desktop caches
are left alone when it is set.

**4. Run it.** `glance` is now on your path, and Glance is in your launcher and in the
"Open With" menu for images.

```bash
glance path/to/image.jpg
```

To remove it again:

```bash
sudo make uninstall
```

### Flatpak

**1. Install the build tool, and the runtime it builds against.** The runtime is about a
gigabyte and is shared with every other GNOME Flatpak you have.

```bash
sudo pacman -S flatpak flatpak-builder                    # Arch
sudo dnf install flatpak flatpak-builder                  # Fedora
sudo apt install flatpak flatpak-builder                  # Debian, Ubuntu

flatpak remote-add --if-not-exists --user \
    flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user flathub \
    org.gnome.Platform//49 \
    org.gnome.Sdk//49 \
    org.freedesktop.Sdk.Extension.rust-stable//25.08
```

**2. Get the source.**

```bash
git clone https://github.com/ABHILESH1412/glance.git
cd glance
```

**3. Build and install.** This builds the checkout you are standing in, so there is
nothing to tag or push first.

```bash
flatpak-builder --user --install --force-clean build-dir \
    build-aux/io.github.abhilesh1412.Glance.yaml
```

**4. Run it.**

```bash
flatpak run io.github.abhilesh1412.Glance path/to/image.jpg
```

To remove it again:

```bash
flatpak uninstall --user io.github.abhilesh1412.Glance
```

<details>
<summary>If you change the dependencies</summary>

Flatpak builds with the network switched off, so every crate has to be declared in
advance. [`build-aux/cargo-sources.json`](build-aux/cargo-sources.json) is that list, and
it is checked in — you only need to regenerate it if `Cargo.lock` changes:

```bash
curl -O https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py

# In a virtual environment, since most distributions now refuse `pip install`
# into the system Python.
python3 -m venv /tmp/fcg
/tmp/fcg/bin/pip install aiohttp PyYAML tomlkit
/tmp/fcg/bin/python flatpak-cargo-generator.py Cargo.lock -o build-aux/cargo-sources.json
```

</details>

Packaging recipes for the AUR, Fedora and Debian live in
[`packaging/`](packaging), and [`docs/PUBLISHING.md`](docs/PUBLISHING.md) is the
step-by-step for getting onto Flathub and into each distribution's archive.

## Development

`make` wraps `cargo build --release`, but a release link takes minutes because of LTO.
There is a `quick` profile for day-to-day work — optimised, without the LTO — which you
want for anything involving camera raw or SVG, since both are unusably slow in a debug
build.

```bash
cargo run --profile quick -- path/to/image.jpg
```

```bash
cargo test     # unit tests
make check     # those, plus desktop-entry and AppStream validation
```

`make check` runs the same validation a distribution or Flathub will run on the way in.
[`testdata/`](testdata/README.md) holds fixtures chosen to catch specific mistakes — an
EXIF-rotated JPEG, a transparent SVG that exposes premultiplied-alpha errors, a corrupt
file, and four CC0 camera raws covering different decode paths.

## Keyboard shortcuts

All of them are listed in the app itself: **Keyboard Shortcuts** in the menu, or
`Ctrl+?`, opens a window with every key, split into General, Pictures and PDFs, with a
search box. The most used:

| Key | Action |
|---|---|
| `Ctrl+O` | Open a file |
| `←` `→` `Space` | Previous / next image |
| `+` `-` `0` `1` | Zoom in, out, fit, 100% |
| `[` `]` | Rotate left / right |
| `Ctrl+E` | Edit panel: draw, write and redact |
| `Ctrl+T` `Ctrl+R` | Rotate and flip / Resize |
| `Ctrl+Z` `Ctrl+Shift+Z` | Undo / redo an edit, or a change to a PDF |
| `Ctrl+S` `Ctrl+Shift+S` | Save in place / Export… |
| `Ctrl+C` | Copy the image, or a PDF's selected text |
| `F11` | Fullscreen |
| `F5` | Slideshow; `Space` pauses it |
| `Ctrl+I` | Image info |
| `F9` `,` `.` `K` | An animation's frames: show them, previous / next frame, play or pause |
| `Delete` | Delete the current image |
| `Esc` | Leave fullscreen, close the panel, or cancel editing |
| `Page Up` `Page Down` `Space` | In a PDF: back / forward a screen |
| `Home` `End` | In a PDF: first / last page |
| `←` `→` | In a PDF: scroll sideways when zoomed in |
| `F9` | In a PDF: show or hide the sidebar (pages, contents, bookmarks) |
| `Ctrl+D` | In a PDF: bookmark the page, or remove its bookmark |
| `Ctrl+F` | In a PDF: find (fullscreen is `F11` there) |
| `Enter` `Shift+Enter` / `F3` `Shift+F3` | In a PDF: next / previous match |
| `Ctrl+H` `Ctrl+U` `Ctrl+Shift+X` | In a PDF: highlight / underline / strike through the selected text |
| `Ctrl+I` | In a PDF: document info |
| `Ctrl+Shift+R` | In a PDF: mark the selected text for redaction |
| `Ctrl+P` | Print |
| `Ctrl+Enter` | Finish writing a note or speech bubble |
| `Ctrl+W` `Ctrl+Q` | Close window / quit |
| `Ctrl+?` | All keyboard shortcuts |

## Known limitations

- **SVG is not an export format.** An SVG can be written out as pixels at any size, but
  pixels cannot be turned back into shapes. To keep an SVG an SVG, copy the file.
- Text and drawings are placed against the picture's current size. Resizing carries them
  along; a crop or rotation applied afterwards will not move them for you.
- Exporting an animated GIF or WebP writes the frame you are looking at.
- "Move to Bin" needs a filesystem that has one. From `/tmp` or some removable media it
  reports that it is unsupported; **Delete Permanently** still works there.
- On a PDF, text boxes, notes and bubbles can be moved but not resized, and drawings
  can be undone but not moved. A PDF text box has no underline.
- A text box in a font of your choosing carries that font inside the file — a few
  hundred kilobytes, once per font per document — so it looks the same everywhere.
- Redacting a PDF page turns it into a picture: it prints and reads the same, but it is
  larger than the text it replaces. Redaction covers what is on the pages; the document's
  title, outline and other details are shown in Document Info and the sidebar, to check.
- Once a file is redacted in place, the old one is gone as far as Glance or any program can
  tell. Backups, snapshots and copies elsewhere are beyond its reach.

Wanted, but not built yet: HDR and wide-gamut output, and progressive loading so a 100 MP
TIFF appears in stages.

## Licence

GPL-3.0-or-later — see [LICENSE](LICENSE). You may use, study, change and share this
program; a changed version you distribute has to stay open under the same terms.
