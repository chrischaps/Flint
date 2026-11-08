//! Setting an entity's style at runtime (#269): what the script calls
//! `set_entity_style`, `set_style_param` and `clear_entity_style` do to the
//! world.
//!
//! A per-entity style is nothing but a component (`ascii_style`, #266), and
//! the renderer reads every styled entity's components again each frame
//! ([`style_mask`](crate::style_mask)), so these only edit components. A
//! parameter changed every frame is a new row in the style's variant table,
//! which the pass uploads into the uniform buffer it already has: no
//! buffer, texture or bind group is made for it.
//!
//! `blend` (#278) is a setting every per-entity style takes: how much of
//! the style the entity is drawn with, 0 to 1. It rides in the style mask
//! beside the entity's id, not in its variant, so ramping it every frame
//! keeps one id and makes no new variant row.
//!
//! Only styles with a component of their own can be set on an entity.
//! Whole-frame styles (Kuwahara, invert) live in the scene's
//! `[post_process.styles]` table and refuse with a message naming the ones
//! that can.

use crate::postprocess::PostProcessConfig;
use crate::style_mask::{self, StyleVariant, BLEND_FIELD, STYLE_MASK_COMPONENT};
use crate::stylize::{self, StyleDescriptor, StyleStage};
use flint_core::EntityId;
use flint_ecs::FlintWorld;

/// The registered styles that can be set on an entity, by name.
pub fn per_entity_styles() -> Vec<&'static str> {
    stylize::registry()
        .iter()
        .filter(|d| d.component.is_some())
        .map(|d| d.name)
        .collect()
}

fn per_entity(style: &str) -> Result<&'static StyleDescriptor, String> {
    let list = || per_entity_styles().join(", ");
    match stylize::find(style) {
        Some(d) if d.component.is_some() => Ok(d),
        Some(_) => Err(format!(
            "`{style}` is a whole-frame style (set it in [post_process.styles.{style}]); \
             styles an entity can take: {}",
            list()
        )),
        None => Err(format!(
            "no style named `{style}`; styles an entity can take: {}",
            list()
        )),
    }
}

/// The per-entity style `id` carries itself (not one inherited from an
/// ancestor), if any.
pub fn entity_style(world: &FlintWorld, id: EntityId) -> Option<&'static str> {
    stylize::registry().iter().find_map(|d| {
        let c = d.component?;
        world.get_component(id, c).map(|_| d.name)
    })
}

/// Draw `id` (and its descendants) in `style`. A style it already had keeps
/// its settings and is switched back on; any other per-entity style, or a
/// `style_mask`, comes off, so an entity has one style at a time.
pub fn set_entity_style(world: &mut FlintWorld, id: EntityId, style: &str) -> Result<(), String> {
    let d = per_entity(style)?;
    let component = d.component.expect("per_entity checked it");
    if world.get_components(id).is_none() {
        return Err(format!("no entity {}", id.0));
    }
    for other in stylize::registry().iter().filter_map(|o| o.component) {
        if other != component {
            let _ = world.remove_component(id, other);
        }
    }
    let _ = world.remove_component(id, STYLE_MASK_COMPONENT);
    if world.get_component(id, component).is_some() {
        world
            .set_field(id, component, "enabled", true.into())
            .map_err(|e| e.to_string())
    } else {
        world
            .set_component(id, component, toml::Value::Table(toml::Table::new()))
            .map_err(|e| e.to_string())
    }
}

