// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Everything a PDF says about itself, for the Document Info window.
//!
//! Gathered on a thread of its own: listing the fonts means reading every
//! page, which in a long document takes a moment. What comes back is plain
//! text, in sections, ready to show; anything the file does not say is left
//! out rather than shown blank.

use std::path::Path;

use gtk::glib;

pub struct Info {
    pub sections: Vec<Section>,
}

pub struct Section {
    pub title: String,
    pub rows: Vec<(String, String)>,
}

impl Section {
    fn new(title: &str) -> Self {
        Section { title: title.to_string(), rows: Vec::new() }
    }

    fn add(&mut self, label: &str, value: impl Into<String>) {
        let value = value.into();
        let value = value.trim();
        if !value.is_empty() {
            self.rows.push((label.to_string(), value.to_string()));
        }
    }
}

/// Read a document's information. `current` is the page being read, whose
/// size is given first. Runs on a worker thread.
pub fn gather(path: &Path, current: usize) -> Result<Info, String> {
    let document =
        poppler::Document::from_file(&super::document::uri(path), None).map_err(|e| e.message().to_string())?;
    let count = document.n_pages().max(0) as usize;

    let mut about = Section::new("Document");
    about.add("Title", document.title().unwrap_or_default());
    about.add("Author", document.author().unwrap_or_default());
    about.add("Subject", document.subject().unwrap_or_default());
    about.add("Keywords", document.keywords().unwrap_or_default());
    about.add("Created with", document.creator().unwrap_or_default());
    about.add("Converted to PDF by", document.producer().unwrap_or_default());
    about.add("Created", document.creation_datetime().map(|d| date(&d)).unwrap_or_default());
    about.add("Modified", document.mod_datetime().map(|d| date(&d)).unwrap_or_default());

    let mut file = Section::new("File");
    file.add("Name", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    file.add("Folder", path.parent().map(|p| p.display().to_string()).unwrap_or_default());
    if let Ok(metadata) = std::fs::metadata(path) {
        file.add("Size", bytes(metadata.len()));
    }
    file.add("PDF version", document.pdf_version_string().unwrap_or_default().replace('-', " "));
    file.add("Standard", document.pdf_subtype_string().unwrap_or_default());
    file.add("Fast web view", if document.is_linearized() { "Yes" } else { "No" });

    let mut pages = Section::new("Pages");
    pages.add("Pages", count.to_string());
    let sizes: Vec<(f64, f64)> = (0..count)
        .filter_map(|i| document.page(i32::try_from(i).ok()?))
        .map(|page| page.size())
        .collect();
    if let Some(&size) = sizes.get(current).or(sizes.first()) {
        let label = if sizes.len() > 1 { "This page" } else { "Page size" };
        pages.add(label, page_size(size));
    }
    let distinct = distinct_sizes(&sizes);
    if distinct.len() > 1 {
        let listed: Vec<String> = distinct
            .iter()
            .map(|&(size, n)| format!("{} — {n} {}", page_size(size), if n == 1 { "page" } else { "pages" }))
            .collect();
        pages.add("Sizes", listed.join("\n"));
    }
    pages.add("Opens as", layout_name(document.page_layout()));
    pages.add("Opens with", mode_name(document.page_mode()));

    let mut contents = Section::new("Contents");
    contents.add("Table of contents", if has_outline(&document) { "Yes" } else { "No" });
    let (notes, marks, others) = count_annotations(&document, count);
    contents.add("Notes and speech bubbles", notes.to_string());
    contents.add("Highlights and other marks", marks.to_string());
    if others > 0 {
        contents.add("Other annotations", others.to_string());
    }
    contents.add("Attachments", document.n_attachments().to_string());

    let mut security = Section::new("Security");
    let allowed = document.permissions();
    if allowed.contains(poppler::Permissions::FULL) {
        security.add("Restrictions", "None");
    } else {
        let yes_no = |flag: poppler::Permissions| if allowed.contains(flag) { "Allowed" } else { "Not allowed" };
        security.add("Printing", yes_no(poppler::Permissions::OK_TO_PRINT));
        security.add("Copying text and images", yes_no(poppler::Permissions::OK_TO_COPY));
        security.add("Changing the document", yes_no(poppler::Permissions::OK_TO_MODIFY));
        security.add("Adding notes", yes_no(poppler::Permissions::OK_TO_ADD_NOTES));
        security.add("Filling in forms", yes_no(poppler::Permissions::OK_TO_FILL_FORM));
        security.add("Assembling pages", yes_no(poppler::Permissions::OK_TO_ASSEMBLE));
    }

    let mut fonts = Section::new("Fonts");
    for (name, detail) in font_list(&document, count) {
        fonts.add(&name, detail);
    }

    let sections = [about, file, pages, contents, security, fonts].into_iter().filter(|s| !s.rows.is_empty()).collect();
    Ok(Info { sections })
}

fn date(when: &glib::DateTime) -> String {
    let local = when.to_local().unwrap_or_else(|_| when.clone());
    local.format("%-d %B %Y, %H:%M").map(|s| s.to_string()).unwrap_or_default()
}

/// A size in bytes as people read it, and exactly.
fn bytes(n: u64) -> String {
    let exact = group(n);
    let units = ["KB", "MB", "GB"];
    let mut value = n as f64;
    let mut unit = None;
    for u in units {
        if value < 1000.0 {
            break;
        }
        value /= 1000.0;
        unit = Some(u);
    }
    match unit {
        Some(unit) => format!("{value:.1} {unit} ({exact} bytes)"),
        None => format!("{exact} bytes"),
    }
}

/// 1234567 as 1,234,567.
fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Paper sizes by name, in points, portrait.
const PAPER: &[(&str, f64, f64)] = &[
    ("A3", 841.89, 1190.55),
    ("A4", 595.28, 841.89),
    ("A5", 419.53, 595.28),
    ("A6", 297.64, 419.53),
    ("B4", 708.66, 1000.63),
    ("B5", 498.90, 708.66),
    ("Letter", 612.0, 792.0),
    ("Legal", 612.0, 1008.0),
    ("Tabloid", 792.0, 1224.0),
    ("Executive", 522.0, 756.0),
];

/// A page size as "A4, portrait · 210 × 297 mm (8.27 × 11.69 in)".
fn page_size((w, h): (f64, f64)) -> String {
    const CLOSE: f64 = 2.0;
    let (short, long) = (w.min(h), w.max(h));
    let orientation = if (w - h).abs() < CLOSE {
        "square"
    } else if w < h {
        "portrait"
    } else {
        "landscape"
    };
    let mm = |pt: f64| (pt / 72.0 * 25.4).round();
    let inches = |pt: f64| pt / 72.0;
    let measure = format!("{} × {} mm ({:.2} × {:.2} in)", mm(w), mm(h), inches(w), inches(h));
    match PAPER.iter().find(|(_, pw, ph)| (pw - short).abs() < CLOSE && (ph - long).abs() < CLOSE) {
        Some((name, _, _)) => format!("{name}, {orientation} · {measure}"),
        None => format!("{orientation} · {measure}", orientation = capitalise(orientation)),
    }
}

fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// The different page sizes in a document, each with how many pages have it,
/// in the order they first appear.
fn distinct_sizes(sizes: &[(f64, f64)]) -> Vec<((f64, f64), usize)> {
    let mut distinct: Vec<((f64, f64), usize)> = Vec::new();
    for &size in sizes {
        match distinct.iter_mut().find(|(s, _)| (s.0 - size.0).abs() < 1.0 && (s.1 - size.1).abs() < 1.0) {
            Some((_, n)) => *n += 1,
            None => distinct.push((size, 1)),
        }
    }
    distinct
}

fn layout_name(layout: poppler::PageLayout) -> &'static str {
    match layout {
        poppler::PageLayout::SinglePage => "One page at a time",
        poppler::PageLayout::OneColumn => "Continuous scroll",
        poppler::PageLayout::TwoColumnLeft | poppler::PageLayout::TwoColumnRight => "Two pages, scrolling",
        poppler::PageLayout::TwoPageLeft | poppler::PageLayout::TwoPageRight => "Two pages at a time",
        _ => "",
    }
}

