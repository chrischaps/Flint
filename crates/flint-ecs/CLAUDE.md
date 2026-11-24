# flint-ecs: the world, its ids and its components (moved out of the root CLAUDE.md by #992)

The root file keeps a short form of each bullet. Reading a component's fields (`world.view`, ADR 0115) is held across the engine: `crates/flint-core/CLAUDE.md`.

## Ids

- **Stable EntityIds** via atomic counter (never recycled), persisted across save/load
- A scene's entity ids are the same every load (#688): each `FlintWorld` counts its own from 1 and a scene spawns in name order, so ids are the names' ranks in any process; every walk of the world is ordered (`entities_with_component` by id, `all_entities` by name). A host that replaces its world (a transition, an editor reload) starts past the old one's ids with `FlintWorld::after` / `flint_scene::load_scene_after`, so a kept id never names a new entity; there is no `EntityId::new()`. Ids still move when an entity is added or renamed, so a seed meant to survive edits comes from the entity's **name**

## Changing and reading components

- `world.get_components_mut(id)` returns a `ComponentsMut` guard, not `&mut DynamicComponents` (#691): bind it `mut`; its `set`/`set_field`/`merge_component`/`remove`/`set_archetype` keep the component index, and there is no way to the map underneath, so there is no index rebuild. One component's value to change in place: `world.get_component_mut(id, name)`. `despawn` takes the entity's whole subtree and returns the ids removed; reparent a child first to keep it
- **`world.write_stamp(id)`** (#1221): a process-wide counter's value, taken each time the entity's components are lent out to write (`get_components_mut`, `get_component_mut`; every setter goes through them) or its parent link changes (`link`, `unlink_from_parent`). Equal readings mean nothing of it changed, so a system that keeps what it read can skip the entity (physics' colliders). A new way to write a record must take a stamp too
- Scene entity `merge_component()` does field-level merge INTO archetype defaults, not full replacement
- Read a component through `world.view(id, comp::COLLIDER)` (`.f32("friction", 0.5)`, `.choice("shape", &[..], "box")`, `.opt_f32(..)`; ADR 0115, `crates/flint-core/CLAUDE.md`): it takes an integer as a number and reports a wrong type or unknown choice once per entity and field. Any other TOML goes through `flint_core::toml_util` (`toml_f64`/`toml_f32`/`toml_vec3`/...). Never `toml::Value::as_float` in a runtime crate: it reads `8` as nothing, and `cargo clippy` warns on it (the root `clippy.toml`, #761) and `scripts/wasm-check.sh` denies it
