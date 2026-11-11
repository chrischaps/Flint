//! The `entity` module: ids, components, transforms, hierarchy, materials
//! and joints. The first module declared through the builder (ADR 0116):
//! the pattern the rest are ported to.

use super::builder::kind::{Any, Array, Bool, Entity, Int, Map, MaybeEntity, Num, Str};
use super::builder::ScriptApi;
use super::{dynamic_to_toml, toml_to_dynamic};
use flint_core::components as comp;
use flint_core::EntityId;
use flint_core::{euler_deg_to_quat, quat_mul, quat_normalize};
use rhai::Dynamic;

/// A transform vector as `#{x, y, z}`.
fn xyz(v: flint_core::Vec3) -> rhai::Map {
    let mut map = rhai::Map::new();
    map.insert("x".into(), Dynamic::from(v.x as f64));
    map.insert("y".into(), Dynamic::from(v.y as f64));
    map.insert("z".into(), Dynamic::from(v.z as f64));
    map
}

fn float3(x: f64, y: f64, z: f64) -> toml::Value {
    toml::Value::Array(vec![
        toml::Value::Float(x),
        toml::Value::Float(y),
        toml::Value::Float(z),
    ])
}

fn id_of(id: EntityId) -> i64 {
    id.raw() as i64
}

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("entity");

    // ── Ids and names ──
    m.func("self_entity", "The entity this script is on.")
        .returns(Entity, "its id")
        .run(|call, ()| id_of(call.ctx_ref().current_entity));

    m.func(
        "this_entity",
        "The entity this script is on (self_entity's other name).",
    )
    .returns(Entity, "its id")
    .run(|call, ()| id_of(call.ctx_ref().current_entity));

    m.func("get_entity", "The entity with this name.")
        .arg("name", Str)
        .returns(Entity, "its id, or -1 when no entity has that name")
        .run(|call, (name,)| {
            let world = call.world()?;
            Ok(world.get_id(&name).map(id_of).unwrap_or(-1))
        });

    m.func(
        "entity_exists",
        "Whether the id names a live entity; never warns.",
    )
    .arg("id", MaybeEntity)
    .returns(Bool, "false for -1 or a despawned id")
    .run(|_, (id,)| id.is_some());

    m.func("entity_name", "The entity's name.")
        .arg("id", Entity)
        .returns(Str, "its name, or \"\" when it has none")
        .run(|call, (id,)| {
            let world = call.world()?;
            Ok(world.get_name(id).unwrap_or("").to_string())
        });

    m.func(
        "spawn_entity",
        "Make an entity with this name, with no components.",
    )
    .arg("name", Str)
    .returns(Entity, "its id, or -1 when the name is taken")
    .run(|call, (name,)| {
        let world = call.world_mut()?;
        Ok(world.spawn(name.as_str()).map(id_of).unwrap_or(-1))
    });

    m.func(
        "despawn_entity",
        "Remove the entity (and its children's link to it).",
    )
    .arg("id", Entity)
    .run(|call, (id,)| {
        let _ = call.world_mut()?.despawn(id);
        Ok(())
    });

    // ── Components ──
    m.func(
        "has_component",
        "Whether the entity has a component of this name.",
    )
    .arg("id", Entity)
    .arg("component", Str)
    .returns(Bool, "")
    .run(|call, (id, component)| {
        let world = call.world()?;
        Ok(world
            .get_components(id)
            .is_some_and(|comps| comps.has(&component)))
    });

    m.func("get_component", "The whole component as a map.")
        .arg("id", Entity)
        .arg("component", Str)
        .returns(Map, "its fields, or () when the entity hasn't it")
        .run(|call, (id, component)| {
            let world = call.world()?;
            Ok(world
                .get_components(id)
                .and_then(|comps| comps.get(&component).cloned())
                .map(toml_to_dynamic)
                .unwrap_or(Dynamic::UNIT))
        });

    m.func("get_field", "One field of a component.")
        .arg("id", Entity)
        .arg("component", Str)
        .arg("field", Str)
        .returns(Any, "its value, or () when it isn't there")
        .run(|call, (id, component, field)| {
            let world = call.world()?;
            Ok(world
                .get_components(id)
                .and_then(|comps| comps.get_field(&component, &field).cloned())
                .map(toml_to_dynamic)
                .unwrap_or(Dynamic::UNIT))
        });

    m.func(
        "set_field",
        "Set one field of a component, making the component if it isn't there.",
    )
    .arg("id", Entity)
    .arg("component", Str)
    .arg("field", Str)
    .arg("value", Any)
    .run(|call, (id, component, field, value)| {
        let world = call.world_mut()?;
        if let Some(tv) = dynamic_to_toml(&value) {
            // World-level set_field maintains the component index, so a
            // component created here (rigidbody, collider, joint, ...) is
            // visible to systems that scan by component.
            let _ = world.set_field(id, &component, &field, tv);
        }
        Ok(())
    });

    m.func(
        "find_entities_with",
        "Every entity that has a component of this name.",
    )
    .arg("component", Str)
    .returns(Array, "their ids")
    .run(|call, (component,)| {
        let world = call.world()?;
        Ok(world
            .entities_with_component(&component)
            .iter()
            .map(|id| Dynamic::from(id_of(*id)))
            .collect::<rhai::Array>())
    });

    m.func(
        "entity_count_with",
        "How many entities have a component of this name.",
    )
    .arg("component", Str)
    .returns(Int, "")
    .run(|call, (component,)| {
        let world = call.world()?;
        Ok(world.entities_with_component(&component).len() as i64)
    });

    // ── Transform ──
    m.func(
        "get_position",
        "The entity's position in its parent's space.",
    )
    .arg("id", Entity)
    .returns(Map, "#{x, y, z}; empty when it has no transform")
    .run(|call, (id,)| {
        let world = call.world()?;
        Ok(world
            .get_transform(id)
            .map(|t| xyz(t.position))
            .unwrap_or_default())
    });

    m.func(
        "set_position",
        "Move the entity to a position in its parent's space.",
    )
    .arg("id", Entity)
    .arg("x", Num)
    .arg("y", Num)
    .arg("z", Num)
    .run(|call, (id, x, y, z)| {
        if let Some(comps) = call.world_mut()?.get_components_mut(id) {
            comps.set_field(comp::TRANSFORM, "position", float3(x, y, z));
        }
        Ok(())
    });

    m.func("get_rotation", "The entity's Euler rotation, degrees.")
        .arg("id", Entity)
        .returns(Map, "#{x, y, z}; empty when it has no transform")
        .run(|call, (id,)| {
            let world = call.world()?;
            Ok(world
                .get_transform(id)
                .map(|t| xyz(t.rotation))
                .unwrap_or_default())
        });

    m.func(
        "set_rotation",
        "Set the entity's Euler rotation in degrees, dropping any quaternion.",
    )
    .arg("id", Entity)
    .arg("x", Num)
    .arg("y", Num)
    .arg("z", Num)
    .run(|call, (id, x, y, z)| {
        if let Some(comps) = call.world_mut()?.get_components_mut(id) {
            comps.set_field(comp::TRANSFORM, "rotation", float3(x, y, z));
            // Clear the quaternion so the Euler angles take effect
            if let Some(table) = comps
                .get_mut(comp::TRANSFORM)
                .and_then(|t| t.as_table_mut())
            {
                table.remove("rotation_quat");
            }
        }
        Ok(())
    });

    m.func("get_scale", "The entity's scale.")
        .arg("id", Entity)
        .returns(
            Map,
            "#{x, y, z}, 1s when unset; empty when it has no transform",
        )
        .run(|call, (id,)| {
            let world = call.world()?;
            Ok(world
                .get_transform(id)
                .map(|t| xyz(t.scale))
                .unwrap_or_default())
        });

    m.func("set_scale", "Set the entity's scale on each axis.")
        .arg("id", Entity)
        .arg("x", Num)
        .arg("y", Num)
        .arg("z", Num)
        .run(|call, (id, x, y, z)| {
            if let Some(comps) = call.world_mut()?.get_components_mut(id) {
                comps.set_field(comp::TRANSFORM, "scale", float3(x, y, z));
            }
            Ok(())
        });

    m.func(
        "get_rotation_quat",
        "The rotation the entity renders with, as a quaternion.",
    )
    .arg("id", Entity)
    .returns(
        Map,
        "#{x, y, z, w}: rotation_quat, else the Euler angles converted; empty when it has no transform",
    )
    .run(|call, (id,)| {
        let world = call.world()?;
        let mut map = rhai::Map::new();
        if let Some(transform) = world.get_transform(id) {
            let q = transform.effective_quat();
            for (key, v) in ["x", "y", "z", "w"].into_iter().zip(q) {
                map.insert(key.into(), Dynamic::from(v as f64));
            }
        }
        Ok(map)
    });

    m.func(
        "set_rotation_quat",
        "Set the entity's rotation as a quaternion (normalised); zeroes the Euler rotation.",
    )
    .arg("id", Entity)
    .arg("x", Num)
    .arg("y", Num)
    .arg("z", Num)
    .arg("w", Num)
    .run(|call, (id, x, y, z, w)| {
        if let Some(comps) = call.world_mut()?.get_components_mut(id) {
            let q = quat_normalize(&[x as f32, y as f32, z as f32, w as f32]);
            write_rotation_quat(comps, q);
        }
        Ok(())
    });

    m.func(
        "rotate_local",
        "Turn the entity about its own axes by these degrees, onto its current orientation.",
    )
    .arg("id", Entity)
    .arg("x_deg", Num)
    .arg("y_deg", Num)
    .arg("z_deg", Num)
    .run(|call, (id, x, y, z)| {
        let world = call.world_mut()?;
        let Some(current) = world.get_transform(id).map(|t| t.effective_quat()) else {
            return Ok(());
        };
        let delta = euler_deg_to_quat(x as f32, y as f32, z as f32);
        let q = quat_normalize(&quat_mul(&current, &delta));
        if let Some(comps) = world.get_components_mut(id) {
            write_rotation_quat(comps, q);
        }
        Ok(())
    });

    m.func(
        "set_shear",
        "Shear the entity: any of #{xy, xz, yx, yz, zx, zy}, ab moving a per unit of b.",
    )
    .arg("id", Entity)
    .arg("shear", Map)
    .run(|call, (id, shear)| {
        let terms = shear_terms(&shear);
        if let Some(comps) = call.world_mut()?.get_components_mut(id) {
            write_shear(comps, terms);
        }
        Ok(())
    });

    m.func("get_shear", "The entity's shear, all six terms.")
        .arg("id", Entity)
        .returns(Map, "#{xy, xz, yx, yz, zx, zy}; zeros when it has none")
        .or_return(shear_map([0.0; 6]))
        .run(|call, (id,)| {
            let world = call.world()?;
            let terms = world
                .get_transform(id)
                .and_then(|t| t.shear)
                .unwrap_or([0.0; 6]);
            Ok(shear_map(terms))
        });

    m.func(
        "clear_shear",
        "Remove the entity's shear: back to rotation and scale.",
    )
    .arg("id", Entity)
    .run(|call, (id,)| {
        if let Some(comps) = call.world_mut()?.get_components_mut(id) {
            write_shear(comps, [0.0; 6]);
        }
        Ok(())
    });

    m.func(
        "set_visible",
        "Show or hide the entity and its descendants; hidden ones still simulate.",
    )
    .arg("id", Entity)
    .arg("visible", Bool)
    .run(|call, (id, visible)| {
        let world = call.world_mut()?;
        // Unchanged: no write, so a pool that re-asserts every frame doesn't
        // touch the component, and showing an entity that was never hidden
        // doesn't give it one
        if world.is_visible(id) == visible {
            return Ok(());
        }
        // World-level set_field keeps the component index, which is how the
        // renderer finds hidden entities without a scan
        let _ = world.set_field(
            id,
            comp::VISIBILITY,
            "visible",
            toml::Value::Boolean(visible),
        );
        Ok(())
    });

    m.func(
        "is_visible",
        "The entity's own visibility switch (a hidden ancestor isn't counted).",
    )
    .arg("id", Entity)
    .returns(Bool, "true when unset")
    .run(|call, (id,)| Ok(call.world()?.is_visible(id)));

    m.func("distance", "The distance between two entities' positions.")
        .arg("a", Entity)
        .arg("b", Entity)
        .returns(Num, "metres; f64::MAX when either has no transform")
        .or_return(f64::MAX)
        .run(|call, (a, b)| {
            let world = call.world()?;
            Ok(match (world.get_transform(a), world.get_transform(b)) {
                (Some(a), Some(b)) => {
                    let dx = (a.position.x - b.position.x) as f64;
                    let dy = (a.position.y - b.position.y) as f64;
                    let dz = (a.position.z - b.position.z) as f64;
                    (dx * dx + dy * dy + dz * dz).sqrt()
                }
                _ => f64::MAX,
            })
        });

    // ── Hierarchy ──
    m.func("set_parent", "Make one entity the child of another.")
        .arg("child", Entity)
        .arg("parent", Entity)
        .run(|call, (child, parent)| {
            let _ = call.world_mut()?.set_parent(child, parent);
            Ok(())
        });

    m.func("get_parent", "The entity's parent.")
        .arg("id", Entity)
        .returns(Entity, "its id, or -1 when it has none")
        .or_return(-1_i64)
        .run(|call, (id,)| Ok(call.world()?.get_parent(id).map(id_of).unwrap_or(-1)));

    m.func("get_children", "The entity's children.")
        .arg("id", Entity)
        .returns(Array, "their ids")
        .run(|call, (id,)| {
            Ok(call
                .world()?
                .get_children(id)
                .into_iter()
                .map(|c| Dynamic::from(id_of(c)))
                .collect::<rhai::Array>())
        });

    m.func(
        "get_world_position",
        "The entity's position in the world, through its parents.",
    )
    .arg("id", Entity)
    .returns(Map, "#{x, y, z}; empty when it has no transform")
    .run(|call, (id,)| {
        Ok(call
            .world()?
            .get_world_position(id)
            .map(xyz)
            .unwrap_or_default())
    });

    // ── Materials ──
    m.func(
        "set_material_color",
        "Set the entity's material base colour.",
    )
    .arg("id", Entity)
    .arg("r", Num)
    .arg("g", Num)
    .arg("b", Num)
    .arg("a", Num)
    .run(|call, (id, r, g, b, a)| {
        if let Some(comps) = call.world_mut()?.get_components_mut(id) {
            for (field, v) in [
                ("base_color_r", r),
                ("base_color_g", g),
                ("base_color_b", b),
                ("base_color_a", a),
            ] {
                comps.set_field(comp::MATERIAL, field, toml::Value::Float(v));
            }
        }
        Ok(())
    });

    m.func(
        "set_material_override",
        "Tint one named glTF material on the entity and its descendants.",
    )
    .arg("id", Entity)
    .arg("material", Str)
    .arg("r", Num)
    .arg("g", Num)
    .arg("b", Num)
    .arg("a", Num)
    .run(|call, (id, material, r, g, b, a)| {
        if material.is_empty() {
            return Ok(());
        }
        let world = call.world_mut()?;
        let colour = toml::Value::Array([r, g, b, a].into_iter().map(toml::Value::Float).collect());
        // An entry that already holds a light (a table) keeps it.
        let holds_light = world
            .get_components(id)
            .and_then(|c| c.get(comp::MATERIAL_OVERRIDES))
            .and_then(|m| m.get(material.as_str()))
            .is_some_and(|v| v.is_table());
        let table = holds_light
            .then(|| material_override_table(world, id, &material))
            .flatten();
        let value = match table {
            Some(mut t) => {
                t.insert("base_color".into(), colour);
                toml::Value::Table(t)
            }
            None => colour,
        };
        let _ = world.set_field(id, comp::MATERIAL_OVERRIDES, &material, value);
        Ok(())
    });

    m.func(
        "set_material_emissive",
        "Recolour one named glTF material's emission (linear RGB) on the entity and its descendants.",
    )
    .arg("id", Entity)
    .arg("material", Str)
    .arg("r", Num)
    .arg("g", Num)
    .arg("b", Num)
    .run(|call, (id, material, r, g, b)| {
        set_material_emissive(call.world_mut()?, id, &material, [r, g, b], None);
        Ok(())
    });

    m.func(
        "set_material_emissive",
        "Recolour one named glTF material's emission and set its strength.",
    )
    .arg("id", Entity)
    .arg("material", Str)
    .arg("r", Num)
    .arg("g", Num)
    .arg("b", Num)
    .arg("strength", Num)
    .run(|call, (id, material, r, g, b, strength)| {
        set_material_emissive(call.world_mut()?, id, &material, [r, g, b], Some(strength));
        Ok(())
    });

    m.func(
        "clear_material_override",
        "Drop the entity's own override of one material, colour and light alike.",
    )
    .arg("id", Entity)
    .arg("material", Str)
    .run(|call, (id, material)| {
        if let Some(t) = call
            .world_mut()?
            .get_components_mut(id)
            .and_then(|comps| comps.get_mut(comp::MATERIAL_OVERRIDES))
            .and_then(|v| v.as_table_mut())
        {
            t.remove(material.as_str());
        }
        Ok(())
    });

    // ── Joints ──
    m.func(
        "set_joint_target",
        "Set the joint's motor target: degrees for a hinge or spherical, metres for prismatic.",
    )
    .arg("id", Entity)
    .arg("value", Num)
    .run(|call, (id, value)| {
        if let Some(comps) = call.world_mut()?.get_components_mut(id) {
            if comps.has(comp::JOINT) {
                comps.set_field(comp::JOINT, "motor_target", toml::Value::Float(value));
            }
        }
        Ok(())
    });

    m.func("get_joint_target", "The joint's motor target.")
        .arg("id", Entity)
        .returns(Num, "0 when it has none")
        .run(|call, (id,)| {
            Ok(call
                .world()?
                .get_components(id)
                .and_then(|comps| comps.get_field(comp::JOINT, "motor_target"))
                .and_then(flint_core::toml_util::toml_f64)
                .unwrap_or(0.0))
        });

    m.func(
        "get_joint_position",
        "The simulated joint coordinate from its rest pose: hinge degrees or prismatic metres.",
    )
    .arg("id", Entity)
    .returns(
        Num,
        "the coordinate, or () when there is no physics or no such joint",
    )
    .run(
        |call, (id,)| match call.physics().and_then(|p| p.joint_position(id)) {
            Some(v) => Dynamic::from(v as f64),
            None => Dynamic::UNIT,
        },
    );
}

