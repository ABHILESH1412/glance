// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Glance's end of the conversation with `glance-ocr`.
//!
//! The helper is started on the first request and stopped by dropping this:
//! its input closes, it exits, and every byte it used goes back to the
//! system. Pictures go to it from a thread of their own (a big one is tens of
//! megabytes down a pipe), and its answers come back on another, so the
//! window never waits on either.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;

use crate::live_text::install::Paths;
use crate::live_text::protocol::{self, Reply};

/// A picture to read: its id, size and RGBA pixels.
struct Job {
    id: u64,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

pub struct Helper {
    jobs: Option<mpsc::Sender<Job>>,
}

/// Where the helper is: beside Glance in a build, or in libexec once
/// installed. `GLANCE_OCR` names it outright, for trying another.
pub fn helper_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("GLANCE_OCR").map(PathBuf::from) {
        return path.is_file().then_some(path);
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [dir.join("glance-ocr"), dir.join("../libexec/glance/glance-ocr")]
        .into_iter()
        .find(|path| path.is_file())
}

impl Helper {
    /// Start the helper, calling `reply` (on the window's main loop) with
    /// each answer as it comes.
    pub fn start(program: &Path, paths: &Paths, reply: impl Fn(Reply) + 'static) -> std::io::Result<Helper> {
        let mut child = Command::new(program)
            .arg("--runtime")
            .arg(&paths.runtime)
            .arg("--detect")
            .arg(&paths.detect)
            .arg("--recognise")
            .arg(&paths.recognise)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");

        let (jobs, inbox) = mpsc::channel::<Job>();
        std::thread::spawn(move || write_jobs(stdin, inbox));

        let (sender, answers) = async_channel::unbounded::<Reply>();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(answer) = protocol::parse_reply(&line) {
                    if sender.send_blocking(answer).is_err() {
                        break;
                    }
                }
            }
        });
        gtk::glib::spawn_future_local(async move {
            while let Ok(answer) = answers.recv().await {
                reply(answer);
            }
            // Its answers have ended: it exited, or crashed.
            reply(Reply::Fail(u64::MAX, "the text reader stopped".into()));
        });
        // Reaped when it exits, so it never lingers as a zombie.
        std::thread::spawn(move || reap(child));
        Ok(Helper { jobs: Some(jobs) })
    }

    /// Ask for a picture to be read. Its answer comes to `reply` as `LINE`s
    /// then `DONE` (or `FAIL`) with this `id`.
    pub fn read(&self, id: u64, width: u32, height: u32, rgba: Vec<u8>) -> bool {
        self.jobs.as_ref().is_some_and(|jobs| jobs.send(Job { id, width, height, rgba }).is_ok())
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        // The writer thread ends and closes the helper's input; the helper
        // sees that and exits.
        self.jobs.take();
    }
}

fn write_jobs(mut stdin: ChildStdin, inbox: mpsc::Receiver<Job>) {
    while let Ok(job) = inbox.recv() {
        let header = protocol::request(job.id, job.width, job.height);
        if stdin.write_all(header.as_bytes()).and_then(|()| stdin.write_all(&job.rgba)).and_then(|()| stdin.flush()).is_err() {
            break;
        }
    }
}

fn reap(mut child: Child) {
    let _ = child.wait();
}
