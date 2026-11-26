//! The debug keys as a table (#717): the player's own group, each scene
//! subsystem's (its file's `DEBUG_KEYS`), the input config's `[debug_keys]`
//! over them, and the dispatch of a press to its owner. A press the table
//! takes never reaches `InputState`; a key the game's actions bind is the
//! game's, as Escape was (#639). F1 shows the table over the game, and
//! `flint-player --list-keys [--json]` prints it.

use super::PlayerApp;
use flint_runtime::{DebugKey, DebugKeyGroup, DebugKeyTable, Standing};
use winit::keyboard::KeyCode;

/// The player's actions, by the names `[debug_keys]` rebinds.
const KEYS: &str = "keys";
/// The action that shows the list, which the controls line names (#716)
pub(super) const SHOW_KEYS_ACTION: &str = KEYS;
const STATS: &str = "stats";
const PANELS: &str = "panels";
const RENDER_PANEL: &str = "render_panel";
const FULLSCREEN: &str = "fullscreen";
/// Capture the next frame for the frame debugger (#1086).
pub(crate) const CAPTURE_FRAME: &str = "capture_frame";
/// Save the scope recorder's last seconds as a trace (#1097).
pub(crate) const SAVE_TRACE: &str = "save_trace";

const SHOW_KEYS: DebugKey = DebugKey {
    action: KEYS,
    key: "F1",
    does: "Show or hide this list of debug keys",
};
const CAPTURE: DebugKey = DebugKey {
    action: CAPTURE_FRAME,
    key: "Shift+F2",
    does: "Capture the next frame for the frame debugger (a .flintframe folder in captures/)",
};
const TRACE: DebugKey = DebugKey {
    action: SAVE_TRACE,
    key: "Shift+F3",
    does:
        "Save the last seconds of the engine's scopes as a trace (traces/); starts recording if off",
};
const SHOW_FULLSCREEN: DebugKey = DebugKey {
    action: FULLSCREEN,
    key: "F11",
    does: "Fullscreen, or back to the window",
};

/// The player's keys with a debug HUD in the run (#1559): F2's graph, F3's
/// and F4's panels.
const HUD_KEYS: &[DebugKey] = &[
    SHOW_KEYS,
    DebugKey {
        action: STATS,
        key: "F2",
        does: "Stats: frame-time graph, hitches, render stats; again: graph alone; again: off",
    },
    CAPTURE,
    TRACE,
    DebugKey {
        action: PANELS,
        key: "F3",
        does: "The scene's debug panels",
    },
    DebugKey {
        action: RENDER_PANEL,
        key: "F4",
        does: "The Rendering & Effects menu",
    },
    SHOW_FULLSCREEN,
];
/// The player's keys in a run without one.
const PLAIN_KEYS: &[DebugKey] = &[
    SHOW_KEYS,
    DebugKey {
        action: STATS,
        key: "F2",
        does: "Stats overlay",
    },
    CAPTURE,
    TRACE,
    SHOW_FULLSCREEN,
];

/// The player's own debug keys: live in every scene. F3 and F4 (and F2's
/// graph) are offered only where the run's launcher lists a debug HUD.
pub(crate) fn player_keys() -> DebugKeyGroup {
    DebugKeyGroup {
        owner: PLAYER_OWNER,
        when: "always",
        keys: if crate::packages::registry().hud().is_some() {
            HUD_KEYS
        } else {
            PLAIN_KEYS
        },
    }
}

/// The owner the player's own keys are under.
const PLAYER_OWNER: &str = "player";

impl PlayerApp {
    /// The run's debug keys now: the player's, each subsystem file's (live
    /// where that subsystem is in the scene), and any a subsystem declares
    /// with no file of its own (a test's), under the input config's
    /// `[debug_keys]` and the game's own bindings.
    pub(crate) fn debug_key_table(&self) -> DebugKeyTable {
        let subsystems = &self.core.subsystems;
        let always: &dyn Fn(&str) -> bool = &|_| true;
        let present: &dyn Fn(&str) -> bool = &|action| subsystems.declares(action);
        let files: Vec<DebugKeyGroup> = crate::packages::registry().debug_key_groups().collect();
        let filed = |action: &str| {
            files
                .iter()
                .any(|g| g.keys.iter().any(|k| k.action == action))
        };
        let unfiled: Vec<DebugKeyGroup> = subsystems
            .debug_keys()
            .filter_map(|(name, keys)| {
                (!keys.is_empty() && !keys.iter().all(|k| filed(k.action))).then_some(
                    DebugKeyGroup {
                        owner: name,
                        when: "this scene",
                        keys,
                    },
                )
            })
            .collect();
        let groups = std::iter::once((player_keys(), always))
            .chain(files.iter().map(|g| (*g, present)))
            .chain(unfiled.into_iter().map(|g| (g, always)));
        DebugKeyTable::build(groups, self.input.config())
    }

