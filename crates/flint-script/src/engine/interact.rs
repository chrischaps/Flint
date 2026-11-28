//! What the player can interact with: the nearest enabled `interactable`
//! in range, for `on_interact` and the HUD's prompt.

use flint_core::components as comp;
use flint_core::EntityId;
use flint_ecs::FlintWorld;

/// Read interactable range and enabled state through the typed view (ADR
/// 0115: a wrong type is the default, reported once). Returns (range,
/// enabled) — defaults to (3.0, true) if no interactable component.
pub(super) fn get_interactable_config(entity: EntityId, world: &FlintWorld) -> (f64, bool) {
    let interactable = world.view(entity, comp::INTERACTABLE);
    (
        interactable.f64("range", 3.0),
        interactable.bool("enabled", true),
    )
}

/// Information about the nearest interactable entity
pub struct NearestInteractable {
    pub entity_id: EntityId,
    pub prompt_text: String,
    pub interaction_type: String,
    pub distance: f64,
}

/// Find the nearest in-range interactable entity to `player` (the host's
/// `PlayerView::player`, #1614). Checks all entities with an `interactable`
/// component that are enabled and within range.
pub fn find_nearest_interactable(
    world: &FlintWorld,
    player: Option<EntityId>,
) -> Option<NearestInteractable> {
    let pt = world.get_transform(player?)?;

    let mut best: Option<NearestInteractable> = None;

    for entity in world.all_entities() {
        let interactable = world.view(entity.id, comp::INTERACTABLE);
        if !interactable.exists() || !interactable.bool("enabled", true) {
            continue;
        }

        let range = interactable.f64("range", 3.0);

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
                let prompt_text = interactable.str("prompt_text", "Interact").to_string();
                let interaction_type = interactable.str("interaction_type", "use").to_string();
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

/// Check if an entity is within `range` distance of `player` (the host's
/// `PlayerView::player`, #1614; `None` is never near)
pub(super) fn is_near_player(
    entity: EntityId,
    world: &FlintWorld,
    range: f64,
    player: Option<EntityId>,
) -> bool {
    let Some(player) = player else {
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

#[cfg(test)]
mod tests {
    //! #1614: who `on_interact` and `find_nearest_interactable` measure
    //! from is the player the host says (`PlayerView::player`), not the
    //! first `character_controller` the world holds.
    use super::*;

    fn table(pairs: &[(&str, toml::Value)]) -> toml::Value {
        toml::Value::Table(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    }

    fn at(world: &mut FlintWorld, name: &str, x: f64) -> EntityId {
        let id = world.spawn(name).unwrap();
        let position = toml::Value::Array(vec![x.into(), 0.0.into(), 0.0.into()]);
        world
            .set_component(id, comp::TRANSFORM, table(&[("position", position)]))
            .unwrap();
        id
    }

    /// A walker with a controller at 0, a script's own rider at 10 with
    /// none, and a door at 11 that opens within 2
    fn world() -> (FlintWorld, EntityId, EntityId, EntityId) {
        let mut world = FlintWorld::new();
        let walker = at(&mut world, "walker", 0.0);
        world
            .set_component(walker, comp::CHARACTER_CONTROLLER, table(&[]))
            .unwrap();
        let rider = at(&mut world, "rider", 10.0);
        let door = at(&mut world, "door", 11.0);
        world
            .set_component(
                door,
                comp::INTERACTABLE,
                table(&[("range", 2.0.into()), ("prompt_text", "Open".into())]),
            )
            .unwrap();
        (world, walker, rider, door)
    }

    #[test]
    fn the_player_the_host_names_is_the_one_measured_from() {
        let (world, walker, rider, door) = world();
        // The controller's walker is 11 away: out of range
        assert!(!is_near_player(door, &world, 2.0, Some(walker)));
        assert!(find_nearest_interactable(&world, Some(walker)).is_none());
        // A script's own controller, with no character_controller, 1 away
        assert!(is_near_player(door, &world, 2.0, Some(rider)));
        let nearest = find_nearest_interactable(&world, Some(rider)).expect("the door");
        assert_eq!(nearest.entity_id, door);
        assert_eq!(nearest.prompt_text, "Open");
    }

    #[test]
    fn no_player_is_near_nothing() {
        let (world, _, _, door) = world();
        assert!(!is_near_player(door, &world, 100.0, None));
        assert!(find_nearest_interactable(&world, None).is_none());
    }
}
