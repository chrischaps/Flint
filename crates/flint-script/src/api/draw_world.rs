//! The `draw_world` module: lines and text placed in the world (ADR 0116).
//! `draw_line_3d` projects onto the HUD with the frame's camera; the
//! `draw_world_*` calls draw in the 3D frame with a depth test, so geometry
//! in front hides them (ADRs 0083, 0104).

use super::builder::kind::{Int, Map as MapKind, Num, Str};
use super::builder::ScriptApi;
use super::text_opts::world_text_from;
use crate::context::{DrawCommand, ScriptCallContext};
use crate::projector::{clip_to_screen, project_clip};

#[inline]
fn rgba(r: f64, g: f64, b: f64, a: f64) -> [f32; 4] {
    [r as f32, g as f32, b as f32, a as f32]
}

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("draw_world");

    m.func(
        "draw_line_3d",
        "A line between two world points, projected onto the HUD and clipped at the near plane.",
    )
    .arg("x1", Num)
    .arg("y1", Num)
    .arg("z1", Num)
    .arg("x2", Num)
    .arg("y2", Num)
    .arg("z2", Num)
    .arg("r", Num)
    .arg("g", Num)
    .arg("b", Num)
    .arg("a", Num)
    .arg("thickness", Num)
    .run(|call, (x1, y1, z1, x2, y2, z2, r, g, b, a, thickness)| {
        push_line_3d(
            call.ctx(),
            [x1, y1, z1],
            [x2, y2, z2],
            rgba(r, g, b, a),
            thickness as f32,
            0,
        );
    });

    m.func("draw_line_3d_ex", "draw_line_3d on a HUD layer.")
        .arg("x1", Num)
        .arg("y1", Num)
        .arg("z1", Num)
        .arg("x2", Num)
        .arg("y2", Num)
        .arg("z2", Num)
        .arg("r", Num)
        .arg("g", Num)
        .arg("b", Num)
        .arg("a", Num)
        .arg("thickness", Num)
        .arg("layer", Int)
        .run(
            |call, (x1, y1, z1, x2, y2, z2, r, g, b, a, thickness, layer)| {
                push_line_3d(
                    call.ctx(),
                    [x1, y1, z1],
                    [x2, y2, z2],
                    rgba(r, g, b, a),
                    thickness as f32,
                    layer as i32,
                );
            },
        );

    m.func(
        "draw_world_line",
        "A line between two world points drawn in the 3D frame, depth-tested; thickness in points.",
    )
    .arg("x1", Num)
    .arg("y1", Num)
    .arg("z1", Num)
    .arg("x2", Num)
    .arg("y2", Num)
    .arg("z2", Num)
    .arg("r", Num)
    .arg("g", Num)
    .arg("b", Num)
    .arg("a", Num)
    .arg("thickness", Num)
    .run(|call, (x1, y1, z1, x2, y2, z2, r, g, b, a, thickness)| {
        call.ctx()
            .world_lines
            .push(flint_core::world_line::WorldLine {
                a: [x1 as f32, y1 as f32, z1 as f32],
                b: [x2 as f32, y2 as f32, z2 as f32],
                color: rgba(r, g, b, a),
                thickness: thickness as f32,
            });
    });

    m.func(
        "draw_world_text",
        "Text at a world point, depth-tested: draw_text_opts's options (no layer) plus valign, facing and up.",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("z", Num)
    .arg("text", Str)
    .arg("size", Num)
    .arg("opts", MapKind)
    .run(|call, (x, y, z, text, size, opts)| {
        let t = world_text_from([x as f32, y as f32, z as f32], &text, size as f32, &opts);
        call.ctx().world_texts.push(t);
    });
}

/// Project a world-space segment with the frame's camera and record it as
/// a HUD line. Clipped against the near plane so a segment that passes the
/// camera still draws; fully-behind segments are dropped.
fn push_line_3d(
    c: &mut ScriptCallContext,
    a: [f64; 3],
    b: [f64; 3],
    color: [f32; 4],
    thickness: f32,
    layer: i32,
) {
    let mut p = project_clip(&c.camera_view_proj, [a[0] as f32, a[1] as f32, a[2] as f32]);
    let mut q = project_clip(&c.camera_view_proj, [b[0] as f32, b[1] as f32, b[2] as f32]);
    const NEAR_W: f32 = 1e-3;
    if p[3] <= NEAR_W && q[3] <= NEAR_W {
        return;
    }
    if p[3] <= NEAR_W || q[3] <= NEAR_W {
        // Move the behind-camera end onto the near plane (linear in clip space).
        let t = (NEAR_W - p[3]) / (q[3] - p[3]);
        let m = [
            p[0] + (q[0] - p[0]) * t,
            p[1] + (q[1] - p[1]) * t,
            p[2] + (q[2] - p[2]) * t,
            NEAR_W,
        ];
        if p[3] <= NEAR_W {
            p = m;
        } else {
            q = m;
        }
    }
    let (sx1, sy1) = clip_to_screen(p, c.screen_width, c.screen_height);
    let (sx2, sy2) = clip_to_screen(q, c.screen_width, c.screen_height);
    c.push_draw(DrawCommand::Line {
        x1: sx1,
        y1: sy1,
        x2: sx2,
        y2: sy2,
        color,
        thickness,
        layer,
    });
}
