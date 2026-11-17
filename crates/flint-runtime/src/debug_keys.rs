//! Debug keys as a table (#717): each owner (the player, a subsystem)
//! declares its actions and their default keys, an input config's
//! `[debug_keys]` block rebinds, unbinds or turns them all off, and the
//! resolved [`DebugKeyTable`] says which key does what in this run.
//!
//! ```toml
//! [debug_keys]
//! enabled = false        # a release build: every key goes to the game
//! stats = "F10"          # rebind an action
//! living_pick_next = "Shift+F8"
//! living_freeze = "none" # unbind one
//! ```
//!
//! A key the game's own actions bind is the game's: the debug action on it
//! stands aside (as Escape already did, #639), and the table says so.

use crate::input::{parse_key_code, InputConfig};
use serde::{Deserialize, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;
use winit::keyboard::KeyCode;

/// One debug action an owner declares: its name (what `[debug_keys]`
/// rebinds), its default key as a config would write it, and what it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DebugKey {
    pub action: &'static str,
    pub key: &'static str,
    pub does: &'static str,
}

/// An owner's debug keys and when they are live (`always`, or the kind of
/// scene that brings the owner).
#[derive(Debug, Clone, Copy)]
pub struct DebugKeyGroup {
    pub owner: &'static str,
    pub when: &'static str,
    pub keys: &'static [DebugKey],
}

/// A key and whether Shift is held with it: `F8`, `Shift+F8`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    pub key: KeyCode,
    pub shift: bool,
}

impl Chord {
    /// Parse a chord as a config writes it: a key name (`F5`, `Backquote`,
    /// `KeyK`, the names bindings use), optionally after `Shift+`.
    /// `Ok(None)` for `none` or an empty string: unbound.
    pub fn parse(text: &str) -> Result<Option<Chord>, String> {
        let text = text.trim();
        if text.is_empty() || text.eq_ignore_ascii_case("none") {
            return Ok(None);
        }
        let (shift, name) = match text.split_once('+') {
            Some((m, name)) if m.trim().eq_ignore_ascii_case("shift") => (true, name.trim()),
            Some((m, _)) => {
                return Err(format!(
                    "'{text}': the one modifier a debug key takes is Shift, not '{m}'"
                ))
            }
            None => (false, text),
        };
        match parse_key_code(name) {
            Some(key) => Ok(Some(Chord { key, shift })),
            None => Err(format!(
                "'{text}': '{name}' is not a key name (they are winit's: F5, Backquote, KeyK, Digit1)"
            )),
        }
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.shift {
            write!(f, "Shift+{:?}", self.key)
        } else {
            write!(f, "{:?}", self.key)
        }
    }
}

impl Serialize for Chord {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// An input config's `[debug_keys]` block: `enabled`, and an action's key
/// by its name. A later layer's `enabled` and each action it names win.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DebugKeysConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(flatten)]
    pub keys: BTreeMap<String, String>,
}

impl DebugKeysConfig {
    /// Lay `over` on this: its `enabled` if it says, each action it names.
    pub fn merge(&mut self, over: DebugKeysConfig) {
        if over.enabled.is_some() {
            self.enabled = over.enabled;
        }
        self.keys.extend(over.keys);
    }

    /// Every key written parses (which actions exist is the table's to say).
    pub fn validate(&self) -> Result<(), String> {
        for (action, key) in &self.keys {
            Chord::parse(key).map_err(|e| format!("[debug_keys] {action} = {e}"))?;
        }
        Ok(())
    }
}

/// Why a declared debug key does nothing in this run, if it doesn't.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "by", rename_all = "snake_case")]
pub enum Standing {
    /// It acts when pressed (if its owner is live).
    Active,
    /// `[debug_keys] <action> = "none"`.
    Unbound,
    /// The game's action of this name binds the key, so the game gets it.
    TakenByGame(String),
    /// An earlier action in the table has the same key.
    Clash(&'static str),
}

/// One row of the resolved table.
#[derive(Debug, Clone, Serialize)]
pub struct DebugKeyEntry {
    pub action: &'static str,
    pub owner: &'static str,
    /// The key it is on, after the config; `None` unbound.
    pub key: Option<Chord>,
    /// Its owner's default, as written.
    pub default_key: &'static str,
    pub does: &'static str,
    /// When its owner is there: `always`, or the scene that brings it.
    pub when: &'static str,
    /// Its owner is in this run's scene now.
    pub live: bool,
    pub standing: Standing,
}

impl DebugKeyEntry {
    /// The key as shown: its chord, or `none`.
    pub fn key_label(&self) -> String {
        self.key
            .map_or_else(|| "none".to_string(), |c| c.to_string())
    }

