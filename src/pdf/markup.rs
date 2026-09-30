// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Highlighting, underlining and striking through text.
//!
//! Marks are ordinary PDF annotations, the kind every reader shows, written
//! into the file itself the moment they are made, as Preview does. There is
//! no separate save to forget, and the file on disk is always what is on
//! screen, so a page drawn later on another thread simply reopens it.
//!
//! The file is replaced whole: written beside the original, then renamed over
//! it. Poppler reads the document lazily from the original while it writes,
//! so writing into that same file would pull it out from under itself, and a
//! crash halfway through would leave half a PDF.

use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::path::Path;

use gtk::glib;
use poppler::prelude::*;

use super::document::{self, Unit};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Highlight,
    Underline,
    StrikeOut,
}

impl Style {
    /// Highlighter yellow; a red pen for the lines, as in Preview.
    fn colour(self) -> (u16, u16, u16) {
        match self {
            Style::Highlight => (0xffff, 0xe400, 0x0000),
            Style::Underline | Style::StrikeOut => (0xd700, 0x2200, 0x2200),
        }
    }

    fn annot_type(self) -> poppler::ffi::PopplerAnnotType {
        match self {
            Style::Highlight => poppler::ffi::POPPLER_ANNOT_HIGHLIGHT,
            Style::Underline => poppler::ffi::POPPLER_ANNOT_UNDERLINE,
            Style::StrikeOut => poppler::ffi::POPPLER_ANNOT_STRIKE_OUT,
        }
    }
}

/// One line of marked text: the box around it, as x1, y1, x2, y2 in points
/// from the page's top-left corner, and which way up the text stands, in
/// quarter turns clockwise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub area: [f64; 4],
    pub quarter: u8,
}

/// One annotation: a style over some lines of one page.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub page: usize,
    pub style: Style,
    pub lines: Vec<Line>,
}

/// A character on the page, its box, and whether the selection takes it.
#[derive(Clone, Copy, Debug)]
pub struct Glyph {
    pub ch: char,
    pub area: [f64; 4],
    pub selected: bool,
}

/// The lines a selection covers on one page. Empty if it takes no text.
pub fn selected_lines(document: &poppler::Document, page: usize, span: [f64; 4], unit: Unit) -> Vec<Line> {
    // The selection's outline, from Poppler, at the same fineness the
    // highlight on screen uses.
    const FINE: f64 = 4.0;
    let Some(page) = i32::try_from(page).ok().and_then(|i| document.page(i)) else {
        return Vec::new();
    };
    let Some(region) = page.selected_region(FINE, unit.style(), &mut document::rectangle(span)) else {
        return Vec::new();
    };
    let glyphs: Vec<Glyph> = glyphs(&page)
        .into_iter()
        .map(|(ch, area)| {
            let (x, y) = ((area[0] + area[2]) / 2.0 * FINE, (area[1] + area[3]) / 2.0 * FINE);
            Glyph { ch, area, selected: region.contains_point(x as i32, y as i32) }
        })
        .collect();
    lines(&glyphs)
}

/// Split the selected glyphs into lines. A line ends at a line break, at a
/// glyph the selection leaves out, and wherever the next glyph does not sit
/// beside the last one: a hyphenated word runs on without a break in the
/// text, but not on the page.
pub fn lines(glyphs: &[Glyph]) -> Vec<Line> {
    let mut runs: Vec<Vec<[f64; 4]>> = Vec::new();
    let mut open = false;
    for glyph in glyphs {
        if glyph.ch == '\n' || !glyph.selected {
            open = false;
            continue;
        }
        if glyph.ch.is_whitespace() {
            continue; // Part of the line, but not what its box is measured by.
        }
        match runs.last_mut() {
            Some(run) if open && run.last().is_some_and(|&last| beside(last, glyph.area)) => run.push(glyph.area),
            _ => runs.push(vec![glyph.area]),
        }
        open = true;
    }

    let mut lines: Vec<(Line, bool)> = runs
        .iter()
        .map(|run| {
            let area = run.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, a| {
                [b[0].min(a[0]), b[1].min(a[1]), b[2].max(a[2]), b[3].max(a[3])]
            });
            let quarter = direction(run);
            (Line { area, quarter: quarter.unwrap_or(0) }, quarter.is_some())
        })
        .collect();
    // A line of one character does not say which way it runs; it runs the
    // way the lines around it do.
    let usual = lines.iter().find(|(_, known)| *known).map_or(0, |(line, _)| line.quarter);
    for (line, known) in &mut lines {
        if !*known {
            line.quarter = usual;
        }
    }
    lines.into_iter().map(|(line, _)| line).collect()
}

