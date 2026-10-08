// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! How annotations look, written into the file.
//!
//! An annotation can carry its own appearance: a small drawing that says
//! exactly what it looks like. Poppler writes one for a text box, but for a
//! highlight, an underline, a note, a shape or ink it works the look out each
//! time it draws one and never stores it. Desktop readers do the same, so on a
//! computer every mark shows; but many readers on phones draw only what is
//! stored, and there the marks were simply missing.
//!
//! Nor, unless it was given a font of its own, for a text box or a speech
//! bubble and its tail.
//!
//! So once Poppler has saved, every annotation of those kinds that has no
//! appearance is given one here, drawn the way Poppler draws it — a text box
//! in Helvetica, laid out as Poppler lays it out — so it looks the same in
//! Glance as before. Annotations that already have one are never touched.

use std::fmt::Write as _;
use std::path::Path;

use super::qpdf::{Object, Qpdf, Value};

/// Flags an annotation can have: hidden, or not drawn at all.
const INVISIBLE: i32 = 1;
const HIDDEN: i32 = 2;
/// A note's colour when it has none of its own: Poppler's yellow.
const NOTE_DEFAULT: [f64; 3] = [1.0, 1.0, 0.0];

/// The four-curve circle constant.
const KAPPA: f64 = 0.552_284_749_83;

/// Give each annotation in `path` that lacks an appearance one, and write
/// the result to `out`, protected as `path` was. Ok(0), with nothing
/// written, when none needed one.
pub fn add_missing(path: &Path, out: &Path, password: Option<&str>) -> Result<usize, String> {
    let document = Qpdf::read(path, password).map_err(|e| e.to_string())?;
    let mut added = 0;
    for page in document.pages() {
        let Some(annots) = document.get(page, "Annots").and_then(|a| document.items(a)) else { continue };
        for annot in annots {
            if give_appearance(&document, annot) {
                added += 1;
            }
        }
    }
    if added > 0 {
        document.write_as_is(out).map_err(|e| e.to_string())?;
    }
    Ok(added)
}

/// One annotation's appearance, if it has none and is of a kind drawn here.
fn give_appearance(document: &Qpdf, annot: Object) -> bool {
    if document.get(annot, "AP").is_some() {
        return false;
    }
    let flags = document.get(annot, "F").and_then(|f| document.integer(f)).unwrap_or(0);
    if flags & (INVISIBLE | HIDDEN) != 0 {
        return false;
    }
    let Some(kind) = document.get(annot, "Subtype").and_then(|s| document.name(s)) else { return false };
    let Some(rect) = numbers(document, document.get(annot, "Rect")).and_then(|r| <[f64; 4]>::try_from(r).ok()) else {
        return false;
    };
    let rect = [rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])];
    let look = Look {
        colour: numbers(document, document.get(annot, "C")),
        inside: numbers(document, document.get(annot, "IC")),
        width: border_width(document, annot),
        opacity: document.get(annot, "CA").and_then(|o| document.number(o)).unwrap_or(1.0).clamp(0.0, 1.0),
    };
    let drawn = match kind.as_str() {
        "Highlight" | "Underline" | "StrikeOut" | "Squiggly" => {
            let quads = numbers(document, document.get(annot, "QuadPoints")).unwrap_or_default();
            markup(&kind, &quads, &look)
        }
        "Ink" => {
            let strokes = document
                .get(annot, "InkList")
                .and_then(|list| document.items(list))
                .map(|list| list.into_iter().filter_map(|s| numbers(document, Some(s))).collect::<Vec<_>>())
                .unwrap_or_default();
            ink(&strokes, &look)
        }
        "Square" | "Circle" => {
            let inset = numbers(document, document.get(annot, "RD")).filter(|d| d.len() == 4).unwrap_or(vec![0.0; 4]);
            let inner = [rect[0] + inset[0], rect[1] + inset[3], rect[2] - inset[2], rect[3] - inset[1]];
            shape(kind == "Circle", inner, &look)
        }
        "Text" => Some(note(rect, &look)),
        "Line" => numbers(document, document.get(annot, "L")).and_then(|l| line(&l, &look)),
        "FreeText" => {
            let text = document.get(annot, "Contents").and_then(|c| document.string(c)).unwrap_or_default();
            let style = document.get(annot, "DA").and_then(|d| document.string(d)).unwrap_or_default();
            Some(free_text(rect, &text, &style, &look))
        }
        _ => None,
    };
    let Some(Drawing { content, blend, font }) = drawn else { return false };

    let mut state: Vec<(&str, Value)> = vec![("Type", Value::Name("ExtGState"))];
    if blend {
        state.push(("BM", Value::Name("Multiply")));
    }
    if look.opacity < 1.0 {
        state.push(("CA", Value::Real(look.opacity)));
        state.push(("ca", Value::Real(look.opacity)));
    }
    let state = document.new_dictionary(state);
    let states = document.new_dictionary(vec![("G0", Value::Object(state))]);
    let mut resources = vec![("ExtGState", Value::Object(states))];
    if font {
        let helvetica = document.new_dictionary(vec![
            ("Type", Value::Name("Font")),
            ("Subtype", Value::Name("Type1")),
            ("BaseFont", Value::Name("Helvetica")),
            ("Encoding", Value::Name("WinAnsiEncoding")),
        ]);
        let fonts = document.new_dictionary(vec![("Helv", Value::Object(helvetica))]);
        resources.push(("Font", Value::Object(fonts)));
    }
    let resources = document.new_dictionary(resources);
    let stream = document.new_stream(
        format!("/G0 gs\n{content}").as_bytes(),
        None,
        vec![
            ("Type", Value::Name("XObject")),
            ("Subtype", Value::Name("Form")),
            ("BBox", Value::Array(rect.iter().map(|&n| Value::Real(n)).collect())),
            ("Resources", Value::Object(resources)),
        ],
    );
    let appearance = document.new_dictionary(vec![("N", Value::Object(stream))]);
    document.set(annot, "AP", Value::Object(appearance));
    true
}

