//! Nodes → backend-neutral vector draw operations in world coordinates.
//!
//! The GPU tessellator, the SVG writer and the PDF writer all consume these ops, so every
//! backend draws the same geometry: shape outlines, gradients, images, strokes and glyphs.

use crate::assets;
use crate::geom::{Rect, V2, v2};
use crate::ink;
use crate::model::{
    BlendMode, Cap, Color, Document, FillRule, ImageMode, Join, Node, NodeId, NodeKind, Paint,
    Stop, StrokeAlign,
};
use crate::text::{self, FaceId};
use crate::vector::{self, VPath};
use i_overlay::core::fill_rule::FillRule as OvFill;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;

#[derive(Debug, Clone, PartialEq)]
pub enum PaintSpec {
    /// Straight-alpha RGBA, opacity already applied.
    Solid([f32; 4]),
    Linear {
        p0: V2,
        p1: V2,
        stops: Vec<Stop>,
        opacity: f32,
    },
    /// Ellipse with center and the two radius vectors.
    Radial {
        center: V2,
        rx: V2,
        ry: V2,
        stops: Vec<Stop>,
        opacity: f32,
    },
    Image {
        asset: String,
        /// World rect of one copy of the image.
        place: Rect,
        tile: bool,
        opacity: f32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum DrawOp {
    Fill {
        path: VPath,
        paint: PaintSpec,
        even_odd: bool,
    },
    Stroke {
        path: VPath,
        color: [f32; 4],
        width: f64,
        cap: Cap,
        join: Join,
        dash: Vec<f64>,
    },
    /// Glyphs sharing one face, size and color; positions are world baselines.
    Glyphs {
        face: FaceId,
        size: f64,
        color: [f32; 4],
        glyphs: Vec<(u16, V2)>,
    },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NodeDraw {
    pub ops: Vec<DrawOp>,
    pub blend: BlendMode,
}

pub fn rgba(c: Color, opacity: f64) -> [f32; 4] {
    c.to_f32(opacity)
}

/// The node's outline in world space (None for text and groups).
pub fn shape_path(n: &Node, wr: Rect) -> Option<VPath> {
    Some(match &n.kind {
        NodeKind::Frame { .. } | NodeKind::Rect => vector::rounded_rect(wr, n.style.corner_radii()),
        NodeKind::Ellipse => vector::ellipse(wr),
        NodeKind::Polygon { sides } => {
            vector::rounded_polygon(&vector::polygon_points(wr, *sides), n.style.corner_radius)
        }
        NodeKind::Star { points, inner } => vector::rounded_polygon(
            &vector::star_points(wr, *points, *inner),
            n.style.corner_radius,
        ),
        NodeKind::Path { subpaths, .. } => vector::from_subpaths(wr.min(), subpaths),
        NodeKind::Line { a, b } => VPath::polyline(&[wr.min() + *a, wr.min() + *b], false),
        NodeKind::Stroke {
            points,
            width,
            thinning,
        } => {
            let o = ink::outline(points, *width, *thinning);
            let pts: Vec<V2> = o.iter().map(|q| wr.min() + *q).collect();
            VPath::polyline(&pts, true)
        }
        NodeKind::Connector { points, route, .. } => {
            let pts: Vec<V2> = points.iter().map(|p| wr.min() + *p).collect();
            crate::connector::line_path(&pts, *route)
        }
        NodeKind::Sticky { .. } => vector::rounded_rect(wr, [n.style.corner_radius.max(2.0); 4]),
        NodeKind::Text { .. } | NodeKind::Group => return None,
    })
}

/// Whether the outline is a closed area (fills apply).
pub fn is_area(n: &Node) -> bool {
    !matches!(
        n.kind,
        NodeKind::Line { .. }
            | NodeKind::Text { .. }
            | NodeKind::Group
            | NodeKind::Connector { .. }
    ) && !matches!(&n.kind, NodeKind::Path { subpaths, .. } if subpaths.iter().all(|s| !s.closed))
}

/// Stroke outline honoring inside/outside alignment for box shapes.
fn stroke_path(n: &Node, wr: Rect) -> Option<VPath> {
    let d = match n.style.stroke_align {
        StrokeAlign::Center => 0.0,
        StrokeAlign::Inside => -n.style.stroke_width * 0.5,
        StrokeAlign::Outside => n.style.stroke_width * 0.5,
    };
    if d == 0.0 {
        return shape_path(n, wr);
    }
    let r = wr.inflate(d);
    if r.w <= 0.0 || r.h <= 0.0 {
        return None;
    }
    match &n.kind {
        NodeKind::Frame { .. } | NodeKind::Rect => Some(vector::rounded_rect(
            r,
            n.style
                .corner_radii()
                .map(|x| if x > 0.0 { (x + d).max(0.0) } else { 0.0 }),
        )),
        NodeKind::Ellipse => Some(vector::ellipse(r)),
        NodeKind::Polygon { sides } => Some(vector::rounded_polygon(
            &vector::polygon_points(r, *sides),
            n.style.corner_radius,
        )),
        NodeKind::Star { points, inner } => Some(vector::rounded_polygon(
            &vector::star_points(r, *points, *inner),
            n.style.corner_radius,
        )),
        _ => shape_path(n, wr),
    }
}

/// Where one copy of an image goes for a given mode.
pub fn image_place(mode: ImageMode, wr: Rect, iw: f64, ih: f64) -> Rect {
    let (iw, ih) = (iw.max(1.0), ih.max(1.0));
    match mode {
        ImageMode::Stretch => wr,
        ImageMode::Tile => Rect::new(wr.x, wr.y, iw, ih),
        ImageMode::Fill | ImageMode::Fit => {
            let s = if mode == ImageMode::Fill {
                (wr.w / iw).max(wr.h / ih)
            } else {
                (wr.w / iw).min(wr.h / ih)
            };
            let size = v2(iw * s, ih * s);
            let c = wr.center();
            Rect::new(c.x - size.x * 0.5, c.y - size.y * 0.5, size.x, size.y)
        }
    }
}

pub fn paint_spec(p: &Paint, wr: Rect, opacity: f64) -> Option<PaintSpec> {
    let at = |u: V2| v2(wr.x + u.x * wr.w, wr.y + u.y * wr.h);
    Some(match p {
        Paint::Solid { color } => PaintSpec::Solid(rgba(*color, opacity)),
        Paint::Linear { stops, from, to } => PaintSpec::Linear {
            p0: at(*from),
            p1: at(*to),
            stops: sorted(stops),
            opacity: opacity as f32,
        },
        Paint::Radial {
            stops,
            center,
            radius,
        } => PaintSpec::Radial {
            center: at(*center),
            rx: v2(radius.x * wr.w, 0.0),
            ry: v2(0.0, radius.y * wr.h),
            stops: sorted(stops),
            opacity: opacity as f32,
        },
        Paint::Image {
            asset,
            mode,
            opacity: o,
        } => {
            let a = assets::get(asset);
            let (iw, ih) = a
                .as_ref()
                .map_or((wr.w, wr.h), |a| (a.width as f64, a.height as f64));
            if a.is_none() {
                // Missing asset: a neutral placeholder.
                return Some(PaintSpec::Solid(rgba(
                    Color::rgb(0xcc, 0xcc, 0xd2),
                    opacity * o,
                )));
            }
            PaintSpec::Image {
                asset: asset.clone(),
                place: image_place(*mode, wr, iw, ih),
                tile: *mode == ImageMode::Tile,
                opacity: (opacity * o) as f32,
            }
        }
    })
}

fn sorted(stops: &[Stop]) -> Vec<Stop> {
    let mut s = stops.to_vec();
    s.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    s
}

/// Draw ops for one node (not its children), in world space.
pub fn node_draw(n: &Node, wr: Rect, opacity: f64) -> NodeDraw {
    let mut ops = Vec::new();
    if matches!(n.kind, NodeKind::Frame { .. } | NodeKind::Rect) {
        box_shadows(n, wr, opacity, false, &mut ops);
    }
    if let Some(r) = n.style.rough {
        ops.extend(super::rough::node_ops(n, wr, opacity, &r));
        return NodeDraw {
            ops,
            blend: n.style.blend,
        };
    }
    match &n.kind {
        NodeKind::Group => {}
        NodeKind::Text { .. } => text_ops(n, wr, opacity, &mut ops),
        NodeKind::Connector { .. } => connector_ops(n, wr, opacity, &mut ops, false),
        NodeKind::Sticky { .. } => {
            let c = n
                .style
                .fills
                .last()
                .map_or(Color::rgb(0xff, 0xdf, 0x6b), |p| p.preview_color());
            let rad = [n.style.corner_radius.max(2.0); 4];
            // Soft drop shadow, then the note and its text.
            for (i, a) in [(3.0, 0.05), (1.5, 0.07)] {
                ops.push(DrawOp::Fill {
                    path: vector::rounded_rect(wr.translate(v2(0.0, i)).inflate(i * 0.5), rad),
                    paint: PaintSpec::Solid(rgba(Color::rgba(0, 0, 0, (a * 255.0) as u8), opacity)),
                    even_odd: false,
                });
            }
            ops.push(DrawOp::Fill {
                path: vector::rounded_rect(wr, rad),
                paint: PaintSpec::Solid(rgba(c, opacity)),
                even_odd: false,
            });
            if let Some(sc) = n.style.stroke
                && n.style.stroke_width > 0.0
            {
                ops.push(DrawOp::Stroke {
                    path: vector::rounded_rect(wr, rad),
                    color: rgba(sc, opacity),
                    width: n.style.stroke_width,
                    cap: n.style.cap,
                    join: n.style.join,
                    dash: n.style.dash.clone(),
                });
            }
            text_ops(n, wr, opacity, &mut ops);
        }
        NodeKind::Line { .. } => {
            if let (Some(c), Some(path)) = (n.style.stroke, shape_path(n, wr)) {
                ops.push(DrawOp::Stroke {
                    path,
                    color: rgba(c, opacity),
                    width: n.style.stroke_width.max(0.0),
                    cap: if n.style.cap == Cap::Butt {
                        Cap::Round
                    } else {
                        n.style.cap
                    },
                    join: n.style.join,
                    dash: n.style.dash.clone(),
                });
            }
        }
        NodeKind::Stroke { .. } => {
            let c = n
                .style
                .fills
                .last()
                .map_or(Color::INK, |p| p.preview_color());
            if let Some(path) = shape_path(n, wr) {
                ops.push(DrawOp::Fill {
                    path,
                    paint: PaintSpec::Solid(rgba(c, opacity)),
                    even_odd: false,
                });
            }
        }
        _ => {
            let Some(path) = shape_path(n, wr) else {
                return NodeDraw::default();
            };
            let even_odd = matches!(
                n.kind,
                NodeKind::Path {
                    fill_rule: FillRule::EvenOdd,
                    ..
                }
            );
            if is_area(n) {
                for p in &n.style.fills {
                    if let Some(paint) = paint_spec(p, wr, opacity) {
                        ops.push(DrawOp::Fill {
                            path: path.clone(),
                            paint,
                            even_odd,
                        });
                    }
                }
            }
            if matches!(n.kind, NodeKind::Frame { .. } | NodeKind::Rect) {
                box_shadows(n, wr, opacity, true, &mut ops);
            }
            if let Some(c) = n.style.stroke
                && n.style.stroke_width > 0.0
                && let Some(sp) = stroke_path(n, wr)
            {
                ops.push(DrawOp::Stroke {
                    path: sp,
                    color: rgba(c, opacity),
                    width: n.style.stroke_width,
                    cap: n.style.cap,
                    join: n.style.join,
                    dash: n.style.dash.clone(),
                });
            }
            if matches!(n.kind, NodeKind::Frame { .. } | NodeKind::Rect) {
                box_border_sides(n, wr, opacity, &mut ops);
            }
        }
    }
    NodeDraw {
        ops,
        blend: n.style.blend,
    }
}

/// Layered outer shadows: several translucent, expanding rounded silhouettes keep
/// a soft edge in GPU, SVG and PDF without depending on raster-only filters.
/// ponytail: no true Gaussian blur and the shadow also shows under a translucent fill;
/// a blur pass in the GPU backend would fix both.
/// `inset` picks the inner shadows, drawn as rings inside the box over its fill.
fn box_shadows(n: &Node, wr: Rect, opacity: f64, inset: bool, ops: &mut Vec<DrawOp>) {
    let outline =
        (inset).then(|| shape_contours(&vector::rounded_rect(wr, n.style.corner_radii())));
    for s in n.style.shadows.iter().filter(|s| s.inset == inset) {
        let base = wr.translate(v2(s.x, s.y));
        // About one layer per px of blur keeps bands invisible.
        let steps = if s.blur > 0.0 {
            (s.blur.ceil() as usize).clamp(2, 16)
        } else {
            1
        };
        // Where all layers overlap, the stack composites back to the shadow's own alpha.
        let alpha = s.color.a as f64 / 255.0 * opacity;
        let layer = 1.0 - (1.0 - alpha).powf(1.0 / steps as f64);
        let color = Color::rgba(s.color.r, s.color.g, s.color.b, 255);
        for i in (0..steps).rev() {
            // Layers ramp from blur/2 inside to blur/2 outside the spread edge, so the
            // half-opacity point sits on that edge like a CSS blur.
            let radius = if steps == 1 {
                s.spread
            } else {
                s.spread - s.blur * 0.5 + s.blur * (i as f64 + 0.5) / steps as f64
            };
            if let Some(outline) = &outline {
                // CSS inset: the box minus its shifted copy, shrunk by spread (and blur).
                let hole = base.inflate(-radius);
                let path = if hole.w <= 0.0 || hole.h <= 0.0 {
                    vector::rounded_rect(wr, n.style.corner_radii())
                } else {
                    let hole = vector::rounded_rect(
                        hole,
                        n.style.corner_radii().map(|x| (x - radius).max(0.0)),
                    );
                    let ring = outline.overlay(
                        &shape_contours(&hole),
                        OverlayRule::Difference,
                        OvFill::NonZero,
                    );
                    let mut path = VPath::new();
                    for c in ring.iter().flatten() {
                        let pts: Vec<V2> = c.iter().map(|p| v2(p[0], p[1])).collect();
                        path.append(&VPath::polyline(&pts, true));
                    }
                    path
                };
                ops.push(DrawOp::Fill {
                    path,
                    paint: PaintSpec::Solid(rgba(color, layer)),
                    even_odd: true,
                });
                continue;
            }
            let r = base.inflate(radius);
            if r.w <= 0.0 || r.h <= 0.0 {
                continue;
            }
            ops.push(DrawOp::Fill {
                path: vector::rounded_rect(
                    r,
                    n.style.corner_radii().map(|x| (x + radius).max(0.0)),
                ),
                paint: PaintSpec::Solid(rgba(color, layer)),
                even_odd: false,
            });
        }
    }
}

fn shape_contours(p: &VPath) -> Vec<Vec<[f64; 2]>> {
    p.flatten(0.1)
        .into_iter()
        .filter(|p| p.pts.len() >= 3)
        .map(|p| p.pts.iter().map(|q| [q.x, q.y]).collect())
        .collect()
}

/// Inside strokes along selected box edges, shared by GPU, SVG and PDF backends.
fn box_border_sides(n: &Node, wr: Rect, opacity: f64, ops: &mut Vec<DrawOp>) {
    for (side, border) in n.style.border_sides.iter().enumerate() {
        let Some(border) = border else { continue };
        if border.width <= 0.0 || wr.w <= 0.0 || wr.h <= 0.0 {
            continue;
        }
        let half = border.width * 0.5;
        let (a, b) = match side {
            0 => (v2(wr.x, wr.y + half), v2(wr.x + wr.w, wr.y + half)),
            1 => (
                v2(wr.x + wr.w - half, wr.y),
                v2(wr.x + wr.w - half, wr.y + wr.h),
            ),
            2 => (
                v2(wr.x, wr.y + wr.h - half),
                v2(wr.x + wr.w, wr.y + wr.h - half),
            ),
            _ => (v2(wr.x + half, wr.y), v2(wr.x + half, wr.y + wr.h)),
        };
        ops.push(DrawOp::Stroke {
            path: VPath::polyline(&[a, b], false),
            color: rgba(border.color, opacity),
            width: border.width,
            cap: Cap::Butt,
            join: Join::Miter,
            dash: border.dash.clone(),
        });
    }
}

/// A connector's line and heads. With `rough`, only the heads are produced (the line comes
/// from the rough renderer).
pub fn connector_ops(n: &Node, wr: Rect, opacity: f64, ops: &mut Vec<DrawOp>, heads_only: bool) {
    let NodeKind::Connector {
        points,
        route,
        start_head,
        end_head,
        ..
    } = &n.kind
    else {
        return;
    };
    let Some(c) = n.style.stroke else { return };
    let color = rgba(c, opacity);
    let sw = n.style.stroke_width.max(0.5);
    let pts: Vec<V2> = points.iter().map(|p| wr.min() + *p).collect();
    if pts.len() < 2 {
        return;
    }
    if !heads_only {
        ops.push(DrawOp::Stroke {
            path: crate::connector::line_path(&pts, *route),
            color,
            width: sw,
            cap: Cap::Round,
            join: Join::Round,
            dash: n.style.dash.clone(),
        });
    }
    let last = pts.len() - 1;
    for (head, tip, from) in [
        (*start_head, pts[0], pts[1]),
        (*end_head, pts[last], pts[last - 1]),
    ] {
        let (lines, polys) = crate::connector::head_shapes(head, tip, from, sw, tip.dist(from));
        for l in lines {
            ops.push(DrawOp::Stroke {
                path: VPath::polyline(&l, false),
                color,
                width: sw,
                cap: Cap::Round,
                join: Join::Round,
                dash: Vec::new(),
            });
        }
        for p in polys {
            let path = VPath::polyline(&p, true);
            ops.push(DrawOp::Fill {
                path: path.clone(),
                paint: PaintSpec::Solid(color),
                even_odd: false,
            });
            ops.push(DrawOp::Stroke {
                path,
                color,
                width: sw,
                cap: Cap::Round,
                join: Join::Round,
                dash: Vec::new(),
            });
        }
    }
}

fn text_ops(n: &Node, wr: Rect, opacity: f64, ops: &mut Vec<DrawOp>) {
    let Some(l) = text::layout_node(n) else {
        return;
    };
    let o = wr.min() + text::text_offset(n);
    let mut i = 0;
    while i < l.glyphs.len() {
        let g0 = l.glyphs[i];
        let mut glyphs = Vec::new();
        while i < l.glyphs.len() {
            let g = &l.glyphs[i];
            if g.face != g0.face || g.size != g0.size || g.color != g0.color {
                break;
            }
            glyphs.push((g.gid, o + v2(g.x, g.baseline)));
            i += 1;
        }
        ops.push(DrawOp::Glyphs {
            face: g0.face,
            size: g0.size,
            color: rgba(g0.color, opacity),
            glyphs,
        });
    }
    // Underline / strikethrough as thin rectangles.
    for g in l.glyphs.iter().filter(|g| g.underline || g.strike) {
        let th = (g.size / 14.0).max(0.5);
        for (on, dy) in [(g.underline, g.size * 0.12), (g.strike, -g.size * 0.28)] {
            if on {
                let r = Rect::new(o.x + g.x, o.y + g.baseline + dy, g.advance, th);
                ops.push(DrawOp::Fill {
                    path: vector::rounded_rect(r, [0.0; 4]),
                    paint: PaintSpec::Solid(rgba(g.color, opacity)),
                    even_odd: false,
                });
            }
        }
    }
}

/// Product of ancestor opacities.
pub fn inherited_opacity(doc: &Document, id: NodeId) -> f64 {
    doc.ancestors(id)
        .iter()
        .filter_map(|a| doc.get(*a))
        .map(|n| n.style.opacity)
        .product()
}

/// World bounds a node draws into (rect plus stroke overhang).
pub fn draw_bounds(n: &Node, wr: Rect) -> Rect {
    let mut pad = match n.style.stroke_align {
        StrokeAlign::Inside => 0.0,
        StrokeAlign::Center => n.style.stroke_width * 0.5,
        StrokeAlign::Outside => n.style.stroke_width,
    };
    if n.style.stroke.is_none() && !matches!(n.kind, NodeKind::Line { .. }) {
        pad = 0.0;
    }
    if n.style.rough.is_some() {
        pad += n.style.stroke_width * 2.0 + 4.0;
    }
    if matches!(n.kind, NodeKind::Line { .. }) {
        pad = pad.max(n.style.stroke_width * 0.5);
    }
    if matches!(n.kind, NodeKind::Connector { .. }) {
        // Arrowheads reach past the end points.
        pad = pad.max(n.style.stroke_width + 20.0);
    }
    if matches!(n.kind, NodeKind::Sticky { .. }) {
        pad = pad.max(4.0);
    }
    let mut bounds = wr.inflate(pad);
    if matches!(n.kind, NodeKind::Frame { .. } | NodeKind::Rect) {
        for s in n.style.shadows.iter().filter(|s| !s.inset) {
            let outer = wr.translate(v2(s.x, s.y)).inflate(s.spread + s.blur);
            if outer.w > 0.0 && outer.h > 0.0 {
                bounds = bounds.union(&outer);
            }
        }
    }
    bounds
}

/// A 2D affine transform: `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Xf {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Xf {
    /// Clockwise rotation by `deg` (y points down, like CSS) about `center`.
    pub fn rotate(center: V2, deg: f64) -> Xf {
        let (s, c) = deg.to_radians().sin_cos();
        Xf {
            a: c,
            b: s,
            c: -s,
            d: c,
            e: center.x - c * center.x + s * center.y,
            f: center.y - s * center.x - c * center.y,
        }
    }
    pub fn apply(&self, p: V2) -> V2 {
        v2(
            self.a * p.x + self.c * p.y + self.e,
            self.b * p.x + self.d * p.y + self.f,
        )
    }
    fn vector(&self, p: V2) -> V2 {
        v2(self.a * p.x + self.c * p.y, self.b * p.x + self.d * p.y)
    }
    /// `self` first, then `outer`.
    pub fn then(&self, outer: &Xf) -> Xf {
        let p = outer;
        Xf {
            a: p.a * self.a + p.c * self.b,
            b: p.b * self.a + p.d * self.b,
            c: p.a * self.c + p.c * self.d,
            d: p.b * self.c + p.d * self.d,
            e: p.a * self.e + p.c * self.f + p.e,
            f: p.b * self.e + p.d * self.f + p.f,
        }
    }
    /// Axis-aligned bounds of a transformed rect.
    pub fn bounds(&self, r: Rect) -> Rect {
        let pts = [
            v2(r.x, r.y),
            v2(r.right(), r.y),
            v2(r.x, r.bottom()),
            v2(r.right(), r.bottom()),
        ]
        .map(|p| self.apply(p));
        let (x0, x1) = (
            pts.iter().map(|p| p.x).fold(f64::INFINITY, f64::min),
            pts.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max),
        );
        let (y0, y1) = (
            pts.iter().map(|p| p.y).fold(f64::INFINITY, f64::min),
            pts.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max),
        );
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
}

/// The transform for a node drawn at `wr` inside a parent transformed by `parent`.
pub fn node_xf(n: &Node, wr: Rect, parent: Option<Xf>) -> Option<Xf> {
    if n.style.rotation == 0.0 {
        return parent;
    }
    let own = Xf::rotate(wr.center(), n.style.rotation);
    Some(parent.map_or(own, |p| own.then(&p)))
}

/// The transform a node is drawn with: its own rotation and every ancestor's.
pub fn world_xf(doc: &Document, id: NodeId) -> Option<Xf> {
    let mut chain = doc.ancestors(id);
    chain.reverse(); // root first
    chain.push(id);
    let mut xf = None;
    for i in chain {
        if let (Some(n), Some(wr)) = (doc.get(i), doc.world_rect(i)) {
            xf = node_xf(n, wr, xf);
        }
    }
    xf
}

/// A world rect as drawn for `id`, after its own and its ancestors' rotations.
pub fn turned(doc: &Document, id: NodeId, r: Rect) -> Rect {
    world_xf(doc, id).map_or(r, |x| x.bounds(r))
}

/// Draw ops moved by a transform. Glyph runs become outline fills so they turn too;
/// image fills keep an upright placement inside the turned outline.
pub fn transform_ops(ops: Vec<DrawOp>, xf: &Xf) -> Vec<DrawOp> {
    let map = |p: &VPath| p.map(|q| xf.apply(q));
    ops.into_iter()
        .map(|op| match op {
            DrawOp::Fill {
                path,
                paint,
                even_odd,
            } => DrawOp::Fill {
                path: map(&path),
                paint: match paint {
                    PaintSpec::Linear {
                        p0,
                        p1,
                        stops,
                        opacity,
                    } => PaintSpec::Linear {
                        p0: xf.apply(p0),
                        p1: xf.apply(p1),
                        stops,
                        opacity,
                    },
                    PaintSpec::Radial {
                        center,
                        rx,
                        ry,
                        stops,
                        opacity,
                    } => PaintSpec::Radial {
                        center: xf.apply(center),
                        rx: xf.vector(rx),
                        ry: xf.vector(ry),
                        stops,
                        opacity,
                    },
                    PaintSpec::Image {
                        asset,
                        place,
                        tile,
                        opacity,
                    } => {
                        let c = xf.apply(place.center());
                        let place =
                            Rect::new(c.x - place.w * 0.5, c.y - place.h * 0.5, place.w, place.h);
                        PaintSpec::Image {
                            asset,
                            place,
                            tile,
                            opacity,
                        }
                    }
                    p => p,
                },
                even_odd,
            },
            DrawOp::Stroke {
                path,
                color,
                width,
                cap,
                join,
                dash,
            } => DrawOp::Stroke {
                path: map(&path),
                color,
                width,
                cap,
                join,
                dash,
            },
            DrawOp::Glyphs {
                face,
                size,
                color,
                glyphs,
            } => {
                let mut path = VPath::new();
                for (gid, o) in glyphs {
                    path.append(&text::glyph_path(face, gid, o, size));
                }
                DrawOp::Fill {
                    path: map(&path),
                    paint: PaintSpec::Solid(color),
                    even_odd: false,
                }
            }
        })
        .collect()
}
