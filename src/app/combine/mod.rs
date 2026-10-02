// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Combine into PDF: pages from any number of PDFs and pictures, laid out in
//! a grid to arrange, then saved as one new PDF.
//!
//! It takes the whole window while it is open, with a header of its own:
//! Cancel and Add Files on the left, Save as PDF on the right. Each file
//! added gets a colour, and every page carries a stripe of it, so after any
//! amount of shuffling it is still plain where each page came from.
//!
//! - Drag pages to put them in order, several at once if several are
//!   selected; drop files from elsewhere between two pages to put them there.
//! - Pointing at a page shows buttons to turn it either way or take it out;
//!   [ and ] turn, and Delete takes out, whatever is selected.
//! - A double-click, or Enter, shows a page across the whole window; arrow
//!   keys step through, and Escape goes back to the grid where it was.
//! - A PDF of more than one page asks which pages to take: all, or a list
//!   like "2-5, 9".
//! - Everything can be undone.
//!
//! Nothing is written until Save, and then only to a new file: the files the
//! pages come from are never changed.

mod tile;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::pdf;
use tile::PageTile;

/// Smallest, starting and largest thumbnail, in pixels along the longer side.
const SIZES: (f64, f64, f64) = (90.0, 150.0, 300.0);
/// A picture's own thumbnail is made this large, at most.
const PICTURE_THUMB: u32 = 512;
/// Text that marks a drag as pages from this grid, not files from elsewhere.
const PAGE_DRAG: &str = "glance-combine-pages";

/// One colour per file, in turn: distinct, and readable as a thin stripe in
/// light and dark alike.
const COLOURS: &[(f32, f32, f32)] = &[
    (0.50, 0.47, 0.87),
    (0.11, 0.62, 0.46),
    (0.85, 0.35, 0.19),
    (0.21, 0.54, 0.87),
    (0.83, 0.33, 0.49),
    (0.73, 0.46, 0.09),
    (0.39, 0.60, 0.13),
    (0.53, 0.53, 0.50),
];

enum Kind {
    Pdf { password: Option<String>, images: pdf::PageImages },
    Picture,
}

/// A file pages came from.
struct Source {
    path: PathBuf,
    name: String,
    kind: Kind,
    colour: gdk::RGBA,
    /// Each page's own width and height: points for a PDF, pixels for a
    /// picture, which has one page.
    shapes: Vec<(f64, f64)>,
    /// Pictures of its pages so far, and the scale each was drawn at.
    thumbs: RefCell<HashMap<usize, (f64, gdk::Texture)>>,
}

/// One page of the document being put together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sheet {
    id: u64,
    source: usize,
    page: usize,
    turn: u8,
}

/// A page's place in the grid.
struct Tile {
    child: gtk::FlowBoxChild,
    picture: PageTile,
    number: gtk::Label,
    caption: gtk::Label,
}

pub struct Combine {
    inner: Rc<Inner>,
}

struct Inner {
    root: adw::ToolbarView,
    title: adw::WindowTitle,
    toasts: adw::ToastOverlay,
    stack: gtk::Stack,
    scroller: gtk::ScrolledWindow,
    flow: gtk::FlowBox,
    legend: gtk::Box,
    save: gtk::Button,
    undo_button: gtk::Button,
    redo_button: gtk::Button,
    preview: PageTile,
    preview_title: gtk::Label,
    sources: RefCell<Vec<Rc<Source>>>,
    sheets: RefCell<Vec<Sheet>>,
    next_id: Cell<u64>,
    undo: RefCell<Vec<Vec<Sheet>>>,
    redo: RefCell<Vec<Vec<Sheet>>>,
    tiles: RefCell<HashMap<u64, Tile>>,
    by_child: RefCell<HashMap<gtk::FlowBoxChild, u64>>,
    /// The pages picked out, and the one a Shift-click runs from.
    picked: RefCell<HashSet<u64>>,
    anchor: Cell<Option<u64>>,
    size: Cell<f64>,
    /// The page shown across the window, by place in the list.
    previewing: Cell<Option<usize>>,
    /// Changed since it was last saved, so leaving asks first.
    changed: Cell<bool>,
    /// Files still to be added, one at a time, and where the next goes.
    queue: RefCell<Vec<(PathBuf, Option<String>)>>,
    insert_at: Cell<Option<usize>>,
    busy: Cell<bool>,
    asked: Cell<bool>,
    /// The paper pictures were last put on, if ever chosen.
    paper: Cell<Option<pdf::Paper>>,
    on_paper: RefCell<Option<Box<dyn Fn(pdf::Paper)>>>,
    /// Leave: with a saved file to open, or none.
    on_close: RefCell<Option<Rc<dyn Fn(Option<PathBuf>)>>>,
}

impl Combine {
    pub fn new() -> Self {
        let title = adw::WindowTitle::new("Combine into PDF", "");
        let header = adw::HeaderBar::builder().title_widget(&title).build();
        let cancel = gtk::Button::with_mnemonic("_Cancel");
        let add = gtk::Button::builder()
            .child(&adw::ButtonContent::builder().icon_name("list-add-symbolic").label("_Add Files…").use_underline(true).build())
            .tooltip_text("Add PDFs or pictures (Ctrl+O)")
            .build();
        let save = gtk::Button::builder().label("_Save as PDF…").use_underline(true).css_classes(["suggested-action"]).build();
        save.set_tooltip_text(Some("Write the pages as one new PDF (Ctrl+S)"));
        let undo_button = gtk::Button::builder().icon_name("edit-undo-symbolic").tooltip_text("Undo (Ctrl+Z)").build();
        let redo_button =
            gtk::Button::builder().icon_name("edit-redo-symbolic").tooltip_text("Redo (Ctrl+Shift+Z)").build();
        header.pack_start(&cancel);
        header.pack_start(&add);
        header.pack_end(&save);
        header.pack_end(&redo_button);
        header.pack_end(&undo_button);

        // Which file is which colour, and how big the pages are drawn.
        let legend = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        legend.set_hexpand(true);
        let legend_scroller = gtk::ScrolledWindow::builder()
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .child(&legend)
            .build();
        let zoom = gtk::Scale::with_range(gtk::Orientation::Horizontal, SIZES.0, SIZES.2, 10.0);
        zoom.set_value(SIZES.1);
        zoom.set_width_request(120);
        zoom.set_draw_value(false);
        zoom.set_tooltip_text(Some("Size of the pages"));
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        bar.set_margin_start(12);
        bar.set_margin_end(12);
        bar.set_margin_top(4);
        bar.set_margin_bottom(4);
        bar.append(&legend_scroller);
        bar.append(&gtk::Image::from_icon_name("zoom-out-symbolic"));
        bar.append(&zoom);
        bar.append(&gtk::Image::from_icon_name("zoom-in-symbolic"));

        let flow = gtk::FlowBox::builder()
            // Selection is kept here rather than by the grid: its own
            // multiple selection turns every drag into a rubber band, and a
            // drag here is for moving pages.
            .selection_mode(gtk::SelectionMode::None)
            .activate_on_single_click(false)
            .homogeneous(true)
            .column_spacing(12)
            .row_spacing(18)
            .max_children_per_line(64)
            .valign(gtk::Align::Start)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();
        flow.add_css_class("combine-grid");
        let scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&flow).build();
        // So moving the focus with the keys scrolls the page into view.
        flow.set_vadjustment(&scroller.vadjustment());
        let grid = gtk::Box::new(gtk::Orientation::Vertical, 0);
        grid.append(&bar);
        grid.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        grid.append(&scroller);

