//! The `style_field` module (#859): the screen field a style's blend can
//! follow (`blend_source = "field"` on a style component or `style_mask`).
//! ADR 0116.
//!
//! Immediate mode, like the draw calls: a frame's field is the shapes the
//! scripts drew that frame, in `on_update` or `on_draw_ui`, and a frame
//! that draws none has no field, so a prop following it draws as at blend
//! 0. Places and lengths are in logical points, the draw space
//! (`screen_width` / `screen_height`); angles in degrees clockwise from 12
//! o'clock, as `draw_ring`'s. Each shape is full inside and falls to 0
//! `soft` points past its edge; the field at a pixel is its strongest
//! shape's there, times the shape's `strength` (0..1, 1 when not given).

use super::builder::kind::Num;
use super::builder::ScriptApi;
use crate::context::{LogLevel, ScriptCallContext, ScriptCommand};
use flint_core::style_field::{FieldShape, MAX_FIELD_SHAPES};

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("style_field");

    for strength in [false, true] {
        let f = m.func(
            "field_disc",
            "Fill a disc of the style field this frame: a style following the field shows inside it.",
        )
        .arg("x", Num)
        .arg("y", Num)
        .arg("radius", Num)
        .arg("soft", Num);
        if strength {
            f.arg("strength", Num).run(|call, (x, y, r, soft, s)| {
                push_disc(call.ctx(), [x, y], r, soft, s);
            });
        } else {
            f.run(|call, (x, y, r, soft)| push_disc(call.ctx(), [x, y], r, soft, 1.0));
        }
    }

    for strength in [false, true] {
        let f = m.func(
            "field_ring",
            "Fill a ring of the style field this frame, width points across: a shockwave's front that turns what it crosses.",
        )
        .arg("x", Num)
        .arg("y", Num)
        .arg("radius", Num)
        .arg("width", Num)
        .arg("soft", Num);
        if strength {
            f.arg("strength", Num).run(|call, (x, y, r, w, soft, s)| {
                push_ring(call.ctx(), [x, y], r, w, soft, s);
            });
        } else {
            f.run(|call, (x, y, r, w, soft)| push_ring(call.ctx(), [x, y], r, w, soft, 1.0));
        }
    }

    for strength in [false, true] {
        let f = m.func(
            "field_cone",
            "Fill a cone of the style field this frame from (x, y), pointing direction degrees clockwise from up and spread degrees either side: a flashlight's beam.",
        )
        .arg("x", Num)
        .arg("y", Num)
        .arg("direction", Num)
        .arg("spread", Num)
        .arg("length", Num)
        .arg("soft", Num);
        if strength {
            f.arg("strength", Num)
                .run(|call, (x, y, dir, spread, len, soft, s)| {
                    push_cone(call.ctx(), [x, y], [dir, spread], len, soft, s);
                });
        } else {
            f.run(|call, (x, y, dir, spread, len, soft)| {
                push_cone(call.ctx(), [x, y], [dir, spread], len, soft, 1.0);
            });
        }
    }
}

/// The screen in uv and lengths as a fraction of its height, from points.
struct Screen {
    w: f32,
    h: f32,
}

impl Screen {
    fn of(c: &ScriptCallContext) -> Self {
        Self {
            w: c.screen_width.max(1.0),
            h: c.screen_height.max(1.0),
        }
    }

    fn uv(&self, p: [f64; 2]) -> [f32; 2] {
        [p[0] as f32 / self.w, p[1] as f32 / self.h]
    }

    /// A length in points, as a fraction of the height; never negative.
    fn len(&self, points: f64) -> f32 {
        (points as f32 / self.h).max(0.0)
    }
}

fn push_disc(c: &mut ScriptCallContext, at: [f64; 2], radius: f64, soft: f64, strength: f64) {
    let s = Screen::of(c);
    push(
        c,
        FieldShape::Disc {
            center: s.uv(at),
            radius: s.len(radius),
            soft: s.len(soft),
            strength: unit(strength),
        },
    );
}

fn push_ring(
    c: &mut ScriptCallContext,
    at: [f64; 2],
    radius: f64,
    width: f64,
    soft: f64,
    strength: f64,
) {
    let s = Screen::of(c);
    push(
        c,
        FieldShape::Ring {
            center: s.uv(at),
            radius: s.len(radius),
            width: s.len(width),
            soft: s.len(soft),
            strength: unit(strength),
        },
    );
}

