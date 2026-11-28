//! What the player says when a scene comes up (#716): one line for what
//! loaded and what went wrong on the way, and one for the controls the
//! scene binds. Each file loaded is a `debug` event of target `load` with a
//! `kind`, which `crate::logging`'s tally counts into the first line.

use super::PlayerApp;
use flint_runtime::{binding_label, parse_key_code, InputConfig};
use std::collections::BTreeSet;
use std::path::Path;

impl PlayerApp {
    /// Start a load's count afresh, so a transition's summary holds only
    /// what that transition loaded and raised
    pub(super) fn begin_load_count(&self) {
        crate::logging::begin_load_count();
    }

    /// The scene is up: its entities, each kind of file it loaded, how
    /// long it took and the warnings on the way, as one `load` line
    /// (`logging::say_loaded`, which `flint edit` says too, #1164)
    pub(super) fn say_loaded(&self) {
        let scene = flint_core::project::shown(Path::new(&self.core.scene_path));
        crate::logging::say_loaded(&scene, self.core.world.entity_count());
    }

    /// The controls the scene binds, said at the start and again after a
    /// transition that changed them
    pub(super) fn say_controls(&mut self) {
        let config = self.input.config();
        let read: BTreeSet<String> = self
            .core
            .script
            .actions_read(config.actions.keys().map(String::as_str))
            .into_iter()
            .map(str::to_string)
            .collect();
        // The key that lists the debug keys, if the table has one on
        // (`[debug_keys] enabled = false` or `keys = "none"` takes it off)
        let table = self.debug_key_table();
        let list_key = table
            .entries
            .iter()
            .find(|e| e.action == super::debug_keys::SHOW_KEYS_ACTION)
            .filter(|e| table.enabled && e.standing == flint_runtime::Standing::Active)
            .map(|e| e.key_label());
        let view = self.core.player_view();
        let line = controls_line(
            config,
            &self.input_layers.named_actions,
            &read,
            Player {
                controls: view.controls,
                mouse_look: view.mouse_look,
            },
            list_key.as_deref(),
        );
        if self.input_layers.said_controls.as_deref() == Some(line.as_str()) {
            return;
        }
        tracing::info!(target: "input", "{line}");
        self.input_layers.said_controls = Some(line);
    }
}

/// What the controls line says of the player (`PlayerView`, ADR 0138):
/// the actions its controller reads, which a scene binds whatever its own
/// layer names (the character controller's six, ADR 0134), and whether the
/// mouse turns it.
#[derive(Clone, Copy, Default)]
pub(super) struct Player<'a> {
    pub controls: &'a [&'a str],
    pub mouse_look: bool,
}

