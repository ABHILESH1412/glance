// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the camera wrote into a photo: EXIF.
//!
//! EXIF is a TIFF directory tucked inside the file: in a JPEG's APP1 block, a
//! PNG's eXIf chunk, a WebP's EXIF chunk, a HEIF's metadata item, or, for TIFF
//! and most camera raw files, the file itself. Raw formats that are not TIFF
//! underneath (Canon's CR3, Fujifilm's RAF) are read through rawler, which
//! knows them.
//!
//! Only what a person would want to see is kept, already in words.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::images::format::{self, Format};

/// How much of a file to look through for the EXIF block of a JPEG, WebP or
/// TIFF. EXIF is at most 64 KB, and comes before the pixels.
const HEAD: u64 = 1024 * 1024;

#[derive(Default, Debug, Clone, PartialEq)]
pub struct Location {
    /// Degrees, north positive.
    pub latitude: f64,
    /// Degrees, east positive.
    pub longitude: f64,
    /// Metres above sea level.
    pub altitude: Option<f64>,
}

/// The camera's account of a photo. Everything is optional: phones,
/// scanners and editors each write a different subset, and many write none.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct Camera {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    /// Seconds, as the fraction the camera gave.
    pub exposure: Option<(u32, u32)>,
    pub f_number: Option<f64>,
    pub iso: Option<u32>,
    /// Millimetres.
    pub focal_length: Option<f64>,
    pub focal_length_35mm: Option<u32>,
    /// Stops.
    pub exposure_bias: Option<f64>,
    pub flash: Option<u16>,
    pub white_balance: Option<u16>,
    pub metering: Option<u16>,
    pub program: Option<u16>,
    /// As written: "YYYY:MM:DD HH:MM:SS".
    pub taken: Option<String>,
    /// From UTC, as written: "+02:00".
    pub offset: Option<String>,
    pub software: Option<String>,
    pub artist: Option<String>,
    pub copyright: Option<String>,
    pub location: Option<Location>,
}

impl Camera {
    pub fn is_empty(&self) -> bool {
        self.rows().is_empty() && self.location.is_none()
    }

    /// The camera, the lens and how the shot was taken, as label and value,
    /// in the order a photographer reads them.
    pub fn rows(&self) -> Vec<(&'static str, String)> {
        let mut rows = Vec::new();
        if let Some(camera) = self.camera_name() {
            rows.push(("Camera", camera));
        }
        if let Some(lens) = &self.lens {
            rows.push(("Lens", lens.clone()));
        }
        if let Some(taken) = &self.taken {
            rows.push(("Taken", describe_date(taken, self.offset.as_deref())));
        }
        if let Some(exposure) = self.exposure.and_then(describe_exposure) {
            rows.push(("Shutter speed", exposure));
        }
        if let Some(f) = self.f_number.filter(|f| *f > 0.0) {
            rows.push(("Aperture", format!("ƒ/{}", trim(f, 1))));
        }
        if let Some(iso) = self.iso.filter(|iso| *iso > 0) {
            rows.push(("ISO", iso.to_string()));
        }
        if let Some(focal) = self.focal_length.filter(|f| *f > 0.0) {
            let mut text = format!("{} mm", trim(focal, 1));
            if let Some(full) = self.focal_length_35mm.filter(|f| *f > 0 && (f64::from(*f) - focal).abs() >= 1.0) {
                text.push_str(&format!(" ({full} mm on 35 mm film)"));
            }
            rows.push(("Focal length", text));
        }
        if let Some(bias) = self.exposure_bias.filter(|b| b.abs() >= 0.05) {
            rows.push(("Exposure compensation", format!("{}{} EV", if bias > 0.0 { "+" } else { "−" }, trim(bias.abs(), 1))));
        }
        if let Some(program) = self.program.and_then(describe_program) {
            rows.push(("Mode", program.into()));
        }
        if let Some(metering) = self.metering.and_then(describe_metering) {
            rows.push(("Metering", metering.into()));
        }
        if let Some(flash) = self.flash {
            rows.push(("Flash", describe_flash(flash).into()));
        }
        if let Some(balance) = self.white_balance {
            rows.push(("White balance", if balance == 0 { "Automatic" } else { "Manual" }.into()));
        }
        for (label, value) in [("Software", &self.software), ("Artist", &self.artist), ("Copyright", &self.copyright)] {
            if let Some(value) = value {
                rows.push((label, value.clone()));
            }
        }
        rows
    }

