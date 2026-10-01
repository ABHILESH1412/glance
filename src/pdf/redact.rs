// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Redaction that cannot be undone, by anyone.
//!
//! A black box drawn over a PDF hides nothing: the text, pictures and drawing
//! instructions under it are still in the file, and any program can lift the
//! box off or simply copy the text out. Taking out exactly what is under a
//! box, and nothing else, means rewriting the page's drawing instructions
//! glyph by glyph and picture by picture — a large, delicate job where one
//! missed case leaks what was meant to go.
//!
//! So a page with anything redacted is replaced by a picture of itself, with
//! the boxes painted into the pixels before they are saved. Nothing of the
//! original page survives: not its text, its fonts, its drawings, its hidden
//! layers, its annotations, or its form fields. The document is then written
//! afresh by qpdf, which keeps only what is still used, so earlier versions
//! of the page kept by incremental saves go too.
//!
//! The page should not stop being text, though. The words outside the boxes
//! are put back as invisible text over the picture, where they were — the
//! way a scanned document is made searchable — so they can still be found,
//! selected and copied. A word any part of which is under a box is left out
//! whole, so not even a letter of it remains.
//!
//! Finally the result is opened again and checked: its redacted pages may
//! hold no word that was not kept.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use gtk::cairo;

use super::document;
use super::markup;
use super::qpdf::{Object, Protection, Qpdf, Value};
use super::rewrite;

/// Pages are pictured at this many dots per inch: print quality.
const DPI: f64 = 300.0;
/// Pixels in one page's picture at most, for very large pages.
const MAX_PIXELS: f64 = 60_000_000.0;
/// How far around a box a glyph counts as under it, in points.
const REACH: f64 = 0.5;
/// Every invisible glyph is this wide, in thousandths of its size; scaling
/// each one horizontally makes it exactly as wide as the letter it stands for.
const GLYPH_WIDTH: f64 = 500.0;

/// One area to black out: a page, counting from zero, and x, y, width and
/// height in points from the page's top-left corner, as Poppler shows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub page: usize,
    pub rect: [f64; 4],
}

#[derive(Debug, PartialEq)]
pub struct Outcome {
    pub pages: usize,
    /// Words taken out of the text, which is what most people redact.
    pub words: usize,
}

/// One page, flattened: its picture with the boxes painted in, and the text
/// to lay invisibly over it.
struct Flat {
    width: f64,
    height: f64,
    pixels: Vec<u8>,
    columns: u32,
    rows: u32,
    kept: Vec<Glyph>,
    words_removed: usize,
    /// Every letter kept, to check the result against.
    letters_kept: Vec<char>,
}

/// Write `source` to `out` with `areas` redacted. Runs on a worker thread.
pub fn redact(source: &Path, password: Option<&str>, areas: &[Area], out: &Path) -> Result<Outcome, String> {
    let mut by_page: BTreeMap<usize, Vec<[f64; 4]>> = BTreeMap::new();
    for area in areas {
        let [x, y, w, h] = area.rect;
        if w > 0.0 && h > 0.0 {
            by_page.entry(area.page).or_default().push([x, y, w, h]);
        }
    }
    if by_page.is_empty() {
        return Err("nothing is marked".into());
    }

    let reader = poppler::Document::from_file(&document::uri(source), password).map_err(|e| e.message().to_string())?;
    let mut flats = Vec::new();
    for (&page, boxes) in &by_page {
        let page_handle =
            i32::try_from(page).ok().and_then(|i| reader.page(i)).ok_or_else(|| format!("there is no page {}", page + 1))?;
        flats.push((page, flatten(&page_handle, boxes)?));
    }
    drop(reader);

    let document = Qpdf::read(source, password).map_err(|e| e.to_string())?;
    let pages = document.pages();
    for (page, flat) in &flats {
        let object = *pages.get(*page).ok_or_else(|| format!("there is no page {}", page + 1))?;
        rewrite_page(&document, object, flat);
    }
    forget_structure(&document);
    prune_fields(&document, &pages);
    document.write(out, Protection::Keep).map_err(|e| e.to_string())?;

    check(out, password, &flats)?;
    Ok(Outcome { pages: flats.len(), words: flats.iter().map(|(_, f)| f.words_removed).sum() })
}

