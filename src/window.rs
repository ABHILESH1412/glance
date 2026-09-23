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

use crate::adjust::{self, Adjustments};
use crate::compress;
use crate::draw;
use crate::image_view::ImageView;
use crate::filmstrip::{self, FilmStrip};
use crate::loader;
use crate::canvas;
use crate::export;
use crate::playlist::{self, Playlist};
use crate::thumbs;

/// How close to an edge the pointer must get before the hidden bars slide back
/// while fullscreen.
const EDGE_REVEAL: f64 = 64.0;

/// What the header bar is currently advertising, so a failed load can restore
/// it instead of leaving a stale "Loading…" behind.
pub struct Shown {
    name: String,
    subtitle: String,
}

/// A colour well that lets the alpha channel be set, so "no background" is a
/// colour you can choose rather than a separate switch.
fn colour_button(initial: gdk::RGBA) -> gtk::ColorDialogButton {
    let dialog = gtk::ColorDialog::new();
    dialog.set_with_alpha(true);
    let button = gtk::ColorDialogButton::new(Some(dialog));
    button.set_rgba(&initial);
    button
}

/// A pixel-dimension entry. Wide range, typed or stepped.
fn dimension_spin() -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(1.0, 30_000.0, 1.0);
    spin.set_numeric(true);
    spin.set_snap_to_ticks(true);
    // Sized to its digits. Letting it expand drags the whole sidebar wider
    // than the picture it is meant to sit beside.
    spin.set_hexpand(false);
    spin.set_width_chars(5);
    spin.set_max_width_chars(6);
    spin
}

/// One tone slider: centred on zero, with a mark there so the neutral point
/// can be found by feel, and its own number drawn beside it.
fn tone_scale() -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, -adjust::RANGE, adjust::RANGE, 1.0);
    scale.set_draw_value(true);
    scale.set_value_pos(gtk::PositionType::Right);
    scale.set_digits(0);
    scale.add_mark(0.0, gtk::PositionType::Bottom, None);
    // A range starting at its minimum would show -100 on a picture nothing had
    // been done to.
    scale.set_value(0.0);
    scale
}

mod imp {
    use super::*;

    pub struct Window {
        pub title: adw::WindowTitle,
        pub toasts: adw::ToastOverlay,
        pub view: ImageView,
        pub toolbar: adw::ToolbarView,
        pub fullscreen_button: gtk::Button,
        pub delete_button: gtk::Button,
        pub copy_button: gtk::Button,
        pub edit_button: gtk::ToggleButton,
        pub edit_panel: gtk::Box,
        pub crop_toggle: gtk::ToggleButton,
        pub crop_options: gtk::Box,
        pub freehand_toggle: gtk::ToggleButton,
        pub resize_button: gtk::Button,
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
        pub rotation_bar: gtk::Box,
        pub rotation_scale: gtk::Scale,
        pub rotation_label: gtk::Label,
        /// Set while pushing the canvas's angle into the slider, so the
        /// slider's own value-changed does not bounce it straight back.
        pub syncing: Cell<bool>,
        pub shown: RefCell<Option<Shown>>,
        /// The other images in the same folder, and where we are in them.
        pub playlist: RefCell<Option<Playlist>>,
        pub transform_open: Cell<bool>,
        /// Bumped on every open so a slow decode that finishes after a newer one
        /// was started can be recognised and discarded.
        pub generation: Cell<u64>,
    }

    impl Default for Window {
        fn default() -> Self {
            Self {
                title: adw::WindowTitle::new("Simple Viewer", ""),
                toasts: adw::ToastOverlay::new(),
                view: ImageView::new(),
                toolbar: adw::ToolbarView::new(),
                fullscreen_button: gtk::Button::from_icon_name("view-fullscreen-symbolic"),
                delete_button: gtk::Button::from_icon_name("user-trash-symbolic"),
                copy_button: gtk::Button::from_icon_name("edit-copy-symbolic"),
                edit_button: gtk::ToggleButton::new(),
                edit_panel: gtk::Box::new(gtk::Orientation::Vertical, 12),
                crop_toggle: gtk::ToggleButton::with_label("Crop"),
                crop_options: gtk::Box::new(gtk::Orientation::Vertical, 8),
                freehand_toggle: gtk::ToggleButton::with_label("Freehand"),
                resize_button: gtk::Button::new(),
                resize_toggle: gtk::ToggleButton::with_label("Resize"),
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
                export_toggle: gtk::ToggleButton::with_label("Export"),
                export_options: gtk::Box::new(gtk::Orientation::Vertical, 6),
                quality_row: gtk::Box::new(gtk::Orientation::Horizontal, 6),
                quality_scale: gtk::Scale::with_range(gtk::Orientation::Horizontal, 1.0, 100.0, 1.0),
                draw_toggle: gtk::ToggleButton::with_label("Draw"),
                draw_options: gtk::Box::new(gtk::Orientation::Vertical, 6),
                draw_tools: RefCell::new(Vec::new()),
                draw_colour: colour_button(gdk::RGBA::new(0.9, 0.15, 0.15, 1.0)),
                draw_width: gtk::SpinButton::with_range(1.0, 200.0, 1.0),
                draw_hint: gtk::Label::new(None),
                text_toggle: gtk::ToggleButton::with_label("Text"),
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
                adjust_toggle: gtk::ToggleButton::with_label("Adjust"),
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
                rotation_bar: gtk::Box::new(gtk::Orientation::Horizontal, 6),
                rotation_scale: gtk::Scale::with_range(
                    gtk::Orientation::Horizontal,
                    -180.0,
                    180.0,
                    1.0,
                ),
                rotation_label: gtk::Label::new(Some("0°")),
                syncing: Cell::new(false),
                shown: RefCell::new(None),
                playlist: RefCell::new(None),
                transform_open: Cell::new(false),
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
        self.set_title(Some("Simple Viewer"));

        // The edit panel lives beside the picture rather than over it, so the
        // image never sits behind the controls being used on it.
        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        // The picture takes the slack; without this the panel and the image
        // split it and the sidebar ends up twice the width it asked for.
        imp.view.widget().set_hexpand(true);
        body.append(imp.view.widget());
        body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        body.append(self.build_edit_panel());
        imp.toasts.set_child(Some(&body));

        let open_button = gtk::Button::from_icon_name("document-open-symbolic");
        open_button.set_tooltip_text(Some("Open Image"));
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
        rotate_section.append(Some("Rotate _Left"), Some("win.rotate-left"));
        rotate_section.append(Some("Rotate _Right"), Some("win.rotate-right"));
        rotate_section.append(Some("Flip _Horizontally"), Some("win.flip-horizontal"));
        rotate_section.append(Some("Flip _Vertically"), Some("win.flip-vertical"));
        rotate_section.append(Some("Reset Rotation"), Some("win.rotate-reset"));

        let about_section = gio::Menu::new();
        about_section.append(Some("_About Simple Viewer"), Some("win.about"));

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
        rotate_button.set_tooltip_text(Some("Rotate and Flip"));
        rotate_button.set_action_name(Some("win.transform-open"));
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

        let resize_button = &imp.resize_button;
        resize_button.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name("view-fullscreen-symbolic")
                .label("Resize")
                .build(),
        ));
        resize_button.set_tooltip_text(Some("Change the pixel size (Ctrl+R)"));
        resize_button.add_css_class("resize-action");
        resize_button.set_action_name(Some("win.resize"));
        resize_button.set_sensitive(false);

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
        action_bar.append(resize_button);
        action_bar.append(delete_button);
        // Nothing to act on until something is open.
        action_bar.set_visible(false);

        // While the options are open the header carries nothing but the way
        // out of them.
        let close_button = gtk::Button::from_icon_name("window-close-symbolic");
        close_button.set_tooltip_text(Some("Close Rotation Options (Esc)"));
        close_button.set_action_name(Some("win.transform-close"));

        let transform_header = adw::HeaderBar::builder()
            .show_start_title_buttons(false)
            .show_end_title_buttons(false)
            .title_widget(&gtk::Label::new(None))
            .build();
        transform_header.pack_end(&close_button);

        let header_stack = &imp.header_stack;
        header_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        header_stack.set_transition_duration(150);
        header_stack.add_named(&header, Some("normal"));
        header_stack.add_named(&transform_header, Some("transform"));
        header_stack.set_visible_child_name("normal");