    /// "Canon EOS R6", not "Canon Canon EOS R6": many cameras repeat the
    /// maker in the model.
    fn camera_name(&self) -> Option<String> {
        match (&self.make, &self.model) {
            (Some(make), Some(model)) => {
                let first = make.split_whitespace().next().unwrap_or(make);
                if model.to_lowercase().starts_with(&first.to_lowercase()) {
                    Some(model.clone())
                } else {
                    Some(format!("{make} {model}"))
                }
            }
            (None, Some(model)) => Some(model.clone()),
            (Some(make), None) => Some(make.clone()),
            (None, None) => None,
        }
    }
}

impl Location {
    /// "48.85837° N, 2.29448° E"
    pub fn coordinates(&self) -> String {
        format!(
            "{:.5}° {}, {:.5}° {}",
            self.latitude.abs(),
            if self.latitude >= 0.0 { "N" } else { "S" },
            self.longitude.abs(),
            if self.longitude >= 0.0 { "E" } else { "W" },
        )
    }
}

/// A number without the decimals it does not need: 8, 2.8, 0.3.
fn trim(value: f64, places: usize) -> String {
    let text = format!("{value:.places$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    }
}

fn describe_exposure((top, bottom): (u32, u32)) -> Option<String> {
    if top == 0 || bottom == 0 {
        return None;
    }
    let seconds = f64::from(top) / f64::from(bottom);
    Some(if seconds >= 0.3 {
        format!("{} s", trim(seconds, 1))
    } else {
        // 1/250 s, however the camera wrote the fraction (10/2500 too).
        format!("1/{} s", (1.0 / seconds).round())
    })
}

fn describe_program(program: u16) -> Option<&'static str> {
    Some(match program {
        1 => "Manual",
        2 => "Program",
        3 => "Aperture priority",
        4 => "Shutter priority",
        5 => "Creative",
        6 => "Action",
        7 => "Portrait",
        8 => "Landscape",
        _ => return None,
    })
}

fn describe_metering(metering: u16) -> Option<&'static str> {
    Some(match metering {
        1 => "Average",
        2 => "Centre-weighted",
        3 => "Spot",
        4 => "Multi-spot",
        5 => "Pattern",
        6 => "Partial",
        _ => return None,
    })
}

fn describe_flash(flash: u16) -> &'static str {
    // Bit 5: the camera has no flash. Bit 0: it fired.
    if flash & 0x20 != 0 {
        "None"
    } else if flash & 0x01 != 0 {
        "Fired"
    } else {
        "Did not fire"
    }
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// "2024:05:01 14:03:22" as "1 May 2024, 14:03:22", with its offset from UTC
/// when the camera gave one.
fn describe_date(written: &str, offset: Option<&str>) -> String {
    let parts: Vec<&str> = written.split([':', ' ']).collect();
    let described = match parts.as_slice() {
        [year, month, day, hour, minute, second, ..] => {
            let month = month.parse::<usize>().ok().filter(|m| (1..=12).contains(m));
            let day = day.parse::<u32>().ok().filter(|d| (1..=31).contains(d));
            match (month, day) {
                (Some(month), Some(day)) => {
                    Some(format!("{day} {} {year}, {hour}:{minute}:{second}", MONTHS[month - 1]))
                }
                _ => None,
            }
        }
        _ => None,
    };
    let mut text = described.unwrap_or_else(|| written.to_string());
    if let Some(offset) = offset.filter(|o| !o.trim().is_empty()) {
        text.push_str(&format!(" (UTC{})", offset.trim()));
    }
    text
}