/// What an annotation is drawn with.
struct Look {
    colour: Option<Vec<f64>>,
    inside: Option<Vec<f64>>,
    width: f64,
    opacity: f64,
}

/// An appearance's drawing, and whether it is laid over the page as a
/// highlighter is: darkening, never covering.
struct Drawing {
    content: String,
    blend: bool,
    /// Writes text, in Helvetica.
    font: bool,
}

fn numbers(document: &Qpdf, object: Option<Object>) -> Option<Vec<f64>> {
    let items = document.items(object?)?;
    items.into_iter().map(|item| document.number(item)).collect()
}

/// The line's thickness: from the border style, the older border array, or
/// one point.
fn border_width(document: &Qpdf, annot: Object) -> f64 {
    if let Some(width) = document.get(annot, "BS").and_then(|bs| document.get(bs, "W")).and_then(|w| document.number(w)) {
        return width.max(0.0);
    }
    if let Some(border) = numbers(document, document.get(annot, "Border")) {
        if let Some(&width) = border.get(2) {
            return width.max(0.0);
        }
    }
    1.0
}

fn n(value: f64) -> String {
    let text = format!("{value:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" { "0".to_string() } else { text.to_string() }
}

/// A colour operator, for strokes or fills; None for no colour at all.
fn colour(components: &[f64], stroke: bool) -> Option<String> {
    let parts: Vec<String> = components.iter().map(|&c| n(c.clamp(0.0, 1.0))).collect();
    let op = match (components.len(), stroke) {
        (1, true) => "G",
        (1, false) => "g",
        (3, true) => "RG",
        (3, false) => "rg",
        (4, true) => "K",
        (4, false) => "k",
        _ => return None,
    };
    Some(format!("{} {op}", parts.join(" ")))
}

/// Highlights, underlines and strike-throughs, line by line of text.
fn markup(kind: &str, quads: &[f64], look: &Look) -> Option<Drawing> {
    let default: &[f64] = match kind {
        "Highlight" => &[1.0, 1.0, 0.0],
        _ => &[0.0, 0.0, 0.0],
    };
    let rgb = look.colour.as_deref().filter(|c| !c.is_empty()).unwrap_or(default);
    let mut out = String::new();
    for quad in quads.as_chunks::<8>().0 {
        let points: Vec<(f64, f64)> = quad.as_chunks::<2>().0.iter().map(|&[x, y]| (x, y)).collect();
        // Readers disagree on the order the corners come in, so they are
        // put in order here: round the middle, and top and bottom by height.
        let (cx, cy) = (points.iter().map(|p| p.0).sum::<f64>() / 4.0, points.iter().map(|p| p.1).sum::<f64>() / 4.0);
        let mut around = points.clone();
        around.sort_by(|a, b| (a.1 - cy).atan2(a.0 - cx).total_cmp(&(b.1 - cy).atan2(b.0 - cx)));
        let mut by_height = points.clone();
        by_height.sort_by(|a, b| a.1.total_cmp(&b.1));
        let (bottom, top) = ((by_height[0], by_height[1]), (by_height[2], by_height[3]));
        let left_right = |(a, b): ((f64, f64), (f64, f64))| if a.0 <= b.0 { (a, b) } else { (b, a) };
        let (bottom, top) = (left_right(bottom), left_right(top));
        let height = ((top.0 .1 + top.1 .1) - (bottom.0 .1 + bottom.1 .1)) / 2.0;
        match kind {
            "Highlight" => {
                let _ = write!(out, "{} m", xy(around[0]));
                for &p in &around[1..] {
                    let _ = write!(out, " {} l", xy(p));
                }
                out.push_str(" h f\n");
            }
            _ => {
                let thickness = (height * 0.07).max(0.75);
                // Underlined just below the letters, struck through their middle.
                let lift = if kind == "StrikeOut" { height / 2.0 } else { thickness };
                let (a, b) = (bottom.0, bottom.1);
                let (a, b) = ((a.0, a.1 + lift), (b.0, b.1 + lift));
                let _ = write!(out, "{} w ", n(thickness));
                if kind == "Squiggly" {
                    let _ = write!(out, "{} m", xy(a));
                    let steps = (((b.0 - a.0).hypot(b.1 - a.1)) / (thickness * 3.0)).max(1.0) as usize;
                    for i in 1..=steps {
                        let t = i as f64 / steps as f64;
                        let up = if i % 2 == 1 { thickness * 1.5 } else { 0.0 };
                        let _ = write!(out, " {} l", xy((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t + up)));
                    }
                    out.push_str(" S\n");
                } else {
                    let _ = writeln!(out, "{} m {} l S", xy(a), xy(b));
                }
            }
        }
    }
    if out.is_empty() {
        return None;
    }
    let is_highlight = kind == "Highlight";
    let paint = colour(rgb, !is_highlight)?;
    Some(Drawing { content: format!("{paint}\n{out}"), blend: is_highlight, font: false })
}

fn xy((x, y): (f64, f64)) -> String {
    format!("{} {}", n(x), n(y))
}

/// Freehand ink, and the lines, arrows and shapes Glance draws as ink.
fn ink(strokes: &[Vec<f64>], look: &Look) -> Option<Drawing> {
    let paint = colour(look.colour.as_deref().filter(|c| !c.is_empty()).unwrap_or(&[0.0]), true)?;
    let mut out = format!("{paint}\n{} w 1 J 1 j\n", n(look.width.max(0.1)));
    let mut any = false;
    for stroke in strokes {
        let points: Vec<(f64, f64)> = stroke.as_chunks::<2>().0.iter().map(|&[x, y]| (x, y)).collect();
        let Some((&first, rest)) = points.split_first() else { continue };
        let _ = write!(out, "{} m", xy(first));
        // A single point is a dot: a line of no length, made round by its cap.
        if rest.is_empty() {
            let _ = write!(out, " {} l", xy(first));
        }
        for &p in rest {
            let _ = write!(out, " {} l", xy(p));
        }
        out.push_str(" S\n");
        any = true;
    }
    any.then_some(Drawing { content: out, blend: false, font: false })
}

/// A rectangle or ellipse, its line inside the box, filled if it has an
/// inside colour.
fn shape(ellipse: bool, [x1, y1, x2, y2]: [f64; 4], look: &Look) -> Option<Drawing> {
    let half = look.width / 2.0;
    let (x1, y1, x2, y2) = (x1 + half, y1 + half, x2 - half, y2 - half);
    if x2 <= x1 || y2 <= y1 {
        return None;
    }
    let stroke = look.colour.as_deref().filter(|_| look.width > 0.0).and_then(|c| colour(c, true));
    let fill = look.inside.as_deref().and_then(|c| colour(c, false));
    let op = match (&stroke, &fill) {
        (Some(_), Some(_)) => "B",
        (Some(_), None) => "S",
        (None, Some(_)) => "f",
        (None, None) => return None,
    };
    let mut out = String::new();
    for paint in [&stroke, &fill].into_iter().flatten() {
        let _ = writeln!(out, "{paint}");
    }
    let _ = writeln!(out, "{} w", n(look.width));
    if ellipse {
        let (cx, cy, rx, ry) = ((x1 + x2) / 2.0, (y1 + y2) / 2.0, (x2 - x1) / 2.0, (y2 - y1) / 2.0);
        let (ox, oy) = (rx * KAPPA, ry * KAPPA);
        let _ = writeln!(out, "{} m", xy((cx + rx, cy)));
        let curve = |out: &mut String, a: (f64, f64), b: (f64, f64), c: (f64, f64)| {
            let _ = writeln!(out, "{} {} {} c", xy(a), xy(b), xy(c));
        };
        curve(&mut out, (cx + rx, cy + oy), (cx + ox, cy + ry), (cx, cy + ry));
        curve(&mut out, (cx - ox, cy + ry), (cx - rx, cy + oy), (cx - rx, cy));
        curve(&mut out, (cx - rx, cy - oy), (cx - ox, cy - ry), (cx, cy - ry));
        curve(&mut out, (cx + ox, cy - ry), (cx + rx, cy - oy), (cx + rx, cy));
        let _ = writeln!(out, "h {op}");
    } else {
        let _ = writeln!(out, "{} {} {} {} re {op}", n(x1), n(y1), n(x2 - x1), n(y2 - y1));
    }
    Some(Drawing { content: out, blend: false, font: false })
}

/// A sticky note: a square of its colour, edged, with lines of writing.
fn note([x1, y1, x2, y2]: [f64; 4], look: &Look) -> Drawing {
    let rgb = look.colour.as_deref().filter(|c| c.len() == 3).unwrap_or(&NOTE_DEFAULT);
    let size = (x2 - x1).min(y2 - y1).max(4.0);
    let (x, y) = (x1 + ((x2 - x1) - size) / 2.0, y1 + ((y2 - y1) - size) / 2.0);
    let inset = size * 0.08;
    let mut out = String::new();
    let _ = writeln!(out, "{}", colour(rgb, false).unwrap_or_default());
    let _ = writeln!(out, "0.25 G {} w", n((size * 0.05).max(0.5)));
    let _ = writeln!(out, "{} {} {} {} re B", n(x + inset), n(y + inset), n(size - inset * 2.0), n(size - inset * 2.0));
    let _ = writeln!(out, "0.35 G {} w 1 J", n((size * 0.06).max(0.5)));
    for i in 1..=3 {
        let line_y = y + size - inset - (size - inset * 2.0) * f64::from(i) / 4.0;
        let _ = writeln!(out, "{} m {} l S", xy((x + size * 0.25, line_y)), xy((x + size * 0.75, line_y)));
    }
    Drawing { content: out, blend: false, font: false }
}

/// A straight line, as a speech bubble's tail is.
fn line(ends: &[f64], look: &Look) -> Option<Drawing> {
    let [x1, y1, x2, y2] = <[f64; 4]>::try_from(ends).ok()?;
    let paint = colour(look.colour.as_deref().filter(|c| !c.is_empty()).unwrap_or(&[0.0]), true)?;
    let content = format!("{paint}\n{} w 1 J\n{} m {} l S\n", n(look.width.max(0.1)), xy((x1, y1)), xy((x2, y2)));
    Some(Drawing { content, blend: false, font: false })
}

/// Helvetica's widths, in thousandths of the size, for the printable ASCII
/// characters from space on. Everything else is taken as an average letter.
const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, // space to /
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, // 0 to 9
    278, 278, 584, 584, 584, 556, 1015, // : to @
    667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, // A to M
    722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, // N to Z
    278, 278, 278, 469, 556, 333, // [ to `
    556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, // a to m
    556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, // n to z
    334, 260, 334, 584, // { to ~
];
/// How far above the baseline Helvetica's capitals reach, as a share of
/// the size.
const ASCENT: f64 = 0.718;

