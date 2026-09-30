// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Drawing pages on a thread of their own.
//!
//! The thread owns its own Poppler document, since Poppler objects cannot be
//! handed between threads. The window does not queue requests one at a time:
//! it replaces the whole list with what it wants now, most urgent first. A fast
//! scroll through a long document therefore never leaves a backlog of pages
//! that have already gone past.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use super::document::{self, Pixels};
use super::layout::Rotation;

/// A page to draw, at how many device pixels per point, turned how far.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Job {
    pub page: usize,
    pub scale: f64,
    pub rotation: Rotation,
}

pub struct Rendered {
    pub page: usize,
    /// The scale that was asked for. The pixels may be at a lower one, if the
    /// request was over the size limit; the view compares against this.
    pub requested: f64,
    /// Drawn turned this far; stale once the view has been turned again.
    pub rotation: Rotation,
    pub pixels: Pixels,
}

#[derive(Default)]
struct Queue {
    jobs: Vec<Job>,
    quit: bool,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        // A panic mid-render cannot leave the queue half-written, so a
        // poisoned lock is still a usable one.
        self.queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The thread stops when this is dropped.
pub struct Renderer {
    shared: Arc<Shared>,
}

impl Renderer {
    pub fn start(uri: String, results: async_channel::Sender<Rendered>) -> Self {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("glance-pdf".into())
            .spawn(move || run(&uri, &worker, &results))
            .expect("the system refused to start a thread");
        Renderer { shared }
    }

    /// Replace whatever is still waiting with `jobs`, most urgent first.
    pub fn want(&self, jobs: Vec<Job>) {
        self.shared.lock().jobs = jobs;
        self.shared.wake.notify_one();
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        let mut queue = self.shared.lock();
        queue.quit = true;
        queue.jobs.clear();
        drop(queue);
        self.shared.wake.notify_one();
    }
}

fn run(uri: &str, shared: &Shared, results: &async_channel::Sender<Rendered>) {
    let Ok(document) = poppler::Document::from_file(uri, None) else {
        return; // The window already reported why when it opened the file.
    };
    while let Some(job) = next(shared) {
        let Some(pixels) = document::render_page(&document, job.page, job.scale, job.rotation) else {
            continue;
        };
        let rendered = Rendered { page: job.page, requested: job.scale, rotation: job.rotation, pixels };
        if results.send_blocking(rendered).is_err() {
            return; // Nobody is listening any more.
        }
    }
}

/// Wait for work. `None` means stop.
fn next(shared: &Shared) -> Option<Job> {
    let mut queue = shared.lock();
    loop {
        if queue.quit {
            return None;
        }
        if !queue.jobs.is_empty() {
            return Some(queue.jobs.remove(0));
        }
        queue = shared.wake.wait(queue).unwrap_or_else(|poisoned| poisoned.into_inner());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(page: usize) -> Job {
        Job { page, scale: 1.0, rotation: Rotation::default() }
    }

    #[test]
    fn new_wishes_replace_old_ones_instead_of_queueing_behind_them() {
        let shared = Shared::default();
        shared.lock().jobs = vec![job(1), job(2)];
        // The view has scrolled on: only page 9 matters now.
        shared.lock().jobs = vec![job(9)];
        assert_eq!(next(&shared), Some(job(9)));
    }

    #[test]
    fn stopping_wins_over_remaining_work() {
        let shared = Shared::default();
        {
            let mut queue = shared.lock();
            queue.jobs = vec![job(0)];
            queue.quit = true;
        }
        assert_eq!(next(&shared), None);
    }
}
