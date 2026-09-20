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
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

use crate::decoders::svg;
use crate::loader::VectorSource;

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
        /// The image's size in its own units. For a photograph this is simply
        /// the pixel size, but for a vector it is the natural size and the
        /// texture behind it may be rendered at any resolution. Keeping the two
        /// apart is what lets an SVG be re-rendered without disturbing the view.
        pub logical: Cell<(f64, f64)>,
        pub vector: RefCell<Option<Arc<VectorSource>>>,
        /// Texture pixels per logical unit, i.e. how much detail is on hand.
        pub rendered_scale: Cell<f64>,
        pub resample: Cell<u64>,
        /// A high-resolution render of just the visible part of a vector
        /// image. Re-rendering the whole thing cannot keep up with deep zoom --
        /// at 30x an 800x800 drawing would be gigapixels -- but the part on
        /// screen is never larger than the window.
        pub tile: RefCell<Option<Tile>>,
        /// Largest scale the last render managed inside the time budget.
        ///
        /// A drawing with blur filters gets dramatically more expensive as it
        /// is enlarged -- the filter region grows with the zoom -- so rendering
        /// straight to the final scale can take seconds, during which the old
        /// tile is stretched and looks exactly like pixelation. This adapts to
        /// whatever the file and the machine can actually manage.
        pub budget_scale: Cell<f64>,
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

/// A patch of a vector image rendered at screen resolution, positioned in the
/// image's own units.
pub struct Tile {
    pub texture: gdk::Texture,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Pixels per image unit, so staleness can be judged after a zoom.
    pub scale: f64,
}

