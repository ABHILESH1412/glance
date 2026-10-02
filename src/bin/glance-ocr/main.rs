// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! `glance-ocr`: reads the text in pictures for Glance's Live Text.
//!
//! A program of its own so that Glance never holds ONNX Runtime or the
//! models: Glance starts this when someone asks for Live Text, hands it
//! pictures over standard input, and closes that when it is done, at which
//! point this exits and every byte it used goes back to the system.
//!
//! ```text
//! glance-ocr --runtime libonnxruntime.so --detect det.onnx --recognise rec.onnx
//! ```
//!
//! reads requests as described in `protocol.rs` until its input closes.
//! With `--image FILE` instead it reads that one picture and prints what it
//! found, for trying it out by hand.

mod engine;
mod geometry;
// Glance's half of the conversation goes unused on this side.
#[allow(dead_code)]
#[path = "../../live_text/protocol.rs"]
mod protocol;

use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

struct Options {
    runtime: PathBuf,
    detect: PathBuf,
    recognise: PathBuf,
    image: Option<PathBuf>,
    threads: usize,
}

fn options() -> Result<Options, String> {
    let mut args = std::env::args().skip(1);
    let (mut runtime, mut detect, mut recognise, mut image, mut threads) = (None, None, None, None, None);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--runtime" => runtime = Some(PathBuf::from(value()?)),
            "--detect" => detect = Some(PathBuf::from(value()?)),
            "--recognise" => recognise = Some(PathBuf::from(value()?)),
            "--image" => image = Some(PathBuf::from(value()?)),
            "--threads" => threads = Some(value()?.parse().map_err(|_| "--threads needs a number")?),
            other => return Err(format!("unknown option {other}")),
        }
    }
    // Half the cores: reading is quick either way, and the rest keep the
    // desktop and Glance itself responsive meanwhile.
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    Ok(Options {
        runtime: runtime.ok_or("--runtime is needed")?,
        detect: detect.ok_or("--detect is needed")?,
        recognise: recognise.ok_or("--recognise is needed")?,
        image,
        threads: threads.unwrap_or((cores / 2).max(1)),
    })
}

fn main() -> ExitCode {
    let options = match options() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("glance-ocr: {message}");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = ort::init_from(&options.runtime).map(|builder| builder.commit()) {
        eprintln!("glance-ocr: cannot load ONNX Runtime: {e}");
        return ExitCode::from(3);
    }
    let mut engine = match engine::Engine::load(&options.detect, &options.recognise, options.threads) {
        Ok(engine) => engine,
        Err(message) => {
            eprintln!("glance-ocr: {message}");
            return ExitCode::from(3);
        }
    };

    if let Some(path) = options.image {
        return match image::open(&path) {
            Ok(picture) => {
                let rgba = picture.to_rgba8();
                let started = std::time::Instant::now();
                match engine.read(rgba.as_raw(), rgba.width() as usize, rgba.height() as usize) {
                    Ok(lines) => {
                        for line in lines {
                            print!("{}", line.message());
                        }
                        eprintln!("glance-ocr: read in {:.2} s", started.elapsed().as_secs_f32());
                        ExitCode::SUCCESS
                    }
                    Err(message) => {
                        eprintln!("glance-ocr: {message}");
                        ExitCode::FAILURE
                    }
                }
            }
            Err(e) => {
                eprintln!("glance-ocr: cannot open {}: {e}", path.display());
                ExitCode::FAILURE
            }
        };
    }

    serve(&mut engine)
}

/// Answer requests until Glance closes our input.
fn serve(engine: &mut engine::Engine) -> ExitCode {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let mut header = String::new();
    loop {
        header.clear();
        match input.read_line(&mut header) {
            // Glance is done with us. Straight out: tidying ONNX Runtime
            // away takes seconds, and the system takes all of it back anyway.
            Ok(0) => std::process::exit(0),
            Ok(_) => {}
            Err(_) => return ExitCode::FAILURE,
        }
        let Some((id, width, height)) = protocol::parse_request(header.trim()) else {
            // Nothing can be trusted after a request that makes no sense:
            // the pixels that follow it are of no known length.
            let _ = output.write_all(protocol::fail(0, "unreadable request").as_bytes());
            return ExitCode::FAILURE;
        };
        let length = width as usize * height as usize * 4;
        let mut pixels = vec![0u8; length];
        if input.read_exact(&mut pixels).is_err() {
            return ExitCode::FAILURE;
        }
        let reply = match engine.read(&pixels, width as usize, height as usize) {
            Ok(lines) => {
                let mut reply: String = lines.iter().map(protocol::Line::message).collect();
                reply.push_str(&protocol::done(id));
                reply
            }
            Err(message) => protocol::fail(id, &message),
        };
        // The pixels are done with before the next picture comes.
        drop(pixels);
        if output.write_all(reply.as_bytes()).and_then(|()| output.flush()).is_err() {
            return ExitCode::FAILURE;
        }
    }
}
