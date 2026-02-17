//! Game State Machine — pushdown automaton for controlling engine systems.
//!
//! The state machine maintains a stack of named states. The top state's
//! [`StateConfig`] determines which engine systems run, pause, or hide.
//! States can be pushed (overlay), popped, or replaced (swap top).

use std::collections::HashMap;

/// Controls whether a system runs, pauses, or is completely hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemPolicy {
    /// System runs normally.
    Run,
    /// System is paused (state preserved but not ticked).
    Pause,
    /// System is hidden (not ticked and not rendered).
    Hidden,
}

/// Configuration for a game state — which systems run and transparency.
#[derive(Debug, Clone)]
pub struct StateConfig {
    pub physics: SystemPolicy,
    pub scripts: SystemPolicy,
    pub animation: SystemPolicy,
    pub particles: SystemPolicy,
    pub audio: SystemPolicy,
    pub rendering: SystemPolicy,
    /// If true, the state below still renders (for pause overlays).
    pub transparent: bool,
}

impl StateConfig {
    /// All systems running, not transparent.
    pub fn playing() -> Self {
        Self {
            physics: SystemPolicy::Run,
            scripts: SystemPolicy::Run,
            animation: SystemPolicy::Run,
            particles: SystemPolicy::Run,
            audio: SystemPolicy::Run,
            rendering: SystemPolicy::Run,
            transparent: false,
        }
    }

    /// Pause overlay — simulation paused, rendering active, transparent.
    pub fn paused() -> Self {
        Self {
            physics: SystemPolicy::Pause,
            scripts: SystemPolicy::Pause,
            animation: SystemPolicy::Pause,
            particles: SystemPolicy::Pause,
            audio: SystemPolicy::Pause,
            rendering: SystemPolicy::Run,
            transparent: true,
        }
    }

    /// Loading screen — all paused except rendering.
    pub fn loading() -> Self {
        Self {
            physics: SystemPolicy::Pause,
            scripts: SystemPolicy::Pause,
            animation: SystemPolicy::Pause,
            particles: SystemPolicy::Pause,
            audio: SystemPolicy::Pause,
            rendering: SystemPolicy::Run,
            transparent: false,
        }
    }
}

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
        }
    }

    /// Register a named state template for later use with `push_state`/`replace_state`.
    pub fn register_state(&mut self, name: &str, config: StateConfig) {
        self.templates.insert(name.to_string(), config);
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
        self.stack
            .last()
            .map(|s| &s.config)
            .unwrap_or_else(|| {
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
            StateConfig {
                physics: SystemPolicy::Pause,
                scripts: SystemPolicy::Run,
                animation: SystemPolicy::Run,
                particles: SystemPolicy::Run,
                audio: SystemPolicy::Run,
                rendering: SystemPolicy::Run,
                transparent: false,
            },
        );
        assert!(sm.push_state("cutscene"));
        assert_eq!(sm.current_state(), "cutscene");
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
        assert_eq!(sm.active_config().physics, SystemPolicy::Run);

        sm.push_state("paused");
        assert_eq!(sm.active_config().physics, SystemPolicy::Pause);
        assert!(sm.active_config().transparent);
    }

    #[test]
    fn builtin_templates() {
        let playing = StateConfig::playing();
        assert_eq!(playing.physics, SystemPolicy::Run);
        assert!(!playing.transparent);

        let paused = StateConfig::paused();
        assert_eq!(paused.physics, SystemPolicy::Pause);
        assert!(paused.transparent);

        let loading = StateConfig::loading();
        assert_eq!(loading.physics, SystemPolicy::Pause);
        assert!(!loading.transparent);
    }
}