        let toolbar = &imp.toolbar;
        toolbar.add_top_bar(header_stack);
        toolbar.add_top_bar(action_bar);
        toolbar.set_content(Some(&imp.toasts));
        toolbar.add_bottom_bar(self.build_rotation_bar());
        toolbar.add_bottom_bar(&imp.strip);
        self.set_content(Some(toolbar));

        // A window claims its accelerators before the focused widget sees the
        // key, so while a size box has focus the bare ones are withdrawn.
        // Without this, typing 300 lands as 3 and Delete removes the file.
        self.connect_focus_widget_notify(|window| {
            let typing = gtk::prelude::GtkWindowExt::focus(window)
                .is_some_and(|widget| widget.is::<gtk::Editable>());
            if let Some(app) = window.application().and_downcast::<adw::Application>() {
                crate::apply_accels(&app, typing);
            }
        });

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

    /// Accept the selection and apply it to the working pixels, so the crop is
    /// what you see and further edits build on it. Nothing reaches the file
    /// until it is saved.
    fn commit_crop(&self) {
        let imp = self.imp();
        let Some(crop) = imp.view.canvas().crop() else {
            return;
        };
        // Put the crop tool away immediately; the pixels follow.
        imp.syncing_panel.set(true);
        imp.crop_toggle.set_active(false);
        imp.freehand_toggle.set_active(false);
        imp.syncing_panel.set(false);
        self.set_cropping(false);
        self.bake(Some(crop));
    }

