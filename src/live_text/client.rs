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

use crate::live_text::install::{CutoutPaths, Paths};
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

/// Find the subject of one picture: start the helper with the subject model,
/// hand it the picture, read back one byte a pixel, 0 for background to 255
/// for the subject, and let it go, memory and all. Blocking: run it on a
/// thread of its own.
pub fn cut_out(program: &Path, paths: &CutoutPaths, width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let stopped = |what: &str| format!("The background remover stopped: {what}");
    let mut child = Command::new(program)
        .arg("--runtime")
        .arg(&paths.runtime)
        .arg("--cutout")
        .arg(&paths.model)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("Could not start the background remover: {e}"))?;
    let mut stdin = child.stdin.take().expect("piped");
    let mut stdout = BufReader::new(child.stdout.take().expect("piped"));
    let sent = stdin
        .write_all(protocol::cut_request(1, width, height).as_bytes())
        .and_then(|()| stdin.write_all(rgba))
        .and_then(|()| stdin.flush());
    let mut header = String::new();
    let answer = stdout.read_line(&mut header);
    // Closing its input lets it go once it has answered.
    drop(stdin);
    let result = match (sent, answer) {
        (_, Ok(n)) if n > 0 => match protocol::parse_reply(&header) {
            Some(Reply::Mask(1, w, h)) if (w, h) == (width, height) => {
                let mut mask = vec![0u8; width as usize * height as usize];
                stdout.read_exact(&mut mask).map(|()| mask).map_err(|e| stopped(&e.to_string()))
            }
            Some(Reply::Fail(_, reason)) => Err(stopped(&reason)),
            _ => Err(stopped("it gave an answer that makes no sense")),
        },
        (Err(e), _) => Err(stopped(&e.to_string())),
        _ => Err(stopped("it ended without answering")),
    };
    let status = child.wait();
    // Refused its options, so it is a helper from before it could do this:
    // in a build, `cargo run` makes Glance afresh but not its helper.
    if result.is_err() && status.as_ref().is_ok_and(|s| s.code() == Some(2)) {
        return Err("The background remover's helper is from an older Glance. Build both again with \
                    `cargo build --release`."
            .to_string());
    }
    // Stopped from outside while it worked: by the system, or a low-memory
    // guard such as earlyoom, for the memory it was using.
    use std::os::unix::process::ExitStatusExt;
    if result.is_err() && status.is_ok_and(|s| matches!(s.signal(), Some(9 | 15))) {
        return Err("The background remover was stopped, most likely because the computer ran short of memory. \
                    Close some programs and try again."
            .to_string());
    }
    result
}