    /// A key pressed, real or scripted: whether it was a debug key, acted
    /// on here. One that was is the debug key's alone: the caller keeps it
    /// (and its release) from the game.
    pub(crate) fn handle_debug_key(&mut self, key_code: KeyCode) -> bool {
        let shift = self.focus.key_modifiers.shift_key();
        let table = self.debug_key_table();
        let Some(entry) = table.lookup(key_code, shift) else {
            return false;
        };
        let (action, owner, key) = (entry.action, entry.owner, entry.key_label());
        if owner == PLAYER_OWNER {
            self.player_debug_key(action, &key);
            return true;
        }
        let open_before = self.panels_open();
        self.subsystems_debug_key(action, &key);
        // A subsystem's panel opened or closed: the cursor follows the
        // panels, as F3's do
        if self.panels_open() != open_before {
            self.cursor_follows_panels();
        }
        true
    }

    /// One of the player's own actions; `key` is the chord it is on.
    fn player_debug_key(&mut self, action: &str, key: &str) {
        match action {
            KEYS => self.hud.key_table_open = !self.hud.key_table_open,
            // Off, the full overlay, the graph alone, off (#1078)
            STATS => self.set_stats_view(self.stats.view.next(self.hud.debug.is_some())),
            CAPTURE_FRAME => self.ask_frame_capture(super::frame_capture::Trigger::Key, key),
            SAVE_TRACE => self.ask_trace(key),
            // The scene's panels (grass, ocean, ...), not Rendering &
            // Effects, which has its own key: the debug HUD's (#1559)
            PANELS => self.toggle_panels(
                flint_package::hud::Toggle::Scene,
                key,
                "no debug panels in this scene",
            ),
            // Rendering & Effects (ADR 0053): the one home for every
            // render/post debug control and their non-binary params.
            RENDER_PANEL => self.toggle_panels(
                flint_package::hud::Toggle::Render,
                key,
                "no renderer active — nothing to tune",
            ),
            FULLSCREEN => {
                if let Some(window) = &self.window {
                    if window.fullscreen().is_some() {
                        window.set_fullscreen(None);
                    } else {
                        window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
                    }
                }
            }
            _ => {}
        }
    }

    /// What the input config makes of the debug keys, said once as a scene
    /// comes up: a `[debug_keys]` name no owner has, two actions on one key.
    pub(super) fn say_debug_key_config(&self) {
        let table = self.debug_key_table();
        for name in &table.unknown {
            tracing::warn!(target: "player", "[debug] [debug_keys] {name}: no debug key has this name (flint-player --list-keys lists them)");
        }
        for e in &table.entries {
            if let Standing::Clash(first) = e.standing {
                tracing::warn!(target: "player",
                    "[debug] [debug_keys] {}: {} is `{first}`'s already, so it does nothing",
                    e.action,
                    e.key_label()
                );
            }
        }
    }

    /// The table `--list-keys` prints: the scene's input layers read (the
    /// game's, the player's rebinds, `--input-config`), no scene started, so
    /// a subsystem's keys say when they are live rather than whether.
    pub(crate) fn list_debug_keys(&mut self) -> DebugKeyTable {
        self.refresh_project();
        if let Err(e) = self.configure_input_bindings() {
            tracing::warn!(target: "input", "[debug] input config: {e:#}");
        }
        self.debug_key_table()
    }
}

/// The F1 card, with the player's word on where its keys are rebound.
pub(super) fn paint_key_table(ctx: &egui::Context, table: &DebugKeyTable) {
    flint_runtime::debug_key_card::paint(
        ctx,
        table,
        "Rebind or turn off in an input config's [debug_keys] · flint-player --list-keys",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_declared_default_is_a_key() {
        let mut groups = vec![player_keys()];
        groups.extend(crate::packages::registry().debug_key_groups());
        for g in groups {
            for k in g.keys {
                assert!(
                    matches!(flint_runtime::Chord::parse(k.key), Ok(Some(_))),
                    "{}'s {} default '{}' doesn't parse",
                    g.owner,
                    k.action,
                    k.key
                );
            }
        }
    }
}
