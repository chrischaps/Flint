//! `--style name[:key=value,...]` (#271): switch any registered style on,
//! and set its parameters and options, from one command-line word, so a
//! style can be previewed on any scene without editing the scene.
//!
//! ```text
//! --style ascii                          the style at its defaults
//! --style ascii:cell=12,ramp=blocks      a parameter (number) and an option (text)
//! --style invert:strength=0.5
//! --style kuwahara:radius=6              Kuwahara's own kuwahara_* fields
//! --style ascii:off                      turn off a style the scene has on
//! ```
//!
//! Keys are checked against the style's descriptor: a number for each of
//! its [`params`](super::StyleDescriptor::params), text for each of its
//! [`options`](super::StyleDescriptor::options) (one of its `choices`
//! when it lists them), and `enabled` / `off`. Values outside a
//! parameter's F4 range are taken as written, as a scene's are.

use super::{find, names, registry, StyleDescriptor};
use crate::postprocess::PostProcessConfig;

/// Kuwahara keeps its settings in `kuwahara_*` fields, not the style
/// table (it predates it); these are the keys `--style kuwahara:` takes.
const KUWAHARA_KEYS: [&str; 4] = ["radius", "sharpness", "hardness", "anisotropy"];

/// One parsed `--style` word.
#[derive(Debug, Clone, PartialEq)]
pub struct StyleSpec {
    pub name: String,
    pub enabled: bool,
    /// `key=value` pairs in the order given.
    pub settings: Vec<(String, String)>,
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
                "the registered styles and their keys:\n{}",
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
            check(descriptor, key, value)?;
            parsed.settings.push((key.to_string(), value.to_string()));
        }
        Ok(parsed)
    }

    /// Write this spec into `config`: the style on (or off) with its
    /// settings, over whatever the scene said.
    pub fn apply(&self, config: &mut PostProcessConfig) {
        if self.name == "kuwahara" {
            config.kuwahara_enabled = self.enabled;
            for (key, value) in &self.settings {
                let n: f32 = value.parse().unwrap_or_default();
                match key.as_str() {
                    "radius" => config.kuwahara_radius = n.round().max(1.0) as u32,
                    "sharpness" => config.kuwahara_sharpness = n,
                    "hardness" => config.kuwahara_hardness = n,
                    "anisotropy" => config.kuwahara_anisotropy = n,
                    _ => {}
                }
            }
            return;
        }
        let descriptor = find(&self.name).expect("checked by parse");
        config.styles.set_enabled(&self.name, self.enabled);
        for (key, value) in &self.settings {
            if descriptor.options.iter().any(|o| o.name == key) {
                config.styles.set_option(&self.name, key, value);
            } else if let Ok(n) = value.parse::<f32>() {
                config.styles.set_param(&self.name, key, n);
            }
        }
    }
}

/// Parse `spec` and write it into `config`.
pub fn apply_style_spec(config: &mut PostProcessConfig, spec: &str) -> Result<(), String> {
    StyleSpec::parse(spec)?.apply(config);
    Ok(())
}

/// One line per registered style with the keys `--style` takes, for help
/// text and error messages.
pub fn style_spec_help() -> String {
    registry()
        .iter()
        .map(|d| format!("{}: {}", d.name, known_keys(d)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn check(descriptor: &StyleDescriptor, key: &str, value: &str) -> Result<(), String> {
    let name = descriptor.name;
    if name == "kuwahara" {
        if !KUWAHARA_KEYS.contains(&key) {
            return Err(format!(
                "kuwahara has no key '{key}'; it takes: {}",
                known_keys(descriptor)
            ));
        }
        return number(name, key, value);
    }
    if let Some(option) = descriptor.options.iter().find(|o| o.name == key) {
        if !option.choices.is_empty() && !option.choices.contains(&value) {
            return Err(format!(
                "{name}: {key} must be one of {}",
                option.choices.join(", ")
            ));
        }
        return Ok(());
    }
    if descriptor.params.iter().any(|p| p.name == key) {
        return number(name, key, value);
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
    let mut keys: Vec<String> = if descriptor.name == "kuwahara" {
        KUWAHARA_KEYS.iter().map(|k| k.to_string()).collect()
    } else {
        descriptor
            .params
            .iter()
            .map(|p| format!("{}={}", p.name, p.default))
            .chain(descriptor.options.iter().map(|o| {
                if o.choices.is_empty() {
                    format!("{}=<text>", o.name)
                } else {
                    format!("{}={}", o.name, o.choices.join("|"))
                }
            }))
            .collect()
    };
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

    #[test]
    fn params_are_numbers_and_options_are_text() {
        let mut config = PostProcessConfig::default();
        apply_style_spec(&mut config, "ascii:cell=12, ramp=blocks,color=tint").unwrap();
        assert_eq!(config.styles.param("ascii", "cell"), Some(12.0));
        assert_eq!(config.styles.option("ascii", "ramp"), Some("blocks"));
        assert_eq!(config.styles.option("ascii", "color"), Some("tint"));
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
    fn kuwahara_writes_its_own_fields() {
        let mut config = PostProcessConfig::default();
        apply_style_spec(&mut config, "kuwahara:radius=6,anisotropy=0.5").unwrap();
        assert!(config.kuwahara_enabled);
        assert_eq!(config.kuwahara_radius, 6);
        assert_eq!(config.kuwahara_anisotropy, 0.5);
        assert!(!config.styles.is_enabled("kuwahara"));
    }

    #[test]
    fn mistakes_name_what_would_have_worked() {
        let unknown = StyleSpec::parse("watercolour").unwrap_err();
        assert!(unknown.contains("ascii"), "{unknown}");
        let key = StyleSpec::parse("ascii:size=3").unwrap_err();
        assert!(key.contains("cell=8"), "{key}");
        let choice = StyleSpec::parse("ascii:color=purple").unwrap_err();
        assert!(choice.contains("source"), "{choice}");
        let nan = StyleSpec::parse("invert:strength=lots").unwrap_err();
        assert!(nan.contains("number"), "{nan}");
        assert!(StyleSpec::parse("ascii:cell").is_err());
        assert!(StyleSpec::parse("kuwahara:cell=3").is_err());
    }

    #[test]
    fn every_registered_style_parses_bare() {
        for d in registry() {
            StyleSpec::parse(d.name).unwrap();
        }
        assert!(style_spec_help().lines().count() == registry().len());
    }
}