        let add_first = gtk::Button::builder()
            .label("_Add Files…")
            .use_underline(true)
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        let empty = adw::StatusPage::builder()
            .icon_name("view-grid-symbolic")
            .title("Nothing to Combine Yet")
            .description("Add PDFs and pictures, or drop them here. Then put the pages in order and save them as one PDF.")
            .child(&add_first)
            .build();

        // One page across the window.
        let back = gtk::Button::builder()
            .child(&adw::ButtonContent::builder().icon_name("go-previous-symbolic").label("_Back").use_underline(true).build())
            .tooltip_text("Back to all the pages (Esc)")
            .build();
        let previous = gtk::Button::builder().icon_name("go-up-symbolic").tooltip_text("Previous page (←)").build();
        let next = gtk::Button::builder().icon_name("go-down-symbolic").tooltip_text("Next page (→)").build();
        for button in [&back, &previous, &next] {
            button.add_css_class("flat");
        }
        let preview_title = gtk::Label::builder().hexpand(true).ellipsize(gtk::pango::EllipsizeMode::Middle).build();
        let preview_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        preview_bar.set_margin_start(6);
        preview_bar.set_margin_end(6);
        preview_bar.set_margin_top(4);
        preview_bar.set_margin_bottom(4);
        preview_bar.append(&back);
        preview_bar.append(&preview_title);
        preview_bar.append(&previous);
        preview_bar.append(&next);
        let preview = PageTile::new();
        preview.set_hexpand(true);
        preview.set_vexpand(true);
        preview.set_margin_top(18);
        preview.set_margin_bottom(18);
        preview.set_margin_start(18);
        preview.set_margin_end(18);
        let viewing = gtk::Box::new(gtk::Orientation::Vertical, 0);
        viewing.append(&preview_bar);
        viewing.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        viewing.append(&preview);

        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&grid, Some("grid"));
        stack.add_named(&viewing, Some("preview"));
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&stack));
        let root = adw::ToolbarView::new();
        root.add_top_bar(&header);
        root.set_content(Some(&toasts));

        let inner = Rc::new(Inner {
            root,
            title,
            toasts,
            stack,
            scroller,
            flow,
            legend,
            save,
            undo_button,
            redo_button,
            preview,
            preview_title,
            sources: RefCell::default(),
            sheets: RefCell::default(),
            next_id: Cell::new(1),
            undo: RefCell::default(),
            redo: RefCell::default(),
            tiles: RefCell::default(),
            by_child: RefCell::default(),
            picked: RefCell::default(),
            anchor: Cell::new(None),
            size: Cell::new(SIZES.1),
            previewing: Cell::new(None),
            changed: Cell::new(false),
            queue: RefCell::default(),
            insert_at: Cell::new(None),
            busy: Cell::new(false),
            asked: Cell::new(false),
            paper: Cell::new(None),
            on_paper: RefCell::default(),
            on_close: RefCell::default(),
        });

        let weak = Rc::downgrade(&inner);
        let on = move |f: fn(&Rc<Inner>)| {
            let weak = weak.clone();
            move || {
                if let Some(inner) = weak.upgrade() {
                    f(&inner);
                }
            }
        };
        let (a, b, c) = (on(Inner::choose_files), on(Inner::choose_files), on(Inner::ask_to_save));
        add.connect_clicked(move |_| a());
        add_first.connect_clicked(move |_| b());
        inner.save.connect_clicked(move |_| c());
        let (u, r, x) = (on(Inner::step_undo), on(Inner::step_redo), on(Inner::ask_to_leave));
        inner.undo_button.connect_clicked(move |_| u());
        inner.redo_button.connect_clicked(move |_| r());
        cancel.connect_clicked(move |_| x());
        let back_to_grid = on(Inner::close_preview);
        back.connect_clicked(move |_| back_to_grid());
        let weak = Rc::downgrade(&inner);
        previous.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.step_preview(-1);
            }
        });
        let weak = Rc::downgrade(&inner);
        next.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.step_preview(1);
            }
        });

        let weak = Rc::downgrade(&inner);
        zoom.connect_value_changed(move |zoom| {
            let Some(inner) = weak.upgrade() else { return };
            inner.size.set(zoom.value());
            for tile in inner.tiles.borrow().values() {
                inner.size_tile(tile);
            }
            inner.want_soon();
        });

        let weak = Rc::downgrade(&inner);
        inner.flow.connect_child_activated(move |_, child| {
            let Some(inner) = weak.upgrade() else { return };
            inner.open_preview(usize::try_from(child.index()).unwrap_or(0));
        });

        // Only pages on screen are drawn, so draw more as they come into view.
        {
            let adjustment = inner.scroller.vadjustment();
            let weak = Rc::downgrade(&inner);
            adjustment.connect_value_changed(move |_| {
                if let Some(inner) = weak.upgrade() {
                    inner.want_soon();
                }
            });
            let weak = Rc::downgrade(&inner);
            adjustment.connect_page_size_notify(move |_| {
                if let Some(inner) = weak.upgrade() {
                    inner.want_soon();
                }
            });
        }

        // A click between the pages lets go of the ones picked.
        let empty_click = gtk::GestureClick::new();
        let weak = Rc::downgrade(&inner);
        empty_click.connect_released(move |_, _, x, y| {
            let Some(inner) = weak.upgrade() else { return };
            if inner.flow.child_at_pos(x as i32, y as i32).is_none() {
                inner.unpick_all();
            }
        });
        inner.flow.add_controller(empty_click);

        Inner::connect_drops(&inner);
        Inner::connect_keys(&inner);
        inner.refresh();
        Combine { inner }
    }

    pub fn widget(&self) -> &adw::ToolbarView {
        &self.inner.root
    }

    /// Called when leaving, with a saved file to open if the reader asked.
    pub fn connect_close(&self, f: impl Fn(Option<PathBuf>) + 'static) {
        self.inner.on_close.replace(Some(Rc::new(f)));
    }

    /// The paper pictures last went on, and a way to remember the next.
    pub fn set_paper(&self, paper: Option<pdf::Paper>, remember: impl Fn(pdf::Paper) + 'static) {
        self.inner.paper.set(paper);
        self.inner.on_paper.replace(Some(Box::new(remember)));
    }

    /// Start afresh, with these files in.
    pub fn start(&self, files: Vec<(PathBuf, Option<String>)>) {
        let inner = &self.inner;
        inner.sources.borrow_mut().clear();
        inner.sheets.borrow_mut().clear();
        inner.undo.borrow_mut().clear();
        inner.redo.borrow_mut().clear();
        inner.tiles.borrow_mut().clear();
        inner.by_child.borrow_mut().clear();
        inner.queue.borrow_mut().clear();
        inner.changed.set(false);
        inner.previewing.set(None);
        inner.refresh();
        inner.add(files, None);
    }

    /// Add files at the end, as if dropped there.
    pub fn add_files(&self, files: Vec<PathBuf>) {
        self.inner.add(files.into_iter().map(|path| (path, None)).collect(), None);
    }
}

impl Inner {
    fn close(&self, open: Option<PathBuf>) {
        let callback = self.on_close.borrow().clone();
        if let Some(callback) = callback {
            callback(open);
        }
    }