/// The camera's account of the picture at `path`, if it gave one.
pub fn read(path: &Path) -> Option<Camera> {
    let found = match format::detect(path) {
        Format::Heif { .. } => heif(path).and_then(|block| parse(&block)),
        Format::Raw => {
            // Most raw files are TIFF underneath; the others rawler reads.
            head(path).and_then(|bytes| parse(&bytes)).filter(|camera| !camera.is_empty()).or_else(|| raw(path))
        }
        Format::Svg | Format::Jpeg2000 | Format::Psd | Format::Icns | Format::Illustrator => None,
        Format::Raster => {
            let bytes = head(path)?;
            if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                png(path).and_then(|block| parse(&block))
            } else {
                block(&bytes).and_then(parse)
            }
        }
    };
    found.filter(|camera| !camera.is_empty())
}

fn head(path: &Path) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(HEAD).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// The TIFF structure inside a JPEG, WebP or TIFF.
fn block(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let mut at = 2;
        while at + 4 <= bytes.len() && bytes[at] == 0xFF {
            let marker = bytes[at + 1];
            if marker == 0xDA {
                break;
            }
            let length = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
            let body = bytes.get(at + 4..at + 2 + length)?;
            if marker == 0xE1 && body.starts_with(b"Exif\0\0") {
                return Some(&body[6..]);
            }
            at += 2 + length;
        }
        None
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        let mut at = 12;
        while at + 8 <= bytes.len() {
            let kind = &bytes[at..at + 4];
            let length = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
            if kind == b"EXIF" {
                let body = bytes.get(at + 8..at + 8 + length)?;
                // Some writers keep JPEG's prefix, some do not.
                return Some(body.strip_prefix(b"Exif\0\0").unwrap_or(body));
            }
            // Chunks are padded to an even length.
            at += 8 + length + (length & 1);
        }
        None
    } else if bytes.starts_with(b"II") || bytes.starts_with(b"MM") {
        Some(bytes)
    } else {
        None
    }
}

/// A PNG's eXIf chunk, walked to rather than read through: it may come after
/// the pixels.
fn png(path: &Path) -> Option<Vec<u8>> {
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(8)).ok()?;
    let mut header = [0u8; 8];
    while file.read_exact(&mut header).is_ok() {
        let length = u32::from_be_bytes(header[0..4].try_into().ok()?);
        match &header[4..8] {
            b"eXIf" => {
                let mut body = vec![0u8; length as usize];
                file.read_exact(&mut body).ok()?;
                return Some(body);
            }
            b"IEND" => return None,
            _ => {
                file.seek(SeekFrom::Current(i64::from(length) + 4)).ok()?;
            }
        }
    }
    None
}

fn heif(path: &Path) -> Option<Vec<u8>> {
    let context = libheif_rs::HeifContext::read_from_file(path.to_str()?).ok()?;
    let handle = context.primary_image_handle().ok()?;
    let mut ids = [0u32; 4];
    let count = handle.metadata_block_ids(&mut ids, b"Exif");
    let data = handle.metadata(*ids.get(..count)?.first()?).ok()?;
    // Four bytes giving where the TIFF header starts, after them.
    let skip = u32::from_be_bytes(data.get(0..4)?.try_into().ok()?) as usize;
    let rest = data.get(4 + skip..)?;
    Some(rest.strip_prefix(b"Exif\0\0").unwrap_or(rest).to_vec())
}

