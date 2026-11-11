//! The `ui` module: data-driven UI documents (`load_ui`), their elements'
//! text, visibility, colours, styles and classes, and their style tokens
//! (ADR 0116). Elements are named by their id in the layout; an id no
//! loaded document has is ignored by the setters.

use super::builder::kind::{Any, Array as ArrayKind, Bool, Int, Map as MapKind, Num, Str};
use super::builder::ScriptApi;
use super::text_opts::array_to_f32s;
use crate::ui::element::StyleValue;
use rhai::{Dynamic, Map};

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("ui");

    m.func(
        "load_ui",
        "Load a UI layout document, a path from the project root.",
    )
    .arg("layout_path", Str)
    .returns(Int, "a handle for unload_ui")
    .run(|call, (path,)| {
        let c = call.ctx();
        // A path from the project root (ADR 0114)
        let root = c.project_root_holding("load_ui", &path);
        c.ui_system.load(&path, &root)
    });

    m.func("unload_ui", "Unload a UI document load_ui returned.")
        .arg("handle", Int)
        .run(|call, (handle,)| call.ctx().ui_system.unload(handle));

    m.func("ui_set_text", "Set an element's text.")
        .arg("element_id", Str)
        .arg("text", Str)
        .run(|call, (id, text)| call.ctx().ui_system.set_text(&id, &text));

    m.func("ui_show", "Show an element.")
        .arg("element_id", Str)
        .run(|call, (id,)| call.ctx().ui_system.show(&id));

    m.func("ui_hide", "Hide an element.")
        .arg("element_id", Str)
        .run(|call, (id,)| call.ctx().ui_system.hide(&id));

    m.func("ui_set_visible", "Show or hide an element.")
        .arg("element_id", Str)
        .arg("visible", Bool)
        .run(|call, (id, visible)| call.ctx().ui_system.set_visible(&id, visible));

    m.func("ui_set_color", "Set an element's text colour, 0..1.")
        .arg("element_id", Str)
        .arg("r", Num)
        .arg("g", Num)
        .arg("b", Num)
        .arg("a", Num)
        .run(|call, (id, r, g, b, a)| {
            call.ctx()
                .ui_system
                .set_color(&id, r as f32, g as f32, b as f32, a as f32);
        });

    m.func(
        "ui_set_bg_color",
        "Set an element's background colour, 0..1.",
    )
    .arg("element_id", Str)
    .arg("r", Num)
    .arg("g", Num)
    .arg("b", Num)
    .arg("a", Num)
    .run(|call, (id, r, g, b, a)| {
        call.ctx()
            .ui_system
            .set_bg_color(&id, r as f32, g as f32, b as f32, a as f32);
    });

    m.func(
        "ui_set_style",
        "Set one style property: a number, a string, a bool, an array of 3, 4 or 6 numbers, or a \"$token\".",
    )
    .arg("element_id", Str)
    .arg("prop", Str)
    .arg("value", Any)
    .run(|call, (id, prop, val)| {
        let style_val = if val.is_float() {
            StyleValue::Float(val.as_float().unwrap_or(0.0) as f32)
        } else if val.is_int() {
            StyleValue::Float(val.as_int().unwrap_or(0) as f32)
        } else if val.is_string() {
            StyleValue::String(val.into_string().unwrap_or_default())
        } else if val.is_bool() {
            StyleValue::Bool(val.as_bool().unwrap_or(false))
        } else if val.is_array() {
            let arr = val.cast::<rhai::Array>();
            match StyleValue::from_numbers(&array_to_f32s(&arr)) {
                Some(sv) => sv,
                None => {
                    tracing::warn!(
                        "ui_set_style({id}, {prop}): expected 3, 4 or 6 numbers, got {}",
                        arr.len()
                    );
                    return;
                }
            }
        } else {
            tracing::warn!(
                "ui_set_style({id}, {prop}): unsupported value type {}",
                val.type_name()
            );
            return;
        };
        call.ctx().ui_system.set_style(&id, &prop, style_val);
    });

    m.func(
        "ui_set_style_array",
        "Set an array style property: 4 numbers (color, padding, uv) or 6 (shadow: dx, dy, r, g, b, a).",
    )
    .arg("element_id", Str)
    .arg("prop", Str)
    .arg("value", ArrayKind)
    .run(|call, (id, prop, val)| {
        let Some(style_val) = StyleValue::from_numbers(&array_to_f32s(&val)) else {
            tracing::warn!(
                "ui_set_style_array({id}, {prop}): expected 4 or 6 numbers, got {}",
                val.len()
            );
            return;
        };
        call.ctx().ui_system.set_style(&id, &prop, style_val);
    });

    m.func(
        "ui_reset_style",
        "Drop every style a script set on an element.",
    )
    .arg("element_id", Str)
    .run(|call, (id,)| call.ctx().ui_system.reset_style(&id));

    m.func("ui_set_class", "Set an element's style class.")
        .arg("element_id", Str)
        .arg("class", Str)
        .run(|call, (id, class)| call.ctx().ui_system.set_class(&id, &class));

    m.func(
        "ui_exists",
        "Whether a loaded document has an element with this id.",
    )
    .arg("element_id", Str)
    .returns(Bool, "")
    .run(|call, (id,)| call.ctx_ref().ui_system.exists(&id));

    m.func(
        "ui_hit",
        "Whether a screen point is inside an element's laid-out rectangle.",
    )
    .arg("element_id", Str)
    .arg("x", Num)
    .arg("y", Num)
    .returns(Bool, "false for an element no document has")
    .run(|call, (id, x, y)| {
        let c = call.ctx();
        let (sw, sh) = (c.screen_width, c.screen_height);
        c.ui_system.hit(&id, x as f32, y as f32, sw, sh)
    });

    m.func(
        "ui_token",
        "A style token from the loaded documents' [tokens] and the shared token file (\"accent\", \"$accent\", \"color.accent\").",
    )
    .arg("name", Str)
    .returns(
        Any,
        "a float, an array of floats, a string or a bool; () for no such token",
    )
    .run(|call, (name,)| match call.ctx_ref().ui_system.token(&name) {
        Some(v) => toml_token_to_dynamic(&v),
        None => Dynamic::UNIT,
    });

    m.func("ui_get_rect", "An element's laid-out rectangle, in points.")
        .arg("element_id", Str)
        .returns(
            MapKind,
            "#{x, y, w, h}, or () for an element no document has",
        )
        .run(|call, (id,)| {
            let c = call.ctx();
            let (sw, sh) = (c.screen_width, c.screen_height);
            match c.ui_system.get_rect(&id, sw, sh) {
                Some((x, y, w, h)) => {
                    let mut map = Map::new();
                    map.insert("x".into(), Dynamic::from(x as f64));
                    map.insert("y".into(), Dynamic::from(y as f64));
                    map.insert("w".into(), Dynamic::from(w as f64));
                    map.insert("h".into(), Dynamic::from(h as f64));
                    Dynamic::from(map)
                }
                None => Dynamic::UNIT,
            }
        });
}

/// Convert a token value to what a script expects: numbers → FLOAT,
/// numeric arrays → Array of FLOAT, strings/bools as-is, anything else `()`.
fn toml_token_to_dynamic(v: &toml::Value) -> Dynamic {
    match v {
        toml::Value::Float(f) => Dynamic::from(*f),
        toml::Value::Integer(i) => Dynamic::from(*i as f64),
        toml::Value::String(s) => Dynamic::from(s.clone()),
        toml::Value::Boolean(b) => Dynamic::from(*b),
        toml::Value::Array(arr) => {
            let out: rhai::Array = arr
                .iter()
                .map(|x| match x {
                    toml::Value::Float(f) => Dynamic::from(*f),
                    toml::Value::Integer(i) => Dynamic::from(*i as f64),
                    toml::Value::String(s) => Dynamic::from(s.clone()),
                    toml::Value::Boolean(b) => Dynamic::from(*b),
                    _ => Dynamic::UNIT,
                })
                .collect();
            Dynamic::from(out)
        }
        _ => Dynamic::UNIT,
    }
}
