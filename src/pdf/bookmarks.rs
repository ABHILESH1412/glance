// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bookmarks: pages the reader has marked to come back to.
//!
//! They are the reader's, not the document's, so they are kept beside
//! Glance's other settings rather than written into the file: marking a page
//! never changes a PDF, and a document shared with someone else does not
//! carry your place in it.
//!
//! One plain file for every document, a `file` line for each followed by its
//! pages. A document is known by its location, and also by the identifier
//! most PDFs carry, so its bookmarks follow it when it is moved or renamed.

use gtk::glib;

#[derive(Clone, Debug, PartialEq)]
pub struct Bookmark {
    /// Counting from zero.
    pub page: usize,
    pub name: String,
}

/// Which document a set of bookmarks belongs to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Key {
    pub uri: String,
    /// The PDF's own permanent identifier, when it has one.
    pub id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    key: Key,
    marks: Vec<Bookmark>,
}

fn file() -> std::path::PathBuf {
    glib::user_config_dir().join("glance").join("bookmarks")
}

/// The bookmarks kept for a document, in page order.
pub fn load(key: &Key) -> Vec<Bookmark> {
    let entries = parse(&std::fs::read_to_string(file()).unwrap_or_default());
    find(&entries, key).map(|i| entries[i].marks.clone()).unwrap_or_default()
}

/// Keep a document's bookmarks, replacing what was kept for it before.
pub fn save(key: &Key, marks: &[Bookmark]) -> Result<(), String> {
    let path = file();
    let mut entries = parse(&std::fs::read_to_string(&path).unwrap_or_default());
    store(&mut entries, key, marks);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Written beside it and moved into place, so a crash halfway never
    // leaves every document's bookmarks cut short.
    let temporary = path.with_extension("new");
    std::fs::write(&temporary, format(&entries)).map_err(|e| e.to_string())?;
    std::fs::rename(&temporary, &path).map_err(|e| e.to_string())
}

/// The entry for a document: by location, or failing that, by identifier,
/// for a file that has moved.
fn find(entries: &[Entry], key: &Key) -> Option<usize> {
    entries.iter().position(|e| e.key.uri == key.uri).or_else(|| {
        let id = key.id.as_deref()?;
        entries.iter().position(|e| e.key.id.as_deref() == Some(id))
    })
}

fn store(entries: &mut Vec<Entry>, key: &Key, marks: &[Bookmark]) {
    if let Some(i) = find(entries, key) {
        entries.remove(i);
    }
    if !marks.is_empty() {
        let mut marks: Vec<Bookmark> =
            marks.iter().map(|m| Bookmark { page: m.page, name: tidy(&m.name) }).collect();
        marks.sort_by_key(|m| m.page);
        marks.dedup_by_key(|m| m.page);
        entries.push(Entry { key: key.clone(), marks });
    }
}

/// A name on one line, with no tabs to break the file's columns.
fn tidy(name: &str) -> String {
    name.split(|c: char| c.is_control()).filter(|w| !w.trim().is_empty()).map(str::trim).collect::<Vec<_>>().join(" ")
}

fn parse(text: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    for line in text.lines() {
        let mut fields = line.splitn(3, '\t');
        match (fields.next(), fields.next(), fields.next()) {
            (Some("file"), Some(uri), id) if !uri.is_empty() => {
                let id = id.filter(|id| !id.is_empty()).map(str::to_string);
                entries.push(Entry { key: Key { uri: uri.to_string(), id }, marks: Vec::new() });
            }
            (Some("page"), Some(number), name) => {
                let (Some(entry), Some(page)) =
                    (entries.last_mut(), number.parse::<usize>().ok().and_then(|n| n.checked_sub(1)))
                else {
                    continue;
                };
                let name = name.map(tidy).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Page {}", page + 1));
                entry.marks.push(Bookmark { page, name });
            }
            _ => {}
        }
    }
    entries.retain(|e| !e.marks.is_empty());
    entries
}

fn format(entries: &[Entry]) -> String {
    let mut text = String::from("# Glance bookmarks: a file, then the pages marked in it.\n");
    for entry in entries {
        text.push_str(&format!("file\t{}\t{}\n", entry.key.uri, entry.key.id.as_deref().unwrap_or("")));
        for mark in &entry.marks {
            text.push_str(&format!("page\t{}\t{}\n", mark.page + 1, tidy(&mark.name)));
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(uri: &str, id: Option<&str>) -> Key {
        Key { uri: uri.to_string(), id: id.map(str::to_string) }
    }

    fn mark(page: usize, name: &str) -> Bookmark {
        Bookmark { page, name: name.to_string() }
    }

    #[test]
    fn bookmarks_come_back_as_they_were_kept() {
        let mut entries = Vec::new();
        store(&mut entries, &key("file:///a.pdf", Some("ab12")), &[mark(9, "Results"), mark(2, "Tab\there")]);
        store(&mut entries, &key("file:///b%20c.pdf", None), &[mark(0, "Page 1")]);
        let read = parse(&format(&entries));
        assert_eq!(read, entries);
        assert_eq!(read[0].marks, vec![mark(2, "Tab here"), mark(9, "Results")], "in page order, tab gone");
    }

    #[test]
    fn a_moved_file_is_found_by_its_identifier() {
        let mut entries = Vec::new();
        store(&mut entries, &key("file:///old.pdf", Some("ab12")), &[mark(4, "Here")]);
        let moved = key("file:///new.pdf", Some("ab12"));
        assert_eq!(find(&entries, &moved), Some(0));
        // Saving under the new name takes the old entry's place.
        store(&mut entries, &moved, &[mark(4, "Here"), mark(6, "There")]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key.uri, "file:///new.pdf");
        assert_eq!(find(&entries, &key("file:///other.pdf", None)), None);
    }

    #[test]
    fn removing_the_last_bookmark_forgets_the_file() {
        let mut entries = Vec::new();
        let k = key("file:///a.pdf", None);
        store(&mut entries, &k, &[mark(1, "One")]);
        store(&mut entries, &k, &[]);
        assert!(entries.is_empty());
    }

    #[test]
    fn nonsense_lines_are_skipped() {
        let text = "page\t3\torphan\nfile\tfile:///a.pdf\t\npage\tzero\tx\npage\t0\tx\npage\t2\t\nnoise\n";
        let entries = parse(text);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key.id, None);
        assert_eq!(entries[0].marks, vec![mark(1, "Page 2")], "an unnamed page gets its number");
    }
}
