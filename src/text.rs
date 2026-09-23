//! Text laid over the picture.
//!
//! A text item is kept as what it is — a string with a font and two colours —
//! rather than as pixels, so it can be moved, restyled and retyped for as long
//! as it is not baked in. Both the on-screen preview and the pixels that get
//! saved go through the same Pango layout and the same GTK render nodes, which
//! is what stops the two drifting apart.

use gtk::prelude::*;
use gtk::{gdk, graphene, pango};
use image::RgbaImage;

/// Padding around the text when it has a background plate, as a fraction of
/// the font size — so the plate keeps its proportions at any size.
const PLATE_PADDING: f64 = 0.22;

#[derive(Clone, Debug)]
pub struct TextItem {
    pub content: String,
    /// Top-left corner in display space: the image's own pixels after any flip
    /// and rotation, which is the frame the pointer is in.
    pub x: f64,
    pub y: f64,
    /// Font size in image pixels, so text keeps its proportion to the picture
    /// rather than to the window it happens to be viewed in.
    pub size: f64,
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub colour: gdk::RGBA,
    /// Alpha zero means no plate behind the text.
    pub background: gdk::RGBA,
}

impl Default for TextItem {
    fn default() -> Self {
        Self {
            content: "Text".to_string(),
            x: 0.0,
            y: 0.0,
            size: 48.0,
            family: "Sans".to_string(),
            bold: false,
            italic: false,
            underline: false,
            colour: gdk::RGBA::WHITE,
            background: gdk::RGBA::new(0.0, 0.0, 0.0, 0.0),
        }
    }
}

/// One text item already drawn into pixels, ready to be composited.
///
/// Rendering has to happen on the main loop, where the font machinery and the
/// renderer live; compositing does not, so the two are separated here and the
/// pixels travel to the worker thread.
pub struct RenderedText {
    pub pixels: RgbaImage,
    pub x: i64,
    pub y: i64,
}

impl TextItem {
    pub fn padding(&self) -> f64 {
        if self.background.alpha() > 0.0 {
            self.size * PLATE_PADDING
        } else {
            0.0
        }
    }

    pub fn layout(&self, widget: &impl IsA<gtk::Widget>) -> pango::Layout {
        let layout = widget.as_ref().create_pango_layout(Some(&self.content));
        let mut font = pango::FontDescription::new();
        font.set_family(&self.family);
        // Absolute, so the size means image pixels rather than points at some
        // assumed resolution.
        font.set_absolute_size(self.size * f64::from(pango::SCALE));
        font.set_weight(if self.bold {
            pango::Weight::Bold
        } else {
            pango::Weight::Normal
        });
        font.set_style(if self.italic {
            pango::Style::Italic
        } else {
            pango::Style::Normal
        });
        layout.set_font_description(Some(&font));
        if self.underline {
            let attributes = pango::AttrList::new();
            attributes.insert(pango::AttrInt::new_underline(pango::Underline::Single));
            layout.set_attributes(Some(&attributes));
        }
        layout
    }

    /// The whole plate: the text plus whatever padding its background needs.
    pub fn bounds(&self, widget: &impl IsA<gtk::Widget>) -> (f64, f64) {
        let (width, height) = self.layout(widget).pixel_size();
        let padding = self.padding();
        (
            f64::from(width) + padding * 2.0,
            f64::from(height) + padding * 2.0,
        )
    }

    /// Is this point, in display space, on the item?
    pub fn contains(&self, widget: &impl IsA<gtk::Widget>, x: f64, y: f64) -> bool {
        let (width, height) = self.bounds(widget);
        x >= self.x && x <= self.x + width && y >= self.y && y <= self.y + height
    }

    /// The item as render nodes, in its own coordinates with the plate's
    /// top-left at the origin. Used for the preview and for the bake alike.
    pub fn to_node(&self, widget: &impl IsA<gtk::Widget>) -> Option<gtk::gsk::RenderNode> {
        let layout = self.layout(widget);
        let (width, height) = self.bounds(widget);
        let snapshot = gtk::Snapshot::new();
        if self.background.alpha() > 0.0 {
            snapshot.append_color(
                &self.background,
                &graphene::Rect::new(0.0, 0.0, width as f32, height as f32),
            );
        }
        let padding = self.padding() as f32;
        snapshot.save();
        snapshot.translate(&graphene::Point::new(padding, padding));
        snapshot.append_layout(&layout, &self.colour);
        snapshot.restore();
        snapshot.to_node()
    }