/// Picture a page with its boxes painted in, and work out which of its text
/// stays.
fn flatten(page: &poppler::Page, boxes: &[[f64; 4]]) -> Result<Flat, String> {
    let (width, height) = page.size();
    let scale = (DPI / 72.0).min((MAX_PIXELS / (width * height).max(1.0)).sqrt());
    let columns = (width * scale).ceil().max(1.0) as i32;
    let rows = (height * scale).ceil().max(1.0) as i32;
    let mut surface =
        cairo::ImageSurface::create(cairo::Format::ARgb32, columns, rows).map_err(|e| e.to_string())?;
    {
        let cr = cairo::Context::new(&surface).map_err(|e| e.to_string())?;
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.paint().map_err(|e| e.to_string())?;
        cr.save().map_err(|e| e.to_string())?;
        let (sx, sy) = (f64::from(columns) / width, f64::from(rows) / height);
        cr.scale(sx, sy);
        // As it is seen, annotations included: they are flattened with it.
        page.render(&cr);
        cr.restore().map_err(|e| e.to_string())?;
        // The boxes, in whole pixels, a pixel wider all round and with no
        // soft edge, so no shade of anything under them is left at the rim.
        cr.set_antialias(cairo::Antialias::None);
        cr.set_source_rgb(0.0, 0.0, 0.0);
        for [x, y, w, h] in boxes {
            let left = (x * sx).floor() - 1.0;
            let top = (y * sy).floor() - 1.0;
            let right = ((x + w) * sx).ceil() + 1.0;
            let bottom = ((y + h) * sy).ceil() + 1.0;
            cr.rectangle(left, top, right - left, bottom - top);
        }
        cr.fill().map_err(|e| e.to_string())?;
    }
    surface.flush();
    let stride = usize::try_from(surface.stride()).map_err(|e| e.to_string())?;
    let data = surface.data().map_err(|e| e.to_string())?;
    let (cols, rws) = (columns as usize, rows as usize);
    let mut pixels = Vec::with_capacity(cols * rws * 3);
    for row in 0..rws {
        let line = &data[row * stride..row * stride + cols * 4];
        for px in line.chunks_exact(4) {
            // Cairo's ARGB32 in memory order, on an opaque white page.
            let (b, g, r) = if cfg!(target_endian = "little") { (px[0], px[1], px[2]) } else { (px[3], px[2], px[1]) };
            pixels.extend_from_slice(&[r, g, b]);
        }
    }

    let (kept, words_removed) = keep_text(&markup::glyphs(page), boxes);
    let letters_kept = kept.iter().map(|(c, _, _)| *c).filter(|c| !c.is_whitespace()).collect();
    Ok(Flat { width, height, pixels, columns: columns as u32, rows: rows as u32, kept, words_removed, letters_kept })
}

/// A letter kept: what it is, its box, and which way its word runs, in
/// quarter turns clockwise from left to right.
type Glyph = (char, [f64; 4], u8);

/// Which way a run of glyph boxes reads, in quarter turns clockwise.
fn direction(boxes: &[[f64; 4]]) -> Option<u8> {
    let (first, last) = (boxes.first()?, boxes.last()?);
    let centre = |b: &[f64; 4]| ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
    let ((x1, y1), (x2, y2)) = (centre(first), centre(last));
    let (dx, dy) = (x2 - x1, y2 - y1);
    if boxes.len() < 2 || (dx.abs() < 0.01 && dy.abs() < 0.01) {
        // One letter: a tall, narrow box is most likely turned text.
        return None;
    }
    Some(if dx.abs() >= dy.abs() {
        if dx >= 0.0 { 0 } else { 2 }
    } else if dy >= 0.0 {
        1
    } else {
        3
    })
}

