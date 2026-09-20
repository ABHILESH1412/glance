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

use crate::image_view::ImageView;
use crate::filmstrip::{self, FilmStrip};
use crate::loader;
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

mod imp {
    use super::*;

    pub struct Window {
        pub title: adw::WindowTitle,
        pub toasts: adw::ToastOverlay,
        pub view: ImageView,
        pub toolbar: adw::ToolbarView,
        pub fullscreen_button: gtk::Button,
        pub delete_button: gtk::Button,
        /// The file on screen, needed to delete it and to find it again after
        /// the folder changes underneath us.
        pub current: RefCell<Option<PathBuf>>,
        /// Watches the folder so images added or removed elsewhere show up here.
        pub monitor: RefCell<Option<gio::FileMonitor>>,
        pub rescan_timer: RefCell<Option<glib::SourceId>>,
        pub header_stack: gtk::Stack,
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
                current: RefCell::new(None),
                monitor: RefCell::new(None),
                rescan_timer: RefCell::new(None),
                header_stack: gtk::Stack::new(),
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

        imp.toasts.set_child(Some(imp.view.widget()));

        let open_button = gtk::Button::from_icon_name("document-open-symbolic");
        open_button.set_tooltip_text(Some("Open Image"));
        open_button.set_action_name(Some("win.open"));

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

        let delete_button = &imp.delete_button;
        delete_button.set_tooltip_text(Some("Delete Image"));
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
        header.pack_end(delete_button);

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
        toolbar.set_content(Some(&imp.toasts));
        toolbar.add_bottom_bar(self.build_rotation_bar());
        toolbar.add_bottom_bar(&imp.strip);
        self.set_content(Some(toolbar));

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
                    window.load(path, false);
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
        imp.strip.set_visible(!open && imp.playlist.borrow().is_some());
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
                        window.load(path, false);
                    }
                }
            ));
            self.add_action(&action);
        }

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
        // first, then closing the transform options.
        let dismiss = gio::SimpleAction::new("dismiss", None);
        dismiss.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| {
                if window.is_fullscreen() {
                    window.unfullscreen();
                } else {
                    window.set_transform_open(false);
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
        self.load(path, true);
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
        self.set_transform_open(false);
        self.update_navigation();
    }

    /// Navigation is pointless with one image, and while the transform options
    /// are open the arrow keys belong to the rotation slider.
    fn update_navigation(&self) {
        let imp = self.imp();
        let enabled = imp.playlist.borrow().is_some() && !imp.transform_open.get();
        for name in ["next-image", "previous-image"] {
            if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(enabled);
            }
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
