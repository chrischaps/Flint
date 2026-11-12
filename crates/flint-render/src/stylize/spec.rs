//! `--style name[:key=value,...]` (#271): switch any registered style on,
//! and set its parameters and options, from one command-line word, so a
//! style can be previewed on any scene without editing the scene.
//!
//! ```text
//! --style ascii                          the style at its defaults
//! --style ascii:cell=12,ramp=blocks      a parameter (number) and an option (text)
//! --style invert:strength=0.5
//! --style kuwahara:radius=6
//! --style ascii:off                      off everywhere: the scene's table and
//!                                        every entity's ascii_style (#376)
//! --style invert:order=0                 run first in its stage (#299)
//! --style kuwahara:feeds_bloom=1         bloom reads the painted image (#300)
//! --style ascii:color=tint,tint=#4dff73  a colour (#315): #rrggbb, or r/g/b
//! --style ascii:tint=0.3/1/0.4
//! --style pencil:reveal=0.5              shown inside half a ragged tear (#406)
//! --style pencil:reveal=0.4,reveal_shape=wipe,reveal_angle=30
//! ```
//!
//! Keys are checked against the style's descriptor: a number for each of
//! its [`params`](super::StyleDescriptor::params), text for each of its
//! [`options`](super::StyleDescriptor::options) (one of its `choices`
//! when it lists them), a colour for each of its
//! [`colors`](super::StyleDescriptor::colors) (`#rrggbb` or `r/g/b`, since
//! commas split keys; its channels `tint_r`... still take numbers), and
//! `enabled` / `off`. A number outside its parameter's F4 range (a
//! colour's channels included) is refused, naming the range (#355): a
//! style clamps it or draws it badly, and either way not as asked. A scene's table keeps
//! such a value and warns.

use super::{color_from_text, find, names, registry, StyleDescriptor};
use crate::postprocess::PostProcessConfig;

/// One parsed `--style` word.
#[derive(Debug, Clone, PartialEq)]
pub struct StyleSpec {
    pub name: String,
    pub enabled: bool,
    /// `key=value` pairs in the order given.
    pub settings: Vec<(String, String)>,
    /// `feeds_bloom=true|false` (#300): whether bloom reads the style's
    /// result; `None` leaves the scene's choice.
    pub feeds_bloom: Option<bool>,
}

impl StyleSpec {
    /// Parse `name[:key=value,...]`, checking the name and keys against
    /// the registry.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let spec = spec.trim();
        let (name, rest) = match spec.split_once(':') {
            Some((name, rest)) => (name.trim(), rest.trim()),
            None => (spec, ""),
        };
        if name == "help" {
            return Err(format!(
                "the registered styles and their keys (`flint render --list-styles` gives \
                 ranges, stages and --json):\n{}",
                style_spec_help()
            ));
        }
        let descriptor = find(name).ok_or_else(|| {
            format!(
                "unknown style '{name}'; registered: {} (`--style help` lists their keys)",
                names().join(", ")
            )
        })?;
        let mut parsed = StyleSpec {
            name: name.to_string(),
            enabled: true,
            settings: Vec::new(),
            feeds_bloom: None,
        };
        for pair in rest.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            if pair == "off" {
                parsed.enabled = false;
                continue;
            }
            if pair == "on" {
                parsed.enabled = true;
                continue;
            }
            let (key, value) = pair.split_once('=').ok_or_else(|| {
                format!(
                    "{name}: '{pair}' is not key=value ({})",
                    known_keys(descriptor)
                )
            })?;
            let (key, value) = (key.trim(), value.trim());
            if key == "enabled" {
                parsed.enabled = match value {
                    "true" | "1" | "on" => true,
                    "false" | "0" | "off" => false,
                    _ => return Err(format!("{name}: enabled must be true or false")),
                };
                continue;
            }
            if key == "feeds_bloom" {
                if descriptor.stage != super::StyleStage::Hdr {
                    return Err(format!(
                        "{name}: feeds_bloom is for hdr styles; {name} draws after bloom"
                    ));
                }
                parsed.feeds_bloom = Some(match value {
                    "true" | "1" | "on" => true,
                    "false" | "0" | "off" => false,
                    _ => return Err(format!("{name}: feeds_bloom must be true or false")),
                });
                continue;
            }
            if key == "order" || key == "blend" {
                number(name, key, value)?;
            } else if super::reveal::is_key(key) {
                // Where it shows (#406): checked, ranges and all
                super::StyleReveal::default()
                    .set_text(key, value)
                    .map_err(|e| format!("{name}: {e}"))?;
            } else {
                check(descriptor, key, value)?;
            }
            parsed.settings.push((key.to_string(), value.to_string()));
        }
        Ok(parsed)
    }

    /// Write this spec into `config`: the style on (or off) with its
    /// settings, over whatever the scene said.
    pub fn apply(&self, config: &mut PostProcessConfig) {
        // Any style's place in its stage (#299) and whether it feeds bloom
        // (#300)
        for (key, value) in &self.settings {
            if key == "order" {
                config.styles.set_order(&self.name, value.parse().ok());
            }
            // How much of it shows (#369)
            if key == "blend" {
                config.styles.set_blend(&self.name, value.parse().ok());
            }
            // Where it shows (#406)
            if super::reveal::is_key(key) {
                let mut reveal = super::StyleReveal::default();
                if reveal.set_text(key, value).is_ok() {
                    let value = reveal_value(&reveal, key);
                    let _ = config.styles.set_reveal(&self.name, key, &value, false);
                }
            }
        }
        if let Some(feeds) = self.feeds_bloom {
            config.styles.set_feeds_bloom(&self.name, Some(feeds));
        }
        let descriptor = find(&self.name).expect("checked by parse");
        if self.enabled {
            config.styles.set_enabled(&self.name, true);
        } else {
            // Off everywhere (#376): the table and every entity that
            // would switch the style on for its own pixels, so a styled
            // scene renders unstyled with the rest of its post kept
            config.styles.set_off_everywhere(&self.name);
        }
        for (key, value) in &self.settings {
            if key == "order" || key == "blend" || super::reveal::is_key(key) {
                continue;
            }
            if descriptor.options.iter().any(|o| o.name == key) {
                config.styles.set_option(&self.name, key, value);
            } else if let Some(color) = descriptor.color_setting(key) {
                if let Some(rgb) = color_from_text(value) {
                    for (p, v) in color.channels.iter().zip(rgb) {
                        config.styles.set_param(&self.name, p.name, v);
                    }
                }
            } else if let Ok(n) = value.parse::<f32>() {
                config.styles.set_param(&self.name, key, n);
            }
        }
    }
}

