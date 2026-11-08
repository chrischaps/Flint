//! What styles exist and what they take, for agents and people (#303):
//! `flint render --list-styles` prints [`catalog_text`], and with `--json`
//! the [`catalog`] as JSON. Everything comes from the registry's
//! descriptors, so a new style (built-in or a `styles/<name>.shader.toml`
//! once [`custom::load_dir`](super::custom::load_dir) has run) appears with
//! no change here.

use super::{is_builtin, registry, ChoiceScope, StyleDescriptor, StyleStage};
use serde::Serialize;
use std::path::Path;

/// One registered style, as listed.
#[derive(Debug, Clone, Serialize)]
pub struct StyleListing {
    pub name: String,
    pub label: String,
    /// `"hdr"` (linear, before fog/bloom/tonemap) or `"display"` (after
    /// the composite, at output resolution).
    pub stage: &'static str,
    /// `"builtin"`, or `"user"` for a `styles/<name>.shader.toml`.
    pub source: &'static str,
    /// Where a scene sets it: `[post_process.styles.<name>]`.
    pub table: String,
    /// The component an entity takes the style with (per-entity styles).
    pub component: Option<String>,
    /// Whether it reads the entity style mask.
    pub uses_mask: bool,
    pub params: Vec<ParamListing>,
    pub options: Vec<OptionListing>,
    /// The colours (#315): a key a scene writes as `[r, g, b]` or
    /// `"#rrggbb"`, over three of `params` (`tint` over `tint_r/g/b`).
    pub colors: Vec<ColorListing>,
}

/// One colour setting.
#[derive(Debug, Clone, Serialize)]
pub struct ColorListing {
    /// The key a scene writes (`tint`).
    pub key: String,
    /// Display values, 0..1 per channel.
    pub default: [f32; 3],
    /// The parameters underneath, red, green, blue.
    pub channels: [String; 3],
    pub label: String,
    pub help: String,
}

/// One numeric parameter.
#[derive(Debug, Clone, Serialize)]
pub struct ParamListing {
    pub name: String,
    /// The key a scene writes in the style's table (the same as `name`).
    pub key: String,
    pub default: f32,
    /// The F4 menu's range. A value outside it is taken as written.
    pub min: f32,
    pub max: f32,
    /// Whole numbers only (a cell size, a count).
    pub integer: bool,
    /// The colour this is a channel of (`tint` for `tint_r`), if any.
    pub color: Option<String>,
    pub label: String,
    pub help: String,
}

/// One named choice (a string in the table).
#[derive(Debug, Clone, Serialize)]
pub struct OptionListing {
    pub name: String,
    pub default: String,
    /// The values offered; empty means any text (a file or set name).
    pub choices: Vec<String>,
    /// The choices were found beside the scene (its palettes, its fonts, a
    /// font's ramps; #322), and a name outside them is still the style's
    /// to resolve (a file path).
    pub open: bool,
    pub label: String,
    pub help: String,
}

/// Every registered style, in registration order.
pub fn catalog() -> Vec<StyleListing> {
    catalog_for(None)
}

/// [`catalog`] with the choices found beside the scene in `scene_dir`
/// (its palettes and glyph sets, #322).
pub fn catalog_for(scene_dir: Option<&Path>) -> Vec<StyleListing> {
    registry()
        .iter()
        .map(|d| listing_in(d, scene_dir))
        .collect()
}

/// One style's listing.
pub fn listing(d: &StyleDescriptor) -> StyleListing {
    listing_in(d, None)
}

/// One style's listing, its found choices looked for beside `scene_dir`.
pub fn listing_in(d: &StyleDescriptor, scene_dir: Option<&Path>) -> StyleListing {
    let scope = ChoiceScope::defaults(scene_dir);
    let help = |name: &str| d.hint(name).map(|h| h.help.to_string()).unwrap_or_default();
    StyleListing {
        name: d.name.to_string(),
        label: d.label.to_string(),
        stage: match d.stage {
            StyleStage::Hdr => "hdr",
            StyleStage::Display => "display",
        },
        source: if is_builtin(d.name) {
            "builtin"
        } else {
            "user"
        },
        table: format!("[post_process.styles.{}]", d.name),
        component: d.component.map(str::to_string),
        uses_mask: d.uses_mask.ever(),
        params: d
            .params
            .iter()
            .map(|p| ParamListing {
                name: p.name.to_string(),
                key: p.name.to_string(),
                default: p.default,
                min: p.min,
                max: p.max,
                integer: d.hint(p.name).is_some_and(|h| h.integer),
                color: d.color_of(p.name).map(|c| c.base.to_string()),
                label: d.label_for(p.name),
                help: help(p.name),
            })
            .collect(),
        options: d
            .options
            .iter()
            .map(|o| OptionListing {
                name: o.name.to_string(),
                default: o.default.to_string(),
                choices: o.choices_in(&scope),
                open: o.discover.is_some(),
                label: d.label_for(o.name),
                help: help(o.name),
            })
            .collect(),
        colors: d
            .colors()
            .into_iter()
            .map(|c| ColorListing {
                key: c.base.to_string(),
                default: c.default_rgb(),
                channels: c.channels.map(|p| p.name.to_string()),
                label: d.label_for(c.base),
                help: help(c.base),
            })
            .collect(),
    }
}

/// [`catalog`] for a terminal: one block per style.
pub fn catalog_text() -> String {
    catalog_text_for(None)
}

