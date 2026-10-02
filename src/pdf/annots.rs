// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Everything Glance writes onto a page — highlights and other text marks,
//! notes, speech bubbles, text boxes and drawings — as one kind of thing, so undo, redo and saving
//! treat them all alike.
//!
//! Each is an ordinary PDF annotation, written into the file the moment it is
//! made, as Preview does. There is no separate save to forget, and the file on
//! disk is always what is on screen, so a page drawn later on another thread
//! simply reopens it.
//!
//! Positions are points from the page's top-left corner, as Poppler shows the
//! page. The PDF measures from the bottom, so they are flipped on the way in
//! and out, and nowhere else.

use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::path::Path;

use gtk::{cairo, glib};

use super::document;
use super::markup::Mark;
use super::ink::Drawing;
use super::notes::{Note, TextBox};

/// A colour as Poppler takes it, 16 bits a channel. Only ever 8 bits' worth
/// of it is used: the PDF stores colours as decimals, which come back a hair
/// different, and snapping both ways makes a colour read back from the file
/// equal to the one it was written with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb(pub u16, pub u16, pub u16);

fn snap(channel: u16) -> u16 {
    (f64::from(channel) / 257.0).round() as u16 * 257
}

impl Rgb {
    pub fn from_rgba(rgba: &gtk::gdk::RGBA) -> Self {
        let channel = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u16 * 257;
        Rgb(channel(rgba.red()), channel(rgba.green()), channel(rgba.blue()))
    }

    fn from_poppler(colour: &poppler::Color) -> Self {
        Rgb(snap(colour.red()), snap(colour.green()), snap(colour.blue()))
    }

    pub(super) fn from_ffi(colour: &poppler::ffi::PopplerColor) -> Self {
        Rgb(snap(colour.red), snap(colour.green), snap(colour.blue))
    }

    pub(super) fn ffi(self) -> poppler::ffi::PopplerColor {
        poppler::ffi::PopplerColor { red: self.0, green: self.1, blue: self.2 }
    }

    pub fn to_rgba(self) -> gtk::gdk::RGBA {
        let channel = |c: u16| f32::from(c) / 65535.0;
        gtk::gdk::RGBA::new(channel(self.0), channel(self.1), channel(self.2), 1.0)
    }

