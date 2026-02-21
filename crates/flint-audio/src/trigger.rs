//! Audio trigger system: maps GameEvents to sound playback
//!
//! Reads `audio_trigger` components from entities and fires sounds
//! when matching events occur (collisions, interactions, trigger volumes).

use flint_core::{EntityId, Vec3};
use flint_ecs::FlintWorld;
use flint_runtime::GameEvent;
use std::collections::HashMap;

/// A sound to play in response to a game event
#[derive(Debug, Clone)]
pub enum AudioCommand {
    /// Play a sound, optionally at a 3D position
    Play {
        sound: String,
        position: Option<Vec3>,
        volume: f64,
    },
    /// Stop all sounds on an entity's spatial track
    Stop { entity: EntityId },
}

/// Rules for which sounds to play on which events
#[derive(Debug, Clone, Default)]
struct TriggerRules {
    on_collision: Option<String>,
    // Reserved for future targeted interaction events.
    #[allow(dead_code)]
    on_interact: Option<String>,
    on_enter: Option<String>,
    on_exit: Option<String>,
}

/// Reads audio_trigger components and generates AudioCommands from GameEvents
pub struct AudioTrigger {
    entity_rules: HashMap<EntityId, TriggerRules>,
}

impl Default for AudioTrigger {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioTrigger {
    pub fn new() -> Self {
        Self {
            entity_rules: HashMap::new(),
        }
    }

    /// Scan the world for entities with `audio_trigger` components
    pub fn load_rules(&mut self, world: &FlintWorld) {
        for entity in world.all_entities() {
            if self.entity_rules.contains_key(&entity.id) {
                continue;
            }

            let components = match world.get_components(entity.id) {
                Some(c) => c,
                None => continue,
            };

            let trigger_data = match components.get("audio_trigger") {
                Some(v) => v,
                None => continue,
            };

            let rules = TriggerRules {
                on_collision: trigger_data
                    .get("on_collision")
                    .and_then(|v| v.as_str().map(String::from)),
                on_interact: trigger_data
                    .get("on_interact")
                    .and_then(|v| v.as_str().map(String::from)),
                on_enter: trigger_data
                    .get("on_enter")
                    .and_then(|v| v.as_str().map(String::from)),
                on_exit: trigger_data
                    .get("on_exit")
                    .and_then(|v| v.as_str().map(String::from)),
            };

            self.entity_rules.insert(entity.id, rules);
        }
    }

    /// Process a list of game events and generate audio commands
    pub fn process_events(
        &self,
        events: &[GameEvent],
        world: &FlintWorld,
    ) -> Vec<AudioCommand> {
        let mut commands = Vec::new();

        for event in events {
            match event {
                GameEvent::CollisionStarted { entity_a, entity_b } => {
                    // Check both entities for on_collision triggers
                    if let Some(cmd) = self.check_collision(*entity_a, world) {
                        commands.push(cmd);
                    }
                    if let Some(cmd) = self.check_collision(*entity_b, world) {
                        commands.push(cmd);
                    }
                }
                GameEvent::TriggerEntered { entity: _, trigger } => {
                    if let Some(rules) = self.entity_rules.get(trigger) {
                        if let Some(sound) = &rules.on_enter {
                            let pos = world
                                .get_transform(*trigger)
                                .map(|t| t.position);
                            commands.push(AudioCommand::Play {
                                sound: sound.clone(),
                                position: pos,
                                volume: 1.0,
                            });
                        }
                    }
                }
                GameEvent::TriggerExited { entity: _, trigger } => {
                    if let Some(rules) = self.entity_rules.get(trigger) {
                        if let Some(sound) = &rules.on_exit {
                            let pos = world
                                .get_transform(*trigger)
                                .map(|t| t.position);
                            commands.push(AudioCommand::Play {
                                sound: sound.clone(),
                                position: pos,
                                volume: 1.0,
                            });
                        }
                    }
                }
                GameEvent::ActionPressed(action) => {
                    if action == "interact" {
                        // For interact, we'd need to know which entity is targeted
                        // This will be expanded when interaction raycasting is added
                    }
                }
                _ => {}
            }
        }

        commands
    }

    fn check_collision(&self, entity: EntityId, world: &FlintWorld) -> Option<AudioCommand> {
        let rules = self.entity_rules.get(&entity)?;
        let sound = rules.on_collision.as_ref()?;
        let pos = world.get_transform(entity).map(|t| t.position);

        Some(AudioCommand::Play {
            sound: sound.clone(),
            position: pos,
            volume: 1.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trigger_loads_rules() {
        let mut world = FlintWorld::new();
        let id = world.spawn("door").unwrap();

        let trigger_data = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert(
                "on_collision".into(),
                toml::Value::String("door_bump.ogg".into()),
            );
            t.insert(
                "on_interact".into(),
                toml::Value::String("door_open.ogg".into()),
            );
            t
        });
        world
            .set_component(id, "audio_trigger", trigger_data)
            .unwrap();

        let mut trigger = AudioTrigger::new();
        trigger.load_rules(&world);

        assert!(trigger.entity_rules.contains_key(&id));
        let rules = &trigger.entity_rules[&id];
        assert_eq!(rules.on_collision.as_deref(), Some("door_bump.ogg"));
        assert_eq!(rules.on_interact.as_deref(), Some("door_open.ogg"));
        assert!(rules.on_enter.is_none());
    }

    #[test]
    fn test_collision_event_generates_command() {
        let mut world = FlintWorld::new();
        let door_id = world.spawn("door").unwrap();
        let player_id = world.spawn("player").unwrap();

        // Add transform to door
        let transform = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert(
                "position".into(),
                toml::Value::Array(vec![
                    toml::Value::Float(5.0),
                    toml::Value::Float(0.0),
                    toml::Value::Float(3.0),
                ]),
            );
            t
        });
        world.set_component(door_id, "transform", transform).unwrap();

        // Add audio_trigger to door
        let trigger_data = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert(
                "on_collision".into(),
                toml::Value::String("door_bump.ogg".into()),
            );
            t
        });
        world
            .set_component(door_id, "audio_trigger", trigger_data)
            .unwrap();

        let mut trigger = AudioTrigger::new();
        trigger.load_rules(&world);

        let events = vec![GameEvent::CollisionStarted {
            entity_a: player_id,
            entity_b: door_id,
        }];

        let commands = trigger.process_events(&events, &world);
        assert_eq!(commands.len(), 1);

        match &commands[0] {
            AudioCommand::Play {
                sound, position, ..
            } => {
                assert_eq!(sound, "door_bump.ogg");
                assert!(position.is_some());
                let pos = position.unwrap();
                assert!((pos.x - 5.0).abs() < 0.01);
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_no_trigger_no_command() {
        let mut world = FlintWorld::new();
        let wall_id = world.spawn("wall").unwrap();
        let player_id = world.spawn("player").unwrap();

        let trigger = AudioTrigger::new();

        let events = vec![GameEvent::CollisionStarted {
            entity_a: player_id,
            entity_b: wall_id,
        }];

        let commands = trigger.process_events(&events, &world);
        assert!(commands.is_empty());
    }
}
