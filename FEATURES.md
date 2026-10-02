# Glance vs. Apple Preview — feature tracker

Apple's Preview opens images, PDFs and 3D models, all in one app. This file lists everything Preview does, grouped by what kind of file it
applies to, and marks where Glance stands on each one.

Tick a box when a feature ships.

- `[x]` — Glance has it
- `[ ]` — Glance does not have it yet
- *partial:* — Glance does some of it; the note says what's missing

| Area | Glance has | Partial |
|---|---|---|
| Images | 39 / 64 | 7 |
| PDF | 17 / 28 | 2 |
| 3D | 0 / 22 | 0 |
| Capture and general | 3 / 8 | 1 |
| **Total** | **59 / 122** | **10** |

---

## 1. Images — viewing

- [x] Open an image and step through others (Glance browses the whole folder)
- [x] Zoom in, zoom out, actual size, fit to window
- [x] Fullscreen
- [x] Rotate the view
- [x] Play animated GIFs
- [x] Copy the image to the clipboard
- [x] Step through an animated GIF frame by frame in a sidebar — every frame with its delay; play, pause, step
- [ ] Thumbnail sidebar — *partial:* Glance has a filmstrip along the bottom
- [ ] Contact sheet (grid of all open images)
- [x] Slideshow — full screen, every 2 to 30 seconds, pause with Space
- [x] Inspector with file details
- [x] EXIF / camera metadata (camera, lens, shutter, ISO, date) — JPEG, PNG, WebP, TIFF, HEIF and camera raw
- [ ] Show where a photo was taken on a map (GPS) — *partial:* the coordinates and altitude are in the inspector, but there is no map
- [x] Live Text — select and copy text inside a photo (OCR) — English and about 45 other Latin-alphabet languages; Hindi to come
- [ ] Loupe — magnify one region under the pointer

## 2. Images — editing

- [x] Crop
- [x] Resize to exact pixels, with aspect-ratio lock
- [x] Resize by percentage, inches/cm, or change resolution (DPI) — with or without resampling; the resolution is kept in JPEG, PNG and BMP
- [x] Rotate 90° left and right
- [x] Flip horizontally and vertically
- [x] Contrast and saturation
- [x] Exposure
- [x] Highlights and shadows
- [x] Temperature and tint (white balance)
- [x] Sepia
- [x] Sharpness — and softness, below zero
- [x] Levels with a histogram, and Auto Levels — per channel or all together
- [ ] Remove background / lift the subject out of a photo
- [x] Convert to another file type
- [x] Quality slider when exporting JPEG
- [x] Undo and redo
- [ ] Revert to the last saved version, or browse older versions — *partial:* Cancel discards unsaved edits

## 3. Markup — shared by images and PDFs

On a PDF the same pens, shapes and text are saved into the file as standard
annotations, so every PDF reader shows them.

- [x] Freehand pen (Preview: Sketch / Draw)
- [x] Highlighter
- [x] Line, arrow, rectangle, ellipse
- [ ] More shapes: star, polygon, rounded rectangle, speech bubble — *partial:* speech bubbles, on PDFs
- [x] Stroke thickness and colour
- [ ] Fill colour for shapes — *partial:* shapes are outlines only
- [x] Text boxes with font, size and colour
- [x] Text background colour, bold, italic, underline — underline on images only; a PDF text box has none
- [ ] Select, move and resize a shape after drawing it — *partial:* text can be dragged, and on a PDF so can notes and speech bubbles; drawn marks can only be undone, and nothing resizes yet
- [ ] Loupe annotation (a magnified circle placed on the image)
- [ ] Sticky notes — *partial:* on PDFs, not yet on images
- [ ] Signatures
- [x] Redact / black out a region — marked first, then applied to a copy or the original

## 4. Colour

- [x] Honour the ICC profile embedded in an image
- [ ] Assign or convert to a different colour profile
- [ ] Soft proofing — preview how an image will look on another screen or printer

## 5. File formats

### Open

- [x] JPEG, PNG, GIF, TIFF, BMP, ICO, HEIC/HEIF
- [x] Camera raw (DNG, CR2, NEF, ARW and more)
- [ ] JPEG 2000
- [ ] PSD (Photoshop)
- [ ] OpenEXR and Radiance HDR
- [ ] ICNS (macOS icons)
- [ ] AI (Illustrator) — these are PDFs inside, so they come free with PDF support
- [x] PDF — see section 6

### Export

- [x] PNG, JPEG, TIFF, BMP, GIF, ICO
- [ ] HEIC
- [ ] JPEG 2000
- [ ] OpenEXR
- [ ] PSD
- [ ] TGA
- [ ] ICNS
- [x] PDF (save an image as a PDF) — with Combine into PDF, on A4, Letter or the picture's own size

---

## 6. PDF — viewing

- [x] Open and render PDFs
- [x] Page thumbnails in a sidebar
- [ ] Contact sheet (grid of every page)
- [x] Table of contents / outline — in the sidebar, following the section being read
- [x] Continuous scroll, single page, and two-page layouts
- [x] Bookmarks — kept beside Glance's settings, so the file is not changed
- [x] Search the text
- [ ] Present as a slideshow
- [x] Document info (title, author, page count, page size)

## 7. PDF — text

- [x] Select and copy text
- [x] Highlight, underline and strike through text, highlights in any colour
- [x] Notes and speech bubbles

