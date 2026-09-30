// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The application window: header bar, actions, and the load pipeline.
//!
//! This is a real `GObject` subclass rather than a plain struct behind an `Rc`.
//! That matters: closures capture a weak reference, and tying that reference to
//! the widget's own lifetime is what keeps it valid for as long as the window is
//! on screen.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::time::Duration;
use std::path::{Path, PathBuf};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, gio, glib};

use crate::images::edit::adjust::Adjustments;
use crate::images::view::ImageView;
use crate::app::filmstrip::{self, FilmStrip};
use crate::images::loader;
use crate::app::playlist::{self, Playlist};
use crate::images::thumbs;
use crate::images::edit::panel::{colour_button, dimension_spin, section_toggle, tone_scale};
use crate::pdf::{self, PdfView};

/// How close to an edge the pointer must get before the hidden bars slide back
/// while fullscreen.
const EDGE_REVEAL: f64 = 64.0;

/// What the header bar is currently advertising, so a failed load can restore
/// it instead of leaving a stale "Loading…" behind.
/// What a load produced. The worker decides which, by reading the file.
enum Loaded {
    Image(loader::LoadedImage),
    Pdf(pdf::Opened),
}

pub struct Shown {
    name: String,
    subtitle: String,
}

mod imp {
    use super::*;

    pub struct Window {
        pub title: adw::WindowTitle,
        pub toasts: adw::ToastOverlay,
        pub view: ImageView,
        /// The reader for PDFs, beside the image view in `content`.
        pub pdf_view: PdfView,
        /// Holds both views; only one is ever on screen.
        pub content: gtk::Stack,
        /// The document on screen is a PDF, so zoom and paging go to it.
        pub showing_pdf: Cell<bool>,
        pub toolbar: adw::ToolbarView,
        pub fullscreen_button: gtk::Button,
        pub delete_button: gtk::Button,
        pub copy_button: gtk::Button,
        pub edit_button: gtk::ToggleButton,
        pub edit_panel: gtk::Box,
        pub crop_toggle: gtk::ToggleButton,
        pub crop_options: gtk::Box,
        pub freehand_toggle: gtk::ToggleButton,
        pub resize_toggle: gtk::ToggleButton,
        pub resize_options: gtk::Box,
        pub width_spin: gtk::SpinButton,
        pub height_spin: gtk::SpinButton,
        pub keep_aspect: gtk::CheckButton,
        pub natural_label: gtk::Label,
        pub format_drop: gtk::DropDown,
        pub format_note: gtk::Label,
        pub size_wanted: gtk::CheckButton,
        pub size_value: gtk::SpinButton,
        pub size_unit: gtk::DropDown,
        pub size_note: gtk::Label,
        pub export_toggle: gtk::ToggleButton,
        pub export_options: gtk::Box,
        pub quality_row: gtk::Box,
        pub quality_scale: gtk::Scale,
        pub draw_toggle: gtk::ToggleButton,
        pub draw_options: gtk::Box,
        pub draw_tools: RefCell<Vec<gtk::ToggleButton>>,
        pub draw_colour: gtk::ColorDialogButton,
        pub draw_width: gtk::SpinButton,
        pub draw_hint: gtk::Label,
        pub text_toggle: gtk::ToggleButton,
        pub text_options: gtk::Box,
        pub text_entry: gtk::Entry,
        pub text_size: gtk::SpinButton,
        pub text_bold: gtk::ToggleButton,
        pub text_italic: gtk::ToggleButton,
        pub text_underline: gtk::ToggleButton,
        pub text_colour: gtk::ColorDialogButton,
        pub text_background: gtk::ColorDialogButton,
        pub text_font: gtk::FontDialogButton,
        pub text_hint: gtk::Label,
        pub adjust_toggle: gtk::ToggleButton,
        pub adjust_options: gtk::Box,
        pub brightness_scale: gtk::Scale,
        pub contrast_scale: gtk::Scale,
        pub saturation_scale: gtk::Scale,
        pub crop_size: gtk::Label,
        pub pending_crop: gtk::Label,
        /// Set while pushing state into the panel, so the toggles do not echo
        /// back and undo what was just applied.
        pub syncing_panel: Cell<bool>,
        /// The pixels being edited, decoded once when the panel opens. Edits
        /// are applied to this, not to the file, so the original is untouched
        /// until it is saved over.
        pub working: RefCell<Option<image::DynamicImage>>,
        /// Previous states, newest last. Real editors keep history in memory
        /// with a limit rather than writing a copy per step, so this does too.
        pub history: RefCell<Vec<image::DynamicImage>>,
        pub redo: RefCell<Vec<image::DynamicImage>>,
        pub dirty: Cell<bool>,
        pub undo_button: gtk::Button,
        pub redo_button: gtk::Button,
        /// The file on screen, needed to delete it and to find it again after
        /// the folder changes underneath us.
        pub current: RefCell<Option<PathBuf>>,
        /// Watches the folder so images added or removed elsewhere show up here.
        pub monitor: RefCell<Option<gio::FileMonitor>>,
        pub rescan_timer: RefCell<Option<glib::SourceId>>,
        pub header_stack: gtk::Stack,
        /// The second bar, under the header: the two actions that change the
        /// file itself, kept away from the view controls.
        pub action_bar: gtk::Box,
        pub rotate_button: gtk::Button,
        pub flip_h_button: gtk::ToggleButton,
        pub flip_v_button: gtk::ToggleButton,
        pub strip: FilmStrip,
        /// Thumbnails already being generated, so a slot that reappears does
        /// not queue the same decode twice.
        pub pending_thumbs: RefCell<HashSet<PathBuf>>,
        pub transform_toggle: gtk::ToggleButton,
        /// The rotate and flip controls. Once a bar along the bottom; now a
        /// section of the edit panel, because everything it does ends up in
        /// the saved pixels and that is where such things belong.
        pub rotation_bar: gtk::Box,
        pub rotation_scale: gtk::Scale,
        /// The angle, both shown and typed. An angle is a number the user
        /// often knows exactly — 90, 7, -3 to straighten a horizon — and
        /// hunting for it with a slider is no way to enter a number you know.
        pub rotation_spin: gtk::SpinButton,
        /// Set while pushing the canvas's angle into the slider, so the
        /// slider's own value-changed does not bounce it straight back.
        pub syncing: Cell<bool>,
        pub shown: RefCell<Option<Shown>>,
        /// The other images in the same folder, and where we are in them.
        pub playlist: RefCell<Option<Playlist>>,
        /// Bumped on every open so a slow decode that finishes after a newer one
        /// was started can be recognised and discarded.
        pub generation: Cell<u64>,
    }