/// [`catalog_for`] for a terminal.
pub fn catalog_text_for(scene_dir: Option<&Path>) -> String {
    let mut out = String::new();
    for s in catalog_for(scene_dir) {
        let mut tags = vec![s.stage.to_string(), s.source.to_string()];
        if let Some(c) = &s.component {
            tags.push(format!("per-entity: {c}"));
        }
        if s.uses_mask {
            tags.push("reads mask".into());
        }
        out.push_str(&format!("{} ({}) [{}]\n", s.name, s.label, tags.join(", ")));
        out.push_str(&format!("  {}\n", s.table));
        if s.params.is_empty() && s.options.is_empty() {
            out.push_str("  (no settings)\n");
        }
        let width = s
            .params
            .iter()
            .map(|p| p.key.len())
            .chain(s.options.iter().map(|o| o.name.len()))
            .max()
            .unwrap_or(0);
        for o in &s.options {
            let choices = if o.choices.is_empty() {
                "<text>".to_string()
            } else if o.open {
                format!("{}|<name>", o.choices.join("|"))
            } else {
                o.choices.join("|")
            };
            out.push_str(&format!(
                "  {:width$}  \"{}\"  {}{}\n",
                o.name,
                o.default,
                choices,
                tail(&o.help)
            ));
        }
        for p in &s.params {
            // A colour's channels are listed as the colour, where red stands
            if let Some(key) = &p.color {
                let c = s.colors.iter().find(|c| &c.key == key);
                if let Some(c) = c.filter(|c| c.channels[0] == p.name) {
                    let [r, g, b] = c.default.map(num);
                    out.push_str(&format!(
                        "  {:width$}  [{r}, {g}, {b}]  colour, or \"#rrggbb\"{}\n",
                        c.key,
                        tail(&c.help)
                    ));
                }
                continue;
            }
            let int = if p.integer { ", integer" } else { "" };
            out.push_str(&format!(
                "  {:width$}  {}  [{} .. {}{}]{}\n",
                p.key,
                num(p.default),
                num(p.min),
                num(p.max),
                int,
                tail(&p.help)
            ));
        }
        out.push('\n');
    }
    out.push_str(
        "Every table also takes `enabled` and `order = <n>` (lower runs first in its stage).\n\
         Try one with `flint render <scene> --style <name>:<key>=<value>`; `--json` for this as JSON.\n",
    );
    out
}

fn tail(help: &str) -> String {
    if help.is_empty() {
        String::new()
    } else {
        format!("  — {help}")
    }
}

/// `4` rather than `4.0`, `0.35` as written.
fn num(n: f32) -> String {
    if n.fract() == 0.0 && n.abs() < 1e7 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_kuwahara_and_invert_with_stages_and_ranges() {
        let all = catalog();
        let kuwahara = all.iter().find(|s| s.name == "kuwahara").unwrap();
        assert_eq!(kuwahara.stage, "hdr");
        let radius = kuwahara.params.iter().find(|p| p.name == "radius").unwrap();
        assert_eq!((radius.min, radius.max, radius.default), (1.0, 8.0, 4.0));
        // Its table key since #298
        assert_eq!(radius.key, "radius");
        assert_eq!(kuwahara.table, "[post_process.styles.kuwahara]");
        assert!(radius.integer);
        let invert = all.iter().find(|s| s.name == "invert").unwrap();
        assert_eq!(invert.stage, "display");
        assert!(invert.params.iter().any(|p| p.name == "strength"));
        assert_eq!(invert.table, "[post_process.styles.invert]");
    }

    #[test]
    fn json_parses_and_lists_every_style() {
        let json = serde_json::to_string(&catalog()).unwrap();
        let back: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(back.as_array().unwrap().len(), registry().len());
    }

    #[test]
    fn kuwahara_lists_the_keys_style_takes() {
        // Every listed key is one `--style kuwahara:` accepts
        let kuwahara = catalog()
            .into_iter()
            .find(|s| s.name == "kuwahara")
            .unwrap();
        for p in &kuwahara.params {
            let spec = format!("kuwahara:{}={}", p.name, p.default);
            assert!(super::super::StyleSpec::parse(&spec).is_ok(), "{spec}");
        }
    }

    #[test]
    fn a_colour_is_listed_once_by_its_key() {
        let ascii = listing(super::super::find("ascii").unwrap());
        // The tint, and the edge ink's colour (#351)
        assert_eq!(ascii.colors.len(), 2);
        assert_eq!(ascii.colors[1].key, "edge_color");
        let tint = &ascii.colors[0];
        assert_eq!(tint.key, "tint");
        assert_eq!(tint.default, [0.35, 1.0, 0.45]);
        assert_eq!(
            tint.channels,
            ["tint_r", "tint_g", "tint_b"].map(String::from)
        );
        let g = ascii.params.iter().find(|p| p.name == "tint_g").unwrap();
        assert_eq!(g.color.as_deref(), Some("tint"));
        let text = catalog_text();
        assert!(text.contains("[0.35, 1, 0.45]  colour"), "{text}");
        assert!(!text.contains("  tint_r "), "{text}");
    }

    #[test]
    fn text_names_every_style() {
        let text = catalog_text();
        for d in registry() {
            assert!(text.contains(&format!("{} (", d.name)), "{}", d.name);
        }
    }
}