    fn toast(&self, message: &str) {
        self.toasts.add_toast(adw::Toast::new(message));
    }

    // -- adding files --

    fn choose_files(self: &Rc<Self>) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("PDFs and pictures"));
        filter.add_mime_type("application/pdf");
        filter.add_mime_type("image/*");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let chooser = gtk::FileDialog::builder().title("Add to the PDF").filters(&filters).modal(true).build();
        let window = self.root.root().and_downcast::<gtk::Window>();
        let weak = Rc::downgrade(self);
        chooser.open_multiple(window.as_ref(), gio::Cancellable::NONE, move |result| {
            let (Some(inner), Ok(files)) = (weak.upgrade(), result) else { return };
            let paths: Vec<(PathBuf, Option<String>)> = (0..files.n_items())
                .filter_map(|i| files.item(i).and_downcast::<gio::File>())
                .filter_map(|file| file.path())
                .map(|path| (path, None))
                .collect();
            inner.add(paths, None);
        });
    }

    /// Add files, one after another, before the page at `at`, or at the end.
    fn add(self: &Rc<Self>, files: Vec<(PathBuf, Option<String>)>, at: Option<usize>) {
        if files.is_empty() {
            return;
        }
        if self.queue.borrow().is_empty() {
            self.insert_at.set(at);
        }
        self.queue.borrow_mut().extend(files);
        self.next_file();
    }

    fn next_file(self: &Rc<Self>) {
        if self.busy.get() {
            return;
        }
        let next = {
            let mut queue = self.queue.borrow_mut();
            (!queue.is_empty()).then(|| queue.remove(0))
        };
        let Some((path, password)) = next else { return };
        self.busy.set(true);
        let (sender, receiver) = async_channel::bounded(1);
        let worker_path = path.clone();
        std::thread::spawn(move || {
            let found = if pdf::is_pdf(&worker_path) {
                Found::Pdf(pdf::open(&worker_path, password.as_deref()))
            } else {
                Found::Picture(picture(&worker_path))
            };
            let _ = sender.send_blocking(found);
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(found) = receiver.recv().await else { return };
            let Some(inner) = weak.upgrade() else { return };
            inner.busy.set(false);
            inner.found(path, found);
        });
    }

    fn found(self: &Rc<Self>, path: PathBuf, found: Found) {
        let name = file_name(&path);
        match found {
            Found::Pdf(Ok(opened)) => {
                if !opened.allowed.assemble {
                    self.toast(&format!("The author of “{name}” does not allow taking its pages out."));
                    self.next_file();
                    return;
                }
                let count = opened.pages.len();
                if count == 1 {
                    self.put(path, opened.password, opened.pages, vec![0]);
                    self.next_file();
                } else {
                    let weak = Rc::downgrade(self);
                    let (password, shapes) = (opened.password, opened.pages);
                    ask_pages(&self.root, &name, count, move |pages| {
                        let Some(inner) = weak.upgrade() else { return };
                        if let Some(pages) = pages {
                            inner.put(path.clone(), password.clone(), shapes.clone(), pages);
                        }
                        inner.next_file();
                    });
                }
            }
            Found::Pdf(Err(pdf::OpenError::Locked { tried })) => {
                let weak = Rc::downgrade(self);
                ask_password(&self.root, &name, tried, move |password| {
                    let Some(inner) = weak.upgrade() else { return };
                    if let Some(password) = password {
                        inner.queue.borrow_mut().insert(0, (path.clone(), Some(password)));
                    }
                    inner.next_file();
                });
            }
            Found::Pdf(Err(pdf::OpenError::Failed(message))) => {
                self.toast(&message);
                self.next_file();
            }
            Found::Picture(Ok((width, height, thumb))) => {
                let source = self.new_source(&path, Kind::Picture, vec![(f64::from(width), f64::from(height))]);
                self.sources.borrow()[source].thumbs.borrow_mut().insert(0, (f64::INFINITY, thumb));
                self.insert_pages(source, vec![0]);
                self.next_file();
            }
            Found::Picture(Err(_)) => {
                self.toast(&format!("“{name}” is not a PDF or a picture Glance can read."));
                self.next_file();
            }
        }
    }

    /// A PDF's chosen pages, into the list.
    fn put(self: &Rc<Self>, path: PathBuf, password: Option<String>, shapes: Vec<(f64, f64)>, pages: Vec<usize>) {
        let weak = Rc::downgrade(self);
        let index = self.sources.borrow().len();
        let images = pdf::PageImages::start(&path, password.clone(), move |page, scale, texture| {
            if let Some(inner) = weak.upgrade() {
                inner.drawn(index, page, scale, texture);
            }
        });
        let source = self.new_source(&path, Kind::Pdf { password, images }, shapes);
        self.insert_pages(source, pages);
    }

    fn new_source(&self, path: &Path, kind: Kind, shapes: Vec<(f64, f64)>) -> usize {
        let mut sources = self.sources.borrow_mut();
        let (r, g, b) = COLOURS[sources.len() % COLOURS.len()];
        sources.push(Rc::new(Source {
            path: path.to_path_buf(),
            name: file_name(path),
            kind,
            colour: gdk::RGBA::new(r, g, b, 1.0),
            shapes,
            thumbs: RefCell::default(),
        }));
        sources.len() - 1
    }

    fn insert_pages(self: &Rc<Self>, source: usize, pages: Vec<usize>) {
        self.remember();
        let added: Vec<Sheet> = pages
            .into_iter()
            .map(|page| {
                let id = self.next_id.get();
                self.next_id.set(id + 1);
                Sheet { id, source, page, turn: 0 }
            })
            .collect();
        let count = added.len();
        {
            let mut sheets = self.sheets.borrow_mut();
            let at = self.insert_at.get().unwrap_or(sheets.len()).min(sheets.len());
            sheets.splice(at..at, added);
            self.insert_at.set(Some(at + count));
            if self.queue.borrow().is_empty() {
                self.insert_at.set(None);
            }
        }
        self.changed.set(true);
        self.refresh();
    }

    // -- changing the list --

    /// Keep the list as it is now, for undo.
    fn remember(&self) {
        self.undo.borrow_mut().push(self.sheets.borrow().clone());
        self.redo.borrow_mut().clear();
    }

    fn step_undo(self: &Rc<Self>) {
        let Some(before) = self.undo.borrow_mut().pop() else { return };
        let now = self.sheets.replace(before);
        self.redo.borrow_mut().push(now);
        self.changed.set(true);
        self.refresh();
    }

    fn step_redo(self: &Rc<Self>) {
        let Some(after) = self.redo.borrow_mut().pop() else { return };
        let now = self.sheets.replace(after);
        self.undo.borrow_mut().push(now);
        self.changed.set(true);
        self.refresh();
    }

    fn selected(&self) -> Vec<u64> {
        let mut ids: Vec<u64> = self.picked.borrow().iter().copied().collect();
        let order: HashMap<u64, usize> = self.sheets.borrow().iter().enumerate().map(|(i, s)| (s.id, i)).collect();
        ids.sort_by_key(|id| order.get(id).copied().unwrap_or(usize::MAX));
        ids
    }

    /// Pick out pages, and show which.
    fn pick(&self, ids: impl IntoIterator<Item = u64>, keep: bool) {
        {
            let mut picked = self.picked.borrow_mut();
            if !keep {
                picked.clear();
            }
            picked.extend(ids);
        }
        self.show_picked();
    }

    fn unpick_all(&self) {
        self.picked.borrow_mut().clear();
        self.show_picked();
    }

    fn show_picked(&self) {
        let picked = self.picked.borrow();
        for (id, tile) in self.tiles.borrow().iter() {
            if picked.contains(id) {
                tile.child.add_css_class("picked");
            } else {
                tile.child.remove_css_class("picked");
            }
        }
    }

    /// A click on a page: alone it picks just that page, with Ctrl it adds or
    /// takes it away, and with Shift it picks the run from the last one.
    fn clicked(&self, id: u64, state: gdk::ModifierType) {
        if state.contains(gdk::ModifierType::SHIFT_MASK) {
            let sheets = self.sheets.borrow();
            let at = |id| sheets.iter().position(|s| s.id == id);
            if let (Some(from), Some(to)) = (self.anchor.get().and_then(at), at(id)) {
                let (a, b) = (from.min(to), from.max(to));
                let run: Vec<u64> = sheets[a..=b].iter().map(|s| s.id).collect();
                drop(sheets);
                self.pick(run, state.contains(gdk::ModifierType::CONTROL_MASK));
                return;
            }
        }
        if state.contains(gdk::ModifierType::CONTROL_MASK) {
            let had = self.picked.borrow_mut().remove(&id);
            if !had {
                self.picked.borrow_mut().insert(id);
            }
            self.show_picked();
        } else {
            self.pick([id], false);
        }
        self.anchor.set(Some(id));
    }

    fn turn(self: &Rc<Self>, ids: &[u64], by: i8) {
        if ids.is_empty() {
            return;
        }
        self.remember();
        for sheet in self.sheets.borrow_mut().iter_mut().filter(|s| ids.contains(&s.id)) {
            sheet.turn = (i16::from(sheet.turn) + i16::from(by)).rem_euclid(4) as u8;
        }
        self.changed.set(true);
        self.refresh();
    }

    fn remove(self: &Rc<Self>, ids: &[u64]) {
        if ids.is_empty() {
            return;
        }
        self.remember();
        self.sheets.borrow_mut().retain(|s| !ids.contains(&s.id));
        self.changed.set(true);
        self.refresh();
        let n = ids.len();
        let toast = adw::Toast::builder()
            .title(if n == 1 { "Took out a page.".to_string() } else { format!("Took out {n} pages.") })
            .button_label("_Undo")
            .build();
        let weak = Rc::downgrade(self);
        toast.connect_button_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.step_undo();
            }
        });
        self.toasts.add_toast(toast);
    }

    /// Move these pages, keeping their order, to stand before the page now
    /// at `to`.
    fn move_to(self: &Rc<Self>, ids: &[u64], to: usize) {
        if ids.is_empty() {
            return;
        }
        let moved = moved(&self.sheets.borrow(), ids, to);
        if *self.sheets.borrow() == moved {
            return;
        }
        self.remember();
        self.sheets.replace(moved);
        self.changed.set(true);
        self.refresh();
    }

    // -- showing --

    /// Bring the grid, the header and everything else in line with the list.
    fn refresh(self: &Rc<Self>) {
        let sheets = self.sheets.borrow().clone();
        let mut tiles = self.tiles.borrow_mut();
        let keep: HashSet<u64> = sheets.iter().map(|s| s.id).collect();
        tiles.retain(|id, _| keep.contains(id));
        self.flow.remove_all();
        self.by_child.borrow_mut().clear();
        let sources = self.sources.borrow();
        for (i, sheet) in sheets.iter().enumerate() {
            let tile = tiles.entry(sheet.id).or_insert_with(|| self.new_tile(sheet.id));
            let source = &sources[sheet.source];
            let (w, h) = source.shapes.get(sheet.page).copied().unwrap_or((1.0, 1.0));
            tile.picture.set_shape(w, h);
            tile.picture.set_turn(sheet.turn);
            tile.picture.set_stripe(Some(source.colour));
            tile.picture.set_texture(source.thumbs.borrow().get(&sheet.page).map(|(_, t)| t.clone()));
            tile.number.set_text(&(i + 1).to_string());
            let caption = match source.kind {
                Kind::Picture => source.name.clone(),
                Kind::Pdf { .. } => format!("{} · p. {}", source.name, sheet.page + 1),
            };
            tile.caption.set_text(&caption);
            tile.child.set_tooltip_text(Some(&caption));
            self.size_tile(tile);
            self.flow.append(&tile.child);
            self.by_child.borrow_mut().insert(tile.child.clone(), sheet.id);
        }
        drop(tiles);
        // Pages taken out are no longer picked.
        let keep_ids: HashSet<u64> = sheets.iter().map(|s| s.id).collect();
        self.picked.borrow_mut().retain(|id| keep_ids.contains(id));
        self.show_picked();

        let pages = sheets.len();
        let used: Vec<usize> = {
            let mut used: Vec<usize> = sheets.iter().map(|s| s.source).collect();
            used.sort_unstable();
            used.dedup();
            used
        };
        self.title.set_subtitle(&match (pages, used.len()) {
            (0, _) => String::new(),
            (1, _) => "1 page".to_string(),
            (p, 1) => format!("{p} pages from 1 file"),
            (p, f) => format!("{p} pages from {f} files"),
        });
        // The legend: each file in use, its colour and its pages.
        while let Some(child) = self.legend.first_child() {
            self.legend.remove(&child);
        }
        for index in used {
            let source = &sources[index];
            let swatch = gtk::DrawingArea::builder().content_width(10).content_height(10).valign(gtk::Align::Center).build();
            let colour = source.colour;
            swatch.set_draw_func(move |_, cr, w, h| {
                cr.set_source_rgba(f64::from(colour.red()), f64::from(colour.green()), f64::from(colour.blue()), 1.0);
                cr.rectangle(0.0, 0.0, f64::from(w), f64::from(h));
                let _ = cr.fill();
            });
            let count = sheets.iter().filter(|s| s.source == index).count();
            let label = gtk::Label::new(Some(&match source.kind {
                Kind::Picture => source.name.clone(),
                Kind::Pdf { .. } => format!("{} · {count} {}", source.name, if count == 1 { "page" } else { "pages" }),
            }));
            label.add_css_class("caption");
            let entry = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            entry.append(&swatch);
            entry.append(&label);
            self.legend.append(&entry);
        }
        drop(sources);

        let showing = self.stack.visible_child_name();
        if showing.as_deref() != Some("preview") {
            self.stack.set_visible_child_name(if pages == 0 { "empty" } else { "grid" });
        }
        self.save.set_sensitive(pages > 0);
        self.undo_button.set_sensitive(!self.undo.borrow().is_empty());
        self.redo_button.set_sensitive(!self.redo.borrow().is_empty());
        self.want_soon();
    }

    fn new_tile(self: &Rc<Self>, id: u64) -> Tile {
        let picture = PageTile::new();
        let tool = |icon: &str, tip: &str| {
            let button = gtk::Button::builder().icon_name(icon).tooltip_text(tip).build();
            button.add_css_class("circular");
            button.add_css_class("osd");
            button
        };
        let left = tool("object-rotate-left-symbolic", "Turn left ([)");
        let right = tool("object-rotate-right-symbolic", "Turn right (])");
        let delete = tool("user-trash-symbolic", "Take out (Delete)");
        let tools = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        tools.set_halign(gtk::Align::End);
        tools.set_valign(gtk::Align::Start);
        tools.set_margin_top(4);
        tools.set_margin_end(4);
        tools.append(&left);
        tools.append(&right);
        tools.append(&delete);
        tools.set_visible(false);
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&picture));
        overlay.add_overlay(&tools);
        let number = gtk::Label::new(None);
        number.add_css_class("heading");
        number.add_css_class("numeric");
        let caption = gtk::Label::builder().ellipsize(gtk::pango::EllipsizeMode::Middle).max_width_chars(1).build();
        caption.add_css_class("caption");
        caption.add_css_class("dim-label");
        let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
        column.append(&overlay);
        column.append(&number);
        column.append(&caption);
        let child = gtk::FlowBoxChild::new();
        child.set_child(Some(&column));
        child.add_css_class("combine-page");

        // The buttons show while the pointer is on the page.
        let hover = gtk::EventControllerMotion::new();
        let shown = tools.clone();
        hover.connect_enter(move |_, _, _| shown.set_visible(true));
        let hidden = tools.clone();
        hover.connect_leave(move |_| hidden.set_visible(false));
        child.add_controller(hover);
        for (button, action) in [(&left, -1i8), (&right, 1), (&delete, 0)] {
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                let Some(inner) = weak.upgrade() else { return };
                if action == 0 {
                    inner.remove(&[id]);
                } else {
                    inner.turn(&[id], action);
                }
            });
        }

        // Clicking picks pages out.
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_PRIMARY);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |gesture, presses, _, _| {
            let Some(inner) = weak.upgrade() else { return };
            let state = gesture.current_event_state();
            let plain = !state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK);
            // A plain press on a page already picked keeps the others, so
            // they can all be dragged together; letting go picks just it.
            if presses == 1 && !(plain && inner.picked.borrow().contains(&id)) {
                inner.clicked(id, state);
            }
        });
        let weak = Rc::downgrade(self);
        click.connect_released(move |gesture, presses, _, _| {
            let Some(inner) = weak.upgrade() else { return };
            let state = gesture.current_event_state();
            let plain = !state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK);
            if presses == 1 && plain {
                inner.pick([id], false);
                inner.anchor.set(Some(id));
            }
        });
        child.add_controller(click);

        // Dragging a page: it, or everything picked if it is picked.
        let drag = gtk::DragSource::new();
        drag.set_actions(gdk::DragAction::MOVE);
        let weak = Rc::downgrade(self);
        drag.connect_prepare(move |_, _, _| {
            let inner = weak.upgrade()?;
            if !inner.picked.borrow().contains(&id) {
                inner.pick([id], false);
                inner.anchor.set(Some(id));
            }
            Some(gdk::ContentProvider::for_value(&PAGE_DRAG.to_value()))
        });
        let face = picture.clone();
        drag.connect_drag_begin(move |source, _| {
            source.set_icon(Some(&gtk::WidgetPaintable::new(Some(&face))), 0, 0);
        });
        child.add_controller(drag);
        Tile { child, picture, number, caption }
    }

    fn size_tile(&self, tile: &Tile) {
        let size = self.size.get() as i32;
        tile.picture.set_size_request(size, size);
        tile.caption.set_width_request(size);
    }

    /// Draw what is on screen, a little after things stop moving.
    fn want_soon(self: &Rc<Self>) {
        if self.asked.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(std::time::Duration::from_millis(60), move || {
            if let Some(inner) = weak.upgrade() {
                inner.asked.set(false);
                inner.want_visible();
            }
        });
    }

    fn want_visible(&self) {
        let viewport = self.scroller.vadjustment();
        let (top, bottom) = (viewport.value() - 200.0, viewport.value() + viewport.page_size() + 400.0);
        let screen = self.root.native().and_then(|n| n.surface()).map_or(1.0, |s| s.scale()).max(1.0);
        let longest_px = self.size.get() * screen;
        let mut wanted: HashMap<usize, Vec<(usize, f64)>> = HashMap::new();
        let sheets = self.sheets.borrow();
        let tiles = self.tiles.borrow();
        let sources = self.sources.borrow();
        for sheet in sheets.iter() {
            let Some(tile) = tiles.get(&sheet.id) else { continue };
            let Some(bounds) = tile.child.compute_bounds(&self.flow) else { continue };
            let y = f64::from(bounds.y());
            if y + f64::from(bounds.height()) < top || y > bottom {
                continue;
            }
            let source = &sources[sheet.source];
            let (w, h) = source.shapes.get(sheet.page).copied().unwrap_or((1.0, 1.0));
            let scale = longest_px / w.max(h).max(1.0);
            let have = source.thumbs.borrow().get(&sheet.page).map_or(0.0, |(s, _)| *s);
            if have < scale * 0.95 {
                let list = wanted.entry(sheet.source).or_default();
                if !list.iter().any(|(p, _)| *p == sheet.page) {
                    list.push((sheet.page, scale));
                }
            }
        }
        for (index, pages) in wanted {
            if let Kind::Pdf { images, .. } = &sources[index].kind {
                images.want(&pages);
            }
        }
    }

    /// A page of a PDF has been drawn.
    fn drawn(&self, source: usize, page: usize, scale: f64, texture: gdk::Texture) {
        let Some(found) = self.sources.borrow().get(source).cloned() else { return };
        let better = found.thumbs.borrow().get(&page).is_none_or(|(s, _)| *s < scale);
        if better {
            found.thumbs.borrow_mut().insert(page, (scale, texture.clone()));
        }
        let tiles = self.tiles.borrow();
        for sheet in self.sheets.borrow().iter().filter(|s| s.source == source && s.page == page) {
            if let (true, Some(tile)) = (better, tiles.get(&sheet.id)) {
                tile.picture.set_texture(Some(texture.clone()));
            }
        }
        if let Some(at) = self.previewing.get() {
            if let Some(sheet) = self.sheets.borrow().get(at) {
                if sheet.source == source && sheet.page == page {
                    self.preview.set_texture(Some(texture));
                }
            }
        }
    }

    // -- one page across the window --

    fn open_preview(self: &Rc<Self>, at: usize) {
        if at >= self.sheets.borrow().len() {
            return;
        }
        self.previewing.set(Some(at));
        self.stack.set_visible_child_name("preview");
        self.show_preview();
    }

    fn step_preview(self: &Rc<Self>, by: i32) {
        let Some(at) = self.previewing.get() else { return };
        let count = self.sheets.borrow().len();
        let to = (at as i64 + i64::from(by)).clamp(0, count as i64 - 1) as usize;
        if to != at {
            self.previewing.set(Some(to));
            self.show_preview();
        }
    }

    fn show_preview(self: &Rc<Self>) {
        let Some(at) = self.previewing.get() else { return };
        let Some(sheet) = self.sheets.borrow().get(at).copied() else { return };
        let source = self.sources.borrow()[sheet.source].clone();
        let (w, h) = source.shapes.get(sheet.page).copied().unwrap_or((1.0, 1.0));
        self.preview.set_shape(w, h);
        self.preview.set_turn(sheet.turn);
        self.preview.set_stripe(None);
        self.preview.set_texture(source.thumbs.borrow().get(&sheet.page).map(|(_, t)| t.clone()));
        let count = self.sheets.borrow().len();
        let from = match source.kind {
            Kind::Picture => source.name.clone(),
            Kind::Pdf { .. } => format!("{}, page {}", source.name, sheet.page + 1),
        };
        self.preview_title.set_text(&format!("Page {} of {count} · {from}", at + 1));
        // Sharp at the size it is shown.
        let screen = self.root.native().and_then(|n| n.surface()).map_or(1.0, |s| s.scale()).max(1.0);
        let longest = f64::from(self.root.width().max(self.root.height()).max(800)) * screen;
        match &source.kind {
            Kind::Pdf { images, .. } => images.want(&[(sheet.page, longest / w.max(h).max(1.0))]),
            Kind::Picture => {
                let path = source.path.clone();
                let weak = Rc::downgrade(self);
                let (sender, receiver) = async_channel::bounded(1);
                std::thread::spawn(move || {
                    let _ = sender.send_blocking(full_picture(&path, longest as u32));
                });
                glib::spawn_future_local(async move {
                    let (Ok(Some(texture)), Some(inner)) = (receiver.recv().await, weak.upgrade()) else { return };
                    if inner.previewing.get() == Some(at) {
                        inner.preview.set_texture(Some(texture));
                    }
                });
            }
        }
    }

    fn close_preview(self: &Rc<Self>) {
        let Some(at) = self.previewing.take() else { return };
        self.stack.set_visible_child_name("grid");
        // Back where it was, with that page picked out.
        let id = self.sheets.borrow().get(at).map(|s| s.id);
        if let Some(id) = id {
            self.pick([id], false);
            self.anchor.set(Some(id));
            if let Some(child) = self.tiles.borrow().get(&id).map(|t| t.child.clone()) {
                child.grab_focus();
            }
        }
    }

    // -- drag and drop, keys --

    fn connect_drops(this: &Rc<Self>) {
        let target = gtk::DropTarget::new(glib::Type::INVALID, gdk::DragAction::COPY | gdk::DragAction::MOVE);
        target.set_types(&[glib::Type::STRING, gdk::FileList::static_type(), gio::File::static_type()]);
        target.connect_accept(|target, drop| {
            let offered = drop.formats();
            target.types().iter().any(|t| offered.types().contains(t))
        });
        let weak = Rc::downgrade(this);
        target.connect_motion(move |target, x, y| {
            let Some(inner) = weak.upgrade() else { return gdk::DragAction::empty() };
            inner.mark_drop(Some(inner.drop_index(x, y)));
            let ours = target.current_drop().is_some_and(|drop| drop.drag().is_some());
            if ours { gdk::DragAction::MOVE } else { gdk::DragAction::COPY }
        });
        let weak = Rc::downgrade(this);
        target.connect_leave(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.mark_drop(None);
            }
        });
        let weak = Rc::downgrade(this);
        target.connect_drop(move |_, value, x, y| {
            let Some(inner) = weak.upgrade() else { return false };
            inner.mark_drop(None);
            let at = inner.drop_index(x, y);
            if value.get::<String>().is_ok_and(|s| s == PAGE_DRAG) {
                let ids = inner.selected();
                inner.move_to(&ids, at);
                return true;
            }
            let files: Vec<PathBuf> = match value.get::<gdk::FileList>() {
                Ok(list) => list.files().iter().filter_map(|f| f.path()).collect(),
                Err(_) => value.get::<gio::File>().ok().and_then(|f| f.path()).into_iter().collect(),
            };
            if files.is_empty() {
                return false;
            }
            inner.add(files.into_iter().map(|p| (p, None)).collect(), Some(at));
            true
        });
        // On the whole page area, so dropping on the empty page works too.
        this.toasts.add_controller(target);
    }

    /// Where a drop at x, y in the page area puts things: before the page
    /// whose left half it is on, or after the one whose right half.
    fn drop_index(&self, x: f64, y: f64) -> usize {
        let count = self.sheets.borrow().len();
        let Some(point) = self.toasts.compute_point(&self.flow, &gtk::graphene::Point::new(x as f32, y as f32)) else {
            return count;
        };
        let (px, py) = (f64::from(point.x()), f64::from(point.y()));
        let mut best: Option<(f64, usize)> = None;
        for i in 0..count {
            let Some(child) = self.flow.child_at_index(i as i32) else { continue };
            let Some(b) = child.compute_bounds(&self.flow) else { continue };
            let (cx, cy) = (f64::from(b.x() + b.width() / 2.0), f64::from(b.y() + b.height() / 2.0));
            // Rows count for more than columns: the drop is on this row.
            let distance = (px - cx).abs() + (py - cy).abs() * 4.0;
            let index = if px > cx { i + 1 } else { i };
            if best.is_none_or(|(d, _)| distance < d) {
                best = Some((distance, index));
            }
        }
        best.map_or(count, |(_, index)| index)
    }

    /// Show where a drop would land: a bar at the side of a page.
    fn mark_drop(&self, at: Option<usize>) {
        let count = self.sheets.borrow().len();
        for i in 0..count {
            if let Some(child) = self.flow.child_at_index(i as i32) {
                child.remove_css_class("drop-before");
                child.remove_css_class("drop-after");
            }
        }
        let Some(at) = at else { return };
        if at < count {
            if let Some(child) = self.flow.child_at_index(at as i32) {
                child.add_css_class("drop-before");
            }
        } else if let Some(child) = count.checked_sub(1).and_then(|last| self.flow.child_at_index(last as i32)) {
            child.add_css_class("drop-after");
        }
    }

    fn connect_keys(this: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(this);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(inner) = weak.upgrade() else { return glib::Propagation::Proceed };
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            let previewing = inner.previewing.get().is_some();
            use gdk::Key;
            let handled = match key {
                Key::Escape if previewing => {
                    inner.close_preview();
                    true
                }
                Key::Escape => {
                    inner.unpick_all();
                    true
                }
                Key::a if ctrl => {
                    let all: Vec<u64> = inner.sheets.borrow().iter().map(|s| s.id).collect();
                    inner.pick(all, false);
                    true
                }
                Key::Left | Key::Up | Key::Page_Up if previewing => {
                    inner.step_preview(-1);
                    true
                }
                Key::Right | Key::Down | Key::Page_Down | Key::space if previewing => {
                    inner.step_preview(1);
                    true
                }
                Key::BackSpace if previewing => {
                    inner.close_preview();
                    true
                }
                Key::z | Key::Z if ctrl && shift => {
                    inner.step_redo();
                    true
                }
                Key::z if ctrl => {
                    inner.step_undo();
                    true
                }
                Key::y if ctrl => {
                    inner.step_redo();
                    true
                }
                Key::o if ctrl => {
                    inner.choose_files();
                    true
                }
                Key::s if ctrl => {
                    inner.ask_to_save();
                    true
                }
                _ if previewing => false,
                Key::Delete | Key::KP_Delete | Key::BackSpace => {
                    let ids = inner.selected();
                    inner.remove(&ids);
                    !ids.is_empty()
                }
                Key::bracketleft => {
                    let ids = inner.selected();
                    inner.turn(&ids, -1);
                    true
                }
                Key::bracketright => {
                    let ids = inner.selected();
                    inner.turn(&ids, 1);
                    true
                }
                // Ctrl with an arrow moves what is selected one place along.
                Key::Left | Key::Right if ctrl => {
                    let ids = inner.selected();
                    let sheets = inner.sheets.borrow().clone();
                    let first = sheets.iter().position(|s| ids.contains(&s.id));
                    let last = sheets.iter().rposition(|s| ids.contains(&s.id));
                    if let (Some(first), Some(last)) = (first, last) {
                        let to = if key == Key::Left { first.saturating_sub(1) } else { (last + 2).min(sheets.len()) };
                        inner.move_to(&ids, to);
                    }
                    true
                }
                _ => false,
            };
            if handled { glib::Propagation::Stop } else { glib::Propagation::Proceed }
        });
        this.root.add_controller(keys);
    }

    // -- leaving and saving --

    fn ask_to_leave(self: &Rc<Self>) {
        if !self.changed.get() || self.sheets.borrow().is_empty() {
            self.close(None);
            return;
        }
        let dialog = adw::AlertDialog::new(
            Some("Discard These Pages?"),
            Some("The pages put together here have not been saved as a PDF yet. The files they came from are not changed either way."),
        );
        dialog.add_response("cancel", "_Keep Working");
        dialog.add_response("discard", "_Discard");
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        dialog.connect_response(Some("discard"), move |_, _| {
            if let Some(inner) = weak.upgrade() {
                inner.close(None);
            }
        });
        dialog.present(Some(&self.root));
    }

    /// Ask the paper for pictures, if there are any, then where to save.
    fn ask_to_save(self: &Rc<Self>) {
        if self.sheets.borrow().is_empty() {
            return;
        }
        let pictures = {
            let sources = self.sources.borrow();
            self.sheets.borrow().iter().any(|s| matches!(sources[s.source].kind, Kind::Picture))
        };
        if !pictures {
            self.choose_destination(pdf::Paper::A4);
            return;
        }
        let weak = Rc::downgrade(self);
        ask_paper(&self.root, self.paper.get(), move |paper| {
            let Some(inner) = weak.upgrade() else { return };
            inner.paper.set(Some(paper));
            if let Some(remember) = inner.on_paper.borrow().as_ref() {
                remember(paper);
            }
            inner.choose_destination(paper);
        });
    }

    fn choose_destination(self: &Rc<Self>, paper: pdf::Paper) {
        let folder = {
            let sources = self.sources.borrow();
            self.sheets.borrow().first().and_then(|s| sources[s.source].path.parent().map(Path::to_path_buf))
        };
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("PDF documents"));
        filter.add_mime_type("application/pdf");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let chooser = gtk::FileDialog::builder()
            .title("Save as PDF")
            .initial_name("Combined.pdf")
            .filters(&filters)
            .modal(true)
            .build();
        if let Some(folder) = folder {
            chooser.set_initial_folder(Some(&gio::File::for_path(folder)));
        }
        let window = self.root.root().and_downcast::<gtk::Window>();
        let weak = Rc::downgrade(self);
        chooser.save(window.as_ref(), gio::Cancellable::NONE, move |result| {
            let (Some(inner), Some(path)) = (weak.upgrade(), result.ok().and_then(|f| f.path())) else { return };
            inner.write(path, paper);
        });
    }

    fn write(self: &Rc<Self>, destination: PathBuf, paper: pdf::Paper) {
        let leaves: Vec<pdf::Leaf> = {
            let sources = self.sources.borrow();
            self.sheets
                .borrow()
                .iter()
                .map(|sheet| {
                    let source = &sources[sheet.source];
                    let origin = match &source.kind {
                        Kind::Pdf { password, .. } => {
                            pdf::Origin::Pdf { path: source.path.clone(), password: password.clone(), page: sheet.page }
                        }
                        Kind::Picture => pdf::Origin::Picture { path: source.path.clone() },
                    };
                    pdf::Leaf { origin, turn: sheet.turn }
                })
                .collect()
        };
        let working = adw::Toast::builder().title("Saving…").timeout(0).build();
        self.toasts.add_toast(working.clone());
        self.save.set_sensitive(false);
        let temporary = pdf::temporary_beside(&destination, "combine");
        let (sender, receiver) = async_channel::bounded(1);
        let (out, keep) = (temporary.clone(), destination.clone());
        std::thread::spawn(move || {
            let result = pdf::combine(&leaves, paper, &out).and_then(|_| pdf::keep_as(&out, &keep));
            if result.is_err() {
                let _ = std::fs::remove_file(&out);
            }
            let _ = sender.send_blocking(result);
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = receiver.recv().await.unwrap_or_else(|_| Err("stopped unexpectedly".into()));
            let Some(inner) = weak.upgrade() else { return };
            working.dismiss();
            inner.save.set_sensitive(true);
            match result {
                Ok(()) => {
                    inner.changed.set(false);
                    let toast = adw::Toast::builder()
                        .title(format!("Saved “{}”.", file_name(&destination)))
                        .button_label("_Open")
                        .timeout(10)
                        .build();
                    let weak = Rc::downgrade(&inner);
                    toast.connect_button_clicked(move |_| {
                        if let Some(inner) = weak.upgrade() {
                            inner.close(Some(destination.clone()));
                        }
                    });
                    inner.toasts.add_toast(toast);
                }
                Err(error) => inner.toast(&format!("Could not save the PDF: {error}")),
            }
        });
    }
}