fn mode_name(mode: poppler::PageMode) -> &'static str {
    match mode {
        poppler::PageMode::UseOutlines => "Table of contents showing",
        poppler::PageMode::UseThumbs => "Page thumbnails showing",
        poppler::PageMode::FullScreen => "Full screen",
        poppler::PageMode::UseAttachments => "Attachments showing",
        _ => "",
    }
}

fn has_outline(document: &poppler::Document) -> bool {
    use glib::translate::ToGlibPtr;
    // SAFETY: an outline's iterator, or null for none, freed at once.
    unsafe {
        let iter = poppler::ffi::poppler_index_iter_new(document.to_glib_none().0);
        if iter.is_null() {
            return false;
        }
        poppler::ffi::poppler_index_iter_free(iter);
        true
    }
}

/// Notes and bubbles, text marks, and everything else, links aside.
fn count_annotations(document: &poppler::Document, count: usize) -> (usize, usize, usize) {
    use poppler::ffi as f;
    let (mut notes, mut marks, mut others) = (0, 0, 0);
    for index in 0..count {
        let Some(page) = super::annots::page(document, index) else { continue };
        for found in super::annots::list(&page) {
            match found.kind {
                f::POPPLER_ANNOT_TEXT | f::POPPLER_ANNOT_FREE_TEXT => notes += 1,
                f::POPPLER_ANNOT_HIGHLIGHT
                | f::POPPLER_ANNOT_UNDERLINE
                | f::POPPLER_ANNOT_STRIKE_OUT
                | f::POPPLER_ANNOT_SQUIGGLY => marks += 1,
                // A bubble's tail, and the pop-ups other programs attach to
                // notes, are parts of something already counted.
                f::POPPLER_ANNOT_LINE | f::POPPLER_ANNOT_POPUP | f::POPPLER_ANNOT_LINK | f::POPPLER_ANNOT_WIDGET => {}
                _ => others += 1,
            }
        }
    }
    (notes, marks, others)
}