## 8. PDF — forms and signatures

- [ ] Fill in interactive form fields
- [x] Add text boxes to forms that aren't interactive — the PDF text tool
- [ ] Draw a signature with the mouse or touchpad and place it on a page
- [ ] Capture a signature from paper with the webcam
- [ ] Save signatures for reuse

## 9. PDF — pages

- [x] Combine several PDFs into one — and pictures, in any order, with only the pages wanted
- [x] Add, delete and reorder pages — in Combine into PDF, saved as a new file
- [ ] Insert a blank page, or pages from another file — *partial:* pages from other files and pictures, but no blank page yet
- [x] Rotate pages — in Combine into PDF, page by page, saved as a new file; the reader's turning is for the view only
- [ ] Crop pages
- [ ] Apply effects to a whole document (black and white, sepia, lighter/darker)

## 10. PDF — security and size

- [x] Password to open — opening protected PDFs, and protecting them with AES-256
- [ ] Permissions (block printing, copying or annotating) — *partial:* respected when reading; setting them is hidden, because they are only a request that other viewers may ignore
- [x] Redact text permanently — redacted pages are rebuilt, the rest of their words kept searchable
- [x] Reduce file size — lossless repacking, or photos scaled to 150 or 96 dpi
- [ ] Lock a file against accidental edits

---

## 11. 3D — formats

Preview has viewed USD files since macOS 12. The wider format list below, and
the editing and export tools in section 13, are what the current guide (macOS 26
onward) covers.

- [ ] USD, USDA, USDC, USDZ
- [ ] glTF and GLB
- [ ] OBJ
- [ ] STL
- [ ] PLY, including Gaussian splats
- [ ] Alembic (.abc)
- [ ] MaterialX (.mtlx)
- [ ] OpenVDB (.vdb volumes)

## 12. 3D — viewing

- [ ] Orbit, pan and zoom around a model
- [ ] View through any camera defined in the file
- [ ] Lighting presets (Studio, Overcast, Sunny, Dusk, Night, City Night, Office)
- [ ] Exposure, gamma and shadow distance controls
- [ ] Choice of renderer: fast real-time, or slower raytraced
- [ ] Object hierarchy panel
- [ ] Inspector: objects, materials, lights, file size and dates
- [ ] Play animations, with a timeline to scrub through
- [ ] Switch measurement units

## 13. 3D — editing and export

- [ ] Edit a scene and save it
- [ ] Export to USD / USDZ, with texture and geometry compression
- [ ] Export a rendered still image, or an image sequence
- [ ] Export an animation as a video
- [ ] Export as PDF

---

## 14. Capture and import

- [ ] Import photos from a camera or phone
- [ ] Import from a scanner (Linux: SANE)
- [ ] Take a screenshot (Linux: the desktop screenshot portal)

## 15. General

- [x] Keyboard shortcuts, all listed in one searchable window (Ctrl+?)
- [x] Save in place and export a copy
- [x] Print — pictures and PDFs
- [ ] Share to other apps
- [ ] Settings window — *partial:* appearance (light / dark / system) is in the menu

---

## Apple-only — no Linux equivalent

These depend on Apple's ecosystem. Not worth chasing as-is.

- Signing on an iPhone or iPad and sending it to the Mac (Continuity)
- Syncing signatures through iCloud
- AutoFill forms from the Contacts app *(GNOME Contacts could stand in)*
- Viewing on Apple Vision Pro, and exporting spatial / immersive photos
- Scanning documents with the iPhone camera
- Sharing through AirDrop

## Already beyond Preview

Things Glance does that Preview doesn't:

- **Compress or inflate an image to an exact file size.** Preview can make a file smaller — a JPEG quality slider, Adjust Size, a "Reduce File Size" filter for PDFs — but you can't ask it for 500 KB and get 500 KB.
- **Browse the whole folder.** Open one image and the arrow keys walk through everything beside it. Preview only steps through files you opened together.
- **Rotate to any angle you type.** Preview rotates images in 90° steps.
- **Night mode for PDFs.** Black paper and white text, with colours keeping their hue, so red ink stays red instead of turning cyan.

---

## Where to start

Roughly in order of value for effort:

1. **PDF forms and signatures** (section 8) — filling in form fields, and a signature
   drawn once and placed anywhere, building on the pen the Draw panel already has.
2. **The rest of PDF pages** (section 9) — a blank page, cropping, and effects over a
   whole document, all at home in Combine into PDF.
3. **PDF contact sheet and slideshow** (section 6) — the last of PDF viewing.
4. **3D** (sections 11–13) — the largest project. glTF, OBJ, STL and PLY have good
   pure-Rust loaders; USD does not, and would need bindings to Pixar's C++ library.

---

*Compiled from Apple's [Preview User Guide for Mac](https://support.apple.com/guide/preview/welcome/mac),
including its pages on [3D formats](https://support.apple.com/guide/preview/prvwenxw4tm2/mac),
[viewing 3D files](https://support.apple.com/guide/preview/prvw11474/mac),
[exporting 3D files](https://support.apple.com/guide/preview/prvw1gxwpz8/mac),
[forms and signatures](https://support.apple.com/guide/preview/prvw35725/mac) and
[password protection](https://support.apple.com/guide/preview/prvw587dd90f/mac), checked
September 2026.*
