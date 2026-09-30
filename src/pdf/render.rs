// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Drawing pages on a thread of their own.
//!
//! The thread owns its own Poppler document, since Poppler objects cannot be
//! handed between threads. The window does not queue requests one at a time:
//! it replaces the whole list with what it wants now, most urgent first. A fast
//! scroll through a long document therefore never leaves a backlog of pages
//! that have already gone past.
//!
//! When the file changes under it — a page marked up — the thread is told to
//! reopen it. Every page it draws says which version of the file it was drawn
//! from, so one already under way when the file changed is recognised as out
//! of date and thrown away.

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
    /// Drawn from this version of the file; see `Renderer::reload`.
    pub revision: u64,
    pub pixels: Pixels,
}

#[derive(Default)]
struct Queue {
    jobs: Vec<Job>,
    /// Reopen the file before drawing anything more, as this version.
    reload: Option<u64>,
    quit: bool,
}

#[derive(Debug, PartialEq)]
enum Work {
    Reload(u64),
    Draw(Job),
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
    /// Start drawing from the file as it is now, calling that `revision`.
    pub fn start(uri: String, revision: u64, results: async_channel::Sender<Rendered>) -> Self {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("glance-pdf".into())
            .spawn(move || run(&uri, revision, &worker, &results))
            .expect("the system refused to start a thread");
        Renderer { shared }
    }

    /// Replace whatever is still waiting with `jobs`, most urgent first.
    pub fn want(&self, jobs: Vec<Job>) {
        self.shared.lock().jobs = jobs;
        self.shared.wake.notify_one();
    }

    /// The file has changed: reopen it before drawing anything else, and
    /// label what is drawn from now on with `revision`.
    pub fn reload(&self, revision: u64) {
        self.shared.lock().reload = Some(revision);
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

fn run(uri: &str, revision: u64, shared: &Shared, results: &async_channel::Sender<Rendered>) {
    let Ok(mut document) = poppler::Document::from_file(uri, None) else {
        return; // The window already reported why when it opened the file.
    };
    let mut revision = revision;
    while let Some(work) = next(shared) {
        let job = match work {
            Work::Reload(now) => {
                // Should the file have gone, keep drawing what is still open.
                if let Ok(reopened) = poppler::Document::from_file(uri, None) {
                    document = reopened;
                }
                revision = now;
                continue;
            }
            Work::Draw(job) => job,
        };
        let Some(pixels) = document::render_page(&document, job.page, job.scale, job.rotation) else {
            continue;
        };
        let rendered = Rendered { page: job.page, requested: job.scale, rotation: job.rotation, revision, pixels };
        if results.send_blocking(rendered).is_err() {
            return; // Nobody is listening any more.
        }
    }
}

/// Wait for work. `None` means stop. A reload comes before any drawing, so
/// nothing asked for after the file changed is drawn from the old one.
fn next(shared: &Shared) -> Option<Work> {
    let mut queue = shared.lock();
    loop {
        if queue.quit {
            return None;
        }
        if let Some(revision) = queue.reload.take() {
            return Some(Work::Reload(revision));
        }
        if !queue.jobs.is_empty() {
            return Some(Work::Draw(queue.jobs.remove(0)));
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
        assert_eq!(next(&shared), Some(Work::Draw(job(9))));
    }

    #[test]
    fn a_changed_file_is_reopened_before_anything_more_is_drawn() {
        let shared = Shared::default();
        shared.lock().jobs = vec![job(3)];
        shared.lock().reload = Some(1);
        assert_eq!(next(&shared), Some(Work::Reload(1)));
        assert_eq!(next(&shared), Some(Work::Draw(job(3))));
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
