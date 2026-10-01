// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Notes, speech bubbles and text boxes.
//!
//! A note is the PDF's own sticky note: an icon on the page, its text shown
//! when it is opened. A text box is words written on the page itself, in a
//! font, size and colour of your choosing. A speech bubble is a text box with
//! a plain look, an outline and a tail pointing at what it is about. PDF has
//! no single annotation for that, so it is two: a text box and a line, which
//! every reader draws just as they look here.
//!
//! Positions are points from the page's top-left corner, as in `annots`.

use gtk::glib;

use super::annots::{self, Annotation, Rgb};
use super::newer;

/// A note's icon, in points.
pub const NOTE_SIZE: f64 = 20.0;
/// Sticky-note yellow.
pub const NOTE_COLOUR: Rgb = Rgb(0xffff, 0xd4d4, 0x2a2a);
/// A bubble is white, with dark text, outline and tail.
pub const BUBBLE_FILL: Rgb = Rgb(0xffff, 0xffff, 0xffff);
const TAIL_COLOUR: Rgb = Rgb(0x3333, 0x3333, 0x3333);
/// How far a new bubble sits from the spot it points at.
const REACH: f64 = 24.0;
/// Kept clear at the page's edges.
const EDGE: f64 = 4.0;
/// Poppler writes a bubble's text in 10-point Helvetica. These are generous,
/// so the box is never too small for what it holds.
const CHAR_WIDTH: f64 = 5.2;
const LINE_HEIGHT: f64 = 11.5;
const PADDING: f64 = 5.0;
const NARROWEST: f64 = 60.0;
const WIDEST: f64 = 216.0;
/// The tail's box is this much larger than the line, all round.
const TAIL_PAD: f64 = 2.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub page: usize,
    /// The icon's box, x1, y1, x2, y2.
    pub area: [f64; 4],
    pub text: String,
    pub colour: Rgb,
}

impl Note {
    /// A new note with its icon's corner at a spot, kept on the page.
    pub fn new(page: usize, spot: (f64, f64), page_size: (f64, f64), text: String) -> Self {
        let (x, y) = keep_on_page(spot, (NOTE_SIZE, NOTE_SIZE), page_size);
        Note { page, area: [x, y, x + NOTE_SIZE, y + NOTE_SIZE], text, colour: NOTE_COLOUR }
    }

    pub fn moved(&self, by: (f64, f64), page_size: (f64, f64)) -> Self {
        let [x1, y1, x2, y2] = self.area;
        let (x, y) = keep_on_page((x1 + by.0, y1 + by.1), (x2 - x1, y2 - y1), page_size);
        Note { area: [x, y, x + (x2 - x1), y + (y2 - y1)], ..self.clone() }
    }
}

/// How a text box's words look.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    /// `None` for Poppler's own small Helvetica, which needs nothing put in
    /// the file. A family named here is embedded in it, so it looks the same
    /// in every reader.
    pub family: Option<String>,
    /// In points.
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub colour: Rgb,
    /// The box's background; `None` lets the page show through.
    pub fill: Option<Rgb>,
    pub border: bool,
}