/// The list with the pages `ids` lifted out, in their order, and put down
/// before the page that was at `to` — or at the end, past the last.
fn moved(sheets: &[Sheet], ids: &[u64], to: usize) -> Vec<Sheet> {
    let to = to.min(sheets.len());
    // Where `to` lands once the moving pages are lifted out.
    let before = sheets[..to].iter().filter(|s| ids.contains(&s.id)).count();
    let mut rest: Vec<Sheet> = sheets.iter().filter(|s| !ids.contains(&s.id)).copied().collect();
    let lifted: Vec<Sheet> = sheets.iter().filter(|s| ids.contains(&s.id)).copied().collect();
    let at = to - before;
    rest.splice(at..at, lifted);
    rest
}

enum Found {
    Pdf(Result<pdf::Opened, pdf::OpenError>),
    /// A picture's width and height in pixels, and a small picture of it.
    Picture(Result<(u32, u32, gdk::Texture), String>),
}

/// A picture's size, and a thumbnail of it. Runs on a worker thread; the
/// texture is made there too, which GDK allows.
fn picture(path: &Path) -> Result<(u32, u32, gdk::Texture), String> {
    let decoded = crate::images::loader::decode(path)?;
    let (width, height) = (decoded.width, decoded.height);
    let texture = texture_of(decoded, PICTURE_THUMB).ok_or("the picture could not be read")?;
    Ok((width, height, texture))
}