/// Whether two glyphs are on the same line: level with each other, or, for
/// text running down a turned page, one above the other.
fn beside(a: [f64; 4], b: [f64; 4]) -> bool {
    let overlap = |a1: f64, a2: f64, b1: f64, b2: f64| a2.min(b2) - a1.max(b1) >= 0.5 * (a2 - a1).min(b2 - b1);
    overlap(a[1], a[3], b[1], b[3]) || overlap(a[0], a[2], b[0], b[2])
}

/// Which way a line reads, from its first glyph to its last: across the page
/// as usual, or down or up a page the PDF turns a quarter. `None` for a
/// single glyph. Text reading right to left is taken as upright too, since
/// that is far more likely than a page turned upside down.
fn direction(run: &[[f64; 4]]) -> Option<u8> {
    let (first, last) = (run.first()?, run.last()?);
    if run.len() < 2 {
        return None;
    }
    let dx = (last[0] + last[2] - first[0] - first[2]) / 2.0;
    let dy = (last[1] + last[3] - first[1] - first[3]) / 2.0;
    Some(if dy.abs() <= dx.abs() {
        0
    } else if dy > 0.0 {
        1
    } else {
        3
    })
}

/// A line's corners as the PDF wants them: the text's own top left, top
/// right, bottom left and bottom right, whichever way the text stands on the
/// page, with y measured up from the bottom. An underline is drawn along the
/// last two, so they have to be the bottom of the text, not of the box.
fn corners(line: Line, page_height: f64) -> [(f64, f64); 4] {
    let [x1, y1, x2, y2] = line.area;
    let points = match line.quarter {
        1 => [(x2, y1), (x2, y2), (x1, y1), (x1, y2)],
        2 => [(x2, y2), (x1, y2), (x2, y1), (x1, y1)],
        3 => [(x1, y2), (x1, y1), (x2, y2), (x2, y1)],
        _ => [(x1, y1), (x2, y1), (x1, y2), (x2, y2)],
    };
    points.map(|(x, y)| (x, page_height - y))
}

/// The box around every line of a mark, y measured up from the bottom, as
/// x1, y1, x2, y2. How Poppler reports the mark back, so also how it is found
/// again.
fn bounds(lines: &[Line], page_height: f64) -> [f64; 4] {
    let b = lines.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, line| {
        let a = line.area;
        [b[0].min(a[0]), b[1].min(a[1]), b[2].max(a[2]), b[3].max(a[3])]
    });
    [b[0], page_height - b[3], b[2], page_height - b[1]]
}

/// Add a mark to the document in memory. Nothing is written until `save`.
pub fn add(document: &poppler::Document, mark: &Mark) {
    let Some(page) = i32::try_from(mark.page).ok().and_then(|i| document.page(i)) else { return };
    if mark.lines.is_empty() {
        return;
    }
    let (_, height) = page.size();
    let annot = create(document, mark.style, &mark.lines, height);
    let (red, green, blue) = mark.style.colour();
    let mut colour = poppler::Color::new();
    colour.set_red(red);
    colour.set_green(green);
    colour.set_blue(blue);
    annot.set_color(Some(&colour));
    // Printed with the page, as a highlighter's ink would be.
    annot.set_flags(poppler::AnnotFlag::PRINT);
    page.add_annot(&annot);
}

/// Take a mark off again. False if it is not there.
pub fn remove(document: &poppler::Document, mark: &Mark) -> bool {
    let Some(page) = i32::try_from(mark.page).ok().and_then(|i| document.page(i)) else { return false };
    match find(&page, mark) {
        Some(annot) => {
            page.remove_annot(&annot);
            true
        }
        None => false,
    }
}

/// Whether the page already carries this very mark.
pub fn exists(document: &poppler::Document, mark: &Mark) -> bool {
    i32::try_from(mark.page).ok().and_then(|i| document.page(i)).is_some_and(|page| find(&page, mark).is_some())
}