/// Reveal key `key` of `reveal` as the TOML value that sets it.
fn reveal_value(reveal: &super::StyleReveal, key: &str) -> toml::Value {
    match key {
        "reveal" => toml::Value::Float(f64::from(reveal.amount.unwrap_or(0.0))),
        "reveal_shape" => toml::Value::String(reveal.shape().name().to_string()),
        "reveal_ink" => toml::Value::Array(
            reveal
                .ink()
                .iter()
                .map(|c| toml::Value::Float(f64::from(*c)))
                .collect(),
        ),
        _ => toml::Value::Float(f64::from(reveal.number(key))),
    }
}

/// Parse `spec` and write it into `config`.
pub fn apply_style_spec(config: &mut PostProcessConfig, spec: &str) -> Result<(), String> {
    StyleSpec::parse(spec)?.apply(config);
    Ok(())
}

/// Write every word of `specs` into `config`, in order (a later word
/// wins), and return the errors of the words that didn't parse, each
/// naming its word. The windowed hosts (#328: `flint-player --style`,
/// `flint edit scene.toml --style`, the web's `?style=`) call this over
/// the scene's config at every load, transition and hot reload, so the
/// command line keeps winning over what the scene authors.
pub fn apply_style_specs(config: &mut PostProcessConfig, specs: &[String]) -> Vec<String> {
    specs
        .iter()
        .filter_map(|spec| {
            apply_style_spec(config, spec)
                .err()
                .map(|e| format!("--style {spec}: {e}"))
        })
        .collect()
}

/// Check every word of `specs` against the registry, for a host to refuse
/// a typo at start. Call it once the scene's user styles are registered
/// (`custom::load_dir`), since a word may name one.
pub fn check_style_specs(specs: &[String]) -> Result<(), String> {
    for spec in specs {
        StyleSpec::parse(spec).map_err(|e| format!("--style {spec}: {e}"))?;
    }
    Ok(())
}

/// One line per registered style with the keys `--style` takes, for help
/// text and error messages.
pub fn style_spec_help() -> String {
    let mut lines: Vec<String> = registry()
        .iter()
        .map(|d| format!("{}: {}", d.name, known_keys(d)))
        .collect();
    lines.push(reveal_help());
    lines.join("\n")
}

