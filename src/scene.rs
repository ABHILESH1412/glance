//! Translating an SVG into GTK render nodes.
//!
//! GSK is a GPU vector rasteriser: it has nodes for filled and stroked paths,
//! gradients, clips, masks, blurs and drop shadows. An SVG whose drawing maps
//! onto those nodes can be handed over whole and then zoomed with a transform —
//! nothing is re-rendered, and a drop shadow stays a drop shadow instead of
//! becoming a twenty-megapixel intermediate buffer at every zoom step. That is
//! how a browser stays fast on the same file, and it is the same technique
//! rather than an approximation of it.
//!
//! Anything that does not map — patterns, embedded images, arbitrary filter
//! chains — returns `None`, and the caller falls back to rasterising with
//! resvg. A partly-correct translation would draw wrong pixels, which is worse
//! than drawing slow ones.

use gtk::prelude::*;
use gtk::{gdk, graphene, gsk};
use resvg::usvg;

/// GTK blurs with a radius where SVG specifies a standard deviation. GSK's
/// blur uses sigma = radius / 2, so this is the conversion, not a fudge.
const BLUR_RADIUS_PER_STD_DEV: f32 = 2.0;

/// Above this many drawn shapes, GSK redrawing the whole scene on every frame
/// of a zoom costs more than rasterising a tile once and reusing it. Measured
/// on a 20,000-path drawing, where the scene spent 1.7s of CPU across a zoom
/// against 0.3s for tiles -- both imperceptible, but the gap grows with the
/// count, and tiles have no trouble with paths. Filters are the other way
/// round, which is why the scene path exists at all.
const MAX_SHAPES: usize = 50_000;

/// Build a render node for the whole drawing, in the image's own units.
///
/// `None` means this file needs the rasterising fallback.
pub fn build(tree: &usvg::Tree) -> Option<gsk::RenderNode> {
    if shapes(tree.root()) > MAX_SHAPES {
        return None;
    }
    group(tree.root())
}

/// How many shapes the drawing shows, counted before translating so a huge
/// one is turned away cheaply.
fn shapes(g: &usvg::Group) -> usize {
    g.children()
        .iter()
        .map(|child| match child {
            usvg::Node::Group(inner) => shapes(inner),
            usvg::Node::Text(t) => shapes(t.flattened()),
            _ => 1,
        })
        .sum()
}

fn group(g: &usvg::Group) -> Option<gsk::RenderNode> {
    let mut children = Vec::new();
    for child in g.children() {
        node(child, &mut children)?;
    }
    let mut out: gsk::RenderNode = gsk::ContainerNode::new(&children).upcast();

    // Blend modes other than normal would need a backdrop GSK does not give a
    // render node, so leave them to resvg.
    if g.blend_mode() != usvg::BlendMode::Normal {
        return None;
    }
    // Same order resvg composites in: filter, then clip, then mask, then
    // opacity, with the group's own transform outermost.
    for filter in g.filters() {
        out = filtered(filter, out)?;
    }
    if let Some(path) = g.clip_path() {
        out = clipped(path, out)?;
    }
    if let Some(mask) = g.mask() {
        out = masked(mask, out)?;
    }
    let opacity = g.opacity().get();
    if opacity < 1.0 {
        out = gsk::OpacityNode::new(&out, opacity).upcast();
    }
    if !g.transform().is_identity() {
        out = gsk::TransformNode::new(&out, Some(&transform(g.transform()))).upcast();
    }
    Some(out)
}

fn node(n: &usvg::Node, out: &mut Vec<gsk::RenderNode>) -> Option<()> {
    match n {
        usvg::Node::Group(g) => out.push(group(g)?),
        usvg::Node::Path(p) => painted(p, out)?,
        // usvg has already turned the glyphs into outlines for us.
        usvg::Node::Text(t) => out.push(group(t.flattened())?),
        // An embedded raster would work as a TextureNode, but it has to be
        // decoded first; not worth it before the vector case is proven.
        usvg::Node::Image(_) => return None,
    }
    Some(())
}