/// `Controls: jump Space/Pad:South, steer A/D | Esc cursor/exit, F1 debug
/// keys`: each bound action the scene uses with its bindings' labels, then
/// the player's own keys. An action is used when the game's layers name it
/// (`named`), a script reads it (`read`), or the player's controller reads
/// it (`player.controls`; `mouse look` joins the keys when the mouse turns
/// the player). A built-in nothing uses (`fire` in a racing game) is left
/// out. `debug_keys` is the key that shows the debug-key list, when one
/// does (the `keys` row of the table, F1 unless `[debug_keys]` moved it).
pub(super) fn controls_line(
    config: &InputConfig,
    named: &BTreeSet<String>,
    read: &BTreeSet<String>,
    player: Player<'_>,
    debug_keys: Option<&str>,
) -> String {
    let listed =
        |name: &str| named.contains(name) || read.contains(name) || player.controls.contains(&name);
    let actions: Vec<String> = config
        .actions
        .iter()
        .filter(|(name, _)| listed(name))
        .filter(|(_, action)| !action.bindings.is_empty())
        .map(|(name, action)| {
            let mut labels: Vec<String> = Vec::new();
            for label in action.bindings.iter().map(binding_label) {
                if !labels.contains(&label) {
                    labels.push(label);
                }
            }
            format!("{name} {}", labels.join("/"))
        })
        .collect();
    let mut line = if actions.is_empty() {
        "Controls: the scene reads no actions".to_string()
    } else {
        format!("Controls: {}", actions.join(", "))
    };

    let mut keys: Vec<String> = Vec::new();
    if player.mouse_look {
        keys.push("mouse look".into());
    }
    // A game that binds Escape has it, and the window doesn't close (#639)
    let escape = parse_key_code("Escape").is_some_and(|k| config.binds_key(k));
    if !escape {
        keys.push("Esc cursor/exit".into());
    }
    // The rest of the player's keys are the debug-key table's, which this
    // key lists over the game (#717)
    if let Some(key) = debug_keys {
        keys.push(format!("{key} debug keys"));
    }
    if !keys.is_empty() {
        line.push_str(" | ");
        line.push_str(&keys.join(", "));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::{controls_line, Player};
    use flint_runtime::InputConfig;
    use std::collections::BTreeSet;

    fn kart() -> InputConfig {
        InputConfig::from_toml_str(
            r#"
            game_id = "kart"
            [actions.accelerate]
            bindings = [{ type = "key", code = "KeyW" }, { type = "gamepad_button", button = "RightTrigger2" }]
            [actions.steer]
            kind = "axis1d"
            bindings = [
                { type = "key", code = "KeyA", scale = -1.0 },
                { type = "key", code = "KeyD" },
                { type = "gamepad_axis", axis = "LeftStickX" },
            ]
            [actions.pause]
            bindings = [{ type = "key", code = "Escape" }]
            "#,
        )
        .unwrap()
    }

    /// The old banner told a kart game to press WASD to move, Space to
    /// jump and Shift to sprint; now it lists what the game binds
    #[test]
    fn a_game_with_its_own_config_lists_only_its_actions() {
        let mut merged = InputConfig::built_in_defaults();
        let game = kart();
        let named: BTreeSet<String> = game.actions.keys().cloned().collect();
        merged.actions.extend(game.actions);
        let line = controls_line(
            &merged,
            &named,
            &BTreeSet::new(),
            Player::default(),
            Some("F1"),
        );
        assert!(
            line.starts_with(
                "Controls: accelerate W/Pad:RightTrigger2, pause Escape, steer A/D/Pad:LeftStickX | "
            ),
            "{line}"
        );
        for gone in ["jump", "sprint", "move_forward", "mouse look", "Esc cursor"] {
            assert!(!line.contains(gone), "{gone} in {line}");
        }
        assert!(line.ends_with("| F1 debug keys"), "{line}");
    }

    /// The Overgrown Ruins' layer names only `pause`; its player walks on
    /// the built-in bindings the character controller reads
    #[test]
    #[cfg(feature = "first-person")]
    fn a_first_person_game_lists_the_walk_beside_its_own_actions() {
        let mut merged = InputConfig::built_in_defaults();
        let game = InputConfig::from_toml_str(
            "[actions.pause]\nbindings = [{ type = \"key\", code = \"Escape\" }]",
        )
        .unwrap();
        let named: BTreeSet<String> = game.actions.keys().cloned().collect();
        merged.actions.extend(game.actions);
        // What the first-person package writes into the player's view (#1614, #1618)
        let walker = Player {
            controls: &flint_first_person::ACTIONS,
            mouse_look: true,
        };
        let line = controls_line(&merged, &named, &BTreeSet::new(), walker, Some("F1"));
        assert!(
            line.starts_with(
                "Controls: jump Space, move_backward S, move_forward W, move_left A, \
                 move_right D, pause Escape, sprint LShift | mouse look, F1 debug keys"
            ),
            "{line}"
        );
        // Bound, but nothing of the engine's reads them and the game named none
        for gone in ["fire", "reload", "weapon_1", "Esc cursor"] {
            assert!(!line.contains(gone), "{gone} in {line}");
        }
    }

    /// The `2d` template moves with WASD from its script and has no
    /// character controller: the actions its script spells are listed
    #[test]
    fn the_actions_a_script_reads_are_listed() {
        let named: BTreeSet<String> = ["toggle_help".to_string()].into();
        let read: BTreeSet<String> = ["move_left", "move_right", "jump"].map(String::from).into();
        let mut config = InputConfig::built_in_defaults();
        config.actions.extend(
            InputConfig::from_toml_str(
                "[actions.toggle_help]\nbindings = [{ type = \"key\", code = \"KeyH\" }]",
            )
            .unwrap()
            .actions,
        );
        let line = controls_line(&config, &named, &read, Player::default(), Some("F1"));
        assert!(
            line.starts_with(
                "Controls: jump Space, move_left A, move_right D, toggle_help H | Esc cursor/exit, F1 debug keys"
            ),
            "{line}"
        );
    }

    #[test]
    fn a_scene_that_reads_no_action_says_so() {
        let none = BTreeSet::new();
        let line = controls_line(
            &InputConfig::built_in_defaults(),
            &none,
            &none,
            Player::default(),
            None,
        );
        assert!(
            line == "Controls: the scene reads no actions | Esc cursor/exit",
            "{line}"
        );
    }
}