fn push_cone(
    c: &mut ScriptCallContext,
    at: [f64; 2],
    [direction, spread]: [f64; 2],
    length: f64,
    soft: f64,
    strength: f64,
) {
    let s = Screen::of(c);
    push(
        c,
        FieldShape::Cone {
            apex: s.uv(at),
            direction: (direction as f32).to_radians(),
            spread: (spread as f32).clamp(0.0, 180.0).to_radians(),
            length: s.len(length),
            soft: s.len(soft),
            strength: unit(strength),
        },
    );
}

/// A strength in 0..1; NaN is none.
fn unit(v: f64) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(0.0, 1.0) as f32
    }
}

/// Add a shape to the frame's field, saying once a run when a frame asks
/// for more than the field holds.
fn push(c: &mut ScriptCallContext, shape: FieldShape) {
    let before = c.style_field.dropped();
    c.style_field.push(shape);
    if c.style_field.dropped() > before && !c.style_field_warned {
        c.style_field_warned = true;
        c.commands.push(ScriptCommand::Log {
            level: LogLevel::Warn,
            message: format!(
                "the style field holds {MAX_FIELD_SHAPES} shapes a frame; the rest are left out"
            ),
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::test_support::scripted;
    use crate::engine::ScriptEngine;
    use flint_core::style_field::{FieldShape, StyleField, MAX_FIELD_SHAPES};

    /// `source`'s field after two frames of `on_update` on a 1000 x 500
    /// point screen: the second frame's alone.
    fn run(source: &str) -> (StyleField, ScriptEngine) {
        let mut engine = ScriptEngine::new();
        let mut world = flint_ecs::FlintWorld::new();
        let id = scripted(&mut world, "caster", "field.rhai");
        {
            let mut c = crate::lock_or_recover(&engine.ctx);
            c.screen_width = 1000.0;
            c.screen_height = 500.0;
        }
        let ast = engine.compile(source).unwrap();
        engine.add_script(id, ast, "field.rhai".into());
        engine.call_updates(&mut world);
        let first = engine.drain_style_field();
        assert!(!first.is_empty(), "the first frame drew a field");
        engine.call_updates(&mut world);
        (engine.drain_style_field(), engine)
    }

    #[test]
    fn shapes_in_points_are_shapes_in_uv_and_heights() {
        let (field, _) = run("fn on_update() {
                field_ring(500, 250, 100, 20, 10);
                field_disc(250.0, 125.0, 50.0, 0.0, 0.5);
                field_cone(500.0, 500.0, 90.0, 400.0, 250.0, 5.0);
            }");
        assert_eq!(
            field.shapes(),
            [
                FieldShape::Ring {
                    center: [0.5, 0.5],
                    radius: 0.2,
                    width: 0.04,
                    soft: 0.02,
                    strength: 1.0,
                },
                FieldShape::Disc {
                    center: [0.25, 0.25],
                    radius: 0.1,
                    soft: 0.0,
                    strength: 0.5,
                },
                FieldShape::Cone {
                    apex: [0.5, 1.0],
                    direction: 90f32.to_radians(),
                    // A spread past half a turn is the whole turn
                    spread: 180f32.to_radians(),
                    length: 0.5,
                    soft: 0.01,
                    strength: 1.0,
                },
            ]
        );
    }

    /// Immediate mode: a frame that draws nothing has no field, and a
    /// frame asking for more than the cap keeps the first and warns once.
    #[test]
    fn a_frame_is_its_own_field_and_holds_the_cap() {
        let (field, _) = run("let frame = 0;
            fn on_update() {
                frame += 1;
                if frame == 1 { field_disc(10.0, 10.0, 5.0, 1.0); }
            }");
        assert!(field.is_empty(), "the second frame drew none: {field:?}");

        let (field, engine) = run("fn on_update() { for i in 0..20 { field_disc(i, 10, 5, 1); } }");
        assert_eq!(field.shapes().len(), MAX_FIELD_SHAPES);
        let warnings = engine
            .drain_commands()
            .into_iter()
            .filter(|c| matches!(c, crate::context::ScriptCommand::Log { message, .. } if message.contains("style field")))
            .count();
        assert_eq!(warnings, 1, "said once, not every frame");
    }
}