/// Wrap a decoded buffer as a texture GTK can draw.
pub fn texture_from(width: u32, height: u32, premultiplied: bool, rgba: Vec<u8>) -> gdk::Texture {
    let bytes = glib::Bytes::from_owned(rgba);
    // Mismatching this against the decoder leaves dark halos around
    // anti-aliased transparent edges.
    let format = if premultiplied {
        gdk::MemoryFormat::R8g8b8a8Premultiplied
    } else {
        gdk::MemoryFormat::R8g8b8a8
    };
    gdk::MemoryTexture::new(
        width as i32,
        height as i32,
        format,
        &bytes,
        width as usize * 4,
    )
    .upcast()
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

    /// Show a vector image, keeping its source so the view can sharpen it.
    pub fn set_vector(&self, source: Arc<VectorSource>, texture: gdk::Texture) {
        let rendered = f64::from(texture.width()) / source.width.max(1.0);
        let (logical_w, logical_h) = (source.width, source.height);
        self.set_texture(Some(texture));
        let imp = self.imp();
        imp.logical.set((logical_w, logical_h));
        imp.vector.replace(Some(source));
        imp.rendered_scale.set(rendered);
        // The window may already demand far more than the first pass gave.
        self.reflow();
    }

    pub fn set_texture(&self, texture: Option<gdk::Texture>) {
        self.stop_animation();
        let imp = self.imp();
        imp.vector.replace(None);
        imp.tile.replace(None);
        imp.budget_scale.set(f64::INFINITY);
        imp.rendered_scale.set(1.0);
        imp.logical.set(
            texture
                .as_ref()
                .map(|t| (f64::from(t.width()), f64::from(t.height())))
                .unwrap_or((1.0, 1.0)),
        );
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
        if self.imp().texture.borrow().is_none() {
            return None;
        }
        Some(self.imp().logical.get())
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
        let fit = (w / bw).min(h / bh);
        // A photograph is never enlarged past its own pixels, because that only
        // produces mush. A vector has no such limit, so let it fill the window.
        if self.imp().vector.borrow().is_some() {
            fit
        } else {
            fit.min(1.0)
        }
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
        self.maybe_resample();
    }

    /// The part of the image on screen, in image units, with a margin so a
    /// small pan does not immediately expose un-rendered area.
    fn visible_region(&self) -> Option<(f64, f64, f64, f64)> {
        let imp = self.imp();
        let (logical_w, logical_h) = imp.logical.get();
        let (w, h) = (self.width() as f64, self.height() as f64);
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        let scale = imp.target_scale.get();
        let rotation = imp.target_rotation.get();
        let centre = imp.target_centre.get();

        // Corners rather than edges: under rotation the viewport is a diamond
        // in image space, and its bounding box is what has to be covered.
        let corners = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)];
        let points: Vec<(f64, f64)> = corners
            .iter()
            .map(|&p| self.to_image(p, scale, rotation, centre))
            .collect();
        let min_x = points.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
        let max_x = points.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
        let min_y = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
        let max_y = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);

        let margin_x = (max_x - min_x) * 0.15;
        let margin_y = (max_y - min_y) * 0.15;
        let x = (min_x - margin_x).max(0.0);
        let y = (min_y - margin_y).max(0.0);
        let right = (max_x + margin_x).min(logical_w);
        let bottom = (max_y + margin_y).min(logical_h);
        if right <= x || bottom <= y {
            return None;
        }
        Some((x, y, right - x, bottom - y))
    }

    /// Re-render the visible part of a vector image when the view wants more
    /// detail than the current tile holds, or has moved off it.
    fn maybe_resample(&self) {
        let imp = self.imp();
        if imp.vector.borrow().is_none() {
            return;
        }
        // Mid-animation the scale is still moving, so wait: render once at the
        // resting size rather than chasing every frame of a zoom.
        if imp.tick.borrow().is_some() {
            return;
        }
        let Some(region) = self.visible_region() else {
            return;
        };
        let wanted = imp.target_scale.get();

        // Skip when the existing tile already covers this view sharply enough.
        if let Some(tile) = imp.tile.borrow().as_ref() {
            let covers = tile.x <= region.0 + 0.5
                && tile.y <= region.1 + 0.5
                && tile.x + tile.width >= region.0 + region.2 - 0.5
                && tile.y + tile.height >= region.1 + region.3 - 0.5;
            if covers && wanted <= tile.scale * 1.2 {
                return;
            }
        }

        let generation = imp.resample.get() + 1;
        imp.resample.set(generation);

        // A first pass at whatever renders quickly, so the view sharpens right
        // away rather than sitting on a stretched tile; the full-quality pass
        // follows once that lands.
        let first = wanted.min(imp.budget_scale.get()).max(0.01);
        self.spawn_render(generation, region, first, wanted);
    }

    /// Time a render should take before the next one is scaled back.
    const RENDER_BUDGET: std::time::Duration = std::time::Duration::from_millis(400);

    fn spawn_render(&self, generation: u64, region: (f64, f64, f64, f64), scale: f64, wanted: f64) {
        let imp = self.imp();
        let Some(source) = imp.vector.borrow().clone() else {
            return;
        };

        let (sender, receiver) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let result = svg::rasterise_region(&source, region, scale);
            let _ = sender.send_blocking((result, started.elapsed()));
        });

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            async move {
                let Ok((result, elapsed)) = receiver.recv().await else {
                    return;
                };
                let imp = canvas.imp();
                // A newer zoom or pan has already asked for something else.
                if imp.resample.get() != generation {
                    return;
                }
                let Some((image, used)) = result else {
                    return;
                };

                // Remember what this machine managed in the time allowed, so
                // the next first pass is sized to stay responsive.
                if elapsed > Self::RENDER_BUDGET {
                    let ratio = Self::RENDER_BUDGET.as_secs_f64() / elapsed.as_secs_f64();
                    // Cost grows with area, so scale back by the square root.
                    imp.budget_scale.set((used * ratio.sqrt()).max(0.01));
                } else if elapsed * 4 < Self::RENDER_BUDGET {
                    // Comfortably quick: stop holding it back.
                    imp.budget_scale.set(f64::INFINITY);
                }

                let texture =
                    texture_from(image.width, image.height, image.premultiplied, image.rgba);
                imp.tile.replace(Some(Tile {
                    texture,
                    x: region.0,
                    y: region.1,
                    width: region.2,
                    height: region.3,
                    scale: used,
                }));
                canvas.queue_draw();

                // That was only the quick pass; follow it with the real thing.
                if used < wanted * 0.99 {
                    canvas.spawn_render(generation, region, wanted, wanted);
                }
            }
        ));
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
        // Laid out against the logical size, so a re-rendered vector texture
        // changes sharpness without moving anything.
        let (logical_w, logical_h) = imp.logical.get();
        let width = logical_w * scale;
        let height = logical_h * scale;

        // At 1:1 and square-on, a half-pixel offset would blur a sharp image.
        let square_on = {
            let off = rotation.rem_euclid(90.0);
            off < 0.01 || off > 89.99
        };
        if (scale - 1.0).abs() < 0.001 && square_on {
            cx = (cx - width / 2.0).round() + width / 2.0;
            cy = (cy - height / 2.0).round() + height / 2.0;
        }

        // Nearest is for inspecting a photograph's actual pixels. A vector has
        // no pixels of its own, so blockiness there is just a stale texture
        // waiting on a re-render -- smooth it instead.
        let is_vector = imp.vector.borrow().is_some();
        let filter = if scale >= NEAREST_ABOVE && square_on && !is_vector {
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
        let to_local = |x: f64, y: f64, w: f64, h: f64| {
            graphene::Rect::new(
                ((x - logical_w / 2.0) * scale) as f32,
                ((y - logical_h / 2.0) * scale) as f32,
                (w * scale) as f32,
                (h * scale) as f32,
            )
        };
        let base_rect = to_local(0.0, 0.0, logical_w, logical_h);

        match imp.tile.borrow().as_ref() {
            Some(tile) => {
                // The whole-image render must not be drawn *underneath* the
                // sharp tile. Enlarged twelve-fold it spreads every edge
                // outwards, and those smeared edges show around the crisp ones
                // as a halo -- which reads as a glow the drawing never had.
                // So it is only painted in the bands the tile does not cover.
                let left = tile.x.max(0.0);
                let top = tile.y.max(0.0);
                let right = (tile.x + tile.width).min(logical_w);
                let bottom = (tile.y + tile.height).min(logical_h);

                for (x, y, w, h) in [
                    (0.0, 0.0, logical_w, top),
                    (0.0, bottom, logical_w, logical_h - bottom),
                    (0.0, top, left, bottom - top),
                    (right, top, logical_w - right, bottom - top),
                ] {
                    if w > 0.01 && h > 0.01 {
                        snapshot.push_clip(&to_local(x, y, w, h));
                        snapshot.append_scaled_texture(&texture, filter, &base_rect);
                        snapshot.pop();
                    }
                }

                snapshot.append_scaled_texture(
                    &tile.texture,
                    gsk::ScalingFilter::Linear,
                    &to_local(tile.x, tile.y, tile.width, tile.height),
                );
            }
            None => snapshot.append_scaled_texture(&texture, filter, &base_rect),
        }
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
                // Panning moves the visible region, so a vector may need a
                // fresh tile even though the zoom has not changed.
                canvas.maybe_resample();
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
