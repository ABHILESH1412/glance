// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! How the reader was last set up — highlight colour, night mode, page
//! layout, the sidebar's tab — remembered between runs.
//!
//! A plain file of `key=value` lines, like the theme's, rather than GSettings,
//! which would need a schema installed system-wide. Anything missing or
//! unreadable falls back to the default: losing a preference is not worth an
//! error.

use gtk::glib;

use crate::pdf::{Mode, Rgb, SidebarView};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reader {
    pub highlight: Rgb,
    pub night: bool,
    pub mode: Mode,
    pub sidebar: SidebarView,
}

impl Default for Reader {
    fn default() -> Self {
        Reader {
            highlight: Rgb(0xffff, 0xe4e4, 0x0000),
            night: false,
            mode: Mode::Continuous,
            sidebar: SidebarView::Pages,
        }
    }
}

fn file() -> std::path::PathBuf {
    glib::user_config_dir().join("glance").join("reader")
}

pub fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Continuous => "continuous",
        Mode::Single => "single",
        Mode::Double => "double",
    }
}

pub fn mode_from(name: &str) -> Option<Mode> {
    match name {
        "continuous" => Some(Mode::Continuous),
        "single" => Some(Mode::Single),
        "double" => Some(Mode::Double),
        _ => None,
    }
}

impl Reader {
    pub fn load() -> Self {
        Self::parse(&std::fs::read_to_string(file()).unwrap_or_default())
    }

    fn parse(text: &str) -> Self {
        let mut reader = Reader::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim();
            match key.trim() {
                "highlight" => reader.highlight = Rgb::from_hex(value).unwrap_or(reader.highlight),
                "night" => reader.night = value == "true",
                "layout" => reader.mode = mode_from(value).unwrap_or(reader.mode),
                "sidebar" => reader.sidebar = SidebarView::from_name(value).unwrap_or(reader.sidebar),
                _ => {}
            }
        }
        reader
    }

    fn text(&self) -> String {
        format!(
            "highlight={}\nnight={}\nlayout={}\nsidebar={}\n",
            self.highlight.hex(),
            self.night,
            mode_name(self.mode),
            self.sidebar.name()
        )
    }

    pub fn save(&self) {
        let path = file();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, self.text());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_come_back_as_they_were_saved() {
        let reader = Reader {
            highlight: Rgb(0x7f7f, 0xe3e3, 0x5a5a),
            night: true,
            mode: Mode::Double,
            sidebar: SidebarView::Contents,
        };
        assert_eq!(Reader::parse(&reader.text()), reader);
    }

    #[test]
    fn nonsense_falls_back_to_the_defaults() {
        assert_eq!(Reader::parse("highlight=blue\nlayout=sideways\nsidebar=left\nnoise"), Reader::default());
    }
}