/// Fill and stroke for one path, in the order the SVG asked for.
fn painted(p: &usvg::Path, out: &mut Vec<gsk::RenderNode>) -> Option<()> {
    if !p.is_visible() {
        return Some(());
    }
    let path = geometry(p.data())?;
    let bounds = rect(p.bounding_box());

    let fill = match p.fill() {
        Some(f) => Some(fill_node(f, &path, &bounds)?),
        None => None,
    };
    let stroke = match p.stroke() {
        Some(s) => Some(stroke_node(s, &path, p)?),
        None => None,
    };
    match p.paint_order() {
        usvg::PaintOrder::FillAndStroke => out.extend(fill.into_iter().chain(stroke)),
        usvg::PaintOrder::StrokeAndFill => out.extend(stroke.into_iter().chain(fill)),
    }
    Some(())
}

fn fill_node(
    fill: &usvg::Fill,
    path: &gsk::Path,
    bounds: &graphene::Rect,
) -> Option<gsk::RenderNode> {
    let rule = match fill.rule() {
        usvg::FillRule::NonZero => gsk::FillRule::Winding,
        usvg::FillRule::EvenOdd => gsk::FillRule::EvenOdd,
    };
    let paint = paint_node(fill.paint(), bounds)?;
    let filled: gsk::RenderNode = gsk::FillNode::new(&paint, path, rule).upcast();
    Some(with_opacity(filled, fill.opacity().get()))
}

fn stroke_node(
    stroke: &usvg::Stroke,
    path: &gsk::Path,
    owner: &usvg::Path,
) -> Option<gsk::RenderNode> {
    let spec = gsk::Stroke::new(stroke.width().get());
    spec.set_line_cap(match stroke.linecap() {
        usvg::LineCap::Butt => gsk::LineCap::Butt,
        usvg::LineCap::Round => gsk::LineCap::Round,
        usvg::LineCap::Square => gsk::LineCap::Square,
    });
    spec.set_line_join(match stroke.linejoin() {
        usvg::LineJoin::Miter => gsk::LineJoin::Miter,
        // GSK has no clipped miter; the difference only shows on very sharp
        // corners past the limit, so treat it as the plain one.
        usvg::LineJoin::MiterClip => gsk::LineJoin::Miter,
        usvg::LineJoin::Round => gsk::LineJoin::Round,
        usvg::LineJoin::Bevel => gsk::LineJoin::Bevel,
    });
    spec.set_miter_limit(stroke.miterlimit().get());
    if let Some(dashes) = stroke.dasharray() {
        spec.set_dash(dashes);
        spec.set_dash_offset(stroke.dashoffset());
    }
    // The painted area is the outline, which reaches half a line width beyond
    // the path itself, so give any gradient the wider box to work with.
    let paint = paint_node(stroke.paint(), &rect(owner.stroke_bounding_box()))?;
    let stroked: gsk::RenderNode = gsk::StrokeNode::new(&paint, path, &spec).upcast();
    Some(with_opacity(stroked, stroke.opacity().get()))
}

fn with_opacity(node: gsk::RenderNode, opacity: f32) -> gsk::RenderNode {
    if opacity < 1.0 {
        gsk::OpacityNode::new(&node, opacity).upcast()
    } else {
        node
    }
}

/// The paint a fill or stroke is made of, as something to clip to the shape.
fn paint_node(paint: &usvg::Paint, bounds: &graphene::Rect) -> Option<gsk::RenderNode> {
    match paint {
        usvg::Paint::Color(c) => Some(gsk::ColorNode::new(&colour(*c, 1.0), bounds).upcast()),
        usvg::Paint::LinearGradient(g) => linear(g, bounds),
        usvg::Paint::RadialGradient(g) => radial(g, bounds),
        // Tiling a subtree has no GSK equivalent short of rendering it to a
        // texture, which is the thing this module exists to avoid.
        usvg::Paint::Pattern(_) => None,
    }
}

fn stops(gradient: &usvg::BaseGradient) -> Vec<gsk::ColorStop> {
    gradient
        .stops()
        .iter()
        .map(|s| gsk::ColorStop::new(s.offset().get(), colour(s.color(), s.opacity().get())))
        .collect()
}

