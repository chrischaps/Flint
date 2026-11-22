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
//! `reveal` (0 to 1, #406) is another: how much of the screen the style
//! shows on, a ragged region (`reveal_shape`) with a drawn edge. Ramping it
//! tears the frame open into the style.

use crate::postprocess::PostProcessConfig;
use crate::style_mask::BLEND_RANGE;
use crate::stylize::{self, StyleDescriptor, StyleRegistry, StyleStage};

fn style<'s>(styles: &'s StyleRegistry, name: &str) -> Result<&'s StyleDescriptor, String> {
    styles.find(name).ok_or_else(|| {
        format!(
            "no style named `{name}`; styles: {}",
            styles.names().join(", ")
        )
    })
}

/// Switch style `name` on or off for the whole frame, keeping its settings.
pub fn set_post_style(
    config: &mut PostProcessConfig,
    styles: &StyleRegistry,
    name: &str,
    enabled: bool,
) -> Result<(), String> {
    let d = style(styles, name)?;
    config.styles.set_enabled(d.name, enabled);
    Ok(())
}

/// Set one setting of style `name`: a parameter (a number, clamped to the
/// style's range), an option (a string, one of its choices when the list is
/// closed), a colour (`tint`: three numbers or `"#rrggbb"`), `enabled`
/// (true or false), `blend` (0 to 1), `blend_source` (`"own"` or `"tear"`,
/// #746), `order` (a number), the reveal's
/// keys (#406: `reveal` 0 to 1, `reveal_shape`, `reveal_invert` (true or
/// false, #1180), `reveal_ink`,
/// `reveal_rim_color`, the `reveal_*` numbers) or, for an HDR style, `feeds_bloom` (true or
/// false). Setting a parameter doesn't switch the style on.
///
/// `Ok(Some(note))` when a number of a parameter, a colour or `blend` lay
/// outside its range and was clamped into it: what to tell the script's
/// author (#492), as [`ParamSet::clamped`](crate::entity_style::ParamSet).
pub fn set_post_style_param(
    config: &mut PostProcessConfig,
    styles: &StyleRegistry,
    name: &str,
    param: &str,
    value: toml::Value,
) -> Result<Option<String>, String> {
    let d = style(styles, name)?;
    let number = flint_core::toml_util::toml_f64(&value).filter(|n| n.is_finite());
    let mut notes: Vec<String> = Vec::new();
    let mut clamp = |p: &stylize::StyleParam, n: f64| {
        let (kept, note) = p.clamp(n);
        notes.extend(note);
        kept as f32
    };
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
            Some(n) => styles.set_blend(d.name, Some(clamp(&BLEND_RANGE, n))),
            None => return wrong("a number from 0 to 1"),
        },
        "order" => match number {
            Some(n) => styles.set_order(d.name, Some(n as f32)),
            None => return wrong("a number"),
        },
        "blend_source" => match value.as_str().and_then(stylize::blend_source) {
            Some(tear) => styles.set_follows_tear(d.name, tear),
            None => return wrong("\"own\" or \"tear\""),
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
        _ if stylize::reveal::is_key(param) => {
            // Clamped, as a parameter is
            styles
                .set_reveal(d.name, param, &value, false)
                .map_err(|e| format!("`{name}` setting {e}"))?;
        }
        _ => {
            if let Some(p) = d.params.iter().find(|p| p.name == param) {
                match number {
                    Some(n) => styles.set_param(d.name, p.name, clamp(p, n as f32 as f64)),
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
                            *v = clamp(p, *v as f64);
                        }
                        styles.set_color(d.name, color.base, clamped);
                    }
                    None => return wrong("a colour: [r, g, b] or \"#rrggbb\""),
                }
            } else {
                let known = d.setting_keys();
                return Err(format!(
                    "`{name}` has no setting `{param}`; it takes enabled, blend, reveal, {}",
                    if known.is_empty() {
                        "nothing else".to_string()
                    } else {
                        known.join(", ")
                    }
                ));
            }
        }
    }
    Ok((!notes.is_empty()).then(|| notes.join("; ")))
}