/// The picture again, at most `longest` pixels along its longer side.
fn full_picture(path: &Path, longest: u32) -> Option<gdk::Texture> {
    texture_of(crate::images::loader::decode(path).ok()?, longest.max(256))
}

fn texture_of(decoded: crate::images::loader::LoadedImage, longest: u32) -> Option<gdk::Texture> {
    let premultiplied = decoded.premultiplied;
    let image = image::RgbaImage::from_raw(decoded.width, decoded.height, decoded.rgba)?;
    let image = if image.width().max(image.height()) > longest {
        image::DynamicImage::ImageRgba8(image).thumbnail(longest, longest).to_rgba8()
    } else {
        image
    };
    let (w, h) = (image.width(), image.height());
    Some(crate::images::canvas::texture_from(w, h, premultiplied, image.into_raw()))
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Which pages of a PDF to take: all of them, or some. `done` gets the pages,
/// counting from zero, or `None` to skip the file.
fn ask_pages(parent: &impl IsA<gtk::Widget>, name: &str, count: usize, done: impl Fn(Option<Vec<usize>>) + 'static) {
    let all = gtk::CheckButton::with_label(&format!("All {count} pages"));
    let some = gtk::CheckButton::with_label("Pages:");
    some.set_group(Some(&all));
    all.set_active(true);
    // An example that makes sense for this many pages.
    let example = match count {
        0..=2 => count.to_string(),
        3..=7 => format!("1, 3-{count}"),
        _ => format!("1-3, 5, 8-{count}"),
    };
    let entry = gtk::Entry::builder().placeholder_text(example).hexpand(true).activates_default(true).build();
    entry.set_sensitive(false);
    let problem = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).build();
    problem.add_css_class("error");
    problem.add_css_class("caption");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&some);
    row.append(&entry);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
    column.append(&all);
    column.append(&row);
    column.append(&problem);
    let dialog = adw::AlertDialog::builder()
        .heading(format!("Pages from “{name}”"))
        .body("Take every page, or only some. Pages can also be taken out afterwards.")
        .extra_child(&column)
        .build();
    dialog.add_responses(&[("skip", "_Skip This File"), ("add", "_Add")]);
    dialog.set_response_appearance("add", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("add"));
    dialog.set_close_response("skip");
    let check = {
        let (entry, some, problem, dialog) = (entry.clone(), some.clone(), problem.clone(), dialog.downgrade());
        move || {
            let result = if some.is_active() { pdf::parse_pages(&entry.text(), count) } else { Ok((0..count).collect()) };
            let wrong = match (&result, some.is_active() && entry.text().trim().is_empty()) {
                (_, true) => Some("Type the pages to take.".to_string()),
                (Err(e), _) => Some(e.clone()),
                _ => None,
            };
            problem.set_text(wrong.as_deref().unwrap_or(""));
            problem.set_visible(wrong.is_some() && !entry.text().is_empty());
            if let Some(dialog) = dialog.upgrade() {
                dialog.set_response_enabled("add", wrong.is_none());
            }
            result.ok().filter(|_| wrong.is_none())
        }
    };
    let check = Rc::new(check);
    {
        let (check, entry) = (check.clone(), entry.clone());
        some.connect_toggled(move |some| {
            entry.set_sensitive(some.is_active());
            if some.is_active() {
                entry.grab_focus();
            }
            check();
        });
    }
    {
        let check = check.clone();
        entry.connect_changed(move |_| {
            check();
        });
    }
    let done = Rc::new(done);
    dialog.connect_response(None, move |_, response| {
        done(if response == "add" { check() } else { None });
    });
    dialog.present(Some(parent));
}