/// The reveal's keys (#406), which every style takes.
fn reveal_help() -> String {
    let mut keys = vec![format!(
        "reveal_shape={}",
        super::RevealShape::NAMES.join("|")
    )];
    keys.push("reveal_ink=#rrggbb".to_string());
    keys.extend(
        super::reveal::NUMBERS
            .iter()
            .map(|n| format!("{}=<{}-{}>", n.key, n.min, n.max)),
    );
    format!("every style, with reveal=<0-1>: {}", keys.join(", "))
}

fn check(descriptor: &StyleDescriptor, key: &str, value: &str) -> Result<(), String> {
    let name = descriptor.name;
    if let Some(option) = descriptor.options.iter().find(|o| o.name == key) {
        // An option whose choices are found at run time (a palette file,
        // a font's ramps) is open: the style resolves the name, and warns
        if option.refuses(value) {
            return Err(format!(
                "{name}: {key} must be one of {}",
                option.choices.join(", ")
            ));
        }
        return Ok(());
    }
    if let Some(color) = descriptor.color_setting(key) {
        let Some(rgb) = color_from_text(value) else {
            return Err(format!(
                "{name}: {key} must be a colour, #rrggbb or r/g/b (0.3/1/0.4), not '{value}'"
            ));
        };
        for (p, v) in color.channels.iter().zip(rgb) {
            if let Some(e) = p.range_error(v) {
                return Err(format!("{name}: {key}={value}: {e}"));
            }
        }
        return Ok(());
    }
    if let Some(param) = descriptor.params.iter().find(|p| p.name == key) {
        number(name, key, value)?;
        let n: f32 = value.parse().expect("checked by number");
        return match param.range_error(n) {
            Some(e) => Err(format!("{name}: {e}")),
            None => Ok(()),
        };
    }
    Err(format!(
        "{name} has no key '{key}'; it takes: {}",
        known_keys(descriptor)
    ))
}

fn number(name: &str, key: &str, value: &str) -> Result<(), String> {
    match value.parse::<f32>() {
        Ok(n) if n.is_finite() => Ok(()),
        _ => Err(format!("{name}: {key} must be a number, not '{value}'")),
    }
}

