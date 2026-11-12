//! Game states: a stack of named states, the top one saying which of the
//! player's systems run this frame (a pause menu over play pauses physics and
//! scripts). States can be pushed (overlay), popped, or replaced (swap top).
//!
//! A state is a policy for each system, by the system's name
//! ([`StateConfig::runs`]): the built-in ones ([`BUILTIN_SYSTEMS`]) and those
//! a host adds with [`GameStateMachine::add_system`] (the player's `living`).
//! [`GameStateMachine::register_state`] refuses a name no system answers to,
//! so a state can't ask for something nothing does (#727). A state never
//! hides the frame: the world draws under every state.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

/// Rapier's fixed steps and the character controller.
pub const PHYSICS: &str = "physics";
/// Scripts' `on_update` and their events (`on_draw_ui` runs under any state),
/// and style tweens.
pub const SCRIPTS: &str = "scripts";
/// Property, skeletal, node and sprite animation and sequences.
pub const ANIMATION: &str = "animation";
/// The particle simulation.
pub const PARTICLES: &str = "particles";
/// Audio triggers, source sync and the FPS listener.
pub const AUDIO: &str = "audio";

/// The systems every host that has a state machine runs, by the names a
/// state's policy uses.
pub const BUILTIN_SYSTEMS: &[&str] = &[PHYSICS, SCRIPTS, ANIMATION, PARTICLES, AUDIO];

/// Whether a system runs under a state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemPolicy {
    /// System runs normally.
    Run,
    /// System is paused (state preserved but not ticked).
    Pause,
}

impl SystemPolicy {
    /// `"run"` or `"pause"` (or capitalized).
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "run" | "Run" => Some(Self::Run),
            "pause" | "Pause" => Some(Self::Pause),
            _ => None,
        }
    }
}

/// Which systems a state runs and pauses, by name. A system the state
/// doesn't name gets the state's default: run for `playing` and for a
/// script's own state, pause for `paused` and `loading`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateConfig {
    otherwise: SystemPolicy,
    named: BTreeMap<String, SystemPolicy>,
}

impl StateConfig {
    /// Every system runs.
    pub fn playing() -> Self {
        Self::all(SystemPolicy::Run)
    }

    /// Every system paused; the frame still draws and `on_draw_ui` still
    /// runs, for the menu.
    pub fn paused() -> Self {
        Self::all(SystemPolicy::Pause)
    }

    /// Every system paused, as `paused`.
    pub fn loading() -> Self {
        Self::all(SystemPolicy::Pause)
    }

    /// Every system under `policy` until [`with`](Self::with) names one.
    pub fn all(policy: SystemPolicy) -> Self {
        Self {
            otherwise: policy,
            named: BTreeMap::new(),
        }
    }

    /// The same, with `system` under `policy`.
    pub fn with(mut self, system: &str, policy: SystemPolicy) -> Self {
        self.named.insert(system.to_string(), policy);
        self
    }

    /// What `system` does under this state.
    pub fn policy(&self, system: &str) -> SystemPolicy {
        self.named.get(system).copied().unwrap_or(self.otherwise)
    }

    /// Whether `system` runs under this state.
    pub fn runs(&self, system: &str) -> bool {
        self.policy(system) == SystemPolicy::Run
    }

    /// The systems this state names, with their policies.
    pub fn named(&self) -> impl Iterator<Item = (&str, SystemPolicy)> {
        self.named.iter().map(|(k, v)| (k.as_str(), *v))
    }

    /// A state from a script's map, `system = "run" | "pause"`, with every
    /// system it doesn't name running. Refuses what was once documented and
    /// never done: `rendering`, `transparent` and `"hidden"`. Which systems
    /// exist is the machine's to say ([`GameStateMachine::register_state`]).
    pub fn parse<'a>(
        entries: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, StateError> {
        let mut config = Self::playing();
        for (key, value) in entries {
            if NOT_IMPLEMENTED_KEYS.contains(&key) {
                return Err(StateError::NotImplemented {
                    key: key.to_string(),
                    value: None,
                });
            }
            if value.eq_ignore_ascii_case("hidden") {
                return Err(StateError::NotImplemented {
                    key: key.to_string(),
                    value: Some(value.to_string()),
                });
            }
            let policy = SystemPolicy::parse(value).ok_or_else(|| StateError::BadPolicy {
                key: key.to_string(),
                value: value.to_string(),
            })?;
            config = config.with(key, policy);
        }
        Ok(config)
    }
}

/// Keys a state once took and nothing ever read (#727).
const NOT_IMPLEMENTED_KEYS: &[&str] = &["rendering", "transparent"];