fn raw(path: &Path) -> Option<Camera> {
    use rawler::decoders::RawDecodeParams;
    let source = rawler::rawsource::RawSource::new(path).ok()?;
    let decoder = rawler::get_decoder(&source).ok()?;
    let metadata = decoder.raw_metadata(&source, &RawDecodeParams::default()).ok()?;
    let exif = metadata.exif;
    let ratio = |r: rawler::formats::tiff::Rational| (r.d != 0).then(|| f64::from(r.n) / f64::from(r.d));
    let signed = |r: rawler::formats::tiff::SRational| (r.d != 0).then(|| f64::from(r.n) / f64::from(r.d));
    let degrees = |parts: [rawler::formats::tiff::Rational; 3]| -> Option<f64> {
        Some(ratio(parts[0])? + ratio(parts[1])? / 60.0 + ratio(parts[2])? / 3600.0)
    };
    let location = exif.gps.as_ref().and_then(|gps| {
        let mut latitude = degrees(gps.gps_latitude?)?;
        let mut longitude = degrees(gps.gps_longitude?)?;
        if gps.gps_latitude_ref.as_deref().is_some_and(|r| r.starts_with('S')) {
            latitude = -latitude;
        }
        if gps.gps_longitude_ref.as_deref().is_some_and(|r| r.starts_with('W')) {
            longitude = -longitude;
        }
        let altitude = gps.gps_altitude.and_then(ratio).map(|a| if gps.gps_altitude_ref == Some(1) { -a } else { a });
        Some(Location { latitude, longitude, altitude })
    });
    let text = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    Some(Camera {
        make: text(&metadata.make),
        model: text(&metadata.model),
        lens: metadata
            .lens
            .map(|lens| format!("{} {}", lens.lens_make, lens.lens_model).trim().to_string())
            .or(exif.lens_model.clone())
            .and_then(|lens| text(&lens)),
        exposure: exif.exposure_time.map(|r| (r.n, r.d)),
        f_number: exif.fnumber.and_then(ratio),
        iso: exif.iso_speed_ratings.map(u32::from).or(exif.iso_speed),
        focal_length: exif.focal_length.and_then(ratio),
        focal_length_35mm: None,
        exposure_bias: exif.exposure_bias.and_then(signed),
        flash: exif.flash,
        white_balance: exif.white_balance,
        metering: exif.metering_mode,
        program: exif.exposure_program,
        taken: exif.date_time_original.or(exif.create_date),
        offset: exif.offset_time_original,
        software: None,
        artist: exif.artist.and_then(|s| text(&s)),
        copyright: exif.copyright.and_then(|s| text(&s)),
        location,
    })
}

/// One value from a TIFF directory, as far as EXIF needs.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Text(String),
    Numbers(Vec<u32>),
    Fractions(Vec<(u32, u32)>),
    Signed(Vec<(i32, i32)>),
}

impl Value {
    fn text(&self) -> Option<String> {
        match self {
            Value::Text(text) => Some(text.trim().to_string()).filter(|t| !t.is_empty()),
            _ => None,
        }
    }

    fn number(&self) -> Option<u32> {
        match self {
            Value::Numbers(numbers) => numbers.first().copied(),
            _ => None,
        }
    }

    fn fraction(&self) -> Option<(u32, u32)> {
        match self {
            Value::Fractions(fractions) => fractions.first().copied(),
            _ => None,
        }
    }

    fn real(&self) -> Option<f64> {
        match self {
            Value::Fractions(f) => f.first().filter(|(_, d)| *d != 0).map(|&(n, d)| f64::from(n) / f64::from(d)),
            Value::Signed(f) => f.first().filter(|(_, d)| *d != 0).map(|&(n, d)| f64::from(n) / f64::from(d)),
            Value::Numbers(n) => n.first().map(|&n| f64::from(n)),
            Value::Text(_) => None,
        }
    }

    /// Degrees, minutes and seconds, as one number of degrees.
    fn degrees(&self) -> Option<f64> {
        match self {
            Value::Fractions(parts) if parts.len() == 3 && parts.iter().all(|(_, d)| *d != 0) => Some(
                parts.iter().zip([1.0, 60.0, 3600.0]).map(|(&(n, d), scale)| f64::from(n) / f64::from(d) / scale).sum(),
            ),
            _ => None,
        }
    }
}

struct Tiff<'a> {
    bytes: &'a [u8],
    little: bool,
}