    /// Draw the item into pixels at image resolution.
    ///
    /// Goes through the same renderer that put it on screen, so what is saved
    /// is what was previewed rather than a second implementation's idea of it.
    pub fn render(&self, widget: &impl IsA<gtk::Widget>) -> Option<RenderedText> {
        if self.content.is_empty() {
            return None;
        }
        let (width, height) = self.bounds(widget);
        let (width, height) = (width.ceil().max(1.0), height.ceil().max(1.0));
        if width > 16384.0 || height > 16384.0 {
            return None;
        }
        let node = self.to_node(widget)?;
        let renderer = widget.as_ref().native()?.renderer()?;
        let texture = renderer.render_texture(
            &node,
            Some(&graphene::Rect::new(0.0, 0.0, width as f32, height as f32)),
        );

        // Ask for straight RGBA rather than GDK's premultiplied BGRA default,
        // so no unpremultiply step of our own can get it subtly wrong.
        let mut downloader = gdk::TextureDownloader::new(&texture);
        downloader.set_format(gdk::MemoryFormat::R8g8b8a8);
        let (bytes, stride) = downloader.download_bytes();

        let (w, h) = (texture.width() as u32, texture.height() as u32);
        let mut pixels = RgbaImage::new(w, h);
        for y in 0..h as usize {
            let row = &bytes[y * stride..y * stride + w as usize * 4];
            for x in 0..w as usize {
                let p = &row[x * 4..x * 4 + 4];
                pixels.put_pixel(
                    x as u32,
                    y as u32,
                    image::Rgba([p[0], p[1], p[2], p[3]]),
                );
            }
        }
        Some(RenderedText {
            pixels,
            x: self.x.round() as i64,
            y: self.y.round() as i64,
        })
    }
}

/// Lay rendered text over an image, in place.
///
/// Straight alpha on both sides, source-over. Anything hanging off an edge is
/// simply not drawn.
pub fn composite(base: &mut RgbaImage, texts: &[RenderedText]) {
    let (bw, bh) = (base.width() as i64, base.height() as i64);
    for text in texts {
        for (tx, ty, pixel) in text.pixels.enumerate_pixels() {
            let (x, y) = (text.x + i64::from(tx), text.y + i64::from(ty));
            if x < 0 || y < 0 || x >= bw || y >= bh {
                continue;
            }
            let src = pixel.0;
            if src[3] == 0 {
                continue;
            }
            let under = base.get_pixel(x as u32, y as u32).0;
            base.put_pixel(x as u32, y as u32, image::Rgba(over(src, under)));
        }
    }
}

/// Source-over for straight alpha, in 0..255.
fn over(src: [u8; 4], dst: [u8; 4]) -> [u8; 4] {
    let sa = f32::from(src[3]) / 255.0;
    let da = f32::from(dst[3]) / 255.0;
    let out_a = sa + da * (1.0 - sa);
    if out_a <= 0.0 {
        return [0, 0, 0, 0];
    }
    let mut out = [0u8; 4];
    for c in 0..3 {
        let s = f32::from(src[c]) / 255.0;
        let d = f32::from(dst[c]) / 255.0;
        out[c] = ((s * sa + d * da * (1.0 - sa)) / out_a * 255.0).round() as u8;
    }
    out[3] = (out_a * 255.0).round() as u8;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_source_replaces_what_is_under_it() {
        assert_eq!(over([10, 20, 30, 255], [200, 200, 200, 255]), [10, 20, 30, 255]);
    }

    #[test]
    fn a_transparent_source_leaves_the_picture_alone() {
        assert_eq!(over([10, 20, 30, 0], [200, 100, 50, 255]), [200, 100, 50, 255]);
    }

    /// Half-covering black over white must land on mid grey, not on either end:
    /// getting the weighting backwards is the classic way to notice too late.
    #[test]
    fn half_alpha_lands_half_way() {
        let [r, g, b, a] = over([0, 0, 0, 128], [255, 255, 255, 255]);
        assert_eq!(a, 255);
        for channel in [r, g, b] {
            assert!((i16::from(channel) - 127).abs() <= 2, "got {channel}");
        }
    }

    /// Text hanging off the edge must be clipped, not wrapped round to the
    /// other side or panic on the way past.
    #[test]
    fn text_past_the_edge_is_clipped() {
        let mut base = RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 255]));
        let patch = RenderedText {
            pixels: RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255])),
            x: 2,
            y: 2,
        };
        composite(&mut base, &[patch]);
        assert_eq!(base.get_pixel(3, 3).0, [255, 0, 0, 255]);
        assert_eq!(base.get_pixel(0, 0).0, [0, 0, 0, 255]);
        assert_eq!(base.get_pixel(1, 3).0, [0, 0, 0, 255]);
    }
}