fn linear(g: &usvg::LinearGradient, bounds: &graphene::Rect) -> Option<gsk::RenderNode> {
    let stops = stops(g);
    let start = graphene::Point::new(g.x1(), g.y1());
    let end = graphene::Point::new(g.x2(), g.y2());
    // The gradient's own coordinates, so the box has to be expressed there too.
    let (local, wrap) = gradient_space(g.transform(), bounds)?;
    let node: gsk::RenderNode = match g.spread_method() {
        usvg::SpreadMethod::Pad => {
            gsk::LinearGradientNode::new(&local, &start, &end, &stops).upcast()
        }
        usvg::SpreadMethod::Repeat => {
            gsk::RepeatingLinearGradientNode::new(&local, &start, &end, &stops).upcast()
        }
        // GSK cannot mirror a gradient.
        usvg::SpreadMethod::Reflect => return None,
    };
    Some(wrap(node))
}

fn radial(g: &usvg::RadialGradient, bounds: &graphene::Rect) -> Option<gsk::RenderNode> {
    // GSK's radial gradient has no focal point, so an off-centre one would
    // come out wrong rather than merely different.
    if (g.fx() - g.cx()).abs() > 1e-4 || (g.fy() - g.cy()).abs() > 1e-4 {
        return None;
    }
    if g.spread_method() != usvg::SpreadMethod::Pad {
        return None;
    }
    let stops = stops(g);
    let centre = graphene::Point::new(g.cx(), g.cy());
    let r = g.r().get();
    let (local, wrap) = gradient_space(g.transform(), bounds)?;
    let node: gsk::RenderNode =
        gsk::RadialGradientNode::new(&local, &centre, r, r, 0.0, 1.0, &stops).upcast();
    Some(wrap(node))
}

/// A gradient is described in its own space. Returns the painted box mapped
/// into that space, and a way to put the result back where it belongs.
fn gradient_space(
    ts: usvg::Transform,
    bounds: &graphene::Rect,
) -> Option<(graphene::Rect, Box<dyn Fn(gsk::RenderNode) -> gsk::RenderNode>)> {
    if ts.is_identity() {
        return Some((*bounds, Box::new(|node| node)));
    }
    let inverse = ts.invert()?;
    let local = transform_rect(inverse, bounds);
    let matrix = transform(ts);
    Some((
        local,
        Box::new(move |node| gsk::TransformNode::new(&node, Some(&matrix)).upcast()),
    ))
}

fn clipped(clip: &usvg::ClipPath, source: gsk::RenderNode) -> Option<gsk::RenderNode> {
    // usvg paints clip shapes solid, so the group doubles as an alpha mask.
    let mut mask = group(clip.root())?;
    if !clip.transform().is_identity() {
        mask = gsk::TransformNode::new(&mask, Some(&transform(clip.transform()))).upcast();
    }
    // A clip path may itself be clipped; the result is the intersection.
    if let Some(inner) = clip.clip_path() {
        mask = clipped(inner, mask)?;
    }
    Some(gsk::MaskNode::new(&source, &mask, gsk::MaskMode::Alpha).upcast())
}

fn masked(mask: &usvg::Mask, source: gsk::RenderNode) -> Option<gsk::RenderNode> {
    let mut shape = group(mask.root())?;
    if let Some(inner) = mask.mask() {
        shape = masked(inner, shape)?;
    }
    // Masks are bounded by their region, which a plain mask node ignores.
    shape = gsk::ClipNode::new(&shape, &rect_nonzero(mask.rect())).upcast();
    let mode = match mask.kind() {
        usvg::MaskType::Luminance => gsk::MaskMode::Luminance,
        usvg::MaskType::Alpha => gsk::MaskMode::Alpha,
    };
    Some(gsk::MaskNode::new(&source, &shape, mode).upcast())
}