impl TextStyle {
    /// A speech bubble: plain dark text, white, outlined.
    pub fn bubble() -> Self {
        TextStyle {
            family: None,
            size: 10.0,
            bold: false,
            italic: false,
            colour: Rgb(0, 0, 0),
            fill: Some(BUBBLE_FILL),
            border: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextBox {
    pub page: usize,
    /// The box, x1, y1, x2, y2.
    pub area: [f64; 4],
    /// Where a speech bubble's tail points. `None` for a plain text box.
    pub tip: Option<(f64, f64)>,
    pub text: String,
    pub style: TextStyle,
}

impl TextBox {
    /// A new speech bubble pointing at `tip`: above and to the right of it,
    /// or wherever else there is room on the page.
    pub fn bubble(page: usize, tip: (f64, f64), page_size: (f64, f64), text: String) -> Self {
        let style = TextStyle::bubble();
        let (w, h) = box_size(&text, &style, None);
        let pw = page_size.0;
        let x = if tip.0 + REACH + w <= pw - EDGE { tip.0 + REACH } else { tip.0 - REACH - w };
        let y = if tip.1 - REACH - h >= EDGE { tip.1 - REACH - h } else { tip.1 + REACH };
        let (x, y) = keep_on_page((x, y), (w, h), page_size);
        TextBox { page, area: [x, y, x + w, y + h], tip: Some(tip), text, style }
    }

    /// A new text box with its top-left corner at `corner`, `size` big.
    pub fn new(page: usize, corner: (f64, f64), size: (f64, f64), page_size: (f64, f64), text: String, style: TextStyle) -> Self {
        let (x, y) = keep_on_page(corner, size, page_size);
        TextBox { page, area: [x, y, x + size.0, y + size.1], tip: None, text, style }
    }

    /// The same box saying something else, or in another style: resized to
    /// `size`, from the same top-left corner.
    pub fn changed(&self, text: String, style: TextStyle, size: (f64, f64), page_size: (f64, f64)) -> Self {
        let (x, y) = keep_on_page((self.area[0], self.area[1]), size, page_size);
        TextBox { area: [x, y, x + size.0, y + size.1], text, style, ..self.clone() }
    }

    /// The box moved; a bubble's tail still points where it did.
    pub fn moved(&self, by: (f64, f64), page_size: (f64, f64)) -> Self {
        let [x1, y1, x2, y2] = self.area;
        let (x, y) = keep_on_page((x1 + by.0, y1 + by.1), (x2 - x1, y2 - y1), page_size);
        TextBox { area: [x, y, x + (x2 - x1), y + (y2 - y1)], ..self.clone() }
    }

    /// The tail, from the box's edge to the tip. None if there is no tip, or
    /// it is under the box.
    pub fn tail(&self) -> Option<((f64, f64), (f64, f64))> {
        let tip = self.tip?;
        Some((attach(self.area, tip)?, tip))
    }
}

/// Where a tail leaves the box: the nearest point of the edge facing the tip,
/// kept away from the corners so it reads as a speech bubble's tail. None if
/// the tip is inside the box.
fn attach(area: [f64; 4], tip: (f64, f64)) -> Option<(f64, f64)> {
    let [x1, y1, x2, y2] = area;
    if (x1..=x2).contains(&tip.0) && (y1..=y2).contains(&tip.1) {
        return None;
    }
    let inset_x = ((x2 - x1) / 4.0).min(16.0);
    let inset_y = ((y2 - y1) / 4.0).min(8.0);
    let (mut x, mut y) = (tip.0.clamp(x1, x2), tip.1.clamp(y1, y2));
    if tip.1 < y1 || tip.1 > y2 {
        x = x.clamp(x1 + inset_x, x2 - inset_x);
    } else {
        y = y.clamp(y1 + inset_y, y2 - inset_y);
    }
    Some((x, y))
}

/// The box a text needs.
///
/// In a chosen font, `measured` is the text's width in points and how many
/// lines it takes, as Pango sets it in that same font; Poppler gives each line
/// exactly the font's size, and the last line's descenders a third more.
/// Without a measurement — Poppler's own font, for a bubble — the size is
/// estimated: wide enough for the longest line, up to a limit, and tall
/// enough for every line once wrapped at that width.
pub fn box_size(text: &str, style: &TextStyle, measured: Option<(f64, usize)>) -> (f64, f64) {
    if let (Some(_), Some((width, lines))) = (&style.family, measured) {
        // A little to spare, or Poppler wraps a line that only just fits.
        let width = (width * 1.04 + 4.0).max(style.size);
        let height = lines.max(1) as f64 * style.size + style.size * 0.35;
        return (width.ceil(), height.ceil());
    }
    let scale = style.size / 10.0;
    let (char_width, line_height, padding) = (CHAR_WIDTH * scale, LINE_HEIGHT * scale, PADDING * scale);
    let longest = text.lines().map(|line| line.chars().count()).max().unwrap_or(0);
    let width = (longest as f64 * char_width + 2.0 * padding).clamp(NARROWEST * scale, WIDEST * scale);
    let per_line = (((width - 2.0 * padding) / char_width).floor() as usize).max(1);
    let lines: usize = text.split('\n').map(|paragraph| wrapped(paragraph, per_line)).sum();
    (width.round(), (lines.max(1) as f64 * line_height + 2.0 * padding).round())
}

/// Lines a paragraph takes, wrapped at word breaks to `per_line` characters.
fn wrapped(paragraph: &str, per_line: usize) -> usize {
    let mut lines = 1;
    let mut used = 0;
    for word in paragraph.split_whitespace() {
        let len = word.chars().count();
        let needed = if used == 0 { len } else { used + 1 + len };
        if needed <= per_line {
            used = needed;
        } else if used == 0 {
            // A word longer than a line is broken wherever it runs out.
            lines += (len - 1) / per_line;
            used = (len - 1) % per_line + 1;
        } else {
            lines += 1 + (len.max(1) - 1) / per_line;
            used = (len.max(1) - 1) % per_line + 1;
        }
    }
    lines
}

/// Top-left corner for something of `size`, moved as little as needed to be
/// on the page.
fn keep_on_page(corner: (f64, f64), size: (f64, f64), page_size: (f64, f64)) -> (f64, f64) {
    let clamp = |v: f64, len: f64, room: f64| v.min(room - EDGE - len).max(EDGE);
    (clamp(corner.0, size.0, page_size.0), clamp(corner.1, size.1, page_size.1))
}

fn padded(a: (f64, f64), b: (f64, f64)) -> [f64; 4] {
    [a.0.min(b.0) - TAIL_PAD, a.1.min(b.1) - TAIL_PAD, a.0.max(b.0) + TAIL_PAD, a.1.max(b.1) + TAIL_PAD]
}

pub(super) fn note_key(page: &poppler::Page, note: &Note) -> (poppler::ffi::PopplerAnnotType, [f64; 4]) {
    let (_, height) = page.size();
    (poppler::ffi::POPPLER_ANNOT_TEXT, annots::flip(note.area, height))
}

/// A text box is the box and, for a bubble, its tail.
pub(super) fn box_keys(page: &poppler::Page, bubble: &TextBox) -> Vec<(poppler::ffi::PopplerAnnotType, [f64; 4])> {
    let (_, height) = page.size();
    let mut keys = vec![(poppler::ffi::POPPLER_ANNOT_FREE_TEXT, annots::flip(bubble.area, height))];
    if let Some((from, to)) = bubble.tail() {
        keys.push((poppler::ffi::POPPLER_ANNOT_LINE, annots::flip(padded(from, to), height)));
    }
    keys
}

pub(super) fn add_note(document: &poppler::Document, page: &poppler::Page, note: &Note) {
    use glib::translate::{from_glib_full, ToGlibPtr};
    use poppler::prelude::*;

    let (_, height) = page.size();
    let mut rect = annots::rectangle(annots::flip(note.area, height));
    // SAFETY: Poppler copies the rectangle and the icon's name.
    let annot: poppler::Annot = unsafe {
        let raw = poppler::ffi::poppler_annot_text_new(document.to_glib_none().0, &mut rect);
        poppler::ffi::poppler_annot_text_set_icon(raw.cast(), c"Note".as_ptr());
        from_glib_full(raw)
    };
    annot.set_contents(&note.text);
    // A note is a comment on the page, not part of it: not printed.
    annots::attach(page, &annot, Some(note.colour), false);
}

pub(super) fn add_box(document: &poppler::Document, page: &poppler::Page, text_box: &TextBox) {
    use glib::translate::{from_glib_full, ToGlibPtr};
    use poppler::prelude::*;

    let (_, height) = page.size();
    let doc = document.to_glib_none().0;
    let style = &text_box.style;
    let mut rect = annots::rectangle(annots::flip(text_box.area, height));
    // SAFETY: Poppler copies the rectangle; the font calls are its own, found
    // by name, and the description is freed once Poppler has copied it.
    let annot: poppler::Annot = unsafe {
        let raw = poppler::ffi::poppler_annot_free_text_new(doc, &mut rect);
        let newer = newer::get();
        if let (Some(family), Some(fonts)) = (&style.family, &newer.fonts) {
            if let Ok(name) = std::ffi::CString::new(family.as_str()) {
                let desc = (fonts.desc_new)(name.as_ptr());
                (*desc).size_pt = style.size;
                (*desc).weight = if style.bold { poppler::ffi::POPPLER_WEIGHT_BOLD } else { poppler::ffi::POPPLER_WEIGHT_NORMAL };
                (*desc).style = if style.italic { poppler::ffi::POPPLER_STYLE_ITALIC } else { poppler::ffi::POPPLER_STYLE_NORMAL };
                (fonts.set_desc)(raw, desc);
                (fonts.desc_free)(desc);
                let mut colour = style.colour.ffi();
                (fonts.set_colour)(raw, &mut colour);
            }
        }
        if !style.border {
            if let Some(border) = &newer.border {
                (border.set)(raw, 0.0);
            }
        }
        from_glib_full(raw)
    };
    annot.set_contents(&text_box.text);
    annots::attach(page, &annot, style.fill, true);

    if let Some((from, to)) = text_box.tail() {
        let flip = |(x, y): (f64, f64)| poppler::ffi::PopplerPoint { x, y: height - y };
        let mut rect = annots::rectangle(annots::flip(padded(from, to), height));
        let (mut start, mut end) = (flip(from), flip(to));
        // SAFETY: Poppler copies the rectangle and both points.
        let tail: poppler::Annot =
            unsafe { from_glib_full(poppler::ffi::poppler_annot_line_new(doc, &mut rect, &mut start, &mut end)) };
        annots::attach(page, &tail, Some(TAIL_COLOUR), true);
    }
}

/// How a text box already in the file looks. One with an outline is shown as
/// a bubble is; one without, as its font says, where Poppler can tell.
fn style_of(annot: &poppler::Annot, fill: Option<Rgb>) -> TextStyle {
    use glib::translate::ToGlibPtr;
    let newer = newer::get();
    let raw: *mut poppler::ffi::PopplerAnnot = annot.to_glib_none().0;
    // SAFETY: Poppler's own calls, found by name; what they hand back is ours
    // to free, and freed here.
    unsafe {
        let border = newer.border.as_ref().map_or(1.0, |border| {
            let mut width = 1.0;
            if (border.get)(raw, &mut width) == 0 { 1.0 } else { width }
        });
        if border > 0.0 {
            return TextStyle { fill, ..TextStyle::bubble() };
        }
        let mut style = TextStyle { fill, border: false, ..TextStyle::bubble() };
        if let Some(fonts) = &newer.fonts {
            let desc = (fonts.get_desc)(raw);
            if !desc.is_null() {
                if !(*desc).font_name.is_null() {
                    style.family = Some(std::ffi::CStr::from_ptr((*desc).font_name).to_string_lossy().into_owned());
                }
                style.size = (*desc).size_pt;
                style.bold = (*desc).weight >= poppler::ffi::POPPLER_WEIGHT_SEMIBOLD;
                style.italic = (*desc).style != poppler::ffi::POPPLER_STYLE_NORMAL;
                (fonts.desc_free)(desc);
            }
            let colour = (fonts.get_colour)(raw);
            if !colour.is_null() {
                style.colour = Rgb::from_ffi(&*colour);
                poppler::ffi::poppler_color_free(colour);
            }
        }
        style
    }
}

/// The notes, bubbles and text boxes on a page, read back from the document,
/// including any another program made.
pub fn on_page(document: &poppler::Document, index: usize) -> Vec<Annotation> {
    let Some(page) = annots::page(document, index) else { return Vec::new() };
    let found = annots::list(&page);
    let lines: Vec<[f64; 4]> =
        found.iter().filter(|f| f.kind == poppler::ffi::POPPLER_ANNOT_LINE).map(|f| f.area).collect();
    found
        .iter()
        .filter_map(|f| match f.kind {
            poppler::ffi::POPPLER_ANNOT_TEXT => Some(Annotation::Note(Note {
                page: index,
                area: f.area,
                text: f.text.clone(),
                colour: f.colour.unwrap_or(NOTE_COLOUR),
            })),
            poppler::ffi::POPPLER_ANNOT_FREE_TEXT => Some(Annotation::TextBox(TextBox {
                page: index,
                area: f.area,
                tip: tip_among(f.area, &lines),
                text: f.text.clone(),
                style: style_of(&f.annot, f.colour),
            })),
            _ => None,
        })
        .collect()
}

/// Which line, if any, is this box's tail, and where it points: a line whose
/// one end is exactly where a tail would leave the box for its other end.
fn tip_among(area: [f64; 4], lines: &[[f64; 4]]) -> Option<(f64, f64)> {
    const CLOSE: f64 = 0.1;
    lines.iter().find_map(|&[x1, y1, x2, y2]| {
        let (x1, y1, x2, y2) = (x1 + TAIL_PAD, y1 + TAIL_PAD, x2 - TAIL_PAD, y2 - TAIL_PAD);
        let ends = [((x1, y1), (x2, y2)), ((x2, y2), (x1, y1)), ((x1, y2), (x2, y1)), ((x2, y1), (x1, y2))];
        ends.into_iter().find_map(|(from, to)| {
            let at = attach(area, to)?;
            ((at.0 - from.0).abs() < CLOSE && (at.1 - from.1).abs() < CLOSE).then_some(to)
        })
    })
}

/// The note or bubble under a point, the one drawn last if they overlap.
pub fn under(annotations: &[Annotation], x: f64, y: f64) -> Option<&Annotation> {
    // A little slack round the edges, for a small icon.
    const SLACK: f64 = 2.0;
    annotations.iter().rev().find(|a| {
        let area = match a {
            Annotation::Note(note) => note.area,
            Annotation::TextBox(text_box) => text_box.area,
            Annotation::Mark(_) | Annotation::Ink(_) => return false,
        };
        (area[0] - SLACK..=area[2] + SLACK).contains(&x) && (area[1] - SLACK..=area[3] + SLACK).contains(&y)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_size_bubble(text: &str) -> (f64, f64) {
        box_size(text, &TextStyle::bubble(), None)
    }

    const A4: (f64, f64) = (595.0, 842.0);

    #[test]
    fn a_bubble_sits_above_and_right_of_what_it_points_at() {
        let bubble = TextBox::bubble(0, (100.0, 400.0), A4, "Hello".into());
        let [x1, _, _, y2] = bubble.area;
        assert_eq!(x1, 100.0 + REACH);
        assert_eq!(y2, 400.0 - REACH);
        // The tail leaves the bottom edge, towards the tip.
        let (from, to) = bubble.tail().unwrap();
        assert_eq!(to, (100.0, 400.0));
        assert_eq!(from.1, y2);
    }

    #[test]
    fn near_the_edges_a_bubble_goes_where_there_is_room() {
        // Top right corner: below and to the left.
        let bubble = TextBox::bubble(0, (580.0, 10.0), A4, "Hello there".into());
        let [x1, y1, x2, _] = bubble.area;
        assert!(x2 <= 580.0 - REACH + 1e-9 && x1 >= EDGE, "{:?}", bubble.area);
        assert_eq!(y1, 10.0 + REACH);
    }

    #[test]
    fn longer_text_makes_a_bigger_bubble_up_to_a_width() {
        let (w1, h1) = box_size_bubble("Hi");
        let (w2, h2) = box_size_bubble("A rather longer remark about this page");
        let (w3, h3) = box_size_bubble(&"word ".repeat(80));
        assert_eq!((w1, h1), (NARROWEST, (LINE_HEIGHT + 2.0 * PADDING).round()));
        assert!(w2 > w1 && h2 == h1, "{w2} {h2}");
        assert!(w3 == WIDEST && h3 > 5.0 * LINE_HEIGHT, "{w3} {h3}");
        // Lines the writer broke count too.
        assert_eq!(box_size_bubble("one\ntwo\nthree").1, (3.0 * LINE_HEIGHT + 2.0 * PADDING).round());
    }

    #[test]
    fn wrapping_breaks_at_spaces_and_inside_words_too_long() {
        assert_eq!(wrapped("aaa bbb ccc", 7), 2);
        assert_eq!(wrapped("aaa bbb ccc", 11), 1);
        assert_eq!(wrapped("abcdefghij", 4), 3);
        assert_eq!(wrapped("", 4), 1);
    }

    #[test]
    fn moving_a_bubble_keeps_its_tail_on_the_same_spot() {
        let bubble = TextBox::bubble(0, (100.0, 400.0), A4, "Hello".into());
        let moved = bubble.moved((50.0, -100.0), A4);
        assert_eq!(moved.tip, bubble.tip);
        assert_eq!(moved.area[0], bubble.area[0] + 50.0);
        // Dragged off the page, it stops at the edge.
        let off = bubble.moved((-1000.0, 0.0), A4);
        assert_eq!(off.area[0], EDGE);
    }

    #[test]
    fn a_tail_is_recognised_from_its_line_alone() {
        let bubble = TextBox::bubble(0, (100.0, 400.0), A4, "Hello".into());
        let (from, to) = bubble.tail().unwrap();
        let line = padded(from, to);
        assert_eq!(tip_among(bubble.area, &[line]), Some((100.0, 400.0)));
        // Some other line on the page is not taken for it.
        assert_eq!(tip_among(bubble.area, &[[10.0, 10.0, 40.0, 40.0]]), None);
    }

    #[test]
    fn a_tip_under_the_box_has_no_tail() {
        let bubble = TextBox { page: 0, area: [10.0, 10.0, 100.0, 50.0], tip: Some((20.0, 20.0)), text: String::new(), style: TextStyle::bubble() };
        assert_eq!(bubble.tail(), None);
    }

    #[test]
    fn notes_stay_on_the_page() {
        let note = Note::new(0, (590.0, 5.0), A4, "x".into());
        // Pulled in from the right edge; already clear of the top one.
        assert_eq!(note.area, [595.0 - EDGE - NOTE_SIZE, 5.0, 595.0 - EDGE, 5.0 + NOTE_SIZE]);
        let above = Note::new(0, (100.0, -30.0), A4, "x".into());
        assert_eq!(above.area[1], EDGE);
    }

    #[test]
    fn the_topmost_note_or_bubble_is_the_one_under_the_pointer() {
        let under_it = Annotation::TextBox(TextBox { page: 0, area: [0.0, 0.0, 100.0, 100.0], tip: None, text: "a".into(), style: TextStyle::bubble() });
        let on_top = Annotation::Note(Note::new(0, (10.0, 10.0), A4, "b".into()));
        let both = [under_it.clone(), on_top.clone()];
        assert_eq!(under(&both, 15.0, 15.0), Some(&on_top));
        assert_eq!(under(&both, 80.0, 80.0), Some(&under_it));
        assert_eq!(under(&both, 300.0, 300.0), None);
    }
}
