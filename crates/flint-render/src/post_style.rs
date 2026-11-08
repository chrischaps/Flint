//! Switching whole-frame styles at runtime (#369): what the script calls
//! `set_post_style`, `set_post_style_param` and `reset_post_style` do to
//! the renderer's [`PostProcessConfig`].
//!
//! [`entity_style`](crate::entity_style) edits an entity's component; these
//! edit the `[post_process.styles.<name>]` table instead, so they reach any
//! registered style: Kuwahara, invert, watercolor, the full-screen ASCII
//! look, and user styles (#274). The host hands the edited config back with
//! `SceneRenderer::set_post_process_config`, and the next frame builds a
//! newly switched-on style itself. A parameter changed every frame is a
//! uniform write in a pass that already exists: nothing is allocated.
//!
//! `blend` (0 to 1) is a setting every style takes: how much of its result
//! shows over what it read. Ramping it is a cross-fade into the style.

use crate::postprocess::PostProcessConfig;
use crate::stylize::{self, StyleDescriptor, StyleStage};

fn style(name: &str) -> Result<&'static StyleDescriptor, String> {
    stylize::find(name).ok_or_else(|| {
        format!(
            "no style named `{name}`; styles: {}",
            stylize::names().join(", ")
        )
    })
}

/// Switch style `name` on or off for the whole frame, keeping its settings.
pub fn set_post_style(
    config: &mut PostProcessConfig,
    name: &str,
    enabled: bool,
) -> Result<(), String> {
    let d = style(name)?;
    config.styles.set_enabled(d.name, enabled);
    Ok(())
}

/// Set one setting of style `name`: a parameter (a number, clamped to the
/// style's range), an option (a string, one of its choices when the list is
/// closed), a colour (`tint`: three numbers or `"#rrggbb"`), `enabled`
/// (true or false), `blend` (0 to 1), `order` (a number) or, for an HDR
/// style, `feeds_bloom` (true or false). Setting a parameter doesn't switch
/// the style on.
pub fn set_post_style_param(
    config: &mut PostProcessConfig,
    name: &str,
    param: &str,
    value: toml::Value,
) -> Result<(), String> {
    let d = style(name)?;
    let number = value
        .as_float()
        .or_else(|| value.as_integer().map(|i| i as f64))
        .filter(|n| n.is_finite());
    let wrong = |want: &str| {
        Err(format!(
            "`{name}` setting `{param}` takes {want}, not {}",
            value.type_str()
        ))
    };
    let styles = &mut config.styles;
    match param {
        "enabled" => match value.as_bool() {
            Some(on) => styles.set_enabled(d.name, on),
            None => return wrong("true or false"),
        },
        "blend" => match number {
            Some(n) => styles.set_blend(d.name, Some(n as f32)),
            None => return wrong("a number from 0 to 1"),
        },
        "order" => match number {
            Some(n) => styles.set_order(d.name, Some(n as f32)),
            None => return wrong("a number"),
        },
        "feeds_bloom" => match value.as_bool() {
            Some(_) if d.stage != StyleStage::Hdr => {
                return Err(format!(
                    "`{name}` is a display style, after bloom; only an hdr style can feed bloom"
                ))
            }
            Some(feeds) => styles.set_feeds_bloom(d.name, Some(feeds)),
            None => return wrong("true or false"),
        },
        _ => {
            if let Some(p) = d.params.iter().find(|p| p.name == param) {
                match number {
                    Some(n) => styles.set_param(d.name, p.name, (n as f32).clamp(p.min, p.max)),
                    None => return wrong("a number"),
                }
            } else if let Some(o) = d.options.iter().find(|o| o.name == param) {
                match value.as_str() {
                    Some(text) if o.refuses(text) => {
                        return Err(format!(
                            "`{name}` setting `{param}` has no value `{text}`; it takes: {}",
                            o.choices.join(", ")
                        ))
                    }
                    Some(text) => styles.set_option(d.name, o.name, text),
                    None => return wrong("a string"),
                }
            } else if let Some(color) = d.color_setting(param) {
                match stylize::color_from_toml(&value) {
                    Some(rgb) => {
                        let mut clamped = rgb;
                        for (v, p) in clamped.iter_mut().zip(color.channels) {
                            *v = v.clamp(p.min, p.max);
                        }
                        styles.set_color(d.name, color.base, clamped);
                    }
                    None => return wrong("a colour: [r, g, b] or \"#rrggbb\""),
                }
            } else {
                let known = d.setting_keys();
                return Err(format!(
                    "`{name}` has no setting `{param}`; it takes enabled, blend, {}",
                    if known.is_empty() {
                        "nothing else".to_string()
                    } else {
                        known.join(", ")
                    }
                ));
            }
        }
    }
    Ok(())
}