/// Whether a glyph's box, x1 y1 x2 y2, touches a redaction box, x y w h.
fn touches(glyph: [f64; 4], area: [f64; 4]) -> bool {
    let [x, y, w, h] = area;
    glyph[0] < x + w + REACH && glyph[2] > x - REACH && glyph[1] < y + h + REACH && glyph[3] > y - REACH
}

/// The glyphs that stay, and the number of words taken out. A word is a run
/// of letters between spaces; one touched anywhere goes whole.
fn keep_text(glyphs: &[(char, [f64; 4])], boxes: &[[f64; 4]]) -> (Vec<Glyph>, usize) {
    let mut kept = Vec::new();
    let mut removed = 0;
    // A word of one letter, or a space, runs the way the last word did.
    let mut way = 0;
    let mut start = 0;
    while start < glyphs.len() {
        if glyphs[start].0.is_whitespace() {
            if glyphs[start].0 == ' ' {
                kept.push((' ', glyphs[start].1, way));
            }
            start += 1;
            continue;
        }
        let end = glyphs[start..].iter().position(|(c, _)| c.is_whitespace()).map_or(glyphs.len(), |n| start + n);
        let word = &glyphs[start..end];
        if word.iter().any(|(_, area)| boxes.iter().any(|b| touches(*area, *b))) {
            removed += 1;
        } else {
            let areas: Vec<[f64; 4]> = word.iter().map(|(_, a)| *a).collect();
            way = direction(&areas).unwrap_or(way);
            kept.extend(word.iter().filter(|(c, _)| !c.is_control()).map(|(c, a)| (*c, *a, way)));
        }
        start = end;
    }
    (kept, removed)
}

/// Put the flattened page in place of the old one's every part.
fn rewrite_page(document: &Qpdf, page: Object, flat: &Flat) {
    let (width, height) = (flat.width, flat.height);
    // Photographs as JPEG, everything else without loss: text stays crisp.
    let picture = image::RgbImage::from_raw(flat.columns, flat.rows, flat.pixels.clone())
        .map(image::DynamicImage::ImageRgb8);
    let jpeg = picture
        .as_ref()
        .filter(|p| !rewrite::looks_drawn(p))
        .and_then(|p| rewrite::encode_jpeg(p, 3, 90));
    let entries = || -> Vec<(&str, Value<'_>)> {
        vec![
            ("Type", Value::Name("XObject")),
            ("Subtype", Value::Name("Image")),
            ("Width", Value::Integer(i64::from(flat.columns))),
            ("Height", Value::Integer(i64::from(flat.rows))),
            ("ColorSpace", Value::Name("DeviceRGB")),
            ("BitsPerComponent", Value::Integer(8)),
        ]
    };
    let image = match &jpeg {
        Some(jpeg) => document.new_stream(jpeg, Some("DCTDecode"), entries()),
        None => document.new_stream(&flat.pixels, None, entries()),
    };

    let mut content = format!("q\n{} 0 0 {} 0 0 cm\n/Page Do\nQ\n", number(width), number(height));
    let fonts = text_layer(document, &flat.kept, height, &mut content);

    let font_entries: Vec<(String, Object)> = fonts.into_iter().enumerate().map(|(i, f)| (format!("T{i}"), f)).collect();
    let font_dict =
        document.new_dictionary(font_entries.iter().map(|(name, font)| (name.as_str(), Value::Object(*font))).collect());
    let xobjects = document.new_dictionary(vec![("Page", Value::Object(image))]);
    let resources =
        document.new_dictionary(vec![("XObject", Value::Object(xobjects)), ("Font", Value::Object(font_dict))]);
    let contents = document.new_stream(content.as_bytes(), None, Vec::new());

    // Everything else the page had goes: annotations, thumbnails, private
    // data, boxes and turns. What is left is a page the size it was shown.
    for key in document.keys(page) {
        if key != "Type" && key != "Parent" {
            document.remove(page, &key);
        }
    }
    document.set(
        page,
        "MediaBox",
        Value::Array(vec![Value::Integer(0), Value::Integer(0), Value::Real(width), Value::Real(height)]),
    );
    document.set(page, "Resources", Value::Object(resources));
    document.set(page, "Contents", Value::Object(contents));
}

