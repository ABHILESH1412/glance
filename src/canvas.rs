//! The zoomable, pannable image surface.
//!
//! Zoom is anchored: whatever image pixel sits under the pointer stays under
//! the pointer, which is what makes wheel-zoom feel like moving a magnifier
//! rather than operating a slider.
//!
//! Every change sets a *target*, and a frame-clock callback eases the drawn
//! state towards it. Retargeting mid-flight is therefore free — spinning the
//! wheel five notches just moves the target four more times while the easing
//! keeps up, instead of queueing five animations.

use std::cell::{Cell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

/// Past this, zooming stops being informative.
const MAX_SCALE: f64 = 32.0;
/// Time constant of the exponential ease, in seconds. Smaller is snappier.
const EASE_TAU: f64 = 0.065;
/// Zoom per unit of scroll delta. One mouse-wheel notch is a delta of 1.
const WHEEL_STEP: f64 = 0.28;
/// Beyond this the point is to inspect individual pixels, so stop smoothing.
const NEAREST_ABOVE: f64 = 3.0;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ImageCanvas {
        pub texture: RefCell<Option<gdk::Texture>>,
        /// What is on screen this frame.
        pub scale: Cell<f64>,
        pub offset: Cell<(f64, f64)>,
        /// What it is easing towards.
        pub target_scale: Cell<f64>,
        pub target_offset: Cell<(f64, f64)>,
        /// Where the anchor says the image *wants* to sit, before clamping.
        ///
        /// While the image is smaller than the viewport on an axis it gets
        /// centred, which is right to look at but destroys the anchor. Feeding
        /// the clamped value back into the next zoom step would let that
        /// centring accumulate, and the point under the cursor would crawl away
        /// as you scrolled. So anchor maths reads this, and only the drawn
        /// offset is clamped.
        pub ideal_offset: Cell<(f64, f64)>,
        /// Once the user zooms, resizing the window must not silently re-fit
        /// and throw away where they were looking.
        pub user_zoomed: Cell<bool>,
        pub tick: RefCell<Option<gtk::TickCallbackId>>,
        pub last_frame: Cell<i64>,
        /// Scroll events carry no coordinates, so track the pointer separately.
        pub pointer: Cell<(f64, f64)>,
        pub drag_origin: Cell<(f64, f64)>,
        /// Whether the current button press has actually moved yet. A press
        /// that never moves is a click, and must not disturb the view.
        pub dragging: Cell<bool>,
        pub pinch_origin: Cell<f64>,
        pub on_zoom_changed: RefCell<Option<Box<dyn Fn(f64)>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ImageCanvas {
        const NAME: &'static str = "SvImageCanvas";
        type Type = super::ImageCanvas;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for ImageCanvas {
        fn constructed(&self) {
            self.parent_constructed();
            self.scale.set(1.0);
            self.target_scale.set(1.0);
            let obj = self.obj();
            obj.set_hexpand(true);
            obj.set_vexpand(true);
            obj.set_focusable(true);
            obj.setup_controllers();
        }

        fn dispose(&self) {
            if let Some(tick) = self.tick.take() {
                tick.remove();
            }
        }
    }

    impl WidgetImpl for ImageCanvas {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            self.obj().draw(snapshot);
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            self.obj().reflow();
        }
    }
}

glib::wrapper! {
    pub struct ImageCanvas(ObjectSubclass<imp::ImageCanvas>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for ImageCanvas {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl ImageCanvas {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_texture(&self, texture: Option<gdk::Texture>) {
        let imp = self.imp();
        imp.texture.replace(texture);
        imp.user_zoomed.set(false);
        // A half-finished animation belongs to the previous image.
        if let Some(tick) = imp.tick.take() {
            tick.remove();
        }
        imp.last_frame.set(0);
        self.reflow();
    }

    pub fn has_image(&self) -> bool {
        self.imp().texture.borrow().is_some()
    }

    /// Called with the on-screen scale as a percentage whenever it changes.
    pub fn connect_zoom_changed(&self, f: impl Fn(f64) + 'static) {
        self.imp().on_zoom_changed.replace(Some(Box::new(f)));
    }

    pub fn zoom_by(&self, factor: f64, anchor: Option<(f64, f64)>) {
        let anchor = anchor.unwrap_or_else(|| self.viewport_center());
        self.set_scale_at(self.imp().target_scale.get() * factor, anchor, true);
    }

    /// Scale the image to fit the window, and let it re-fit on resize again.
    pub fn zoom_fit(&self) {
        let imp = self.imp();
        if imp.texture.borrow().is_none() {
            return;
        }
        let scale = self.fit_scale();
        let offset = self.centered_offset(scale);
        imp.target_scale.set(scale);
        imp.target_offset.set(offset);
        imp.ideal_offset.set(offset);
        imp.user_zoomed.set(false);
        self.animate();
    }

    /// One image pixel per screen pixel.
    pub fn zoom_actual(&self) {
        self.set_scale_at(1.0, self.viewport_center(), true);
    }

    pub fn is_fitted(&self) -> bool {
        (self.imp().target_scale.get() - self.fit_scale()).abs() < 0.001
    }

    // -- geometry ---------------------------------------------------------

    fn texture_size(&self) -> Option<(f64, f64)> {
        self.imp()
            .texture
            .borrow()
            .as_ref()
            .map(|t| (t.width() as f64, t.height() as f64))
    }

    /// The scale at which the image just fits. Capped at 1.0 so a small image
    /// sits at its natural size instead of being blown up into mush.
    fn fit_scale(&self) -> f64 {
        let Some((tw, th)) = self.texture_size() else {
            return 1.0;
        };
        let (w, h) = (self.width() as f64, self.height() as f64);
        if w <= 0.0 || h <= 0.0 || tw <= 0.0 || th <= 0.0 {
            return 1.0;
        }
        (w / tw).min(h / th).min(1.0)
    }

    fn viewport_center(&self) -> (f64, f64) {
        (self.width() as f64 / 2.0, self.height() as f64 / 2.0)
    }

    fn centered_offset(&self, scale: f64) -> (f64, f64) {
        let Some((tw, th)) = self.texture_size() else {
            return (0.0, 0.0);
        };
        (
            (self.width() as f64 - tw * scale) / 2.0,
            (self.height() as f64 - th * scale) / 2.0,
        )
    }

    /// Keep the image from being dragged off into empty space: an axis that
    /// overflows the viewport stays glued to its edges, and one that fits is
    /// centred.
    fn clamp_offset(&self, offset: (f64, f64), scale: f64) -> (f64, f64) {
        let Some((tw, th)) = self.texture_size() else {
            return offset;
        };
        let (vw, vh) = (self.width() as f64, self.height() as f64);
        let (iw, ih) = (tw * scale, th * scale);
        let x = if iw <= vw {
            (vw - iw) / 2.0
        } else {
            offset.0.clamp(vw - iw, 0.0)
        };
        let y = if ih <= vh {
            (vh - ih) / 2.0
        } else {
            offset.1.clamp(vh - ih, 0.0)
        };
        (x, y)
    }

    fn is_pannable(&self) -> bool {
        let Some((tw, th)) = self.texture_size() else {
            return false;
        };
        let scale = self.imp().target_scale.get();
        tw * scale > self.width() as f64 + 0.5 || th * scale > self.height() as f64 + 0.5
    }

    fn set_scale_at(&self, scale: f64, anchor: (f64, f64), smooth: bool) {
        let imp = self.imp();
        if imp.texture.borrow().is_none() {
            return;
        }
        let previous = imp.target_scale.get();
        let scale = scale.clamp(self.fit_scale(), MAX_SCALE);
        if (scale - previous).abs() < 1e-9 {
            return;
        }

        // Pin the image point under the anchor so it does not slide away.
        let (ax, ay) = anchor;
        let (ox, oy) = imp.ideal_offset.get();
        let image_x = (ax - ox) / previous;
        let image_y = (ay - oy) / previous;
        let ideal = (ax - image_x * scale, ay - image_y * scale);
        let offset = self.clamp_offset(ideal, scale);

        imp.target_scale.set(scale);
        imp.ideal_offset.set(ideal);
        imp.target_offset.set(offset);
        imp.user_zoomed.set(true);

        if smooth {
            self.animate();
        } else {
            if let Some(tick) = imp.tick.take() {
                tick.remove();
            }
            imp.scale.set(scale);
            imp.offset.set(offset);
            self.settle();
        }
    }

    /// Re-fit or re-clamp after the widget changes size.
    fn reflow(&self) {
        let imp = self.imp();
        if imp.texture.borrow().is_none() {
            return;
        }
        let (scale, offset) = if imp.user_zoomed.get() {
            // A window that grew can leave the image smaller than a fit.
            let scale = imp.target_scale.get().max(self.fit_scale());
            (scale, self.clamp_offset(imp.target_offset.get(), scale))
        } else {
            let scale = self.fit_scale();
            (scale, self.centered_offset(scale))
        };

        // Resizing should not animate; the window is already moving.
        if let Some(tick) = imp.tick.take() {
            tick.remove();
        }
        imp.target_scale.set(scale);
        imp.target_offset.set(offset);
        imp.ideal_offset.set(offset);
        imp.scale.set(scale);
        imp.offset.set(offset);
        self.settle();
    }

    fn settle(&self) {
        self.update_cursor();
        self.notify_zoom();
        self.queue_draw();
    }

    fn update_cursor(&self) {
        let name = if self.is_pannable() { Some("grab") } else { None };
        self.set_cursor_from_name(name);
    }

    fn notify_zoom(&self) {
        if let Some(callback) = self.imp().on_zoom_changed.borrow().as_ref() {
            callback(self.imp().scale.get() * 100.0);
        }
    }

    // -- animation --------------------------------------------------------

    fn animate(&self) {
        let imp = self.imp();
        if imp.tick.borrow().is_some() {
            return; // Already easing; the new target is picked up next frame.
        }
        imp.last_frame.set(0);
        let id = self.add_tick_callback(|canvas, clock| canvas.step(clock));
        imp.tick.replace(Some(id));
    }

    fn step(&self, clock: &gdk::FrameClock) -> glib::ControlFlow {
        let imp = self.imp();
        let now = clock.frame_time();
        let last = imp.last_frame.get();
        imp.last_frame.set(now);
        // Clamped so a stalled frame does not teleport the view.
        let dt = if last == 0 {
            1.0 / 60.0
        } else {
            ((now - last) as f64 / 1_000_000.0).clamp(0.0, 0.1)
        };
        let alpha = 1.0 - (-dt / EASE_TAU).exp();

        let (scale, target_scale) = (imp.scale.get(), imp.target_scale.get());
        let (ox, oy) = imp.offset.get();
        let (tx, ty) = imp.target_offset.get();

        // Interpolate scale geometrically: 1x to 2x should feel like 2x to 4x.
        let next_scale = (scale.ln() + (target_scale.ln() - scale.ln()) * alpha).exp();
        let next = (ox + (tx - ox) * alpha, oy + (ty - oy) * alpha);

        let settled = (next_scale / target_scale - 1.0).abs() < 0.001
            && (tx - next.0).abs() < 0.3
            && (ty - next.1).abs() < 0.3;

        if settled {
            imp.scale.set(target_scale);
            imp.offset.set((tx, ty));
            imp.tick.replace(None);
            self.settle();
            return glib::ControlFlow::Break;
        }

        imp.scale.set(next_scale);
        imp.offset.set(next);
        self.notify_zoom();
        self.queue_draw();
        glib::ControlFlow::Continue
    }

    // -- drawing ----------------------------------------------------------

    fn draw(&self, snapshot: &gtk::Snapshot) {
        let imp = self.imp();
        let Some(texture) = imp.texture.borrow().clone() else {
            return;
        };
        let scale = imp.scale.get();
        let (mut x, mut y) = imp.offset.get();
        let width = texture.width() as f64 * scale;
        let height = texture.height() as f64 * scale;

        // At 1:1 a half-pixel offset would blur a perfectly sharp image.
        if (scale - 1.0).abs() < 0.001 {
            x = x.round();
            y = y.round();
        }

        let filter = if scale >= NEAREST_ABOVE {
            gsk::ScalingFilter::Nearest
        } else if scale < 1.0 {
            // Mipmapped, so downscaled photos do not shimmer.
            gsk::ScalingFilter::Trilinear
        } else {
            gsk::ScalingFilter::Linear
        };

        snapshot.append_scaled_texture(
            &texture,
            filter,
            &graphene::Rect::new(x as f32, y as f32, width as f32, height as f32),
        );
    }
}

// -- input ----------------------------------------------------------------

impl ImageCanvas {
    fn setup_controllers(&self) {
        self.add_controller(self.motion_controller());
        self.add_controller(self.scroll_controller());
        self.add_controller(self.drag_gesture());
        self.add_controller(self.click_gesture());
        self.add_controller(self.pinch_gesture());
    }

    /// Scroll events carry no position, so the pointer is tracked separately
    /// and used as the zoom anchor.
    fn motion_controller(&self) -> gtk::EventControllerMotion {
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            move |_, x, y| canvas.imp().pointer.set((x, y))
        ));
        motion
    }

    fn scroll_controller(&self) -> gtk::EventControllerScroll {
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.connect_scroll(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, _, dy| {
                if !canvas.has_image() {
                    return glib::Propagation::Proceed;
                }
                // Exponential, so a touchpad's fractional deltas and a mouse
                // wheel's whole notches both feel proportional.
                canvas.zoom_by((-dy * WHEEL_STEP).exp(), Some(canvas.imp().pointer.get()));
                glib::Propagation::Stop
            }
        ));
        scroll
    }

    fn drag_gesture(&self) -> gtk::GestureDrag {
        let drag = gtk::GestureDrag::new();

        // A press records where the image is but changes nothing. A double
        // click arrives as a press too, and tearing down the animation it just
        // started would cancel the zoom.
        drag.connect_drag_begin(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            move |_, _, _| {
                let imp = canvas.imp();
                imp.dragging.set(false);
                imp.drag_origin.set(imp.offset.get());
            }
        ));

        drag.connect_drag_update(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            move |_, dx, dy| {
                let imp = canvas.imp();
                if !canvas.is_pannable() {
                    return;
                }
                if !imp.dragging.get() {
                    // GTK also reports a zero-movement update as a click ends.
                    // Treating that as a pan would cancel the animation the
                    // click itself started.
                    if dx.abs() < 1.0 && dy.abs() < 1.0 {
                        return;
                    }
                    imp.dragging.set(true);
                    // `dx` is measured from the press point and already includes
                    // the drag threshold, so the press-time origin gives exact
                    // 1:1 tracking. Only when the view was still easing does the
                    // origin need rebasing, to avoid snapping backwards.
                    if let Some(tick) = imp.tick.take() {
                        tick.remove();
                        let current = imp.offset.get();
                        imp.drag_origin.set((current.0 - dx, current.1 - dy));
                    }
                    imp.target_scale.set(imp.scale.get());
                    canvas.set_cursor_from_name(Some("grabbing"));
                }

                let (ox, oy) = imp.drag_origin.get();
                let offset = canvas.clamp_offset((ox + dx, oy + dy), imp.target_scale.get());
                imp.offset.set(offset);
                imp.target_offset.set(offset);
                // Dragging re-establishes where the image actually is.
                imp.ideal_offset.set(offset);
                imp.user_zoomed.set(true);
                canvas.queue_draw();
            }
        ));

        drag.connect_drag_end(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            move |_, _, _| {
                canvas.imp().dragging.set(false);
                canvas.update_cursor();
            }
        ));

        drag
    }

    /// Double click toggles between fitting the window and 1:1, anchored where
    /// the click landed.
    fn click_gesture(&self) -> gtk::GestureClick {
        let click = gtk::GestureClick::new();
        click.connect_pressed(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            move |_, presses, x, y| {
                if presses != 2 || !canvas.has_image() {
                    return;
                }
                if canvas.is_fitted() {
                    canvas.set_scale_at(1.0, (x, y), true);
                } else {
                    canvas.zoom_fit();
                }
            }
        ));
        click
    }

    fn pinch_gesture(&self) -> gtk::GestureZoom {
        let pinch = gtk::GestureZoom::new();

        pinch.connect_begin(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            move |_, _| {
                let imp = canvas.imp();
                imp.pinch_origin.set(imp.target_scale.get());
            }
        ));

        pinch.connect_scale_changed(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            move |gesture, delta| {
                let Some(centre) = gesture.bounding_box_center() else {
                    return;
                };
                let base = canvas.imp().pinch_origin.get();
                // Pinch should stay glued to the fingers, so no easing here.
                canvas.set_scale_at(base * delta, centre, false);
            }
        ));

        pinch
    }
}