// ─── Helpers ─────────────────────────────────────────────

fn shear_map(terms: [f32; 6]) -> rhai::Map {
    let mut map = rhai::Map::new();
    for (key, v) in flint_core::Shear::KEYS.iter().zip(terms) {
        map.insert((*key).into(), Dynamic::from(v as f64));
    }
    map
}

/// Recolour `material`'s emission on `id` (#624); `strength` replaces its
/// emissive_strength. Writes `material_overrides.<material>` as a table,
/// keeping a base colour set by set_material_override.
fn set_material_emissive(
    world: &mut flint_ecs::FlintWorld,
    id: EntityId,
    material: &str,
    rgb: [f64; 3],
    strength: Option<f64>,
) {
    if material.is_empty() {
        return;
    }
    let mut t = material_override_table(world, id, material).unwrap_or_default();
    t.insert(
        "emissive".into(),
        toml::Value::Array(rgb.iter().map(|&v| toml::Value::Float(v)).collect()),
    );
    if let Some(s) = strength {
        t.insert("emissive_strength".into(), toml::Value::Float(s));
    }
    let _ = world.set_field(
        id,
        comp::MATERIAL_OVERRIDES,
        material,
        toml::Value::Table(t),
    );
}

/// The entity's own `material_overrides.<material>` as a table (#624): a
/// table as is, a plain base colour array as `{ base_color = [...] }`,
/// `None` when it has none.
fn material_override_table(
    world: &flint_ecs::FlintWorld,
    id: EntityId,
    material: &str,
) -> Option<toml::Table> {
    match world
        .get_components(id)?
        .get(comp::MATERIAL_OVERRIDES)?
        .get(material)?
    {
        toml::Value::Table(t) => Some(t.clone()),
        colour @ toml::Value::Array(_) => {
            let mut t = toml::Table::new();
            t.insert("base_color".into(), colour.clone());
            Some(t)
        }
        _ => None,
    }
}