    impl Default for Window {
        fn default() -> Self {
            Self {
                title: adw::WindowTitle::new("Glance", ""),
                toasts: adw::ToastOverlay::new(),
                view: ImageView::new(),
                pdf_view: PdfView::new(),
                content: gtk::Stack::new(),
                showing_pdf: Cell::new(false),
                toolbar: adw::ToolbarView::new(),
                fullscreen_button: gtk::Button::from_icon_name("view-fullscreen-symbolic"),
                delete_button: gtk::Button::from_icon_name("user-trash-symbolic"),
                copy_button: gtk::Button::from_icon_name("edit-copy-symbolic"),
                edit_button: gtk::ToggleButton::new(),
                edit_panel: gtk::Box::new(gtk::Orientation::Vertical, 12),
                crop_toggle: section_toggle("edit-cut-symbolic", "Crop"),
                crop_options: gtk::Box::new(gtk::Orientation::Vertical, 8),
                freehand_toggle: gtk::ToggleButton::with_label("Freehand"),
                resize_toggle: section_toggle("view-fullscreen-symbolic", "Resize"),
                resize_options: gtk::Box::new(gtk::Orientation::Vertical, 6),
                width_spin: dimension_spin(),
                height_spin: dimension_spin(),
                keep_aspect: gtk::CheckButton::with_label("Keep aspect ratio"),
                natural_label: gtk::Label::new(None),
                format_drop: gtk::DropDown::default(),
                format_note: gtk::Label::new(None),
                size_wanted: gtk::CheckButton::with_label("Aim for a file size"),
                size_value: gtk::SpinButton::with_range(1.0, 99_999.0, 10.0),
                size_unit: gtk::DropDown::default(),
                size_note: gtk::Label::new(None),
                export_toggle: section_toggle("document-send-symbolic", "Export"),
                export_options: gtk::Box::new(gtk::Orientation::Vertical, 6),
                quality_row: gtk::Box::new(gtk::Orientation::Horizontal, 6),
                quality_scale: gtk::Scale::with_range(gtk::Orientation::Horizontal, 1.0, 100.0, 1.0),
                draw_toggle: section_toggle("applications-graphics-symbolic", "Draw"),
                draw_options: gtk::Box::new(gtk::Orientation::Vertical, 6),
                draw_tools: RefCell::new(Vec::new()),
                draw_colour: colour_button(gdk::RGBA::new(0.9, 0.15, 0.15, 1.0)),
                draw_width: gtk::SpinButton::with_range(1.0, 200.0, 1.0),
                draw_hint: gtk::Label::new(None),
                text_toggle: section_toggle("insert-text-symbolic", "Text"),
                text_options: gtk::Box::new(gtk::Orientation::Vertical, 6),
                text_entry: gtk::Entry::new(),
                text_size: gtk::SpinButton::with_range(6.0, 2000.0, 1.0),
                text_bold: gtk::ToggleButton::with_label("B"),
                text_italic: gtk::ToggleButton::with_label("I"),
                text_underline: gtk::ToggleButton::with_label("U"),
                text_colour: colour_button(gdk::RGBA::WHITE),
                text_background: colour_button(gdk::RGBA::new(0.0, 0.0, 0.0, 0.0)),
                text_font: gtk::FontDialogButton::new(Some(gtk::FontDialog::new())),
                text_hint: gtk::Label::new(None),
                adjust_toggle: section_toggle("display-brightness-symbolic", "Adjust"),
                adjust_options: gtk::Box::new(gtk::Orientation::Vertical, 4),
                brightness_scale: tone_scale(),
                contrast_scale: tone_scale(),
                saturation_scale: tone_scale(),
                crop_size: gtk::Label::new(None),
                pending_crop: gtk::Label::new(None),
                syncing_panel: Cell::new(false),
                working: RefCell::new(None),
                history: RefCell::new(Vec::new()),
                redo: RefCell::new(Vec::new()),
                dirty: Cell::new(false),
                undo_button: gtk::Button::from_icon_name("edit-undo-symbolic"),
                redo_button: gtk::Button::from_icon_name("edit-redo-symbolic"),
                current: RefCell::new(None),
                monitor: RefCell::new(None),
                rescan_timer: RefCell::new(None),
                header_stack: gtk::Stack::new(),
                action_bar: gtk::Box::new(gtk::Orientation::Horizontal, 6),
                rotate_button: gtk::Button::from_icon_name("object-rotate-right-symbolic"),
                flip_h_button: gtk::ToggleButton::new(),
                flip_v_button: gtk::ToggleButton::new(),
                strip: FilmStrip::new(),
                pending_thumbs: RefCell::new(HashSet::new()),
                transform_toggle: gtk::ToggleButton::new(),
                rotation_bar: gtk::Box::new(gtk::Orientation::Vertical, 6),
                rotation_scale: gtk::Scale::with_range(
                    gtk::Orientation::Horizontal,
                    -180.0,
                    180.0,
                    1.0,
                ),
                rotation_spin: gtk::SpinButton::with_range(-180.0, 180.0, 1.0),
                syncing: Cell::new(false),
                shown: RefCell::new(None),
                playlist: RefCell::new(None),
                generation: Cell::new(0),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Window {
        const NAME: &'static str = "SvWindow";
        type Type = super::Window;
        type ParentType = adw::ApplicationWindow;
    }

    impl ObjectImpl for Window {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().build_ui();
            self.obj().install_actions();
        }
    }

    impl WidgetImpl for Window {}
    impl WindowImpl for Window {}
    impl ApplicationWindowImpl for Window {}
    impl AdwApplicationWindowImpl for Window {}
}

glib::wrapper! {
    pub struct Window(ObjectSubclass<imp::Window>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl Window {
    pub fn new(app: &adw::Application) -> Self {
        glib::Object::builder().property("application", app).build()
    }

    fn build_ui(&self) {
        let imp = self.imp();

        self.set_default_size(900, 620);
        self.set_title(Some("Glance"));

        // The edit panel lives beside the picture rather than over it, so the
        // image never sits behind the controls being used on it.
        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        // The picture takes the slack; without this the panel and the image
        // split it and the sidebar ends up twice the width it asked for.
        imp.view.widget().set_hexpand(true);
        let content = &imp.content;
        content.set_hexpand(true);
        content.add_named(imp.view.widget(), Some("image"));
        content.add_named(imp.pdf_view.widget(), Some("pdf"));
        content.set_visible_child_name("image");
        body.append(content);
        imp.pdf_view.connect_status(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |status| window.show_pdf_status(status)
        ));
        body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        body.append(self.build_edit_panel());
        imp.toasts.set_child(Some(&body));

        let open_button = gtk::Button::from_icon_name("document-open-symbolic");
        open_button.set_tooltip_text(Some("Open (Ctrl+O)"));
        open_button.set_action_name(Some("win.open"));

        let edit_section = gio::Menu::new();
        edit_section.append(Some("_Edit…"), Some("win.edit"));
        edit_section.append(Some("_Save"), Some("win.save"));
        edit_section.append(Some("_Export…"), Some("win.export"));

        let clipboard_section = gio::Menu::new();
        clipboard_section.append(Some("_Copy Image"), Some("win.copy"));

        let file_section = gio::Menu::new();
        file_section.append(Some("_Delete Image…"), Some("win.delete"));

        let view_section = gio::Menu::new();
        view_section.append(Some("_Fullscreen"), Some("win.fullscreen"));

        let theme_section = gio::Menu::new();
        theme_section.append(Some("Follow _System"), Some("app.theme::system"));
        theme_section.append(Some("_Light"), Some("app.theme::light"));
        theme_section.append(Some("_Dark"), Some("app.theme::dark"));

        let navigate_section = gio::Menu::new();
        navigate_section.append(Some("_Previous Image"), Some("win.previous-image"));
        navigate_section.append(Some("_Next Image"), Some("win.next-image"));

        let zoom_section = gio::Menu::new();
        zoom_section.append(Some("Zoom _In"), Some("win.zoom-in"));
        zoom_section.append(Some("Zoom _Out"), Some("win.zoom-out"));
        zoom_section.append(Some("_Fit to Window"), Some("win.zoom-fit"));
        zoom_section.append(Some("_Actual Size"), Some("win.zoom-actual"));

        let rotate_section = gio::Menu::new();
        rotate_section.append(Some("Rotate and _Flip…"), Some("win.transform-open"));
        // Everything below is the quick version that does not need the panel.
        rotate_section.append(Some("Rotate _Left"), Some("win.rotate-left"));
        rotate_section.append(Some("Rotate _Right"), Some("win.rotate-right"));
        rotate_section.append(Some("Flip _Horizontally"), Some("win.flip-horizontal"));
        rotate_section.append(Some("Flip _Vertically"), Some("win.flip-vertical"));
        rotate_section.append(Some("Reset Rotation"), Some("win.rotate-reset"));

        let about_section = gio::Menu::new();
        about_section.append(Some("_About Glance"), Some("win.about"));

        let menu = gio::Menu::new();
        menu.append_section(None, &clipboard_section);
        menu.append_section(None, &edit_section);
        menu.append_section(None, &file_section);
        menu.append_section(None, &view_section);
        menu.append_section(None, &navigate_section);
        menu.append_section(None, &zoom_section);
        menu.append_section(None, &rotate_section);
        menu.append_section(Some("Appearance"), &theme_section);
        menu.append_section(None, &about_section);
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Main Menu")
            .menu_model(&menu)
            .primary(true)
            .build();

        // Opens the transform options. Nothing to transform until an image is
        // loaded, so it starts switched off.
        let rotate_button = &imp.rotate_button;
        // A quick quarter turn to look at something sideways. Rotating in
        // earnest — a free angle, flips, anything that gets saved — lives in
        // the edit panel.
        rotate_button.set_tooltip_text(Some("Turn 90° to look (])"));
        rotate_button.set_action_name(Some("win.rotate-right"));
        rotate_button.set_sensitive(false);

        let copy_button = &imp.copy_button;
        copy_button.set_tooltip_text(Some("Copy Image (Ctrl+C)"));
        copy_button.set_action_name(Some("win.copy"));
        copy_button.set_sensitive(false);

        // These two act on the file rather than on the view, so they live on
        // their own bar below with a name beside the icon. Colour says which
        // is which before the label is read: amber for the reversible one,
        // red for the one that removes a file.
        let edit_button = &imp.edit_button;
        edit_button.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name("document-edit-symbolic")
                .label("Edit")
                .build(),
        ));
        edit_button.set_tooltip_text(Some("Edit this image (Ctrl+E)"));
        edit_button.add_css_class("edit-action");
        edit_button.set_sensitive(false);

        let delete_button = &imp.delete_button;
        delete_button.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name("user-trash-symbolic")
                .label("Delete")
                .build(),
        ));
        delete_button.set_tooltip_text(Some("Delete this image (Delete)"));
        delete_button.add_css_class("delete-action");
        delete_button.set_action_name(Some("win.delete"));
        delete_button.set_sensitive(false);