/// Only the two filters that have an exact GSK counterpart. Everything else —
/// colour matrices, turbulence, multi-step chains — goes to resvg.
fn filtered(filter: &usvg::filter::Filter, source: gsk::RenderNode) -> Option<gsk::RenderNode> {
    let [primitive] = filter.primitives() else {
        return None;
    };
    match primitive.kind() {
        usvg::filter::Kind::GaussianBlur(blur) => {
            let (x, y) = (blur.std_dev_x().get(), blur.std_dev_y().get());
            // GSK blurs both axes together.
            if (x - y).abs() > 1e-3 {
                return None;
            }
            Some(gsk::BlurNode::new(&source, x * BLUR_RADIUS_PER_STD_DEV).upcast())
        }
        usvg::filter::Kind::DropShadow(shadow) => {
            let (x, y) = (shadow.std_dev_x().get(), shadow.std_dev_y().get());
            if (x - y).abs() > 1e-3 {
                return None;
            }
            let spec = gsk::Shadow::new(
                colour(shadow.color(), shadow.opacity().get()),
                shadow.dx(),
                shadow.dy(),
                x * BLUR_RADIUS_PER_STD_DEV,
            );
            // A shadow node draws the shadow and then the source on top, which
            // is exactly what feDropShadow means.
            Some(gsk::ShadowNode::new(&source, &[spec]).upcast())
        }
        _ => None,
    }
}

fn geometry(path: &usvg::tiny_skia_path::Path) -> Option<gsk::Path> {
    use usvg::tiny_skia_path::PathSegment;
    let builder = gsk::PathBuilder::new();
    for segment in path.segments() {
        match segment {
            PathSegment::MoveTo(p) => builder.move_to(p.x, p.y),
            PathSegment::LineTo(p) => builder.line_to(p.x, p.y),
            PathSegment::QuadTo(c, p) => builder.quad_to(c.x, c.y, p.x, p.y),
            PathSegment::CubicTo(a, b, p) => builder.cubic_to(a.x, a.y, b.x, b.y, p.x, p.y),
            PathSegment::Close => builder.close(),
        }
    }
    Some(builder.to_path())
}

fn colour(c: usvg::Color, opacity: f32) -> gdk::RGBA {
    gdk::RGBA::new(
        f32::from(c.red) / 255.0,
        f32::from(c.green) / 255.0,
        f32::from(c.blue) / 255.0,
        opacity,
    )
}

fn rect(r: usvg::Rect) -> graphene::Rect {
    graphene::Rect::new(r.x(), r.y(), r.width(), r.height())
}

fn rect_nonzero(r: usvg::NonZeroRect) -> graphene::Rect {
    graphene::Rect::new(r.x(), r.y(), r.width(), r.height())
}

fn transform_rect(ts: usvg::Transform, r: &graphene::Rect) -> graphene::Rect {
    let corners = [
        (r.x(), r.y()),
        (r.x() + r.width(), r.y()),
        (r.x(), r.y() + r.height()),
        (r.x() + r.width(), r.y() + r.height()),
    ];
    let mapped: Vec<(f32, f32)> = corners
        .iter()
        .map(|&(x, y)| {
            (
                ts.sx * x + ts.kx * y + ts.tx,
                ts.ky * x + ts.sy * y + ts.ty,
            )
        })
        .collect();
    let min_x = mapped.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
    let max_x = mapped.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
    let min_y = mapped.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
    let max_y = mapped.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
    graphene::Rect::new(min_x, min_y, max_x - min_x, max_y - min_y)
}

/// An SVG affine as a GSK transform. graphene multiplies row vectors, so the
/// translation lands in the last row rather than the last column.
fn transform(ts: usvg::Transform) -> gsk::Transform {
    let matrix = graphene::Matrix::from_float([
        ts.sx, ts.ky, 0.0, 0.0, //
        ts.kx, ts.sy, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        ts.tx, ts.ty, 0.0, 1.0,
    ]);
    gsk::Transform::new().matrix(&matrix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(svg: &str) -> usvg::Tree {
        usvg::Tree::from_str(svg, &usvg::Options::default()).unwrap()
    }

    /// Counting has to see through groups, or a deeply nested drawing would
    /// look tiny and sail past the budget.
    #[test]
    fn shapes_are_counted_through_groups() {
        let tree = parse(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">
                 <rect width="2" height="2"/>
                 <g><circle cx="5" cy="5" r="1"/>
                    <g><circle cx="7" cy="7" r="1"/><circle cx="8" cy="8" r="1"/></g>
                 </g>
               </svg>"#,
        );
        assert_eq!(shapes(tree.root()), 4);
    }

    /// An empty drawing must not be counted as something worth translating.
    #[test]
    fn an_empty_drawing_counts_as_nothing() {
        let tree = parse(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"/>"#);
        assert_eq!(shapes(tree.root()), 0);
    }
}
