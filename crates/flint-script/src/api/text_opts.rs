//! Helpers the draw modules share: reading numbers out of script values,
//! `draw_text_opts`'s option map, `draw_world_text`'s, and text
//! measurement. Not a module of functions.

use crate::context::ScriptCallContext;
use rhai::{Array, Dynamic, ImmutableString, Map};

/// Read a Rhai number (INT or FLOAT) as f32; anything else is `None`.
pub(crate) fn dyn_to_f32(d: &Dynamic) -> Option<f32> {
    if let Some(f) = d.clone().try_cast::<f64>() {
        Some(f as f32)
    } else {
        d.clone().try_cast::<i64>().map(|i| i as f32)
    }
}

/// Convert a Rhai array of numbers (INT or FLOAT mixed) to f32s, skipping non-numbers.
pub(crate) fn array_to_f32s(arr: &Array) -> Vec<f32> {
    arr.iter().filter_map(dyn_to_f32).collect()
}

/// Parsed `draw_text_opts` option map. Every key is optional; defaults are
/// white, left-aligned, layer 0, no stroke/shadow, default font.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TextOpts {
    pub color: [f32; 4],
    pub align: u8,
    pub layer: i32,
    pub stroke: Option<([f32; 4], f32)>,
    pub font: Option<String>,
    pub spacing: f32,
    pub shadow: Option<([f32; 4], f32, f32)>,
    /// A blurred halo under the stroke: colour and reach in points (#626).
    pub glow: Option<([f32; 4], f32)>,
}

impl Default for TextOpts {
    fn default() -> Self {
        Self {
            color: [1.0, 1.0, 1.0, 1.0],
            align: 0,
            layer: 0,
            stroke: None,
            font: None,
            spacing: 0.0,
            shadow: None,
            glow: None,
        }
    }
}

impl TextOpts {
    pub(crate) fn from_map(opts: &Map) -> Self {
        let mut o = Self::default();
        if let Some(v) = opts.get("font") {
            if let Some(s) = v.clone().try_cast::<ImmutableString>() {
                if !s.is_empty() {
                    o.font = Some(s.to_string());
                }
            }
        }
        if let Some(v) = opts.get("color") {
            if let Some(arr) = v.clone().try_cast::<Array>() {
                if let [r, g, b, a] = array_to_f32s(&arr)[..] {
                    o.color = [r, g, b, a];
                }
            }
        }
        if let Some(v) = opts.get("align") {
            if let Some(s) = v.clone().try_cast::<ImmutableString>() {
                o.align = match s.as_str() {
                    "center" => 1,
                    "right" => 2,
                    _ => 0,
                };
            } else if let Some(i) = dyn_to_f32(v) {
                o.align = (i as i64).clamp(0, 2) as u8;
            }
        }
        if let Some(v) = opts.get("stroke") {
            if let Some(arr) = v.clone().try_cast::<Array>() {
                if let [r, g, b, a, w] = array_to_f32s(&arr)[..] {
                    o.stroke = Some(([r, g, b, a], w));
                }
            }
        }
        if let Some(v) = opts.get("spacing") {
            if let Some(f) = dyn_to_f32(v) {
                o.spacing = f;
            }
        }
        if let Some(v) = opts.get("layer") {
            if let Some(f) = dyn_to_f32(v) {
                o.layer = f as i32;
            }
        }
        if let Some(v) = opts.get("shadow") {
            if let Some(arr) = v.clone().try_cast::<Array>() {
                if let [dx, dy, r, g, b, a] = array_to_f32s(&arr)[..] {
                    o.shadow = Some(([r, g, b, a], dx, dy));
                }
            }
        }
        if let Some(v) = opts.get("glow") {
            if let Some(arr) = v.clone().try_cast::<Array>() {
                if let [r, g, b, a, radius] = array_to_f32s(&arr)[..] {
                    o.glow = Some(([r, g, b, a], radius));
                }
            }
        }
        o
    }
}

