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
//! Only styles with a component of their own can be set on an entity.
//! Whole-frame styles (Kuwahara, invert) live in the scene's
//! `[post_process.styles]` table and refuse with a message naming the ones
//! that can.

use crate::style_mask::STYLE_MASK_COMPONENT;
use crate::stylize::{self, StyleDescriptor};
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
/// style's range), an option (a string), `enabled` (a bool), or a 3-array
/// for a parameter split into `_r`/`_g`/`_b` (`tint`). Returns the style.
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
    } else if ["r", "g", "b"]
        .iter()
        .all(|c| d.params.iter().any(|p| p.name == format!("{param}_{c}")))
    {
        match value
            .as_array()
            .map(|a| a.iter().map(number).collect::<Option<Vec<_>>>())
        {
            Some(Some(rgb)) if rgb.len() == 3 => {
                toml::Value::Array(rgb.into_iter().map(toml::Value::Float).collect())
            }
            _ => return wrong("an array of three numbers"),
        }
    } else {
        let mut known: Vec<&str> = d.params.iter().map(|p| p.name).collect();
        known.extend(d.options.iter().map(|o| o.name));
        return Err(format!(
            "`{style}` has no setting `{param}`; it takes enabled, {}",
            known.join(", ")
        ));
    };
    world
        .set_field(id, component, param, stored)
        .map_err(|e| e.to_string())?;
    Ok(style)
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
        // A failed edit leaves the component as it was
        assert_eq!(
            world.get_component(barrel, "ascii_style"),
            Some(&toml::Value::Table(toml::Table::new()))
        );
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
}
