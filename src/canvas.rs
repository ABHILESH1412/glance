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
use std::time::Duration;

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
        /// Frames of an animated file, with how long each is shown. Empty for a
        /// still image.
        pub frames: RefCell<Vec<(gdk::Texture, Duration)>>,
        pub frame_index: Cell<usize>,
        pub frame_timer: RefCell<Option<glib::SourceId>>,
        /// What is on screen this frame. The image is tracked by its centre
        /// rather than a corner, which is what makes rotation, fitting and
        /// zoom-anchoring all reduce to the same bit of maths.
        pub scale: Cell<f64>,
        pub centre: Cell<(f64, f64)>,
        /// Degrees clockwise, deliberately left unwrapped: animating from 170
        /// to 260 must turn 90 degrees forwards, not 270 backwards.
        pub rotation: Cell<f64>,
        /// What it is easing towards.
        pub target_scale: Cell<f64>,
        pub target_centre: Cell<(f64, f64)>,
        pub target_rotation: Cell<f64>,
        /// Mirroring, applied in the image's own frame so a horizontal flip
        /// always mirrors its left and right, whatever angle it is turned to.
        pub flip_h: Cell<bool>,
        pub flip_v: Cell<bool>,
        /// Once the user zooms, resizing the window must not silently re-fit
        /// and throw away where they were looking.
        pub user_zoomed: Cell<bool>,
        /// Where the anchor says the image *wants* to sit, before clamping.
        ///
        /// While the image is smaller than the viewport it gets centred, which
        /// is right to look at but destroys the anchor. Feeding the clamped
        /// value back into the next zoom step would let that centring
        /// accumulate and the point under the cursor would crawl away.
        pub ideal_centre: Cell<(f64, f64)>,
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
        pub on_rotation_changed: RefCell<Option<Box<dyn Fn(f64)>>>,
        pub on_flip_changed: RefCell<Option<Box<dyn Fn(bool, bool)>>>,
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
            if let Some(timer) = self.frame_timer.take() {
                timer.remove();
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

/// Fold an angle into [-180, 180), which is what the slider and the readout use.
fn normalise(degrees: f64) -> f64 {
    let wrapped = (degrees + 180.0).rem_euclid(360.0) - 180.0;
    // rem_euclid can hand back exactly 180.0 for inputs just under it.
    if wrapped >= 180.0 {
        wrapped - 360.0
    } else {
        wrapped
    }
}

impl ImageCanvas {
    pub fn new() -> Self {
        Self::default()
    }

    /// Play an animated image. The view is reset as for any newly opened file.
    pub fn set_animation(&self, frames: Vec<(gdk::Texture, Duration)>) {
        let Some(first) = frames.first().map(|(texture, _)| texture.clone()) else {
            return;
        };
        // Sets the view up and stops whatever was playing before.
        self.set_texture(Some(first));
        let imp = self.imp();
        imp.frames.replace(frames);
        imp.frame_index.set(0);
        self.schedule_frame();
    }

    fn stop_animation(&self) {
        let imp = self.imp();
        if let Some(timer) = imp.frame_timer.take() {
            timer.remove();
        }
        imp.frames.replace(Vec::new());
        imp.frame_index.set(0);
    }

    fn schedule_frame(&self) {
        let imp = self.imp();
        let delay = {
            let frames = imp.frames.borrow();
            if frames.len() < 2 {
                return;
            }
            frames[imp.frame_index.get()].1
        };
        let id = glib::timeout_add_local_once(
            delay,
            glib::clone!(
                #[weak(rename_to = canvas)]
                self,
                move || {
                    canvas.imp().frame_timer.replace(None);
                    canvas.advance_frame();
                }
            ),
        );
        imp.frame_timer.replace(Some(id));
    }

    fn advance_frame(&self) {
        let imp = self.imp();
        let next = {
            let frames = imp.frames.borrow();
            if frames.is_empty() {
                return;
            }
            (imp.frame_index.get() + 1) % frames.len()
        };
        imp.frame_index.set(next);
        let texture = imp.frames.borrow()[next].0.clone();
        // Swapped directly: zoom, rotation and flips must survive the frame.
        imp.texture.replace(Some(texture));
        self.queue_draw();
        self.schedule_frame();
    }

    pub fn set_texture(&self, texture: Option<gdk::Texture>) {
        self.stop_animation();
        let imp = self.imp();
        imp.texture.replace(texture);
        imp.user_zoomed.set(false);
        imp.rotation.set(0.0);
        imp.target_rotation.set(0.0);
        imp.flip_h.set(false);
        imp.flip_v.set(false);
        // A half-finished animation belongs to the previous image.
        if let Some(tick) = imp.tick.take() {
            tick.remove();
        }
        imp.last_frame.set(0);
        self.reflow();
        self.notify_rotation();
        self.notify_flip();
    }

    pub fn has_image(&self) -> bool {
        self.imp().texture.borrow().is_some()
    }

    /// Called with the on-screen scale as a percentage whenever it changes.
    pub fn connect_zoom_changed(&self, f: impl Fn(f64) + 'static) {
        self.imp().on_zoom_changed.replace(Some(Box::new(f)));
    }

    /// Called with the rotation in degrees, folded into [-180, 180).
    pub fn connect_rotation_changed(&self, f: impl Fn(f64) + 'static) {
        self.imp().on_rotation_changed.replace(Some(Box::new(f)));
    }

    /// Called with (horizontal, vertical) whenever mirroring changes.
    pub fn connect_flip_changed(&self, f: impl Fn(bool, bool) + 'static) {
        self.imp().on_flip_changed.replace(Some(Box::new(f)));
    }

    pub fn flip_horizontal(&self) -> bool {
        self.imp().flip_h.get()
    }

    pub fn flip_vertical(&self) -> bool {
        self.imp().flip_v.get()
    }

    /// Mirror left-to-right. Instant rather than eased: a half-finished mirror
    /// is a squashed image, which reads as a glitch rather than a transition.
    pub fn toggle_flip_horizontal(&self) {
        let imp = self.imp();
        if imp.texture.borrow().is_none() {
            return;
        }
        imp.flip_h.set(!imp.flip_h.get());
        self.after_flip();
    }

    pub fn toggle_flip_vertical(&self) {
        let imp = self.imp();
        if imp.texture.borrow().is_none() {
            return;
        }
        imp.flip_v.set(!imp.flip_v.get());
        self.after_flip();
    }

    fn after_flip(&self) {
        let imp = self.imp();
        // Mirroring does not change how much room the image needs, so the
        // scale and the centre both still hold; only the picture changes.
        imp.target_centre
            .set(self.clamp_centre(imp.ideal_centre.get(), imp.target_scale.get(), imp.target_rotation.get()));
        imp.centre.set(imp.target_centre.get());
        self.settle();
        self.notify_flip();
    }

    /// -1.0 on a mirrored axis, 1.0 otherwise.
    fn flips(&self) -> (f64, f64) {
        let imp = self.imp();
        (
            if imp.flip_h.get() { -1.0 } else { 1.0 },
            if imp.flip_v.get() { -1.0 } else { 1.0 },
        )
    }

    // -- rotation ---------------------------------------------------------

    /// The current rotation, folded into [-180, 180) for display.
    pub fn rotation(&self) -> f64 {
        normalise(self.imp().target_rotation.get())
    }

    /// Turn by a relative amount, easing into place. Used by the 90 degree
    /// buttons; the unwrapped target is what keeps the turn going the short way.
    pub fn rotate_by(&self, degrees: f64) {
        let imp = self.imp();
        if imp.texture.borrow().is_none() {
            return;
        }
        self.apply_rotation(imp.target_rotation.get() + degrees, true);
    }

    /// Set an absolute angle without easing. Used by the slider, which needs the
    /// image to track the handle exactly.
    pub fn set_rotation(&self, degrees: f64) {
        let imp = self.imp();
        if imp.texture.borrow().is_none() {
            return;
        }
        let current = imp.target_rotation.get();
        // Move to the nearest unwrapped equivalent so the slider never induces
        // a long way round.
        let delta = normalise(degrees - normalise(current));
        if delta.abs() < 1e-9 {
            return;
        }
        self.apply_rotation(current + delta, false);
    }

    fn apply_rotation(&self, rotation: f64, smooth: bool) {
        let imp = self.imp();

        let (scale, centre) = if imp.user_zoomed.get() {
            // Keep whatever is in the middle of the viewport in the middle.
            let middle = self.viewport_centre();
            let pinned = self.to_image(middle, imp.target_scale.get(), imp.target_rotation.get(), imp.ideal_centre.get());
            let scale = imp.target_scale.get();
            let centre = self.centre_placing(pinned, middle, scale, rotation);
            (scale, centre)
        } else {
            // A fitted image stays fitted: the rotated bounding box is what has
            // to fit, so the scale changes as it turns.
            (self.fit_scale_for(rotation), self.viewport_centre())
        };

        imp.target_rotation.set(rotation);
        imp.target_scale.set(scale);
        imp.ideal_centre.set(centre);
        imp.target_centre.set(self.clamp_centre(centre, scale, rotation));

        if smooth {
            self.animate();
        } else {
            if let Some(tick) = imp.tick.take() {
                tick.remove();
            }
            imp.rotation.set(rotation);
            imp.scale.set(scale);
            imp.centre.set(imp.target_centre.get());
            self.settle();
        }
        self.notify_rotation();
    }

    // -- zoom -------------------------------------------------------------

    pub fn zoom_by(&self, factor: f64, anchor: Option<(f64, f64)>) {
        let anchor = anchor.unwrap_or_else(|| self.viewport_centre());
        self.set_scale_at(self.imp().target_scale.get() * factor, anchor, true);
    }

    /// Scale the image to fit the window, and let it re-fit on resize again.
    pub fn zoom_fit(&self) {
        let imp = self.imp();
        if imp.texture.borrow().is_none() {
            return;
        }
        let scale = self.fit_scale();
        let centre = self.viewport_centre();
        imp.target_scale.set(scale);
        imp.target_centre.set(centre);
        imp.ideal_centre.set(centre);
        imp.user_zoomed.set(false);
        self.animate();
    }

    /// One image pixel per screen pixel.
    pub fn zoom_actual(&self) {
        self.set_scale_at(1.0, self.viewport_centre(), true);
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

    fn viewport_centre(&self) -> (f64, f64) {
        (self.width() as f64 / 2.0, self.height() as f64 / 2.0)
    }

    /// How much room a rotated image needs, per unit of scale.
    fn unit_bounds(&self, rotation: f64) -> Option<(f64, f64)> {
        let (tw, th) = self.texture_size()?;
        let (sin, cos) = rotation.to_radians().sin_cos();
        let (sin, cos) = (sin.abs(), cos.abs());
        Some((tw * cos + th * sin, tw * sin + th * cos))
    }

    fn fit_scale(&self) -> f64 {
        self.fit_scale_for(self.imp().target_rotation.get())
    }

    /// The scale at which the image just fits once turned. Capped at 1.0 so a
    /// small image sits at its natural size instead of being blown up to mush.
    fn fit_scale_for(&self, rotation: f64) -> f64 {
        let Some((bw, bh)) = self.unit_bounds(rotation) else {
            return 1.0;
        };
        let (w, h) = (self.width() as f64, self.height() as f64);
        if w <= 0.0 || h <= 0.0 || bw <= 0.0 || bh <= 0.0 {
            return 1.0;
        }
        (w / bw).min(h / bh).min(1.0)
    }

    /// Widget point -> image pixel.
    fn to_image(&self, point: (f64, f64), scale: f64, rotation: f64, centre: (f64, f64)) -> (f64, f64) {
        let Some((tw, th)) = self.texture_size() else {
            return (0.0, 0.0);
        };
        let (dx, dy) = (point.0 - centre.0, point.1 - centre.1);
        let (sin, cos) = (-rotation).to_radians().sin_cos();
        let rx = dx * cos - dy * sin;
        let ry = dx * sin + dy * cos;
        let (fx, fy) = self.flips();
        (rx / (scale * fx) + tw / 2.0, ry / (scale * fy) + th / 2.0)
    }

    /// Where the image centre must sit for `image` to land on `point`.
    fn centre_placing(&self, image: (f64, f64), point: (f64, f64), scale: f64, rotation: f64) -> (f64, f64) {
        let Some((tw, th)) = self.texture_size() else {
            return point;
        };
        let (fx, fy) = self.flips();
        let (ox, oy) = (
            (image.0 - tw / 2.0) * scale * fx,
            (image.1 - th / 2.0) * scale * fy,
        );
        let (sin, cos) = rotation.to_radians().sin_cos();
        let rx = ox * cos - oy * sin;
        let ry = ox * sin + oy * cos;
        (point.0 - rx, point.1 - ry)
    }

    /// Keep the image from being dragged off into empty space: an axis that
    /// overflows the viewport stays glued to its edges, one that fits is centred.
    fn clamp_centre(&self, centre: (f64, f64), scale: f64, rotation: f64) -> (f64, f64) {
        let Some((ubw, ubh)) = self.unit_bounds(rotation) else {
            return centre;
        };
        let (vw, vh) = (self.width() as f64, self.height() as f64);
        let (bw, bh) = (ubw * scale, ubh * scale);
        let x = if bw <= vw {
            vw / 2.0
        } else {
            centre.0.clamp(vw - bw / 2.0, bw / 2.0)
        };
        let y = if bh <= vh {
            vh / 2.0
        } else {
            centre.1.clamp(vh - bh / 2.0, bh / 2.0)
        };
        (x, y)
    }

    fn is_pannable(&self) -> bool {
        let imp = self.imp();
        let Some((ubw, ubh)) = self.unit_bounds(imp.target_rotation.get()) else {
            return false;
        };
        let scale = imp.target_scale.get();
        ubw * scale > self.width() as f64 + 0.5 || ubh * scale > self.height() as f64 + 0.5
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
        let rotation = imp.target_rotation.get();
        let pinned = self.to_image(anchor, previous, rotation, imp.ideal_centre.get());
        let ideal = self.centre_placing(pinned, anchor, scale, rotation);

        imp.target_scale.set(scale);
        imp.ideal_centre.set(ideal);
        imp.target_centre.set(self.clamp_centre(ideal, scale, rotation));
        imp.user_zoomed.set(true);

        if smooth {
            self.animate();
        } else {
            if let Some(tick) = imp.tick.take() {
                tick.remove();
            }
            imp.scale.set(scale);
            imp.centre.set(imp.target_centre.get());
            self.settle();
        }
    }

    /// Re-fit or re-clamp after the widget changes size.
    fn reflow(&self) {
        let imp = self.imp();
        if imp.texture.borrow().is_none() {
            return;
        }
        let rotation = imp.target_rotation.get();
        let (scale, centre) = if imp.user_zoomed.get() {
            // A window that grew can leave the image smaller than a fit.
            let scale = imp.target_scale.get().max(self.fit_scale());
            (scale, self.clamp_centre(imp.target_centre.get(), scale, rotation))
        } else {
            (self.fit_scale_for(rotation), self.viewport_centre())
        };

        // Resizing should not animate; the window is already moving.
        if let Some(tick) = imp.tick.take() {
            tick.remove();
        }
        imp.target_scale.set(scale);
        imp.target_centre.set(centre);
        imp.ideal_centre.set(centre);
        imp.rotation.set(rotation);
        imp.scale.set(scale);
        imp.centre.set(centre);
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

    fn notify_rotation(&self) {
        if let Some(callback) = self.imp().on_rotation_changed.borrow().as_ref() {
            callback(self.rotation());
        }
    }

    fn notify_flip(&self) {
        if let Some(callback) = self.imp().on_flip_changed.borrow().as_ref() {
            callback(self.imp().flip_h.get(), self.imp().flip_v.get());
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
        let (cx, cy) = imp.centre.get();
        let (tx, ty) = imp.target_centre.get();
        let (rotation, target_rotation) = (imp.rotation.get(), imp.target_rotation.get());

        // Interpolate scale geometrically: 1x to 2x should feel like 2x to 4x.
        let next_scale = (scale.ln() + (target_scale.ln() - scale.ln()) * alpha).exp();
        let next = (cx + (tx - cx) * alpha, cy + (ty - cy) * alpha);
        let next_rotation = rotation + (target_rotation - rotation) * alpha;

        let settled = (next_scale / target_scale - 1.0).abs() < 0.001
            && (tx - next.0).abs() < 0.3
            && (ty - next.1).abs() < 0.3
            && (target_rotation - next_rotation).abs() < 0.05;

        if settled {
            imp.scale.set(target_scale);
            imp.centre.set((tx, ty));
            imp.rotation.set(target_rotation);
            imp.tick.replace(None);
            self.settle();
            return glib::ControlFlow::Break;
        }

        imp.scale.set(next_scale);
        imp.centre.set(next);
        imp.rotation.set(next_rotation);
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
        let rotation = imp.rotation.get();
        let (mut cx, mut cy) = imp.centre.get();
        let width = texture.width() as f64 * scale;
        let height = texture.height() as f64 * scale;

        // At 1:1 and square-on, a half-pixel offset would blur a sharp image.
        let square_on = {
            let off = rotation.rem_euclid(90.0);
            off < 0.01 || off > 89.99
        };
        if (scale - 1.0).abs() < 0.001 && square_on {
            cx = (cx - width / 2.0).round() + width / 2.0;
            cy = (cy - height / 2.0).round() + height / 2.0;
        }

        let filter = if scale >= NEAREST_ABOVE && square_on {
            gsk::ScalingFilter::Nearest
        } else if scale < 1.0 {
            // Mipmapped, so downscaled photos do not shimmer.
            gsk::ScalingFilter::Trilinear
        } else {
            gsk::ScalingFilter::Linear
        };

        snapshot.save();
        snapshot.translate(&graphene::Point::new(cx as f32, cy as f32));
        if rotation != 0.0 {
            snapshot.rotate(rotation as f32);
        }
        // Mirror inside the rotated frame, so the flip follows the picture
        // rather than the screen. The drawn rect is centred on the origin, so a
        // negative factor mirrors it in place.
        let (fx, fy) = self.flips();
        if fx < 0.0 || fy < 0.0 {
            snapshot.scale(fx as f32, fy as f32);
        }
        snapshot.append_scaled_texture(
            &texture,
            filter,
            &graphene::Rect::new(
                (-width / 2.0) as f32,
                (-height / 2.0) as f32,
                width as f32,
                height as f32,
            ),
        );
        snapshot.restore();
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
                imp.drag_origin.set(imp.centre.get());
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
                        let current = imp.centre.get();
                        imp.drag_origin.set((current.0 - dx, current.1 - dy));
                    }
                    imp.target_scale.set(imp.scale.get());
                    imp.target_rotation.set(imp.rotation.get());
                    canvas.set_cursor_from_name(Some("grabbing"));
                }

                let (ox, oy) = imp.drag_origin.get();
                let centre = canvas.clamp_centre(
                    (ox + dx, oy + dy),
                    imp.target_scale.get(),
                    imp.target_rotation.get(),
                );
                imp.centre.set(centre);
                imp.target_centre.set(centre);
                // Dragging re-establishes where the image actually is.
                imp.ideal_centre.set(centre);
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