/// A `draw_world_text` call as the host draws it: `draw_text_opts`'s
/// options, centred and middled on the point unless `align` or `valign`
/// say otherwise, and on a plane when `facing` is a usable normal.
pub(crate) fn world_text_from(
    at: [f32; 3],
    text: &str,
    size: f32,
    opts: &Map,
) -> flint_core::world_text::WorldText {
    let o = TextOpts::from_map(opts);
    let align = if opts.contains_key("align") {
        o.align
    } else {
        1
    };
    let valign = match opts.get("valign") {
        Some(v) => match v.clone().try_cast::<ImmutableString>() {
            Some(s) => match s.as_str() {
                "top" => 0,
                "bottom" => 2,
                _ => 1,
            },
            None => dyn_to_f32(v).map_or(1, |i| (i as i64).clamp(0, 2) as u8),
        },
        None => 1,
    };
    let vec3 = |key: &str| -> Option<[f32; 3]> {
        let arr = opts.get(key)?.clone().try_cast::<Array>()?;
        match array_to_f32s(&arr)[..] {
            [x, y, z] => Some([x, y, z]),
            _ => None,
        }
    };
    let plane = vec3("facing")
        .filter(|n| n.iter().any(|c| *c != 0.0))
        .map(|normal| flint_core::world_text::TextPlane {
            normal,
            up: vec3("up").unwrap_or([0.0, 1.0, 0.0]),
        });
    flint_core::world_text::WorldText {
        at,
        text: text.to_string(),
        size,
        color: o.color,
        align,
        valign,
        stroke: o.stroke,
        font: o.font,
        spacing: o.spacing,
        shadow: o.shadow,
        glow: o.glow,
        plane,
    }
}

