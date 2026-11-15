//! What the draw modules share: reading numbers out of script values, the
//! opts map the draw calls take, and text measurement. Not a module of
//! functions.
//!
//! **The opts map** (#702). A draw primitive takes what places it
//! positionally (a position and a size, two points, a centre and radii)
//! and everything else in a trailing map, every key optional:
//! `draw_rect(x, y, w, h, #{color: "#ff8800", rounding: 6, layer: 2})`.
//! One reader, [`Opts::read`], knows every key any call takes and the shape
//! each must have; a call names the keys it takes, and [`read_opts`] warns
//! once per function and key about one it doesn't take (a typo:
//! `colour`) or a value of the wrong shape, then draws without it.

use super::builder::{Color, PureKind};
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

/// The colour a draw takes when its opts name none.
pub(crate) const WHITE: [f32; 4] = [1.0; 4];

/// A rectangle's corner rounding in points: one radius for every corner,
/// or each corner's (top left, top right, bottom right, bottom left).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Rounding {
    All(f32),
    Corners([f32; 4]),
}

/// An opts map, read: every key any draw call takes, `None` where the map
/// didn't name it (or named it with a value of the wrong shape).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Opts {
    pub color: Option<[f32; 4]>,
    pub layer: Option<i32>,
    pub thickness: Option<f32>,
    pub rounding: Option<Rounding>,
    /// A sprite's part of its image, `[u0, v0, u1, v1]`, 0..1.
    pub uv: Option<[f32; 4]>,
    /// A gradient's two colours.
    pub from: Option<[f32; 4]>,
    pub to: Option<[f32; 4]>,
    pub vertical: Option<bool>,
    /// Whether a polygon outline closes back to its first point.
    pub closed: Option<bool>,
    // Text
    pub font: Option<String>,
    pub align: Option<u8>,
    pub valign: Option<u8>,
    pub stroke: Option<([f32; 4], f32)>,
    pub spacing: Option<f32>,
    pub shadow: Option<([f32; 4], f32, f32)>,
    pub glow: Option<([f32; 4], f32)>,
    pub facing: Option<[f32; 3]>,
    pub up: Option<[f32; 3]>,
    // Animation layers
    pub mode: Option<String>,
    pub mask: Option<String>,
}

/// What was wrong with one key of an opts map.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Problem {
    pub key: String,
    /// The key is one the function takes, and its value had the wrong
    /// shape; else the function doesn't take the key at all.
    pub known: bool,
}

/// What a key's value must be, for the warning.
pub(crate) fn wants(key: &str) -> &'static str {
    match key {
        "color" | "from" | "to" => "a colour ([r, g, b], [r, g, b, a] or \"#rrggbb\")",
        "layer" => "a number",
        "thickness" | "spacing" => "a number",
        "rounding" => "a number, or [tl, tr, br, bl]",
        "uv" => "[u0, v0, u1, v1]",
        "vertical" | "closed" => "true or false",
        "font" | "mode" | "mask" => "a string",
        "align" => "\"left\", \"center\" or \"right\"",
        "valign" => "\"top\", \"middle\" or \"bottom\"",
        "stroke" => "[r, g, b, a, width]",
        "shadow" => "[dx, dy, r, g, b, a]",
        "glow" => "[r, g, b, a, radius]",
        "facing" | "up" => "[x, y, z]",
        _ => "",
    }
}

/// `N` numbers from an array of exactly `N`.
fn floats<const N: usize>(v: &Dynamic) -> Option<[f32; N]> {
    let arr = v.read_lock::<Array>()?;
    if arr.len() != N {
        return None;
    }
    let mut out = [0.0f32; N];
    for (slot, d) in out.iter_mut().zip(arr.iter()) {
        *slot = dyn_to_f32(d)?;
    }
    Some(out)
}

fn color(v: &Dynamic) -> Option<[f32; 4]> {
    <Color as PureKind>::convert_pure(&mut v.clone()).ok()
}

fn string(v: &Dynamic) -> Option<String> {
    v.read_lock::<ImmutableString>().map(|s| s.to_string())
}

fn set<T>(slot: &mut Option<T>, v: Option<T>) -> bool {
    let ok = v.is_some();
    if ok {
        *slot = v;
    }
    ok
}

impl Opts {
    /// Read `map`, taking only `keys`. The problems are the keys it held
    /// that aren't in `keys` and those whose value had the wrong shape; the
    /// rest is read regardless. No problem, no allocation.
    pub(crate) fn read(map: &Map, keys: &[&str]) -> (Self, Vec<Problem>) {
        let mut o = Self::default();
        let mut problems = Vec::new();
        for (k, v) in map {
            let key = k.as_str();
            let known = keys.contains(&key);
            if !(known && o.take(key, v)) {
                problems.push(Problem {
                    key: key.to_string(),
                    known,
                });
            }
        }
        (o, problems)
    }