        let fullscreen_button = &imp.fullscreen_button;
        fullscreen_button.set_tooltip_text(Some("Fullscreen (F11)"));
        fullscreen_button.set_action_name(Some("win.fullscreen"));

        let header = adw::HeaderBar::builder()
            .title_widget(&imp.title)
            .build();
        header.pack_start(&open_button);
        header.pack_end(&menu_button);
        header.pack_end(rotate_button);
        header.pack_end(fullscreen_button);
        header.pack_end(copy_button);

        let action_bar = &imp.action_bar;
        action_bar.add_css_class("toolbar");
        action_bar.add_css_class("image-actions");
        action_bar.append(edit_button);
        action_bar.append(delete_button);
        // Nothing to act on until something is open.
        action_bar.set_visible(false);

        let header_stack = &imp.header_stack;
        header_stack.add_named(&header, Some("normal"));
        header_stack.set_visible_child_name("normal");

        let toolbar = &imp.toolbar;
        toolbar.add_top_bar(header_stack);
        toolbar.add_top_bar(action_bar);
        toolbar.set_content(Some(&imp.toasts));
        toolbar.add_bottom_bar(&imp.strip);
        self.set_content(Some(toolbar));

        // Focus moving into or out of a text box changes which keys are free;
        // `refresh_accels` says why.
        self.connect_focus_widget_notify(|window| window.refresh_accels());