/// Why a state can't be registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateError {
    /// The state names a system the host doesn't have.
    UnknownSystem { key: String, known: Vec<String> },
    /// `rendering`, `transparent`, or a `"hidden"` policy.
    NotImplemented { key: String, value: Option<String> },
    /// A policy other than `"run"` or `"pause"`.
    BadPolicy { key: String, value: String },
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSystem { key, known } => write!(
                f,
                "`{key}` is not a system a state can run or pause; the systems are {}",
                known.join(", ")
            ),
            Self::NotImplemented { key, value: None } if key == "transparent" => write!(
                f,
                "`transparent` is not implemented: the world under a state always draws, so leave it out"
            ),
            Self::NotImplemented { key, value: None } => write!(
                f,
                "`{key}` is not implemented: a state runs or pauses systems and the frame always draws, so leave it out"
            ),
            Self::NotImplemented {
                key,
                value: Some(value),
            } => write!(
                f,
                "`{key}: \"{value}\"` is not implemented: a state can run or pause a system, not hide it"
            ),
            Self::BadPolicy { key, value } => write!(
                f,
                "`{key}: \"{value}\"` is not a policy: use \"run\" or \"pause\""
            ),
        }
    }
}

impl std::error::Error for StateError {}

/// A named game state with its system configuration.
#[derive(Debug, Clone)]
pub struct GameState {
    pub name: String,
    pub config: StateConfig,
}

/// Pushdown automaton state machine.
///
/// Maintains a stack of [`GameState`]s. The top state determines which
/// systems run. Pre-registered templates allow states to be pushed by name.
pub struct GameStateMachine {
    stack: Vec<GameState>,
    templates: HashMap<String, StateConfig>,
    /// The systems a state may name: the built-in ones, then the host's.
    systems: Vec<String>,
}