/// Measure text with the host's installed measurer, falling back to the
/// glyph-count estimate (0.6 em per char) when none is installed.
pub(crate) fn measure_text_with(
    c: &ScriptCallContext,
    text: &str,
    size: f32,
    font: Option<&str>,
    spacing: f32,
) -> (f32, f32) {
    match &c.text_measurer {
        Some(m) => m(text, size, font, spacing),
        None => {
            let chars = text.chars().count() as f32;
            (chars * size * 0.6 + spacing * (chars - 1.0).max(0.0), size)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn arr(vals: &[Dynamic]) -> Dynamic {
        Dynamic::from(vals.to_vec())
    }

    #[test]
    fn empty_opts_are_white_left_layer0() {
        let o = TextOpts::from_map(&Map::new());
        assert_eq!(o, TextOpts::default());
        assert_eq!(o.color, [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn parses_every_key_with_mixed_int_and_float() {
        let mut m = Map::new();
        m.insert("font".into(), Dynamic::from("display"));
        m.insert(
            "color".into(),
            arr(&[
                Dynamic::from(1_i64),
                Dynamic::from(0.5_f64),
                Dynamic::from(0_i64),
                Dynamic::from(1.0_f64),
            ]),
        );
        m.insert("align".into(), Dynamic::from("center"));
        m.insert(
            "stroke".into(),
            arr(&[
                Dynamic::from(0_i64),
                Dynamic::from(0_i64),
                Dynamic::from(0_i64),
                Dynamic::from(1_i64),
                Dynamic::from(2.0_f64),
            ]),
        );
        m.insert("spacing".into(), Dynamic::from(1.5_f64));
        m.insert("layer".into(), Dynamic::from(3_i64));
        m.insert(
            "shadow".into(),
            arr(&[
                Dynamic::from(2_i64),
                Dynamic::from(3.0_f64),
                Dynamic::from(0.0_f64),
                Dynamic::from(0.0_f64),
                Dynamic::from(0.0_f64),
                Dynamic::from(0.6_f64),
            ]),
        );
        m.insert(
            "glow".into(),
            arr(&[
                Dynamic::from(1_i64),
                Dynamic::from(0.2_f64),
                Dynamic::from(0.6_f64),
                Dynamic::from(0.9_f64),
                Dynamic::from(18_i64),
            ]),
        );
        let o = TextOpts::from_map(&m);
        assert_eq!(o.glow, Some(([1.0, 0.2, 0.6, 0.9], 18.0)));
        assert_eq!(world_text_from([0.0; 3], "x", 1.0, &m).glow, o.glow);
        assert_eq!(o.font.as_deref(), Some("display"));
        assert_eq!(o.color, [1.0, 0.5, 0.0, 1.0]);
        assert_eq!(o.align, 1);
        assert_eq!(o.stroke, Some(([0.0, 0.0, 0.0, 1.0], 2.0)));
        assert_eq!(o.spacing, 1.5);
        assert_eq!(o.layer, 3);
        assert_eq!(o.shadow, Some(([0.0, 0.0, 0.0, 0.6], 2.0, 3.0)));
    }

    #[test]
    fn malformed_arrays_and_empty_font_are_ignored() {
        let mut m = Map::new();
        m.insert("font".into(), Dynamic::from(""));
        m.insert("color".into(), arr(&[Dynamic::from(1.0_f64)]));
        m.insert("align".into(), Dynamic::from("right"));
        m.insert("stroke".into(), Dynamic::from("nope"));
        m.insert("glow".into(), arr(&vec![Dynamic::from(1.0_f64); 4]));
        let o = TextOpts::from_map(&m);
        assert_eq!(o.glow, None, "a glow needs five numbers");
        assert_eq!(o.font, None);
        assert_eq!(o.color, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(o.align, 2);
        assert_eq!(o.stroke, None);
    }

    #[test]
    fn world_text_is_a_centred_billboard_by_default() {
        let t = world_text_from([1.0, 2.0, 3.0], "Ada's", 14.0, &Map::new());
        assert_eq!(t.at, [1.0, 2.0, 3.0]);
        assert_eq!(t.text, "Ada's");
        assert_eq!((t.align, t.valign), (1, 1));
        assert_eq!(t.color, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(t.plane, None);
    }

    #[test]
    fn world_text_takes_the_text_opts_and_a_plane() {
        let mut m = Map::new();
        m.insert("font".into(), Dynamic::from("display"));
        m.insert("align".into(), Dynamic::from("left"));
        m.insert("valign".into(), Dynamic::from("bottom"));
        m.insert(
            "facing".into(),
            arr(&[
                Dynamic::from(0_i64),
                Dynamic::from(0_i64),
                Dynamic::from(-1.0_f64),
            ]),
        );
        let t = world_text_from([0.0; 3], "x", 0.2, &m);
        assert_eq!(t.font.as_deref(), Some("display"));
        assert_eq!((t.align, t.valign), (0, 2));
        let plane = t.plane.unwrap();
        assert_eq!(plane.normal, [0.0, 0.0, -1.0]);
        assert_eq!(plane.up, [0.0, 1.0, 0.0]);

        m.insert(
            "up".into(),
            arr(&[
                Dynamic::from(1_i64),
                Dynamic::from(0_i64),
                Dynamic::from(0_i64),
            ]),
        );
        assert_eq!(
            world_text_from([0.0; 3], "x", 0.2, &m).plane.unwrap().up,
            [1.0, 0.0, 0.0]
        );
    }

    #[test]
    fn world_text_without_a_usable_normal_is_a_billboard() {
        let mut m = Map::new();
        m.insert(
            "facing".into(),
            arr(&[
                Dynamic::from(0_i64),
                Dynamic::from(0_i64),
                Dynamic::from(0_i64),
            ]),
        );
        assert_eq!(world_text_from([0.0; 3], "x", 9.0, &m).plane, None);
        m.insert("facing".into(), arr(&[Dynamic::from(1_i64)]));
        assert_eq!(world_text_from([0.0; 3], "x", 9.0, &m).plane, None);
    }

    #[test]
    fn measure_uses_installed_measurer() {
        let mut c = ScriptCallContext::new();
        assert_eq!(measure_text_with(&c, "abcd", 10.0, None, 0.0), (24.0, 10.0));
        c.text_measurer = Some(Arc::new(
            |text: &str, size: f32, font: Option<&str>, sp: f32| {
                let base = if font == Some("display") { 2.0 } else { 1.0 };
                (text.len() as f32 * size * base + sp, size * 1.5)
            },
        ));
        assert_eq!(measure_text_with(&c, "abcd", 10.0, None, 0.0), (40.0, 15.0));
        assert_eq!(
            measure_text_with(&c, "ab", 10.0, Some("display"), 1.0),
            (41.0, 15.0)
        );
    }

    #[test]
    fn style_array_maps_by_length() {
        use crate::ui::element::StyleValue;
        assert_eq!(
            StyleValue::from_numbers(&array_to_f32s(&vec![
                Dynamic::from(1_i64),
                Dynamic::from(2.0_f64),
                Dynamic::from(3_i64),
                Dynamic::from(4_i64)
            ])),
            Some(StyleValue::Color([1.0, 2.0, 3.0, 4.0]))
        );
        assert_eq!(
            StyleValue::from_numbers(&[1.0, 2.0, 0.0, 0.0, 0.0, 1.0]),
            Some(StyleValue::Shadow(1.0, 2.0, [0.0, 0.0, 0.0, 1.0]))
        );
        assert_eq!(StyleValue::from_numbers(&[1.0, 2.0]), None);
    }
}