/// Set one setting of `id`'s style: a parameter (a number, clamped to the
/// style's range), an option (a string), `enabled` (a bool), `blend` (a
/// number, clamped to 0..1; #278), or a colour (`tint`: a 3-array or a
/// `"#rrggbb"` string, stored as the array; #315). Returns the style.
pub fn set_style_param(
    world: &mut FlintWorld,
    id: EntityId,
    param: &str,
    value: toml::Value,
) -> Result<&'static str, String> {
    let style = entity_style(world, id).ok_or_else(|| {
        format!(
            "entity {} has no style of its own; call set_entity_style first",
            id.0
        )
    })?;
    let d = stylize::find(style).expect("entity_style found it registered");
    let component = d.component.expect("entity_style found its component");
    let number = |v: &toml::Value| v.as_float().or_else(|| v.as_integer().map(|i| i as f64));
    let wrong = |want: &str| {
        Err(format!(
            "`{style}` setting `{param}` takes {want}, not {}",
            value.type_str()
        ))
    };
    let stored = if param == "enabled" {
        match value.as_bool() {
            Some(b) => b.into(),
            None => return wrong("true or false"),
        }
    } else if param == BLEND_FIELD {
        match number(&value) {
            Some(n) => toml::Value::Float(n.clamp(0.0, 1.0)),
            None => return wrong("a number from 0 to 1"),
        }
    } else if let Some(p) = d.params.iter().find(|p| p.name == param) {
        match number(&value) {
            Some(n) => toml::Value::Float(n.clamp(p.min as f64, p.max as f64)),
            None => return wrong("a number"),
        }
    } else if d.options.iter().any(|o| o.name == param) {
        match value.as_str() {
            Some(_) => value,
            None => return wrong("a string"),
        }
    } else if let Some(color) = d.color_setting(param) {
        // A colour (#315): three numbers or "#rrggbb", stored as the array
        // the component's schema types it as, each channel in its range
        match stylize::color_from_toml(&value) {
            Some(rgb) => toml::Value::Array(
                rgb.iter()
                    .zip(color.channels)
                    .map(|(v, p)| toml::Value::Float(v.clamp(p.min, p.max) as f64))
                    .collect(),
            ),
            None => return wrong("a colour: [r, g, b] or \"#rrggbb\""),
        }
    } else {
        let known = d.setting_keys();
        return Err(format!(
            "`{style}` has no setting `{param}`; it takes enabled, blend, {}",
            known.join(", ")
        ));
    };
    world
        .set_field(id, component, param, stored)
        .map_err(|e| e.to_string())?;
    Ok(style)
}

/// Where a tween of one of `id`'s style settings starts and ends (#370):
/// its style, then the current value and the target, each one number or
/// a colour's three channels (`tint`). The current value is the entity's
/// own field, else the scene's `[post_process.styles.<name>]` table in
/// `config`, else the style's default (`blend`: 1). The target is checked
/// and clamped as [`set_style_param`] would. Only numbers, `blend` and
/// colours ease; options and `enabled` refuse with a message.
pub fn tween_endpoints(
    world: &FlintWorld,
    id: EntityId,
    param: &str,
    target: &toml::Value,
    config: &crate::postprocess::PostProcessConfig,
) -> Result<(&'static str, Vec<f64>, Vec<f64>), String> {
    let style = entity_style(world, id).ok_or_else(|| {
        format!(
            "entity {} has no style of its own; call set_entity_style first",
            id.0
        )
    })?;
    let d = stylize::find(style).expect("entity_style found it registered");
    let component = d.component.expect("entity_style found its component");
    let number = |v: &toml::Value| v.as_float().or_else(|| v.as_integer().map(|i| i as f64));
    let own = world
        .get_component(id, component)
        .and_then(|c| c.get(param).cloned());
    let wrong = |want: &str| {
        Err(format!(
            "`{style}` setting `{param}` eases to {want}, not {}",
            target.type_str()
        ))
    };
    if param == BLEND_FIELD {
        let Some(to) = number(target) else {
            return wrong("a number from 0 to 1");
        };
        let from = own.as_ref().and_then(number).unwrap_or(1.0);
        return Ok((style, vec![from.clamp(0.0, 1.0)], vec![to.clamp(0.0, 1.0)]));
    }
    if let Some(p) = d.params.iter().find(|p| p.name == param) {
        let Some(to) = number(target) else {
            return wrong("a number");
        };
        let (lo, hi) = (p.min as f64, p.max as f64);
        let from = own
            .as_ref()
            .and_then(number)
            .unwrap_or_else(|| d.param(config, param) as f64);
        return Ok((style, vec![from.clamp(lo, hi)], vec![to.clamp(lo, hi)]));
    }
    if let Some(color) = d.color_setting(param) {
        let Some(to) = stylize::color_from_toml(target) else {
            return wrong("a colour: [r, g, b] or \"#rrggbb\"");
        };
        let from = own
            .as_ref()
            .and_then(stylize::color_from_toml)
            .unwrap_or_else(|| d.color(config, param));
        let clamp = |rgb: [f32; 3]| -> Vec<f64> {
            rgb.iter()
                .zip(color.channels)
                .map(|(v, p)| v.clamp(p.min, p.max) as f64)
                .collect()
        };
        return Ok((style, clamp(from), clamp(to)));
    }
    if param == "enabled" || d.options.iter().any(|o| o.name == param) {
        return Err(format!(
            "`{style}` setting `{param}` can't ease (it isn't a number or a colour); \
             set it with set_style_param"
        ));
    }
    Err(format!(
        "`{style}` has no setting `{param}`; it takes enabled, blend, {}",
        d.setting_keys().join(", ")
    ))
}