fn create(document: &poppler::Document, style: Style, lines: &[Line], page_height: f64) -> poppler::Annot {
    use glib::translate::{from_glib_full, ToGlibPtr};
    use poppler::ffi::{PopplerPoint, PopplerQuadrilateral, PopplerRectangle};

    let [x1, y1, x2, y2] = bounds(lines, page_height);
    let mut rect = PopplerRectangle { x1, y1, x2, y2 };
    // SAFETY: the array holds plain structs and is released after Poppler
    // has copied what it needs from it; the rectangle is read, not kept.
    unsafe {
        let size = std::mem::size_of::<PopplerQuadrilateral>() as u32;
        let quads = glib::ffi::g_array_sized_new(glib::ffi::GFALSE, glib::ffi::GFALSE, size, lines.len() as u32);
        for &line in lines {
            let [p1, p2, p3, p4] = corners(line, page_height).map(|(x, y)| PopplerPoint { x, y });
            let quad = PopplerQuadrilateral { p1, p2, p3, p4 };
            glib::ffi::g_array_append_vals(quads, std::ptr::from_ref(&quad).cast(), 1);
        }
        let doc = document.to_glib_none().0;
        let annot = match style {
            Style::Highlight => poppler::ffi::poppler_annot_text_markup_new_highlight(doc, &mut rect, quads),
            Style::Underline => poppler::ffi::poppler_annot_text_markup_new_underline(doc, &mut rect, quads),
            Style::StrikeOut => poppler::ffi::poppler_annot_text_markup_new_strikeout(doc, &mut rect, quads),
        };
        glib::ffi::g_array_unref(quads);
        from_glib_full(annot)
    }
}

/// The page's annotation matching a mark: same kind, same place.
fn find(page: &poppler::Page, mark: &Mark) -> Option<poppler::Annot> {
    use glib::translate::{from_glib_none, ToGlibPtr};

    // Poppler stores coordinates as written in the file, so allow for them
    // coming back a hair different from what was given.
    const CLOSE: f64 = 0.05;
    let (_, height) = page.size();
    let want = bounds(&mark.lines, height);
    let mut found = None;
    // SAFETY: the list and every mapping in it belong to Poppler until freed
    // below; the annotation found is taken with a reference of its own.
    unsafe {
        let list = poppler::ffi::poppler_page_get_annot_mapping(page.to_glib_none().0);
        let mut node = list;
        while !node.is_null() {
            let mapping = (*node).data.cast::<poppler::ffi::PopplerAnnotMapping>();
            if found.is_none() && !mapping.is_null() && !(*mapping).annot.is_null() {
                let area = (*mapping).area;
                let corners = [area.x1, area.y1, area.x2, area.y2];
                let near = corners.iter().zip(want).all(|(a, b)| (a - b).abs() < CLOSE);
                let kind = poppler::ffi::poppler_annot_get_annot_type((*mapping).annot);
                if near && kind == mark.style.annot_type() {
                    found = Some(from_glib_none((*mapping).annot));
                }
            }
            node = (*node).next;
        }
        poppler::ffi::poppler_page_free_annot_mapping(list);
    }
    found
}

/// Every character on the page, in reading order, with its box.
fn glyphs(page: &poppler::Page) -> Vec<(char, [f64; 4])> {
    use glib::translate::ToGlibPtr;

    let Some(text) = page.text() else { return Vec::new() };
    let mut rects: *mut poppler::ffi::PopplerRectangle = std::ptr::null_mut();
    let mut count: u32 = 0;
    // SAFETY: Poppler hands back an array of `count` rectangles, ours to free.
    let areas: Vec<[f64; 4]> = unsafe {
        if poppler::ffi::poppler_page_get_text_layout(page.to_glib_none().0, &mut rects, &mut count)
            == glib::ffi::GFALSE
            || rects.is_null()
        {
            return Vec::new();
        }
        let areas =
            std::slice::from_raw_parts(rects, count as usize).iter().map(|r| [r.x1, r.y1, r.x2, r.y2]).collect();
        glib::ffi::g_free(rects.cast());
        areas
    };
    // One box per character of the page's text. Should they ever disagree,
    // pairing them up would mark the wrong words, so mark nothing.
    if text.chars().count() != areas.len() {
        return Vec::new();
    }
    text.chars().zip(areas).collect()
}