    /// Fold everything pending into the working pixels, so what is on screen
    /// becomes what the next edit builds on. This is the step that puts an
    /// edit into the undo history.
    fn bake(&self, crop: Option<canvas::CropSelection>) {
        let imp = self.imp();
        let canvas = imp.view.canvas();
        let Some(display) = canvas.display_size() else {
            return;
        };
        let Some(working) = imp.working.borrow().clone() else {
            self.toast("Still preparing this image for editing.");
            return;
        };
        let live = canvas.live_edits();
        if live.is_identity() && crop.is_none() {
            return;
        }

        let (sender, receiver) = async_channel::bounded(1);
        std::thread::spawn(move || {
            // The copy kept for undo is made here rather than on the main loop.
            let previous = working.clone();
            let result = export::apply(working, live, crop.as_ref(), display)
                .map(|baked| (previous, baked));
            let _ = sender.send_blocking(result);
        });

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                match receiver.recv().await {
                    Ok(Ok((previous, baked))) => {
                        window.push_history(previous);
                        window.imp().working.replace(Some(baked));
                        window.show_working();
                        window.update_edit_state();
                    }
                    Ok(Err(message)) => window.toast(&message),
                    Err(_) => window.toast("The editor stopped unexpectedly."),
                }
            }
        ));
    }

    /// How many states of history to keep. Each is a full copy of the image,
    /// so this trades memory for depth the way every editor has to.
    const HISTORY_LIMIT: usize = 8;

    /// Decode the file into the editing buffer, once, when editing starts.
    fn load_working(&self) {
        let imp = self.imp();
        if imp.working.borrow().is_some() {
            return;
        }
        let Some(source) = imp.current.borrow().clone() else {
            return;
        };
        imp.crop_toggle.set_sensitive(false);

        let (sender, receiver) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = sender.send_blocking(export::open(&source));
        });
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                match receiver.recv().await {
                    Ok(Ok(image)) => {
                        window.imp().working.replace(Some(image));
                        window.imp().crop_toggle.set_sensitive(true);
                    }
                    _ => window.toast("This image cannot be edited."),
                }
            }
        ));
    }

    /// Throw away the edit session, which belongs to one image.
    fn reset_editing(&self) {
        let imp = self.imp();
        imp.working.replace(None);
        imp.history.borrow_mut().clear();
        imp.redo.borrow_mut().clear();
        imp.dirty.set(false);
        // Zero the canvas and the sliders together rather than relying on
        // whatever replaces the texture next: a slider still reading -50 over
        // an untouched picture is a lie the next session would inherit.
        imp.view.canvas().set_adjustments(Adjustments::default());
        self.sync_tone_panel();
        imp.view.canvas().reset_size();
        imp.resize_toggle.set_active(false);
        imp.view.canvas().clear_text();
        imp.text_toggle.set_active(false);
        imp.view.canvas().clear_marks();
        imp.draw_toggle.set_active(false);
        self.update_edit_state();
    }

    fn update_edit_state(&self) {
        let imp = self.imp();
        let canvas = imp.view.canvas();
        let undo = !imp.history.borrow().is_empty() || canvas.has_marks() || canvas.has_text();
        let redo = !imp.redo.borrow().is_empty();
        imp.undo_button.set_sensitive(undo);
        imp.redo_button.set_sensitive(redo);
        for (name, enabled) in [("undo", undo), ("redo", redo)] {
            if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(enabled);
            }
        }
        let steps = imp.history.borrow().len();
        imp.pending_crop.set_visible(imp.dirty.get());
        imp.pending_crop.set_text(&match steps {
            0 => "Edited. Save to write it back.".to_string(),
            1 => "1 edit. Save to write it back.".to_string(),
            n => format!("{n} edits. Save to write it back."),
        });
    }

    /// Put an image on screen as the thing being edited.
    fn show_working(&self) {
        let imp = self.imp();
        let Some(image) = imp.working.borrow().clone() else {
            return;
        };
        let rgba = image.to_rgba8();
        let (width, height) = rgba.dimensions();
        let texture = canvas::texture_from(width, height, false, rgba.into_raw());
        // Resets zoom, rotation and flips, which is right: they are now baked
        // into these pixels.
        // Resets zoom, rotation, flips and tone, which is right: they are now
        // baked into these pixels. The sliders have to follow.
        imp.view.canvas().set_texture(Some(texture));
        self.sync_tone_panel();
        // Baking gives the picture a new real size, and turns the tool off.
        imp.resize_toggle.set_active(false);
        self.sync_resize_panel();
        // Baking burns the words into the pixels, so the tool starts empty.
        imp.text_toggle.set_active(false);
        self.sync_text_panel();
        // The strokes are pixels now, but the pen should still be in hand:
        // closing the tool after every save would make drawing a chore.
        // `set_texture` dropped the canvas's copy, so hand it back.
        self.sync_draw_tool();
        imp.title
            .set_subtitle(&format!("Edited · {width} × {height}"));
    }

    fn push_history(&self, previous: image::DynamicImage) {
        let imp = self.imp();
        let mut history = imp.history.borrow_mut();
        history.push(previous);
        if history.len() > Self::HISTORY_LIMIT {
            history.remove(0);
        }
        drop(history);
        imp.redo.borrow_mut().clear();
        imp.dirty.set(true);
    }

    fn undo(&self) {
        let imp = self.imp();
        let Some(previous) = imp.history.borrow_mut().pop() else {
            return;
        };
        if let Some(current) = imp.working.replace(Some(previous)) {
            imp.redo.borrow_mut().push(current);
        }
        self.show_working();
        self.update_edit_state();
    }

    fn redo(&self) {
        let imp = self.imp();
        let Some(next) = imp.redo.borrow_mut().pop() else {
            return;
        };
        if let Some(current) = imp.working.replace(Some(next)) {
            imp.history.borrow_mut().push(current);
        }
        self.show_working();
        self.update_edit_state();
    }

    /// Everything the view is showing, as pixels: the working image with any
    /// rotation or flip that has not been baked in yet.
    fn rendered(&self) -> Option<image::DynamicImage> {
        let imp = self.imp();
        let canvas = imp.view.canvas();
        let working = imp.working.borrow().clone()?;
        let display = canvas.display_size()?;
        export::apply(working, canvas.live_edits(), None, display).ok()
    }

    /// Cheaply: asking `live_edits` would draw every text item into pixels
    /// just to find out whether there are any.
    fn has_live_transform(&self) -> bool {
        let canvas = self.imp().view.canvas();
        canvas.rotation().abs() > 0.01
            || canvas.flip_horizontal()
            || canvas.flip_vertical()
            || canvas.has_resize()
            || canvas.has_text()
            || canvas.has_marks()
            || !canvas.adjustments().is_identity()
    }

    fn downloads_dir() -> PathBuf {
        glib::user_special_dir(glib::UserDirectory::Downloads).unwrap_or_else(glib::home_dir)
    }


    /// Put the picture on the clipboard exactly as it is on screen, edits and
    /// all, so pasting elsewhere gives what the viewer is showing.
    fn copy_to_clipboard(&self) {
        let imp = self.imp();
        let Some(source) = imp.current.borrow().clone() else {
            return;
        };
        let canvas = imp.view.canvas();
        let Some(display) = canvas.display_size() else {
            return;
        };
        let live = canvas.live_edits();
        // Reuse the editing buffer when there is one; otherwise decode afresh
        // rather than retaining a full-resolution copy just to copy once.
        let existing = imp.working.borrow().clone();

        let (sender, receiver) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let result = match existing {
                Some(image) => Ok(image),
                None => export::open(&source),
            }
            .and_then(|image| export::apply(image, live, None, display));
            let _ = sender.send_blocking(result);
        });

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                match receiver.recv().await {
                    Ok(Ok(image)) => {
                        let rgba = image.to_rgba8();
                        let (width, height) = rgba.dimensions();
                        let texture =
                            canvas::texture_from(width, height, false, rgba.into_raw());
                        window.clipboard().set_texture(&texture);
                        window.toast("Image copied.");
                    }
                    Ok(Err(message)) => window.toast(&message),
                    Err(_) => window.toast("Copying stopped unexpectedly."),
                }
            }
        ));
    }

    /// Save writes over the image being viewed, which is what Save means.
    /// Save writes over the file being viewed, so it asks first.
    ///
    /// The original is gone once this runs — there is no copy kept and nothing
    /// to undo it with — which is worth one click to be sure of, and the
    /// wording points at Export for anyone who wanted a copy instead.
    fn save_default(&self) {
        let Some(source) = self.imp().current.borrow().clone() else {
            return;
        };
        if !self.imp().dirty.get() && !self.has_live_transform() {
            self.toast("No changes to save.");
            return;
        }
        let name = source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "this image".to_string());

        let dialog = adw::AlertDialog::new(
            Some("Replace the original?"),
            Some(&format!(
                "“{name}” will be overwritten with these edits, and the original \
                 cannot be brought back. Export writes a copy instead."
            )),
        );
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("replace", "Replace");
        dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
        // Escape and clicking away both leave the file alone.
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.connect_response(
            None,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, response| {
                    if response == "replace" {
                        window.write_edited(source.clone(), true);
                    }
                }
            ),
        );
        dialog.present(Some(self));
    }

    /// Write a copy in the chosen format, at whatever size the resize tool is
    /// showing.
    fn export(&self) {
        let Some(source) = self.imp().current.borrow().clone() else {
            return;
        };
        let Some(target) = export::TARGETS.get(self.imp().format_drop.selected() as usize) else {
            return;
        };
        let canvas = self.imp().view.canvas();
        // Ask the format before the file dialog: a refusal after choosing a
        // name and a folder is a refusal arriving too late to be useful.
        if let Some((width, height)) = canvas.target_size() {
            if let Some(refusal) = target.refusal(width, height) {
                self.toast(&refusal);
                return;
            }
        }
        let stem = source
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "image".to_string());
        let dialog = gtk::FileDialog::builder()
            .title(format!("Export as {}", target.label))
            .modal(true)
            .initial_name(format!("{stem}.{}", target.extension))
            .initial_folder(&gio::File::for_path(Self::downloads_dir()))
            .build();
        dialog.save(
            Some(self),
            gio::Cancellable::NONE,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |result| match result {
                    Ok(file) => {
                        if let Some(path) = file.path() {
                            match window.size_wanted() {
                                Some(wanted) => window.write_fitted(path, wanted),
                                None => window.write_edited(path, false),
                            }
                        }
                    }
                    Err(error) => {
                        if !error.matches(gtk::DialogError::Dismissed) {
                            window.toast(&format!("Could not export: {error}"));
                        }
                    }
                }
            ),
        );
    }

    /// Export with a file size to hit, rather than whatever the encoder's
    /// defaults produce.
    fn write_fitted(&self, destination: PathBuf, wanted: u64) {
        self.load_working();
        let Some(image) = self.rendered() else {
            self.toast("Nothing to export yet.");
            return;
        };
        let Some(target) = export::TARGETS.get(self.imp().format_drop.selected() as usize) else {
            return;
        };
        let original = (image.width(), image.height());
        self.imp()
            .size_note
            .set_text(&format!("Working towards {}…", compress::describe(wanted)));

        let (sender, receiver) = async_channel::bounded(1);
        let path = destination.clone();
        std::thread::spawn(move || {
            // A dozen or so encodes of a full-size picture: nowhere near the
            // main loop.
            let outcome = compress::fit_to_size(&image, target, wanted).and_then(|fit| {
                std::fs::write(&path, &fit.bytes)
                    .map_err(|error| format!("Could not write the file: {error}"))
                    .map(|()| fit)
            });
            let _ = sender.send_blocking(outcome);
        });

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                match receiver.recv().await {
                    Ok(Ok(fit)) => {
                        let name = destination
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        // Say exactly what it took, including anything that is
                        // not picture: padding is honest or it is nothing.
                        let mut how = Vec::new();
                        if let Some(quality) = fit.quality {
                            how.push(format!("quality {quality}"));
                            // The search knows what the dial should have said.
                            window.imp().quality_scale.set_value(f64::from(quality));
                        }
                        if (fit.width, fit.height) != original {
                            how.push(format!("scaled to {} × {}", fit.width, fit.height));
                        }
                        if fit.padding > 0 {
                            how.push(format!("{} of padding", compress::describe(fit.padding)));
                        }
                        let detail = if how.is_empty() {
                            String::new()
                        } else {
                            format!(" — {}", how.join(", "))
                        };
                        window.imp().size_note.set_text(&format!(
                            "Wrote {}{detail}.",
                            compress::describe(fit.size())
                        ));
                        window.toast(&format!("Exported {name}"));
                    }
                    Ok(Err(message)) => {
                        window.imp().size_note.set_text(&message);
                        window.toast(&message);
                    }
                    Err(_) => window.toast("The exporter stopped unexpectedly."),
                }
            }
        ));
    }

    /// `in_place` means this became the file on screen, so the session carries
    /// on from the saved pixels with nothing left pending.
    fn write_edited(&self, destination: PathBuf, in_place: bool) {
        self.load_working();
        let Some(image) = self.rendered() else {
            self.toast("Nothing to save yet.");
            return;
        };

        let (sender, receiver) = async_channel::bounded(1);
        let target = destination.clone();
        let encoded = image.clone();
        let quality = self.quality();
        std::thread::spawn(move || {
            // Encoding a large image is slow enough to matter.
            let _ = sender.send_blocking(export::write(&encoded, &target, quality));
        });

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                match receiver.recv().await {
                    Ok(Ok(())) => {
                        let shown = destination
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        if in_place {
                            // The transforms are on disk now, so fold them into
                            // the working pixels and start clean.
                            let imp = window.imp();
                            imp.working.replace(Some(image));
                            imp.history.borrow_mut().clear();
                            imp.redo.borrow_mut().clear();
                            imp.dirty.set(false);
                            window.show_working();
                            window.imp().title.set_subtitle("");
                            window.update_edit_state();
                        }
                        window.toast(&format!("Saved {shown}"));
                    }
                    Ok(Err(message)) => window.toast(&message),
                    Err(_) => window.toast("The exporter stopped unexpectedly."),
                }
            }
        ));
    }

    /// The edit sidebar: tools at the top, output at the bottom.
    fn build_edit_panel(&self) -> &gtk::Box {
        let imp = self.imp();
        let panel = &imp.edit_panel;
        panel.set_width_request(300);
        panel.set_hexpand(false);
        panel.set_margin_top(12);
        panel.set_margin_bottom(12);
        panel.set_margin_start(12);
        panel.set_margin_end(12);
        panel.set_visible(false);

        // The tools live in a scroller. There are enough of them now that an
        // expanded section overflows a short window, and without this GTK
        // squeezes the whole column — sliding every button out from under the
        // pointer that was about to press one.
        let tools = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let scroller = gtk::ScrolledWindow::builder()
            .child(&tools)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .vexpand(true)
            .build();
        panel.append(&scroller);

        let heading = gtk::Label::new(Some("Edit"));
        heading.add_css_class("title-4");
        heading.set_xalign(0.0);
        tools.append(&heading);

        let history_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        history_row.set_homogeneous(true);
        imp.undo_button.set_tooltip_text(Some("Undo (Ctrl+Z)"));
        imp.undo_button.set_action_name(Some("win.undo"));
        imp.undo_button.set_sensitive(false);
        imp.redo_button.set_tooltip_text(Some("Redo (Ctrl+Shift+Z)"));
        imp.redo_button.set_action_name(Some("win.redo"));
        imp.redo_button.set_sensitive(false);
        history_row.append(&imp.undo_button);
        history_row.append(&imp.redo_button);
        tools.append(&history_row);

        // -- crop tool --
        let crop = &imp.crop_toggle;
        crop.set_tooltip_text(Some("Choose the part of the image to keep"));
        crop.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                if window.imp().syncing_panel.get() {
                    return;
                }
                if button.is_active() {
                    window.close_other_sections(button);
                }
                window.set_cropping(button.is_active());
            }
        ));
        tools.append(crop);

        let options = &imp.crop_options;
        // Hidden until the crop tool is picked, so the panel stays quiet.
        options.set_visible(false);

        let aspect_label = gtk::Label::new(Some("Aspect ratio"));
        aspect_label.add_css_class("dim-label");
        aspect_label.set_xalign(0.0);
        options.append(&aspect_label);

        // The ratios people actually crop to, plus unconstrained.
        let ratios: [(&str, f64); 8] = [
            ("Free", 0.0),
            ("1:1", 1.0),
            ("4:5", 4.0 / 5.0),
            ("5:4", 5.0 / 4.0),
            ("3:2", 3.0 / 2.0),
            ("2:3", 2.0 / 3.0),
            ("16:9", 16.0 / 9.0),
            ("9:16", 9.0 / 16.0),
        ];
        let grid = gtk::FlowBox::new();
        grid.set_selection_mode(gtk::SelectionMode::None);
        grid.set_max_children_per_line(4);
        grid.set_row_spacing(4);
        grid.set_column_spacing(4);
        let mut first: Option<gtk::ToggleButton> = None;
        for (label, ratio) in ratios {
            let button = gtk::ToggleButton::with_label(label);
            match &first {
                // One group, so picking a ratio releases the last one.
                Some(anchor) => button.set_group(Some(anchor)),
                None => {
                    button.set_active(true);
                    first = Some(button.clone());
                }
            }
            button.connect_toggled(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |button| {
                    if !button.is_active() || window.imp().syncing_panel.get() {
                        return;
                    }
                    window.imp().freehand_toggle.set_active(false);
                    window.imp().view.canvas().set_aspect(ratio);
                }
            ));
            grid.append(&button);
        }
        options.append(&grid);

        let freehand = &imp.freehand_toggle;
        freehand.set_tooltip_text(Some("Draw the area to keep"));
        freehand.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                if window.imp().syncing_panel.get() {
                    return;
                }
                window.imp().view.canvas().set_freehand(button.is_active());
            }
        ));
        options.append(freehand);

        imp.crop_size.add_css_class("dim-label");
        imp.crop_size.add_css_class("numeric");
        imp.crop_size.set_xalign(0.0);
        options.append(&imp.crop_size);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.set_homogeneous(true);
        let reset = gtk::Button::with_label("Reset");
        reset.set_action_name(Some("win.crop-reset"));
        let confirm = gtk::Button::with_label("OK");
        confirm.add_css_class("suggested-action");
        confirm.set_action_name(Some("win.crop-apply"));
        actions.append(&reset);
        actions.append(&confirm);
        options.append(&actions);
        tools.append(options);

        // -- resize --
        let sizing = &imp.resize_toggle;
        sizing.set_tooltip_text(Some("Change the pixel size"));
        sizing.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                let open = button.is_active();
                if open {
                    window.close_other_sections(button);
                }
                window.imp().resize_options.set_visible(open);
                // The handles belong to the tool, so they come and go with it.
                window.imp().view.canvas().set_resizing(open);
                if open {
                    window.sync_resize_panel();
                }
            }
        ));
        tools.append(sizing);

        let sizes = &imp.resize_options;
        sizes.set_visible(false);

        // Side by side with the caption above each, so the pair reads as one
        // measurement and the panel keeps its width.
        let fields = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        fields.set_homogeneous(true);
        for (label, spin) in [("Width", &imp.width_spin), ("Height", &imp.height_spin)] {
            let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
            let caption = gtk::Label::new(Some(label));
            caption.add_css_class("dim-label");
            caption.set_xalign(0.0);
            column.append(&caption);
            column.append(spin);
            fields.append(&column);
            spin.connect_value_changed(glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[strong]
                label,
                move |_| window.size_typed(label == "Width")
            ));
        }
        sizes.append(&fields);

        let keep = &imp.keep_aspect;
        // On by default: stretching a photograph out of shape is almost never
        // what someone reaching for a resize wants. The canvas has to be told
        // separately — setting the box before its handler exists tells nobody,
        // and the handles would then ignore the lock the box is showing.
        keep.set_active(true);
        imp.view.canvas().set_keep_aspect(true);
        keep.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                let keep = button.is_active();
                window.imp().view.canvas().set_keep_aspect(keep);
                // Ticking it should put a stretched picture straight, not just
                // promise to hold the stretch from here on.
                if keep && !window.imp().syncing_panel.get() {
                    window.size_typed(true);
                }
            }
        ));
        sizes.append(keep);

        imp.natural_label.add_css_class("dim-label");
        imp.natural_label.set_xalign(0.0);
        imp.natural_label.set_wrap(true);
        sizes.append(&imp.natural_label);

        let size_actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        size_actions.set_homogeneous(true);
        let size_reset = gtk::Button::with_label("Reset");
        size_reset.set_action_name(Some("win.resize-reset"));
        let size_apply = gtk::Button::with_label("Apply");
        size_apply.add_css_class("suggested-action");
        size_apply.set_tooltip_text(Some("Resample the image to this size, so it becomes an undo step"));
        size_apply.set_action_name(Some("win.resize-apply"));
        size_actions.append(&size_reset);
        size_actions.append(&size_apply);
        sizes.append(&size_actions);
        tools.append(sizes);

        // -- tone --
        let tone = &imp.adjust_toggle;
        tone.set_tooltip_text(Some("Brightness, contrast and saturation"));
        tone.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                if button.is_active() {
                    window.close_other_sections(button);
                }
                window.imp().adjust_options.set_visible(button.is_active());
            }
        ));
        tools.append(tone);

        let tones = &imp.adjust_options;
        tones.set_visible(false);
        for (label, scale) in [
            ("Brightness", &imp.brightness_scale),
            ("Contrast", &imp.contrast_scale),
            ("Saturation", &imp.saturation_scale),
        ] {
            let caption = gtk::Label::new(Some(label));
            caption.add_css_class("dim-label");
            caption.set_xalign(0.0);
            caption.set_margin_top(4);
            tones.append(&caption);
            scale.connect_value_changed(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_| window.tone_changed()
            ));
            tones.append(scale);
        }

        let tone_actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        tone_actions.set_homogeneous(true);
        tone_actions.set_margin_top(4);
        let tone_reset = gtk::Button::with_label("Reset");
        tone_reset.set_action_name(Some("win.adjust-reset"));
        let tone_apply = gtk::Button::with_label("Apply");
        tone_apply.add_css_class("suggested-action");
        tone_apply.set_tooltip_text(Some("Fix these values into the image, so they can be undone as a step"));
        tone_apply.set_action_name(Some("win.adjust-apply"));
        tone_actions.append(&tone_reset);
        tone_actions.append(&tone_apply);
        tones.append(&tone_actions);
        tools.append(tones);

        // -- drawing --
        let pens = &imp.draw_toggle;
        pens.set_tooltip_text(Some("Draw on the picture"));
        pens.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                let open = button.is_active();
                if open {
                    window.close_other_sections(button);
                }
                window.imp().draw_options.set_visible(open);
                window.sync_draw_tool();
                if !open {
                    window.imp().view.canvas().set_draw_tool(None);
                }
            }
        ));
        tools.append(pens);

        let strokes = &imp.draw_options;
        strokes.set_visible(false);

        // One tool at a time, so picking one lets the last go.
        let tool_grid = gtk::FlowBox::new();
        tool_grid.set_selection_mode(gtk::SelectionMode::None);
        // Two across: three named buttons side by side made the whole sidebar
        // wider than the picture needed it to be.
        tool_grid.set_max_children_per_line(2);
        tool_grid.set_row_spacing(4);
        tool_grid.set_column_spacing(4);
        let mut anchor: Option<gtk::ToggleButton> = None;
        for tool in draw::TOOLS {
            let button = gtk::ToggleButton::new();
            button.set_child(Some(
                &adw::ButtonContent::builder()
                    .icon_name(tool.icon())
                    .label(tool.label())
                    .build(),
            ));
            button.set_tooltip_text(Some(tool.label()));
            match &anchor {
                Some(first) => button.set_group(Some(first)),
                None => anchor = Some(button.clone()),
            }
            button.connect_toggled(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_| window.sync_draw_tool()
            ));
            tool_grid.append(&button);
            imp.draw_tools.borrow_mut().push(button);
        }
        strokes.append(&tool_grid);

        let stroke_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let width_caption = gtk::Label::new(Some("Width"));
        width_caption.add_css_class("dim-label");
        imp.draw_width.set_value(6.0);
        imp.draw_width.set_width_chars(4);
        imp.draw_width.set_tooltip_text(Some("Line thickness, in the image's own pixels"));
        imp.draw_width.connect_value_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |spin| window.imp().view.canvas().set_draw_width(spin.value())
        ));
        let colour_caption = gtk::Label::new(Some("Colour"));
        colour_caption.add_css_class("dim-label");
        imp.draw_colour.set_tooltip_text(Some("Colour of the ink"));
        imp.draw_colour.connect_rgba_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| window.imp().view.canvas().set_draw_colour(button.rgba())
        ));
        stroke_row.append(&width_caption);
        stroke_row.append(&imp.draw_width);
        stroke_row.append(&colour_caption);
        stroke_row.append(&imp.draw_colour);
        strokes.append(&stroke_row);

        let draw_actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        draw_actions.set_homogeneous(true);
        let undo_stroke = gtk::Button::with_label("Undo stroke");
        undo_stroke.set_tooltip_text(Some("Take back the last thing laid over the picture (Ctrl+Z)"));
        undo_stroke.set_action_name(Some("win.undo"));
        let clear = gtk::Button::with_label("Clear");
        clear.set_tooltip_text(Some("Remove every stroke"));
        clear.set_action_name(Some("win.draw-clear"));
        draw_actions.append(&undo_stroke);
        draw_actions.append(&clear);
        strokes.append(&draw_actions);

        imp.draw_hint.add_css_class("dim-label");
        imp.draw_hint.set_xalign(0.0);
        imp.draw_hint.set_wrap(true);
        imp.draw_hint.set_text("Pick a tool, then drag on the picture.");
        strokes.append(&imp.draw_hint);
        tools.append(strokes);

        // -- text --
        let text = &imp.text_toggle;
        text.set_tooltip_text(Some("Lay words over the picture"));
        text.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                let open = button.is_active();
                if open {
                    window.close_other_sections(button);
                }
                window.imp().text_options.set_visible(open);
                // The grab handles belong to the tool, so they go with it.
                window.imp().view.canvas().set_text_tool(open);
                if open {
                    window.sync_text_panel();
                }
            }
        ));
        tools.append(text);

        let words = &imp.text_options;
        words.set_visible(false);

        imp.text_entry.set_placeholder_text(Some("Type something"));
        imp.text_entry.connect_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |entry| {
                let content = entry.text().to_string();
                window.edit_text(move |item| item.content = content.clone());
            }
        ));
        words.append(&imp.text_entry);

        imp.text_font.set_level(gtk::FontLevel::Family);
        imp.text_font.set_use_font(true);
        imp.text_font.connect_font_desc_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                let family = button
                    .font_desc()
                    .and_then(|desc| desc.family())
                    .map(|f| f.to_string());
                if let Some(family) = family {
                    window.edit_text(move |item| item.family = family.clone());
                }
            }
        ));
        words.append(&imp.text_font);

        let style_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let size_label = gtk::Label::new(Some("Size"));
        size_label.add_css_class("dim-label");
        imp.text_size.set_value(48.0);
        imp.text_size.set_width_chars(4);
        imp.text_size.set_tooltip_text(Some("Font size, in the image's own pixels"));
        imp.text_size.connect_value_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |spin| {
                let size = spin.value();
                window.edit_text(move |item| item.size = size);
            }
        ));
        style_row.append(&size_label);
        style_row.append(&imp.text_size);

        // B, I and U, drawn as what they do.
        for (button, css, tip) in [
            (&imp.text_bold, "text-bold", "Bold"),
            (&imp.text_italic, "text-italic", "Italic"),
            (&imp.text_underline, "text-underline", "Underline"),
        ] {
            button.add_css_class(css);
            button.set_tooltip_text(Some(tip));
            button.connect_toggled(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_| window.push_text_style()
            ));
            style_row.append(button);
        }
        words.append(&style_row);

        let colour_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for (label, button, tip) in [
            ("Text", &imp.text_colour, "Colour of the letters"),
            ("Behind", &imp.text_background, "Plate behind the letters — set its opacity to zero for none"),
        ] {
            let caption = gtk::Label::new(Some(label));
            caption.add_css_class("dim-label");
            button.set_tooltip_text(Some(tip));
            button.connect_rgba_notify(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_| window.push_text_style()
            ));
            colour_row.append(&caption);
            colour_row.append(button);
        }
        words.append(&colour_row);

        let text_actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        text_actions.set_homogeneous(true);
        let add = gtk::Button::with_label("Add");
        add.add_css_class("suggested-action");
        add.set_action_name(Some("win.text-add"));
        let remove = gtk::Button::with_label("Remove");
        remove.set_action_name(Some("win.text-remove"));
        text_actions.append(&add);
        text_actions.append(&remove);
        words.append(&text_actions);

        imp.text_hint.add_css_class("dim-label");
        imp.text_hint.set_xalign(0.0);
        imp.text_hint.set_wrap(true);
        words.append(&imp.text_hint);
        tools.append(words);

        imp.pending_crop.add_css_class("dim-label");
        imp.pending_crop.set_xalign(0.0);
        imp.pending_crop.set_wrap(true);
        imp.pending_crop.set_visible(false);
        tools.append(&imp.pending_crop);

        // -- output --
        // Writing a copy is a section like the tools, so the sidebar shows one
        // group of controls at a time instead of all of them at once. Only the
        // two ways out stay pinned below.
        let sending = &imp.export_toggle;
        sending.set_tooltip_text(Some("Format, size and quality for a copy"));
        sending.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                if button.is_active() {
                    window.close_other_sections(button);
                }
                window.imp().export_options.set_visible(button.is_active());
            }
        ));
        tools.append(sending);
        let sending_box = &imp.export_options;
        sending_box.set_visible(false);
        tools.append(sending_box);

        // Export writes a copy in another format, at whatever size the resize
        // tool is showing. Grouped with the other writers, above the way out.
        let export_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        // Names only: the caveats go in the line below, where they can be read
        // without making the sidebar wide enough to hold them.
        let formats = gtk::StringList::new(&[]);
        for target in export::TARGETS {
            formats.append(target.label);
        }
        let drop = &imp.format_drop;
        drop.set_model(Some(&formats));
        drop.set_selected(0);
        drop.set_hexpand(true);
        drop.set_tooltip_text(Some(
            "A drawing can be written as pixels, but pixels cannot be written \
             as a drawing — so SVG is not offered here, whatever the original was.",
        ));
        drop.connect_selected_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| {
                window.describe_format();
                window.describe_target_size();
            }
        ));
        // The format goes at the very top of the section so it cannot move:
        // choosing a lossy one adds a caveat and a quality dial below it, and a
        // picker that slides away as you use it is a picker you fight.
        let format_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        format_row.append(drop);
        sending_box.append(&format_row);

        let export_button = gtk::Button::with_label("Export…");
        export_button.set_hexpand(true);
        export_button.set_tooltip_text(Some(
            "Write a copy somewhere else, in the format and at the size chosen \
             here (Ctrl+Shift+S)",
        ));
        export_button.set_action_name(Some("win.export"));
        export_row.append(&export_button);
        // The note goes above the row, not below it. Everything from here down
        // is anchored to the bottom of the panel, so a note appearing when the
        // format changes would otherwise shove the Export button out from under
        // the pointer that just picked the format.
        imp.format_note.add_css_class("dim-label");
        imp.format_note.set_xalign(0.0);
        imp.format_note.set_wrap(true);
        // Both notes go above every control in this block. Everything from
        // here down is anchored to the bottom of the panel, so a note that
        // grows to a second line would otherwise slide the controls above it
        // out from under the pointer. Only the notes themselves move.

        // How hard the lossy formats squeeze, when no size is being aimed at.
        let quality_caption = gtk::Label::new(Some("Quality"));
        quality_caption.add_css_class("dim-label");
        let quality = &imp.quality_scale;
        quality.set_draw_value(true);
        quality.set_value_pos(gtk::PositionType::Right);
        quality.set_digits(0);
        quality.set_hexpand(true);
        quality.set_value(f64::from(export::DEFAULT_QUALITY));
        quality.set_tooltip_text(Some(
            "How much detail JPEG keeps. Higher is a bigger file; the lossless \
             formats ignore it.",
        ));
        imp.quality_row.append(&quality_caption);
        imp.quality_row.append(quality);
        sending_box.append(&imp.quality_row);

        // Aiming at a file size: the thing people otherwise go to an
        // advertising-funded website for.
        imp.size_wanted.set_tooltip_text(Some(
            "Squeeze the file down to this, or pad it up to it if it is smaller",
        ));
        imp.size_wanted.connect_toggled(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.describe_target_size()
        ));
        // Visible only once the panel knows the format; set after both exist.
        imp.size_value.set_value(500.0);
        imp.size_value.set_width_chars(5);
        imp.size_value.set_hexpand(true);
        imp.size_value.connect_value_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.describe_target_size()
        ));
        let units = gtk::StringList::new(&["KB", "MB"]);
        imp.size_unit.set_model(Some(&units));
        imp.size_unit.set_selected(0);
        imp.size_unit.connect_selected_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.describe_target_size()
        ));
        for control in [
            imp.size_value.upcast_ref::<gtk::Widget>(),
            imp.size_unit.upcast_ref::<gtk::Widget>(),
        ] {
            imp.size_wanted
                .bind_property("active", control, "sensitive")
                .sync_create()
                .build();
        }
        let size_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        size_row.append(&imp.size_value);
        size_row.append(&imp.size_unit);
        imp.size_note.add_css_class("dim-label");
        imp.size_note.set_xalign(0.0);
        imp.size_note.set_wrap(true);

        sending_box.append(&imp.size_wanted);
        sending_box.append(&size_row);
        sending_box.append(&export_row);
        // Both notes after every control, for the same reason.
        sending_box.append(&imp.format_note);
        sending_box.append(&imp.size_note);
        self.describe_format();
        self.describe_target_size();

        // The way out that keeps nothing. Above the save buttons rather than
        // beside them, so leaving and committing are never one slip apart.
        let cancel = gtk::Button::with_label("Cancel");
        cancel.set_tooltip_text(Some("Leave the editor without saving (Esc)"));
        cancel.set_action_name(Some("win.edit-cancel"));
        panel.append(&cancel);

        // Two ways out with pixels, not three: write back over the original,
        // or write a copy with the format, size and place of your choosing.
        // Export covers everything the old Save As did and more.
        let save = gtk::Button::with_label("Save");
        save.add_css_class("suggested-action");
        save.set_tooltip_text(Some(
            "Write these changes back over the original file (Ctrl+S)",
        ));
        save.set_action_name(Some("win.save"));
        panel.append(&save);

        // Clicking or dragging a piece of text has to reach the panel, the
        // same way typing in the panel reaches the picture.
        imp.view.canvas().connect_text_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move || {
                window.sync_text_panel();
                // A stroke or a line of text is now something undo can take
                // back, so the action has to be told it has work to do.
                window.update_edit_state();
            }
        ));

        // Dragging a handle has to reach the numbers, the same way typing a
        // number reaches the handles.
        imp.view.canvas().connect_resize_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.sync_resize_panel()
        ));

        imp.view.canvas().connect_crop_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |size| {
                window.imp().crop_size.set_text(&match size {
                    Some((w, h)) => format!("Selection: {w} × {h} px"),
                    None => "Draw an area to keep".to_string(),
                });
            }
        ));

        panel
    }

    /// A number was typed or stepped: push it at the canvas, which redraws the
    /// picture at that size without resampling anything.
    fn size_typed(&self, width_led: bool) {
        let imp = self.imp();
        if imp.syncing_panel.get() {
            return;
        }
        let canvas = imp.view.canvas();
        let Some((nw, nh)) = canvas.natural_size() else {
            return;
        };
        let mut width = imp.width_spin.value().round().max(1.0);
        let mut height = imp.height_spin.value().round().max(1.0);
        if imp.keep_aspect.is_active() {
            let ratio = f64::from(nw) / f64::from(nh).max(1e-9);
            // Whichever box was touched leads; the other follows.
            if width_led {
                height = (width / ratio).round().max(1.0);
            } else {
                width = (height * ratio).round().max(1.0);
            }
        }
        // Writing the size back would rewrite the box being typed into, under
        // the cursor, so only the other one is touched and the echo from the
        // canvas is suppressed for the duration.
        imp.syncing_panel.set(true);
        canvas.set_target_size(width as u32, height as u32);
        if width_led {
            imp.height_spin.set_value(height);
        } else {
            imp.width_spin.set_value(width);
        }
        imp.syncing_panel.set(false);
        self.describe_size();
    }

    /// One tool at a time. Five sections open at once made a sidebar taller
    /// than the window, and you can only use one of them anyway.
    fn close_other_sections(&self, keep: &gtk::ToggleButton) {
        let imp = self.imp();
        for section in [
            &imp.crop_toggle,
            &imp.resize_toggle,
            &imp.adjust_toggle,
            &imp.draw_toggle,
            &imp.text_toggle,
            &imp.export_toggle,
        ] {
            if section != keep && section.is_active() {
                section.set_active(false);
            }
        }
    }

    /// Hand the canvas whichever tool is pressed in, or none.
    fn sync_draw_tool(&self) {
        let imp = self.imp();
        let chosen = if imp.draw_toggle.is_active() {
            imp.draw_tools
                .borrow()
                .iter()
                .position(|button| button.is_active())
                .and_then(|index| draw::TOOLS.get(index).copied())
        } else {
            None
        };
        let canvas = imp.view.canvas();
        canvas.set_draw_colour(imp.draw_colour.rgba());
        canvas.set_draw_width(imp.draw_width.value());
        canvas.set_draw_tool(chosen);
        imp.draw_hint.set_text(match chosen {
            Some(tool) => match tool {
                draw::Tool::Pen | draw::Tool::Highlighter => "Drag on the picture to draw.",
                _ => "Drag on the picture from one corner to the other.",
            },
            None => "Pick a tool, then drag on the picture.",
        });
    }

    /// Change the selected item, unless the panel is only echoing the canvas
    /// back at itself.
    fn edit_text(&self, change: impl FnOnce(&mut crate::text::TextItem)) {
        if self.imp().syncing_panel.get() {
            return;
        }
        self.imp().view.canvas().update_selected_text(change);
    }

    /// The three style toggles and the two colours, pushed together: they all
    /// come from the same widgets and it is not worth a closure each.
    fn push_text_style(&self) {
        let imp = self.imp();
        if imp.syncing_panel.get() {
            return;
        }
        let (bold, italic, underline) = (
            imp.text_bold.is_active(),
            imp.text_italic.is_active(),
            imp.text_underline.is_active(),
        );
        let (colour, background) = (imp.text_colour.rgba(), imp.text_background.rgba());
        imp.view.canvas().update_selected_text(move |item| {
            item.bold = bold;
            item.italic = italic;
            item.underline = underline;
            item.colour = colour;
            item.background = background;
        });
    }

    /// Show whichever item is selected. With none, the controls keep whatever
    /// they were set to and become the recipe for the next one added.
    fn sync_text_panel(&self) {
        let imp = self.imp();
        let canvas = imp.view.canvas();
        let selected = canvas.selected_text();
        imp.text_hint.set_text(&match (&selected, canvas.has_text()) {
            (Some(_), _) => "Drag it into place on the picture.".to_string(),
            (None, true) => "Click a piece of text on the picture to select it.".to_string(),
            (None, false) => "Add lays a new line in the middle of the view.".to_string(),
        });
        let Some(item) = selected else {
            self.update_text_actions();
            return;
        };
        imp.syncing_panel.set(true);
        imp.text_entry.set_text(&item.content);
        imp.text_size.set_value(item.size);
        imp.text_bold.set_active(item.bold);
        imp.text_italic.set_active(item.italic);
        imp.text_underline.set_active(item.underline);
        imp.text_colour.set_rgba(&item.colour);
        imp.text_background.set_rgba(&item.background);
        imp.syncing_panel.set(false);
        self.update_text_actions();
    }

    fn update_text_actions(&self) {
        let selected = self.imp().view.canvas().selected_text().is_some();
        if let Some(action) = self
            .lookup_action("text-remove")
            .and_downcast::<gio::SimpleAction>()
        {
            action.set_enabled(selected);
        }
    }

    /// A new item takes its look from whatever the controls currently show, so
    /// adding two in a row gives two that match.
    fn text_from_panel(&self) -> crate::text::TextItem {
        let imp = self.imp();
        let content = imp.text_entry.text().to_string();
        crate::text::TextItem {
            content: if content.is_empty() {
                "Text".to_string()
            } else {
                content
            },
            size: imp.text_size.value(),
            family: imp
                .text_font
                .font_desc()
                .and_then(|desc| desc.family())
                .map(|f| f.to_string())
                .unwrap_or_else(|| "Sans".to_string()),
            bold: imp.text_bold.is_active(),
            italic: imp.text_italic.is_active(),
            underline: imp.text_underline.is_active(),
            colour: imp.text_colour.rgba(),
            background: imp.text_background.rgba(),
            ..Default::default()
        }
    }

    /// The size an export should aim for, if one was asked for.
    fn size_wanted(&self) -> Option<u64> {
        let imp = self.imp();
        if !imp.size_wanted.is_active() {
            return None;
        }
        let unit = if imp.size_unit.selected() == 1 {
            compress::MB
        } else {
            compress::KB
        };
        Some((imp.size_value.value().max(1.0) as u64).saturating_mul(unit))
    }

    /// The quality to write with, where the format has a dial and nothing
    /// else is choosing it.
    fn quality(&self) -> Option<u8> {
        let imp = self.imp();
        self.chosen_target()
            .filter(|target| target.lossy())
            .map(|_| imp.quality_scale.value().round().clamp(1.0, 100.0) as u8)
    }

    fn chosen_target(&self) -> Option<&'static export::Target> {
        export::TARGETS.get(self.imp().format_drop.selected() as usize)
    }

    /// The dial is only meaningful for a lossy format, and only when a size
    /// target is not already choosing the quality for you.
    fn update_quality_row(&self) {
        let imp = self.imp();
        let lossy = self.chosen_target().map(|t| t.lossy()).unwrap_or(false);
        // Always present, so choosing a format cannot slide the controls
        // below it; just inert when there is no quality to choose.
        imp.quality_scale.set_sensitive(lossy && !imp.size_wanted.is_active());
    }

    /// What aiming at a size will mean for this format, or what the last
    /// attempt achieved.
    fn describe_target_size(&self) {
        let imp = self.imp();
        self.update_quality_row();
        let Some(wanted) = self.size_wanted() else {
            let current = imp
                .current
                .borrow()
                .as_ref()
                .and_then(|path| std::fs::metadata(path).ok())
                .map(|meta| meta.len());
            imp.size_note.set_visible(current.is_some());
            if let Some(size) = current {
                imp.size_note
                    .set_text(&format!("This file is {} on disk.", compress::describe(size)));
            }
            return;
        };
        let lossy = self.chosen_target().map(|target| target.lossy()).unwrap_or(false);
        imp.size_note.set_visible(true);
        imp.size_note.set_text(&if lossy {
            format!(
                "Quality will be dialled to land just under {}, and the picture \
                 scaled down only if quality alone cannot get there.",
                compress::describe(wanted)
            )
        } else {
            format!(
                "This format has no quality dial, so {} can only be reached by \
                 scaling the picture down. JPEG will hold more detail at a size.",
                compress::describe(wanted)
            )
        });
    }

    /// What the chosen format will cost the picture, if anything.
    fn describe_format(&self) {
        let imp = self.imp();
        let note = export::TARGETS
            .get(imp.format_drop.selected() as usize)
            .and_then(|target| target.caveat);
        imp.format_note.set_visible(note.is_some());
        imp.format_note.set_text(note.unwrap_or_default());
    }

    /// The line under the boxes, and whether Reset and Apply have anything to
    /// act on.
    fn describe_size(&self) {
        let imp = self.imp();
        let canvas = imp.view.canvas();
        let (Some((w, h)), Some((nw, nh))) = (canvas.target_size(), canvas.natural_size()) else {
            return;
        };
        let percent = f64::from(w) / f64::from(nw).max(1e-9) * 100.0;
        imp.natural_label.set_text(&if (w, h) == (nw, nh) {
            format!("Original size, {nw} × {nh}")
        } else {
            format!("From {nw} × {nh} — {percent:.0}% of the width")
        });
        self.update_resize_actions();
    }

    /// Put the canvas's size back into the boxes, after a handle drag or a
    /// bake changed it behind their back.
    fn sync_resize_panel(&self) {
        let imp = self.imp();
        let canvas = imp.view.canvas();
        let (Some((w, h)), Some((nw, nh))) = (canvas.target_size(), canvas.natural_size()) else {
            return;
        };
        let _ = (nw, nh);
        imp.syncing_panel.set(true);
        imp.width_spin.set_value(f64::from(w));
        imp.height_spin.set_value(f64::from(h));
        imp.syncing_panel.set(false);
        self.describe_size();
    }

    fn update_resize_actions(&self) {
        let pending = self.imp().view.canvas().has_resize();
        for name in ["resize-reset", "resize-apply"] {
            if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(pending);
            }
        }
    }

    /// A slider moved: push the three values at the canvas, which shows them
    /// without touching a pixel of the image.
    fn tone_changed(&self) {
        let imp = self.imp();
        if imp.syncing_panel.get() {
            return;
        }
        imp.view.canvas().set_adjustments(Adjustments {
            brightness: imp.brightness_scale.value(),
            contrast: imp.contrast_scale.value(),
            saturation: imp.saturation_scale.value(),
        });
        self.update_tone_actions();
    }

    /// Put the canvas's values back into the sliders, after something baked
    /// them into the pixels and reset them.
    fn sync_tone_panel(&self) {
        let imp = self.imp();
        let adjust = imp.view.canvas().adjustments();
        imp.syncing_panel.set(true);
        imp.brightness_scale.set_value(adjust.brightness);
        imp.contrast_scale.set_value(adjust.contrast);
        imp.saturation_scale.set_value(adjust.saturation);
        imp.syncing_panel.set(false);
        self.update_tone_actions();
    }

    fn update_tone_actions(&self) {
        let pending = !self.imp().view.canvas().adjustments().is_identity();
        for name in ["adjust-reset", "adjust-apply"] {
            if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(pending);
            }
        }
    }

    /// Enter or leave the interactive crop.
    fn set_cropping(&self, active: bool) {
        let imp = self.imp();
        imp.crop_options.set_visible(active);
        imp.view.canvas().set_cropping(active);
        if active {
            imp.pending_crop.set_visible(false);
        }
        self.update_crop_actions();
    }

    fn update_crop_actions(&self) {
        let cropping = self.imp().view.canvas().is_cropping();
        for name in ["crop-apply", "crop-reset"] {
            if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(cropping);
            }
        }
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

    /// The rotation bar: two quarter-turn buttons either side of a free-angle
    /// slider, with the current angle spelled out between them.
    fn build_rotation_bar(&self) -> &gtk::Box {
        let imp = self.imp();
        let bar = &imp.rotation_bar;

        bar.add_css_class("toolbar");
        bar.set_margin_top(6);
        bar.set_margin_bottom(6);
        bar.set_margin_start(12);
        bar.set_margin_end(12);
        // Nothing to rotate until an image is open.
        bar.set_visible(false);

        let left = gtk::Button::from_icon_name("object-rotate-left-symbolic");
        left.set_tooltip_text(Some("Rotate Left 90°"));
        left.set_action_name(Some("win.rotate-left"));
        left.add_css_class("flat");

        let right = gtk::Button::from_icon_name("object-rotate-right-symbolic");
        right.set_tooltip_text(Some("Rotate Right 90°"));
        right.set_action_name(Some("win.rotate-right"));
        right.add_css_class("flat");

        let slider = &imp.rotation_scale;
        slider.set_hexpand(true);
        slider.set_draw_value(false);
        // The angle runs either side of zero, so a bar filling from the far
        // left would read as though 0 were most of the way along.
        slider.set_has_origin(false);
        slider.set_value(0.0);
        // Detents at the quarter turns and at upright, so the useful angles are
        // easy to find by eye.
        for mark in [-180.0, -90.0, 0.0, 90.0, 180.0] {
            slider.add_mark(mark, gtk::PositionType::Bottom, None);
        }

        let label = &imp.rotation_label;
        // Fixed width, otherwise the slider shuffles sideways as digits appear.
        label.set_width_chars(6);
        label.set_xalign(1.0);
        label.add_css_class("numeric");
        label.add_css_class("dim-label");

        let flip_h = &imp.flip_h_button;
        flip_h.set_icon_name("object-flip-horizontal-symbolic");
        flip_h.set_tooltip_text(Some("Flip Horizontally"));
        flip_h.set_action_name(Some("win.flip-horizontal"));
        flip_h.add_css_class("flat");

        let flip_v = &imp.flip_v_button;
        flip_v.set_icon_name("object-flip-vertical-symbolic");
        flip_v.set_tooltip_text(Some("Flip Vertically"));
        flip_v.set_action_name(Some("win.flip-vertical"));
        flip_v.add_css_class("flat");

        let reset = gtk::Button::from_icon_name("edit-undo-symbolic");
        reset.set_tooltip_text(Some("Reset Rotation"));
        reset.set_action_name(Some("win.rotate-reset"));
        reset.add_css_class("flat");

        bar.append(&left);
        bar.append(slider);
        bar.append(label);
        bar.append(&right);
        bar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        bar.append(flip_h);
        bar.append(flip_v);
        bar.append(&reset);

        slider.connect_value_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |scale| {
                if window.imp().syncing.get() {
                    return;
                }
                window.imp().view.canvas().set_rotation(scale.value());
            }
        ));

        imp.view.canvas().connect_flip_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |h, v| {
                // Reflect state without re-triggering the actions.
                window.imp().flip_h_button.set_active(h);
                window.imp().flip_v_button.set_active(v);
            }
        ));

        imp.view.canvas().connect_rotation_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |degrees| window.show_rotation(degrees)
        ));

        bar
    }

    /// Show or hide the transform options. While they are open the header
    /// carries only the close button, so the controls have the user's full
    /// attention and there is one obvious way back.
    fn set_transform_open(&self, open: bool) {
        let imp = self.imp();
        // Nothing to transform with no image, and Escape must stay harmless
        // when the options are already closed.
        if open && !imp.view.canvas().has_image() {
            return;
        }
        imp.transform_open.set(open);
        imp.rotation_bar.set_visible(open);
        imp.header_stack
            .set_visible_child_name(if open { "transform" } else { "normal" });
        self.update_navigation();
    }

    /// Push the canvas's angle back into the slider and the readout.
    fn show_rotation(&self, degrees: f64) {
        let imp = self.imp();
        imp.rotation_label.set_text(&format!("{degrees:.0}°"));
        if (imp.rotation_scale.value() - degrees).abs() > 0.01 {
            imp.syncing.set(true);
            imp.rotation_scale.set_value(degrees);
            imp.syncing.set(false);
        }
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
                move |_, _| window.imp().view.canvas().zoom_by(factor, None)
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
                window.imp().resize_button.set_sensitive(has_file);
                window.update_navigation();
            }
        ));

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
                imp.resize_toggle.set_active(!imp.resize_toggle.is_active());
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
            move |_, _| window.copy_to_clipboard()
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
                } else if window.imp().transform_open.get() {
                    window.set_transform_open(false);
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

        let transform_close = gio::SimpleAction::new("transform-close", None);
        transform_close.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.set_transform_open(false)
        ));
        self.add_action(&transform_close);

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
            move |_, _| window.imp().view.canvas().zoom_fit()
        ));
        self.add_action(&zoom_fit);

        let zoom_actual = gio::SimpleAction::new("zoom-actual", None);
        zoom_actual.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.imp().view.canvas().zoom_actual()
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
        filter.set_name(Some("Images"));
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
        ] {
            filter.add_mime_type(mime);
        }
        // Raw formats are matched by suffix: the shared-mime database does not
        // recognise every camera maker's container, and a raw file the picker
        // greys out is a raw file the user cannot open.
        for suffix in [
            "3fr", "arw", "cr2", "cr3", "crw", "dcr", "dng", "erf", "fff", "iiq", "kdc", "mef",
            "mos", "mrw", "nef", "nrw", "orf", "pef", "raf", "raw", "rw2", "rwl", "sr2", "srf",
            "srw", "x3f", "heic", "heif", "avif", "svg", "svgz",
        ] {
            filter.add_suffix(suffix);
        }

        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);

        let dialog = gtk::FileDialog::builder()
            .title("Open Image")
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
    fn load(&self, path: PathBuf, rescan: bool) {
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
        imp.view.show_loading();

        let (sender, receiver) = async_channel::bounded(1);
        let scan_path = path.clone();
        std::thread::spawn(move || {
            // Both the decode and the directory listing are filesystem work, so
            // they belong on this side of the channel.
            let decoded = loader::decode(&scan_path);
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
                    Ok(image) => {
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
                        window.imp().resize_button.set_sensitive(true);
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
        imp.title.set_title("Simple Viewer");
        imp.title.set_subtitle("");
        imp.rotate_button.set_sensitive(false);
        imp.delete_button.set_sensitive(false);
        imp.copy_button.set_sensitive(false);
        // Turning the toggle off restores the filmstrip and the arrow keys.
        imp.edit_button.set_active(false);
        imp.edit_button.set_sensitive(false);
        imp.resize_button.set_sensitive(false);
        imp.action_bar.set_visible(false);
        self.set_transform_open(false);
        self.update_navigation();
    }

    /// Navigation is pointless with one image; while the transform options are
    /// open the arrow keys belong to the rotation slider; and while editing the
    /// picture on screen is unsaved work, so stepping off it — by key, by arrow
    /// or by thumbnail — is switched off and the filmstrip goes with it.
    fn update_navigation(&self) {
        let imp = self.imp();
        let busy = imp.transform_open.get() || imp.edit_button.is_active();
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

    /// Leave the editor, throwing the session away. Asked for out loud first:
    /// the pixels on screen may be several edits from the file on disk.
    fn cancel_editing(&self) {
        let imp = self.imp();
        if !imp.edit_button.is_active() {
            return;
        }
        let unsaved = imp.dirty.get() || self.has_live_transform();
        let dialog = adw::AlertDialog::new(
            Some("Cancel Editing?"),
            Some(if unsaved {
                "Your unsaved changes to this image will be lost."
            } else {
                "This will close the editor."
            }),
        );
        dialog.add_response("no", "No");
        dialog.add_response("yes", "Yes");
        if unsaved {
            dialog.set_response_appearance("yes", adw::ResponseAppearance::Destructive);
        }
        // Escape and clicking away both mean "carry on editing".
        dialog.set_default_response(Some("no"));
        dialog.set_close_response("no");
        dialog.connect_response(
            None,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, response| {
                    if response == "yes" {
                        window.discard_editing();
                    }
                }
            ),
        );
        dialog.present(Some(self));
    }

    /// Drop the edit session and put the file back on screen untouched.
    fn discard_editing(&self) {
        let imp = self.imp();
        self.reset_editing();
        // Closing the panel also puts the crop tool away and restores the
        // filmstrip, through the toggle's own handler.
        imp.edit_button.set_active(false);
        // The canvas is showing edited pixels with a live rotation possibly on
        // top, so re-read the file rather than trying to unwind either.
        let path = imp.current.borrow().clone();
        if let Some(path) = path {
            self.load(path, false);
        }
    }

    fn fail(&self, message: &str) {
        let imp = self.imp();
        match imp.shown.borrow().as_ref() {
            Some(shown) => {
                imp.title.set_title(&shown.name);
                imp.title.set_subtitle(&shown.subtitle);
            }
            None => {
                imp.title.set_title("Simple Viewer");
                imp.title.set_subtitle("");
            }
        }
        imp.view.show_idle();
        self.toast(message);
    }

    fn toast(&self, message: &str) {
        self.imp().toasts.add_toast(adw::Toast::new(message));
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name("Simple Viewer")
            .application_icon("image-x-generic-symbolic")
            .version(env!("CARGO_PKG_VERSION"))
            .comments("A small image viewer for GNOME.")
            .build();
        about.present(Some(self));
    }
}