    /// `#rrggbb`, for remembering it between runs.
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0 >> 8, self.1 >> 8, self.2 >> 8)
    }

    pub fn from_hex(text: &str) -> Option<Self> {
        let text = text.trim().strip_prefix('#')?;
        if text.len() != 6 {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(text.get(i..i + 2)?, 16).ok().map(|b| u16::from(b) * 257);
        Some(Rgb(byte(0)?, byte(2)?, byte(4)?))
    }

    fn poppler(self) -> poppler::Color {
        let mut colour = poppler::Color::new();
        colour.set_red(self.0);
        colour.set_green(self.1);
        colour.set_blue(self.2);
        colour
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Annotation {
    Mark(Mark),
    Note(Note),
    TextBox(TextBox),
    Ink(Drawing),
}

impl Annotation {
    pub fn page(&self) -> usize {
        match self {
            Annotation::Mark(mark) => mark.page,
            Annotation::Note(note) => note.page,
            Annotation::TextBox(text_box) => text_box.page,
            Annotation::Ink(drawing) => drawing.page,
        }
    }

    /// Put it on its page, in the document in memory. Nothing is written
    /// until `save`.
    pub fn add(&self, document: &poppler::Document) {
        let Some(page) = page(document, self.page()) else { return };
        match self {
            Annotation::Mark(mark) => super::markup::add(document, &page, mark),
            Annotation::Note(note) => super::notes::add_note(document, &page, note),
            Annotation::TextBox(text_box) => super::notes::add_box(document, &page, text_box),
            Annotation::Ink(drawing) => super::ink::add(document, &page, drawing),
        }
    }

    /// Take it off its page again. False if it is not there.
    pub fn remove(&self, document: &poppler::Document) -> bool {
        let Some(page) = page(document, self.page()) else { return false };
        // Another program's drawing has a box of its own making, so it is
        // looked for by its strokes when it is not where Glance would put it.
        if let Annotation::Ink(drawing) = self {
            let (kind, rect) = super::ink::key(&page, drawing);
            let Some(annot) = find(&page, kind, rect).or_else(|| super::ink::find(&page, drawing)) else { return false };
            page.remove_annot(&annot);
            return true;
        }
        let parts = match self {
            Annotation::Mark(mark) => vec![super::markup::key(&page, mark)],
            Annotation::Note(note) => vec![super::notes::note_key(&page, note)],
            Annotation::TextBox(text_box) => super::notes::box_keys(&page, text_box),
            Annotation::Ink(drawing) => vec![super::ink::key(&page, drawing)],
        };
        let mut removed = false;
        for (kind, rect) in parts {
            if let Some(annot) = find(&page, kind, rect) {
                page.remove_annot(&annot);
                removed = true;
            }
        }
        removed
    }
}

pub(super) fn page(document: &poppler::Document, index: usize) -> Option<poppler::Page> {
    i32::try_from(index).ok().and_then(|i| document.page(i))
}

/// Flip a box from top-left coordinates, x1, y1, x2, y2, to the PDF's own,
/// measured up from the bottom of the page.
pub(super) fn flip(area: [f64; 4], page_height: f64) -> [f64; 4] {
    let [x1, y1, x2, y2] = area;
    [x1.min(x2), page_height - y1.max(y2), x1.max(x2), page_height - y1.min(y2)]
}

pub(super) fn rectangle(area: [f64; 4]) -> poppler::ffi::PopplerRectangle {
    let [x1, y1, x2, y2] = area;
    poppler::ffi::PopplerRectangle { x1, y1, x2, y2 }
}

/// Finish off a new annotation and put it on the page: its colour, if it has
/// one, and printed with the page, as ink on paper would be.
pub(super) fn attach(page: &poppler::Page, annot: &poppler::Annot, colour: Option<Rgb>, print: bool) {
    use poppler::prelude::*;
    if let Some(colour) = colour {
        annot.set_color(Some(&colour.poppler()));
    }
    if print {
        annot.set_flags(poppler::AnnotFlag::PRINT);
    }
    page.add_annot(annot);
}

/// One annotation on a page, as Poppler reports it back.
pub(super) struct Found {
    pub kind: poppler::ffi::PopplerAnnotType,
    /// Top-left coordinates, x1, y1, x2, y2.
    pub area: [f64; 4],
    pub text: String,
    pub colour: Option<Rgb>,
    pub annot: poppler::Annot,
}

/// Every annotation on a page.
pub(super) fn list(page: &poppler::Page) -> Vec<Found> {
    use glib::translate::{from_glib_none, ToGlibPtr};
    use poppler::prelude::*;

    let (_, height) = page.size();
    let mut found = Vec::new();
    // SAFETY: the list and every mapping in it belong to Poppler until freed
    // below; each annotation kept is taken with a reference of its own.
    unsafe {
        let list = poppler::ffi::poppler_page_get_annot_mapping(page.to_glib_none().0);
        let mut node = list;
        while !node.is_null() {
            let mapping = (*node).data.cast::<poppler::ffi::PopplerAnnotMapping>();
            if !mapping.is_null() && !(*mapping).annot.is_null() {
                let a = (*mapping).area;
                let annot: poppler::Annot = from_glib_none((*mapping).annot);
                found.push(Found {
                    kind: poppler::ffi::poppler_annot_get_annot_type((*mapping).annot),
                    area: flip([a.x1, a.y1, a.x2, a.y2], height),
                    text: annot.contents().map(|t| t.to_string()).unwrap_or_default(),
                    colour: annot.color().map(|c| Rgb::from_poppler(&c)),
                    annot,
                });
            }
            node = (*node).next;
        }
        poppler::ffi::poppler_page_free_annot_mapping(list);
    }
    found
}

/// The page's annotation of a kind, at a place, if there is one. `rect` is in
/// the PDF's own coordinates, as the annotation was made with.
pub(super) fn find(
    page: &poppler::Page,
    kind: poppler::ffi::PopplerAnnotType,
    rect: [f64; 4],
) -> Option<poppler::Annot> {
    // Poppler stores coordinates as written in the file, so allow for them
    // coming back a hair different from what was given.
    const CLOSE: f64 = 0.05;
    let (_, height) = page.size();
    let want = flip(rect, height);
    list(page)
        .into_iter()
        .find(|found| found.kind == kind && found.area.iter().zip(want).all(|(a, b)| (a - b).abs() < CLOSE))
        .map(|found| found.annot)
}

/// Like `find`, but for the annotation's colour.
pub(super) fn colour_of(page: &poppler::Page, kind: poppler::ffi::PopplerAnnotType, rect: [f64; 4]) -> Option<Option<Rgb>> {
    use poppler::prelude::*;
    find(page, kind, rect).map(|annot| annot.color().map(|c| Rgb::from_poppler(&c)))
}

/// Have Poppler work out how new annotations look, so the file carries their
/// appearance with it. Poppler only does that when it draws them, and a reader
/// that finds none has to guess, each in its own way.
pub(super) fn settle(document: &poppler::Document, pages: &[usize]) {
    let Ok(surface) = cairo::ImageSurface::create(cairo::Format::ARgb32, 1, 1) else { return };
    let Ok(cr) = cairo::Context::new(&surface) else { return };
    for &index in pages {
        if let Some(page) = page(document, index) {
            let (w, h) = page.size();
            cr.save().ok();
            cr.scale(1.0 / w.max(1.0), 1.0 / h.max(1.0));
            page.render(&cr);
            cr.restore().ok();
        }
    }
}

/// Write the document, annotations and all, over the file it came from.
///
/// The file is replaced whole: written beside the original, then renamed over
/// it. Poppler reads the document lazily from the original while it writes, so
/// writing into that same file would pull it out from under itself, and a
/// crash halfway through would leave half a PDF.
pub fn save(document: &poppler::Document, path: &Path) -> Result<(), String> {
    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    // The file itself, not a link to it: replacing a link would leave the
    // real file as it was and turn the link into a copy.
    let target = fs::canonicalize(path).map_err(|e| format!("Could not save “{name}”: {e}"))?;
    // Replacing a file only needs its folder to be writable. A file marked
    // read-only is left alone rather than slipped round.
    if let Err(error) = OpenOptions::new().write(true).open(&target) {
        eprintln!("glance: {}: {error}", target.display());
        return Err(match error.kind() {
            ErrorKind::PermissionDenied | ErrorKind::ReadOnlyFilesystem => {
                format!("“{name}” is read-only, so it cannot be marked up.")
            }
            _ => format!("Could not save “{name}”: {error}"),
        });
    }
    let temp = target.with_file_name(format!(".{name}.glance-{}", std::process::id()));
    if let Err(error) = replace(document, &temp, &target) {
        let _ = fs::remove_file(&temp);
        eprintln!("glance: {}: {error}", target.display());
        return Err(format!("Could not save “{name}”: {error}"));
    }
    Ok(())
}

/// Write to `temp`, then put it in `target`'s place.
fn replace(document: &poppler::Document, temp: &Path, target: &Path) -> Result<(), String> {
    let io = |e: std::io::Error| e.to_string();
    document.save(&document::uri(temp)).map_err(|e| e.message().to_string())?;
    let permissions = fs::metadata(target).map_err(io)?.permissions();
    fs::set_permissions(temp, permissions).map_err(io)?;
    // On the disk before it takes the original's place, so a crash leaves
    // one whole file or the other.
    File::open(temp).and_then(|file| file.sync_all()).map_err(io)?;
    fs::rename(temp, target).map_err(io)
}

#[cfg(test)]
mod tests {
    use super::super::notes::{Note, TextBox, TextStyle};
    use super::*;

    #[test]
    fn a_colour_survives_being_remembered() {
        let yellow = Rgb(0xffff, 0xe4e4, 0x0000);
        assert_eq!(yellow.hex(), "#ffe400");
        assert_eq!(Rgb::from_hex("#ffe400"), Some(yellow));
        assert_eq!(Rgb::from_hex("ffe400"), None);
        assert_eq!(Rgb::from_hex("#ffe4"), None);
        assert_eq!(Rgb::from_hex("#gge400"), None);
    }

    /// The smallest PDF with some text on it: one A4 page.
    fn tiny_pdf(path: &Path) {
        let content = b"BT /F1 24 Tf 72 700 Td (Hello annotations) Tj ET";
        let objects: [Vec<u8>; 5] = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R \
              /Resources << /Font << /F1 5 0 R >> >> >>"
                .to_vec(),
            [format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(), content.to_vec(), b"\nendstream".to_vec()]
                .concat(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n", i + 1).bytes());
            out.extend(object);
            out.extend(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for offset in offsets {
            out.extend(format!("{offset:010} 00000 n \n").bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
        fs::write(path, out).unwrap();
    }

    #[test]
    fn annotations_are_saved_and_found_again() {
        use super::super::markup::{Line, Style};
        use super::super::notes;

        let dir = std::env::temp_dir().join(format!("glance-annots-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.pdf");
        tiny_pdf(&path);
        let open = || poppler::Document::from_file(&document::uri(&path), None).unwrap();

        let a4 = (595.0, 842.0);
        let mark = Annotation::Mark(Mark {
            page: 0,
            style: Style::Highlight,
            lines: vec![Line { area: [72.0, 120.0, 200.0, 146.0], quarter: 0 }],
            colour: Rgb(0x7f7f, 0xe3e3, 0x5a5a),
        });
        let note = Annotation::Note(Note::new(0, (400.0, 300.0), a4, "A note".into()));
        let bubble = Annotation::TextBox(TextBox::bubble(0, (150.0, 500.0), a4, "Look here, please".into()));
        let words = Annotation::TextBox(TextBox::new(
            0,
            (300.0, 600.0),
            (180.0, 30.0),
            a4,
            "In a font of my own".into(),
            TextStyle {
                family: Some("DejaVu Sans".into()),
                size: 18.0,
                bold: true,
                italic: false,
                colour: Rgb(0xc0c0, 0x1c1c, 0x2828),
                fill: None,
                border: false,
            },
        ));
        let drawing = Annotation::Ink(super::super::ink::Drawing {
            page: 0,
            tool: crate::images::edit::draw::Tool::Arrow,
            strokes: vec![vec![(100.0, 700.0), (200.0, 720.0)], vec![(190.0, 712.0), (200.0, 720.0), (188.0, 724.0)]],
            colour: Rgb(0x1c1c, 0x7171, 0xd8d8),
            width: 3.0,
        });

        let doc = open();
        let all = [&mark, &note, &bubble, &words, &drawing];
        for a in all {
            a.add(&doc);
        }
        settle(&doc, &[0]);
        save(&doc, &path).unwrap();
        drop(doc);

        // Read back by another document, as the next run would.
        let doc = open();
        let pinned = notes::on_page(&doc, 0);
        assert_eq!(pinned, vec![note.clone(), bubble.clone(), words.clone()]);
        let Annotation::Mark(m) = &mark else { unreachable!() };
        assert_eq!(super::super::markup::existing_colour(&doc, m), Some(m.colour));
        // The text box carries its own appearance, for other readers.
        let bytes = fs::read(&path).unwrap();
        assert!(bytes.windows(4).any(|w| w == b"/AP "), "no appearance stream written");

        // And each can be taken off again, tail and all.
        for a in all {
            assert!(a.remove(&doc), "{a:?} not found to remove");
        }
        settle(&doc, &[0]);
        save(&doc, &path).unwrap();
        let doc = open();
        let page = page(&doc, 0).unwrap();
        assert!(list(&page).is_empty(), "annotations left behind");

        // Undone then redone, in the same document, as the reader does.
        let doc = open();
        for a in all {
            a.add(&doc);
        }
        save(&doc, &path).unwrap();
        for a in all {
            assert!(a.remove(&doc), "{a:?} not there to take back");
        }
        save(&doc, &path).unwrap();
        for a in all {
            a.add(&doc);
        }
        settle(&doc, &[0]);
        save(&doc, &path).unwrap();
        let again = list(&super::page(&open(), 0).unwrap()).len();
        // Five, the bubble's tail making a sixth.
        assert_eq!(again, 6, "not everything came back after undo and redo");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn colours_come_back_as_they_went_in() {
        assert_eq!(snap(54483), 54484);
        assert_eq!(snap(0xffff), 0xffff);
        assert_eq!(Rgb::from_rgba(&Rgb(0xd4d4, 0x2a2a, 0).to_rgba()), Rgb(0xd4d4, 0x2a2a, 0));
    }

    #[test]
    fn flipping_measures_from_the_bottom() {
        assert_eq!(flip([10.0, 20.0, 50.0, 30.0], 100.0), [10.0, 70.0, 50.0, 80.0]);
        // Corners either way round give the same box.
        assert_eq!(flip([50.0, 30.0, 10.0, 20.0], 100.0), [10.0, 70.0, 50.0, 80.0]);
    }
}