/// Take `id`'s own style off (any per-entity style component and any
/// `style_mask`). Returns whether it had one. A style it inherits from an
/// ancestor, or the scene's whole-frame style, still reaches it; to keep it
/// smooth under those, set `enabled` to false instead.
pub fn clear_entity_style(world: &mut FlintWorld, id: EntityId) -> bool {
    let mut had = false;
    for c in stylize::registry()
        .iter()
        .filter_map(|d| d.component)
        .chain([STYLE_MASK_COMPONENT])
    {
        had |= matches!(world.remove_component(id, c), Ok(Some(_)));
    }
    had
}

// ---------------------------------------------------------------------------
// Reading it back (#372)
// ---------------------------------------------------------------------------

/// The style an entity is drawn in, as [`drawn_style`] finds it.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawnStyle {
    /// The style's name (`ascii`, `kuwahara`, or whatever a `style_mask`
    /// names).
    pub style: String,
    /// The variant's name, the one the mask's id stands for
    /// (`ascii{ramp=blocks}`, `ascii@barrel`); for a whole-frame style,
    /// the style's name.
    pub variant: String,
    /// The entity the style comes from: the entity itself, or the ancestor
    /// it inherits from. `None` for the scene's whole-frame style.
    pub root: Option<EntityId>,
    /// The settings the root's style component gives (empty for a
    /// whole-frame style), which [`StyleVariant::param`] falls back from.
    pub settings: StyleVariant,
}

/// What `id` is drawn in under `config` (#372): its own style or the one it
/// inherits from its nearest styled ancestor (a style component or a
/// `style_mask`, the same rule the mask is drawn by), else the scene's
/// whole-frame style. Where several whole-frame styles run, the last in the
/// chain (display after HDR, then `order`) is the one named: it draws over
/// the rest. An entity opted out (`enabled = false` on its style component)
/// isn't drawn in that style, even where it covers the frame; a
/// `style_mask` opt-out only takes it out of the per-entity style.
///
/// `None` when nothing styles it, or post-processing is off.
pub fn drawn_style(
    world: &FlintWorld,
    config: &PostProcessConfig,
    id: EntityId,
) -> Option<DrawnStyle> {
    if !config.enabled || world.get_components(id).is_none() {
        return None;
    }
    let mut opted_out: Option<String> = None;
    if let Some(root) = style_mask::style_root(world, id) {
        if let Some((variant, settings)) = style_mask::request(world, config, root) {
            if !settings.off {
                return Some(DrawnStyle {
                    style: settings.style.clone(),
                    variant,
                    root: Some(root),
                    settings,
                });
            }
            opted_out = Some(settings.style);
        }
    }
    [StyleStage::Hdr, StyleStage::Display]
        .into_iter()
        .flat_map(|stage| stylize::stage_chain(stage, config))
        .map(|(d, _)| d)
        .rfind(|d| {
            d.covers_frame(config) && opted_out.as_deref() != Some(d.name)
        })
        .map(|d| DrawnStyle {
            style: d.name.to_string(),
            variant: d.name.to_string(),
            root: None,
            settings: StyleVariant::plain(d.name),
        })
}