fn width_of(c: char, size: f64) -> f64 {
    let code = c as u32;
    let thousandths = if (32..127).contains(&code) { HELVETICA[(code - 32) as usize] } else { 556 };
    f64::from(thousandths) * size / 1000.0
}

/// A character as Windows-1252, the encoding the font is given; anything
/// it has no place for becomes a question mark.
fn win_ansi(c: char) -> u8 {
    match c {
        ' '..='~' => c as u8,
        '\u{a0}'..='\u{ff}' => c as u32 as u8,
        '€' => 0x80,
        '‚' => 0x82,
        '„' => 0x84,
        '…' => 0x85,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        '™' => 0x99,
        _ => b'?',
    }
}

/// The text's lines, broken between words to fit `width`, and inside a word
/// only when it is longer than a line.
fn wrap(text: &str, size: f64, width: f64) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        let mut line_width = 0.0;
        for word in paragraph.split(' ') {
            let word_width: f64 = word.chars().map(|c| width_of(c, size)).sum();
            let space = if line.is_empty() { 0.0 } else { width_of(' ', size) };
            if !line.is_empty() && line_width + space + word_width > width {
                lines.push(std::mem::take(&mut line));
                line_width = 0.0;
            }
            if !line.is_empty() {
                line.push(' ');
                line_width += width_of(' ', size);
            }
            for c in word.chars() {
                let w = width_of(c, size);
                if !line.is_empty() && line_width + w > width && line_width > 0.0 && word_width > width {
                    lines.push(std::mem::take(&mut line));
                    line_width = 0.0;
                }
                line.push(c);
                line_width += w;
            }
        }
        lines.push(line);
    }
    lines
}

