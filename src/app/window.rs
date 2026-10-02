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

use crate::images::view::ImageView;
use crate::app::filmstrip::{self, FilmStrip};
use crate::app::menus;
use crate::app::prefs;
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
    /// A PDF that needs a password; `tried` when the one given was wrong.
    Locked { tried: bool },
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
        /// Page thumbnails beside the reader, for PDFs.
        pub split: adw::OverlaySplitView,
        pub sidebar_button: gtk::ToggleButton,
        /// The page number box in the header, and "of 20" beside it.
        pub page_box: gtk::Box,
        pub page_entry: gtk::Entry,
        pub page_total: gtk::Label,
        /// Where the reader is, for putting the page box back after a typo.
        pub pdf_status: Cell<Option<pdf::Status>>,
        pub menu_button: gtk::MenuButton,
        /// Finding text in a PDF: the bar under the header, and its parts.
        pub search_bar: gtk::SearchBar,
        pub search_entry: gtk::SearchEntry,
        pub search_count: gtk::Label,
        pub search_previous: gtk::Button,
        pub search_next: gtk::Button,
        pub search_button: gtk::ToggleButton,
        pub pdf_tools: std::cell::OnceCell<crate::app::pdf_tools::PdfTools>,
        /// Highlight colour, night mode and layout, as last chosen.
        pub reader_prefs: Cell<prefs::Reader>,
        /// The main menu differs by document: an image's has editing and
        /// flipping in it, a PDF's has its pages.
        pub image_menu: gio::Menu,
        pub pdf_menu: gio::Menu,
        /// Holds both views; only one is ever on screen.
        pub content: gtk::Stack,
        /// The document on screen is a PDF, so zoom and paging go to it.
        pub showing_pdf: Cell<bool>,
        /// Asks for a protected PDF's password.
        pub locked: crate::app::locked::LockedPage,
        /// The window's two faces: viewing, and combining pages into a PDF.
        pub faces: gtk::Stack,
        pub combine: crate::app::combine::Combine,
        pub combining: Cell<bool>,
        /// Says how many areas are marked for redaction, and applies them.
        pub redaction_banner: adw::Banner,
        /// A picture's redactions are in its working pixels, not yet saved.
        pub redactions_baked: Cell<bool>,
        /// The password to try on the next PDF opened, then forgotten.
        pub password: RefCell<Option<String>>,
        /// The page to show when the next PDF opens: the one being read
        /// before the file was rewritten.
        pub reopen_at: Cell<Option<usize>>,
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
        /// What the width and height boxes count in.
        pub resize_unit: gtk::DropDown,
        pub resolution_spin: gtk::SpinButton,
        pub resample: gtk::CheckButton,
        /// The resolution the file records, in pixels per inch, if any.
        pub dpi_file: Cell<Option<f64>>,
        /// The resolution the picture will be saved with, if any.
        pub dpi: Cell<Option<f64>>,
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
        pub draw_colour: crate::app::colour::ColourButton,
        pub draw_width: gtk::SpinButton,
        pub draw_hint: gtk::Label,
        pub text_toggle: gtk::ToggleButton,
        pub text_options: gtk::Box,
        /// Marking areas of the picture for redaction.
        pub redact_toggle: gtk::ToggleButton,
        pub redact_options: gtk::Box,
        pub text_entry: gtk::Entry,
        pub text_size: gtk::SpinButton,
        pub text_bold: gtk::ToggleButton,
        pub text_italic: gtk::ToggleButton,
        pub text_underline: gtk::ToggleButton,
        pub text_colour: crate::app::colour::ColourButton,
        pub text_background: crate::app::colour::ColourButton,
        pub text_font: gtk::FontDialogButton,
        pub text_hint: gtk::Label,
        pub adjust_toggle: gtk::ToggleButton,
        pub adjust_options: gtk::Box,
        pub brightness_scale: gtk::Scale,
        pub contrast_scale: gtk::Scale,
        pub saturation_scale: gtk::Scale,
        pub exposure_scale: gtk::Scale,
        pub highlights_scale: gtk::Scale,
        pub shadows_scale: gtk::Scale,
        pub temperature_scale: gtk::Scale,
        pub tint_scale: gtk::Scale,
        pub sepia_scale: gtk::Scale,
        pub sharpness_scale: gtk::Scale,
        /// The panel saying what the picture on screen is, and its toggle.
        pub inspector: std::rc::Rc<crate::app::inspector::Inspector>,
        pub info_button: gtk::ToggleButton,
        /// What is known of the picture on screen, for the inspector.
        pub picture: RefCell<Option<crate::app::inspector::Picture>>,
        /// An animation's frames, in the sidebar.
        pub frames: std::rc::Rc<crate::app::frames::FramesPane>,
        /// Whether the frames were last left open, to open them again for
        /// the next animation.
        pub frames_wanted: Cell<bool>,
        /// The slideshow: running, paused, the pointer on its controls.
        pub slideshow: Cell<bool>,
        pub slideshow_paused: Cell<bool>,
        pub slideshow_hovered: Cell<bool>,
        /// Moves on to the next picture.
        pub slideshow_timer: RefCell<Option<glib::SourceId>>,
        /// Hides the controls and the pointer when left alone.
        pub slideshow_linger: RefCell<Option<glib::SourceId>>,
        pub slideshow_controls: gtk::Revealer,
        pub slideshow_play: gtk::Button,
        pub slideshow_position: gtk::Label,
        pub slideshow_every: gtk::DropDown,
        /// The inspector, the sidebar and full screen as they were before.
        pub slideshow_restore: Cell<(bool, bool, bool)>,
        pub levels_toggle: gtk::ToggleButton,
        pub levels_options: gtk::Box,
        pub levels_graph: crate::images::edit::levels::LevelsGraph,
        pub levels_channel: gtk::DropDown,
        pub levels_black: gtk::SpinButton,
        pub levels_gamma: gtk::SpinButton,
        pub levels_white: gtk::SpinButton,
        pub levels_auto: gtk::Button,
        /// Says the colours have levels of their own, when the RGB handles
        /// cannot show them.
        pub levels_note: gtk::Label,
        /// Works out tone the GPU cannot show, and the histogram.
        pub tone_worker: RefCell<Option<crate::images::edit::tone::ToneWorker>>,
        /// Waits for a slider to rest before the whole picture is worked out.
        pub tone_timer: RefCell<Option<glib::SourceId>>,
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
                split: adw::OverlaySplitView::new(),
                sidebar_button: gtk::ToggleButton::new(),
                page_box: gtk::Box::new(gtk::Orientation::Horizontal, 6),
                page_entry: gtk::Entry::new(),
                page_total: gtk::Label::new(None),
                pdf_status: Cell::new(None),
                menu_button: gtk::MenuButton::new(),
                search_bar: gtk::SearchBar::new(),
                search_entry: gtk::SearchEntry::new(),
                search_count: gtk::Label::new(None),
                search_previous: gtk::Button::from_icon_name("go-up-symbolic"),
                search_next: gtk::Button::from_icon_name("go-down-symbolic"),
                search_button: gtk::ToggleButton::new(),
                pdf_tools: std::cell::OnceCell::new(),
                reader_prefs: Cell::new(prefs::Reader::load()),
                image_menu: gio::Menu::new(),
                pdf_menu: gio::Menu::new(),
                content: gtk::Stack::new(),
                showing_pdf: Cell::new(false),
                locked: crate::app::locked::LockedPage::new(),
                password: RefCell::default(),
                faces: gtk::Stack::new(),
                combine: crate::app::combine::Combine::new(),
                combining: Cell::new(false),
                redaction_banner: adw::Banner::new(""),
                redactions_baked: Cell::new(false),
                reopen_at: Cell::new(None),
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
                resize_unit: gtk::DropDown::new(None::<gio::ListModel>, None::<gtk::Expression>),
                resolution_spin: gtk::SpinButton::with_range(1.0, 10_000.0, 1.0),
                resample: gtk::CheckButton::new(),
                dpi_file: Cell::new(None),
                dpi: Cell::new(None),
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
                redact_toggle: section_toggle("view-conceal-symbolic", "Redact"),
                redact_options: gtk::Box::new(gtk::Orientation::Vertical, 6),
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
                exposure_scale: tone_scale(),
                highlights_scale: tone_scale(),
                shadows_scale: tone_scale(),
                temperature_scale: tone_scale(),
                tint_scale: tone_scale(),
                sepia_scale: crate::images::edit::tone_panel::one_sided_scale(),
                sharpness_scale: tone_scale(),
                inspector: crate::app::inspector::Inspector::new(),
                info_button: gtk::ToggleButton::new(),
                picture: RefCell::new(None),
                frames: crate::app::frames::FramesPane::new(),
                frames_wanted: Cell::new(false),
                slideshow: Cell::new(false),
                slideshow_paused: Cell::new(false),
                slideshow_hovered: Cell::new(false),
                slideshow_timer: RefCell::new(None),
                slideshow_linger: RefCell::new(None),
                slideshow_controls: gtk::Revealer::new(),
                slideshow_play: gtk::Button::new(),
                slideshow_position: gtk::Label::new(None),
                slideshow_every: gtk::DropDown::new(None::<gio::ListModel>, None::<gtk::Expression>),
                slideshow_restore: Cell::new((false, false, false)),
                levels_toggle: section_toggle("glance-levels-symbolic", "Levels"),
                levels_options: gtk::Box::new(gtk::Orientation::Vertical, 4),
                levels_graph: crate::images::edit::levels::LevelsGraph::new(),
                levels_channel: gtk::DropDown::new(None::<gio::ListModel>, None::<gtk::Expression>),
                levels_black: gtk::SpinButton::with_range(0.0, 253.0, 1.0),
                levels_gamma: gtk::SpinButton::with_range(0.1, 9.99, 0.05),
                levels_white: gtk::SpinButton::with_range(2.0, 255.0, 1.0),
                levels_auto: gtk::Button::new(),
                levels_note: gtk::Label::new(None),
                tone_worker: RefCell::new(None),
                tone_timer: RefCell::new(None),
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
            self.obj().install_viewing_actions();
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
        content.add_named(&imp.locked.root, Some("locked"));
        imp.locked.connect_unlock(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |password| {
                let Some(path) = window.imp().current.borrow().clone() else { return };
                window.imp().password.replace(Some(password));
                window.load(path, false);
            }
        ));
        content.set_visible_child_name("image");
        // The page sidebar sits over the reader on a narrow window and beside it
        // on a wide one. Images never show it.
        let split = &imp.split;
        split.set_hexpand(true);
        split.set_sidebar(Some(imp.pdf_view.sidebar()));
        split.set_content(Some(content));
        // Wide enough for a heading in the table of contents to be read.
        split.set_min_sidebar_width(200.0);
        split.set_max_sidebar_width(260.0);
        split.set_show_sidebar(false);
        body.append(split);
        imp.pdf_view.connect_status(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |status| window.show_pdf_status(status)
        ));
        self.build_pdf_context_menu();
        imp.pdf_view.connect_error(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |error| window.toast(&error)
        ));
        imp.pdf_view.connect_redactions(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.update_redaction_banner()
        ));
        imp.pdf_view.connect_bookmarks(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |event| window.on_bookmark(event)
        ));
        // As the reader was last left.
        let reader = imp.reader_prefs.get();
        imp.pdf_view.set_mode(reader.mode);
        imp.pdf_view.set_night(reader.night);
        imp.pdf_view.set_sidebar_view(reader.sidebar);
        imp.pdf_view.connect_sidebar_view(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |view| window.update_prefs(|prefs| prefs.sidebar = view)
        ));
        body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        body.append(self.build_edit_panel());
        let pdf_tools = crate::app::pdf_tools::PdfTools::new(self);
        body.append(&pdf_tools.root);
        let _ = imp.pdf_tools.set(pdf_tools);
        body.append(&imp.inspector.root);
        // The slideshow's controls float over the picture.
        let stage = gtk::Overlay::new();
        stage.set_child(Some(&body));
        stage.add_overlay(&imp.slideshow_controls);
        imp.toasts.set_child(Some(&stage));

        let open_button = gtk::Button::from_icon_name("document-open-symbolic");
        open_button.set_tooltip_text(Some("Open (Ctrl+O)"));
        open_button.set_action_name(Some("win.open"));

        // Built in `menus`; copied into the two models the menu button
        // switches between.
        for (model, built) in [(&imp.image_menu, menus::image_menu()), (&imp.pdf_menu, menus::pdf_menu())] {
            for i in 0..built.n_items() {
                model.append_item(&gio::MenuItem::from_model(&built, i));
            }
        }
        let menu = &imp.image_menu;

        let menu_button = &imp.menu_button;
        menu_button.set_icon_name("open-menu-symbolic");
        menu_button.set_tooltip_text(Some("Main Menu"));
        menu_button.set_menu_model(Some(menu));
        self.fill_menu(false);
        menu_button.set_primary(true);

        // For PDFs only: the sidebar of pages, contents and bookmarks, and the
        // page you are on, which can be typed over to go somewhere else.
        let sidebar_button = &imp.sidebar_button;
        sidebar_button.set_icon_name("sidebar-show-symbolic");
        sidebar_button.set_tooltip_text(Some("Sidebar (F9)"));
        sidebar_button.set_action_name(Some("win.show-pages"));
        sidebar_button.set_visible(false);

        let page_entry = &imp.page_entry;
        page_entry.set_width_chars(3);
        page_entry.set_max_width_chars(5);
        gtk::prelude::EntryExt::set_alignment(page_entry, 1.0);
        page_entry.set_input_purpose(gtk::InputPurpose::Digits);
        page_entry.set_tooltip_text(Some("Go to page"));
        page_entry.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.go_to_typed_page()
        ));
        // Clicking away without pressing Enter puts the real page number back.
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.show_page_number()
        ));
        page_entry.add_controller(focus);
        imp.page_total.add_css_class("dim-label");
        imp.page_total.add_css_class("numeric");
        self.build_search_bar();

        let page_box = &imp.page_box;
        page_box.append(page_entry);
        page_box.append(&imp.page_total);
        page_box.set_visible(false);

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
        header.pack_start(&imp.sidebar_button);
        header.pack_start(&open_button);
        header.pack_start(&imp.page_box);
        header.pack_end(menu_button);
        let info_button = &imp.info_button;
        info_button.set_icon_name("document-properties-symbolic");
        info_button.set_tooltip_text(Some("Image Info (Ctrl+I)"));
        info_button.set_action_name(Some("win.inspector"));
        header.pack_end(info_button);
        header.pack_end(rotate_button);
        header.pack_end(fullscreen_button);
        header.pack_end(copy_button);
        header.pack_end(&imp.search_button);

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
        toolbar.add_top_bar(&imp.search_bar);
        toolbar.add_top_bar(action_bar);
        let banner = &imp.redaction_banner;
        banner.set_button_label(Some("_Apply…"));
        banner.set_use_markup(false);
        banner.connect_button_clicked(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.apply_redactions()
        ));
        toolbar.add_top_bar(banner);
        toolbar.set_content(Some(&imp.toasts));
        toolbar.add_bottom_bar(&imp.strip);
        imp.faces.add_named(toolbar, Some("view"));
        imp.faces.add_named(imp.combine.widget(), Some("combine"));
        imp.faces.set_transition_type(gtk::StackTransitionType::Crossfade);
        self.set_content(Some(&imp.faces));
        let paper = imp.reader_prefs.get().paper;
        imp.combine.set_paper(paper, glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |paper| window.update_prefs(|prefs| prefs.paper = Some(paper))
        ));
        imp.combine.connect_close(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |open| window.stop_combining(open)
        ));

        // Focus moving into or out of a text box changes which keys are free;
        // `refresh_accels` says why.
        self.connect_focus_widget_notify(|window| window.refresh_accels());

        // Fullscreen means the picture and nothing else, so the bars fold away.
        self.connect_fullscreened_notify(|window| {
            window.sync_fullscreen();
            // Leaving full screen, however it was done, ends a slideshow.
            if !window.is_fullscreen() {
                window.stop_slideshow();
            }
        });

        // ...but they come back when the pointer reaches an edge, so there is
        // always a visible way out rather than only a key to guess at.
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _, y| {
                // A slideshow has its own controls, and keeps the bars away.
                if window.imp().slideshow.get() {
                    window.wake_slideshow_controls();
                    return;
                }
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
            let _ = sender.send_blocking(thumbs::generate(&worker_path, filmstrip::SLOT_W, filmstrip::SLOT_H));
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
    pub(crate) fn sync_fullscreen(&self) {
        let imp = self.imp();
        let full = self.is_fullscreen();
        // A slideshow is the picture alone, even where full screen is refused.
        let bare = full || imp.slideshow.get();
        imp.toolbar.set_reveal_top_bars(!bare);
        imp.toolbar.set_reveal_bottom_bars(!bare);
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
                let files = match value.get::<gdk::FileList>() {
                    Ok(list) => list.files(),
                    Err(_) => value.get::<gio::File>().ok().into_iter().collect(),
                };
                if files.is_empty() {
                    return false;
                }
                // Several files: open the first, or combine them all.
                window.open_files(files);
                true
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
                move |_, _| {
                    let imp = window.imp();
                    if imp.showing_pdf.get() {
                        imp.pdf_view.rotate_by(if degrees > 0.0 { 1 } else { -1 });
                    } else {
                        imp.view.canvas().rotate_by(degrees);
                    }
                }
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
                // One panel beside the picture at a time: the editor's.
                if open {
                    if let Some(action) = window.lookup_action("inspector").and_downcast::<gio::SimpleAction>() {
                        action.change_state(&false.to_variant());
                    }
                }
                window.imp().info_button.set_sensitive(!open);
                if open {
                    // Decode once, when editing actually starts, rather than
                    // holding a full-resolution buffer for every image browsed.
                    window.load_working();
                } else {
                    // Leaving the panel puts the tools away with it.
                    window.imp().crop_toggle.set_active(false);
                    window.imp().adjust_toggle.set_active(false);
                    window.imp().levels_toggle.set_active(false);
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
        let paging: [(&str, fn(&PdfView)); 8] = [
            ("page-down", |view| view.scroll_pages(1.0)),
            ("page-up", |view| view.scroll_pages(-1.0)),
            ("line-down", |view| view.scroll_lines(1.0)),
            ("line-up", |view| view.scroll_lines(-1.0)),
            ("scroll-right", |view| view.scroll_across(1.0)),
            ("scroll-left", |view| view.scroll_across(-1.0)),
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
                // A PDF's Draw and Text panel takes the image editor's key.
                if window.imp().showing_pdf.get() {
                    window.set_draw_panel(!window.draw_panel_open());
                    return;
                }
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
                let imp = window.imp();
                imp.view.canvas().reset_size();
                imp.dpi.set(imp.dpi_file.get());
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
            move |_, _| window.reset_sliders()
        ));
        self.add_action(&adjust_reset);

        let levels_reset = gio::SimpleAction::new("levels-reset", None);
        levels_reset.set_enabled(false);
        levels_reset.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.reset_levels()
        ));
        self.add_action(&levels_reset);

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
                let imp = window.imp();
                if !imp.showing_pdf.get() {
                    window.copy_to_clipboard();
                } else if !imp.pdf_view.allowed().copy {
                    window.toast("The author of this document does not allow copying its text.");
                } else if imp.pdf_view.copy_selection() {
                    window.toast("Text copied.");
                } else {
                    window.toast("Select some text to copy first.");
                }
            }
        ));
        self.add_action(&copy);

        // The page sidebar. A stateful action, so the header button and the
        // menu's check mark stay in step with each other.
        let show_pages = gio::SimpleAction::new_stateful("show-pages", None, &false.to_variant());
        show_pages.set_enabled(false);
        show_pages.connect_change_state(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |action, state| {
                let Some(show) = state.and_then(|state| state.get::<bool>()) else { return };
                action.set_state(&show.to_variant());
                let imp = window.imp();
                imp.split.set_show_sidebar(show);
                if imp.showing_pdf.get() {
                    imp.pdf_view.set_sidebar_active(show);
                } else if imp.view.canvas().frame_count() > 1 && !imp.slideshow.get() {
                    // Remembered for the next animation.
                    imp.frames_wanted.set(show);
                }
            }
        ));
        self.add_action(&show_pages);
        // On a narrow window the sidebar floats over the page, and a click
        // beside it closes it. Keep the button in step when that happens.
        self.imp().split.connect_show_sidebar_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |split| {
                if let Some(action) = window.lookup_action("show-pages").and_downcast::<gio::SimpleAction>() {
                    if action.state().and_then(|state| state.get::<bool>()) != Some(split.shows_sidebar()) {
                        action.change_state(&split.shows_sidebar().to_variant());
                    }
                }
            }
        ));

        // Marking up a PDF's text. Each is saved into the file at once, and
        // has its own undo, apart from the image editor's.
        let styles = [
            ("mark-highlight", pdf::Style::Highlight, "highlight"),
            ("mark-underline", pdf::Style::Underline, "underline"),
            ("mark-strike", pdf::Style::StrikeOut, "strike through"),
        ];
        for (name, style, verb) in styles {
            let action = gio::SimpleAction::new(name, None);
            action.connect_activate(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, _| {
                    let imp = window.imp();
                    if !imp.showing_pdf.get() {
                        return;
                    }
                    if !imp.pdf_view.allowed().annotate {
                        window.toast(pdf::NOT_ANNOTATABLE);
                        return;
                    }
                    match imp.pdf_view.mark(style, imp.reader_prefs.get().highlight) {
                        pdf::Marked::Done => {}
                        pdf::Marked::NothingSelected => {
                            window.toast(&format!("Select some text to {verb} first."));
                        }
                        pdf::Marked::Failed(error) => window.toast(&error),
                    }
                }
            ));
            self.add_action(&action);
        }
        for (name, back) in [("undo-mark", true), ("redo-mark", false)] {
            let action = gio::SimpleAction::new(name, None);
            action.set_enabled(false);
            action.connect_activate(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, _| {
                    let view = &window.imp().pdf_view;
                    // Marks not yet applied are the newest thing to take back.
                    if back && view.unmark_last_redaction() {
                        return;
                    }
                    let done = if back { view.undo() } else { view.redo() };
                    if let Err(error) = done {
                        window.toast(&error);
                    }
                }
            ));
            self.add_action(&action);
        }
        self.install_reader_actions();
        self.imp().pdf_view.connect_history(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |can_undo, can_redo| {
                for (name, enabled) in [("undo-mark", can_undo), ("redo-mark", can_redo)] {
                    if let Some(action) = window.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                        action.set_enabled(enabled);
                    }
                }
            }
        ));

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
        // Finding text. `find` opens the bar and puts the cursor in it; the
        // other two step between matches from anywhere in the window.
        let find = gio::SimpleAction::new("find", None);
        find.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                let imp = window.imp();
                if imp.showing_pdf.get() {
                    imp.search_bar.set_search_mode(true);
                    imp.search_entry.grab_focus();
                    imp.search_entry.select_region(0, -1);
                }
            }
        ));
        self.add_action(&find);
        for (name, forward) in [("find-next", true), ("find-previous", false)] {
            let action = gio::SimpleAction::new(name, None);
            action.connect_activate(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, _| {
                    if window.imp().showing_pdf.get() {
                        window.imp().pdf_view.search_step(forward);
                    }
                }
            ));
            self.add_action(&action);
        }

        let dismiss = gio::SimpleAction::new("dismiss", None);
        dismiss.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                // The search bar first: it is the last thing opened.
                if window.imp().search_bar.is_search_mode() {
                    window.imp().search_bar.set_search_mode(false);
                } else if window.imp().slideshow.get() {
                    window.stop_slideshow();
                } else if window.imp().inspector.root.is_visible() {
                    if let Some(action) = window.lookup_action("inspector").and_downcast::<gio::SimpleAction>() {
                        action.change_state(&false.to_variant());
                    }
                } else if window.draw_panel_open() {
                    window.set_draw_panel(false);
                } else if window.is_fullscreen() {
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
        // Trying a password: the padlock stays until the document opens.
        if imp.password.borrow().is_none() {
            imp.content.set_visible_child_name("image");
            imp.view.show_loading();
        }

        let (sender, receiver) = async_channel::bounded(1);
        let scan_path = path.clone();
        let password = imp.password.take();
        std::thread::spawn(move || {
            // Both the decode and the directory listing are filesystem work, so
            // they belong on this side of the channel.
            let is_pdf = pdf::is_pdf(&scan_path);
            let decoded = if is_pdf {
                match pdf::open(&scan_path, password.as_deref()) {
                    Ok(opened) => Ok(Loaded::Pdf(opened)),
                    Err(pdf::OpenError::Locked { tried }) => Ok(Loaded::Locked { tried }),
                    Err(pdf::OpenError::Failed(message)) => Err(message),
                }
            } else {
                loader::decode(&scan_path).map(Loaded::Image)
            };
            // A PDF is read on its own, not browsed with its neighbours.
            let siblings = (rescan && !is_pdf).then(|| playlist::siblings(&scan_path));
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
                    Ok(Loaded::Locked { tried }) => window.show_locked(&name, tried),
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
                        let picture = crate::app::inspector::Picture {
                            label: image.label.clone(),
                            width: shown_w,
                            height: shown_h,
                            animation: (image.animation.len() > 1).then(|| {
                                (image.animation.len(), image.animation.iter().map(|frame| frame.delay).sum())
                            }),
                        };
                        window.imp().view.show_image(image);
                        window.picture_shown(picture);
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
        // A PDF is not in the image listing, and must not be mistaken for an
        // image that has vanished from the folder.
        if imp.showing_pdf.get() {
            return;
        }
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
        self.picture_gone();
        self.update_navigation();
    }

    /// Navigation is pointless with one image; while the transform options are
    /// open the arrow keys belong to the rotation slider; and while editing the
    /// picture on screen is unsaved work, so stepping off it — by key, by arrow
    /// or by thumbnail — is switched off and the filmstrip goes with it.
    fn update_navigation(&self) {
        let imp = self.imp();
        let busy = imp.edit_button.is_active();
        let enabled = imp.playlist.borrow().is_some() && !busy && !imp.showing_pdf.get();
        for name in ["next-image", "previous-image", "slideshow"] {
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
        self.slideshow_skip();
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
        self.picture_gone();

        // Read on its own: no neighbours, no filmstrip, no watching the folder.
        imp.monitor.replace(None);
        imp.playlist.replace(None);
        imp.strip.clear();

        let pages = opened.pages.len();
        let subtitle = format!("PDF · {pages} {}", if pages == 1 { "page" } else { "pages" });
        imp.title.set_subtitle(&subtitle);
        imp.shown.replace(Some(Shown { name, subtitle }));
        imp.page_total.set_text(&format!("of {pages}"));
        imp.showing_pdf.set(true);
        // A new document starts without a search.
        imp.search_bar.set_search_mode(false);
        imp.search_entry.set_text("");
        imp.pdf_view.show(opened);
        match imp.reopen_at.take() {
            // The same document, rewritten: back where it was, tools and all.
            Some(page) => imp.pdf_view.go_to_page(page),
            // Another document starts plain, without the last one's tools.
            None => self.set_draw_panel(false),
        }
        imp.content.set_visible_child_name("pdf");

        imp.rotate_button.set_sensitive(true);
        imp.edit_button.set_sensitive(false);
        self.show_chrome(true);
    }

    /// A protected PDF, waiting for its password.
    fn show_locked(&self, name: &str, tried: bool) {
        self.leave_pdf();
        let imp = self.imp();
        imp.view.show_idle();
        imp.title.set_title(name);
        imp.title.set_subtitle("Locked PDF");
        imp.shown.replace(Some(Shown { name: name.to_string(), subtitle: "Locked PDF".to_string() }));
        imp.action_bar.set_visible(false);
        for button in [&imp.delete_button, &imp.copy_button] {
            button.set_sensitive(false);
        }
        imp.edit_button.set_sensitive(false);
        // Nothing to copy or turn until it is open.
        imp.copy_button.set_visible(false);
        imp.rotate_button.set_visible(false);
        imp.content.set_visible_child_name("locked");
        imp.locked.show(name, tried);
    }

    /// The header and bars for the kind of document on screen. Images keep
    /// the layout they always had; a PDF drops what only makes sense for a
    /// picture and gains its pages.
    fn show_chrome(&self, pdf: bool) {
        let imp = self.imp();
        imp.action_bar.set_visible(!pdf);
        imp.info_button.set_visible(!pdf);
        if pdf {
            // The sidebar is the document's pages again, not an animation's
            // frames.
            imp.split.set_sidebar(Some(imp.pdf_view.sidebar()));
            imp.sidebar_button.set_tooltip_text(Some("Sidebar (F9)"));
        }
        imp.copy_button.set_visible(!pdf);
        imp.sidebar_button.set_visible(pdf);
        imp.page_box.set_visible(pdf);
        imp.search_button.set_visible(pdf);
        // A PDF keeps its header to finding its way round: the rest is in
        // the menu. Pictures keep their own buttons.
        imp.rotate_button.set_visible(!pdf);
        imp.fullscreen_button.set_visible(!pdf);
        if !pdf {
            self.set_draw_panel(false);
        }
        if !pdf {
            imp.search_bar.set_search_mode(false);
        }
        imp.menu_button.set_menu_model(Some(if pdf { &imp.pdf_menu } else { &imp.image_menu }));
        self.fill_menu(pdf);
        if let Some(action) = self.lookup_action("show-pages").and_downcast::<gio::SimpleAction>() {
            action.set_enabled(pdf);
            if !pdf {
                action.change_state(&false.to_variant());
            }
        }
        if let Some(action) = self.lookup_action("bookmark").and_downcast::<gio::SimpleAction>() {
            action.set_enabled(pdf);
        }
        self.update_navigation();
        self.refresh_accels();
        self.update_redaction_banner();
    }

    /// Back to images: stop drawing pages and free them.
    fn leave_pdf(&self) {
        let imp = self.imp();
        if imp.showing_pdf.replace(false) {
            imp.pdf_view.clear();
            imp.pdf_status.set(None);
            self.show_chrome(false);
        }
        imp.content.set_visible_child_name("image");
    }

    fn show_pdf_status(&self, status: pdf::Status) {
        let imp = self.imp();
        if !imp.showing_pdf.get() {
            return;
        }
        imp.pdf_status.set(Some(status));
        self.sync_bookmark();
        let noun = if status.pages == 1 { "page" } else { "pages" };
        imp.title.set_subtitle(&format!("PDF · {} {noun} · {:.0}%", status.pages, status.percent));
        // Not while someone is typing a number into it.
        if !imp.page_entry.state_flags().contains(gtk::StateFlags::FOCUS_WITHIN) {
            self.show_page_number();
        }
    }

    /// The bar under the header for finding text in a PDF: what to find, how
    /// many there are and which one this is, and buttons to step through them.
    /// The reader's own settings and additions: highlight colour, layout,
    /// night mode, notes and bubbles, and the document's details.
    fn install_reader_actions(&self) {
        let imp = self.imp();
        let prefs = imp.reader_prefs.get();

        let colour = gio::SimpleAction::new_stateful(
            "highlight-colour",
            Some(glib::VariantTy::STRING),
            &prefs.highlight.hex().to_variant(),
        );
        colour.connect_change_state(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |action, value| {
                let Some(rgb) = value.and_then(|v| v.str()).and_then(pdf::Rgb::from_hex) else { return };
                action.set_state(&rgb.hex().to_variant());
                let imp = window.imp();
                window.update_prefs(|prefs| prefs.highlight = rgb);
                // Choosing a colour with text selected highlights it in that
                // colour, as in Preview.
                if imp.showing_pdf.get() {
                    if let pdf::Marked::Failed(error) = imp.pdf_view.mark(pdf::Style::Highlight, rgb) {
                        window.toast(&error);
                    }
                }
            }
        ));
        self.add_action(&colour);

        let pick = gio::SimpleAction::new("pick-highlight-colour", None);
        pick.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                let current = window.imp().reader_prefs.get().highlight.to_rgba();
                let weak = window.downgrade();
                crate::app::colour::choose(&window, "Highlight Colour", &current, false, move |rgba| {
                    if let Some(window) = weak.upgrade() {
                        let hex = pdf::Rgb::from_rgba(&rgba).hex().to_variant();
                        gio::prelude::ActionGroupExt::change_action_state(&window, "highlight-colour", &hex);
                    }
                });
            }        ));
        self.add_action(&pick);

        let layout = gio::SimpleAction::new_stateful(
            "pdf-layout",
            Some(glib::VariantTy::STRING),
            &prefs::mode_name(prefs.mode).to_variant(),
        );
        layout.connect_change_state(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |action, value| {
                let Some(mode) = value.and_then(|v| v.str()).and_then(prefs::mode_from) else { return };
                action.set_state(&prefs::mode_name(mode).to_variant());
                window.imp().pdf_view.set_mode(mode);
                window.update_prefs(|prefs| prefs.mode = mode);
            }
        ));
        self.add_action(&layout);

        let night = gio::SimpleAction::new_stateful("night-mode", None, &prefs.night.to_variant());
        night.connect_change_state(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |action, value| {
                let Some(on) = value.and_then(|v| v.get::<bool>()) else { return };
                action.set_state(&on.to_variant());
                window.imp().pdf_view.set_night(on);
                window.update_prefs(|prefs| prefs.night = on);
            }
        ));
        self.add_action(&night);

        // The page being read, bookmarked or not: a check in the menu.
        let bookmark = gio::SimpleAction::new_stateful("bookmark", None, &false.to_variant());
        bookmark.set_enabled(false);
        bookmark.connect_change_state(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                let imp = window.imp();
                if imp.showing_pdf.get() {
                    imp.pdf_view.toggle_bookmark();
                }
                window.sync_bookmark();
            }
        ));
        self.add_action(&bookmark);

        let pin = gio::SimpleAction::new("pin", Some(glib::VariantTy::STRING));
        pin.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, value| {
                let imp = window.imp();
                if !imp.showing_pdf.get() {
                    return;
                }
                let (kind, here) = match value.and_then(|v| v.str()) {
                    Some("note") => (pdf::Pinned::Note, false),
                    Some("note-here") => (pdf::Pinned::Note, true),
                    Some("bubble") => (pdf::Pinned::Bubble, false),
                    Some("bubble-here") => (pdf::Pinned::Bubble, true),
                    _ => return,
                };
                if !imp.pdf_view.allowed().annotate {
                    window.toast(pdf::NOT_ANNOTATABLE);
                    return;
                }
                imp.pdf_view.pin(kind, here);
            }
        ));
        self.add_action(&pin);

        // The Draw and Text panel, open or not: a check in the menu, Ctrl+E,
        // and the panel's own close button.
        let draw_panel = gio::SimpleAction::new_stateful("draw-panel", None, &false.to_variant());
        draw_panel.connect_change_state(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |action, value| {
                let Some(open) = value.and_then(|v| v.get::<bool>()) else { return };
                let open = open && window.imp().showing_pdf.get();
                if open && !window.imp().pdf_view.allowed().annotate {
                    window.toast(pdf::NOT_ANNOTATABLE);
                    return;
                }
                action.set_state(&open.to_variant());
                if let Some(tools) = window.imp().pdf_tools.get() {
                    tools.set_open(open);
                }
            }
        ));
        self.add_action(&draw_panel);

        let shortcuts = gio::SimpleAction::new("show-shortcuts", None);
        shortcuts.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| crate::app::shortcuts::present(&window, window.imp().showing_pdf.get())
        ));
        self.add_action(&shortcuts);

        let info = gio::SimpleAction::new("document-info", None);
        info.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                let imp = window.imp();
                if !imp.showing_pdf.get() {
                    return;
                }
                if let Some(path) = imp.pdf_view.path() {
                    crate::app::doc_info::present(&window, path, imp.pdf_view.password(), imp.pdf_view.current_page());
                }
            }
        ));
        self.add_action(&info);

        let protect = gio::SimpleAction::new("protect", None);
        protect.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                let imp = window.imp();
                let Some(path) = imp.pdf_view.path().filter(|_| imp.showing_pdf.get()) else { return };
                let allowed = imp.pdf_view.allowed();
                let shown = path.clone();
                crate::app::protect::present(&window, path, imp.pdf_view.password(), allowed, glib::clone!(
                    #[weak]
                    window,
                    move |outcome| match outcome {
                        crate::app::protect::Outcome::Replaced { password } => {
                            window.toast(if password.is_some() {
                                "Saved. It now needs its password to open."
                            } else {
                                "Saved. It opens without a password now."
                            });
                            window.reopen(&shown, password);
                        }
                        crate::app::protect::Outcome::Copied(copy) => {
                            window.toast(&format!("Saved a copy, “{}”.", file_name(&copy)));
                        }
                    }
                ));
            }
        ));
        self.add_action(&protect);

        let reduce = gio::SimpleAction::new("reduce-size", None);
        reduce.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                let imp = window.imp();
                let Some(path) = imp.pdf_view.path().filter(|_| imp.showing_pdf.get()) else { return };
                if !imp.pdf_view.allowed().everything {
                    window.toast("The author of this document does not allow changing it.");
                    return;
                }
                let password = imp.pdf_view.password();
                let shown = path.clone();
                crate::app::shrink::present(&window, path, password.clone(), glib::clone!(
                    #[weak]
                    window,
                    move |outcome| {
                        use crate::app::shrink::{readable, Outcome};
                        match outcome {
                            Outcome::Replaced { before, after } => {
                                window.toast(&format!("Saved: {} → {}.", readable(before), readable(after)));
                                window.reopen(&shown, password.clone());
                            }
                            Outcome::Copied { path, before, after } => window.toast(&format!(
                                "Saved “{}”: {} → {}.",
                                file_name(&path),
                                readable(before),
                                readable(after)
                            )),
                        }
                    }
                ));
            }
        ));
        self.add_action(&reduce);

        // Combine into PDF, starting with whatever is open.
        let combine = gio::SimpleAction::new("combine", None);
        combine.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                let imp = window.imp();
                let current = imp.current.borrow().clone();
                let password = imp.showing_pdf.get().then(|| imp.pdf_view.password()).flatten();
                window.start_combining(current.map(|path| (path, password)).into_iter().collect());
            }
        ));
        self.add_action(&combine);

        let print = gio::SimpleAction::new("print", None);
        print.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.print()
        ));
        self.add_action(&print);

        let mark_redact = gio::SimpleAction::new("mark-redact", None);
        mark_redact.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                let imp = window.imp();
                if !imp.showing_pdf.get() {
                    return;
                }
                if !imp.pdf_view.allowed().everything {
                    window.toast("The author of this document does not allow changing it.");
                    return;
                }
                if let pdf::Marked::NothingSelected = imp.pdf_view.mark_redaction() {
                    window.toast("Select text to redact, or mark an area with Redact in the Edit panel.");
                }
            }
        ));
        self.add_action(&mark_redact);
        let apply = gio::SimpleAction::new("apply-redactions", None);
        apply.set_enabled(false);
        apply.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.apply_redactions()
        ));
        self.add_action(&apply);
        let here = gio::SimpleAction::new("unredact-here", None);
        here.set_enabled(false);
        here.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                if !window.imp().pdf_view.unmark_redaction_here() {
                    window.toast("Right-click inside a marked area to unmark it.");
                }
            }
        ));
        self.add_action(&here);
        let all = gio::SimpleAction::new("unredact-all", None);
        all.set_enabled(false);
        all.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.imp().pdf_view.clear_redactions()
        ));
        self.add_action(&all);
    }

    /// Print what is on screen: a PDF's pages, or the picture with its edits.
    fn print(&self) {
        let imp = self.imp();
        let Some(path) = imp.current.borrow().clone() else { return };
        let name = file_name(&path);
        if imp.showing_pdf.get() {
            if !imp.pdf_view.allowed().print {
                self.toast("The author of this document does not allow printing it.");
                return;
            }
            let uri = gio::File::for_path(&path).uri();
            crate::app::print::print_pdf(self, &uri, imp.pdf_view.password().as_deref(), &name);
            return;
        }
        // Being edited: print the edits too, as they look.
        if let Some(picture) = self.rendered() {
            crate::app::print::print_picture(self, picture, &name);
            return;
        }
        let (sender, receiver) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = sender.send_blocking(crate::images::edit::export::open(&path));
        });
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                match receiver.recv().await {
                    Ok(Ok(picture)) => crate::app::print::print_picture(&window, picture, &name),
                    Ok(Err(error)) => window.toast(&format!("Could not print: {error}")),
                    Err(_) => {}
                }
            }
        ));
    }

    /// Open what was handed over: one file, or for several, ask whether to
    /// open the first or combine them all into a PDF. While combining, they
    /// are added to it.
    pub fn open_files(&self, files: Vec<gio::File>) {
        let imp = self.imp();
        let paths: Vec<std::path::PathBuf> = files.iter().filter_map(|f| f.path()).collect();
        if imp.combining.get() {
            imp.combine.add_files(paths);
            return;
        }
        match files.as_slice() {
            [] => {}
            [one] => self.open_file(one),
            [first, ..] => {
                let first = first.clone();
                let count = files.len();
                let dialog = adw::AlertDialog::new(
                    Some(&format!("Open {count} Files")),
                    Some("Look at the first of them, or put them all together as one PDF?"),
                );
                dialog.add_responses(&[("open", "_Open the First"), ("combine", "_Combine into PDF")]);
                dialog.set_response_appearance("combine", adw::ResponseAppearance::Suggested);
                dialog.set_default_response(Some("combine"));
                dialog.set_close_response("open");
                dialog.connect_response(None, glib::clone!(
                    #[weak(rename_to = window)]
                    self,
                    move |_, response| {
                        if response == "combine" {
                            window.start_combining(paths.iter().map(|p| (p.clone(), None)).collect());
                        } else {
                            window.open_file(&first);
                        }
                    }
                ));
                dialog.present(Some(self));
            }
        }
    }

    /// Turn the window over to combining pages, starting with these files.
    pub(crate) fn start_combining(&self, files: Vec<(std::path::PathBuf, Option<String>)>) {
        let imp = self.imp();
        if imp.combining.replace(true) {
            imp.combine.add_files(files.into_iter().map(|(p, _)| p).collect());
            return;
        }
        imp.faces.set_visible_child_name("combine");
        imp.combine.start(files);
        self.refresh_accels();
    }

    /// Back to viewing, opening the PDF just made if asked to.
    fn stop_combining(&self, open: Option<std::path::PathBuf>) {
        let imp = self.imp();
        imp.combining.set(false);
        imp.faces.set_visible_child_name("view");
        self.refresh_accels();
        if let Some(path) = open {
            self.load(path, true);
        }
    }

    /// Put the row of view buttons into the main menu. Setting a menu makes
    /// a new one each time, so this follows every change of menu.
    fn fill_menu(&self, pdf: bool) {
        let popover = self.imp().menu_button.popover().and_downcast::<gtk::PopoverMenu>();
        if let Some(popover) = popover {
            popover.add_child(&menus::view_buttons(pdf), menus::VIEW_BUTTONS);
        }
    }

    /// Whether the document on screen has redactions waiting to be applied.
    pub(crate) fn redactions_pending(&self) -> bool {
        let imp = self.imp();
        if imp.showing_pdf.get() {
            !imp.pdf_view.redactions().is_empty()
        } else {
            imp.view.canvas().redaction_count() > 0 || imp.redactions_baked.get()
        }
    }

    /// Show or hide the banner, and say what is waiting.
    pub(crate) fn update_redaction_banner(&self) {
        let imp = self.imp();
        let count =
            if imp.showing_pdf.get() { imp.pdf_view.redactions().len() } else { imp.view.canvas().redaction_count() };
        let pending = self.redactions_pending();
        imp.redaction_banner.set_title(&match count {
            0 => "Redactions not saved yet".to_string(),
            1 => "1 area marked for redaction".to_string(),
            n => format!("{n} areas marked for redaction"),
        });
        imp.redaction_banner.set_revealed(pending);
        for (name, enabled) in [("apply-redactions", pending), ("unredact-all", count > 0), ("unredact-here", count > 0)] {
            if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(enabled);
            }
        }
        if imp.showing_pdf.get() && count > 0 {
            if let Some(action) = self.lookup_action("undo-mark").and_downcast::<gio::SimpleAction>() {
                action.set_enabled(true);
            }
        }
    }

    /// Ask, then apply what is marked for redaction.
    pub(crate) fn apply_redactions(&self) {
        let imp = self.imp();
        if !self.redactions_pending() {
            return;
        }
        let Some(path) = imp.current.borrow().clone() else { return };
        let pdf = imp.showing_pdf.get();
        if pdf && !imp.pdf_view.allowed().everything {
            self.toast("The author of this document does not allow changing it.");
            return;
        }
        let count = if pdf { imp.pdf_view.redactions().len() } else { imp.view.canvas().redaction_count() };
        let original = path.clone();
        crate::app::redact::confirm(self, &path, count.max(1), pdf, glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |choice| {
                if pdf {
                    window.redact_pdf(&original, choice);
                } else {
                    match choice {
                        crate::app::redact::Choice::Original => window.write_edited(original.clone(), true),
                        crate::app::redact::Choice::Copy(copy) => window.write_edited(copy, false),
                    }
                }
            }
        ));
    }

    /// Redact the PDF on screen into a copy, or in place, in the background.
    fn redact_pdf(&self, path: &std::path::Path, choice: crate::app::redact::Choice) {
        use crate::app::redact::Choice;
        let imp = self.imp();
        let areas = imp.pdf_view.redactions();
        let password = imp.pdf_view.password();
        let temporary = pdf::temporary_beside(path, "redact");
        let working = adw::Toast::builder().title("Redacting…").timeout(0).build();
        imp.toasts.add_toast(working.clone());
        let (sender, receiver) = async_channel::bounded(1);
        let (source, out, secret) = (path.to_path_buf(), temporary.clone(), password.clone());
        std::thread::spawn(move || {
            let result = pdf::redact(&source, secret.as_deref(), &areas, &out);
            if result.is_err() {
                let _ = std::fs::remove_file(&out);
            }
            let _ = sender.send_blocking(result);
        });
        let path = path.to_path_buf();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                let result = receiver.recv().await.unwrap_or_else(|_| Err("stopped unexpectedly".into()));
                working.dismiss();
                let outcome = match result {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        window.toast(&format!("Could not redact: {error}. Nothing was changed."));
                        return;
                    }
                };
                let pages = if outcome.pages == 1 { "1 page".to_string() } else { format!("{} pages", outcome.pages) };
                match choice {
                    Choice::Original => match pdf::install(&temporary, &path) {
                        Ok(()) => {
                            crate::app::redact::forget_thumbnails(&path);
                            window.toast(&format!("Redacted, on {pages}."));
                            window.reopen(&path, password.clone());
                        }
                        Err(error) => {
                            let _ = std::fs::remove_file(&temporary);
                            window.toast(&format!("Could not save the redacted file: {error}. Nothing was changed."));
                        }
                    },
                    Choice::Copy(copy) => match pdf::keep_as(&temporary, &copy) {
                        Ok(()) => {
                            let toast = adw::Toast::builder()
                                .title(format!("Saved a redacted copy, “{}”. The original is unchanged.", file_name(&copy)))
                                .button_label("_Open")
                                .build();
                            toast.connect_button_clicked(glib::clone!(
                                #[weak]
                                window,
                                move |_| {
                                    window.imp().password.replace(password.clone());
                                    window.load(copy.clone(), false);
                                }
                            ));
                            window.imp().toasts.add_toast(toast);
                        }
                        Err(error) => {
                            let _ = std::fs::remove_file(&temporary);
                            window.toast(&format!("Could not save the redacted copy: {error}"));
                        }
                    },
                }
            }
        ));
    }

    /// Open the document on screen again, after it was rewritten, at the page
    /// it was on.
    fn reopen(&self, path: &std::path::Path, password: Option<String>) {
        let imp = self.imp();
        imp.password.replace(password);
        imp.reopen_at.set(Some(imp.pdf_view.current_page()));
        self.load(path.to_path_buf(), false);
    }

    /// The menu's check follows the page being read.
    fn sync_bookmark(&self) {
        let imp = self.imp();
        let on = imp.showing_pdf.get() && imp.pdf_view.is_bookmarked();
        if let Some(action) = self.lookup_action("bookmark").and_downcast::<gio::SimpleAction>() {
            if action.state().and_then(|state| state.get::<bool>()) != Some(on) {
                action.set_state(&on.to_variant());
            }
        }
    }

    fn on_bookmark(&self, event: pdf::BookmarkEvent) {
        self.sync_bookmark();
        match event {
            // In the list, a new bookmark shows for itself.
            pdf::BookmarkEvent::Added { seen: true, .. } => {}
            pdf::BookmarkEvent::Added { page, seen: false } => self.toast(&format!("Bookmarked page {}.", page + 1)),
            pdf::BookmarkEvent::Removed(mark) => {
                let toast = adw::Toast::builder()
                    .title(format!("Removed the bookmark on page {}.", mark.page + 1))
                    .button_label("_Undo")
                    .build();
                toast.connect_button_clicked(glib::clone!(
                    #[weak(rename_to = window)]
                    self,
                    move |_| {
                        window.imp().pdf_view.restore_bookmark(mark.clone());
                        window.sync_bookmark();
                    }
                ));
                self.imp().toasts.add_toast(toast);
            }
            pdf::BookmarkEvent::Failed(error) => self.toast(&error),
        }
    }

    pub(crate) fn update_prefs(&self, change: impl FnOnce(&mut prefs::Reader)) {
        let imp = self.imp();
        let mut prefs = imp.reader_prefs.get();
        change(&mut prefs);
        imp.reader_prefs.set(prefs);
        prefs.save();
    }

    fn draw_panel_open(&self) -> bool {
        self.action_state("draw-panel").and_then(|state| state.get::<bool>()).unwrap_or(false)
    }

    fn set_draw_panel(&self, open: bool) {
        gio::prelude::ActionGroupExt::change_action_state(self, "draw-panel", &open.to_variant());
    }

    /// A right-click on a page offers what can be done with the selection.
    fn build_pdf_context_menu(&self) {
        let clipboard = gio::Menu::new();
        clipboard.append(Some("_Copy"), Some("win.copy"));
        let marks = gio::Menu::new();
        marks.append(Some("_Highlight"), Some("win.mark-highlight"));
        marks.append(Some("_Underline"), Some("win.mark-underline"));
        marks.append(Some("_Strike Through"), Some("win.mark-strike"));
        let pins = gio::Menu::new();
        pins.append(Some("Add _Note Here"), Some("win.pin::note-here"));
        pins.append(Some("Add Speech _Bubble Here"), Some("win.pin::bubble-here"));
        pins.append(Some("_Bookmark This Page"), Some("win.bookmark"));
        marks.append(Some("_Redact"), Some("win.mark-redact"));
        let unmark = gio::Menu::new();
        unmark.append(Some("Remove Redaction _Mark"), Some("win.unredact-here"));
        unmark.append(Some("Unmark _All Redactions"), Some("win.unredact-all"));
        let menu = gio::Menu::new();
        menu.append_section(None, &clipboard);
        menu.append_section(None, &marks);
        menu.append_section(None, &pins);
        menu.append_section(None, &unmark);

        let reader = self.imp().pdf_view.widget();
        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_parent(reader);
        popover.set_has_arrow(false);
        popover.set_halign(gtk::Align::Start);
        // Not a child the scrolled window knows about, so it is let go of by
        // hand, or GTK complains when the window closes.
        reader.connect_destroy(glib::clone!(
            #[weak]
            popover,
            move |_| popover.unparent()
        ));

        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        click.connect_pressed(glib::clone!(
            #[weak]
            popover,
            #[weak(rename_to = window)]
            self,
            move |gesture, _, x, y| {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                // So "Add Note Here" knows where here is.
                window.imp().pdf_view.set_menu_point(x, y);
                popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                popover.popup();
            }
        ));
        reader.add_controller(click);
    }

    fn build_search_bar(&self) {
        let imp = self.imp();
        let entry = &imp.search_entry;
        entry.set_hexpand(true);
        entry.set_max_width_chars(40);
        entry.set_placeholder_text(Some("Find in document"));
        imp.search_count.add_css_class("dim-label");
        imp.search_count.add_css_class("numeric");
        imp.search_previous.set_tooltip_text(Some("Previous Match (Shift+Enter)"));
        imp.search_next.set_tooltip_text(Some("Next Match (Enter)"));
        imp.search_previous.set_action_name(Some("win.find-previous"));
        imp.search_next.set_action_name(Some("win.find-next"));
        let steps = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        steps.add_css_class("linked");
        steps.append(&imp.search_previous);
        steps.append(&imp.search_next);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.append(entry);
        row.append(&imp.search_count);
        row.append(&steps);
        let bar = &imp.search_bar;
        bar.set_child(Some(&row));
        bar.connect_entry(entry);
        bar.set_show_close_button(true);

        let button = &imp.search_button;
        button.set_icon_name("system-search-symbolic");
        button.set_tooltip_text(Some("Find (Ctrl+F)"));
        button.set_visible(false);
        button.bind_property("active", bar, "search-mode-enabled").bidirectional().sync_create().build();

        entry.connect_search_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |entry| window.imp().pdf_view.search(&entry.text())
        ));
        for (forward, signal) in [(true, "activate"), (true, "next-match"), (false, "previous-match")] {
            entry.connect_local(signal, false, glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[upgrade_or]
                None,
                move |_| {
                    window.imp().pdf_view.search_step(forward);
                    None
                }
            ));
        }
        // Shift+Enter for the one before, as in a browser.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, modifiers| {
                let enter = matches!(key, gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter);
                if enter && modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
                    window.imp().pdf_view.search_step(false);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        ));
        entry.add_controller(keys);

        // Closing the bar takes the marks off the pages and hands the keys back
        // to the document; opening it again searches for whatever it still holds.
        bar.connect_search_mode_enabled_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |bar| {
                let imp = window.imp();
                if bar.is_search_mode() {
                    imp.pdf_view.search(&imp.search_entry.text());
                } else {
                    imp.pdf_view.clear_search();
                    if imp.search_entry.state_flags().contains(gtk::StateFlags::FOCUS_WITHIN) {
                        gtk::prelude::GtkWindowExt::set_focus(&window, None::<&gtk::Widget>);
                    }
                }
            }
        ));

        imp.pdf_view.connect_search_status(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |status| window.show_search_status(status)
        ));
        self.show_search_status(pdf::SearchStatus { current: None, total: 0, done: true });
    }

    fn show_search_status(&self, status: pdf::SearchStatus) {
        let imp = self.imp();
        let typed = !imp.search_entry.text().trim().is_empty();
        let more = if status.done { "" } else { "…" };
        let text = match (status.total, status.current) {
            (0, _) if !status.done => "Searching…".to_string(),
            (0, _) if typed => "No matches".to_string(),
            (0, _) => String::new(),
            (total, Some(current)) => format!("{current} of {total}{more}"),
            (total, None) => format!("{total} found{more}"),
        };
        imp.search_count.set_text(&text);
        imp.search_previous.set_sensitive(status.total > 0);
        imp.search_next.set_sensitive(status.total > 0);
        // Red, the way a search box says it found nothing.
        if typed && status.done && status.total == 0 {
            imp.search_entry.add_css_class("error");
        } else {
            imp.search_entry.remove_css_class("error");
        }
    }

    /// Put the page being read into the page box.
    fn show_page_number(&self) {
        let imp = self.imp();
        if let Some(status) = imp.pdf_status.get() {
            imp.page_entry.set_text(&status.page.to_string());
        }
    }

    /// Go to the page typed into the header, then hand the keys back to the
    /// document so Page Down and friends work again straight away.
    fn go_to_typed_page(&self) {
        let imp = self.imp();
        if let (Some(status), Ok(page)) = (imp.pdf_status.get(), imp.page_entry.text().trim().parse::<usize>()) {
            imp.pdf_view.go_to_page(page.clamp(1, status.pages) - 1);
        }
        gtk::prelude::GtkWindowExt::set_focus(self, None::<&gtk::Widget>);
        self.show_page_number();
    }

    /// A window claims its shortcuts before the focused widget sees the key,
    /// so while a text box has focus the bare keys are withdrawn; without
    /// that, typing 300 lands as 3 and Delete removes the file. A PDF borrows
    /// the paging keys from the folder navigation.
    pub(crate) fn refresh_accels(&self) {
        // A text view is not an Editable, but a note's text is typed into one.
        let typing = gtk::prelude::GtkWindowExt::focus(self)
            .is_some_and(|widget| widget.is::<gtk::Editable>() || widget.is::<gtk::TextView>());
        if let Some(app) = self.application().and_downcast::<adw::Application>() {
            let imp = self.imp();
            crate::apply_accels(&app, typing, imp.showing_pdf.get(), imp.combining.get(), imp.slideshow.get());
        }
    }
}

/// A file's name, for messages.
fn file_name(path: &std::path::Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}
