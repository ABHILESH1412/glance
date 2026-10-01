// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! A document's table of contents, as its author wrote it: the outline every
//! PDF reader shows beside the pages.
//!
//! Read once, on the thread that opens the document, into plain data that can
//! cross back to the window. poppler-rs leaves out the call that says where a
//! heading leads, so this walks the outline through Poppler's C interface.

use std::ffi::CStr;

use gtk::glib::translate::ToGlibPtr;
use poppler::ffi;

/// Deeper than any real table of contents; a damaged one that loops back on
/// itself stops here.
const MAX_DEPTH: usize = 32;
/// Headings read in all, for the same reason.
const MAX_HEADINGS: usize = 20_000;

#[derive(Clone, Debug, PartialEq)]
pub struct Heading {
    pub title: String,
    /// Where it leads, if anywhere in this document: a page, counting from
    /// zero, and how far down it, in points from the top, when the document
    /// says.
    pub target: Option<(usize, Option<f64>)>,
    /// Shown open at first, as the author left it.
    pub open: bool,
    pub children: Vec<Heading>,
}

impl Heading {
    pub fn page(&self) -> Option<usize> {
        self.target.map(|(page, _)| page)
    }
}

/// The outline, or nothing if the document has none. `sizes` are the pages'
/// sizes in points, for measuring from the top rather than the bottom.
pub fn read(document: &poppler::Document, sizes: &[(f64, f64)]) -> Vec<Heading> {
    let doc: *mut ffi::PopplerDocument = document.to_glib_none().0;
    let mut budget = MAX_HEADINGS;
    // SAFETY: the iterator belongs to this call and is freed here; `level`
    // only reads through it while it is alive.
    unsafe {
        let iter = ffi::poppler_index_iter_new(doc);
        if iter.is_null() {
            return Vec::new();
        }
        let headings = level(doc, iter, sizes, 0, &mut budget);
        ffi::poppler_index_iter_free(iter);
        headings
    }
}

/// One level of the outline, and everything under it.
unsafe fn level(
    doc: *mut ffi::PopplerDocument,
    iter: *mut ffi::PopplerIndexIter,
    sizes: &[(f64, f64)],
    depth: usize,
    budget: &mut usize,
) -> Vec<Heading> {
    let mut headings = Vec::new();
    loop {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let action = ffi::poppler_index_iter_get_action(iter);
        let (title, target) = if action.is_null() {
            (String::new(), None)
        } else {
            let title = text((*action).any.title);
            let target = if (*action).type_ == ffi::POPPLER_ACTION_GOTO_DEST {
                destination(doc, (*action).goto_dest.dest, sizes)
            } else {
                None
            };
            ffi::poppler_action_free(action);
            (title, target)
        };
        let mut children = Vec::new();
        if depth < MAX_DEPTH {
            let child = ffi::poppler_index_iter_get_child(iter);
            if !child.is_null() {
                children = level(doc, child, sizes, depth + 1, budget);
                ffi::poppler_index_iter_free(child);
            }
        }
        let open = ffi::poppler_index_iter_is_open(iter) != 0;
        let title = if title.is_empty() { "Untitled".to_string() } else { title };
        headings.push(Heading { title, target, open, children });
        if ffi::poppler_index_iter_next(iter) == 0 {
            break;
        }
    }
    headings
}