/// The password for a protected PDF, or `None` to skip it.
fn ask_password(parent: &impl IsA<gtk::Widget>, name: &str, tried: bool, done: impl Fn(Option<String>) + 'static) {
    let entry = gtk::PasswordEntry::builder().show_peek_icon(true).activates_default(true).build();
    let dialog = adw::AlertDialog::builder()
        .heading(format!("“{name}” Is Locked"))
        .body(if tried { "That password did not open it. Try again, or skip it." } else { "Enter its password to add its pages." })
        .extra_child(&entry)
        .build();
    dialog.add_responses(&[("skip", "_Skip This File"), ("unlock", "_Unlock")]);
    dialog.set_response_appearance("unlock", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("unlock"));
    dialog.set_close_response("skip");
    let done = Rc::new(done);
    let typed = entry.clone();
    dialog.connect_response(None, move |_, response| {
        let password = typed.text().to_string();
        done((response == "unlock" && !password.is_empty()).then_some(password));
    });
    dialog.present(Some(parent));
    entry.grab_focus();
}

/// The paper pictures go on, asked every time with the last answer ready.
/// Nothing is chosen for the reader the first time.
fn ask_paper(parent: &impl IsA<gtk::Widget>, last: Option<pdf::Paper>, done: impl Fn(pdf::Paper) + 'static) {
    let choices = [
        (pdf::Paper::A4, "A4", "Each picture fitted to a sheet of A4, turned to suit it"),
        (pdf::Paper::Letter, "US Letter", "Each picture fitted to a sheet of Letter, turned to suit it"),
        (pdf::Paper::Own, "The Picture's Own Size", "Each page exactly the shape of its picture"),
    ];
    let group = adw::PreferencesGroup::new();
    let mut first: Option<gtk::CheckButton> = None;
    let mut buttons = Vec::new();
    for (paper, title, subtitle) in choices {
        let check = gtk::CheckButton::new();
        match &first {
            Some(first) => check.set_group(Some(first)),
            None => first = Some(check.clone()),
        }
        check.set_active(last == Some(paper));
        let row = adw::ActionRow::builder().title(title).subtitle(subtitle).activatable_widget(&check).build();
        row.add_prefix(&check);
        group.add(&row);
        buttons.push((paper, check));
    }
    let dialog = adw::AlertDialog::builder()
        .heading("Paper for Pictures")
        .body("Each picture becomes a page of its own. What should it be on?")
        .extra_child(&group)
        .build();
    dialog.add_responses(&[("cancel", "_Cancel"), ("continue", "_Continue")]);
    dialog.set_response_appearance("continue", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("continue"));
    dialog.set_close_response("cancel");
    // Until something is chosen there is nothing to continue with.
    dialog.set_response_enabled("continue", last.is_some());
    for (_, check) in &buttons {
        let dialog = dialog.downgrade();
        check.connect_toggled(move |_| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.set_response_enabled("continue", true);
            }
        });
    }
    dialog.connect_response(Some("continue"), move |_, _| {
        if let Some((paper, _)) = buttons.iter().find(|(_, c)| c.is_active()) {
            done(*paper);
        }
    });
    dialog.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(n: u64) -> Vec<Sheet> {
        (1..=n).map(|id| Sheet { id, source: 0, page: id as usize - 1, turn: 0 }).collect()
    }

    fn order(sheets: &[Sheet]) -> Vec<u64> {
        sheets.iter().map(|s| s.id).collect()
    }

    #[test]
    fn pages_move_to_where_they_are_dropped() {
        let pages = list(6);
        // One page to the front, and one to the end.
        assert_eq!(order(&moved(&pages, &[5], 0)), vec![5, 1, 2, 3, 4, 6]);
        assert_eq!(order(&moved(&pages, &[2], 6)), vec![1, 3, 4, 5, 6, 2]);
        // Forward past others: it lands before the page that was at `to`.
        assert_eq!(order(&moved(&pages, &[1], 4)), vec![2, 3, 4, 1, 5, 6]);
        // Several, picked apart, go together in their own order.
        assert_eq!(order(&moved(&pages, &[2, 5], 0)), vec![2, 5, 1, 3, 4, 6]);
        assert_eq!(order(&moved(&pages, &[1, 4], 6)), vec![2, 3, 5, 6, 1, 4]);
        // Dropped onto its own place, nothing changes.
        assert_eq!(order(&moved(&pages, &[3], 2)), order(&pages));
        assert_eq!(order(&moved(&pages, &[3], 3)), order(&pages));
        // Past the end is the end.
        assert_eq!(order(&moved(&pages, &[1], 99)), vec![2, 3, 4, 5, 6, 1]);
    }
}
