// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The inspector: a panel beside the picture saying what the file is, and
//! what the camera wrote into it. It follows along as you browse.
//!
//! Reading a file's details and its EXIF happens on a thread: a slow disk or
//! a large raw file must not hold up stepping to the next picture.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::images::exif::Camera;

/// What the window already knows about the picture on screen.
#[derive(Clone, Debug)]
pub struct Picture {
    /// The format, as the header shows it: "JPEG", "RAW (preview)".
    pub label: String,
    pub width: u32,
    pub height: u32,
    /// For an animation: how many frames, and how long one loop takes.
    pub animation: Option<(usize, Duration)>,
}

/// What the file says, read on a thread.
#[derive(Default)]
struct Details {
    kind: Option<String>,
    size: Option<u64>,
    modified: Option<String>,
    created: Option<String>,
    resolution: Option<f64>,
    camera: Option<Camera>,
}

fn when(time: Option<glib::DateTime>) -> Option<String> {
    let local = time?.to_local().ok()?;
    local.format("%-d %b %Y, %H:%M").ok().map(|text| text.to_string())
}

fn gather(path: &Path) -> Details {
    let file = gio::File::for_path(path);
    let info = file
        .query_info(
            "standard::content-type,standard::size,time::modified,time::created",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .ok();
    Details {
        kind: info
            .as_ref()
            .and_then(|info| info.content_type())
            .map(|kind| gio::content_type_get_description(&kind).to_string()),
        size: info.as_ref().map(|info| info.size().max(0) as u64),
        modified: when(info.as_ref().and_then(|info| info.modification_date_time())),
        // Not every filesystem records it.
        created: when(
            info.as_ref()
                .filter(|info| info.has_attribute("time::created"))
                .and_then(|info| glib::DateTime::from_unix_utc(info.attribute_uint64("time::created") as i64).ok()),
        ),
        resolution: crate::images::resolution::read(path),
        camera: crate::images::exif::read(path),
    }
}

/// "4000 × 3000 (12.0 megapixels)"
pub fn dimensions(width: u32, height: u32) -> String {
    let megapixels = f64::from(width) * f64::from(height) / 1_000_000.0;
    if megapixels >= 0.1 {
        format!("{width} × {height} ({megapixels:.1} megapixels)")
    } else {
        format!("{width} × {height}")
    }
}

/// "24 frames, 2.4 s a loop"
pub fn animation(frames: usize, total: Duration) -> String {
    format!("{frames} frames, {:.1} s a loop", total.as_secs_f64())
}

/// A group of label-and-value rows, refilled for each picture.
struct Section {
    group: adw::PreferencesGroup,
    rows: RefCell<Vec<adw::ActionRow>>,
}

impl Section {
    fn new(title: &str) -> Self {
        Section { group: adw::PreferencesGroup::builder().title(title).build(), rows: RefCell::default() }
    }

    fn fill(&self, rows: &[(&str, String)]) {
        for row in self.rows.borrow_mut().drain(..) {
            self.group.remove(&row);
        }
        for (label, value) in rows {
            let row = adw::ActionRow::builder().title(*label).subtitle(value.as_str()).build();
            // The value is what is being looked up, so it reads as the
            // heading and can be selected and copied.
            row.add_css_class("property");
            row.set_subtitle_selectable(true);
            self.group.add(&row);
            self.rows.borrow_mut().push(row);
        }
        self.group.set_visible(!rows.is_empty());
    }
}

pub struct Inspector {
    pub root: gtk::Box,
    file: Section,
    picture: Section,
    camera: Section,
    location: Section,
    /// Said when a picture has no camera details, rather than an empty gap.
    no_camera: gtk::Label,
    /// Which request the panel is showing, so a slow answer for a picture
    /// already left behind is dropped.
    generation: Rc<Cell<u64>>,
}

impl Inspector {
    pub fn new() -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
        column.set_margin_top(12);
        column.set_margin_bottom(18);
        column.set_margin_start(12);
        column.set_margin_end(12);
        let heading = gtk::Label::new(Some("Image Info"));
        heading.add_css_class("title-4");
        heading.set_xalign(0.0);
        column.append(&heading);

        let inspector = Inspector {
            root: root.clone(),
            file: Section::new("File"),
            picture: Section::new("Picture"),
            camera: Section::new("Camera"),
            location: Section::new("Location"),
            no_camera: gtk::Label::new(Some("The file has no camera details.")),
            generation: Rc::default(),
        };
        inspector.no_camera.add_css_class("dim-label");
        inspector.no_camera.set_xalign(0.0);
        inspector.no_camera.set_wrap(true);
        column.append(&inspector.file.group);
        column.append(&inspector.picture.group);
        column.append(&inspector.camera.group);
        column.append(&inspector.no_camera);
        column.append(&inspector.location.group);

        let scroller = gtk::ScrolledWindow::builder()
            .child(&column)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .hexpand(true)
            .build();
        root.append(&scroller);
        root.set_width_request(300);
        root.set_hexpand(false);
        root.set_visible(false);
        Rc::new(inspector)
    }

    /// Show what is known about `path` at once, and the rest once read.
    pub fn show(self: &Rc<Self>, path: &Path, picture: Picture) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let folder = path.parent().map(|p| p.display().to_string()).unwrap_or_default();
        self.file.fill(&[("Name", name.clone()), ("Folder", folder.clone())]);
        self.fill_picture(&picture, None);
        self.camera.fill(&[]);
        self.location.fill(&[]);
        self.no_camera.set_visible(false);

        let (sender, receiver) = async_channel::bounded(1);
        let path: PathBuf = path.to_path_buf();
        std::thread::spawn(move || {
            let _ = sender.send_blocking(gather(&path));
        });
        let inspector = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(details) = receiver.recv().await else { return };
            let Some(inspector) = inspector.upgrade() else { return };
            if inspector.generation.get() != generation {
                return;
            }
            let mut file = vec![("Name", name), ("Folder", folder)];
            if let Some(kind) = details.kind.clone() {
                file.push(("Kind", kind));
            }
            if let Some(size) = details.size {
                file.push(("Size", glib::format_size(size).to_string()));
            }
            if let Some(modified) = details.modified.clone() {
                file.push(("Modified", modified));
            }
            if let Some(created) = details.created.clone() {
                file.push(("Created", created));
            }
            inspector.file.fill(&file);
            inspector.fill_picture(&picture, Some(&details));
            match &details.camera {
                Some(camera) => {
                    inspector.camera.fill(&camera.rows());
                    inspector.no_camera.set_visible(camera.rows().is_empty());
                    let mut place = Vec::new();
                    if let Some(location) = &camera.location {
                        place.push(("Coordinates", location.coordinates()));
                        if let Some(altitude) = location.altitude {
                            place.push(("Altitude", format!("{altitude:.0} m")));
                        }
                    }
                    inspector.location.fill(&place);
                }
                None => {
                    inspector.camera.fill(&[]);
                    inspector.no_camera.set_visible(true);
                }
            }
        });
    }

    fn fill_picture(&self, picture: &Picture, details: Option<&Details>) {
        let mut rows = vec![("Format", picture.label.clone()), ("Dimensions", dimensions(picture.width, picture.height))];
        if let Some(details) = details {
            rows.push((
                "Resolution",
                match details.resolution {
                    Some(dpi) => {
                        let inches = |pixels: u32| f64::from(pixels) / dpi;
                        format!(
                            "{dpi:.0} ppi, {:.1} × {:.1} in printed",
                            inches(picture.width),
                            inches(picture.height)
                        )
                    }
                    None => "Not recorded".to_string(),
                },
            ));
        }
        if let Some((frames, total)) = picture.animation {
            rows.push(("Animation", animation(frames, total)));
        }
        self.picture.fill(&rows);
    }

    /// Nothing to show: no picture, or a PDF.
    pub fn clear(&self) {
        self.generation.set(self.generation.get() + 1);
        for section in [&self.file, &self.picture, &self.camera, &self.location] {
            section.fill(&[]);
        }
        self.no_camera.set_visible(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_loops_are_said_plainly() {
        assert_eq!(dimensions(4000, 3000), "4000 × 3000 (12.0 megapixels)");
        assert_eq!(dimensions(16, 16), "16 × 16");
        assert_eq!(animation(24, Duration::from_millis(2400)), "24 frames, 2.4 s a loop");
    }

    #[test]
    fn a_file_s_details_are_read() {
        let dir = std::env::temp_dir().join(format!("glance-inspect-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shot.jpg");
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::new(8, 8));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Jpeg).unwrap();
        // With the camera's block put in after the start marker.
        let tiff = crate::images::exif::tests::sample(false);
        let mut jpeg = bytes.into_inner();
        let mut app1 = vec![0xFF, 0xE1];
        app1.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
        app1.extend_from_slice(b"Exif\0\0");
        app1.extend_from_slice(&tiff);
        jpeg.splice(2..2, app1);
        std::fs::write(&path, crate::images::resolution::stamp(jpeg, 300.0)).unwrap();

        let details = gather(&path);
        assert!(details.size.unwrap() > 100);
        assert!(details.modified.is_some());
        assert_eq!(details.resolution, Some(300.0));
        assert_eq!(details.camera.and_then(|c| c.model), Some("Canon EOS R6".into()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
