// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Showing tone the GPU cannot, and the histogram for levels.
//!
//! One thread per picture being edited works through requests, always the
//! newest: a slider dragged across its range asks dozens of times, and only
//! where it stops matters. A copy no longer than `QUICK` on its longest side
//! answers while the slider moves; the whole picture follows once it rests,
//! so zooming in shows the real thing.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::mpsc;

use gtk::glib;
use image::{DynamicImage, RgbaImage};

use crate::images::edit::adjust::{Adjustments, Histogram};

/// The quick copy's longest side: about a screen's worth.
const QUICK: u32 = 2048;

/// How much of the picture a request wants drawn.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Detail {
    /// Only the histogram.
    None,
    Quick,
    Full,
}

struct Job {
    generation: u64,
    adjust: Adjustments,
    detail: Detail,
    histogram: bool,
}

/// What came back for one request.
pub struct Toned {
    /// Width, height and straight-alpha RGBA.
    pub pixels: Option<(u32, u32, Vec<u8>)>,
    pub histogram: Option<Histogram>,
}

pub struct ToneWorker {
    jobs: mpsc::Sender<Job>,
    generation: Rc<Cell<u64>>,
    /// Whether the quick copy is the whole picture anyway.
    small: bool,
}

impl ToneWorker {
    /// Start on `image`, calling `done` with each answer that is still the
    /// newest when it arrives. Stops when dropped.
    pub fn start(image: DynamicImage, done: impl Fn(Toned) + 'static) -> Self {
        let small = image.width().max(image.height()) <= QUICK * 5 / 4;
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (sender, receiver) = async_channel::unbounded::<(u64, Toned)>();
        std::thread::spawn(move || {
            let full = image.into_rgba8();
            let longest = full.width().max(full.height());
            let quick = (!small).then(|| {
                let scale = f64::from(QUICK) / f64::from(longest);
                let width = ((f64::from(full.width()) * scale).round() as u32).max(1);
                let height = ((f64::from(full.height()) * scale).round() as u32).max(1);
                image::imageops::resize(&full, width, height, image::imageops::FilterType::Triangle)
            });
            let quick: &RgbaImage = quick.as_ref().unwrap_or(&full);
            while let Ok(mut job) = inbox.recv() {
                // Skip to the newest; what was asked before it is stale.
                while let Ok(newer) = inbox.try_recv() {
                    job = newer;
                }
                let histogram = job.histogram.then(|| job.adjust.histogram(quick));
                let source = match job.detail {
                    Detail::None => None,
                    Detail::Quick => Some(quick),
                    Detail::Full => Some(&full),
                };
                let pixels = source.map(|source| {
                    let mut toned = source.clone();
                    job.adjust.render(&mut toned, longest);
                    (toned.width(), toned.height(), toned.into_raw())
                });
                if sender.send_blocking((job.generation, Toned { pixels, histogram })).is_err() {
                    break;
                }
            }
        });
        let generation = Rc::new(Cell::new(0));
        let newest = generation.clone();
        glib::spawn_future_local(async move {
            while let Ok((generation, toned)) = receiver.recv().await {
                if generation == newest.get() {
                    done(toned);
                }
            }
        });
        ToneWorker { jobs, generation, small }
    }

    /// Ask for the picture with `adjust`, at `detail`, and for the histogram
    /// if `histogram`. Anything asked before is dropped.
    pub fn ask(&self, adjust: Adjustments, detail: Detail, histogram: bool) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let detail = if detail == Detail::Full && self.small { Detail::Quick } else { detail };
        let _ = self.jobs.send(Job { generation, adjust, detail, histogram });
    }

    /// Whether the quick answer is already the whole picture.
    pub fn is_small(&self) -> bool {
        self.small
    }
}
