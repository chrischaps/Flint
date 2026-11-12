//! The naming conventions (#702): each renamed function keeps its old name
//! for one release, deprecated and doing the same thing, and each draw
//! primitive's opts form draws what its positional forms draw.

use crate::engine::ScriptEngine;

/// Every rename of #702, old name -> new.
pub(crate) const RENAMES: &[(&str, &str)] = &[
    ("this_entity", "self_entity"),
    ("log_info", "log"),
    ("lod_scale", "get_lod_scale"),
    ("draw_distance_scale", "get_draw_distance_scale"),
    ("current_scene", "get_current_scene"),
    ("current_state", "get_current_state"),
    ("state_stack", "get_state_stack"),
    ("sprite_play", "play_sprite"),
    ("sprite_stop", "stop_sprite"),
    ("sprite_set_speed", "set_sprite_speed"),
    ("sprite_is_playing", "is_sprite_playing"),
    ("camera_follow", "follow_with_camera"),
    ("camera_follow_position", "get_camera_follow_position"),
    ("camera_follow_set", "set_camera_follow_position"),
    ("camera_shake", "shake_camera"),
    ("camera_shake_stop", "stop_camera_shake"),
    ("camera_apply_shake", "apply_camera_shake"),
];

/// The deprecated forms that became an opts map on the base name.
const TO_OPTS: &[(&str, usize, &str, usize)] = &[
    ("draw_text_ex", 9, "draw_text", 5),
    ("draw_text_stroked", 13, "draw_text", 5),
    ("draw_text_stroked_ex", 14, "draw_text", 5),
    ("draw_text_opts", 5, "draw_text", 5),
    ("measure_text_ex", 4, "measure_text", 3),
    ("draw_rect_ex", 10, "draw_rect", 5),
    ("draw_rect_ex4", 13, "draw_rect", 5),
    ("draw_rect_gradient", 14, "draw_rect_gradient", 5),
    ("draw_rect_outline_ex", 11, "draw_rect_outline", 5),
    ("draw_circle_ex", 8, "draw_circle", 4),
    ("draw_circle_outline_ex", 9, "draw_circle_outline", 4),
    ("draw_line_ex", 10, "draw_line", 5),
    ("draw_sprite_ex", 14, "draw_sprite", 6),
    ("draw_line_3d_ex", 12, "draw_line_3d", 7),
    ("set_anim_layer_ex", 6, "set_anim_layer", 5),
];

#[test]
fn a_renamed_function_keeps_its_old_name_deprecated_for_one_release() {
    let engine = ScriptEngine::new();
    let reg = engine.api_registry();
    let at = |name: &str| -> Vec<_> { reg.entries().iter().filter(|e| e.name == name).collect() };
    for (old, new) in RENAMES {
        let (olds, news) = (at(old), at(new));
        assert!(!olds.is_empty(), "{old} is gone: it stays for one release");
        assert!(!news.is_empty(), "{new} isn't declared");
        for o in &olds {
            let note = o
                .deprecated
                .unwrap_or_else(|| panic!("{old} isn't deprecated"));
            assert_eq!(note, format!("use {new}"), "{old}");
            let twin = news
                .iter()
                .find(|n| n.params.len() == o.params.len())
                .unwrap_or_else(|| panic!("{new} has no arity {}", o.params.len()));
            assert!(twin.deprecated.is_none(), "{new} is deprecated");
            assert_eq!(
                (o.module, &o.params, o.returns),
                (twin.module, &twin.params, twin.returns),
                "{old} and {new} differ"
            );
        }
    }
    for (old, old_arity, new, new_arity) in TO_OPTS {
        let o = at(old)
            .into_iter()
            .find(|e| e.params.len() == *old_arity)
            .unwrap_or_else(|| panic!("{old}/{old_arity} is gone"));
        let note = o
            .deprecated
            .unwrap_or_else(|| panic!("{old} isn't deprecated"));
        assert!(note.starts_with(&format!("use {new}(")), "{old}: {note}");
        let n = at(new)
            .into_iter()
            .find(|e| e.params.len() == *new_arity)
            .unwrap_or_else(|| panic!("{new}/{new_arity} isn't declared"));
        assert!(n.deprecated.is_none(), "{new}/{new_arity} is deprecated");
        assert_eq!(n.params.last().map(|p| p.name), Some("opts"), "{new}");
    }
}

/// Every draw primitive has an opts form, and keeps its positional one.
#[test]
fn every_draw_primitive_has_an_opts_form() {
    let engine = ScriptEngine::new();
    let reg = engine.api_registry();
    for name in [
        "draw_text",
        "draw_rect",
        "draw_rect_gradient",
        "draw_rect_outline",
        "draw_circle",
        "draw_circle_outline",
        "draw_line",
        "draw_ring",
        "draw_arc",
        "draw_polygon",
        "draw_polygon_outline",
        "draw_sprite",
        "draw_line_3d",
        "draw_world_line",
        "draw_world_text",
        "measure_text",
    ] {
        let forms: Vec<_> = reg.entries().iter().filter(|e| e.name == name).collect();
        assert!(
            forms
                .iter()
                .any(|e| e.deprecated.is_none() && e.params.last().map(|p| p.kind) == Some("map")),
            "{name} has no opts form"
        );
        if name != "draw_world_text" && name != "draw_rect_gradient" {
            assert!(
                forms
                    .iter()
                    .any(|e| e.deprecated.is_none()
                        && e.params.last().map(|p| p.kind) != Some("map")),
                "{name} lost its positional form"
            );
        }
    }
}