/// A number as PDF writes it.
fn number(value: f64) -> String {
    let text = format!("{value:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Write the kept glyphs into `content` as invisible text, and make the fonts
/// they use: blank glyphs of one width, each code mapped back to its letter,
/// up to 256 letters to a font.
fn text_layer(document: &Qpdf, glyphs: &[Glyph], page_height: f64, content: &mut String) -> Vec<Object> {
    let mut codes: HashMap<char, (usize, u8)> = HashMap::new();
    let mut letters: Vec<Vec<char>> = Vec::new();
    for (ch, _, _) in glyphs {
        if codes.contains_key(ch) {
            continue;
        }
        if letters.last().is_none_or(|font| font.len() == 256) {
            letters.push(Vec::new());
        }
        let font = letters.len() - 1;
        let code = letters[font].len() as u8;
        letters[font].push(*ch);
        codes.insert(*ch, (font, code));
    }
    if glyphs.is_empty() {
        return Vec::new();
    }
    content.push_str("BT\n3 Tr\n");
    for (ch, [x1, y1, x2, y2], way) in glyphs {
        let (font, code) = codes[ch];
        // In PDF's terms, measured up from the bottom of the page.
        let (top, bottom) = (page_height - y1, page_height - y2);
        let (across, along) = if way % 2 == 0 { (y2 - y1, x2 - x1) } else { (x2 - x1, y2 - y1) };
        let size = across.max(1.0);
        let stretch = 100.0 * along.max(0.1) / (size * GLYPH_WIDTH / 1000.0);
        // The baseline sits about a fifth of the way up a letter, from its
        // foot; which side that is depends on which way the text runs.
        let foot = size * 0.2;
        let matrix = match way {
            1 => format!("0 -1 1 0 {} {}", number(x1 + foot), number(top)),
            2 => format!("-1 0 0 -1 {} {}", number(*x2), number(top - foot)),
            3 => format!("0 1 -1 0 {} {}", number(x2 - foot), number(bottom)),
            _ => format!("1 0 0 1 {} {}", number(*x1), number(bottom + foot)),
        };
        let _ = writeln!(content, "/T{font} {} Tf {} Tz {matrix} Tm <{code:02X}> Tj", number(size), number(stretch));
    }
    content.push_str("ET\n");
    letters.iter().map(|chars| invisible_font(document, chars)).collect()
}

/// A Type 3 font of blank glyphs, `chars` giving what each code stands for.
fn invisible_font(document: &Qpdf, chars: &[char]) -> Object {
    let blank = document.new_stream(format!("{} 0 d0", GLYPH_WIDTH as i64).as_bytes(), None, Vec::new());
    let procs = document.new_dictionary(vec![("g", Value::Object(blank))]);
    let mut differences = vec![Value::Integer(0)];
    differences.extend((0..chars.len()).map(|_| Value::Name("g")));
    let encoding = document.new_dictionary(vec![("Type", Value::Name("Encoding")), ("Differences", Value::Array(differences))]);
    let unicode = document.new_stream(to_unicode(chars).as_bytes(), None, Vec::new());
    document.new_dictionary(vec![
        ("Type", Value::Name("Font")),
        ("Subtype", Value::Name("Type3")),
        ("FontBBox", Value::Array(vec![Value::Integer(0), Value::Integer(-200), Value::Integer(500), Value::Integer(800)])),
        (
            "FontMatrix",
            Value::Array(vec![
                Value::Real(0.001),
                Value::Integer(0),
                Value::Integer(0),
                Value::Real(0.001),
                Value::Integer(0),
                Value::Integer(0),
            ]),
        ),
        ("CharProcs", Value::Object(procs)),
        ("Encoding", Value::Object(encoding)),
        ("FirstChar", Value::Integer(0)),
        ("LastChar", Value::Integer(chars.len() as i64 - 1)),
        ("Widths", Value::Array(chars.iter().map(|_| Value::Integer(GLYPH_WIDTH as i64)).collect())),
        ("Resources", Value::Object(document.new_dictionary(Vec::new()))),
        ("ToUnicode", Value::Object(unicode)),
    ])
}

/// The map from a font's codes back to letters, so the text can be found,
/// selected and copied.
fn to_unicode(chars: &[char]) -> String {
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
         1 begincodespacerange\n<00> <FF>\nendcodespacerange\n",
    );
    for block in chars.chunks(100).enumerate() {
        let (first, letters) = block;
        let _ = writeln!(cmap, "{} beginbfchar", letters.len());
        for (i, ch) in letters.iter().enumerate() {
            let code = first * 100 + i;
            let mut units = [0u16; 2];
            let utf16: String = ch.encode_utf16(&mut units).iter().map(|u| format!("{u:04X}")).collect();
            let _ = writeln!(cmap, "<{code:02X}> <{utf16}>");
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    cmap
}

/// A tagged document's structure can carry the words of a page a second
/// time, as alternative or replacement text for screen readers; it goes.
fn forget_structure(document: &Qpdf) {
    let root = document.root();
    document.remove(root, "StructTreeRoot");
    document.remove(root, "MarkInfo");
}

/// A form field keeps its value in the form, not on the page. Fields whose
/// every widget was on a redacted page go with them.
fn prune_fields(document: &Qpdf, pages: &[Object]) {
    let root = document.root();
    let Some(form) = document.get(root, "AcroForm") else { return };
    let Some(fields) = document.get(form, "Fields").and_then(|f| document.items(f)) else { return };
    let mut shown: HashSet<(i32, i32)> = HashSet::new();
    for page in pages {
        for annotation in document.get(*page, "Annots").and_then(|a| document.items(a)).unwrap_or_default() {
            shown.insert(document.id(annotation));
        }
    }
    let kept: Vec<Value<'_>> =
        fields.into_iter().filter(|f| keep_field(document, *f, &shown, 0)).map(Value::Object).collect();
    document.set(form, "Fields", Value::Array(kept));
}

fn keep_field(document: &Qpdf, field: Object, shown: &HashSet<(i32, i32)>, depth: usize) -> bool {
    let is_widget = document.get(field, "Subtype").and_then(|s| document.name(s)).as_deref() == Some("Widget");
    if is_widget && shown.contains(&document.id(field)) {
        return true;
    }
    let Some(kids) = document.get(field, "Kids").and_then(|k| document.items(k)) else {
        // A widget no page shows any more goes; a field with no widget at
        // all was on no page, so nothing of it was redacted.
        return !is_widget;
    };
    if depth > 32 {
        return false;
    }
    let kept: Vec<Object> = kids.into_iter().filter(|k| keep_field(document, *k, shown, depth + 1)).collect();
    if kept.is_empty() {
        return false;
    }
    document.set(field, "Kids", Value::Array(kept.into_iter().map(Value::Object).collect()));
    true
}

/// Open the result and make sure each redacted page holds no letter that was
/// not kept — the guarantee, checked rather than assumed. Letters rather than
/// words, as Poppler may space the kept words a little differently.
fn check(out: &Path, password: Option<&str>, flats: &[(usize, Flat)]) -> Result<(), String> {
    let written = poppler::Document::from_file(&document::uri(out), password)
        .map_err(|e| format!("the redacted file did not open: {}", e.message()))?;
    for (page, flat) in flats {
        let Some(shown) = i32::try_from(*page).ok().and_then(|i| written.page(i)) else {
            return Err(format!("page {} is missing from the redacted file", page + 1));
        };
        if !shown.annot_mapping().is_empty() {
            return Err(format!("page {} still has annotations", page + 1));
        }
        let mut allowed: HashMap<char, usize> = HashMap::new();
        for letter in &flat.letters_kept {
            *allowed.entry(*letter).or_default() += 1;
        }
        let text = shown.text().map(|t| t.to_string()).unwrap_or_default();
        for letter in text.chars().filter(|c| !c.is_whitespace()) {
            match allowed.get_mut(&letter) {
                Some(count) if *count > 0 => *count -= 1,
                _ => return Err(format!("page {} still has text that should have gone", page + 1)),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyphs(text: &str, x: f64, y: f64) -> Vec<(char, [f64; 4])> {
        text.chars().enumerate().map(|(i, c)| (c, [x + 6.0 * i as f64, y, x + 6.0 * (i + 1) as f64, y + 12.0])).collect()
    }

    /// A page of Helvetica text, turned and cropped as asked, with a note on
    /// it, written by hand.
    fn secret_pdf(path: &Path, rotate: i32, crop: &str) {
        let content = b"BT /F1 18 Tf 72 700 Td (Name: John Secret Smith) Tj 0 -30 Td (Account 12345678 closed) Tj ET";
        let objects: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] {crop} /Rotate {rotate} /Contents 4 0 R \
                 /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>"
            )
            .into_bytes(),
            [format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(), content.to_vec(), b"\nendstream".to_vec()]
                .concat(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            b"<< /Type /Annot /Subtype /Text /Rect [300 600 320 620] /Contents (Secret note) >>".to_vec(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n", i + 1).bytes());
            out.extend(object);
            out.extend(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for offset in offsets {
            out.extend(format!("{offset:010} 00000 n \n").bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
        std::fs::write(path, out).unwrap();
    }

    /// Where a word is on the page, as Glance's selection would mark it.
    fn area_of(path: &Path, word: &str) -> [f64; 4] {
        let document = poppler::Document::from_file(&document::uri(path), None).unwrap();
        let page = document.page(0).unwrap();
        let glyphs = markup::glyphs(&page);
        let text: String = glyphs.iter().map(|(c, _)| *c).collect();
        let start = text.find(word).expect("the word is on the page");
        let start = text[..start].chars().count();
        let boxes = &glyphs[start..start + word.chars().count()];
        let x1 = boxes.iter().map(|(_, b)| b[0]).fold(f64::INFINITY, f64::min);
        let y1 = boxes.iter().map(|(_, b)| b[1]).fold(f64::INFINITY, f64::min);
        let x2 = boxes.iter().map(|(_, b)| b[2]).fold(f64::NEG_INFINITY, f64::max);
        let y2 = boxes.iter().map(|(_, b)| b[3]).fold(f64::NEG_INFINITY, f64::max);
        [x1, y1, x2 - x1, y2 - y1]
    }

    /// Every stream in the file, decoded, and the file's own bytes.
    fn everything_in(path: &Path) -> Vec<u8> {
        let document = Qpdf::read(path, None).unwrap();
        let mut all = std::fs::read(path).unwrap();
        let mut seen = HashSet::new();
        let mut stack: Vec<Object> = document.pages();
        stack.push(document.root());
        while let Some(object) = stack.pop() {
            if !seen.insert(document.id(object)) && document.id(object).0 != 0 {
                continue;
            }
            if document.is_stream(object) {
                all.extend(document.stream_data(object, true).or_else(|| document.stream_data(object, false)).unwrap_or_default());
            }
            let dict = object;
            for key in document.keys(dict) {
                if key != "Parent" {
                    stack.extend(document.get(dict, &key));
                }
            }
            if let Some(items) = document.items(object) {
                stack.extend(items);
            }
        }
        all
    }

    fn contains(haystack: &[u8], needle: &str) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle.as_bytes())
    }

    fn check_redaction(rotate: i32, crop: &str) {
        let dir = std::env::temp_dir().join(format!("glance-redact-{rotate}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (original, redacted) = (dir.join("in.pdf"), dir.join("out.pdf"));
        secret_pdf(&original, rotate, crop);
        assert!(contains(&everything_in(&original), "Secret"), "the test is only worth something if it is there");

        let secret = area_of(&original, "Secret");
        let digits = area_of(&original, "12345678");
        let outcome = redact(&original, None, &[Area { page: 0, rect: secret }, Area { page: 0, rect: digits }], &redacted)
            .unwrap();
        assert_eq!(outcome, Outcome { pages: 1, words: 2 });

        // Nothing of either word, anywhere: not in the text, not in any
        // stream, not in the note that sat on the page.
        let everything = everything_in(&redacted);
        for gone in ["Secret", "12345678", "Secret note"] {
            assert!(!contains(&everything, gone), "{gone:?} is still in the file");
        }
        let document = poppler::Document::from_file(&document::uri(&redacted), None).unwrap();
        let page = document.page(0).unwrap();
        let text = page.text().unwrap().to_string();
        assert!(!text.contains("Secret") && !text.contains("1234"), "{text:?}");
        // The rest still reads, and is found where it was.
        for kept in ["John", "Smith", "Account", "closed"] {
            assert!(text.contains(kept), "{kept:?} should still be text: {text:?}");
        }
        let found = page.find_text("Smith");
        assert_eq!(found.len(), 1);
        assert_eq!(page.size(), document::Source { uri: document::uri(&original), password: None }
            .load().unwrap().page(0).unwrap().size(), "the page is the size it was shown");

        // And where the word was, the page is black.
        let (w, h) = page.size();
        let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, w as i32, h as i32).unwrap();
        {
            let cr = cairo::Context::new(&surface).unwrap();
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.paint().unwrap();
            page.render(&cr);
        }
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().unwrap();
        let [x, y, bw, bh] = secret;
        let (cx, cy) = ((x + bw / 2.0) as usize, (y + bh / 2.0) as usize);
        let pixel = &data[cy * stride + cx * 4..cy * stride + cx * 4 + 3];
        assert_eq!(pixel, &[0, 0, 0], "the middle of the box is black");
        drop(data);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn redacted_words_are_gone_for_good() {
        check_redaction(0, "");
    }

    #[test]
    fn a_turned_and_cropped_page_is_redacted_where_it_is_seen() {
        check_redaction(90, "/CropBox [36 36 576 756]");
    }

    #[test]
    fn a_word_touched_anywhere_goes_whole() {
        // "account 12345678 closed": a box over the middle of the number.
        let line = glyphs("account 12345678 closed", 100.0, 200.0);
        let number_middle = [100.0 + 6.0 * 11.0, 202.0, 6.0, 4.0];
        let (kept, removed) = keep_text(&line, &[number_middle]);
        let text: String = kept.iter().map(|(c, _, _)| *c).collect();
        assert_eq!(text, "account  closed");
        assert!(kept.iter().all(|(_, _, way)| *way == 0), "it reads left to right");
        assert_eq!(removed, 1);
    }

    #[test]
    fn letters_beside_a_box_are_not_caught_by_it() {
        let line = glyphs("ab cd", 100.0, 200.0);
        // Ends a point before "c" starts.
        let (kept, removed) = keep_text(&line, &[[0.0, 200.0, 117.0, 12.0]]);
        assert_eq!(removed, 1, "only \"ab\"");
        assert_eq!(kept.iter().map(|(c, _, _)| *c).collect::<String>(), " cd");
    }

    #[test]
    fn letters_map_back_to_themselves() {
        let cmap = to_unicode(&['A', 'é', '😀']);
        assert!(cmap.contains("<00> <0041>"));
        assert!(cmap.contains("<01> <00E9>"));
        assert!(cmap.contains("<02> <D83DDE00>"), "beyond the first plane, as a surrogate pair");
    }

    #[test]
    fn numbers_are_written_short() {
        assert_eq!(number(612.0), "612");
        assert_eq!(number(0.5), "0.5");
        assert_eq!(number(12.3456), "12.346");
    }
}
