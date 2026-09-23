# Glance

A fast, native image viewer for Linux, written in Rust with GTK 4 and libadwaita.

It opens a file, shows it properly, and gets out of the way. Photographs, screenshots,
vector art and camera raw all open through the same window, and browsing a folder is a
matter of pressing an arrow key.

## Why

Most Linux image viewers fall into one of two camps: fast but looking like they escaped
from 2005, or modern-looking but slow and heavy for what is fundamentally a
pixel-blitting problem. Glance aims at both — a GNOME-native interface that starts
instantly and stays responsive on a 50MP photograph.

The implementation leans on two decisions:

**Rust, because an image viewer is a parser for untrusted binary data.** Image decoders
are a classic source of memory-corruption bugs — libjpeg, libpng and WebP have all had
serious ones. Rust removes that entire class of defect from the decoding path.

**GTK 4 and libadwaita, because the look should be the desktop's, not the app's.**
Rounded corners, header bars, adaptive layouts and automatic dark mode come from the
platform rather than being reimplemented.

Two things follow from that which are easy to get wrong and are handled here
deliberately: **decoding never runs on the main thread**, so a huge file cannot freeze
the window, and **SVGs are kept as vectors** rather than rasterised once at load, so
zooming into one stays sharp instead of turning into a grid of squares.

Most SVGs go further than that: where the drawing maps onto GTK's own render nodes it is
**translated into them once at load and then handed to the GPU**, so a zoom is a matter
of changing a transform. Nothing is re-rasterised and memory does not move. It matters
most for drawings with shadows or blurs, where rasterising means rebuilding the filter at
every zoom level: on one logo with three nested drop shadows, zooming to 23× cost 571 MB
and 11.6s of CPU through the rasteriser and 0 MB and 1.0s through the render nodes.
Anything the translation cannot express exactly — patterns, embedded rasters, filter
chains with no GTK equivalent — falls back to the rasteriser rather than being
approximated.

## Features

### Viewing

- Fit to window, actual size (100%), and free zoom up to 32×
- **Zoom anchors on the pointer** — the pixel under the cursor stays under the cursor,
  which makes wheel-zoom feel like moving a magnifier rather than working a slider
- Click and drag to pan when zoomed in
- Smooth, animated zoom and pan; scale interpolates geometrically, so 1×→2× feels like
  2×→4×
- Double-click toggles between fit and 100%
- Touchpad pinch-to-zoom
- Header shows the file name, format, pixel dimensions and live zoom percentage

### Browsing a folder

- Left/right arrows step through the images beside the open one, wrapping at both ends
- A **filmstrip** along the bottom showing neighbouring thumbnails, with the position
  (`3/47`) above it and arrows either side; click any thumbnail to jump to it
- The strip adapts to the window: fewer thumbnails as it narrows, down to one
- Files sort in **natural order**, so `IMG_2` comes before `IMG_10` rather than after it
- **The folder stays in sync.** Add or remove images from a file manager or the terminal
  and the count, strip and navigation update on their own — no reopening

### Transforms

- Rotate 90° either way, or any angle with a slider and a live degree readout
- Flip horizontally and vertically, applied in the image's own frame so a flip mirrors
  the picture regardless of how it is rotated
- All of it lives in a panel opened from the header, so it stays out of the way

### Editing

- **Crop** from a panel opened with the Edit button: drag out a rectangle, then move
  it or pull any of its eight handles, with the cursor changing to say which you are
  about to do
- Aspect presets for the usual social sizes — 1:1, 4:5, 5:4, 3:2, 2:3, 16:9, 9:16 — or
  free, or **freehand**, tracing any shape with the pointer
- `Enter` applies the crop **to the picture you are looking at**, so the next crop, the
  next rotation and the next flip all build on the result, the way an editor works
- **Brightness, contrast and saturation**, each on a slider centred on zero. Dragging
  one is instant whatever the image size, because the preview is a colour matrix the GPU
  applies as it draws rather than a pass over every pixel; the same numbers are baked
  into the pixels when you save. Contrast doubles at +100 and halves at -100, and
  saturation mixes towards the Rec. 709 luma, so desaturating a red does not leave it
  muddy