impl GameStateMachine {
    /// Creates a new state machine with built-in templates and "playing" as the initial state.
    pub fn new() -> Self {
        let mut templates = HashMap::new();
        templates.insert("playing".to_string(), StateConfig::playing());
        templates.insert("paused".to_string(), StateConfig::paused());
        templates.insert("loading".to_string(), StateConfig::loading());

        let initial = GameState {
            name: "playing".to_string(),
            config: StateConfig::playing(),
        };

        Self {
            stack: vec![initial],
            templates,
            systems: BUILTIN_SYSTEMS.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// A system of the host's own that reads its policy by `name`, so a
    /// state may name it.
    pub fn add_system(&mut self, name: &str) {
        if !self.systems.iter().any(|s| s == name) {
            self.systems.push(name.to_string());
        }
    }

    /// The systems a state may name.
    pub fn systems(&self) -> &[String] {
        &self.systems
    }

    /// Register a named state template for later use with
    /// `push_state`/`replace_state`. Refuses a state naming a system this
    /// machine's host doesn't have, and leaves any state of that name as it
    /// was.
    pub fn register_state(&mut self, name: &str, config: StateConfig) -> Result<(), StateError> {
        if let Some((key, _)) = config
            .named()
            .find(|(key, _)| !self.systems.iter().any(|s| s == key))
        {
            return Err(StateError::UnknownSystem {
                key: key.to_string(),
                known: self.systems.clone(),
            });
        }
        self.templates.insert(name.to_string(), config);
        Ok(())
    }

    /// Push a state onto the stack by template name. Returns false if template not found.
    pub fn push_state(&mut self, name: &str) -> bool {
        if let Some(config) = self.templates.get(name).cloned() {
            self.stack.push(GameState {
                name: name.to_string(),
                config,
            });
            true
        } else {
            false
        }
    }

    /// Pop the top state. Returns the popped state, or None if only one state remains.
    pub fn pop_state(&mut self) -> Option<GameState> {
        if self.stack.len() > 1 {
            self.stack.pop()
        } else {
            None
        }
    }

    /// Replace the top state with a new one by template name. Returns false if template not found.
    pub fn replace_state(&mut self, name: &str) -> bool {
        if let Some(config) = self.templates.get(name).cloned() {
            if let Some(top) = self.stack.last_mut() {
                top.name = name.to_string();
                top.config = config;
            }
            true
        } else {
            false
        }
    }

    /// Returns the name of the current (top) state.
    pub fn current_state(&self) -> &str {
        self.stack
            .last()
            .map(|s| s.name.as_str())
            .unwrap_or("playing")
    }

    /// Returns the names of all states on the stack (bottom to top).
    pub fn stack_names(&self) -> Vec<&str> {
        self.stack.iter().map(|s| s.name.as_str()).collect()
    }

    /// Returns the active configuration (from the top state).
    pub fn active_config(&self) -> &StateConfig {
        self.stack.last().map(|s| &s.config).unwrap_or_else(|| {
            // Should never happen since we prevent popping the last state
            &self.stack[0].config
        })
    }

    /// Returns the number of states on the stack.
    pub fn stack_depth(&self) -> usize {
        self.stack.len()
    }
}

impl Default for GameStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_starts_with_playing() {
        let sm = GameStateMachine::new();
        assert_eq!(sm.current_state(), "playing");
        assert_eq!(sm.stack_depth(), 1);
    }

    #[test]
    fn push_and_pop() {
        let mut sm = GameStateMachine::new();
        assert!(sm.push_state("paused"));
        assert_eq!(sm.current_state(), "paused");
        assert_eq!(sm.stack_depth(), 2);

        let popped = sm.pop_state();
        assert!(popped.is_some());
        assert_eq!(popped.unwrap().name, "paused");
        assert_eq!(sm.current_state(), "playing");
    }

    #[test]
    fn cannot_pop_last_state() {
        let mut sm = GameStateMachine::new();
        assert!(sm.pop_state().is_none());
        assert_eq!(sm.stack_depth(), 1);
    }

    #[test]
    fn replace_state() {
        let mut sm = GameStateMachine::new();
        assert!(sm.replace_state("loading"));
        assert_eq!(sm.current_state(), "loading");
        assert_eq!(sm.stack_depth(), 1);
    }

    #[test]
    fn push_unknown_template_fails() {
        let mut sm = GameStateMachine::new();
        assert!(!sm.push_state("nonexistent"));
        assert_eq!(sm.stack_depth(), 1);
    }

    #[test]
    fn replace_unknown_template_fails() {
        let mut sm = GameStateMachine::new();
        assert!(!sm.replace_state("nonexistent"));
        assert_eq!(sm.current_state(), "playing");
    }

    #[test]
    fn register_custom_state() {
        let mut sm = GameStateMachine::new();
        sm.register_state(
            "cutscene",
            StateConfig::playing().with(PHYSICS, SystemPolicy::Pause),
        )
        .unwrap();
        assert!(sm.push_state("cutscene"));
        assert_eq!(sm.current_state(), "cutscene");
        assert!(!sm.active_config().runs(PHYSICS));
        assert!(sm.active_config().runs(SCRIPTS));
    }

    #[test]
    fn stack_names_order() {
        let mut sm = GameStateMachine::new();
        sm.push_state("paused");
        sm.push_state("loading");
        assert_eq!(sm.stack_names(), vec!["playing", "paused", "loading"]);
    }

    #[test]
    fn active_config_reflects_top() {
        let mut sm = GameStateMachine::new();
        assert!(sm.active_config().runs(PHYSICS));

        sm.push_state("paused");
        assert_eq!(sm.active_config().policy(PHYSICS), SystemPolicy::Pause);
    }

    #[test]
    fn builtin_templates() {
        for system in BUILTIN_SYSTEMS {
            assert!(StateConfig::playing().runs(system));
            assert!(!StateConfig::paused().runs(system));
            assert!(!StateConfig::loading().runs(system));
        }
        // A host's own system follows the template too: `paused` pauses the
        // player's living props, as it did when they followed `animation`.
        assert!(!StateConfig::paused().runs("living"));
        assert!(StateConfig::playing().runs("living"));
    }

    #[test]
    fn a_state_names_only_the_systems_it_changes_and_the_rest_run() {
        let config = StateConfig::parse([("physics", "pause"), ("audio", "Run")]).unwrap();
        assert_eq!(config.policy(PHYSICS), SystemPolicy::Pause);
        assert_eq!(config.policy(AUDIO), SystemPolicy::Run);
        assert_eq!(config.policy(SCRIPTS), SystemPolicy::Run);
        assert_eq!(config.named().count(), 2);
    }

    #[test]
    fn what_was_documented_and_never_done_is_refused_by_name() {
        for key in ["rendering", "transparent"] {
            let err = StateConfig::parse([(key, "run")]).unwrap_err();
            assert!(matches!(&err, StateError::NotImplemented { key: k, .. } if k == key));
            let message = err.to_string();
            assert!(message.contains(key), "{message}");
            assert!(message.contains("not implemented"), "{message}");
        }
        let err = StateConfig::parse([("physics", "hidden")]).unwrap_err();
        assert!(err.to_string().contains("physics: \"hidden\""), "{err}");
        assert!(err.to_string().contains("not implemented"), "{err}");
    }

    #[test]
    fn a_policy_that_is_neither_run_nor_pause_is_refused() {
        let err = StateConfig::parse([("audio", "loud")]).unwrap_err();
        assert_eq!(
            err,
            StateError::BadPolicy {
                key: "audio".into(),
                value: "loud".into()
            }
        );
        assert!(err.to_string().contains("\"run\" or \"pause\""), "{err}");
    }

    #[test]
    fn a_state_naming_a_system_the_host_lacks_is_refused_and_registers_nothing() {
        let mut sm = GameStateMachine::new();
        let config = StateConfig::parse([("living", "pause")]).unwrap();
        let err = sm.register_state("menu", config.clone()).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("`living`"), "{message}");
        assert!(message.contains("physics, scripts"), "{message}");
        assert!(!sm.push_state("menu"));

        // Once the host says it has one, the same state registers.
        sm.add_system("living");
        sm.register_state("menu", config).unwrap();
        assert!(sm.push_state("menu"));
        assert!(!sm.active_config().runs("living"));
        assert!(sm.active_config().runs(ANIMATION));
    }
}