/// A script's shear map as the six terms, in `Shear::KEYS` order. Integers
/// count (`#{xz: 1}`); an unknown key or a non-number warns and is skipped.
fn shear_terms(shear: &rhai::Map) -> [f32; 6] {
    let mut terms = [0.0f32; 6];
    for (key, value) in shear.iter() {
        let Some(i) = flint_core::Shear::KEYS
            .iter()
            .position(|k| *k == key.as_str())
        else {
            tracing::warn!(
                "set_shear: unknown term '{}' (expected xy, xz, yx, yz, zx, zy)",
                key
            );
            continue;
        };
        let v = value
            .as_float()
            .ok()
            .or_else(|| value.as_int().ok().map(|n| n as f64));
        match v {
            Some(v) => terms[i] = v as f32,
            None => tracing::warn!("set_shear: '{}' is not a number", key),
        }
    }
    terms
}

/// Store a shear (#625) as the table a scene would write, only the terms
/// that aren't zero; an all-zero shear removes the key.
fn write_shear(comps: &mut flint_ecs::DynamicComponents, terms: [f32; 6]) {
    if terms.iter().any(|v| *v != 0.0) {
        comps.set_field(comp::TRANSFORM, "shear", flint_core::Shear::to_toml(&terms));
    } else if let Some(table) = comps
        .get_mut(comp::TRANSFORM)
        .and_then(|t| t.as_table_mut())
    {
        table.remove("shear");
    }
}

/// Store a quaternion as the entity's rotation. The Euler `rotation` is
/// zeroed so readers that only know Euler see a neutral value rather than a
/// stale one; `to_matrix()` uses the quaternion whenever it is present.
fn write_rotation_quat(comps: &mut flint_ecs::DynamicComponents, q: [f32; 4]) {
    comps.set_field(
        comp::TRANSFORM,
        "rotation_quat",
        toml::Value::Array(q.iter().map(|c| toml::Value::Float(*c as f64)).collect()),
    );
    comps.set_field(comp::TRANSFORM, "rotation", float3(0.0, 0.0, 0.0));
}
