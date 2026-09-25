<div align="center">

<img src="data/compare/glance.png" width="96" alt="Glance">

# Glance

**A fast, native image viewer for Linux.**

It opens a file, shows it properly, and gets out of the way.

</div>

![Glance showing a photograph, with the rest of the folder along the bottom](data/screenshots/viewing.png)

## What it is

An image viewer first: photographs, screenshots, vector art and camera raw all open in
the same window, and browsing a folder is a matter of pressing an arrow key.

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
Animated GIF and WebP play, and keep playing while you zoom or rotate.

**Browsing** — a filmstrip of the folder along the bottom, arrow keys to step through it,
and the folder is watched so files added elsewhere show up.

**Editing** — one panel, one section at a time:

| Section | What it does |
|---|---|
| **Rotate & Flip** | Quarter turns, or any angle typed into the box. A separate header button turns the picture 90° just to look at it, without changing the file |
| **Crop** | Drag a rectangle, pull any of its eight handles, or pick an aspect preset |
| **Resize** | Handles on the picture or numbers in the panel, kept in step, with an optional aspect lock |
| **Adjust** | Brightness, contrast and saturation, applied live on the GPU and baked identically on save |
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
<tr><td>Windows and macOS builds</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td><td align="center">❌</td><td align="center">✅</td><td align="center">✅</td></tr>
</table>

Icons are each project's own, from their Flathub listings, used to identify them.

**The short version.** Glance is the fastest and the most capable editor of the six, and
the only one that can hit a file size on request. It is not a photo library: if you want
tags, catalogs, batch jobs or a camera import wizard, gThumb is still the answer, and
nomacs if you need Windows too. Loupe is the closest in spirit — same toolkit, same
language — but stops at crop and rotate.

## Installing

**Flatpak** — the one that works everywhere, since GTK, libadwaita and libheif come from
the GNOME runtime:

```bash
flatpak-builder --user --install --force-clean build-dir \
    build-aux/io.github.abhilesh1412.Glance.yaml
```

**From source:**

```bash
sudo make install          # /usr/local
sudo make install PREFIX=/usr
sudo make uninstall
```

`make install` also puts the desktop entry, icons and AppStream metainfo in place; without
those the program runs but never shows up in a launcher or as a handler for a JPEG.
`DESTDIR` works as usual for packagers.

## Building

Needs **GTK 4.14** and **libadwaita 1.5** or newer — the versions in Ubuntu 24.04 LTS —
plus **libheif** for HEIC and AVIF, and a Rust toolchain. Everything else is pure Rust and
comes in through Cargo.

```bash
sudo pacman -S gtk4 libadwaita libheif rust                       # Arch
sudo dnf install gtk4-devel libadwaita-devel libheif-devel cargo  # Fedora
sudo apt install libgtk-4-dev libadwaita-1-dev libheif-dev cargo  # Debian, Ubuntu

cargo build --release
./target/release/glance path/to/image.jpg
```

There is also a `quick` profile — optimised, but without the LTO that makes a release link
take minutes. Use it for anything involving camera raw or SVG, which are unusably slow in
a debug build.

```bash
cargo run --profile quick -- path/to/image.jpg
```

`cargo test` runs the unit tests; `make check` adds the desktop-entry and AppStream
validation that a distribution will run on the way in. [`testdata/`](testdata/README.md)
holds fixtures chosen to catch specific mistakes — an EXIF-rotated JPEG, a transparent SVG
that exposes premultiplied-alpha errors, a corrupt file, and four CC0 camera raws.

## Keyboard shortcuts

| Key | Action |
|---|---|
| `Ctrl+O` | Open an image |
| `←` `→` `Space` | Previous / next image |
| `+` `-` `0` `1` | Zoom in, out, fit, 100% |
| `[` `]` | Rotate left / right |
| `Ctrl+E` | Edit panel |
| `Ctrl+T` `Ctrl+R` | Rotate and flip / Resize |
| `Ctrl+Z` `Ctrl+Shift+Z` | Undo / redo an edit |
| `Ctrl+S` `Ctrl+Shift+S` | Save in place / Export… |
| `Ctrl+C` | Copy the image to the clipboard |
| `F11` | Fullscreen |
| `Delete` | Delete the current image |
| `Esc` | Leave fullscreen, close the panel, or cancel editing |
| `Ctrl+W` `Ctrl+Q` | Close window / quit |

## Known limitations

- **SVG is not an export format.** An SVG can be written out as pixels at any size, but
  pixels cannot be turned back into shapes. To keep an SVG an SVG, copy the file.
- Text and drawings are placed against the picture's current size. Resizing carries them
  along; a crop or rotation applied afterwards will not move them for you.
- Exporting an animated GIF or WebP writes the frame you are looking at.
- "Move to Bin" needs a filesystem that has one. From `/tmp` or some removable media it
  reports that it is unsupported; **Delete Permanently** still works there.

Wanted, but not built yet: HDR and wide-gamut output, and progressive loading so a 100 MP
TIFF appears in stages.

## Licence

GPL-3.0-or-later — see [LICENSE](LICENSE). You may use, study, change and share this
program; a changed version you distribute has to stay open under the same terms.