fn known_keys(descriptor: &StyleDescriptor) -> String {
    let mut keys: Vec<String> = {
        let mut params: Vec<String> = Vec::new();
        for p in descriptor.params {
            match descriptor.color_of(p.name) {
                Some(c) if std::ptr::eq(c.channels[0], p) => {
                    let [r, g, b] = c.default_rgb();
                    params.push(format!("{}={r}/{g}/{b}", c.base));
                }
                Some(_) => {}
                None => params.push(format!("{}={}", p.name, p.default)),
            }
        }
        params
            .into_iter()
            .chain(descriptor.options.iter().map(|o| {
                if o.discover.is_some() {
                    let found = o.choices_in(&super::ChoiceScope::defaults(None));
                    format!("{}={}|<name>", o.name, found.join("|"))
                } else if o.choices.is_empty() {
                    format!("{}=<text>", o.name)
                } else {
                    format!("{}={}", o.name, o.choices.join("|"))
                }
            }))
            .collect()
    };
    keys.push("order=<n>".to_string());
    keys.push("blend=1".to_string());
    // Where it shows (#406); the reveal_* keys are listed once, by
    // style_spec_help
    keys.push("reveal=<0-1>, reveal_*".to_string());
    if descriptor.stage == super::StyleStage::Hdr {
        keys.push(format!("feeds_bloom={}", descriptor.feeds_bloom));
    }
    keys.push("off".to_string());
    keys.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_name_turns_the_style_on_at_its_defaults() {
        let mut config = PostProcessConfig::default();
        apply_style_spec(&mut config, "ascii").unwrap();
        assert!(config.styles.is_enabled("ascii"));
        assert_eq!(config.styles.param("ascii", "cell"), None);
    }

    /// #376: `off` (and `enabled=false`) is off everywhere, not only the
    /// table; `on` or a bare name lifts it, so a later word wins.
    #[test]
    fn off_is_off_everywhere_and_a_later_word_lifts_it() {
        let mut config = PostProcessConfig::default();
        apply_style_spec(&mut config, "ascii:cell=10").unwrap();
        apply_style_spec(&mut config, "ascii:off").unwrap();
        assert!(!config.styles.is_enabled("ascii"));
        assert!(config.styles.is_off_everywhere("ascii"));
        assert_eq!(config.styles.param("ascii", "cell"), Some(10.0));
        apply_style_spec(&mut config, "ascii").unwrap();
        assert!(config.styles.is_enabled("ascii"));
        assert!(!config.styles.is_off_everywhere("ascii"));
        apply_style_spec(&mut config, "ascii:enabled=false").unwrap();
        assert!(config.styles.is_off_everywhere("ascii"));
        apply_style_spec(&mut config, "ascii:on").unwrap();
        assert!(!config.styles.is_off_everywhere("ascii"));
    }

    #[test]
    fn params_are_numbers_and_options_are_text() {
        let mut config = PostProcessConfig::default();
        apply_style_spec(&mut config, "ascii:cell=12, ramp=blocks,color=tint").unwrap();
        assert_eq!(config.styles.param("ascii", "cell"), Some(12.0));
        assert_eq!(config.styles.option("ascii", "ramp"), Some("blocks"));
        assert_eq!(config.styles.option("ascii", "color"), Some("tint"));
    }

    #[test]
    fn a_colour_is_one_key_in_hex_or_slashes() {
        let mut config = PostProcessConfig::default();
        apply_style_spec(&mut config, "ascii:color=tint,tint=0.3/1/0.4").unwrap();
        assert_eq!(config.styles.param("ascii", "tint_r"), Some(0.3));
        assert_eq!(config.styles.param("ascii", "tint_g"), Some(1.0));
        assert_eq!(config.styles.param("ascii", "tint_b"), Some(0.4));
        apply_style_spec(&mut config, "ascii:tint=#ff8000").unwrap();
        let ascii = find("ascii").unwrap();
        assert_eq!(ascii.color(&config, "tint"), [1.0, 128.0 / 255.0, 0.0]);
        // A channel on its own is still a number
        apply_style_spec(&mut config, "ascii:tint_b=0.5").unwrap();
        assert_eq!(config.styles.param("ascii", "tint_b"), Some(0.5));
        let bad = StyleSpec::parse("ascii:tint=green").unwrap_err();
        assert!(bad.contains("#rrggbb"), "{bad}");
        assert!(StyleSpec::parse("ascii:tint=1/1").is_err());
        let help = style_spec_help();
        assert!(help.contains("tint=0.35/1/0.45"), "{help}");
        assert!(!help.contains("tint_r="), "{help}");
    }

    #[test]
    fn off_turns_a_scene_style_off() {
        let mut config = PostProcessConfig::default();
        config.styles.set_enabled("invert", true);
        apply_style_spec(&mut config, "invert:off").unwrap();
        assert!(!config.styles.is_enabled("invert"));
        apply_style_spec(&mut config, "invert:enabled=true,strength=0.5").unwrap();
        assert!(config.styles.is_enabled("invert"));
        assert_eq!(config.styles.param("invert", "strength"), Some(0.5));
    }

    #[test]
    fn kuwahara_writes_its_style_table() {
        let mut config = PostProcessConfig::default();
        apply_style_spec(&mut config, "kuwahara:radius=6,anisotropy=0.5").unwrap();
        assert!(config.styles.is_enabled("kuwahara"));
        assert_eq!(config.styles.param("kuwahara", "radius"), Some(6.0));
        assert_eq!(config.styles.param("kuwahara", "anisotropy"), Some(0.5));
    }

    #[test]
    fn order_is_a_key_of_every_style() {
        let mut config = PostProcessConfig::default();
        apply_style_spec(&mut config, "invert:order=0,strength=0.5").unwrap();
        assert_eq!(config.styles.order("invert"), Some(0.0));
        assert_eq!(config.styles.param("invert", "strength"), Some(0.5));
        apply_style_spec(&mut config, "kuwahara:order=-1").unwrap();
        assert_eq!(config.styles.order("kuwahara"), Some(-1.0));
        assert!(config.styles.is_enabled("kuwahara"));
        assert!(StyleSpec::parse("ascii:order=soon").is_err());
        assert!(style_spec_help().contains("order=<n>"));
    }

    #[test]
    fn feeds_bloom_is_for_hdr_styles() {
        let mut config = PostProcessConfig::default();
        apply_style_spec(&mut config, "kuwahara:feeds_bloom=1,radius=6").unwrap();
        assert!(config.styles.is_enabled("kuwahara"));
        assert_eq!(config.styles.feeds_bloom("kuwahara"), Some(true));
        assert_eq!(config.styles.param("kuwahara", "radius"), Some(6.0));
        assert!(find("kuwahara").unwrap().feeds_bloom_in(&config));
        // Not a parameter: it never lands in the params
        assert_eq!(config.styles.param("kuwahara", "feeds_bloom"), None);
        let display = StyleSpec::parse("ascii:feeds_bloom=1").unwrap_err();
        assert!(display.contains("hdr"), "{display}");
        assert!(StyleSpec::parse("kuwahara:feeds_bloom=maybe").is_err());
        assert!(style_spec_help().contains("feeds_bloom=false"));
    }

    #[test]
    fn mistakes_name_what_would_have_worked() {
        let unknown = StyleSpec::parse("watercolour").unwrap_err();
        assert!(unknown.contains("ascii"), "{unknown}");
        let key = StyleSpec::parse("ascii:size=3").unwrap_err();
        assert!(key.contains("cell=8"), "{key}");
        let choice = StyleSpec::parse("ascii:color=purple").unwrap_err();
        assert!(choice.contains("source"), "{choice}");
        // Open lists only suggest (#318): a glyph set's own ramp, a palette file
        assert!(StyleSpec::parse("ascii:ramp=runes").is_ok());
        assert!(StyleSpec::parse("ascii:palette=palettes/dusk.palette.toml").is_ok());
        let nan = StyleSpec::parse("invert:strength=lots").unwrap_err();
        assert!(nan.contains("number"), "{nan}");
        assert!(StyleSpec::parse("ascii:cell").is_err());
        assert!(StyleSpec::parse("kuwahara:cell=3").is_err());
    }

    #[test]
    fn a_number_outside_its_range_is_refused_naming_the_range() {
        let cell = StyleSpec::parse("ascii:cell=400").unwrap_err();
        assert_eq!(cell, "ascii: cell=400 is outside its range 4..32");
        assert!(StyleSpec::parse("ascii:cell=3.5").is_err());
        // The ends are inside
        assert!(StyleSpec::parse("ascii:cell=4").is_ok());
        assert!(StyleSpec::parse("ascii:cell=32").is_ok());
        // A colour's channels have ranges too
        let tint = StyleSpec::parse("ascii:tint=2/1/0.4").unwrap_err();
        assert!(
            tint.contains("tint_r=2 is outside its range 0..1"),
            "{tint}"
        );
        assert!(StyleSpec::parse("ascii:tint=#ffffff").is_ok());
        // `order` is a place in the chain, not a parameter: any number
        assert!(StyleSpec::parse("ascii:order=1000").is_ok());
        // Every default sits inside its own range
        for d in registry() {
            for p in d.params {
                assert!(p.range_error(p.default).is_none(), "{}.{}", d.name, p.name);
            }
        }
    }

    #[test]
    fn a_list_of_words_applies_in_order_and_names_the_bad_ones() {
        let mut config = PostProcessConfig::default();
        let words = [
            "ascii:cell=12".to_string(),
            "watercolour".to_string(),
            "ascii:cell=6".to_string(),
        ];
        let errors = apply_style_specs(&mut config, &words);
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].starts_with("--style watercolour:"),
            "{}",
            errors[0]
        );
        // The good words still landed, the later one winning
        assert!(config.styles.is_enabled("ascii"));
        assert_eq!(config.styles.param("ascii", "cell"), Some(6.0));
        assert!(check_style_specs(&words[..1]).is_ok());
        assert!(check_style_specs(&words)
            .unwrap_err()
            .contains("watercolour"));
    }

    #[test]
    fn every_registered_style_parses_bare() {
        for d in registry() {
            StyleSpec::parse(d.name).unwrap();
        }
        // A line a style, then the reveal's keys every style takes (#406)
        let help = style_spec_help();
        assert_eq!(help.lines().count(), registry().len() + 1);
        assert!(help
            .lines()
            .last()
            .unwrap()
            .contains("reveal_shape=tear|wipe|iris|panel"));
    }

    #[test]
    fn reveal_keys_parse_check_and_apply() {
        let mut config = PostProcessConfig::default();
        apply_style_spec(
            &mut config,
            "pencil:reveal=0.4,reveal_shape=wipe,reveal_angle=30,reveal_ink=#102030",
        )
        .unwrap();
        assert!(config.styles.is_enabled("pencil"));
        let r = config.styles.reveal("pencil").unwrap();
        assert_eq!(r.amount, Some(0.4));
        assert_eq!(r.shape(), super::super::RevealShape::Wipe);
        assert_eq!(r.number("reveal_angle"), 30.0);
        assert!((r.ink()[2] - 0x30 as f32 / 255.0).abs() < 1e-4);
        // Out of range, a wrong shape or a non-number is refused by name
        for bad in [
            "pencil:reveal=2",
            "pencil:reveal_shape=star",
            "pencil:reveal_edge=big",
            "pencil:reveal_ink=red",
        ] {
            let e = StyleSpec::parse(bad).unwrap_err();
            assert!(e.contains("reveal"), "{bad}: {e}");
        }
    }
}