    /// Read one key's value into its field; false when it is the wrong
    /// shape (the field is left `None`).
    fn take(&mut self, key: &str, v: &Dynamic) -> bool {
        match key {
            "color" => set(&mut self.color, color(v)),
            "from" => set(&mut self.from, color(v)),
            "to" => set(&mut self.to, color(v)),
            "layer" => set(&mut self.layer, dyn_to_f32(v).map(|f| f as i32)),
            "thickness" => set(&mut self.thickness, dyn_to_f32(v)),
            "spacing" => set(&mut self.spacing, dyn_to_f32(v)),
            "rounding" => {
                let r = dyn_to_f32(v)
                    .map(Rounding::All)
                    .or_else(|| floats::<4>(v).map(Rounding::Corners));
                set(&mut self.rounding, r)
            }
            "uv" => set(&mut self.uv, floats::<4>(v)),
            "vertical" => set(&mut self.vertical, v.as_bool().ok()),
            "closed" => set(&mut self.closed, v.as_bool().ok()),
            // An empty font is the default one: valid, and nothing to set
            "font" => match string(v) {
                Some(s) => {
                    if !s.is_empty() {
                        self.font = Some(s);
                    }
                    true
                }
                None => false,
            },
            "align" => {
                let a = match string(v) {
                    Some(s) => match s.as_str() {
                        "left" => Some(0),
                        "center" => Some(1),
                        "right" => Some(2),
                        _ => None,
                    },
                    None => dyn_to_f32(v).map(|i| (i as i64).clamp(0, 2) as u8),
                };
                set(&mut self.align, a)
            }
            "valign" => {
                let a = match string(v) {
                    Some(s) => match s.as_str() {
                        "top" => Some(0),
                        "middle" | "center" => Some(1),
                        "bottom" => Some(2),
                        _ => None,
                    },
                    None => dyn_to_f32(v).map(|i| (i as i64).clamp(0, 2) as u8),
                };
                set(&mut self.valign, a)
            }
            "stroke" => set(
                &mut self.stroke,
                floats::<5>(v).map(|[r, g, b, a, w]| ([r, g, b, a], w)),
            ),
            "shadow" => set(
                &mut self.shadow,
                floats::<6>(v).map(|[dx, dy, r, g, b, a]| ([r, g, b, a], dx, dy)),
            ),
            "glow" => set(
                &mut self.glow,
                floats::<5>(v).map(|[r, g, b, a, radius]| ([r, g, b, a], radius)),
            ),
            "facing" => set(&mut self.facing, floats::<3>(v)),
            "up" => set(&mut self.up, floats::<3>(v)),
            "mode" => set(&mut self.mode, string(v)),
            "mask" => set(&mut self.mask, string(v)),
            _ => false,
        }
    }

    /// The colour, white when unnamed.
    #[inline]
    pub(crate) fn color(&self) -> [f32; 4] {
        self.color.unwrap_or(WHITE)
    }

    /// The layer, 0 when unnamed.
    #[inline]
    pub(crate) fn layer(&self) -> i32 {
        self.layer.unwrap_or(0)
    }

    /// The thickness in points, 1 when unnamed.
    #[inline]
    pub(crate) fn thickness(&self) -> f32 {
        self.thickness.unwrap_or(1.0)
    }
}

/// [`Opts::read`] for a call of `func`: each problem warns once per
/// function and key (`ScriptCallContext::opts_warned`), and the draw goes
/// ahead without it.
pub(crate) fn read_opts(
    c: &mut ScriptCallContext,
    func: &'static str,
    map: &Map,
    keys: &[&str],
) -> Opts {
    let (o, problems) = Opts::read(map, keys);
    for p in problems {
        warn_opts(c, func, &p, keys);
    }
    o
}

/// Warn about one problem of `func`'s opts, once per function and key.
pub(crate) fn warn_opts(c: &mut ScriptCallContext, func: &'static str, p: &Problem, keys: &[&str]) {
    if !c.opts_warned.insert((func, p.key.clone())) {
        return;
    }
    if p.known {
        tracing::warn!(
            "{func}: opts key \"{}\" should be {}; ignored. Warned once for this function and key",
            p.key,
            wants(&p.key)
        );
    } else {
        tracing::warn!(
            "{func}: opts key \"{}\" is not one it takes ({}); ignored. Warned once for this \
             function and key",
            p.key,
            keys.join(", ")
        );
    }
}

/// The keys `draw_text`'s opts take.
pub(crate) const TEXT_KEYS: &[&str] = &[
    "color", "font", "align", "stroke", "spacing", "layer", "shadow", "glow",
];

/// The keys `draw_world_text`'s opts take: the text keys but `layer`, and
/// where it stands.
pub(crate) const WORLD_TEXT_KEYS: &[&str] = &[
    "color", "font", "align", "valign", "stroke", "spacing", "shadow", "glow", "facing", "up",
];

/// A text draw's options, defaults filled in: white, left-aligned, layer 0,
/// no stroke/shadow/glow, the default font.
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
        Self::from_opts(Opts::default())
    }
}