/// The number style `name`'s setting `param` has in `config` now, where a
/// tween of it starts (#406, `tween_post_style_param`): `blend`, `reveal`
/// (0 when the style has none, so a tween from there is a tear opening), a
/// `reveal_*` number, or a parameter. Not a number setting: an error naming
/// what it is.
pub fn post_style_number(
    config: &PostProcessConfig,
    styles: &StyleRegistry,
    name: &str,
    param: &str,
) -> Result<f32, String> {
    let d = style(styles, name)?;
    let styles = &config.styles;
    if param == "blend" {
        return Ok(styles.blend(d.name));
    }
    if param == "reveal" {
        return Ok(styles.reveal(d.name).and_then(|r| r.amount).unwrap_or(0.0));
    }
    if stylize::reveal::NUMBERS.iter().any(|n| n.key == param) {
        let reveal = styles
            .get(d.name)
            .map(|s| s.reveal.clone())
            .unwrap_or_default();
        return Ok(reveal.number(param));
    }
    if d.params.iter().any(|p| p.name == param) {
        return Ok(d.param(config, param));
    }
    Err(format!(
        "`{name}` setting `{param}` isn't a number to ease; numbers: blend, reveal, {}",
        d.params
            .iter()
            .map(|p| p.name)
            .chain(stylize::reveal::NUMBERS.iter().map(|n| n.key))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Put style `name` back to what `authored` (the scene's own config) says:
/// on or off, and every value, as the scene loaded it.
pub fn reset_post_style(
    config: &mut PostProcessConfig,
    styles: &StyleRegistry,
    authored: &PostProcessConfig,
    name: &str,
) -> Result<(), String> {
    let d = style(styles, name)?;
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
        let kuwahara = stylize::builtin("kuwahara").unwrap();
        assert!(!kuwahara.is_enabled(&config));

        set_post_style(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            "kuwahara",
            true,
        )
        .unwrap();
        assert!(kuwahara.is_enabled(&config));
        set_post_style_param(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            "kuwahara",
            "radius",
            6_i64.into(),
        )
        .unwrap();
        assert_eq!(kuwahara.param(&config, "radius"), 6.0);
        // Clamped to the descriptor's range, as the F4 slider is, and said
        // (#492); in range says nothing
        let note = set_post_style_param(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            "kuwahara",
            "radius",
            99.0.into(),
        )
        .unwrap();
        assert_eq!(kuwahara.param(&config, "radius"), 8.0);
        assert_eq!(
            note.as_deref(),
            Some("radius=99 is outside its range 1..8; clamped to 8")
        );
        let note = set_post_style_param(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            "kuwahara",
            "blend",
            0.25.into(),
        )
        .unwrap();
        assert_eq!(config.styles.blend("kuwahara"), 0.25);
        assert_eq!(note, None);
        let note = set_post_style_param(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            "kuwahara",
            "blend",
            (-3.0).into(),
        )
        .unwrap();
        assert_eq!(config.styles.blend("kuwahara"), 0.0);
        assert!(note
            .unwrap()
            .starts_with("blend=-3 is outside its range 0..1"));
        set_post_style_param(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            "kuwahara",
            "blend",
            1.0.into(),
        )
        .unwrap();
        assert_eq!(config.styles.blend("kuwahara"), 1.0);
        assert_eq!(config.styles.get("kuwahara").unwrap().blend, None);

        // Off keeps the settings; on again finds them
        set_post_style(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            "kuwahara",
            false,
        )
        .unwrap();
        assert!(!kuwahara.is_enabled(&config));
        set_post_style(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            "kuwahara",
            true,
        )
        .unwrap();
        assert_eq!(kuwahara.param(&config, "radius"), 8.0);

        // Reset: back to the scene's, which never had it
        reset_post_style(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            &authored,
            "kuwahara",
        )
        .unwrap();
        assert_eq!(config.styles, authored.styles);
    }

    #[test]
    fn reset_restores_what_the_scene_authored() {
        let mut authored = PostProcessConfig::default();
        authored.styles.set_enabled("invert", true);
        authored.styles.set_param("invert", "strength", 0.4);
        let mut config = authored.clone();
        set_post_style(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            "invert",
            false,
        )
        .unwrap();
        set_post_style_param(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            "invert",
            "strength",
            1.0.into(),
        )
        .unwrap();
        reset_post_style(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            &authored,
            "invert",
        )
        .unwrap();
        assert_eq!(config.styles, authored.styles);
    }

    #[test]
    fn options_colours_and_the_reserved_keys_check_their_values() {
        let mut config = PostProcessConfig::default();
        let ascii = stylize::builtin("ascii").unwrap();
        set_post_style_param(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            "ascii",
            "ramp",
            "blocks".into(),
        )
        .unwrap();
        assert_eq!(ascii.option(&config, "ramp"), "blocks");
        set_post_style_param(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            "ascii",
            "tint",
            "#ff8000".into(),
        )
        .unwrap();
        let tint = ascii.color(&config, "tint");
        assert!(
            (tint[0] - 1.0).abs() < 1e-3 && tint[2].abs() < 1e-3,
            "{tint:?}"
        );
        // A closed list refuses what it doesn't know, naming its choices
        let e = set_post_style_param(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            "ascii",
            "scope",
            "planet".into(),
        )
        .unwrap_err();
        assert!(e.contains("screen"), "{e}");
        let e = set_post_style_param(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            "ascii",
            "cell",
            "big".into(),
        )
        .unwrap_err();
        assert!(e.contains("a number"), "{e}");
        let e = set_post_style_param(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            "invert",
            "feeds_bloom",
            true.into(),
        )
        .unwrap_err();
        assert!(e.contains("display style"), "{e}");
        set_post_style_param(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            "kuwahara",
            "feeds_bloom",
            true.into(),
        )
        .unwrap();
        assert_eq!(config.styles.feeds_bloom("kuwahara"), Some(true));
        let e = set_post_style_param(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            "kuwahara",
            "glow",
            1.0.into(),
        )
        .unwrap_err();
        assert!(e.contains("radius"), "{e}");
        let e = set_post_style(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            "watercolour",
            true,
        )
        .unwrap_err();
        assert!(e.contains("watercolor"), "{e}");
        // Nothing refused changed anything
        assert!(!ascii.is_enabled(&config));
    }

    #[test]
    fn a_blend_reads_from_the_scene_and_writes_back_as_toml() {
        let tables: std::collections::BTreeMap<String, toml::Table> =
            toml::from_str("[kuwahara]\nblend = 0.5\n[invert]\nblend = \"half\"\n").unwrap();
        let (styles, warnings) =
            stylize::StyleSettings::from_tables(&tables, &crate::stylize::StyleRegistry::new());
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
        let toml = stylize::style_toml(stylize::builtin("kuwahara").unwrap(), &config);
        assert!(toml.contains("blend = 0.5\n"), "{toml}");
    }
}
