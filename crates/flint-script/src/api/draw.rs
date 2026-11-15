//! The `draw` module: the immediate-mode 2D draw calls a script's
//! `on_draw_ui` makes (text, rectangles, circles, lines, rings, polygons,
//! sprites), the clip and over-UI stacks, the screen size and text
//! measurement (ADR 0116). Coordinates are logical points from the top
//! left, colours 0..1, and `layer` orders draws (higher on top).
//!
//! Each primitive has two forms under one name (#702): what places it and
//! its colour as positional numbers (`draw_rect(x, y, w, h, r, g, b, a)`),
//! and what places it plus an opts map for everything else
//! (`draw_rect(x, y, w, h, #{color, rounding, layer})`, `opts.rs`). The
//! `_ex` variants and the long positional lists are deprecated names of the
//! opts forms, kept for one release.
//!
//! These are the hottest functions in the API: a HUD makes hundreds of
//! them a frame. They go through `.run()`, whose exact-typed twin is as
//! fast as a typed closure for float calls and whose all-int copy
//! (functions with four or more `Num` parameters) is as fast for int
//! calls (docs/design/script-api-overhead.md, which also has what an opts
//! map costs).

use super::builder::kind::{Array, Bool, Int, Map as MapKind, Num, Str};
use super::builder::{Call, ScriptApi};
use super::opts::{
    measure_text_with, read_opts, warn_opts, Opts, Problem, Rounding, TextOpts, TEXT_KEYS, WHITE,
};
use crate::context::DrawCommand;
use rhai::{Dynamic, Map};

#[inline]
fn rgba(r: f64, g: f64, b: f64, a: f64) -> [f32; 4] {
    [r as f32, g as f32, b as f32, a as f32]
}

#[inline]
fn push(call: &mut Call<'_>, cmd: DrawCommand) {
    call.ctx().push_draw(cmd);
}

/// The opts of a call of `func`, its problems warned about once.
#[inline]
fn opts(call: &mut Call<'_>, func: &'static str, map: &Map, keys: &[&str]) -> Opts {
    read_opts(call.ctx(), func, map, keys)
}

// The keys each opts form takes.
const RECT: &[&str] = &["color", "rounding", "layer"];
const RECT_OUTLINE: &[&str] = &["color", "thickness", "rounding", "layer"];
const GRADIENT: &[&str] = &["from", "to", "vertical", "layer"];
const FILL: &[&str] = &["color", "layer"];
const STROKE: &[&str] = &["color", "thickness", "layer"];
const POLYGON_OUTLINE: &[&str] = &["color", "thickness", "closed", "layer"];
const SPRITE: &[&str] = &["uv", "color", "layer"];
const MEASURE: &[&str] = &["font", "spacing"];

/// A text draw.
fn text(call: &mut Call<'_>, x: f64, y: f64, text: &str, size: f64, o: TextOpts) {
    push(
        call,
        DrawCommand::Text {
            x: x as f32,
            y: y as f32,
            text: text.to_string(),
            size: size as f32,
            color: o.color,
            layer: o.layer,
            align: o.align,
            stroke: o.stroke,
            font: o.font,
            letter_spacing: o.spacing,
            shadow: o.shadow,
            glow: o.glow,
        },
    );
}

/// A text draw's options in the default font, left-aligned.
fn plain_text(color: [f32; 4], layer: i64, stroke: Option<([f32; 4], f32)>) -> TextOpts {
    TextOpts {
        color,
        layer: layer as i32,
        stroke,
        ..TextOpts::default()
    }
}

/// A filled rectangle: one rounding for every corner, or each its own.
#[allow(clippy::too_many_arguments)]
fn rect(
    call: &mut Call<'_>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    color: [f32; 4],
    rounding: Rounding,
    layer: i32,
) {
    let (x, y, w, h) = (x as f32, y as f32, w as f32, h as f32);
    let cmd = match rounding {
        Rounding::All(rounding) => DrawCommand::RectFilled {
            x,
            y,
            w,
            h,
            color,
            rounding,
            layer,
        },
        Rounding::Corners(rounding) => DrawCommand::RectRounded4 {
            x,
            y,
            w,
            h,
            color,
            rounding,
            layer,
        },
    };
    push(call, cmd);
}

