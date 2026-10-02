// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Getting Live Text's parts onto the computer, the first time it is used:
//! ONNX Runtime, which runs the models, and the two PP-OCRv6 models
//! themselves. About 41 MB, downloaded once, into
//! `~/.local/share/glance/ocr`, and kept.
//!
//! Every file is checked against a SHA-256 fixed here, in the source, before
//! it is kept: a download that was cut short, corrupted, or swapped for
//! something else on the way is thrown away, never run. Files are written
//! beside their final name and renamed into place only once checked, so a
//! cancelled or failed download leaves nothing half-done behind.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use gtk::{gio, glib};
use gtk::prelude::*;
use soup::prelude::*;

/// Where it comes from, and what it must be.
pub struct Part {
    /// For the person: what this is.
    pub name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    /// The download's size in bytes, for the progress bar and the question.
    pub size: u64,
    /// What it is kept as, in the Live Text folder.
    pub file: &'static str,
    /// For an archive, the one file inside it that is wanted.
    pub member: Option<&'static str>,
}

const RUNTIME_X86_64: Part = Part {
    name: "Text engine (ONNX Runtime 1.30)",
    url: "https://github.com/microsoft/onnxruntime/releases/download/v1.30.0/onnxruntime-linux-x64-1.30.0.tgz",
    sha256: "a5ed5a3cac51fbb2e90da632ae43d19212faaa20e76484e62bcb7c23ddb3b3fd",
    size: 11_306_877,
    file: "libonnxruntime.so.1.30.0",
    member: Some("lib/libonnxruntime.so.1.30.0"),
};

const RUNTIME_AARCH64: Part = Part {
    name: "Text engine (ONNX Runtime 1.30)",
    url: "https://github.com/microsoft/onnxruntime/releases/download/v1.30.0/onnxruntime-linux-aarch64-1.30.0.tgz",
    sha256: "e16a27a8ed330bbc698df7330b0cf56e722f354e3bcc92118682c74ef3c3e3da",
    size: 10_269_495,
    file: "libonnxruntime.so.1.30.0",
    member: Some("lib/libonnxruntime.so.1.30.0"),
};

pub const DETECT: Part = Part {
    name: "Finding text (PP-OCRv6)",
    url: "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv6/det/PP-OCRv6_det_small.onnx",
    sha256: "090f04abcd9d9a7498bc4ebf677e4cb9bdce1fe4197ddb7e529f1ef44e1ff94f",
    size: 9_929_594,
    file: "PP-OCRv6_det_small.onnx",
    member: None,
};

pub const RECOGNISE: Part = Part {
    name: "Reading text: English and the European languages (PP-OCRv6)",
    url: "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv6/rec/PP-OCRv6_rec_small.onnx",
    sha256: "6f327246b50388f3c176ae304bd95767ea6dc0c9ae92153ef8cbe210b3c14884",
    size: 21_234_383,
    file: "PP-OCRv6_rec_small.onnx",
    member: None,
};

/// The runtime for this computer, if ONNX Runtime is published for it.
pub fn runtime() -> Option<&'static Part> {
    match std::env::consts::ARCH {
        "x86_64" => Some(&RUNTIME_X86_64),
        "aarch64" => Some(&RUNTIME_AARCH64),
        _ => None,
    }
}

/// Everything Live Text needs, for this computer.
pub fn parts() -> Vec<&'static Part> {
    runtime().into_iter().chain([&DETECT, &RECOGNISE]).collect()
}

/// The Live Text folder.
pub fn folder() -> PathBuf {
    glib::user_data_dir().join("glance").join("ocr")
}

/// The files the helper is started with, once everything is in place.
#[derive(Clone, Debug, PartialEq)]
pub struct Paths {
    pub runtime: PathBuf,
    pub detect: PathBuf,
    pub recognise: PathBuf,
}

/// The record of what was downloaded and checked: one line per part, its
/// file and the SHA-256 it was checked against. A part counts as there only
/// if its line matches what this version of Glance wants.
const RECORD: &str = "contents";

fn record_line(part: &Part) -> String {
    format!("{} {}", part.file, part.sha256)
}

