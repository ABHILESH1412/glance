//! The application window: header bar, actions, and the load pipeline.
//!
//! This is a real `GObject` subclass rather than a plain struct behind an `Rc`.
//! That matters: closures capture a weak reference, and tying that reference to
//! the widget's own lifetime is what keeps it valid for as long as the window is
//! on screen.

use std::cell::{Cell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, gio, glib};

use crate::image_view::ImageView;
use crate::loader;

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
        pub rotation_bar: gtk::Box,
        pub rotation_scale: gtk::Scale,
        pub rotation_label: gtk::Label,
        /// Set while pushing the canvas's angle into the slider, so the
        /// slider's own value-changed does not bounce it straight back.
        pub syncing: Cell<bool>,
        pub shown: RefCell<Option<Shown>>,
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

        let zoom_section = gio::Menu::new();
        zoom_section.append(Some("Zoom _In"), Some("win.zoom-in"));
        zoom_section.append(Some("Zoom _Out"), Some("win.zoom-out"));
        zoom_section.append(Some("_Fit to Window"), Some("win.zoom-fit"));
        zoom_section.append(Some("_Actual Size"), Some("win.zoom-actual"));

        let rotate_section = gio::Menu::new();
        rotate_section.append(Some("Rotate _Left"), Some("win.rotate-left"));
        rotate_section.append(Some("Rotate _Right"), Some("win.rotate-right"));
        rotate_section.append(Some("Reset Rotation"), Some("win.rotate-reset"));

        let about_section = gio::Menu::new();
        about_section.append(Some("_About Simple Viewer"), Some("win.about"));

        let menu = gio::Menu::new();
        menu.append_section(None, &zoom_section);
        menu.append_section(None, &rotate_section);
        menu.append_section(None, &about_section);
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Main Menu")
            .menu_model(&menu)
            .primary(true)
            .build();

        let header = adw::HeaderBar::builder()
            .title_widget(&imp.title)
            .build();
        header.pack_start(&open_button);
        header.pack_end(&menu_button);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&imp.toasts));
        toolbar.add_bottom_bar(self.build_rotation_bar());
        self.set_content(Some(&toolbar));

        imp.view.canvas().connect_zoom_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |percent| window.show_zoom(percent)
        ));

        self.setup_drop_target();
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

        let reset = gtk::Button::from_icon_name("edit-undo-symbolic");
        reset.set_tooltip_text(Some("Reset Rotation"));
        reset.set_action_name(Some("win.rotate-reset"));
        reset.add_css_class("flat");

        bar.append(&left);
        bar.append(slider);
        bar.append(label);
        bar.append(&right);
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

        imp.view.canvas().connect_rotation_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |degrees| window.show_rotation(degrees)
        ));

        bar
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
        let imp = self.imp();

        let Some(path) = file.path() else {
            self.toast("That location is not a local file.");
            return;
        };

        let generation = imp.generation.get() + 1;
        imp.generation.set(generation);

        let name = file
            .basename()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "Image".to_string());
        imp.title.set_title(&name);
        imp.title.set_subtitle("Loading…");
        imp.view.show_loading();

        let (sender, receiver) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = sender.send_blocking(loader::decode(&path));
        });

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                let Ok(result) = receiver.recv().await else {
                    window.fail("The image loader stopped unexpectedly.");
                    return;
                };
                if window.imp().generation.get() != generation {
                    return; // Superseded by a newer open.
                }
                match result {
                    Ok(image) => {
                        let subtitle = format!("{} · {} × {}", image.label, image.width, image.height);
                        window.imp().title.set_subtitle(&subtitle);
                        window.imp().shown.replace(Some(Shown { name, subtitle }));
                        window.imp().view.show_image(image);
                        window.imp().rotation_bar.set_visible(true);
                    }
                    Err(message) => window.fail(&message),
                }
            }
        ));
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