#[allow(clippy::too_many_arguments)]
fn rect_outline(
    call: &mut Call<'_>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    color: [f32; 4],
    thickness: f32,
    rounding: f32,
    layer: i32,
) {
    push(
        call,
        DrawCommand::RectOutline {
            x: x as f32,
            y: y as f32,
            w: w as f32,
            h: h as f32,
            color,
            thickness,
            rounding,
            layer,
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn gradient(
    call: &mut Call<'_>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    color_a: [f32; 4],
    color_b: [f32; 4],
    vertical: bool,
    layer: i32,
) {
    push(
        call,
        DrawCommand::RectGradient {
            x: x as f32,
            y: y as f32,
            w: w as f32,
            h: h as f32,
            color_a,
            color_b,
            vertical,
            layer,
        },
    );
}

fn circle(call: &mut Call<'_>, x: f64, y: f64, radius: f64, color: [f32; 4], layer: i32) {
    push(
        call,
        DrawCommand::CircleFilled {
            x: x as f32,
            y: y as f32,
            radius: radius as f32,
            color,
            layer,
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn circle_outline(
    call: &mut Call<'_>,
    x: f64,
    y: f64,
    radius: f64,
    color: [f32; 4],
    thickness: f32,
    layer: i32,
) {
    push(
        call,
        DrawCommand::CircleOutline {
            x: x as f32,
            y: y as f32,
            radius: radius as f32,
            color,
            thickness,
            layer,
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn line(
    call: &mut Call<'_>,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    color: [f32; 4],
    thickness: f32,
    layer: i32,
) {
    push(
        call,
        DrawCommand::Line {
            x1: x1 as f32,
            y1: y1 as f32,
            x2: x2 as f32,
            y2: y2 as f32,
            color,
            thickness,
            layer,
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn ring(
    call: &mut Call<'_>,
    cx: f64,
    cy: f64,
    r_inner: f32,
    r_outer: f32,
    start_deg: f64,
    end_deg: f64,
    color: [f32; 4],
    layer: i32,
) {
    push(
        call,
        DrawCommand::Ring {
            cx: cx as f32,
            cy: cy as f32,
            r_inner,
            r_outer,
            start_deg: start_deg as f32,
            end_deg: end_deg as f32,
            color,
            layer,
        },
    );
}

/// An arc: a ring centred on `radius`, `thickness` wide.
#[allow(clippy::too_many_arguments)]
fn arc(
    call: &mut Call<'_>,
    cx: f64,
    cy: f64,
    radius: f64,
    start_deg: f64,
    end_deg: f64,
    color: [f32; 4],
    thickness: f32,
    layer: i32,
) {
    let half = thickness * 0.5;
    let r_inner = (radius as f32 - half).max(0.0);
    let r_outer = radius as f32 + half;
    ring(
        call, cx, cy, r_inner, r_outer, start_deg, end_deg, color, layer,
    );
}

fn polygon(call: &mut Call<'_>, func: &str, points: &rhai::Array, color: [f32; 4], layer: i32) {
    let Some(points) = polygon_points(func, points) else {
        return;
    };
    if points.len() < 3 {
        return;
    }
    push(
        call,
        DrawCommand::Polygon {
            points,
            color,
            layer,
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn polygon_outline(
    call: &mut Call<'_>,
    func: &str,
    points: &rhai::Array,
    color: [f32; 4],
    thickness: f32,
    closed: bool,
    layer: i32,
) {
    let Some(points) = polygon_points(func, points) else {
        return;
    };
    if points.len() < 2 {
        return;
    }
    push(
        call,
        DrawCommand::PolygonOutline {
            points,
            color,
            thickness,
            closed,
            layer,
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn sprite(
    call: &mut Call<'_>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    name: &str,
    uv: [f32; 4],
    tint: [f32; 4],
    layer: i32,
) {
    push(
        call,
        DrawCommand::Sprite {
            x: x as f32,
            y: y as f32,
            w: w as f32,
            h: h as f32,
            name: name.to_string(),
            uv,
            tint,
            layer,
        },
    );
}

/// `#{width, height}` of a measured text.
fn size_map((w, h): (f32, f32)) -> Map {
    let mut map = Map::new();
    map.insert("width".into(), Dynamic::from(w as f64));
    map.insert("height".into(), Dynamic::from(h as f64));
    map
}

/// The points of a polygon call, or `None` (having warned) when they are
/// neither `[[x, y], ...]` nor a flat even-length list of numbers.
fn polygon_points(func: &str, points: &rhai::Array) -> Option<Vec<[f32; 2]>> {
    let parsed = crate::draw2d::parse_points(points);
    if parsed.is_none() {
        tracing::warn!(
            "{func}: points must be [[x,y],...] or a flat even-length [x0,y0,...] list of numbers"
        );
    }
    parsed
}

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("draw");

    // ── Text ──
    m.func(
        "draw_text",
        "Draw text at a point, its top left, in the default font.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("text", Str)
    .arg("size", Num)
    .arg("r", Num)
    .arg("g", Num)
    .arg("b", Num)
    .arg("a", Num)
    .run(|call, (x, y, t, size, r, g, b, a)| {
        text(call, x, y, &t, size, plain_text(rgba(r, g, b, a), 0, None));
    });

    m.func(
        "draw_text",
        "Draw text at a point; opts: color, font, align, stroke [r, g, b, a, width], spacing, layer, shadow [dx, dy, r, g, b, a], glow [r, g, b, a, radius].",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("text", Str)
    .arg("size", Num)
    .arg("opts", MapKind)
    .run(|call, (x, y, t, size, map)| {
        let o = TextOpts::from_opts(opts(call, "draw_text", &map, TEXT_KEYS));
        text(call, x, y, &t, size, o);
    });

    m.func("draw_text_ex", "draw_text on a layer.")
        .arg("x", Num)
        .arg("y", Num)
        .arg("text", Str)
        .arg("size", Num)
        .arg("r", Num)
        .arg("g", Num)
        .arg("b", Num)
        .arg("a", Num)
        .arg("layer", Int)
        .deprecated("use draw_text(x, y, text, size, #{color, layer})")
        .run(|call, (x, y, t, size, r, g, b, a, layer)| {
            let o = plain_text(rgba(r, g, b, a), layer, None);
            text(call, x, y, &t, size, o);
        });

    m.func(
        "draw_text_stroked",
        "draw_text with an outline: its colour and width in points.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("text", Str)
    .arg("size", Num)
    .arg("r", Num)
    .arg("g", Num)
    .arg("b", Num)
    .arg("a", Num)
    .arg("stroke_r", Num)
    .arg("stroke_g", Num)
    .arg("stroke_b", Num)
    .arg("stroke_a", Num)
    .arg("stroke_width", Num)
    .deprecated("use draw_text(x, y, text, size, #{color, stroke: [r, g, b, a, width]})")
    .run(|call, (x, y, t, size, r, g, b, a, sr, sg, sb, sa, sw)| {
        let stroke = Some((rgba(sr, sg, sb, sa), sw as f32));
        text(
            call,
            x,
            y,
            &t,
            size,
            plain_text(rgba(r, g, b, a), 0, stroke),
        );
    });

    m.func("draw_text_stroked_ex", "draw_text_stroked on a layer.")
        .arg("x", Num)
        .arg("y", Num)
        .arg("text", Str)
        .arg("size", Num)
        .arg("r", Num)
        .arg("g", Num)
        .arg("b", Num)
        .arg("a", Num)
        .arg("stroke_r", Num)
        .arg("stroke_g", Num)
        .arg("stroke_b", Num)
        .arg("stroke_a", Num)
        .arg("stroke_width", Num)
        .arg("layer", Int)
        .deprecated("use draw_text(x, y, text, size, #{color, stroke: [r, g, b, a, width], layer})")
        .run(
            |call, (x, y, t, size, r, g, b, a, sr, sg, sb, sa, sw, layer)| {
                let stroke = Some((rgba(sr, sg, sb, sa), sw as f32));
                let o = plain_text(rgba(r, g, b, a), layer, stroke);
                text(call, x, y, &t, size, o);
            },
        );

    m.func(
        "draw_text_opts",
        "Draw text with a map of any of font, color, align, stroke, spacing, layer, shadow, glow.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("text", Str)
    .arg("size", Num)
    .arg("opts", MapKind)
    .deprecated("use draw_text(x, y, text, size, opts)")
    .run(|call, (x, y, t, size, map)| {
        let o = TextOpts::from_opts(opts(call, "draw_text_opts", &map, TEXT_KEYS));
        text(call, x, y, &t, size, o);
    });

    m.func(
        "measure_text",
        "The size text would draw at, in the default font.",
    )
    .arg("text", Str)
    .arg("size", Num)
    .returns(MapKind, "#{width, height} in points")
    .run(|call, (t, size)| {
        size_map(measure_text_with(
            call.ctx_ref(),
            &t,
            size as f32,
            None,
            0.0,
        ))
    });

    m.func(
        "measure_text",
        "The size text would draw at; opts: font (\"\" for the default), spacing (extra letter spacing).",
    )
    .arg("text", Str)
    .arg("size", Num)
    .arg("opts", MapKind)
    .returns(MapKind, "#{width, height} in points")
    .run(|call, (t, size, map)| {
        let o = opts(call, "measure_text", &map, MEASURE);
        size_map(measure_text_with(
            call.ctx_ref(),
            &t,
            size as f32,
            o.font.as_deref(),
            o.spacing.unwrap_or(0.0),
        ))
    });

    m.func(
        "measure_text_ex",
        "The size text would draw at in a font (\"\" for the default) with extra letter spacing.",
    )
    .arg("text", Str)
    .arg("size", Num)
    .arg("font", Str)
    .arg("spacing", Num)
    .returns(MapKind, "#{width, height} in points")
    .deprecated("use measure_text(text, size, #{font, spacing})")
    .run(|call, (t, size, font, spacing)| {
        let family = (!font.is_empty()).then_some(font.as_str());
        size_map(measure_text_with(
            call.ctx_ref(),
            &t,
            size as f32,
            family,
            spacing as f32,
        ))
    });

    // ── Rectangles ──
    m.func("draw_rect", "Fill a rectangle.")
        .arg("x", Num)
        .arg("y", Num)
        .arg("w", Num)
        .arg("h", Num)
        .arg("r", Num)
        .arg("g", Num)
        .arg("b", Num)
        .arg("a", Num)
        .run(|call, (x, y, w, h, r, g, b, a)| {
            rect(call, x, y, w, h, rgba(r, g, b, a), Rounding::All(0.0), 0);
        });

    m.func(
        "draw_rect",
        "Fill a rectangle; opts: color, rounding (a radius, or [tl, tr, br, bl]), layer.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("w", Num)
    .arg("h", Num)
    .arg("opts", MapKind)
    .run(|call, (x, y, w, h, map)| {
        let o = opts(call, "draw_rect", &map, RECT);
        let rounding = o.rounding.unwrap_or(Rounding::All(0.0));
        rect(call, x, y, w, h, o.color(), rounding, o.layer());
    });

    m.func(
        "draw_rect_ex",
        "Fill a rectangle with rounded corners, on a layer.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("w", Num)
    .arg("h", Num)
    .arg("r", Num)
    .arg("g", Num)
    .arg("b", Num)
    .arg("a", Num)
    .arg("rounding", Num)
    .arg("layer", Int)
    .deprecated("use draw_rect(x, y, w, h, #{color, rounding, layer})")
    .run(|call, (x, y, w, h, r, g, b, a, rounding, layer)| {
        let rounding = Rounding::All(rounding as f32);
        rect(call, x, y, w, h, rgba(r, g, b, a), rounding, layer as i32);
    });

    m.func(
        "draw_rect_ex4",
        "Fill a rectangle with its own rounding at each corner (top left, top right, bottom right, bottom left).",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("w", Num)
    .arg("h", Num)
    .arg("r", Num)
    .arg("g", Num)
    .arg("b", Num)
    .arg("a", Num)
    .arg("tl", Num)
    .arg("tr", Num)
    .arg("br", Num)
    .arg("bl", Num)
    .arg("layer", Int)
    .deprecated("use draw_rect(x, y, w, h, #{color, rounding: [tl, tr, br, bl], layer})")
    .run(|call, (x, y, w, h, r, g, b, a, tl, tr, br, bl, layer)| {
        let rounding = Rounding::Corners([tl as f32, tr as f32, br as f32, bl as f32]);
        rect(call, x, y, w, h, rgba(r, g, b, a), rounding, layer as i32);
    });

    m.func(
        "draw_rect_gradient",
        "Fill a rectangle from one colour to another; opts: from, to (colours, white), vertical (top to bottom, true; false is left to right), layer.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("w", Num)
    .arg("h", Num)
    .arg("opts", MapKind)
    .run(|call, (x, y, w, h, map)| {
        let o = opts(call, "draw_rect_gradient", &map, GRADIENT);
        let (from, to) = (o.from.unwrap_or(WHITE), o.to.unwrap_or(WHITE));
        let vertical = o.vertical.unwrap_or(true);
        gradient(call, x, y, w, h, from, to, vertical, o.layer());
    });

    m.func(
        "draw_rect_gradient",
        "Fill a rectangle from one colour to another, top to bottom when vertical, else left to right.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("w", Num)
    .arg("h", Num)
    .arg("r1", Num)
    .arg("g1", Num)
    .arg("b1", Num)
    .arg("a1", Num)
    .arg("r2", Num)
    .arg("g2", Num)
    .arg("b2", Num)
    .arg("a2", Num)
    .arg("vertical", Bool)
    .arg("layer", Int)
    .deprecated("use draw_rect_gradient(x, y, w, h, #{from, to, vertical, layer})")
    .run(
        |call, (x, y, w, h, r1, g1, b1, a1, r2, g2, b2, a2, vertical, layer)| {
            let (from, to) = (rgba(r1, g1, b1, a1), rgba(r2, g2, b2, a2));
            gradient(call, x, y, w, h, from, to, vertical, layer as i32);
        },
    );

    m.func(
        "draw_rect_outline",
        "Outline a rectangle, thickness in points.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("w", Num)
    .arg("h", Num)
    .arg("r", Num)
    .arg("g", Num)
    .arg("b", Num)
    .arg("a", Num)
    .arg("thickness", Num)
    .run(|call, (x, y, w, h, r, g, b, a, thickness)| {
        let (color, t) = (rgba(r, g, b, a), thickness as f32);
        rect_outline(call, x, y, w, h, color, t, 0.0, 0);
    });

    m.func(
        "draw_rect_outline",
        "Outline a rectangle; opts: color, thickness (points, 1), rounding (a radius), layer.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("w", Num)
    .arg("h", Num)
    .arg("opts", MapKind)
    .run(|call, (x, y, w, h, map)| {
        let o = opts(call, "draw_rect_outline", &map, RECT_OUTLINE);
        let rounding = match o.rounding {
            None => 0.0,
            Some(Rounding::All(r)) => r,
            // An outline has one radius: four corners are a misshapen key
            Some(Rounding::Corners(c)) => {
                let p = Problem {
                    key: "rounding".into(),
                    known: true,
                };
                warn_opts(call.ctx(), "draw_rect_outline", &p, RECT_OUTLINE);
                c[0]
            }
        };
        let (color, thickness, layer) = (o.color(), o.thickness(), o.layer());
        rect_outline(call, x, y, w, h, color, thickness, rounding, layer);
    });

    m.func(
        "draw_rect_outline_ex",
        "Outline a rectangle with rounded corners, on a layer.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("w", Num)
    .arg("h", Num)
    .arg("r", Num)
    .arg("g", Num)
    .arg("b", Num)
    .arg("a", Num)
    .arg("thickness", Num)
    .arg("rounding", Num)
    .arg("layer", Int)
    .deprecated("use draw_rect_outline(x, y, w, h, #{color, thickness, rounding, layer})")
    .run(
        |call, (x, y, w, h, r, g, b, a, thickness, rounding, layer)| {
            let color = rgba(r, g, b, a);
            let (t, rd, l) = (thickness as f32, rounding as f32, layer as i32);
            rect_outline(call, x, y, w, h, color, t, rd, l);
        },
    );

    // ── Circles ──
    m.func("draw_circle", "Fill a circle.")
        .arg("x", Num)
        .arg("y", Num)
        .arg("radius", Num)
        .arg("r", Num)
        .arg("g", Num)
        .arg("b", Num)
        .arg("a", Num)
        .run(|call, (x, y, radius, r, g, b, a)| {
            circle(call, x, y, radius, rgba(r, g, b, a), 0);
        });

    m.func("draw_circle", "Fill a circle; opts: color, layer.")
        .arg("x", Num)
        .arg("y", Num)
        .arg("radius", Num)
        .arg("opts", MapKind)
        .run(|call, (x, y, radius, map)| {
            let o = opts(call, "draw_circle", &map, FILL);
            circle(call, x, y, radius, o.color(), o.layer());
        });

    m.func("draw_circle_ex", "Fill a circle on a layer.")
        .arg("x", Num)
        .arg("y", Num)
        .arg("radius", Num)
        .arg("r", Num)
        .arg("g", Num)
        .arg("b", Num)
        .arg("a", Num)
        .arg("layer", Int)
        .deprecated("use draw_circle(x, y, radius, #{color, layer})")
        .run(|call, (x, y, radius, r, g, b, a, layer)| {
            circle(call, x, y, radius, rgba(r, g, b, a), layer as i32);
        });

    m.func(
        "draw_circle_outline",
        "Outline a circle, thickness in points.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("radius", Num)
    .arg("r", Num)
    .arg("g", Num)
    .arg("b", Num)
    .arg("a", Num)
    .arg("thickness", Num)
    .run(|call, (x, y, radius, r, g, b, a, thickness)| {
        circle_outline(call, x, y, radius, rgba(r, g, b, a), thickness as f32, 0);
    });

    m.func(
        "draw_circle_outline",
        "Outline a circle; opts: color, thickness (points, 1), layer.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("radius", Num)
    .arg("opts", MapKind)
    .run(|call, (x, y, radius, map)| {
        let o = opts(call, "draw_circle_outline", &map, STROKE);
        circle_outline(call, x, y, radius, o.color(), o.thickness(), o.layer());
    });

    m.func("draw_circle_outline_ex", "Outline a circle on a layer.")
        .arg("x", Num)
        .arg("y", Num)
        .arg("radius", Num)
        .arg("r", Num)
        .arg("g", Num)
        .arg("b", Num)
        .arg("a", Num)
        .arg("thickness", Num)
        .arg("layer", Int)
        .deprecated("use draw_circle_outline(x, y, radius, #{color, thickness, layer})")
        .run(|call, (x, y, radius, r, g, b, a, thickness, layer)| {
            let color = rgba(r, g, b, a);
            circle_outline(call, x, y, radius, color, thickness as f32, layer as i32);
        });

    // ── Lines, rings, polygons ──
    m.func(
        "draw_line",
        "Draw a line between two points, thickness in points.",
    )
    .arg("x1", Num)
    .arg("y1", Num)
    .arg("x2", Num)
    .arg("y2", Num)
    .arg("r", Num)
    .arg("g", Num)
    .arg("b", Num)
    .arg("a", Num)
    .arg("thickness", Num)
    .run(|call, (x1, y1, x2, y2, r, g, b, a, thickness)| {
        line(call, x1, y1, x2, y2, rgba(r, g, b, a), thickness as f32, 0);
    });

    m.func(
        "draw_line",
        "Draw a line between two points; opts: color, thickness (points, 1), layer.",
    )
    .arg("x1", Num)
    .arg("y1", Num)
    .arg("x2", Num)
    .arg("y2", Num)
    .arg("opts", MapKind)
    .run(|call, (x1, y1, x2, y2, map)| {
        let o = opts(call, "draw_line", &map, STROKE);
        line(call, x1, y1, x2, y2, o.color(), o.thickness(), o.layer());
    });

    m.func("draw_line_ex", "draw_line on a layer.")
        .arg("x1", Num)
        .arg("y1", Num)
        .arg("x2", Num)
        .arg("y2", Num)
        .arg("r", Num)
        .arg("g", Num)
        .arg("b", Num)
        .arg("a", Num)
        .arg("thickness", Num)
        .arg("layer", Int)
        .deprecated("use draw_line(x1, y1, x2, y2, #{color, thickness, layer})")
        .run(|call, (x1, y1, x2, y2, r, g, b, a, thickness, layer)| {
            let color = rgba(r, g, b, a);
            line(call, x1, y1, x2, y2, color, thickness as f32, layer as i32);
        });

    m.func(
        "draw_ring",
        "Fill an annular sector: degrees clockwise from 12 o'clock, end after start, r_inner 0 for a pie.",
    )
    .arg("cx", Num)
    .arg("cy", Num)
    .arg("r_inner", Num)
    .arg("r_outer", Num)
    .arg("start_deg", Num)
    .arg("end_deg", Num)
    .arg("r", Num)
    .arg("g", Num)
    .arg("b", Num)
    .arg("a", Num)
    .arg("layer", Int)
    .run(
        |call, (cx, cy, r_inner, r_outer, start_deg, end_deg, r, g, b, a, layer)| {
            let (ri, ro) = (r_inner as f32, r_outer as f32);
            let color = rgba(r, g, b, a);
            ring(call, cx, cy, ri, ro, start_deg, end_deg, color, layer as i32);
        },
    );

    m.func(
        "draw_ring",
        "Fill an annular sector (degrees clockwise from 12 o'clock); opts: color, layer.",
    )
    .arg("cx", Num)
    .arg("cy", Num)
    .arg("r_inner", Num)
    .arg("r_outer", Num)
    .arg("start_deg", Num)
    .arg("end_deg", Num)
    .arg("opts", MapKind)
    .run(
        |call, (cx, cy, r_inner, r_outer, start_deg, end_deg, map)| {
            let o = opts(call, "draw_ring", &map, FILL);
            let (ri, ro) = (r_inner as f32, r_outer as f32);
            let (color, layer) = (o.color(), o.layer());
            ring(call, cx, cy, ri, ro, start_deg, end_deg, color, layer);
        },
    );

    m.func(
        "draw_arc",
        "Stroke an arc of a circle: draw_ring centred on radius, thickness wide.",
    )
    .arg("cx", Num)
    .arg("cy", Num)
    .arg("radius", Num)
    .arg("start_deg", Num)
    .arg("end_deg", Num)
    .arg("r", Num)
    .arg("g", Num)
    .arg("b", Num)
    .arg("a", Num)
    .arg("thickness", Num)
    .arg("layer", Int)
    .run(
        |call, (cx, cy, radius, start_deg, end_deg, r, g, b, a, thickness, layer)| {
            let color = rgba(r, g, b, a);
            let (t, l) = (thickness as f32, layer as i32);
            arc(call, cx, cy, radius, start_deg, end_deg, color, t, l);
        },
    );

    m.func(
        "draw_arc",
        "Stroke an arc of a circle; opts: color, thickness (points, 1), layer.",
    )
    .arg("cx", Num)
    .arg("cy", Num)
    .arg("radius", Num)
    .arg("start_deg", Num)
    .arg("end_deg", Num)
    .arg("opts", MapKind)
    .run(|call, (cx, cy, radius, start_deg, end_deg, map)| {
        let o = opts(call, "draw_arc", &map, STROKE);
        let (color, t, l) = (o.color(), o.thickness(), o.layer());
        arc(call, cx, cy, radius, start_deg, end_deg, color, t, l);
    });

    m.func(
        "draw_polygon",
        "Fill a convex polygon: points [[x, y], ...] or flat [x0, y0, x1, y1, ...].",
    )
    .arg("points", Array)
    .arg("r", Num)
    .arg("g", Num)
    .arg("b", Num)
    .arg("a", Num)
    .arg("layer", Int)
    .run(|call, (points, r, g, b, a, layer)| {
        polygon(
            call,
            "draw_polygon",
            &points,
            rgba(r, g, b, a),
            layer as i32,
        );
    });

    m.func(
        "draw_polygon",
        "Fill a convex polygon (points [[x, y], ...] or flat); opts: color, layer.",
    )
    .arg("points", Array)
    .arg("opts", MapKind)
    .run(|call, (points, map)| {
        let o = opts(call, "draw_polygon", &map, FILL);
        polygon(call, "draw_polygon", &points, o.color(), o.layer());
    });

    m.func(
        "draw_polygon_outline",
        "Stroke a polygon's edges, closing it back to the first point when closed.",
    )
    .arg("points", Array)
    .arg("r", Num)
    .arg("g", Num)
    .arg("b", Num)
    .arg("a", Num)
    .arg("thickness", Num)
    .arg("closed", Bool)
    .arg("layer", Int)
    .run(|call, (points, r, g, b, a, thickness, closed, layer)| {
        let (color, t, l) = (rgba(r, g, b, a), thickness as f32, layer as i32);
        polygon_outline(call, "draw_polygon_outline", &points, color, t, closed, l);
    });

    m.func(
        "draw_polygon_outline",
        "Stroke a polygon's edges; opts: color, thickness (points, 1), closed (back to the first point, true), layer.",
    )
    .arg("points", Array)
    .arg("opts", MapKind)
    .run(|call, (points, map)| {
        let o = opts(call, "draw_polygon_outline", &map, POLYGON_OUTLINE);
        let (color, t, l) = (o.color(), o.thickness(), o.layer());
        let closed = o.closed.unwrap_or(true);
        polygon_outline(call, "draw_polygon_outline", &points, color, t, closed, l);
    });

    // ── Sprites ──
    m.func(
        "draw_sprite",
        "Draw a whole image (a sprite name) into a rectangle.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("w", Num)
    .arg("h", Num)
    .arg("name", Str)
    .run(|call, (x, y, w, h, name)| {
        sprite(call, x, y, w, h, &name, [0.0, 0.0, 1.0, 1.0], WHITE, 0);
    });

    m.func(
        "draw_sprite",
        "Draw an image (a sprite name) into a rectangle; opts: uv [u0, v0, u1, v1] (0..1, the whole image), color (multiplies the image), layer.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("w", Num)
    .arg("h", Num)
    .arg("name", Str)
    .arg("opts", MapKind)
    .run(|call, (x, y, w, h, name, map)| {
        let o = opts(call, "draw_sprite", &map, SPRITE);
        let uv = o.uv.unwrap_or([0.0, 0.0, 1.0, 1.0]);
        sprite(call, x, y, w, h, &name, uv, o.color(), o.layer());
    });

    m.func(
        "draw_sprite_ex",
        "Draw part of an image (uv 0..1) into a rectangle, tinted, on a layer.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("w", Num)
    .arg("h", Num)
    .arg("name", Str)
    .arg("u0", Num)
    .arg("v0", Num)
    .arg("u1", Num)
    .arg("v1", Num)
    .arg("r", Num)
    .arg("g", Num)
    .arg("b", Num)
    .arg("a", Num)
    .arg("layer", Int)
    .deprecated("use draw_sprite(x, y, w, h, name, #{uv: [u0, v0, u1, v1], color, layer})")
    .run(
        |call, (x, y, w, h, name, u0, v0, u1, v1, r, g, b, a, layer)| {
            let uv = [u0 as f32, v0 as f32, u1 as f32, v1 as f32];
            sprite(call, x, y, w, h, &name, uv, rgba(r, g, b, a), layer as i32);
        },
    );

    // ── Clip and over-UI stacks ──
    m.func(
        "push_clip",
        "Confine every draw until the matching pop_clip to a rectangle (nested pushes intersect).",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("w", Num)
    .arg("h", Num)
    .run(|call, (x, y, w, h)| {
        call.ctx()
            .push_clip([x as f32, y as f32, w as f32, h as f32]);
    });

    m.func(
        "pop_clip",
        "End the innermost push_clip; an unbalanced pop is ignored, and the stack resets each frame.",
    )
    .run(|call, ()| call.ctx().pop_clip());

    m.func(
        "push_over_ui",
        "Draw above the game's egui panels until the matching pop_over_ui (pushes nest).",
    )
    .run(|call, ()| call.ctx().push_over_ui());

    m.func(
        "pop_over_ui",
        "End the innermost push_over_ui; an unbalanced pop is ignored, and the depth resets each frame.",
    )
    .run(|call, ()| call.ctx().pop_over_ui());

    // ── The screen ──
    m.func(
        "screen_width",
        "The window's width in logical points, the draw space.",
    )
    .returns(Num, "points")
    .run(|call, ()| call.ctx_ref().screen_width as f64);

    m.func(
        "screen_height",
        "The window's height in logical points, the draw space.",
    )
    .returns(Num, "points")
    .run(|call, ()| call.ctx_ref().screen_height as f64);
}
