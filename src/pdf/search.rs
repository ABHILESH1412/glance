// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Finding text in a PDF, on a thread of its own.
//!
//! A long document takes a while to search, and the window must not freeze
//! while it does, so the thread reports each page's matches as it finds them
//! and the count grows as you watch. Typing another letter drops the search in
//! progress and starts again.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gtk::glib;

/// One match: the areas it covers, as x, y, width, height in points from the
/// page's top-left corner. Usually one area; two or more when the words wrap
/// onto the next line.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub page: usize,
    pub areas: Vec<[f64; 4]>,
}

pub enum Found {
    Page(Vec<Match>),
    Done,
}

/// The search stops when this is dropped.
pub struct Searcher {
    stop: Arc<AtomicBool>,
}

impl Searcher {
    pub fn start(uri: String, query: String, results: async_channel::Sender<Found>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let worker = stop.clone();
        std::thread::Builder::new()
            .name("glance-search".into())
            .spawn(move || run(&uri, &query, &worker, &results))
            .expect("the system refused to start a thread");
        Searcher { stop }
    }
}

impl Drop for Searcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn run(uri: &str, query: &str, stop: &AtomicBool, results: &async_channel::Sender<Found>) {
    let Ok(document) = poppler::Document::from_file(uri, None) else {
        let _ = results.send_blocking(Found::Done);
        return;
    };
    // Case does not matter, accents do not matter, and a phrase broken across
    // two lines is still found — the way people expect to search.
    let options = poppler::FindFlags::MULTILINE | poppler::FindFlags::IGNORE_DIACRITICS;
    for index in 0..document.n_pages() {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let Some(page) = document.page(index) else { continue };
        let (_, height) = page.size();
        let mut matches: Vec<Match> = Vec::new();
        let mut continues = false;
        for ([x1, y1, x2, y2], carries_on) in find(&page, query, options) {
            let area = flip(x1, y1, x2, y2, height);
            match matches.last_mut() {
                // The previous area said the match carries on into this one.
                Some(last) if continues => last.areas.push(area),
                _ => matches.push(Match { page: index as usize, areas: vec![area] }),
            }
            continues = carries_on;
        }
        if !matches.is_empty() && results.send_blocking(Found::Page(matches)).is_err() {
            return; // Nobody is listening: the search was replaced.
        }
    }
    let _ = results.send_blocking(Found::Done);
}

/// Every area Poppler matched on a page, as its corners, and whether the match
/// carries on into the next area — a phrase wrapping onto the next line.
///
/// This talks to Poppler's C interface directly, on purpose. The Rust binding
/// copies each rectangle into memory of its own, but Poppler keeps the "carries
/// on" flag in a larger record that extends past the plain rectangle. Asking
/// the binding's copy for it reads beyond the end of the copy: undefined
/// behaviour, which in testing simply answered "no" and split every wrapped
/// match in two. Poppler's own rectangles, read before they are freed, are the
/// only place the flag can be read correctly.
fn find(page: &poppler::Page, query: &str, options: poppler::FindFlags) -> Vec<([f64; 4], bool)> {
    use glib::translate::{IntoGlib, ToGlibPtr};
    let Ok(text) = std::ffi::CString::new(query) else {
        return Vec::new(); // A NUL in the query can match nothing.
    };
    let mut found = Vec::new();
    // SAFETY: `page` is a live PopplerPage for the whole call, and `text` a
    // NUL-terminated string that outlives it. Poppler hands back a list it
    // allocated, of rectangles it allocated as the extended kind that
    // `find_get_match_continued` requires. Each rectangle is read, then freed
    // exactly once; the list itself is freed after the last of them.
    unsafe {
        let list = poppler::ffi::poppler_page_find_text_with_options(
            page.to_glib_none().0,
            text.as_ptr(),
            options.into_glib(),
        );
        let mut node = list;
        while !node.is_null() {
            let rect = (*node).data.cast::<poppler::ffi::PopplerRectangle>();
            if !rect.is_null() {
                let corners = [(*rect).x1, (*rect).y1, (*rect).x2, (*rect).y2];
                let carries_on = poppler::ffi::poppler_rectangle_find_get_match_continued(rect) != glib::ffi::GFALSE;
                found.push((corners, carries_on));
                poppler::ffi::poppler_rectangle_free(rect);
            }
            node = (*node).next;
        }
        glib::ffi::g_list_free(list);
    }
    found
}

/// Poppler reports search matches measured up from the bottom of the page —
/// PDF's own convention — unlike its selection calls, which measure down from
/// the top. Everything else here measures from the top, so flip them.
fn flip(x1: f64, y1: f64, x2: f64, y2: f64, page_height: f64) -> [f64; 4] {
    [x1.min(x2), page_height - y1.max(y2), (x2 - x1).abs(), (y2 - y1).abs()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_match_is_measured_from_the_top_of_the_page() {
        // Measured from Poppler on a test page: "line 1", drawn with its
        // baseline 470pt down an 842pt page, came back as y 369.4 to 381.6.
        let [x, y, w, h] = flip(60.0, 369.4, 200.0, 381.6, 842.0);
        assert_eq!((x, w), (60.0, 140.0));
        assert!((y - 460.4).abs() < 1e-9 && (h - 12.2).abs() < 1e-9, "top {y}, height {h}");
        assert!(y < 470.0 && y + h > 470.0, "the area holds the baseline it was drawn on");
    }

    #[test]
    fn corners_given_either_way_round_make_the_same_area() {
        assert_eq!(flip(60.0, 369.0, 200.0, 381.0, 842.0), flip(200.0, 381.0, 60.0, 369.0, 842.0));
    }
}