    /// Whether the key was moved off its default.
    pub fn rebound(&self) -> bool {
        Chord::parse(self.default_key).ok().flatten() != self.key
    }
}

/// The debug keys of a run: every owner's actions with the config's rebinds,
/// which are live, and which stand aside for the game.
#[derive(Debug, Clone, Serialize)]
pub struct DebugKeyTable {
    /// `[debug_keys] enabled`: off, no key is a debug key.
    pub enabled: bool,
    pub entries: Vec<DebugKeyEntry>,
    /// Actions a `[debug_keys]` block names that no owner declares.
    pub unknown: Vec<String>,
}

impl DebugKeyTable {
    /// Resolve `groups` (each with whether its owner is live) against the
    /// input config's `[debug_keys]` and its actions' key bindings.
    pub fn build<'a>(
        groups: impl IntoIterator<Item = (DebugKeyGroup, &'a dyn Fn(&str) -> bool)>,
        config: &InputConfig,
    ) -> Self {
        let block = config.debug_keys.clone().unwrap_or_default();
        let mut entries: Vec<DebugKeyEntry> = Vec::new();
        for (group, live) in groups {
            for k in group.keys {
                let written = block.keys.get(k.action).map(String::as_str);
                // A default that doesn't parse is the owner's bug: a test
                // holds every declared default (`every_default_parses`)
                let key = Chord::parse(written.unwrap_or(k.key)).ok().flatten();
                let live = live(k.action);
                let standing = match key {
                    None => Standing::Unbound,
                    Some(chord) => {
                        if let Some(game) = config.action_binding_key(chord.key) {
                            Standing::TakenByGame(game.to_string())
                        } else if let Some(first) = entries.iter().find(|e| {
                            // Only owners both here can clash: an absent
                            // one's key is free for a present one
                            live && e.live && e.key == Some(chord) && e.standing == Standing::Active
                        }) {
                            Standing::Clash(first.action)
                        } else {
                            Standing::Active
                        }
                    }
                };
                entries.push(DebugKeyEntry {
                    action: k.action,
                    owner: group.owner,
                    key,
                    default_key: k.key,
                    does: k.does,
                    when: group.when,
                    live,
                    standing,
                });
            }
        }
        let unknown = block
            .keys
            .keys()
            .filter(|name| !entries.iter().any(|e| e.action == name.as_str()))
            .cloned()
            .collect();
        Self {
            enabled: block.enabled.unwrap_or(true),
            entries,
            unknown,
        }
    }

    /// The live, active action a press of `key` (with Shift or not) is, if
    /// any. A Shift chord is matched first; a plain key's action also
    /// answers with Shift held, as the old `match` did.
    pub fn lookup(&self, key: KeyCode, shift: bool) -> Option<&DebugKeyEntry> {
        if !self.enabled {
            return None;
        }
        let find = |shift| {
            self.entries.iter().find(|e| {
                e.live && e.standing == Standing::Active && e.key == Some(Chord { key, shift })
            })
        };
        find(shift).or_else(|| if shift { find(false) } else { None })
    }

