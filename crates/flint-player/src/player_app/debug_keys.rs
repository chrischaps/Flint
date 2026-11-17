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
#[cfg(feature = "debug-hud")]
const PANELS: &str = "panels";
#[cfg(feature = "debug-hud")]
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

#[cfg(feature = "debug-hud")]
const PLAYER_KEYS: &[DebugKey] = &[
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
#[cfg(not(feature = "debug-hud"))]
const PLAYER_KEYS: &[DebugKey] = &[
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

/// The player's own debug keys: live in every scene.
pub(crate) const PLAYER: DebugKeyGroup = DebugKeyGroup {
    owner: "player",
    when: "always",
    keys: PLAYER_KEYS,
};

impl PlayerApp {
    /// The run's debug keys now: the player's, each subsystem file's (live
    /// where that subsystem is in the scene), and any a subsystem declares
    /// with no file of its own (a test's), under the input config's
    /// `[debug_keys]` and the game's own bindings.
    pub(crate) fn debug_key_table(&self) -> DebugKeyTable {
        let subsystems = &self.core.subsystems;
        let always: &dyn Fn(&str) -> bool = &|_| true;
        let present: &dyn Fn(&str) -> bool = &|action| subsystems.declares(action);
        let files = super::subsystem::subsystem_debug_keys();
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
        let groups = std::iter::once((PLAYER, always))
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
        if owner == PLAYER.owner {
            self.player_debug_key(action, &key);
            return true;
        }
        #[cfg(feature = "debug-hud")]
        let open_before = self.debug_panels.iter().filter(|p| p.is_open()).count();
        self.subsystems_debug_key(action, &key);
        // A subsystem's panel opened or closed: the cursor follows the
        // panels, as F3's do
        #[cfg(feature = "debug-hud")]
        {
            let open = self.debug_panels.iter().filter(|p| p.is_open()).count();
            if open != open_before {
                self.cursor_follows_panels();
            }
        }
        true
    }

    /// One of the player's own actions; `key` is the chord it is on.
    fn player_debug_key(&mut self, action: &str, key: &str) {
        match action {
            KEYS => self.hud.key_table_open = !self.hud.key_table_open,
            // Off, the full overlay, the graph alone, off (#1078)
            STATS => self.set_stats_view(self.stats.view.next()),
            CAPTURE_FRAME => self.ask_frame_capture(super::frame_capture::Trigger::Key, key),
            SAVE_TRACE => self.ask_trace(key),
            #[cfg(feature = "debug-hud")]
            PANELS => {
                // The scene-component panels (grass, ocean, ...), not the
                // Rendering & Effects panel, which has its own key and
                // would otherwise flip out of phase with it.
                let mut any = false;
                for panel in &mut self.debug_panels {
                    if panel.name() != flint_debug_ui::RENDER_DEBUG_PANEL {
                        panel.toggle();
                        any = true;
                    }
                }
                if any {
                    self.cursor_follows_panels();
                } else {
                    self.hud.key_note.say(key, "no debug panels in this scene");
                }
            }
            // Rendering & Effects (ADR 0053): the one home for every
            // render/post debug control and their non-binary params.
            #[cfg(feature = "debug-hud")]
            RENDER_PANEL => self.toggle_named_panel(
                flint_debug_ui::RENDER_DEBUG_PANEL,
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

    /// The cursor follows the panels: let go while any is open (the render
    /// panel too), captured again once none is, in a first-person scene.
    #[cfg(feature = "debug-hud")]
    fn cursor_follows_panels(&mut self) {
        if self.debug_panels.iter().any(|p| p.is_open()) {
            self.release_cursor();
        } else if self.core.physics.has_player_entity() {
            self.capture_cursor();
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

/// A key's cap as the card shows it: the punctuation keys as their glyph.
fn cap(label: &str) -> String {
    let glyph = |name: &str| match name {
        "Backquote" => "`".to_string(),
        "Backslash" => "\\".to_string(),
        "BracketLeft" => "[".to_string(),
        "BracketRight" => "]".to_string(),
        "Semicolon" => ";".to_string(),
        "Quote" => "'".to_string(),
        "Comma" => ",".to_string(),
        "Period" => ".".to_string(),
        "Slash" => "/".to_string(),
        "Minus" => "-".to_string(),
        "Equal" => "=".to_string(),
        other => other
            .strip_prefix("Key")
            .or_else(|| other.strip_prefix("Digit"))
            .unwrap_or(other)
            .to_string(),
    };
    match label.strip_prefix("Shift+") {
        Some(rest) => format!("⇧ {}", glyph(rest)),
        None => glyph(label),
    }
}

/// The F1 card: every debug key of the run, by owner, the live ones bright,
/// the rest dim with the scene that would bring them, a key the game took
/// struck through. Centred, over everything, taking no input.
pub(super) fn paint_key_table(ctx: &egui::Context, table: &DebugKeyTable) {
    use egui::{Color32, CornerRadius, Frame, Margin, RichText, Stroke};
    // The key note's palette (key_note.rs): slate card, amber keycaps
    let ink = Color32::from_rgb(226, 230, 238);
    let dim = Color32::from_rgb(132, 138, 152);
    let amber = Color32::from_rgb(255, 214, 140);
    let keycap = |ui: &mut egui::Ui, label: &str, live: bool, struck: bool| {
        let (fill, edge, text) = if live && !struck {
            (
                Color32::from_rgba_premultiplied(36, 28, 13, 36),
                Color32::from_rgba_premultiplied(150, 115, 54, 150),
                amber,
            )
        } else {
            (
                Color32::from_rgba_premultiplied(10, 10, 12, 24),
                Color32::from_rgba_premultiplied(48, 50, 56, 60),
                dim,
            )
        };
        Frame::NONE
            .fill(fill)
            .stroke(Stroke::new(1.0, edge))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::symmetric(6, 1))
            .show(ui, |ui| {
                let mut t = RichText::new(cap(label)).monospace().size(12.0).color(text);
                if struck {
                    t = t.strikethrough();
                }
                ui.label(t);
            });
    };
    egui::Area::new(egui::Id::new("debug_key_table"))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            Frame::NONE
                .fill(Color32::from_rgba_premultiplied(12, 14, 19, 235))
                .stroke(Stroke::new(
                    1.0,
                    Color32::from_rgba_premultiplied(40, 40, 40, 40),
                ))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::symmetric(18, 14))
                .show(ui, |ui| {
                    ui.set_max_width(520.0);
                    ui.label(RichText::new("Debug keys").size(17.0).color(ink).strong());
                    ui.add_space(6.0);
                    let mut owner = "";
                    egui::Grid::new("debug_key_rows")
                        .num_columns(2)
                        .spacing([12.0, 5.0])
                        .show(ui, |ui| {
                            for e in &table.entries {
                                if e.owner != owner {
                                    owner = e.owner;
                                    let live_group =
                                        table.entries.iter().any(|o| o.owner == owner && o.live);
                                    ui.label("");
                                    let head = if live_group {
                                        RichText::new(owner).size(11.0).color(dim)
                                    } else {
                                        RichText::new(format!("{owner} · in {}", e.when))
                                            .size(11.0)
                                            .color(dim)
                                            .italics()
                                    };
                                    ui.label(head);
                                    ui.end_row();
                                }
                                let struck = matches!(e.standing, Standing::TakenByGame(_));
                                let bright = e.live && e.standing == Standing::Active;
                                keycap(ui, &e.key_label(), bright, struck);
                                let mut line = e.does.to_string();
                                if e.rebound() && e.key.is_some() {
                                    line.push_str(&format!(" — moved from {}", cap(e.default_key)));
                                }
                                match &e.standing {
                                    Standing::TakenByGame(a) => {
                                        line.push_str(&format!(" — the game's `{a}` has the key"))
                                    }
                                    Standing::Clash(a) => {
                                        line.push_str(&format!(" — `{a}` has the key first"))
                                    }
                                    Standing::Unbound => line.push_str(" — unbound"),
                                    Standing::Active => {}
                                }
                                ui.label(RichText::new(line).size(13.0).color(if bright {
                                    ink
                                } else {
                                    dim
                                }));
                                ui.end_row();
                            }
                        });
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(
                            "Rebind or turn off in an input config's [debug_keys] · \
                             flint-player --list-keys",
                        )
                        .size(11.0)
                        .color(dim),
                    );
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_declared_default_is_a_key() {
        let mut groups = vec![PLAYER];
        groups.extend(super::super::subsystem::subsystem_debug_keys());
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

    #[test]
    fn caps_show_glyphs() {
        assert_eq!(cap("Backquote"), "`");
        assert_eq!(cap("Shift+F8"), "⇧ F8");
        assert_eq!(cap("KeyK"), "K");
        assert_eq!(cap("none"), "none");
    }
}