/// Put style `name` back to what `authored` (the scene's own config) says:
/// on or off, and every value, as the scene loaded it.
pub fn reset_post_style(
    config: &mut PostProcessConfig,
    authored: &PostProcessConfig,
    name: &str,
) -> Result<(), String> {
    let d = style(name)?;
    config.styles.restore(d.name, &authored.styles);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_clear_a_whole_frame_style() {
        let authored = PostProcessConfig::default();
        let mut config = authored.clone();
        let kuwahara = stylize::find("kuwahara").unwrap();
        assert!(!kuwahara.is_enabled(&config));

        set_post_style(&mut config, "kuwahara", true).unwrap();
        assert!(kuwahara.is_enabled(&config));
        set_post_style_param(&mut config, "kuwahara", "radius", 6_i64.into()).unwrap();
        assert_eq!(kuwahara.param(&config, "radius"), 6.0);
        // Clamped to the descriptor's range, as the F4 slider is
        set_post_style_param(&mut config, "kuwahara", "radius", 99.0.into()).unwrap();
        assert_eq!(kuwahara.param(&config, "radius"), 8.0);
        set_post_style_param(&mut config, "kuwahara", "blend", 0.25.into()).unwrap();
        assert_eq!(config.styles.blend("kuwahara"), 0.25);
        set_post_style_param(&mut config, "kuwahara", "blend", (-3.0).into()).unwrap();
        assert_eq!(config.styles.blend("kuwahara"), 0.0);
        set_post_style_param(&mut config, "kuwahara", "blend", 1.0.into()).unwrap();
        assert_eq!(config.styles.blend("kuwahara"), 1.0);
        assert_eq!(config.styles.get("kuwahara").unwrap().blend, None);

        // Off keeps the settings; on again finds them
        set_post_style(&mut config, "kuwahara", false).unwrap();
        assert!(!kuwahara.is_enabled(&config));
        set_post_style(&mut config, "kuwahara", true).unwrap();
        assert_eq!(kuwahara.param(&config, "radius"), 8.0);

        // Reset: back to the scene's, which never had it
        reset_post_style(&mut config, &authored, "kuwahara").unwrap();
        assert_eq!(config.styles, authored.styles);
    }

    #[test]
    fn reset_restores_what_the_scene_authored() {
        let mut authored = PostProcessConfig::default();
        authored.styles.set_enabled("invert", true);
        authored.styles.set_param("invert", "strength", 0.4);
        let mut config = authored.clone();
        set_post_style(&mut config, "invert", false).unwrap();
        set_post_style_param(&mut config, "invert", "strength", 1.0.into()).unwrap();
        reset_post_style(&mut config, &authored, "invert").unwrap();
        assert_eq!(config.styles, authored.styles);
    }

    #[test]
    fn options_colours_and_the_reserved_keys_check_their_values() {
        let mut config = PostProcessConfig::default();
        let ascii = stylize::find("ascii").unwrap();
        set_post_style_param(&mut config, "ascii", "ramp", "blocks".into()).unwrap();
        assert_eq!(ascii.option(&config, "ramp"), "blocks");
        set_post_style_param(&mut config, "ascii", "tint", "#ff8000".into()).unwrap();
        let tint = ascii.color(&config, "tint");
        assert!(
            (tint[0] - 1.0).abs() < 1e-3 && tint[2].abs() < 1e-3,
            "{tint:?}"
        );
        // A closed list refuses what it doesn't know, naming its choices
        let e = set_post_style_param(&mut config, "ascii", "scope", "planet".into()).unwrap_err();
        assert!(e.contains("screen"), "{e}");
        let e = set_post_style_param(&mut config, "ascii", "cell", "big".into()).unwrap_err();
        assert!(e.contains("a number"), "{e}");
        let e =
            set_post_style_param(&mut config, "invert", "feeds_bloom", true.into()).unwrap_err();
        assert!(e.contains("display style"), "{e}");
        set_post_style_param(&mut config, "kuwahara", "feeds_bloom", true.into()).unwrap();
        assert_eq!(config.styles.feeds_bloom("kuwahara"), Some(true));
        let e = set_post_style_param(&mut config, "kuwahara", "glow", 1.0.into()).unwrap_err();
        assert!(e.contains("radius"), "{e}");
        let e = set_post_style(&mut config, "watercolour", true).unwrap_err();
        assert!(e.contains("watercolor"), "{e}");
        // Nothing refused changed anything
        assert!(!ascii.is_enabled(&config));
    }

    #[test]
    fn a_blend_reads_from_the_scene_and_writes_back_as_toml() {
        let tables: std::collections::BTreeMap<String, toml::Table> =
            toml::from_str("[kuwahara]\nblend = 0.5\n[invert]\nblend = \"half\"\n").unwrap();
        let (styles, warnings) = stylize::StyleSettings::from_tables(&tables);
        assert_eq!(styles.blend("kuwahara"), 0.5);
        assert_eq!(styles.blend("invert"), 1.0);
        assert!(
            warnings.iter().any(|w| w.contains("invert.blend")),
            "{warnings:?}"
        );
        let config = PostProcessConfig {
            styles,
            ..Default::default()
        };
        let toml = stylize::style_toml(stylize::find("kuwahara").unwrap(), &config);
        assert!(toml.contains("blend = 0.5\n"), "{toml}");
    }
}