fn recorded(folder: &Path) -> Vec<String> {
    std::fs::read_to_string(folder.join(RECORD))
        .map(|text| text.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

fn is_there(folder: &Path, part: &Part) -> bool {
    recorded(folder).contains(&record_line(part)) && folder.join(part.file).is_file()
}

/// What still has to be downloaded.
pub fn missing() -> Vec<&'static Part> {
    let folder = folder();
    parts().into_iter().filter(|part| !is_there(&folder, part)).collect()
}

/// The files, if everything is there.
pub fn installed() -> Option<Paths> {
    let folder = folder();
    let runtime = runtime()?;
    [runtime, &DETECT, &RECOGNISE].iter().all(|part| is_there(&folder, part)).then(|| Paths {
        runtime: folder.join(runtime.file),
        detect: folder.join(DETECT.file),
        recognise: folder.join(RECOGNISE.file),
    })
}

/// How much room a part takes once kept, if it is there.
pub fn kept_size(part: &Part) -> Option<u64> {
    let folder = folder();
    is_there(&folder, part).then(|| std::fs::metadata(folder.join(part.file)).map(|m| m.len()).ok()).flatten()
}

/// Take it all away again: everything in the Live Text folder.
pub fn remove() -> std::io::Result<()> {
    match std::fs::remove_dir_all(folder()) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// How the download is getting on: bytes so far, and in all.
pub type Progress = Box<dyn Fn(u64, u64) + Send>;

/// Download, check and keep each of `parts`, telling `progress` as it goes.
/// Blocking: run it on a thread. `cancel` stops it between any two reads.
pub fn download(parts: &[&Part], progress: Progress, cancel: &gio::Cancellable) -> Result<(), String> {
    let folder = folder();
    std::fs::create_dir_all(&folder).map_err(|e| format!("Could not make {}: {e}", folder.display()))?;
    let total: u64 = parts.iter().map(|part| part.size).sum();
    let mut before = 0;
    // One connection for all of them; it is made here because a session
    // belongs to the thread it is made on.
    let session = soup::Session::new();
    session.set_user_agent(&format!("Glance/{} ", env!("CARGO_PKG_VERSION")));
    for part in parts {
        let fetched = fetch(&session, part, &folder, &|done| progress(before + done, total), cancel)?;
        keep(part, &fetched, &folder)?;
        before += part.size;
    }
    progress(total, total);
    Ok(())
}

/// Download one part beside its final name, checking it as it comes.
fn fetch(
    session: &soup::Session,
    part: &Part,
    folder: &Path,
    progress: &dyn Fn(u64),
    cancel: &gio::Cancellable,
) -> Result<PathBuf, String> {
    let failed = |what: &str| format!("Could not download “{}”: {what}", part.name);
    let message = soup::Message::new("GET", part.url).map_err(|e| failed(&e.to_string()))?;
    let stream = session.send(&message, Some(cancel)).map_err(|e| failed(&e.to_string()))?;
    if message.status() != soup::Status::Ok {
        return Err(failed(&format!("the server answered {:?}", message.status())));
    }
    let partial = folder.join(format!("{}.download", part.file));
    let mut file = std::fs::File::create(&partial).map_err(|e| failed(&e.to_string()))?;
    let mut checksum = glib::Checksum::new(glib::ChecksumType::Sha256).ok_or_else(|| failed("no SHA-256"))?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    let outcome = loop {
        let read = match stream.read(&mut buffer, Some(cancel)) {
            Ok(0) => break Ok(()),
            Ok(read) => read,
            Err(e) if e.matches(gio::IOErrorEnum::Cancelled) => break Err("Cancelled.".to_string()),
            Err(e) => break Err(failed(&e.to_string())),
        };
        checksum.update(&buffer[..read]);
        if let Err(e) = file.write_all(&buffer[..read]) {
            break Err(failed(&e.to_string()));
        }
        done += read as u64;
        // Never more than promised: a server sending far more than the
        // file should be is not sending the file.
        if done > part.size {
            break Err(failed("it was larger than it should be"));
        }
        progress(done);
    };
    let checked = outcome.and_then(|()| {
        let sum = checksum.string().unwrap_or_default();
        if done != part.size || sum != part.sha256 {
            Err(failed("it did not arrive intact. Try again later"))
        } else {
            Ok(())
        }
    });
    drop(file);
    match checked {
        Ok(()) => Ok(partial),
        Err(e) => {
            let _ = std::fs::remove_file(&partial);
            Err(e)
        }
    }
}

/// Put a checked download in place, unpacking it first if it is an archive,
/// and note it in the record.
fn keep(part: &Part, fetched: &Path, folder: &Path) -> Result<(), String> {
    let failed = |what: String| format!("Could not keep “{}”: {what}", part.name);
    let target = folder.join(part.file);
    let staged = folder.join(format!("{}.new", part.file));
    let result = match part.member {
        Some(member) => {
            let archive = std::fs::File::open(fetched).map_err(|e| failed(e.to_string()))?;
            let mut out = std::fs::File::create(&staged).map_err(|e| failed(e.to_string()))?;
            extract(flate2::read::GzDecoder::new(archive), member, &mut out).map_err(failed)
        }
        None => std::fs::rename(fetched, &staged).map_err(|e| failed(e.to_string())),
    };
    let _ = std::fs::remove_file(fetched);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }
    std::fs::rename(&staged, &target).map_err(|e| failed(e.to_string()))?;
    // The record last, so it never mentions a file that is not there.
    let mut lines: Vec<String> = recorded(folder).into_iter().filter(|line| !line.starts_with(&format!("{} ", part.file))).collect();
    lines.push(record_line(part));
    std::fs::write(folder.join(RECORD), lines.join("\n") + "\n").map_err(|e| failed(e.to_string()))
}

/// Copy the one file in a tar stream whose path ends with `member` into
/// `out`. Tar is 512-byte blocks: a header naming the file and giving its
/// size in octal, then its contents padded to a whole block.
fn extract(mut tar: impl Read, member: &str, out: &mut impl Write) -> Result<(), String> {
    let mut header = [0u8; 512];
    loop {
        if tar.read_exact(&mut header).is_err() {
            return Err(format!("{member} is not in the archive"));
        }
        if header.iter().all(|&b| b == 0) {
            return Err(format!("{member} is not in the archive"));
        }
        let text = |range: std::ops::Range<usize>| {
            let bytes = &header[range];
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            String::from_utf8_lossy(&bytes[..end]).trim().to_string()
        };
        let name = text(0..100);
        // The ustar format keeps a longer path's start separately.
        let prefix = if &header[257..262] == b"ustar" { text(345..500) } else { String::new() };
        let path = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let size = u64::from_str_radix(&text(124..136), 8).map_err(|_| "the archive is damaged".to_string())?;
        let kind = header[156];
        let padded = size.div_ceil(512) * 512;
        if (kind == b'0' || kind == 0) && path.ends_with(member) {
            let copied = std::io::copy(&mut (&mut tar).take(size), out).map_err(|e| e.to_string())?;
            return if copied == size { Ok(()) } else { Err("the archive ended early".to_string()) };
        }
        std::io::copy(&mut (&mut tar).take(padded), &mut std::io::sink()).map_err(|e| e.to_string())?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tar with these files, as `tar` writes them.
    fn tar(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (path, contents) in files {
            let mut header = [0u8; 512];
            header[..path.len()].copy_from_slice(path.as_bytes());
            header[124..135].copy_from_slice(format!("{:011o}", contents.len()).as_bytes());
            header[156] = b'0';
            header[257..262].copy_from_slice(b"ustar");
            out.extend_from_slice(&header);
            out.extend_from_slice(contents);
            out.resize(out.len().div_ceil(512) * 512, 0);
        }
        out.resize(out.len() + 1024, 0);
        out
    }

    #[test]
    fn the_one_wanted_file_comes_out_of_the_archive() {
        let archive = tar(&[
            ("onnxruntime-linux-x64-1.30.0/LICENSE", b"MIT"),
            ("onnxruntime-linux-x64-1.30.0/lib/libonnxruntime.so.1.30.0", &[7u8; 1300]),
            ("onnxruntime-linux-x64-1.30.0/lib/other", b"x"),
        ]);
        let mut out = Vec::new();
        extract(archive.as_slice(), "lib/libonnxruntime.so.1.30.0", &mut out).unwrap();
        assert_eq!(out, vec![7u8; 1300]);
        let mut nothing = Vec::new();
        assert!(extract(archive.as_slice(), "lib/missing.so", &mut nothing).is_err());
        assert!(extract(&archive[..700], "lib/libonnxruntime.so.1.30.0", &mut nothing).is_err(), "cut short");
    }

    #[test]
    fn a_part_counts_only_when_recorded_with_the_checksum_wanted() {
        let folder = std::env::temp_dir().join(format!("glance-live-text-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        assert!(!is_there(&folder, &DETECT));
        std::fs::write(folder.join(DETECT.file), b"model").unwrap();
        assert!(!is_there(&folder, &DETECT), "present but never checked");
        std::fs::write(folder.join(RECORD), format!("{} 0000\n", DETECT.file)).unwrap();
        assert!(!is_there(&folder, &DETECT), "checked against another version");
        std::fs::write(folder.join(RECORD), record_line(&DETECT) + "\n").unwrap();
        assert!(is_there(&folder, &DETECT));
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn every_part_is_pinned() {
        for part in [&RUNTIME_X86_64, &RUNTIME_AARCH64, &DETECT, &RECOGNISE] {
            assert!(part.url.starts_with("https://"), "{}", part.name);
            assert_eq!(part.sha256.len(), 64);
            assert!(part.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
            assert!(part.size > 1_000_000);
        }
        let total: u64 = parts().iter().map(|part| part.size).sum();
        assert!((40_000_000..45_000_000).contains(&total), "about 41 MB: {total}");
    }
}