/// Write the document, marks and all, over the file it came from.
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
    use super::*;

    fn glyph(ch: char, x: f64, y: f64) -> Glyph {
        Glyph { ch, area: [x, y, x + 8.0, y + 12.0], selected: true }
    }

    /// A line of text, one glyph every 8 points from `x`, at height `y`.
    fn text(s: &str, x: f64, y: f64) -> Vec<Glyph> {
        s.chars().enumerate().map(|(i, ch)| glyph(ch, x + 8.0 * i as f64, y)).collect()
    }

    /// Text running down the page, one glyph every 8 points from `y`.
    fn column(s: &str, x: f64, y: f64) -> Vec<Glyph> {
        s.chars()
            .enumerate()
            .map(|(i, ch)| {
                let top = y + 8.0 * i as f64;
                Glyph { ch, area: [x, top, x + 12.0, top + 8.0], selected: true }
            })
            .collect()
    }

    #[test]
    fn each_line_of_a_selection_is_marked_on_its_own() {
        let mut glyphs = text("one two", 72.0, 100.0);
        glyphs.push(glyph('\n', 128.0, 100.0));
        glyphs.extend(text("three", 72.0, 120.0));
        let got = lines(&glyphs);
        assert_eq!(
            got,
            vec![
                Line { area: [72.0, 100.0, 128.0, 112.0], quarter: 0 },
                Line { area: [72.0, 120.0, 112.0, 132.0], quarter: 0 },
            ]
        );
    }

    #[test]
    fn a_word_hyphenated_across_lines_is_still_two_lines() {
        // No line break in the text, but the second half is back at the left
        // on the line below.
        let mut glyphs = text("exam-", 300.0, 100.0);
        glyphs.extend(text("ple", 72.0, 120.0));
        assert_eq!(lines(&glyphs).len(), 2);
    }

    #[test]
    fn glyphs_left_out_of_the_selection_are_not_marked() {
        let mut glyphs = text("keep skip", 72.0, 100.0);
        for g in &mut glyphs[4..] {
            g.selected = false;
        }
        assert_eq!(lines(&glyphs), vec![Line { area: [72.0, 100.0, 104.0, 112.0], quarter: 0 }]);
        for g in &mut glyphs {
            g.selected = false;
        }
        assert!(lines(&glyphs).is_empty());
    }

    #[test]
    fn spaces_at_the_ends_do_not_stretch_the_mark() {
        let glyphs = text(" word ", 72.0, 100.0);
        assert_eq!(lines(&glyphs), vec![Line { area: [80.0, 100.0, 112.0, 112.0], quarter: 0 }]);
    }

    #[test]
    fn text_running_down_a_turned_page_is_one_line() {
        // A page the PDF turns a quarter clockwise: each glyph sits below the
        // one before it.
        let down = column("down", 200.0, 100.0);
        let got = lines(&down);
        assert_eq!(got, vec![Line { area: [200.0, 100.0, 212.0, 132.0], quarter: 1 }]);
        let up: Vec<Glyph> = down.iter().rev().copied().collect();
        assert_eq!(lines(&up)[0].quarter, 3);
    }

    #[test]
    fn a_single_letter_runs_the_way_its_neighbours_do() {
        let mut glyphs = column("ab", 200.0, 100.0);
        glyphs.push(Glyph { ch: '\n', ..glyphs[1] });
        glyphs.push(Glyph { ch: 'c', area: [180.0, 100.0, 192.0, 108.0], selected: true });
        let got = lines(&glyphs);
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].quarter, 1);
    }

    #[test]
    fn the_underline_edge_is_the_bottom_of_the_text() {
        // Corners three and four carry the underline.
        let line = |quarter| Line { area: [10.0, 20.0, 50.0, 30.0], quarter };
        let h = 100.0;
        // Upright: along the bottom of the box, which is y 30 from the top.
        let [_, _, c, d] = corners(line(0), h);
        assert_eq!((c, d), ((10.0, 70.0), (50.0, 70.0)));
        // Turned clockwise, the text's bottom faces left.
        let [_, _, c, d] = corners(line(1), h);
        assert_eq!((c.0, d.0), (10.0, 10.0));
        // Turned anticlockwise, it faces right.
        let [_, _, c, d] = corners(line(3), h);
        assert_eq!((c.0, d.0), (50.0, 50.0));
    }

    #[test]
    fn bounds_are_measured_from_the_bottom() {
        let lines = [
            Line { area: [10.0, 20.0, 50.0, 30.0], quarter: 0 },
            Line { area: [5.0, 32.0, 40.0, 42.0], quarter: 0 },
        ];
        assert_eq!(bounds(&lines, 100.0), [5.0, 58.0, 50.0, 80.0]);
    }
}
