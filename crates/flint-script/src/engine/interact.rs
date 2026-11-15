//! What the player can interact with: the nearest enabled `interactable`
//! in range, for `on_interact` and the HUD's prompt.

use flint_core::components as comp;
use flint_core::toml_util::toml_f64;
use flint_core::EntityId;
use flint_ecs::FlintWorld;

/// Read interactable range and enabled state from entity components.
/// Returns (range, enabled) — defaults to (3.0, true) if no interactable component.
pub(super) fn get_interactable_config(entity: EntityId, world: &FlintWorld) -> (f64, bool) {
    let Some(comps) = world.get_components(entity) else {
        return (3.0, true);
    };
    let Some(interactable) = comps.get(comp::INTERACTABLE) else {
        return (3.0, true);
    };
    let range = interactable.get("range").and_then(toml_f64).unwrap_or(3.0);
    let enabled = interactable
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    (range, enabled)
}

/// Information about the nearest interactable entity
pub struct NearestInteractable {
    pub entity_id: EntityId,
    pub prompt_text: String,
    pub interaction_type: String,
    pub distance: f64,
}

/// Find the nearest in-range interactable entity to the player.
/// Checks all entities with an `interactable` component that are enabled and within range.
pub fn find_nearest_interactable(world: &FlintWorld) -> Option<NearestInteractable> {
    // Find player entity
    let player_id = world
        .all_entities()
        .iter()
        .find(|e| {
            world
                .get_components(e.id)
                .map(|c| c.has(comp::CHARACTER_CONTROLLER))
                .unwrap_or(false)
        })
        .map(|e| e.id)?;
    let pt = world.get_transform(player_id)?;

    let mut best: Option<NearestInteractable> = None;

    for entity in world.all_entities() {
        let Some(comps) = world.get_components(entity.id) else {
            continue;
        };
        let Some(interactable) = comps.get(comp::INTERACTABLE) else {
            continue;
        };

        let enabled = interactable
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if !enabled {
            continue;
        }

        let range = interactable.get("range").and_then(toml_f64).unwrap_or(3.0);

        let Some(et) = world.get_transform(entity.id) else {
            continue;
        };
        let dx = (pt.position.x - et.position.x) as f64;
        let dy = (pt.position.y - et.position.y) as f64;
        let dz = (pt.position.z - et.position.z) as f64;
        let dist = (dx * dx + dy * dy + dz * dz).sqrt();

        if dist <= range {
            let is_closer = best.as_ref().is_none_or(|b| dist < b.distance);
            if is_closer {
                let prompt_text = interactable
                    .get("prompt_text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Interact")
                    .to_string();
                let interaction_type = interactable
                    .get("interaction_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("use")
                    .to_string();
                best = Some(NearestInteractable {
                    entity_id: entity.id,
                    prompt_text,
                    interaction_type,
                    distance: dist,
                });
            }
        }
    }

    best
}

/// Check if an entity is within `range` distance of the player entity
pub(super) fn is_near_player(entity: EntityId, world: &FlintWorld, range: f64) -> bool {
    // Find player entity (entity with character_controller component)
    let player_id = world
        .all_entities()
        .iter()
        .find(|e| {
            world
                .get_components(e.id)
                .map(|c| c.has(comp::CHARACTER_CONTROLLER))
                .unwrap_or(false)
        })
        .map(|e| e.id);

    let Some(player) = player_id else {
        return false;
    };
    let Some(pt) = world.get_transform(player) else {
        return false;
    };
    let Some(et) = world.get_transform(entity) else {
        return false;
    };

    let dx = (pt.position.x - et.position.x) as f64;
    let dy = (pt.position.y - et.position.y) as f64;
    let dz = (pt.position.z - et.position.z) as f64;
    (dx * dx + dy * dy + dz * dz).sqrt() <= range
}