/// A text box: its box filled and outlined as Poppler draws one, and its
/// words in Helvetica at the size and colour its style asks for.
fn free_text([x1, y1, x2, y2]: [f64; 4], text: &str, style: &str, look: &Look) -> Drawing {
    // The style is a scrap of page description: "/Helv 12 Tf 0 0 1 rg".
    let tokens: Vec<&str> = style.split_whitespace().collect();
    let mut size = 10.0;
    let mut ink: Vec<f64> = vec![0.0];
    for (i, token) in tokens.iter().enumerate() {
        let before = |k: usize| -> Vec<f64> { tokens[i.saturating_sub(k)..i].iter().filter_map(|t| t.parse().ok()).collect() };
        match *token {
            "Tf" => size = before(1).first().copied().filter(|s| *s > 0.0).unwrap_or(size),
            "g" => ink = before(1),
            "rg" => ink = before(3),
            "k" => ink = before(4),
            _ => {}
        }
    }
    let border = look.width;
    let mut out = String::new();
    let fill = look.colour.as_deref().and_then(|c| colour(c, false));
    let stroke = (border > 0.0).then(|| colour(&ink, true)).flatten();
    if fill.is_some() || stroke.is_some() {
        for paint in [&stroke, &fill].into_iter().flatten() {
            let _ = writeln!(out, "{paint}");
        }
        let half = border / 2.0;
        let op = match (&fill, &stroke) {
            (Some(_), Some(_)) => "b",
            (Some(_), None) => "f",
            _ => "S",
        };
        let _ = writeln!(out, "{} w {} {} {} {} re {op}", n(border), n(x1 + half), n(y1 + half), n(x2 - x1 - border), n(y2 - y1 - border));
    }
    let margin = border * 2.0;
    let width = (x2 - x1 - margin * 2.0).max(1.0);
    let _ = writeln!(out, "{} {} {} {} re W n", n(x1 + margin), n(y1 + margin), n(width), n((y2 - y1 - margin * 2.0).max(1.0)));
    let _ = writeln!(out, "BT\n{}\n/Helv {} Tf", colour(&ink, false).unwrap_or_else(|| "0 g".into()), n(size));
    let mut y = y2 - margin - size * ASCENT;
    for line in wrap(text, size, width) {
        let bytes: Vec<u8> = line.chars().map(win_ansi).collect();
        let mut escaped = String::new();
        for b in bytes {
            match b {
                b'(' | b')' | b'\\' => {
                    escaped.push('\\');
                    escaped.push(b as char);
                }
                32..=126 => escaped.push(b as char),
                _ => {
                    let _ = write!(escaped, "\\{b:03o}");
                }
            }
        }
        let _ = writeln!(out, "1 0 0 1 {} {} Tm ({escaped}) Tj", n(x1 + margin), n(y));
        y -= size;
    }
    out.push_str("ET\n");
    Drawing { content: out, blend: false, font: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(colour: &[f64]) -> Look {
        Look { colour: Some(colour.to_vec()), inside: None, width: 2.0, opacity: 1.0 }
    }

    #[test]
    fn a_highlight_fills_each_line_and_darkens_rather_than_covers() {
        // Two lines, corners in the order Acrobat and Poppler write them.
        let quads = [10.0, 30.0, 90.0, 30.0, 10.0, 20.0, 90.0, 20.0, 10.0, 18.0, 60.0, 18.0, 10.0, 8.0, 60.0, 8.0];
        let drawn = markup("Highlight", &quads, &look(&[1.0, 0.9, 0.0])).unwrap();
        assert!(drawn.blend);
        assert!(drawn.content.starts_with("1 0.9 0 rg"));
        assert_eq!(drawn.content.matches(" h f").count(), 2);
    }

    #[test]
    fn underline_sits_under_the_text_and_strike_through_across_it() {
        let quad = [10.0, 30.0, 90.0, 30.0, 10.0, 20.0, 90.0, 20.0];
        let under = markup("Underline", &quad, &look(&[0.0])).unwrap().content;
        let struck = markup("StrikeOut", &quad, &look(&[0.0])).unwrap().content;
        assert!(under.contains("10 20.75 m 90 20.75 l S"), "{under}");
        assert!(struck.contains("10 25 m 90 25 l S"), "{struck}");
    }

    #[test]
    fn ink_strokes_every_line_and_a_lone_point_becomes_a_dot() {
        let drawn = ink(&[vec![0.0, 0.0, 5.0, 5.0, 9.0, 1.0], vec![3.0, 3.0]], &look(&[0.0, 0.0, 1.0])).unwrap();
        assert!(drawn.content.contains("0 0 m 5 5 l 9 1 l S"));
        assert!(drawn.content.contains("3 3 m 3 3 l S"));
        assert!(ink(&[], &look(&[0.0])).is_none());
    }

    #[test]
    fn a_text_box_wraps_its_words_in_its_own_style() {
        let style = Look { colour: Some(vec![1.0, 1.0, 0.8]), inside: None, width: 1.0, opacity: 1.0 };
        let drawn = free_text([100.0, 100.0, 200.0, 160.0], "Look here, please (now)", "/Helv 12 Tf 0 0 1 rg", &style);
        assert!(drawn.font);
        assert!(drawn.content.contains("1 1 0.8 rg") && drawn.content.contains("0 0 1 RG"), "{}", drawn.content);
        assert!(drawn.content.contains("/Helv 12 Tf"));
        // Too wide for one line of the box: broken between words.
        assert_eq!(drawn.content.matches(" Tj").count(), 2, "{}", drawn.content);
        assert!(drawn.content.contains("\\(now\\)"), "brackets are escaped");
        assert_eq!(wrap("a\nb", 10.0, 100.0), ["a", "b"]);
        assert_eq!(win_ansi('é'), 0xe9);
        assert_eq!(win_ansi('—'), 0x97);
        assert_eq!(win_ansi('漢'), b'?');
    }

    #[test]
    fn a_shape_keeps_its_line_inside_its_box_and_fills_only_when_asked() {
        let plain = shape(false, [0.0, 0.0, 100.0, 50.0], &look(&[1.0, 0.0, 0.0])).unwrap().content;
        assert!(plain.contains("1 1 98 48 re S"), "{plain}");
        let filled = Look { inside: Some(vec![0.0, 1.0, 0.0]), ..look(&[1.0, 0.0, 0.0]) };
        let filled = shape(true, [0.0, 0.0, 100.0, 50.0], &filled).unwrap().content;
        assert!(filled.contains("0 1 0 rg") && filled.ends_with("h B\n"), "{filled}");
    }
}