- **Apply** fixes the current slider values into the image so they become an undo step
  and further edits build on them; **Reset** returns all three to zero. Leaving them
  unapplied is fine — saving, copying and cropping all carry them along
- **Resize** from the header or `Ctrl+R`: drag any of the eight grips on the picture
  itself, or type the pixels into the panel — each follows the other, so you can pull it
  roughly into shape and then round the number off. **Keep aspect ratio** is on by
  default and applies to both; unticking it lets the picture stretch
- Nothing is resampled while you drag: the picture is simply drawn into a different
  rectangle, so a grip stays smooth on an image of any size, and the one resample that
  does happen — Lanczos, on export or apply — happens once
- **Export** writes a copy in another format at whatever size the resize tool is
  showing, leaving the original alone. PNG, JPEG, WebP, TIFF, BMP, GIF and ICO, with a
  line under the picker for what a format will cost you — JPEG has no transparency, GIF
  is 256 colours and one frame. A format with a limit of its own says so before the file
  dialog opens rather than after: ICO refuses anything past 256 × 256
- **Text** laid over the picture: type it, pick the family, the size in image pixels,
  bold, italic, underline, the colour of the letters and the colour of the plate behind
  them — set that one's opacity to zero for no plate. Drag it anywhere on the picture;
  add as many lines as you like and click one to select it
- Text stays text for as long as it is unsaved, so it can be moved, restyled and
  retyped; the preview on screen and the pixels that get written go through the same
  layout and the same render nodes, so what you position is what you get
- **Aim for a file size** when exporting — "make this 500 KB", the thing people
  otherwise upload their photographs to an advertising-funded website for. Tick the box,
  give a number in KB or MB, and the export works towards it: for JPEG it searches the
  quality dial for the best-looking file that fits, and scales the picture down only if
  quality alone cannot get there. Formats with no quality dial can only be met by
  scaling, and the panel says so
- The same box handles the opposite problem — a form demanding a *minimum* size. The
  file is padded up to it with a metadata block the format already has a place for (a
  JPEG comment, a PNG text chunk), so not a pixel changes and the file stays valid. The
  panel reports exactly how much filler went in
- Undo and redo the last several edits with `Ctrl+Z` and `Ctrl+Shift+Z`
- **Save** writes the edited image back over the original; **Save As…** writes a copy,
  offering Downloads by default. Nothing touches disk until you ask it to
- **Cancel**, or `Esc`, leaves the editor and asks first, so a session several edits
  deep is never thrown away by a stray key
- Editing is modal on purpose: the filmstrip, the arrow keys and Delete all switch off
  while the picture on screen is unsaved work, so there is no way to wander off it by
  accident

### Formats

| Family | Formats |
|---|---|
| Raster | PNG, JPEG, GIF, WebP, TIFF, BMP, ICO, QOI, TGA, PNM |
| Modern | HEIC / HEIF, AVIF |
| Vector | SVG, SVGZ |
| Camera raw | CR2, CR3, CRW, NEF, NRW, ARW, DNG, RAF, ORF, RW2, PEF, SRW, and 14 more |

- **Animated GIF and WebP play**, and keep playing while you zoom, rotate or flip
- **EXIF orientation is honoured**, so phone photos are upright instead of sideways
- **SVGs render as vectors at every zoom level**, drawn by the GPU as paths where that
  is possible and otherwise re-rasterised a visible region at a time — never by enlarging
  pixels
- Camera raw uses the full-size JPEG the camera embedded where there is one, and
  demosaics the sensor data when there is not

### Managing files

- Open via the header button, `Ctrl+O`, drag-and-drop, or a path on the command line
- **Copy to clipboard** from the header or with `Ctrl+C`, ready to paste into a chat,
  a document or an editor; the copy reflects the current view, so a rotated or cropped
  picture arrives rotated or cropped
- Delete with a confirmation offering **Move to Bin** (recoverable) or **Delete
  Permanently** (not), clearly distinguished; afterwards the view moves to the next image

### Layout

- A second bar under the header carries the two actions that change the file rather
  than the view — **Edit** in amber, **Delete** in red — each with its name beside its
  icon, which keeps them out of the row of view controls and makes the destructive one
  impossible to hit by mistake