    /// The table as text, one row a key: what `--list-keys` prints.
    pub fn text(&self) -> String {
        use fmt::Write as _;
        let mut out = String::new();
        if self.enabled {
            out.push_str("Debug keys ([debug_keys] in an input config rebinds them; enabled = false turns them off)\n");
        } else {
            out.push_str(
                "Debug keys: off ([debug_keys] enabled = false); every key goes to the game\n",
            );
        }
        let key_w = self
            .entries
            .iter()
            .map(|e| e.key_label().len())
            .max()
            .unwrap_or(4)
            .max(4);
        let action_w = self
            .entries
            .iter()
            .map(|e| e.action.len())
            .max()
            .unwrap_or(6);
        for e in &self.entries {
            let mut notes = Vec::new();
            if !e.live {
                notes.push(format!("in {}", e.when));
            }
            if e.rebound() {
                notes.push(format!("default {}", e.default_key));
            }
            match &e.standing {
                Standing::Active | Standing::Unbound => {}
                Standing::TakenByGame(a) => notes.push(format!("the game's `{a}` binds it")),
                Standing::Clash(a) => notes.push(format!("`{a}` has it first")),
            }
            let notes = if notes.is_empty() {
                String::new()
            } else {
                format!("  ({})", notes.join("; "))
            };
            let _ = writeln!(
                out,
                "  {:key_w$}  {:action_w$}  {:7}  {}{notes}",
                e.key_label(),
                e.action,
                e.owner,
                e.does,
            );
        }
        for name in &self.unknown {
            let _ = writeln!(out, "  [debug_keys] {name}: no debug key has this name");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAYER: DebugKeyGroup = DebugKeyGroup {
        owner: "player",
        when: "always",
        keys: &[
            DebugKey {
                action: "stats",
                key: "F2",
                does: "stats",
            },
            DebugKey {
                action: "quick",
                key: "F5",
                does: "quick",
            },
        ],
    };
    const GROVE: DebugKeyGroup = DebugKeyGroup {
        owner: "living",
        when: "a scene with living props",
        keys: &[
            DebugKey {
                action: "pick",
                key: "F8",
                does: "pick",
            },
            DebugKey {
                action: "pick_next",
                key: "Shift+F8",
                does: "next",
            },
        ],
    };

    fn table(config: &str, grove_live: bool) -> DebugKeyTable {
        let config = InputConfig::from_toml_str(&format!("version = 1\n{config}")).unwrap();
        let always: &dyn Fn(&str) -> bool = &|_| true;
        let grove: &dyn Fn(&str) -> bool = &move |_| grove_live;
        DebugKeyTable::build([(PLAYER, always), (GROVE, grove)], &config)
    }

    #[test]
    fn chords_parse_and_print_as_written() {
        let shift_f8 = Chord::parse("Shift+F8").unwrap().unwrap();
        assert_eq!(
            shift_f8,
            Chord {
                key: KeyCode::F8,
                shift: true
            }
        );
        assert_eq!(shift_f8.to_string(), "Shift+F8");
        assert_eq!(Chord::parse("shift + F8").unwrap(), Some(shift_f8));
        assert_eq!(Chord::parse("none").unwrap(), None);
        assert_eq!(Chord::parse("").unwrap(), None);
        assert!(Chord::parse("Ctrl+F8").unwrap_err().contains("Shift"));
        assert!(Chord::parse("F99").unwrap_err().contains("not a key name"));
    }

    #[test]
    fn defaults_are_live_only_where_their_owner_is() {
        let t = table("", false);
        assert_eq!(t.lookup(KeyCode::F2, false).unwrap().action, "stats");
        assert!(t.lookup(KeyCode::F8, false).is_none(), "no grove, no F8");
        let t = table("", true);
        assert_eq!(t.lookup(KeyCode::F8, false).unwrap().action, "pick");
        assert_eq!(t.lookup(KeyCode::F8, true).unwrap().action, "pick_next");
        // A plain key's action still answers with Shift held
        assert_eq!(t.lookup(KeyCode::F2, true).unwrap().action, "stats");
        assert!(t.lookup(KeyCode::F9, false).is_none());
    }

    #[test]
    fn a_config_rebinds_unbinds_and_turns_them_off() {
        let t = table("[debug_keys]\nstats = \"F10\"\nquick = \"none\"\n", true);
        assert!(t.lookup(KeyCode::F2, false).is_none(), "moved off F2");
        assert_eq!(t.lookup(KeyCode::F10, false).unwrap().action, "stats");
        assert!(t.lookup(KeyCode::F5, false).is_none(), "unbound");
        assert!(t.text().contains("default F2"), "{}", t.text());
        let off = table("[debug_keys]\nenabled = false\n", true);
        assert!(!off.enabled);
        assert!(off.lookup(KeyCode::F2, false).is_none());
        assert!(off.text().contains("off"));
    }

    #[test]
    fn a_key_the_game_binds_is_the_games() {
        let t = table(
            "[actions.quicksave]\nkind = \"button\"\n[[actions.quicksave.bindings]]\ntype = \"key\"\ncode = \"F5\"\n",
            true,
        );
        assert!(t.lookup(KeyCode::F5, false).is_none());
        let quick = t.entries.iter().find(|e| e.action == "quick").unwrap();
        assert_eq!(quick.standing, Standing::TakenByGame("quicksave".into()));
        assert!(t.text().contains("the game's `quicksave` binds it"));
    }

    #[test]
    fn a_clash_keeps_the_first_and_an_unknown_name_is_reported() {
        let t = table("[debug_keys]\npick = \"F2\"\nzoom = \"F12\"\n", true);
        assert_eq!(t.lookup(KeyCode::F2, false).unwrap().action, "stats");
        let pick = t.entries.iter().find(|e| e.action == "pick").unwrap();
        assert_eq!(pick.standing, Standing::Clash("stats"));
        assert_eq!(t.unknown, ["zoom"]);
        // With the grove gone there is no clash to report
        let t = table("[debug_keys]\npick = \"F2\"\n", false);
        let pick = t.entries.iter().find(|e| e.action == "pick").unwrap();
        assert_eq!(pick.standing, Standing::Active);
    }

    #[test]
    fn layers_merge_enabled_and_each_name() {
        let mut a = DebugKeysConfig::default();
        a.keys.insert("stats".into(), "F10".into());
        a.keys.insert("pick".into(), "F7".into());
        let mut b = DebugKeysConfig {
            enabled: Some(false),
            ..Default::default()
        };
        b.keys.insert("pick".into(), "F6".into());
        a.merge(b);
        assert_eq!(a.enabled, Some(false));
        assert_eq!(a.keys["stats"], "F10");
        assert_eq!(a.keys["pick"], "F6");
        assert!(
            InputConfig::from_toml_str("version = 1\n[debug_keys]\nstats = \"F99\"\n")
                .unwrap_err()
                .to_string()
                .contains("[debug_keys] stats")
        );
    }
}