impl<'a> Tiff<'a> {
    fn new(bytes: &'a [u8]) -> Option<Self> {
        let little = match bytes.get(0..2)? {
            b"II" => true,
            b"MM" => false,
            _ => return None,
        };
        Some(Tiff { bytes, little })
    }

    fn u16_at(&self, at: usize) -> Option<u16> {
        let b: [u8; 2] = self.bytes.get(at..at + 2)?.try_into().ok()?;
        Some(if self.little { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
    }

    fn u32_at(&self, at: usize) -> Option<u32> {
        let b: [u8; 4] = self.bytes.get(at..at + 4)?.try_into().ok()?;
        Some(if self.little { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    }

    /// The tags of the directory at `offset`. Damaged entries are skipped.
    fn directory(&self, offset: usize) -> Vec<(u16, Value)> {
        let Some(count) = self.u16_at(offset) else { return Vec::new() };
        (0..usize::from(count).min(512))
            .filter_map(|i| {
                let entry = offset + 2 + i * 12;
                let tag = self.u16_at(entry)?;
                Some((tag, self.value(entry)?))
            })
            .collect()
    }

    fn value(&self, entry: usize) -> Option<Value> {
        let kind = self.u16_at(entry + 2)?;
        let count = self.u32_at(entry + 4)? as usize;
        let size = match kind {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 => 4,
            5 | 10 => 8,
            _ => return None,
        };
        if count == 0 || count > 65_536 {
            return None;
        }
        let length = size * count;
        let at = if length <= 4 { entry + 8 } else { self.u32_at(entry + 8)? as usize };
        let data = self.bytes.get(at..at + length)?;
        Some(match kind {
            2 => Value::Text(String::from_utf8_lossy(data.split(|&b| b == 0).next().unwrap_or(data)).into_owned()),
            1 | 7 => Value::Numbers(data.iter().map(|&b| u32::from(b)).collect()),
            3 => Value::Numbers((0..count).filter_map(|i| self.u16_at(at + i * 2).map(u32::from)).collect()),
            4 => Value::Numbers((0..count).filter_map(|i| self.u32_at(at + i * 4)).collect()),
            5 => Value::Fractions(
                (0..count).filter_map(|i| Some((self.u32_at(at + i * 8)?, self.u32_at(at + i * 8 + 4)?))).collect(),
            ),
            10 => Value::Signed(
                (0..count)
                    .filter_map(|i| Some((self.u32_at(at + i * 8)? as i32, self.u32_at(at + i * 8 + 4)? as i32)))
                    .collect(),
            ),
            _ => return None,
        })
    }
}

/// Read EXIF from a TIFF structure: the first directory, and the EXIF and
/// GPS directories it points to.
fn parse(bytes: &[u8]) -> Option<Camera> {
    let tiff = Tiff::new(bytes)?;
    let first = tiff.directory(tiff.u32_at(4)? as usize);
    let find = |list: &[(u16, Value)], tag: u16| list.iter().find(|(t, _)| *t == tag).map(|(_, v)| v.clone());
    let sub = |tag: u16| {
        find(&first, tag).and_then(|v| v.number()).map(|offset| tiff.directory(offset as usize)).unwrap_or_default()
    };
    let exif = sub(0x8769);
    let gps = sub(0x8825);
    // EXIF tags are meant to be in their own directory, but some writers put
    // them in the first one.
    let get = |tag: u16| find(&exif, tag).or_else(|| find(&first, tag));

    let location = (|| {
        let mut latitude = find(&gps, 0x0002)?.degrees()?;
        let mut longitude = find(&gps, 0x0004)?.degrees()?;
        if find(&gps, 0x0001).and_then(|v| v.text()).is_some_and(|r| r.starts_with('S')) {
            latitude = -latitude;
        }
        if find(&gps, 0x0003).and_then(|v| v.text()).is_some_and(|r| r.starts_with('W')) {
            longitude = -longitude;
        }
        // Many phones write 0, 0 when they had no fix.
        if latitude == 0.0 && longitude == 0.0 {
            return None;
        }
        let below = find(&gps, 0x0005).and_then(|v| v.number()) == Some(1);
        let altitude = find(&gps, 0x0006).and_then(|v| v.real()).map(|a| if below { -a } else { a });
        Some(Location { latitude, longitude, altitude })
    })();

    let lens = get(0xA434).and_then(|v| v.text()).map(|model| {
        match get(0xA433).and_then(|v| v.text()) {
            Some(make) if !model.to_lowercase().starts_with(&make.to_lowercase()) => format!("{make} {model}"),
            _ => model,
        }
    });

    Some(Camera {
        make: find(&first, 0x010F).and_then(|v| v.text()),
        model: find(&first, 0x0110).and_then(|v| v.text()),
        lens,
        exposure: get(0x829A).and_then(|v| v.fraction()),
        f_number: get(0x829D).and_then(|v| v.real()),
        iso: get(0x8827).and_then(|v| v.number()),
        focal_length: get(0x920A).and_then(|v| v.real()),
        focal_length_35mm: get(0xA405).and_then(|v| v.number()),
        exposure_bias: get(0x9204).and_then(|v| v.real()),
        flash: get(0x9209).and_then(|v| v.number()).map(|n| n as u16),
        white_balance: get(0xA403).and_then(|v| v.number()).map(|n| n as u16),
        metering: get(0x9207).and_then(|v| v.number()).map(|n| n as u16),
        program: get(0x8822).and_then(|v| v.number()).map(|n| n as u16),
        taken: get(0x9003).and_then(|v| v.text()).or_else(|| find(&first, 0x0132).and_then(|v| v.text())),
        offset: get(0x9011).and_then(|v| v.text()),
        software: find(&first, 0x0131).and_then(|v| v.text()),
        artist: find(&first, 0x013B).and_then(|v| v.text()),
        copyright: find(&first, 0x8298).and_then(|v| v.text()),
        location,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A TIFF structure written the way cameras write it, for tests here
    /// and elsewhere.
    pub(crate) struct Writer {
        little: bool,
        entries: Vec<Vec<(u16, u16, u32, Vec<u8>)>>,
    }

    impl Writer {
        pub(crate) fn new(little: bool) -> Self {
            Writer { little, entries: vec![Vec::new(), Vec::new(), Vec::new()] }
        }

        fn u16(&self, v: u16) -> [u8; 2] {
            if self.little { v.to_le_bytes() } else { v.to_be_bytes() }
        }

        fn u32(&self, v: u32) -> [u8; 4] {
            if self.little { v.to_le_bytes() } else { v.to_be_bytes() }
        }

        /// `directory`: 0 the first, 1 EXIF, 2 GPS.
        pub(crate) fn text(mut self, directory: usize, tag: u16, text: &str) -> Self {
            let mut data = text.as_bytes().to_vec();
            data.push(0);
            let count = data.len() as u32;
            self.entries[directory].push((tag, 2, count, data));
            self
        }

        pub(crate) fn short(mut self, directory: usize, tag: u16, value: u16) -> Self {
            let data = self.u16(value).to_vec();
            self.entries[directory].push((tag, 3, 1, data));
            self
        }

        pub(crate) fn fractions(mut self, directory: usize, tag: u16, values: &[(u32, u32)]) -> Self {
            let mut data = Vec::new();
            for &(n, d) in values {
                data.extend_from_slice(&self.u32(n));
                data.extend_from_slice(&self.u32(d));
            }
            self.entries[directory].push((tag, 5, values.len() as u32, data));
            self
        }

        pub(crate) fn signed(mut self, directory: usize, tag: u16, n: i32, d: i32) -> Self {
            let mut data = self.u32(n as u32).to_vec();
            data.extend_from_slice(&self.u32(d as u32));
            self.entries[directory].push((tag, 10, 1, data));
            self
        }

        pub(crate) fn build(mut self) -> Vec<u8> {
            // Pointers from the first directory to the other two, filled in
            // once their places are known.
            let has_exif = !self.entries[1].is_empty();
            let has_gps = !self.entries[2].is_empty();
            if has_exif {
                self.entries[0].push((0x8769, 4, 1, vec![0; 4]));
            }
            if has_gps {
                self.entries[0].push((0x8825, 4, 1, vec![0; 4]));
            }
            let sizes: Vec<usize> = self
                .entries
                .iter()
                .map(|list| 2 + list.len() * 12 + 4 + list.iter().map(|e| if e.3.len() > 4 { e.3.len() } else { 0 }).sum::<usize>())
                .collect();
            let starts = [8, 8 + sizes[0], 8 + sizes[0] + sizes[1]];
            let mut out = Vec::new();
            out.extend_from_slice(if self.little { b"II" } else { b"MM" });
            out.extend_from_slice(&self.u16(42));
            out.extend_from_slice(&self.u32(8));
            for (index, list) in self.entries.iter().enumerate() {
                let mut extra = starts[index] + 2 + list.len() * 12 + 4;
                let mut tail = Vec::new();
                out.extend_from_slice(&self.u16(list.len() as u16));
                for (tag, kind, count, data) in list {
                    out.extend_from_slice(&self.u16(*tag));
                    out.extend_from_slice(&self.u16(*kind));
                    out.extend_from_slice(&self.u32(*count));
                    let pointer = match *tag {
                        0x8769 => Some(starts[1] as u32),
                        0x8825 => Some(starts[2] as u32),
                        _ => None,
                    };
                    if let Some(pointer) = pointer {
                        out.extend_from_slice(&self.u32(pointer));
                    } else if data.len() <= 4 {
                        let mut inline = data.clone();
                        inline.resize(4, 0);
                        out.extend_from_slice(&inline);
                    } else {
                        out.extend_from_slice(&self.u32(extra as u32));
                        extra += data.len();
                        tail.extend_from_slice(data);
                    }
                }
                out.extend_from_slice(&self.u32(0));
                out.extend_from_slice(&tail);
            }
            out
        }
    }

    pub(crate) fn sample(little: bool) -> Vec<u8> {
        Writer::new(little)
            .text(0, 0x010F, "Canon")
            .text(0, 0x0110, "Canon EOS R6")
            .text(0, 0x0131, "Glance tests")
            .fractions(1, 0x829A, &[(1, 250)])
            .fractions(1, 0x829D, &[(28, 10)])
            .short(1, 0x8827, 400)
            .fractions(1, 0x920A, &[(50, 1)])
            .signed(1, 0x9204, -1, 3)
            .short(1, 0x9209, 0x10)
            .short(1, 0x8822, 3)
            .short(1, 0xA403, 0)
            .text(1, 0x9003, "2024:05:01 14:03:22")
            .text(1, 0x9011, "+02:00")
            .text(1, 0xA433, "Canon")
            .text(1, 0xA434, "RF24-105mm F4 L IS USM")
            .text(2, 0x0001, "N")
            .fractions(2, 0x0002, &[(48, 1), (51, 1), (2964, 100)])
            .text(2, 0x0003, "E")
            .fractions(2, 0x0004, &[(2, 1), (17, 1), (4013, 100)])
            .short(2, 0x0005, 0)
            .fractions(2, 0x0006, &[(35, 1)])
            .build()
    }

    #[test]
    fn a_camera_s_account_is_read_in_either_byte_order() {
        for little in [true, false] {
            let camera = parse(&sample(little)).expect("parses");
            assert_eq!(camera.camera_name().as_deref(), Some("Canon EOS R6"));
            assert_eq!(camera.lens.as_deref(), Some("Canon RF24-105mm F4 L IS USM"));
            assert_eq!(camera.exposure, Some((1, 250)));
            assert_eq!(camera.iso, Some(400));
            let location = camera.location.clone().expect("has a location");
            assert!((location.latitude - 48.858_233).abs() < 1e-5, "{}", location.latitude);
            assert!((location.longitude - 2.294_481).abs() < 1e-5);
            assert_eq!(location.altitude, Some(35.0));
            assert_eq!(location.coordinates(), "48.85823° N, 2.29448° E");
        }
    }

    #[test]
    fn values_are_said_as_photographers_say_them() {
        let rows = parse(&sample(true)).unwrap().rows();
        let get = |label: &str| rows.iter().find(|(l, _)| *l == label).map(|(_, v)| v.as_str());
        assert_eq!(get("Camera"), Some("Canon EOS R6"));
        assert_eq!(get("Shutter speed"), Some("1/250 s"));
        assert_eq!(get("Aperture"), Some("ƒ/2.8"));
        assert_eq!(get("ISO"), Some("400"));
        assert_eq!(get("Focal length"), Some("50 mm"));
        assert_eq!(get("Exposure compensation"), Some("−0.3 EV"));
        assert_eq!(get("Mode"), Some("Aperture priority"));
        assert_eq!(get("Flash"), Some("Did not fire"));
        assert_eq!(get("Taken"), Some("1 May 2024, 14:03:22 (UTC+02:00)"));
        assert_eq!(describe_exposure((10, 2500)).as_deref(), Some("1/250 s"));
        assert_eq!(describe_exposure((2, 1)).as_deref(), Some("2 s"));
        assert_eq!(describe_exposure((1, 2)).as_deref(), Some("0.5 s"));
    }

    #[test]
    fn exif_is_found_in_every_kind_of_file() {
        let tiff = sample(true);
        // JPEG: an APP1 block after the start marker.
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        jpeg.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
        jpeg.extend_from_slice(b"Exif\0\0");
        jpeg.extend_from_slice(&tiff);
        jpeg.extend_from_slice(&[0xFF, 0xDA, 0, 2]);
        assert_eq!(block(&jpeg), Some(tiff.as_slice()));
        // WebP: an EXIF chunk among the others, with or without the prefix.
        for prefix in [&b""[..], b"Exif\0\0"] {
            let mut webp = b"RIFF\0\0\0\0WEBPVP8X".to_vec();
            webp.extend_from_slice(&10u32.to_le_bytes());
            webp.extend_from_slice(&[0; 10]);
            webp.extend_from_slice(b"EXIF");
            webp.extend_from_slice(&((tiff.len() + prefix.len()) as u32).to_le_bytes());
            webp.extend_from_slice(prefix);
            webp.extend_from_slice(&tiff);
            assert_eq!(block(&webp), Some(tiff.as_slice()));
        }
        // PNG: an eXIf chunk after the pixels, found by walking the chunks.
        let dir = std::env::temp_dir().join(format!("glance-exif-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shot.png");
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::new(4, 4));
        let mut png_bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut png_bytes, image::ImageFormat::Png).unwrap();
        let mut png_bytes = png_bytes.into_inner();
        let end = png_bytes.len() - 12;
        let mut chunk = (tiff.len() as u32).to_be_bytes().to_vec();
        chunk.extend_from_slice(b"eXIf");
        chunk.extend_from_slice(&tiff);
        chunk.extend_from_slice(&[0; 4]);
        png_bytes.splice(end..end, chunk);
        std::fs::write(&path, &png_bytes).unwrap();
        assert_eq!(read(&path).and_then(|c| c.iso), Some(400));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_photo_with_nothing_to_say_says_nothing() {
        assert_eq!(parse(&Writer::new(true).build()).map(|c| c.is_empty()), Some(true));
        assert!(parse(b"not a tiff").is_none());
        // No fix: 0, 0 is not a place the photo was taken.
        let nowhere = Writer::new(true)
            .text(2, 0x0001, "N")
            .fractions(2, 0x0002, &[(0, 1), (0, 1), (0, 1)])
            .text(2, 0x0003, "E")
            .fractions(2, 0x0004, &[(0, 1), (0, 1), (0, 1)])
            .build();
        assert!(parse(&nowhere).unwrap().location.is_none());
    }
}