/// The value setting `param` of `id`'s style has in effect (#372): the
/// entity's (or the ancestor's it inherits), else the scene's
/// `[post_process.styles.<name>]` table, else the style's default. A
/// parameter is a float, an option a string, a colour (`tint`) a 3-array,
/// `blend` the root's 0..1 (1 for a whole-frame style) and `enabled` true.
/// `None` when the entity is drawn in no style, the style isn't registered,
/// or it has no such setting.
pub fn style_setting(
    world: &FlintWorld,
    config: &PostProcessConfig,
    id: EntityId,
    param: &str,
) -> Option<toml::Value> {
    let drawn = drawn_style(world, config, id)?;
    let d = stylize::find(&drawn.style)?;
    let v = &drawn.settings;
    if param == "enabled" {
        return Some(true.into());
    }
    if param == BLEND_FIELD {
        let blend = drawn
            .root
            .map_or(1.0, |r| style_mask::requested_blend(world, r));
        return Some(toml::Value::Float(blend as f64));
    }
    if d.params.iter().any(|p| p.name == param) {
        return Some(toml::Value::Float(v.param(d, config, param) as f64));
    }
    if d.options.iter().any(|o| o.name == param) {
        return Some(v.option(d, config, param).into());
    }
    let color = d.color_setting(param)?;
    Some(toml::Value::Array(
        color
            .channels
            .iter()
            .map(|p| toml::Value::Float(v.param(d, config, p.name) as f64))
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::postprocess::PostProcessConfig;
    use crate::style_mask::{resolve, StyleIds};
    use std::collections::HashMap;

    fn ids(world: &FlintWorld) -> (StyleIds, HashMap<EntityId, u16>) {
        let mut ids = StyleIds::default();
        let mut out = HashMap::new();
        resolve(world, &PostProcessConfig::default(), &mut ids, &mut out);
        (ids, out)
    }

    #[test]
    fn set_then_clear_switches_an_entity_in_and_out() {
        let mut world = FlintWorld::new();
        let barrel = world.spawn("barrel").unwrap();
        assert!(ids(&world).1.is_empty());

        set_entity_style(&mut world, barrel, "ascii").unwrap();
        assert_eq!(entity_style(&world, barrel), Some("ascii"));
        let (table, out) = ids(&world);
        assert_eq!(out.get(&barrel).copied(), table.id("ascii"));

        assert!(clear_entity_style(&mut world, barrel));
        assert!(
            !clear_entity_style(&mut world, barrel),
            "nothing left to clear"
        );
        assert_eq!(entity_style(&world, barrel), None);
        assert!(ids(&world).1.is_empty());
        assert!(world.entities_with_component("ascii_style").is_empty());
    }

    #[test]
    fn params_become_the_entitys_variant() {
        let mut world = FlintWorld::new();
        let barrel = world.spawn("barrel").unwrap();
        set_entity_style(&mut world, barrel, "ascii").unwrap();
        set_style_param(&mut world, barrel, "cell", 12_i64.into()).unwrap();
        set_style_param(&mut world, barrel, "ramp", "blocks".into()).unwrap();
        set_style_param(
            &mut world,
            barrel,
            "tint",
            toml::Value::Array(vec![1.0.into(), 0_i64.into(), 0.5.into()]),
        )
        .unwrap();
        // Out of range is clamped to the style's own range
        set_style_param(&mut world, barrel, "glow", 9.0.into()).unwrap();
        let (table, out) = ids(&world);
        let v = table.variant(out[&barrel]).unwrap();
        assert_eq!(v.params["cell"], 12.0);
        assert_eq!(v.options["ramp"], "blocks");
        assert_eq!(v.params["tint_g"], 0.0);
        assert_eq!(v.params["tint_b"], 0.5);
        let max_glow = stylize::find("ascii")
            .unwrap()
            .params
            .iter()
            .find(|p| p.name == "glow")
            .unwrap()
            .max;
        assert_eq!(v.params["glow"], max_glow);

        // Switching off is an opt-out, and on again keeps the settings
        set_style_param(&mut world, barrel, "enabled", false.into()).unwrap();
        let (table, out) = ids(&world);
        assert!(table.variant(out[&barrel]).unwrap().off);
        assert!(!table.requests("ascii"));
        set_entity_style(&mut world, barrel, "ascii").unwrap();
        let (table, out) = ids(&world);
        assert_eq!(table.variant(out[&barrel]).unwrap().params["cell"], 12.0);
    }

    #[test]
    fn mistakes_say_what_would_have_worked() {
        let mut world = FlintWorld::new();
        let barrel = world.spawn("barrel").unwrap();
        let e = set_style_param(&mut world, barrel, "cell", 8.0.into()).unwrap_err();
        assert!(e.contains("set_entity_style first"), "{e}");
        let e = set_entity_style(&mut world, barrel, "kuwahara").unwrap_err();
        assert!(e.contains("whole-frame") && e.contains("ascii"), "{e}");
        let e = set_entity_style(&mut world, barrel, "watercolour").unwrap_err();
        assert!(e.contains("no style named"), "{e}");
        set_entity_style(&mut world, barrel, "ascii").unwrap();
        let e = set_style_param(&mut world, barrel, "cell", "big".into()).unwrap_err();
        assert!(e.contains("a number"), "{e}");
        let e = set_style_param(&mut world, barrel, "colour", "tint".into()).unwrap_err();
        assert!(e.contains("no setting") && e.contains("color"), "{e}");
        // A colour is named once, by its key, not its channels (#315)
        assert!(e.contains("tint") && !e.contains("tint_r"), "{e}");
        let e = set_style_param(&mut world, barrel, "tint", "green".into()).unwrap_err();
        assert!(e.contains("#rrggbb"), "{e}");
        // A failed edit leaves the component as it was
        assert_eq!(
            world.get_component(barrel, "ascii_style"),
            Some(&toml::Value::Table(toml::Table::new()))
        );
    }

    #[test]
    fn blend_rides_beside_the_id_not_in_the_variant() {
        use crate::style_mask::resolve_blended;
        let mut world = FlintWorld::new();
        let barrel = world.spawn("barrel").unwrap();
        let hoop = world.spawn("hoop").unwrap();
        world.set_parent(hoop, barrel).unwrap();
        let bench = world.spawn("bench").unwrap();
        set_entity_style(&mut world, barrel, "ascii").unwrap();
        set_entity_style(&mut world, bench, "ascii").unwrap();
        let blended = |world: &FlintWorld| {
            let (mut ids, mut out, mut blends) =
                (StyleIds::default(), HashMap::new(), HashMap::new());
            resolve_blended(
                world,
                &PostProcessConfig::default(),
                &mut ids,
                &mut out,
                &mut blends,
            );
            (ids, out, blends)
        };
        let (table, out, blends) = blended(&world);
        assert!(blends.is_empty(), "fully styled entities carry no blend");
        let id = out[&barrel];

        // A ramp: every step keeps the one shared variant
        for step in 0..=4 {
            let b = step as f64 / 4.0;
            set_style_param(&mut world, barrel, "blend", b.into()).unwrap();
            let (now, out, blends) = blended(&world);
            assert_eq!(now, table, "blend {b} made a new variant");
            assert_eq!(out[&barrel], id);
            assert_eq!(out[&bench], id, "the bench shares the barrel's id");
            if b < 1.0 {
                assert_eq!(blends[&barrel], b as f32);
                assert_eq!(blends[&hoop], b as f32, "a child fades with its root");
            } else {
                assert!(blends.is_empty());
            }
            assert!(!blends.contains_key(&bench));
        }
        // Clamped, and it must be a number
        set_style_param(&mut world, barrel, "blend", 3_i64.into()).unwrap();
        assert!(blended(&world).2.is_empty());
        let e = set_style_param(&mut world, barrel, "blend", "half".into()).unwrap_err();
        assert!(e.contains("0 to 1"), "{e}");
        // A style_mask's blend counts too
        let mut mask = toml::Table::new();
        mask.insert("style".into(), "ascii".into());
        mask.insert("blend".into(), 0.25.into());
        world
            .set_component(bench, STYLE_MASK_COMPONENT, mask.into())
            .unwrap();
        world.remove_component(bench, "ascii_style").unwrap();
        assert_eq!(blended(&world).2[&bench], 0.25);
    }

    #[test]
    fn setting_a_style_replaces_a_style_mask() {
        let mut world = FlintWorld::new();
        let bench = world.spawn("bench").unwrap();
        let mut mask = toml::Table::new();
        mask.insert("style".into(), "default".into());
        world
            .set_component(bench, STYLE_MASK_COMPONENT, mask.into())
            .unwrap();
        set_entity_style(&mut world, bench, "ascii").unwrap();
        assert!(world.get_component(bench, STYLE_MASK_COMPONENT).is_none());
        assert_eq!(entity_style(&world, bench), Some("ascii"));
    }

    fn drawn(world: &FlintWorld, config: &PostProcessConfig, id: EntityId) -> Option<String> {
        drawn_style(world, config, id).map(|d| d.style)
    }

    #[test]
    fn reads_back_own_inherited_and_whole_frame_styles() {
        let mut world = FlintWorld::new();
        let barrel = world.spawn("barrel").unwrap();
        let hoop = world.spawn("hoop").unwrap();
        let bench = world.spawn("bench").unwrap();
        world.set_parent(hoop, barrel).unwrap();
        let mut config = PostProcessConfig::default();
        assert_eq!(drawn(&world, &config, barrel), None);

        set_entity_style(&mut world, barrel, "ascii").unwrap();
        set_style_param(&mut world, barrel, "ramp", "blocks".into()).unwrap();
        let own = drawn_style(&world, &config, barrel).unwrap();
        assert_eq!(own.variant, "ascii{ramp=blocks}");
        assert_eq!(own.root, Some(barrel));
        // The hoop inherits the barrel's style and its settings
        let inherited = drawn_style(&world, &config, hoop).unwrap();
        assert_eq!(inherited.root, Some(barrel));
        assert_eq!(inherited.variant, own.variant);
        assert_eq!(drawn(&world, &config, bench), None);

        // The scene's whole-frame style reaches the rest; an opt-out
        // keeps its entity out of it, and a scope of entities draws nothing
        config.styles.set_enabled("kuwahara", true);
        assert_eq!(drawn(&world, &config, bench).as_deref(), Some("kuwahara"));
        config.styles.set_enabled("ascii", true);
        assert_eq!(drawn(&world, &config, bench).as_deref(), Some("ascii"));
        set_style_param(&mut world, barrel, "enabled", false.into()).unwrap();
        assert_eq!(drawn(&world, &config, hoop).as_deref(), Some("kuwahara"));
        config.styles.set_option("ascii", "scope", "entities");
        config.styles.set_enabled("kuwahara", false);
        assert_eq!(drawn(&world, &config, bench), None);
        config.enabled = false;
        set_entity_style(&mut world, barrel, "ascii").unwrap();
        assert_eq!(drawn(&world, &config, barrel), None, "post is off");
    }

    #[test]
    fn a_setting_reads_entity_then_scene_then_default() {
        let mut world = FlintWorld::new();
        let barrel = world.spawn("barrel").unwrap();
        let hoop = world.spawn("hoop").unwrap();
        world.set_parent(hoop, barrel).unwrap();
        let mut config = PostProcessConfig::default();
        let read = |world: &FlintWorld, config: &PostProcessConfig, p: &str| {
            style_setting(world, config, hoop, p)
        };
        assert_eq!(read(&world, &config, "cell"), None, "no style");

        set_entity_style(&mut world, barrel, "ascii").unwrap();
        assert_eq!(read(&world, &config, "cell"), Some(8.0.into()), "default");
        config.styles.set_param("ascii", "cell", 10.0);
        assert_eq!(read(&world, &config, "cell"), Some(10.0.into()), "scene");
        set_style_param(&mut world, barrel, "cell", 99_i64.into()).unwrap();
        assert_eq!(read(&world, &config, "cell"), Some(32.0.into()), "clamped");
        set_style_param(&mut world, barrel, "ramp", "blocks".into()).unwrap();
        assert_eq!(read(&world, &config, "ramp"), Some("blocks".into()));
        set_style_param(&mut world, barrel, "tint", "#ff8000".into()).unwrap();
        let tint = read(&world, &config, "tint").unwrap();
        let tint: Vec<f64> = tint
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_float().unwrap())
            .collect();
        assert_eq!(tint.len(), 3);
        assert!(tint[0] > tint[1] && tint[1] > tint[2], "{tint:?}");
        assert_eq!(read(&world, &config, "blend"), Some(1.0.into()));
        set_style_param(&mut world, barrel, "blend", 0.25.into()).unwrap();
        assert_eq!(read(&world, &config, "blend"), Some(0.25.into()));
        assert_eq!(read(&world, &config, "enabled"), Some(true.into()));
        assert_eq!(read(&world, &config, "colour"), None, "no such setting");
    }
}