- Both bars fold away in fullscreen

### Appearance

- Follow the system theme, or force light or dark; the choice is remembered between
  launches
- Fullscreen that hides every bar so the image has the whole screen, with the chrome
  sliding back when the pointer reaches an edge

## Keyboard shortcuts

| Key | Action |
|---|---|
| `Ctrl+O` | Open an image |
| `←` `→` / `Page Up` `Page Down` / `Space` `Backspace` | Previous / next image |
| `+` `-` | Zoom in / out |
| `0` | Fit to window |
| `1` | Actual size (100%) |
| `[` `]` | Rotate left / right |
| `Ctrl+Shift+R` | Reset rotation |
| `Ctrl+H` / `Ctrl+J` | Flip horizontally / vertically |
| `Ctrl+T` | Rotate and flip options |
| `Ctrl+E` | Edit panel |
| `Ctrl+R` | Resize |
| `Enter` | Apply the crop |
| `Ctrl+Z` / `Ctrl+Shift+Z` | Undo / redo an edit |
| `Ctrl+S` / `Ctrl+Shift+S` | Save in place / Save As… |
| `Ctrl+C` | Copy the image to the clipboard |
| `F11` / `Ctrl+F` | Fullscreen |
| `Delete` | Delete the current image |
| `Esc` | Leave fullscreen, close the options panel, or cancel editing |
| `Ctrl+W` / `Ctrl+Q` | Close window / quit |

## Building

### Requirements

System libraries, with the Arch package names:

```bash
sudo pacman -S gtk4 libadwaita libheif rust
```

`libheif` supplies HEIC and AVIF decoding. Everything else — PNG, JPEG, SVG, camera raw
— is pure Rust and comes in through Cargo.

### Build and run

```bash
cargo build --release
./target/release/simple-viewer path/to/image.jpg
```

For development there is a `quick` profile: optimised, but without the fat LTO that makes
a release link take minutes. Use it for anything involving camera raw or SVG, which are
unusably slow in a debug build.

```bash
cargo run --profile quick -- path/to/image.jpg
```

## Testing

`testdata/` holds fixtures chosen to catch specific mistakes — an EXIF-rotated JPEG, a
transparent SVG that exposes premultiplied-alpha errors, a deliberately corrupt file, and
four CC0 camera raws covering different decode paths. See
[`testdata/README.md`](testdata/README.md) for what each one is for.

```bash
cargo test
```

## Not yet implemented

Wanted, but not built:

- A quality slider as such; quality is chosen for you when you aim at a file size, and
  is the encoder's default otherwise
- Freehand drawing and shapes
- ICC colour management — without it, photos from wide-gamut cameras look slightly off
- HDR and wide-gamut display output
- Progressive loading for very large images, so a 100MP TIFF appears in stages

## Known limitations

- **SVG is not an export format.** A drawing can be written as pixels — open an SVG and
  export it as a PNG or a JPEG at any size — but pixels cannot be written back as
  shapes, so there is no photograph-to-SVG direction to offer. Exporting an SVG
  unchanged is what Save As is for.
- Text is positioned against the picture's current size. Resizing afterwards carries it
  along, but a crop or a rotation applied after the fact will not move it for you.
- Exporting an animated GIF or WebP writes the frame you are looking at. A still picture
  exported *as* a GIF is fine — it is a single-frame GIF — but nothing here creates
  animation.

- An SVG that falls back to the rasteriser — one using patterns, embedded images, or a
  filter chain beyond a blur or a drop shadow — still takes a moment to reach full
  sharpness at deep zoom, and a heavily filtered one is expensive while it does. A quick
  approximation appears immediately and is replaced once the full render finishes.
- A drawing with more than about fifty thousand shapes is rasterised rather than
  translated, because redrawing that many paths every frame costs more than reusing a
  rendered tile.
- "Move to Bin" needs a filesystem that has one. Deleting from `/tmp` or some removable
  media reports that it is unsupported rather than binning the file; **Delete
  Permanently** still works there.
- The binary is named `simple-viewer` while the project is Glance. Cosmetic, but they
  should be reconciled.