/// A heading's title on one line: authors' outlines carry stray line breaks
/// and runs of spaces.
fn text(raw: *const std::ffi::c_char) -> String {
    if raw.is_null() {
        return String::new();
    }
    // SAFETY: Poppler's titles are NUL-terminated and live as long as the
    // action, which outlives this call.
    let raw = unsafe { CStr::from_ptr(raw) }.to_string_lossy();
    raw.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

/// Where a destination leads: a page, and how far down it.
unsafe fn destination(
    doc: *mut ffi::PopplerDocument,
    dest: *mut ffi::PopplerDest,
    sizes: &[(f64, f64)],
) -> Option<(usize, Option<f64>)> {
    if dest.is_null() {
        return None;
    }
    // A named destination is looked up in the document's table of names.
    let (found, owned) = if (*dest).type_ == ffi::POPPLER_DEST_NAMED {
        if (*dest).named_dest.is_null() {
            return None;
        }
        let found = ffi::poppler_document_find_dest(doc, (*dest).named_dest);
        if found.is_null() {
            return None;
        }
        (found, true)
    } else {
        (dest, false)
    };
    let page = usize::try_from((*found).page_num).ok().and_then(|n| n.checked_sub(1)).filter(|&p| p < sizes.len());
    // `change_top` is the second of three one-bit fields that share the word
    // the bindings call `change_left`.
    let change_top = ((*found).change_left >> 1) & 1 == 1;
    let has_top = matches!(
        (*found).type_,
        ffi::POPPLER_DEST_XYZ | ffi::POPPLER_DEST_FITH | ffi::POPPLER_DEST_FITBH | ffi::POPPLER_DEST_FITR
    );
    let top = (has_top && change_top).then_some((*found).top);
    if owned {
        ffi::poppler_dest_free(found);
    }
    let page = page?;
    // PDF measures up from the bottom of the page; the reader, down from the top.
    let height = sizes[page].1;
    let y = top.filter(|t| t.is_finite()).map(|t| (height - t).clamp(0.0, height));
    Some((page, y))
}

/// The heading a page falls under: the last one, in reading order, that
/// starts on it or before it, as deep as the outline goes.
pub fn covering(headings: &[Heading], page: usize) -> Option<&Heading> {
    let mut best = None;
    walk(headings, &mut |heading| {
        if heading.page().is_some_and(|p| p <= page) {
            best = Some(heading);
        }
    });
    best
}

fn walk<'a>(headings: &'a [Heading], visit: &mut impl FnMut(&'a Heading)) {
    for heading in headings {
        visit(heading);
        walk(&heading.children, visit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Three A4 pages with an outline: a chapter with two sections under it,
    /// one by page and position, one by name, and a heading that leads
    /// nowhere.
    fn outlined_pdf(path: &std::path::Path) {
        let objects: Vec<Vec<u8>> = vec![
            // 1: catalog, with the outline and a table of named destinations.
            b"<< /Type /Catalog /Pages 2 0 R /Outlines 6 0 R /Dests << /methods [4 0 R /FitH 500] >> >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>".to_vec(),
            // 6: the outline's root.
            b"<< /Type /Outlines /First 7 0 R /Last 10 0 R /Count 4 >>".to_vec(),
            // 7: a chapter, open, with two sections.
            b"<< /Title (1  Introduction\n) /Parent 6 0 R /Next 10 0 R /First 8 0 R /Last 9 0 R /Count 2 \
              /Dest [3 0 R /Fit] >>"
                .to_vec(),
            b"<< /Title (1.1 Background) /Parent 7 0 R /Next 9 0 R /Dest [3 0 R /XYZ 72 642 0] >>".to_vec(),
            b"<< /Title (1.2 Methods) /Parent 7 0 R /Prev 8 0 R /Dest (methods) >>".to_vec(),
            // 10: closed (negative count), leading nowhere.
            b"<< /Title (Appendix) /Parent 6 0 R /Prev 7 0 R /First 11 0 R /Last 11 0 R /Count -1 >>".to_vec(),
            b"<< /Title (A.1 Tables) /Parent 10 0 R /Dest [5 0 R /XYZ null null null] >>".to_vec(),
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
    fn the_outline_is_read_as_the_author_wrote_it() {
        let dir = std::env::temp_dir().join(format!("glance-outline-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("outlined.pdf");
        outlined_pdf(&path);
        let document = poppler::Document::from_file(&super::super::document::uri(&path), None).unwrap();
        let a4 = (595.0, 842.0);
        let headings = read(&document, &[a4, a4, a4]);
        fs::remove_dir_all(&dir).unwrap();

        assert_eq!(headings.len(), 2);
        let intro = &headings[0];
        assert_eq!(intro.title, "1 Introduction", "line breaks and doubled spaces tidied");
        assert_eq!(intro.target, Some((0, None)), "a whole-page view has no position");
        assert!(intro.open);
        assert_eq!(intro.children[0].title, "1.1 Background");
        assert_eq!(intro.children[0].target, Some((0, Some(200.0))), "642 up from the bottom is 200 down");
        assert_eq!(intro.children[1].target, Some((1, Some(342.0))), "found by name");

        let appendix = &headings[1];
        assert_eq!(appendix.target, None);
        assert!(!appendix.open);
        assert_eq!(appendix.children[0].target, Some((2, None)), "an unchanged top is no position");
    }

    #[test]
    fn a_page_falls_under_the_last_heading_before_it() {
        let heading = |title: &str, page: Option<usize>, children| Heading {
            title: title.to_string(),
            target: page.map(|p| (p, None)),
            open: true,
            children,
        };
        let outline = vec![
            heading("One", Some(0), vec![heading("1.1", Some(2), vec![]), heading("1.2", Some(5), vec![])]),
            heading("Nowhere", None, vec![]),
            heading("Two", Some(9), vec![]),
        ];
        let title = |page| covering(&outline, page).map(|h| h.title.as_str());
        assert_eq!(title(0), Some("One"));
        assert_eq!(title(1), Some("One"));
        assert_eq!(title(4), Some("1.1"));
        assert_eq!(title(8), Some("1.2"));
        assert_eq!(title(30), Some("Two"));
        assert_eq!(covering(&[], 3), None);
    }
}
