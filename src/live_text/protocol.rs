// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! What Glance and its `glance-ocr` helper say to each other, over the
//! helper's standard input and output.
//!
//! Glance asks with one line, `READ <id> <width> <height>`, followed by the
//! picture as `width * height * 4` bytes of RGBA. The helper answers with a
//! `LINE` for every line of text it found, then `DONE <id>`, or `FAIL <id>
//! <reason>` if it could not read the picture at all. Plain lines, so either
//! side can be tried by hand in a terminal.
//!
//! A `LINE` gives its confidence, the four corners of the line on the picture
//! (top left, top right, bottom right, bottom left), where each character
//! starts and ends along the line as a fraction of its length, and last the
//! text itself, which may have spaces in it but never a line break.

/// One line of text found on a picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    /// 0 to 1: how sure the recogniser is.
    pub score: f32,
    /// Top left, top right, bottom right, bottom left, in the picture's pixels.
    pub quad: [[f32; 2]; 4],
    /// Where the characters start and end along the line, 0 at its start and
    /// 1 at its end: one more than there are characters.
    pub cuts: Vec<f32>,
}

/// An answer from the helper.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Line(Line),
    Done(u64),
    Fail(u64, String),
}

/// The line that asks for a picture to be read. The pixels follow it.
pub fn request(id: u64, width: u32, height: u32) -> String {
    format!("READ {id} {width} {height}\n")
}

/// The id, width and height from a request line.
pub fn parse_request(line: &str) -> Option<(u64, u32, u32)> {
    let mut parts = line.split_whitespace();
    if parts.next()? != "READ" {
        return None;
    }
    let id = parts.next()?.parse().ok()?;
    let width = parts.next()?.parse().ok()?;
    let height = parts.next()?.parse().ok()?;
    (width > 0 && height > 0 && parts.next().is_none()).then_some((id, width, height))
}

impl Line {
    /// As a `LINE` message, ending with a line break.
    pub fn message(&self) -> String {
        let mut out = format!("LINE {:.4}", self.score);
        for [x, y] in self.quad {
            out.push_str(&format!(" {x:.1} {y:.1}"));
        }
        out.push_str(&format!(" {}", self.cuts.len()));
        for cut in &self.cuts {
            out.push_str(&format!(" {cut:.4}"));
        }
        // Never a line break inside: it would end the message early.
        out.push(' ');
        out.push_str(&self.text.replace(['\n', '\r'], " "));
        out.push('\n');
        out
    }
}

pub fn done(id: u64) -> String {
    format!("DONE {id}\n")
}

pub fn fail(id: u64, reason: &str) -> String {
    format!("FAIL {id} {}\n", reason.replace(['\n', '\r'], " "))
}

/// Read one answer line, without its line break.
pub fn parse_reply(line: &str) -> Option<Reply> {
    let line = line.trim_end_matches(['\n', '\r']);
    let (kind, rest) = line.split_once(' ')?;
    match kind {
        "DONE" => Some(Reply::Done(rest.trim().parse().ok()?)),
        "FAIL" => {
            let (id, reason) = rest.split_once(' ').unwrap_or((rest, ""));
            Some(Reply::Fail(id.parse().ok()?, reason.to_string()))
        }
        "LINE" => {
            // Numbers first, a fixed count of them and then the cuts; the
            // text is whatever is left, spaces and all.
            let mut rest = rest;
            let mut next = || -> Option<f32> {
                let (word, tail) = rest.split_once(' ').unwrap_or((rest, ""));
                rest = tail;
                word.parse().ok()
            };
            let score = next()?;
            let mut quad = [[0.0; 2]; 4];
            for corner in &mut quad {
                *corner = [next()?, next()?];
            }
            let count = next()? as usize;
            if count > 100_000 {
                return None;
            }
            let cuts = (0..count).map(|_| next()).collect::<Option<Vec<_>>>()?;
            Some(Reply::Line(Line { text: rest.to_string(), score, quad, cuts }))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_answers_survive_the_trip() {
        assert_eq!(parse_request(request(7, 640, 480).trim()), Some((7, 640, 480)));
        assert_eq!(parse_request("READ 7 0 480"), None);
        assert_eq!(parse_request("READ 7 640"), None);

        let line = Line {
            text: "Total due:  $1,284.50".into(),
            score: 0.9871,
            quad: [[10.0, 20.5], [300.0, 20.5], [300.0, 52.0], [10.0, 52.0]],
            cuts: vec![0.0, 0.25, 0.5, 1.0],
        };
        assert_eq!(parse_reply(&line.message()), Some(Reply::Line(line)));
        assert_eq!(parse_reply(&done(3)), Some(Reply::Done(3)));
        assert_eq!(parse_reply(&fail(3, "no runtime\nhere")), Some(Reply::Fail(3, "no runtime here".into())));
        assert_eq!(parse_reply("NONSENSE"), None);
    }

    #[test]
    fn a_line_break_in_the_text_cannot_split_the_message() {
        let line = Line { text: "one\ntwo".into(), score: 1.0, quad: [[0.0; 2]; 4], cuts: vec![] };
        let message = line.message();
        assert_eq!(message.matches('\n').count(), 1);
        assert!(matches!(parse_reply(&message), Some(Reply::Line(l)) if l.text == "one two"));
    }
}