        // Fullscreen means the picture and nothing else, so the bars fold away.
        self.connect_fullscreened_notify(|window| window.sync_fullscreen());

        // ...but they come back when the pointer reaches an edge, so there is
        // always a visible way out rather than only a key to guess at.
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _, y| {
                if !window.is_fullscreen() {
                    return;
                }
                let height = window.height() as f64;
                let imp = window.imp();
                imp.toolbar.set_reveal_top_bars(y < EDGE_REVEAL);
                imp.toolbar
                    .set_reveal_bottom_bars(y > height - EDGE_REVEAL * 2.0);
            }
        ));
        self.add_controller(motion);

        imp.view.canvas().connect_zoom_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |percent| window.show_zoom(percent)
        ));

        imp.strip.connect_selected(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |index| {
                let target = window.imp().playlist.borrow_mut().as_mut().and_then(|l| l.jump_to(index));
                if let Some(path) = target {
                    window.navigate_to(path, false);
                }
            }
        ));

        imp.strip.connect_needs_thumbnail(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |path| window.request_thumbnail(path)
        ));

        self.setup_drop_target();
    }

    /// Generate one thumbnail on a worker thread and hand it to the strip.
    fn request_thumbnail(&self, path: PathBuf) {
        let imp = self.imp();
        if !imp.pending_thumbs.borrow_mut().insert(path.clone()) {
            return; // Already being made.
        }

        let (sender, receiver) = async_channel::bounded(1);
        let worker_path = path.clone();
        std::thread::spawn(move || {
            let (w, h) = (filmstrip::SLOT_W, filmstrip::SLOT_H);
            // By name, not content: the strip must not open every file in a
            // folder just to learn what each one is.
            let thumbnail = if pdf::has_pdf_extension(&worker_path) {
                pdf::thumbnail(&worker_path, w, h)
            } else {
                thumbs::generate(&worker_path, w, h)
            };
            let _ = sender.send_blocking(thumbnail);
        });

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                let result = receiver.recv().await;
                window.imp().pending_thumbs.borrow_mut().remove(&path);
                let Ok(Ok(image)) = result else {
                    // A thumbnail that will not decode just stays blank; the
                    // failure is reported properly if the file is opened.
                    return;
                };
                let bytes = glib::Bytes::from_owned(image.rgba);
                let format = if image.premultiplied {
                    gdk::MemoryFormat::R8g8b8a8Premultiplied
                } else {
                    gdk::MemoryFormat::R8g8b8a8
                };
                let texture = gdk::MemoryTexture::new(
                    image.width as i32,
                    image.height as i32,
                    format,
                    &bytes,
                    image.width as usize * 4,
                );
                window.imp().strip.set_thumbnail(&path, texture.upcast_ref());
            }
        ));
    }

    /// Match the chrome and the button to whether we are fullscreen.
    fn sync_fullscreen(&self) {
        let imp = self.imp();
        let full = self.is_fullscreen();
        imp.toolbar.set_reveal_top_bars(!full);
        imp.toolbar.set_reveal_bottom_bars(!full);
        imp.fullscreen_button.set_icon_name(if full {
            "view-restore-symbolic"
        } else {
            "view-fullscreen-symbolic"
        });
        imp.fullscreen_button.set_tooltip_text(Some(if full {
            "Leave Fullscreen (F11)"
        } else {
            "Fullscreen (F11)"
        }));
    }

    /// Accepts a file dropped anywhere on the window.
    fn setup_drop_target(&self) {
        let target = gtk::DropTarget::new(glib::Type::INVALID, gdk::DragAction::COPY);
        // A file manager may hand over either a list or a single file.
        target.set_types(&[gdk::FileList::static_type(), gio::File::static_type()]);

        // Nautilus offers a plain file drag as MOVE, not COPY. GTK's default
        // handling intersects the drag's actions with this target's, finds
        // nothing in common, and silently refuses: `enter` fires, the highlight
        // appears, and then nothing happens on release. Both of the handlers
        // below are needed to get past that.
        //
        // Judge a drag on what it carries, not on the action it proposes: this
        // viewer only ever reads the file, so MOVE and COPY are the same thing
        // to it.
        target.connect_accept(|target, drop| {
            let offered = drop.formats();
            target.types().iter().any(|t| offered.types().contains(t))
        });

        // Answer COPY for every motion. That permits the drop, and because the
        // source is told the file was copied rather than moved, it does not go
        // on to delete the thing it just handed over.
        target.connect_motion(|_, _, _| gdk::DragAction::COPY);

        target.connect_enter(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[upgrade_or]
            gdk::DragAction::empty(),
            move |_, _, _| {
                window.add_css_class("drop-active");
                gdk::DragAction::COPY
            }
        ));

        target.connect_leave(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.remove_css_class("drop-active")
        ));

        target.connect_drop(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[upgrade_or]
            false,
            move |_, value, _, _| {
                window.remove_css_class("drop-active");
                // Dropping several files opens the first; this viewer shows one
                // image at a time.
                let file = match value.get::<gdk::FileList>() {
                    Ok(list) => list.files().into_iter().next(),
                    Err(_) => value.get::<gio::File>().ok(),
                };
                match file {
                    Some(file) => {
                        window.open_file(&file);
                        true
                    }
                    None => false,
                }
            }
        ));

        self.add_controller(target);
    }

    /// The header bar carries the live zoom level alongside the format and size.
    fn show_zoom(&self, percent: f64) {
        let imp = self.imp();
        if imp.showing_pdf.get() {
            return; // The PDF view reports its own zoom, with the page.
        }
        let shown = imp.shown.borrow();
        let Some(shown) = shown.as_ref() else {
            return;
        };
        imp.title
            .set_subtitle(&format!("{} · {percent:.0}%", shown.subtitle));
    }

    fn install_actions(&self) {
        let open = gio::SimpleAction::new("open", None);
        open.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.choose_file()
        ));
        self.add_action(&open);

        let about = gio::SimpleAction::new("about", None);
        about.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.show_about()
        ));
        self.add_action(&about);

        for (name, factor) in [("zoom-in", 1.3), ("zoom-out", 1.0 / 1.3)] {
            let action = gio::SimpleAction::new(name, None);
            action.connect_activate(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, _| {
                    let imp = window.imp();
                    if imp.showing_pdf.get() {
                        imp.pdf_view.zoom_by(factor);
                    } else {
                        imp.view.canvas().zoom_by(factor, None);
                    }
                }
            ));
            self.add_action(&action);
        }

        for (name, degrees) in [("rotate-left", -90.0), ("rotate-right", 90.0)] {
            let action = gio::SimpleAction::new(name, None);
            action.connect_activate(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, _| window.imp().view.canvas().rotate_by(degrees)
            ));
            self.add_action(&action);
        }

        let rotate_reset = gio::SimpleAction::new("rotate-reset", None);
        rotate_reset.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.imp().view.canvas().set_rotation(0.0)
        ));
        self.add_action(&rotate_reset);

        for (name, delta) in [("next-image", 1isize), ("previous-image", -1isize)] {
            let action = gio::SimpleAction::new(name, None);
            // Enabled once a folder with more than one image is known.
            action.set_enabled(false);
            action.connect_activate(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, _| {
                    let next = window.imp().playlist.borrow_mut().as_mut().map(|l| l.step(delta));
                    if let Some(path) = next {
                        window.navigate_to(path, false);
                    }
                }
            ));
            self.add_action(&action);
        }

        self.imp().edit_button.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                let open = button.is_active();
                window.imp().edit_panel.set_visible(open);
                if open {
                    // Decode once, when editing actually starts, rather than
                    // holding a full-resolution buffer for every image browsed.
                    window.load_working();
                } else {
                    // Leaving the panel puts the tools away with it.
                    window.imp().crop_toggle.set_active(false);
                    window.imp().adjust_toggle.set_active(false);
                    window.imp().resize_toggle.set_active(false);
                    window.imp().text_toggle.set_active(false);
                    window.imp().draw_toggle.set_active(false);
                }
                // Deleting the file you are in the middle of editing is a
                // trap, so it goes away along with the filmstrip.
                let has_file = window.imp().current.borrow().is_some();
                window.imp().delete_button.set_sensitive(!open && has_file);
                window.update_navigation();
            }
        ));

        // Moving through a PDF. These only have keys while one is open; see
        // `PDF_KEYS` in main.rs.
        let paging: [(&str, fn(&PdfView)); 6] = [
            ("page-down", |view| view.scroll_pages(1.0)),
            ("page-up", |view| view.scroll_pages(-1.0)),
            ("line-down", |view| view.scroll_lines(1.0)),
            ("line-up", |view| view.scroll_lines(-1.0)),
            ("page-first", PdfView::scroll_to_start),
            ("page-last", PdfView::scroll_to_end),
        ];
        for (name, step) in paging {
            let action = gio::SimpleAction::new(name, None);
            action.connect_activate(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, _| {
                    if window.imp().showing_pdf.get() {
                        step(&window.imp().pdf_view);
                    }
                }
            ));
            self.add_action(&action);
        }

        let edit = gio::SimpleAction::new("edit", None);
        edit.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                let button = &window.imp().edit_button;
                if button.is_sensitive() {
                    button.set_active(!button.is_active());
                }
            }
        ));
        self.add_action(&edit);

        for name in ["undo", "redo"] {
            let action = gio::SimpleAction::new(name, None);
            action.set_enabled(false);
            action.connect_activate(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, _| {
                    if name == "undo" {
                        // The newest thing first: having just drawn a stroke,
                        // undo should take that back, not a crop from before
                        // it. Anything still unbaked is newer than everything
                        // baked, because baking clears it.
                        if !window.imp().view.canvas().undo_overlay() {
                            window.undo();
                        }
                    } else {
                        window.redo();
                    }
                }
            ));
            self.add_action(&action);
        }

        let edit_cancel = gio::SimpleAction::new("edit-cancel", None);
        edit_cancel.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.cancel_editing()
        ));
        self.add_action(&edit_cancel);

        // Opening the tool from the header: the panel comes with it, because
        // the numbers live there.
        let resize = gio::SimpleAction::new("resize", None);
        resize.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                let imp = window.imp();
                if !imp.edit_button.is_sensitive() {
                    return;
                }
                imp.edit_button.set_active(true);
                imp.resize_toggle.set_active(true);
            }
        ));
        self.add_action(&resize);

        let resize_reset = gio::SimpleAction::new("resize-reset", None);
        resize_reset.set_enabled(false);
        resize_reset.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                window.imp().view.canvas().reset_size();
                window.sync_resize_panel();
            }
        ));
        self.add_action(&resize_reset);

        let resize_apply = gio::SimpleAction::new("resize-apply", None);
        resize_apply.set_enabled(false);
        resize_apply.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.bake(None)
        ));
        self.add_action(&resize_apply);

        let export = gio::SimpleAction::new("export", None);
        export.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.export()
        ));
        self.add_action(&export);

        let draw_clear = gio::SimpleAction::new("draw-clear", None);
        draw_clear.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.imp().view.canvas().clear_marks()
        ));
        self.add_action(&draw_clear);

        let text_add = gio::SimpleAction::new("text-add", None);
        text_add.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                let item = window.text_from_panel();
                window.imp().view.canvas().add_text(item);
            }
        ));
        self.add_action(&text_add);

        let text_remove = gio::SimpleAction::new("text-remove", None);
        text_remove.set_enabled(false);
        text_remove.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.imp().view.canvas().remove_selected_text()
        ));
        self.add_action(&text_remove);

        let adjust_reset = gio::SimpleAction::new("adjust-reset", None);
        adjust_reset.set_enabled(false);
        adjust_reset.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                window.imp().view.canvas().set_adjustments(Adjustments::default());
                window.sync_tone_panel();
            }
        ));
        self.add_action(&adjust_reset);

        let adjust_apply = gio::SimpleAction::new("adjust-apply", None);
        adjust_apply.set_enabled(false);
        adjust_apply.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.bake(None)
        ));
        self.add_action(&adjust_apply);

        let crop_apply = gio::SimpleAction::new("crop-apply", None);
        crop_apply.set_enabled(false);
        crop_apply.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.commit_crop()
        ));
        self.add_action(&crop_apply);

        let crop_reset = gio::SimpleAction::new("crop-reset", None);
        crop_reset.set_enabled(false);
        crop_reset.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                window.imp().freehand_toggle.set_active(false);
                window.imp().view.canvas().reset_crop();
            }
        ));
        self.add_action(&crop_reset);

        let save = gio::SimpleAction::new("save", None);
        save.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.save_default()
        ));
        self.add_action(&save);


        let copy = gio::SimpleAction::new("copy", None);
        copy.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                // Copying is of a picture; a PDF page is not one yet.
                if !window.imp().showing_pdf.get() {
                    window.copy_to_clipboard();
                }
            }
        ));
        self.add_action(&copy);

        let delete = gio::SimpleAction::new("delete", None);
        delete.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.confirm_delete()
        ));
        self.add_action(&delete);

        let fullscreen = gio::SimpleAction::new("fullscreen", None);
        fullscreen.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                if window.is_fullscreen() {
                    window.unfullscreen();
                } else {
                    window.fullscreen();
                }
            }
        ));
        self.add_action(&fullscreen);

        // Escape should undo whatever is currently "on top": leaving fullscreen
        // first, then the transform options, then the editor. The editor is
        // last because it is the only one that asks before it goes.
        let dismiss = gio::SimpleAction::new("dismiss", None);
        dismiss.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                if window.is_fullscreen() {
                    window.unfullscreen();
                } else {
                    window.cancel_editing();
                }
            }
        ));
        self.add_action(&dismiss);

        let transform_open = gio::SimpleAction::new("transform-open", None);
        transform_open.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.set_transform_open(true)
        ));
        self.add_action(&transform_open);


        let flip_h = gio::SimpleAction::new("flip-horizontal", None);
        flip_h.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.imp().view.canvas().toggle_flip_horizontal()
        ));
        self.add_action(&flip_h);

        let flip_v = gio::SimpleAction::new("flip-vertical", None);
        flip_v.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.imp().view.canvas().toggle_flip_vertical()
        ));
        self.add_action(&flip_v);

        let zoom_fit = gio::SimpleAction::new("zoom-fit", None);
        zoom_fit.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                let imp = window.imp();
                if imp.showing_pdf.get() {
                    imp.pdf_view.zoom_fit();
                } else {
                    imp.view.canvas().zoom_fit();
                }
            }
        ));
        self.add_action(&zoom_fit);

        let zoom_actual = gio::SimpleAction::new("zoom-actual", None);
        zoom_actual.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                let imp = window.imp();
                if imp.showing_pdf.get() {
                    imp.pdf_view.zoom_actual();
                } else {
                    imp.view.canvas().zoom_actual();
                }
            }
        ));
        self.add_action(&zoom_actual);

        let close = gio::SimpleAction::new("close", None);
        close.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.close()
        ));
        self.add_action(&close);
    }

    fn choose_file(&self) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Images and PDFs"));
        for mime in [
            "image/png",
            "image/jpeg",
            "image/gif",
            "image/webp",
            "image/tiff",
            "image/bmp",
            "image/vnd.microsoft.icon",
            "image/x-portable-anymap",
            "image/x-tga",
            "image/qoi",
            "image/heif",
            "image/heic",
            "image/avif",
            "image/svg+xml",
            "application/pdf",
        ] {
            filter.add_mime_type(mime);
        }
        // Raw formats are matched by suffix: the shared-mime database does not
        // recognise every camera maker's container, and a raw file the picker
        // greys out is a raw file the user cannot open.
        for suffix in [
            "3fr", "arw", "cr2", "cr3", "crw", "dcr", "dng", "erf", "fff", "iiq", "kdc", "mef",
            "mos", "mrw", "nef", "nrw", "orf", "pef", "raf", "raw", "rw2", "rwl", "sr2", "srf",
            "srw", "x3f", "heic", "heif", "avif", "svg", "svgz", "pdf",
        ] {
            filter.add_suffix(suffix);
        }

        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);

        let dialog = gtk::FileDialog::builder()
            .title("Open")
            .modal(true)
            .filters(&filters)
            .default_filter(&filter)
            .build();

        dialog.open(
            Some(self),
            gio::Cancellable::NONE,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |result| match result {
                    Ok(file) => window.open_file(&file),
                    Err(error) => {
                        // Closing the dialog is not a failure worth reporting.
                        if !error.matches(gtk::DialogError::Dismissed) {
                            window.toast(&format!("Could not open file: {error}"));
                        }
                    }
                }
            ),
        );
    }

    pub fn open_file(&self, file: &gio::File) {
        let Some(path) = file.path() else {
            self.toast("That location is not a local file.");
            return;
        };
        // Opened from outside, so the folder it lives in is new to us.
        self.navigate_to(path, true);
    }

    /// Go to another image, checking first that nothing unsaved is lost.
    fn navigate_to(&self, path: PathBuf, rescan: bool) {
        if !self.imp().dirty.get() {
            self.load(path, rescan);
            return;
        }
        let dialog = adw::AlertDialog::new(
            Some("Discard Changes?"),
            Some("This image has edits that have not been saved."),
        );
        dialog.add_response("cancel", "Keep Editing");
        dialog.add_response("discard", "Discard");
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.connect_response(
            None,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, response| {
                    if response == "discard" {
                        window.load(path.clone(), rescan);
                    }
                }
            ),
        );
        dialog.present(Some(self));
    }

    /// `rescan` reads the folder listing again. Stepping through that listing
    /// does not need it; arriving at a new folder does.
    pub(crate) fn load(&self, path: PathBuf, rescan: bool) {
        let imp = self.imp();

        let generation = imp.generation.get() + 1;
        imp.generation.set(generation);

        imp.current.replace(Some(path.clone()));
        if rescan {
            self.watch_folder(&path);
        }

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Image".to_string());
        imp.title.set_title(&name);
        imp.title.set_subtitle("Loading…");
        imp.content.set_visible_child_name("image");
        imp.view.show_loading();

        let (sender, receiver) = async_channel::bounded(1);
        let scan_path = path.clone();
        std::thread::spawn(move || {
            // Both the decode and the directory listing are filesystem work, so
            // they belong on this side of the channel.
            let decoded = if pdf::is_pdf(&scan_path) {
                pdf::open(&scan_path).map(Loaded::Pdf)
            } else {
                loader::decode(&scan_path).map(Loaded::Image)
            };
            let siblings = rescan.then(|| playlist::siblings(&scan_path));
            let _ = sender.send_blocking((decoded, siblings));
        });

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                let Ok((result, siblings)) = receiver.recv().await else {
                    window.fail("The image loader stopped unexpectedly.");
                    return;
                };
                if window.imp().generation.get() != generation {
                    return; // Superseded by a newer open.
                }

                if let Some(files) = siblings {
                    window.imp().playlist.replace(Playlist::new(files, &path));
                    window.update_navigation();
                    match window.imp().playlist.borrow().as_ref() {
                        Some(list) => window.imp().strip.set_playlist(list.files(), list.index()),
                        None => window.imp().strip.clear(),
                    }
                } else if let Some(list) = window.imp().playlist.borrow().as_ref() {
                    // Same folder, new position: no need to re-clone the listing.
                    window.imp().strip.set_index(list.index());
                }

                match result {
                    Ok(Loaded::Pdf(opened)) => window.show_pdf(opened, name),
                    Ok(Loaded::Image(image)) => {
                        window.leave_pdf();
                        // A vector's size is its natural size, not whatever
                        // resolution it happened to be rasterised at first.
                        let (shown_w, shown_h) = image
                            .vector
                            .as_ref()
                            .map(|v| (v.width.round() as u32, v.height.round() as u32))
                            .unwrap_or((image.width, image.height));
                        // The position lives in the filmstrip, not here.
                        let subtitle = format!("{} · {shown_w} × {shown_h}", image.label);
                        window.imp().title.set_subtitle(&subtitle);
                        window.imp().shown.replace(Some(Shown { name, subtitle }));
                        window.imp().view.show_image(image);
                        window.imp().rotate_button.set_sensitive(true);
                        window.imp().delete_button.set_sensitive(true);
                        window.imp().edit_button.set_sensitive(true);
                        window.imp().copy_button.set_sensitive(true);
                        window.describe_target_size();
                        window.imp().action_bar.set_visible(true);
                        // The canvas drops the old selection, so put the panel
                        // back in step with it.
                        let imp = window.imp();
                        imp.syncing_panel.set(true);
                        imp.crop_toggle.set_active(false);
                        imp.freehand_toggle.set_active(false);
                        imp.syncing_panel.set(false);
                        imp.crop_options.set_visible(false);
                        imp.pending_crop.set_visible(false);
                        window.update_crop_actions();
                        // Edits belong to the image they were made on.
                        window.reset_editing();
                        if imp.edit_button.is_active() {
                            window.load_working();
                        }
                    }
                    Err(message) => window.fail(&message),
                }
            }
        ));
    }

    /// Ask before removing anything, and keep the two kinds of removal clearly
    /// apart: the bin is recoverable, deleting is not.
    fn confirm_delete(&self) {
        let Some(path) = self.imp().current.borrow().clone() else {
            return;
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "this image".to_string());

        let dialog = adw::AlertDialog::new(
            Some("Delete Image?"),
            Some(&format!("“{name}” will be removed from this folder.")),
        );
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("trash", "Move to Bin");
        dialog.add_response("delete", "Delete Permanently");
        dialog.set_response_appearance("trash", adw::ResponseAppearance::Suggested);
        // Marked destructive because it cannot be undone.
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        // Escape or clicking away cancels, and Cancel is what is focused.
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        dialog.connect_response(
            None,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, response| match response {
                    "trash" => window.remove_current(path.clone(), false),
                    "delete" => window.remove_current(path.clone(), true),
                    _ => {}
                }
            ),
        );
        dialog.present(Some(self));
    }

    fn remove_current(&self, path: PathBuf, permanent: bool) {
        let imp = self.imp();
        // Worked out before the file goes, while the listing still has it.
        let replacement = imp
            .playlist
            .borrow()
            .as_ref()
            .and_then(|list| list.neighbour_of(&path));
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "this image".to_string());
        let file = gio::File::for_path(&path);

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                // Off the main loop: trashing can mean a copy across devices.
                let result = if permanent {
                    file.delete_future(glib::Priority::DEFAULT).await
                } else {
                    file.trash_future(glib::Priority::DEFAULT).await
                };

                match result {
                    Ok(()) => {
                        match replacement {
                            Some(next) => window.load(next, true),
                            None => window.show_empty(),
                        }
                        window.toast(if permanent {
                            "Image deleted."
                        } else {
                            "Image moved to the bin."
                        });
                    }
                    Err(error) => {
                        window.toast(&format!("Could not remove “{name}”: {error}"));
                    }
                }
            }
        ));
    }

    /// Watch the folder so images added or removed elsewhere are reflected here.
    fn watch_folder(&self, path: &Path) {
        let Some(directory) = path.parent() else {
            return;
        };
        let Ok(monitor) = gio::File::for_path(directory)
            .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        else {
            return;
        };
        monitor.connect_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _, _, event| {
                use gio::FileMonitorEvent as Event;
                if matches!(
                    event,
                    Event::Created
                        | Event::Deleted
                        | Event::MovedIn
                        | Event::MovedOut
                        | Event::Renamed
                        | Event::Moved
                ) {
                    window.schedule_rescan();
                }
            }
        ));
        // Replaces any previous watch; only one folder is ever open.
        self.imp().monitor.replace(Some(monitor));
    }

    /// Copying a batch of files in fires an event per file, so wait for the
    /// flurry to stop rather than re-reading the directory each time.
    fn schedule_rescan(&self) {
        let imp = self.imp();
        if let Some(timer) = imp.rescan_timer.take() {
            timer.remove();
        }
        let id = glib::timeout_add_local_once(
            Duration::from_millis(400),
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move || {
                    window.imp().rescan_timer.replace(None);
                    window.rescan_folder();
                }
            ),
        );
        imp.rescan_timer.replace(Some(id));
    }

    fn rescan_folder(&self) {
        let Some(current) = self.imp().current.borrow().clone() else {
            return;
        };
        let (sender, receiver) = async_channel::bounded(1);
        let scan_path = current.clone();
        std::thread::spawn(move || {
            let _ = sender.send_blocking(playlist::siblings(&scan_path));
        });

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                let Ok(files) = receiver.recv().await else {
                    return;
                };
                window.apply_listing(files, &current);
            }
        ));
    }

    /// Fold a fresh directory listing into the view.
    fn apply_listing(&self, files: Vec<PathBuf>, current: &Path) {
        let imp = self.imp();
        // Ignore a listing for a file we have since navigated away from.
        if imp.current.borrow().as_deref() != Some(current) {
            return;
        }

        let canonical = current
            .canonicalize()
            .unwrap_or_else(|_| current.to_path_buf());
        if !files.iter().any(|p| *p == canonical) {
            // The image on screen has gone from the folder. Show whatever was
            // next to it, or nothing if the folder is now empty.
            let replacement = imp
                .playlist
                .borrow()
                .as_ref()
                .and_then(|list| list.neighbour_of(current))
                .or_else(|| files.first().cloned());
            match replacement {
                Some(next) => self.load(next, true),
                None => self.show_empty(),
            }
            return;
        }

        imp.playlist.replace(Playlist::new(files, current));
        self.update_navigation();
        match imp.playlist.borrow().as_ref() {
            Some(list) => imp.strip.set_playlist(list.files(), list.index()),
            None => imp.strip.clear(),
        }
    }

    /// Back to the state before anything was opened.
    fn show_empty(&self) {
        let imp = self.imp();
        imp.current.replace(None);
        imp.playlist.replace(None);
        imp.shown.replace(None);
        imp.monitor.replace(None);
        imp.strip.clear();
        imp.view.canvas().set_texture(None);
        imp.view.show_idle();
        self.leave_pdf();
        imp.title.set_title("Glance");
        imp.title.set_subtitle("");
        imp.rotate_button.set_sensitive(false);
        imp.delete_button.set_sensitive(false);
        imp.copy_button.set_sensitive(false);
        // Turning the toggle off restores the filmstrip and the arrow keys.
        imp.edit_button.set_active(false);
        imp.edit_button.set_sensitive(false);
        imp.action_bar.set_visible(false);
        self.update_navigation();
    }

    /// Navigation is pointless with one image; while the transform options are
    /// open the arrow keys belong to the rotation slider; and while editing the
    /// picture on screen is unsaved work, so stepping off it — by key, by arrow
    /// or by thumbnail — is switched off and the filmstrip goes with it.
    fn update_navigation(&self) {
        let imp = self.imp();
        let busy = imp.edit_button.is_active();
        let enabled = imp.playlist.borrow().is_some() && !busy;
        for name in ["next-image", "previous-image"] {
            if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(enabled);
            }
        }
        // Told, not set: a folder rescan rebuilds the strip, and it must not
        // be able to put itself back on screen while the editor is open.
        imp.strip.set_allowed(enabled);
    }

    fn fail(&self, message: &str) {
        let imp = self.imp();
        match imp.shown.borrow().as_ref() {
            Some(shown) => {
                imp.title.set_title(&shown.name);
                imp.title.set_subtitle(&shown.subtitle);
            }
            None => {
                imp.title.set_title("Glance");
                imp.title.set_subtitle("");
            }
        }
        imp.view.show_idle();
        // A failed open leaves whatever was on screen before, PDF or not.
        imp.content.set_visible_child_name(if imp.showing_pdf.get() { "pdf" } else { "image" });
        self.toast(message);
    }

    pub(crate) fn toast(&self, message: &str) {
        self.imp().toasts.add_toast(adw::Toast::new(message));
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name("Glance")
            // The program's own icon. It is found either from the installed
            // icon theme or, when this has not been installed at all, from the
            // copy compiled into the binary — see data/glance.gresource.xml.
            // There is no graceful fallback here: an icon name GTK cannot
            // resolve draws the broken-image placeholder.
            .application_icon(crate::APP_ID)
            .version(env!("CARGO_PKG_VERSION"))
            .comments("Look at pictures, and make small changes to them.")
            .developer_name("Abhilesh Singh")
            .developers(vec!["Abhilesh Singh".to_string()])
            .copyright("© 2026 Abhilesh Singh")
            .license_type(gtk::License::Gpl30)
            .website("https://github.com/ABHILESH1412/glance")
            // A form rather than the issue tracker: most people who hit a bug
            // in an image viewer do not have a GitHub account and will not make
            // one to tell you about it.
            .issue_url(
                "https://docs.google.com/forms/d/e/\
                 1FAIpQLScaT101kS47nEdg55rp-HUFi0DprDgKlXr6fPfWthmQAHERrg/viewform\
                 ?usp=publish-editor",
            )
            .build();
        about.present(Some(self));
    }

    /// Show an opened PDF in place of whatever image was on screen.
    fn show_pdf(&self, opened: pdf::Opened, name: String) {
        let imp = self.imp();
        // Editing is for images. Anything unsaved was already asked about
        // before the load began.
        self.reset_editing();
        imp.edit_button.set_active(false);
        // Let go of the last picture: nothing on screen needs it now.
        imp.view.canvas().set_texture(None);

        let pages = opened.pages.len();
        let subtitle = format!("PDF · {pages} {}", if pages == 1 { "page" } else { "pages" });
        imp.title.set_subtitle(&subtitle);
        imp.shown.replace(Some(Shown { name, subtitle }));
        imp.showing_pdf.set(true);
        imp.pdf_view.show(opened);
        imp.content.set_visible_child_name("pdf");

        imp.rotate_button.set_sensitive(false);
        imp.copy_button.set_sensitive(false);
        imp.edit_button.set_sensitive(false);
        imp.delete_button.set_sensitive(true);
        imp.action_bar.set_visible(true);
        self.refresh_accels();
    }

    /// Back to images: stop drawing pages and free them.
    fn leave_pdf(&self) {
        let imp = self.imp();
        if imp.showing_pdf.replace(false) {
            imp.pdf_view.clear();
            self.refresh_accels();
        }
        imp.content.set_visible_child_name("image");
    }

    fn show_pdf_status(&self, status: pdf::Status) {
        let imp = self.imp();
        if imp.showing_pdf.get() {
            imp.title.set_subtitle(&format!(
                "PDF · page {} of {} · {:.0}%",
                status.page, status.pages, status.percent
            ));
        }
    }

    /// A window claims its shortcuts before the focused widget sees the key,
    /// so while a text box has focus the bare keys are withdrawn; without
    /// that, typing 300 lands as 3 and Delete removes the file. A PDF borrows
    /// the paging keys from the folder navigation.
    pub(crate) fn refresh_accels(&self) {
        let typing = gtk::prelude::GtkWindowExt::focus(self).is_some_and(|widget| widget.is::<gtk::Editable>());
        if let Some(app) = self.application().and_downcast::<adw::Application>() {
            crate::apply_accels(&app, typing, self.imp().showing_pdf.get());
        }
    }
}
