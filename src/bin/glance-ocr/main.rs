// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! `glance-ocr`: reads the text in pictures for Glance's Live Text, and finds
//! their subject for its background remover.
//!
//! A program of its own so that Glance never holds ONNX Runtime or the
//! models: Glance starts this when someone asks for Live Text, hands it
//! pictures over standard input, and closes that when it is done, at which
//! point this exits and every byte it used goes back to the system.
//!
//! ```text
//! glance-ocr --runtime libonnxruntime.so --detect det.onnx --recognise rec.onnx
//! glance-ocr --runtime libonnxruntime.so --cutout birefnet.onnx
//! ```
//!
//! reads requests as described in `protocol.rs` until its input closes:
//! `READ` with the text models, `CUT` with the subject model. With `--image
//! FILE` instead it reads that one picture and prints what it found, or with
//! `--cutout` writes the picture without its background to `--out FILE.png`,
//! for trying it out by hand.

mod cutout;
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
    /// The text models, for `READ`.
    text: Option<(PathBuf, PathBuf)>,
    /// The subject model, for `CUT`.
    cutout: Option<PathBuf>,
    image: Option<PathBuf>,
    out: Option<PathBuf>,
    threads: usize,
}

fn options() -> Result<Options, String> {
    let mut args = std::env::args().skip(1);
    let (mut runtime, mut detect, mut recognise, mut cutout, mut image, mut out, mut threads) =
        (None, None, None, None, None, None, None);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--runtime" => runtime = Some(PathBuf::from(value()?)),
            "--detect" => detect = Some(PathBuf::from(value()?)),
            "--recognise" => recognise = Some(PathBuf::from(value()?)),
            "--cutout" => cutout = Some(PathBuf::from(value()?)),
            "--image" => image = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--threads" => threads = Some(value()?.parse().map_err(|_| "--threads needs a number")?),
            other => return Err(format!("unknown option {other}")),
        }
    }
    let text = match (detect, recognise) {
        (Some(detect), Some(recognise)) => Some((detect, recognise)),
        (None, None) => None,
        _ => return Err("--detect and --recognise go together".to_string()),
    };
    if text.is_none() && cutout.is_none() {
        return Err("--detect and --recognise, or --cutout, are needed".to_string());
    }
    // Half the cores: the rest keep the desktop and Glance itself
    // responsive meanwhile.
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    Ok(Options {
        runtime: runtime.ok_or("--runtime is needed")?,
        text,
        cutout,
        image,
        out,
        threads: threads.unwrap_or((cores / 2).max(1)),
    })
}

/// Whatever models this run was given.
struct Engines {
    text: Option<engine::Engine>,
    cutout: Option<cutout::Cutout>,
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
    let loading = std::time::Instant::now();
    let loaded = (|| -> Result<Engines, String> {
        let text = match &options.text {
            Some((detect, recognise)) => Some(engine::Engine::load(detect, recognise, options.threads)?),
            None => None,
        };
        let cutout = match &options.cutout {
            Some(model) => Some(cutout::Cutout::load(model, options.threads)?),
            None => None,
        };
        Ok(Engines { text, cutout })
    })();
    let mut engines = match loaded {
        Ok(engines) => engines,
        Err(message) => {
            eprintln!("glance-ocr: {message}");
            return ExitCode::from(3);
        }
    };

    if options.image.is_some() {
        eprintln!("glance-ocr: models loaded in {:.2} s", loading.elapsed().as_secs_f32());
    }
    if let (Some(path), Some(cutout)) = (&options.image, engines.cutout.as_mut()) {
        return cut_one(cutout, path, options.out.as_deref());
    }
    if let (Some(path), Some(engine)) = (options.image, engines.text.as_mut()) {
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

    serve(&mut engines)
}

/// Take the background off one picture and write it out, saying how long
/// it took.
fn cut_one(cutout: &mut cutout::Cutout, path: &std::path::Path, out: Option<&std::path::Path>) -> ExitCode {
    eprintln!("glance-ocr: {}", cutout.describe());
    let picture = match image::open(path) {
        Ok(picture) => picture.to_rgba8(),
        Err(e) => {
            eprintln!("glance-ocr: cannot open {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let (width, height) = picture.dimensions();
    let started = std::time::Instant::now();
    let mask = match cutout.mask(picture.as_raw(), width as usize, height as usize) {
        Ok(mask) => mask,
        Err(message) => {
            eprintln!("glance-ocr: {message}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("glance-ocr: {width} × {height} cut out in {:.2} s", started.elapsed().as_secs_f32());
    let Some(out) = out else { return ExitCode::SUCCESS };
    let mut cut = picture;
    for (pixel, &alpha) in cut.pixels_mut().zip(&mask) {
        pixel[3] = ((u16::from(pixel[3]) * u16::from(alpha)) / 255) as u8;
    }
    match cut.save(out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("glance-ocr: cannot write {}: {e}", out.display());
            ExitCode::FAILURE
        }
    }
}

/// Answer requests until Glance closes our input.
fn serve(engines: &mut Engines) -> ExitCode {
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
        let Some((ask, id, width, height)) = protocol::parse_request(header.trim()) else {
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
        let reply: Vec<u8> = match (ask, engines.text.as_mut(), engines.cutout.as_mut()) {
            (protocol::Ask::Read, Some(engine), _) => match engine.read(&pixels, width as usize, height as usize) {
                Ok(lines) => {
                    let mut reply: String = lines.iter().map(protocol::Line::message).collect();
                    reply.push_str(&protocol::done(id));
                    reply.into_bytes()
                }
                Err(message) => protocol::fail(id, &message).into_bytes(),
            },
            (protocol::Ask::Cut, _, Some(cutout)) => match cutout.mask(&pixels, width as usize, height as usize) {
                Ok(mask) => {
                    let mut reply = protocol::mask(id, width, height).into_bytes();
                    reply.extend_from_slice(&mask);
                    reply
                }
                Err(message) => protocol::fail(id, &message).into_bytes(),
            },
            (protocol::Ask::Read, None, _) => protocol::fail(id, "started without the text models").into_bytes(),
            (protocol::Ask::Cut, _, None) => protocol::fail(id, "started without the subject model").into_bytes(),
        };
        // The pixels are done with before the next picture comes.
        drop(pixels);
        if output.write_all(&reply).and_then(|()| output.flush()).is_err() {
            return ExitCode::FAILURE;
        }
    }
}