/// Every font the document uses: its name, and what kind it is.
fn font_list(document: &poppler::Document, count: usize) -> Vec<(String, String)> {
    let info = poppler::FontInfo::new(document);
    let Some(mut iter) = info.scan(i32::try_from(count).unwrap_or(i32::MAX)) else { return Vec::new() };
    let mut fonts = Vec::new();
    loop {
        let name = iter.full_name().or_else(|| iter.name()).map(|n| n.to_string()).unwrap_or_default();
        let kind = match iter.font_type() {
            poppler::FontType::Type1 | poppler::FontType::Type1c | poppler::FontType::Type1cot => "Type 1",
            poppler::FontType::Type3 => "Type 3",
            poppler::FontType::Truetype | poppler::FontType::Truetypeot => "TrueType",
            poppler::FontType::CidType0 | poppler::FontType::CidType0c | poppler::FontType::CidType0cot => {
                "Type 1 (CID)"
            }
            poppler::FontType::CidType2 | poppler::FontType::CidType2ot => "TrueType (CID)",
            _ => "Unknown type",
        };
        let embedded = if iter.is_subset() {
            "embedded subset"
        } else if iter.is_embedded() {
            "embedded"
        } else {
            "not embedded"
        };
        let mut detail = format!("{kind}, {embedded}");
        if !iter.is_embedded() {
            if let Some(stand_in) = iter.substitute_name() {
                detail.push_str(&format!(", shown as {stand_in}"));
            }
        }
        if !name.is_empty() {
            fonts.push((name, detail));
        }
        if !iter.next() {
            break;
        }
    }
    fonts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paper_sizes_are_named_either_way_up() {
        assert_eq!(page_size((595.0, 842.0)), "A4, portrait · 210 × 297 mm (8.26 × 11.69 in)");
        assert_eq!(page_size((842.0, 595.0)), "A4, landscape · 297 × 210 mm (11.69 × 8.26 in)");
        assert!(page_size((612.0, 792.0)).starts_with("Letter, portrait · 216 × 279 mm"));
        assert!(page_size((300.0, 300.0)).starts_with("Square · 106 × 106 mm"));
    }

    #[test]
    fn sizes_read_as_people_say_them() {
        assert_eq!(bytes(512), "512 bytes");
        assert_eq!(bytes(1_234_567), "1.2 MB (1,234,567 bytes)");
        assert_eq!(group(1000), "1,000");
        assert_eq!(group(999), "999");
    }

    #[test]
    fn mixed_page_sizes_are_counted() {
        let a4 = (595.0, 842.0);
        let got = distinct_sizes(&[a4, a4, (842.0, 595.0), (595.3, 841.9)]);
        assert_eq!(got, vec![(a4, 3), ((842.0, 595.0), 1)]);
    }

    #[test]
    fn empty_values_are_left_out() {
        let mut section = Section::new("x");
        section.add("Title", "  ");
        section.add("Author", "Ann");
        assert_eq!(section.rows, vec![("Author".to_string(), "Ann".to_string())]);
    }
}
