//! Walking the images that sit alongside the open one.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// Everything the decoders can open, by extension.
///
/// Extension rather than content sniffing on purpose: a folder of a few
/// thousand files would otherwise mean a few thousand file opens, and this has
/// to answer instantly. A file that lies about its extension simply fails to
/// decode later, with the usual message.
const EXTENSIONS: &[&str] = &[
    // raster
    "png", "jpg", "jpeg", "jpe", "jfif", "gif", "webp", "tif", "tiff", "bmp", "ico", "qoi", "pnm",
    "pbm", "pgm", "ppm", "tga", //
    // heif family
    "heic", "heif", "avif", //
    // vector
    "svg", "svgz", //
    // camera raw
    "3fr", "arw", "cr2", "cr3", "crw", "dcr", "dng", "erf", "fff", "iiq", "kdc", "mef", "mos",
    "mrw", "nef", "nrw", "orf", "pef", "raf", "raw", "rw2", "rwl", "sr2", "srf", "srw", "x3f",
];

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| EXTENSIONS.contains(&e.as_str()))
}

/// List the images beside `path`, ordered the way a file manager would show
/// them. Touches the filesystem, so call it off the main thread.
pub fn siblings(path: &Path) -> Vec<PathBuf> {
    let Some(directory) = path.parent() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };

    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_image(path) && path.is_file())
        // Canonicalised so the file that was opened can be found in here even
        // when it arrived by a symlink or a relative path.
        .map(|path| path.canonicalize().unwrap_or(path))
        .collect();

    files.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
    files
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Order digit runs by value, so IMG_2 comes before IMG_10. Plain alphabetical
/// ordering gets that backwards, which is glaring in a folder of camera files.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut left = a.chars().peekable();
    let mut right = b.chars().peekable();
    loop {
        match (left.peek().copied(), right.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                if x.is_ascii_digit() && y.is_ascii_digit() {
                    match take_number(&mut left).cmp(&take_number(&mut right)) {
                        Ordering::Equal => {}
                        other => return other,
                    }
                } else {
                    match x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase()) {
                        Ordering::Equal => {
                            left.next();
                            right.next();
                        }
                        other => return other,
                    }
                }
            }
        }
    }
}

fn take_number(chars: &mut std::iter::Peekable<std::str::Chars>) -> u128 {
    let mut value: u128 = 0;
    while let Some(digit) = chars.peek().and_then(|c| c.to_digit(10)) {
        // A pathologically long run of digits saturates rather than wrapping.
        value = value.saturating_mul(10).saturating_add(u128::from(digit));
        chars.next();
    }
    value
}

/// The images in a folder, and which one is on screen.
pub struct Playlist {
    files: Vec<PathBuf>,
    index: usize,
}

impl Playlist {
    /// `None` when there is nothing to navigate: a lone image, or a file that
    /// is somehow not in its own directory listing.
    pub fn new(files: Vec<PathBuf>, current: &Path) -> Option<Self> {
        if files.len() < 2 {
            return None;
        }
        let current = current.canonicalize().unwrap_or_else(|_| current.to_path_buf());
        let index = files.iter().position(|path| *path == current)?;
        Some(Self { files, index })
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    pub fn index(&self) -> usize {
        self.index
    }

    /// What to show once `path` is gone: the next image, or the previous one
    /// if it was the last. `None` when nothing would be left.
    pub fn neighbour_of(&self, path: &Path) -> Option<PathBuf> {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let index = self.files.iter().position(|p| *p == canonical)?;
        if self.files.len() < 2 {
            return None;
        }
        let next = if index + 1 < self.files.len() {
            index + 1
        } else {
            index - 1
        };
        Some(self.files[next].clone())
    }

    /// Jump straight to a position, for a thumbnail click.
    pub fn jump_to(&mut self, index: usize) -> Option<PathBuf> {
        let path = self.files.get(index)?.clone();
        self.index = index;
        Some(path)
    }

    /// 1-based, for showing to a person.
    pub fn position(&self) -> usize {
        self.index + 1
    }

    /// Step forwards or backwards, wrapping at both ends so neither arrow key
    /// ever becomes a dead key.
    pub fn step(&mut self, delta: isize) -> PathBuf {
        let count = self.files.len() as isize;
        let next = (self.index as isize + delta).rem_euclid(count);
        self.index = next as usize;
        self.files[self.index].clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_sort_by_value() {
        let mut names = vec!["IMG_10.jpg", "IMG_2.jpg", "IMG_1.jpg"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["IMG_1.jpg", "IMG_2.jpg", "IMG_10.jpg"]);
    }

    #[test]
    fn ordering_ignores_case() {
        assert_eq!(natural_cmp("apple.png", "Banana.png"), Ordering::Less);
    }

    #[test]
    fn neighbour_prefers_the_next_image() {
        let files = vec![PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")];
        let list = Playlist { files, index: 1 };
        // Canonicalising a path that does not exist falls back to the path
        // itself, which is what makes this work off-disk.
        assert_eq!(list.neighbour_of(Path::new("b")), Some(PathBuf::from("c")));
        assert_eq!(list.neighbour_of(Path::new("c")), Some(PathBuf::from("b")));
        assert_eq!(list.neighbour_of(Path::new("zzz")), None);
    }

    #[test]
    fn stepping_wraps_both_ways() {
        let files = vec![PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")];
        let mut list = Playlist { files, index: 0 };
        assert_eq!(list.step(-1), PathBuf::from("c"));
        assert_eq!(list.step(1), PathBuf::from("a"));
        assert_eq!(list.position(), 1);
    }
}