/// The draws a script makes, as text.
fn draws(src: &str) -> Vec<String> {
    let engine = ScriptEngine::new();
    engine.rhai_engine().run(src).unwrap();
    engine
        .drain_draw_commands()
        .into_iter()
        .map(|d| format!("{:?}", d.cmd))
        .collect()
}

#[test]
fn an_opts_form_draws_what_the_old_positional_form_drew() {
    for (old, new) in [
        (
            "draw_text_ex(1, 2, \"hi\", 16, 1, 0.5, 0, 1, 3)",
            "draw_text(1, 2, \"hi\", 16, #{color: [1, 0.5, 0, 1], layer: 3})",
        ),
        (
            "draw_text_stroked_ex(1, 2, \"hi\", 16, 1, 1, 1, 1, 0, 0, 0, 1, 2, 3)",
            "draw_text(1, 2, \"hi\", 16, #{stroke: [0, 0, 0, 1, 2], layer: 3})",
        ),
        (
            "draw_text_opts(1, 2, \"hi\", 16, #{align: \"center\", spacing: 2})",
            "draw_text(1, 2, \"hi\", 16, #{align: \"center\", spacing: 2})",
        ),
        (
            "draw_rect(1, 2, 3, 4, 1, 0, 0, 1)",
            "draw_rect(1, 2, 3, 4, #{color: \"#ff0000\"})",
        ),
        (
            "draw_rect_ex(1, 2, 3, 4, 0.5, 0.5, 0.5, 1, 6, 2)",
            "draw_rect(1, 2, 3, 4, #{color: [0.5, 0.5, 0.5], rounding: 6, layer: 2})",
        ),
        (
            "draw_rect_ex4(1, 2, 3, 4, 1, 1, 1, 1, 1, 2, 3, 4, 5)",
            "draw_rect(1, 2, 3, 4, #{rounding: [1, 2, 3, 4], layer: 5})",
        ),
        (
            "draw_rect_gradient(1, 2, 3, 4, 1, 0, 0, 1, 0, 0, 1, 1, false, 2)",
            "draw_rect_gradient(1, 2, 3, 4, #{from: \"#ff0000\", to: [0, 0, 1], vertical: false, layer: 2})",
        ),
        (
            "draw_rect_outline_ex(1, 2, 3, 4, 1, 1, 1, 1, 2, 5, 1)",
            "draw_rect_outline(1, 2, 3, 4, #{thickness: 2, rounding: 5, layer: 1})",
        ),
        (
            "draw_circle_ex(1, 2, 3, 1, 1, 1, 1, 4)",
            "draw_circle(1, 2, 3, #{layer: 4})",
        ),
        (
            "draw_circle_outline_ex(1, 2, 3, 1, 1, 1, 1, 2, 4)",
            "draw_circle_outline(1, 2, 3, #{thickness: 2, layer: 4})",
        ),
        (
            "draw_line_ex(1, 2, 3, 4, 1, 1, 1, 1, 2, -1)",
            "draw_line(1, 2, 3, 4, #{thickness: 2, layer: -1})",
        ),
        (
            "draw_ring(1, 2, 3, 4, 0, 90, 1, 1, 1, 1, 2)",
            "draw_ring(1, 2, 3, 4, 0, 90, #{layer: 2})",
        ),
        (
            "draw_arc(1, 2, 3, 0, 90, 1, 1, 1, 1, 2, 4)",
            "draw_arc(1, 2, 3, 0, 90, #{thickness: 2, layer: 4})",
        ),
        (
            "draw_polygon([0, 0, 1, 0, 1, 1], 1, 1, 1, 1, 2)",
            "draw_polygon([0, 0, 1, 0, 1, 1], #{layer: 2})",
        ),
        (
            "draw_polygon_outline([0, 0, 1, 0, 1, 1], 1, 1, 1, 1, 2, true, 3)",
            "draw_polygon_outline([0, 0, 1, 0, 1, 1], #{thickness: 2, layer: 3})",
        ),
        (
            "draw_sprite_ex(1, 2, 3, 4, \"s\", 0, 0, 0.5, 0.5, 1, 1, 1, 0.5, 2)",
            "draw_sprite(1, 2, 3, 4, \"s\", #{uv: [0, 0, 0.5, 0.5], color: [1, 1, 1, 0.5], layer: 2})",
        ),
    ] {
        assert_eq!(draws(old), draws(new), "{old} vs {new}");
        assert_eq!(draws(new).len(), 1, "{new}");
    }
}

#[test]
fn a_misspelled_opts_key_is_ignored_and_the_draw_goes_ahead() {
    assert_eq!(
        draws("draw_rect(1, 2, 3, 4, #{colour: [1, 0, 0, 1], layer: 2})"),
        draws("draw_rect(1, 2, 3, 4, #{layer: 2})")
    );
}

#[test]
fn a_renamed_function_does_what_its_old_name_does() {
    let engine = ScriptEngine::new();
    engine
        .rhai_engine()
        .run("set_lod_scale(0.5); if get_lod_scale() != lod_scale() { throw \"lod\"; }")
        .unwrap();
    engine
        .rhai_engine()
        .run(
            "set_camera_follow_position(3.0, 4.0); \
             let a = get_camera_follow_position(); let b = camera_follow_position(); \
             if a.x != 3.0 || b.y != 4.0 { throw \"follow\"; }",
        )
        .unwrap();
}
