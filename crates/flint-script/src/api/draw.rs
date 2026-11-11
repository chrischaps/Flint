//! The `draw` module: the immediate-mode 2D draw calls a script's
//! `on_draw_ui` makes (text, rectangles, circles, lines, rings, polygons,
//! sprites), the clip and over-UI stacks, the screen size and text
//! measurement (ADR 0116). Coordinates are logical points from the top
//! left, colours 0..1, and `layer` orders draws (higher on top).
//!
//! These are the hottest functions in the API: a HUD makes hundreds of
//! them a frame. They go through `.run()`, whose exact-typed twin is as
//! fast as a typed closure for float calls and whose all-int copy
//! (functions with four or more `Num` parameters) is as fast for int
//! calls (docs/design/script-api-overhead.md).

use super::builder::kind::{Array, Bool, Int, Map as MapKind, Num, Str};
use super::builder::{Call, ScriptApi};
use super::text_opts::{measure_text_with, TextOpts};
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

/// A text draw with no options: default font, left-aligned.
#[allow(clippy::too_many_arguments)]
fn text(
    call: &mut Call<'_>,
    x: f64,
    y: f64,
    text: &str,
    size: f64,
    color: [f32; 4],
    layer: i64,
    stroke: Option<([f32; 4], f32)>,
) {
    push(
        call,
        DrawCommand::Text {
            x: x as f32,
            y: y as f32,
            text: text.to_string(),
            size: size as f32,
            color,
            layer: layer as i32,
            align: 0,
            stroke,
            font: None,
            letter_spacing: 0.0,
            shadow: None,
            glow: None,
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
        text(call, x, y, &t, size, rgba(r, g, b, a), 0, None);
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
        .run(|call, (x, y, t, size, r, g, b, a, layer)| {
            text(call, x, y, &t, size, rgba(r, g, b, a), layer, None);
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
    .run(|call, (x, y, t, size, r, g, b, a, sr, sg, sb, sa, sw)| {
        let stroke = Some((rgba(sr, sg, sb, sa), sw as f32));
        text(call, x, y, &t, size, rgba(r, g, b, a), 0, stroke);
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
        .run(
            |call, (x, y, t, size, r, g, b, a, sr, sg, sb, sa, sw, layer)| {
                let stroke = Some((rgba(sr, sg, sb, sa), sw as f32));
                text(call, x, y, &t, size, rgba(r, g, b, a), layer, stroke);
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
    .run(|call, (x, y, t, size, opts)| {
        let o = TextOpts::from_map(&opts);
        push(
            call,
            DrawCommand::Text {
                x: x as f32,
                y: y as f32,
                text: t.to_string(),
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
        "measure_text_ex",
        "The size text would draw at in a font (\"\" for the default) with extra letter spacing.",
    )
    .arg("text", Str)
    .arg("size", Num)
    .arg("font", Str)
    .arg("spacing", Num)
    .returns(MapKind, "#{width, height} in points")
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
            push(
                call,
                DrawCommand::RectFilled {
                    x: x as f32,
                    y: y as f32,
                    w: w as f32,
                    h: h as f32,
                    color: rgba(r, g, b, a),
                    rounding: 0.0,
                    layer: 0,
                },
            );
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
    .run(|call, (x, y, w, h, r, g, b, a, rounding, layer)| {
        push(
            call,
            DrawCommand::RectFilled {
                x: x as f32,
                y: y as f32,
                w: w as f32,
                h: h as f32,
                color: rgba(r, g, b, a),
                rounding: rounding as f32,
                layer: layer as i32,
            },
        );
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
    .run(|call, (x, y, w, h, r, g, b, a, tl, tr, br, bl, layer)| {
        push(
            call,
            DrawCommand::RectRounded4 {
                x: x as f32,
                y: y as f32,
                w: w as f32,
                h: h as f32,
                color: rgba(r, g, b, a),
                rounding: [tl as f32, tr as f32, br as f32, bl as f32],
                layer: layer as i32,
            },
        );
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
    .run(
        |call, (x, y, w, h, r1, g1, b1, a1, r2, g2, b2, a2, vertical, layer)| {
            push(
                call,
                DrawCommand::RectGradient {
                    x: x as f32,
                    y: y as f32,
                    w: w as f32,
                    h: h as f32,
                    color_a: rgba(r1, g1, b1, a1),
                    color_b: rgba(r2, g2, b2, a2),
                    vertical,
                    layer: layer as i32,
                },
            );
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
        push(
            call,
            DrawCommand::RectOutline {
                x: x as f32,
                y: y as f32,
                w: w as f32,
                h: h as f32,
                color: rgba(r, g, b, a),
                thickness: thickness as f32,
                rounding: 0.0,
                layer: 0,
            },
        );
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
    .run(
        |call, (x, y, w, h, r, g, b, a, thickness, rounding, layer)| {
            push(
                call,
                DrawCommand::RectOutline {
                    x: x as f32,
                    y: y as f32,
                    w: w as f32,
                    h: h as f32,
                    color: rgba(r, g, b, a),
                    thickness: thickness as f32,
                    rounding: rounding as f32,
                    layer: layer as i32,
                },
            );
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
            push(
                call,
                DrawCommand::CircleFilled {
                    x: x as f32,
                    y: y as f32,
                    radius: radius as f32,
                    color: rgba(r, g, b, a),
                    layer: 0,
                },
            );
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
        .run(|call, (x, y, radius, r, g, b, a, layer)| {
            push(
                call,
                DrawCommand::CircleFilled {
                    x: x as f32,
                    y: y as f32,
                    radius: radius as f32,
                    color: rgba(r, g, b, a),
                    layer: layer as i32,
                },
            );
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
        push(
            call,
            DrawCommand::CircleOutline {
                x: x as f32,
                y: y as f32,
                radius: radius as f32,
                color: rgba(r, g, b, a),
                thickness: thickness as f32,
                layer: 0,
            },
        );
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
        .run(|call, (x, y, radius, r, g, b, a, thickness, layer)| {
            push(
                call,
                DrawCommand::CircleOutline {
                    x: x as f32,
                    y: y as f32,
                    radius: radius as f32,
                    color: rgba(r, g, b, a),
                    thickness: thickness as f32,
                    layer: layer as i32,
                },
            );
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
        push(
            call,
            DrawCommand::Line {
                x1: x1 as f32,
                y1: y1 as f32,
                x2: x2 as f32,
                y2: y2 as f32,
                color: rgba(r, g, b, a),
                thickness: thickness as f32,
                layer: 0,
            },
        );
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
        .run(|call, (x1, y1, x2, y2, r, g, b, a, thickness, layer)| {
            push(
                call,
                DrawCommand::Line {
                    x1: x1 as f32,
                    y1: y1 as f32,
                    x2: x2 as f32,
                    y2: y2 as f32,
                    color: rgba(r, g, b, a),
                    thickness: thickness as f32,
                    layer: layer as i32,
                },
            );
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
            push(
                call,
                DrawCommand::Ring {
                    cx: cx as f32,
                    cy: cy as f32,
                    r_inner: r_inner as f32,
                    r_outer: r_outer as f32,
                    start_deg: start_deg as f32,
                    end_deg: end_deg as f32,
                    color: rgba(r, g, b, a),
                    layer: layer as i32,
                },
            );
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
            let half = (thickness * 0.5) as f32;
            push(
                call,
                DrawCommand::Ring {
                    cx: cx as f32,
                    cy: cy as f32,
                    r_inner: (radius as f32 - half).max(0.0),
                    r_outer: radius as f32 + half,
                    start_deg: start_deg as f32,
                    end_deg: end_deg as f32,
                    color: rgba(r, g, b, a),
                    layer: layer as i32,
                },
            );
        },
    );

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
        let Some(points) = polygon_points("draw_polygon", &points) else {
            return;
        };
        if points.len() < 3 {
            return;
        }
        push(
            call,
            DrawCommand::Polygon {
                points,
                color: rgba(r, g, b, a),
                layer: layer as i32,
            },
        );
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
        let Some(points) = polygon_points("draw_polygon_outline", &points) else {
            return;
        };
        if points.len() < 2 {
            return;
        }
        push(
            call,
            DrawCommand::PolygonOutline {
                points,
                color: rgba(r, g, b, a),
                thickness: thickness as f32,
                closed,
                layer: layer as i32,
            },
        );
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
        push(
            call,
            DrawCommand::Sprite {
                x: x as f32,
                y: y as f32,
                w: w as f32,
                h: h as f32,
                name: name.to_string(),
                uv: [0.0, 0.0, 1.0, 1.0],
                tint: [1.0, 1.0, 1.0, 1.0],
                layer: 0,
            },
        );
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
    .run(
        |call, (x, y, w, h, name, u0, v0, u1, v1, r, g, b, a, layer)| {
            push(
                call,
                DrawCommand::Sprite {
                    x: x as f32,
                    y: y as f32,
                    w: w as f32,
                    h: h as f32,
                    name: name.to_string(),
                    uv: [u0 as f32, v0 as f32, u1 as f32, v1 as f32],
                    tint: rgba(r, g, b, a),
                    layer: layer as i32,
                },
            );
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