impl TextOpts {
    /// The text options of a map, its problems dropped (the tests' form).
    #[cfg(test)]
    pub(crate) fn from_map(opts: &Map) -> Self {
        Self::from_opts(Opts::read(opts, TEXT_KEYS).0)
    }

    pub(crate) fn from_opts(o: Opts) -> Self {
        Self {
            color: o.color(),
            align: o.align.unwrap_or(0),
            layer: o.layer(),
            stroke: o.stroke,
            font: o.font,
            spacing: o.spacing.unwrap_or(0.0),
            shadow: o.shadow,
            glow: o.glow,
        }
    }
}

/// A `draw_world_text` call as the host draws it, from its map's problems
/// dropped (the tests' form); see [`world_text_from_opts`].
#[cfg(test)]
pub(crate) fn world_text_from(
    at: [f32; 3],
    text: &str,
    size: f32,
    opts: &Map,
) -> flint_core::world_text::WorldText {
    world_text_from_opts(at, text, size, Opts::read(opts, WORLD_TEXT_KEYS).0)
}

/// A `draw_world_text` call as the host draws it: the text options,
/// centred and middled on the point unless `align` or `valign` say
/// otherwise, and on a plane when `facing` is a usable normal.
pub(crate) fn world_text_from_opts(
    at: [f32; 3],
    text: &str,
    size: f32,
    o: Opts,
) -> flint_core::world_text::WorldText {
    let align = o.align.unwrap_or(1);
    let valign = o.valign.unwrap_or(1);
    let plane = o
        .facing
        .filter(|n| n.iter().any(|c| *c != 0.0))
        .map(|normal| flint_core::world_text::TextPlane {
            normal,
            up: o.up.unwrap_or([0.0, 1.0, 0.0]),
        });
    let t = TextOpts::from_opts(o);
    flint_core::world_text::WorldText {
        at,
        text: text.to_string(),
        size,
        color: t.color,
        align,
        valign,
        stroke: t.stroke,
        font: t.font,
        spacing: t.spacing,
        shadow: t.shadow,
        glow: t.glow,
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

    #[test]
    fn a_colour_is_an_array_or_hex() {
        let keys = &["color", "from", "to"];
        let mut m = Map::new();
        m.insert("color".into(), Dynamic::from("#ff8000"));
        m.insert(
            "from".into(),
            arr(&[
                Dynamic::from(1_i64),
                Dynamic::from(0.5_f64),
                Dynamic::from(0_i64),
            ]),
        );
        m.insert("to".into(), Dynamic::from("#00000080"));
        let (o, problems) = Opts::read(&m, keys);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(o.color, Some([1.0, 128.0 / 255.0, 0.0, 1.0]));
        assert_eq!(o.from, Some([1.0, 0.5, 0.0, 1.0]), "alpha 1 when left out");
        assert_eq!(o.to, Some([0.0, 0.0, 0.0, 128.0 / 255.0]));
    }

    #[test]
    fn rounding_is_one_radius_or_four_corners() {
        let mut m = Map::new();
        m.insert("rounding".into(), Dynamic::from(6_i64));
        assert_eq!(
            Opts::read(&m, &["rounding"]).0.rounding,
            Some(Rounding::All(6.0))
        );
        m.insert(
            "rounding".into(),
            arr(&[
                Dynamic::from(1_i64),
                Dynamic::from(2.0_f64),
                Dynamic::from(3_i64),
                Dynamic::from(4_i64),
            ]),
        );
        assert_eq!(
            Opts::read(&m, &["rounding"]).0.rounding,
            Some(Rounding::Corners([1.0, 2.0, 3.0, 4.0]))
        );
    }

    #[test]
    fn a_key_not_taken_or_misshapen_is_a_problem_and_the_rest_is_read() {
        let mut m = Map::new();
        m.insert("colour".into(), Dynamic::from("#ffffff"));
        m.insert("layer".into(), Dynamic::from("top"));
        m.insert("thickness".into(), Dynamic::from(3_i64));
        let (o, problems) = Opts::read(&m, &["color", "layer", "thickness"]);
        assert_eq!(o.thickness, Some(3.0));
        assert_eq!(o.layer, None);
        assert_eq!(
            problems,
            vec![
                Problem {
                    key: "colour".into(),
                    known: false
                },
                Problem {
                    key: "layer".into(),
                    known: true
                },
            ]
        );
        // A key another function takes is still not this one's
        let mut m = Map::new();
        m.insert("rounding".into(), Dynamic::from(2_i64));
        let (o, problems) = Opts::read(&m, &["color"]);
        assert_eq!(o.rounding, None);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn a_problem_warns_once_per_function_and_key() {
        let mut c = ScriptCallContext::new();
        let mut m = Map::new();
        m.insert("colour".into(), Dynamic::from("#ffffff"));
        read_opts(&mut c, "draw_rect", &m, &["color"]);
        read_opts(&mut c, "draw_rect", &m, &["color"]);
        read_opts(&mut c, "draw_circle", &m, &["color"]);
        assert_eq!(c.opts_warned.len(), 2);
    }
}
