//! Input state management

use flint_core::{FlintError, Result};
use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::Instant;
use winit::keyboard::KeyCode;

#[derive(Debug, Clone)]
pub struct TouchPoint {
    pub id: u64,
    pub position: (f64, f64),
    pub start_position: (f64, f64),
    pub phase: TouchPhase,
    pub start_time: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SwipeDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputConfig {
    #[serde(default = "default_config_version")]
    pub version: u32,
    #[serde(default)]
    pub game_id: String,
    #[serde(default)]
    pub actions: BTreeMap<String, ActionConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionConfig {
    #[serde(default)]
    pub kind: ActionKind,
    #[serde(default)]
    pub bindings: Vec<Binding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ActionKind {
    #[default]
    Button,
    Axis1d,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GamepadSelector {
    #[default]
    Any,
    Index(u32),
}

impl Serialize for GamepadSelector {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            GamepadSelector::Any => serializer.serialize_str("any"),
            GamepadSelector::Index(index) => serializer.serialize_u32(*index),
        }
    }
}

impl<'de> Deserialize<'de> for GamepadSelector {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Name(String),
            Index(u32),
        }

        match Repr::deserialize(deserializer)? {
            Repr::Name(name) if name.eq_ignore_ascii_case("any") => Ok(GamepadSelector::Any),
            Repr::Name(name) => Err(de::Error::custom(format!(
                "invalid gamepad selector '{name}', expected 'any' or index"
            ))),
            Repr::Index(index) => Ok(GamepadSelector::Index(index)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AxisDirection {
    Positive,
    Negative,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Binding {
    Key {
        code: String,
        #[serde(default = "default_scale")]
        scale: f32,
    },
    MouseButton {
        button: String,
    },
    MouseDelta {
        axis: String,
        #[serde(default = "default_scale")]
        scale: f32,
    },
    MouseWheel {
        axis: String,
        #[serde(default = "default_scale")]
        scale: f32,
    },
    GamepadButton {
        button: String,
        #[serde(default)]
        gamepad: GamepadSelector,
    },
    GamepadAxis {
        axis: String,
        #[serde(default)]
        gamepad: GamepadSelector,
        #[serde(default = "default_deadzone")]
        deadzone: f32,
        #[serde(default = "default_scale")]
        scale: f32,
        #[serde(default)]
        invert: bool,
        #[serde(default)]
        threshold: Option<f32>,
        #[serde(default)]
        direction: Option<AxisDirection>,
    },
    TouchZone {
        zone: String,
        #[serde(default = "default_scale")]
        scale: f32,
    },
    Swipe {
        direction: SwipeDirection,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebindMode {
    Replace,
    Add,
    Swap,
}

impl Default for ActionConfig {
    fn default() -> Self {
        Self {
            kind: ActionKind::Button,
            bindings: Vec::new(),
        }
    }
}

impl Default for InputConfig {
    fn default() -> Self {
        Self::built_in_defaults()
    }
}

fn default_config_version() -> u32 {
    1
}

fn default_scale() -> f32 {
    1.0
}

fn default_deadzone() -> f32 {
    0.15
}

impl InputConfig {
    pub fn built_in_defaults() -> Self {
        let mut actions = BTreeMap::new();
        for (action, key) in [
            ("move_forward", "KeyW"),
            ("move_backward", "KeyS"),
            ("move_left", "KeyA"),
            ("move_right", "KeyD"),
            ("jump", "Space"),
            ("interact", "KeyE"),
            ("sprint", "ShiftLeft"),
            ("weapon_1", "Digit1"),
            ("weapon_2", "Digit2"),
            ("reload", "KeyR"),
        ] {
            actions.insert(
                action.into(),
                ActionConfig {
                    kind: ActionKind::Button,
                    bindings: vec![Binding::Key {
                        code: key.into(),
                        scale: 1.0,
                    }],
                },
            );
        }
        actions.insert(
            "fire".into(),
            ActionConfig {
                kind: ActionKind::Button,
                bindings: vec![Binding::MouseButton {
                    button: "Left".into(),
                }],
            },
        );
        Self {
            version: default_config_version(),
            game_id: "flint".into(),
            actions,
        }
    }

    pub fn from_toml_str(content: &str) -> Result<Self> {
        let config: InputConfig = toml::from_str(content)
            .map_err(|e| FlintError::RuntimeError(format!("invalid input config: {e}")))?;
        config.validate()?;
        Ok(config)
    }

    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path).map_err(|e| {
            FlintError::RuntimeError(format!(
                "failed to read input config '{}': {e}",
                path.display()
            ))
        })?;
        Self::from_toml_str(&content)
    }

    pub fn validate(&self) -> Result<()> {
        if self.version == 0 {
            return Err(FlintError::RuntimeError(
                "input config version must be >= 1".into(),
            ));
        }

        for (action, cfg) in &self.actions {
            if action.trim().is_empty() {
                return Err(FlintError::RuntimeError(
                    "action name cannot be empty".into(),
                ));
            }
            for binding in &cfg.bindings {
                validate_binding(binding, action)?;
            }
        }
        Ok(())
    }
}

/// Tracks keyboard, mouse, and gamepad input state per frame.
pub struct InputState {
    keys_down: HashSet<KeyCode>,
    keys_just_pressed: HashSet<KeyCode>,
    keys_just_released: HashSet<KeyCode>,

    mouse_buttons_down: HashSet<u32>,
    mouse_buttons_just_pressed: HashSet<u32>,
    mouse_buttons_just_released: HashSet<u32>,

    gamepad_buttons_down: HashSet<(u32, String)>,
    gamepad_buttons_just_pressed: HashSet<(u32, String)>,
    gamepad_buttons_just_released: HashSet<(u32, String)>,
    gamepad_button_values: HashMap<(u32, String), f32>,
    gamepad_axes: HashMap<(u32, String), f32>,
    last_frame_gamepad_axes: HashMap<(u32, String), f32>,

    pub mouse_position: (f64, f64),
    mouse_delta: (f64, f64),
    raw_mouse_delta: (f64, f64),
    mouse_wheel_delta: (f32, f32),

    touches: HashMap<u64, TouchPoint>,
    touches_just_started: HashSet<u64>,
    touches_just_ended: HashSet<u64>,
    touch_tap_just_fired: Vec<(f64, f64)>,
    touch_swipe_just_fired: Vec<(SwipeDirection, f64, f64)>,
    screen_size: (f64, f64),
    emulate_touch_from_mouse: bool,
    mouse_touch_active: bool,

    config: InputConfig,
    last_frame_pressed_actions: HashSet<String>,
}

impl Default for InputState {
    fn default() -> Self {
        Self::new()
    }
}

impl InputState {
    pub fn new() -> Self {
        Self {
            keys_down: HashSet::new(),
            keys_just_pressed: HashSet::new(),
            keys_just_released: HashSet::new(),
            mouse_buttons_down: HashSet::new(),
            mouse_buttons_just_pressed: HashSet::new(),
            mouse_buttons_just_released: HashSet::new(),
            gamepad_buttons_down: HashSet::new(),
            gamepad_buttons_just_pressed: HashSet::new(),
            gamepad_buttons_just_released: HashSet::new(),
            gamepad_button_values: HashMap::new(),
            gamepad_axes: HashMap::new(),
            last_frame_gamepad_axes: HashMap::new(),
            mouse_position: (0.0, 0.0),
            mouse_delta: (0.0, 0.0),
            raw_mouse_delta: (0.0, 0.0),
            mouse_wheel_delta: (0.0, 0.0),
            touches: HashMap::new(),
            touches_just_started: HashSet::new(),
            touches_just_ended: HashSet::new(),
            touch_tap_just_fired: Vec::new(),
            touch_swipe_just_fired: Vec::new(),
            screen_size: (1280.0, 720.0),
            emulate_touch_from_mouse: true,
            mouse_touch_active: false,
            config: InputConfig::built_in_defaults(),
            last_frame_pressed_actions: HashSet::new(),
        }
    }

    pub fn config(&self) -> &InputConfig {
        &self.config
    }

    pub fn all_action_names(&self) -> Vec<String> {
        self.config.actions.keys().cloned().collect()
    }

    pub fn action_config(&self, action: &str) -> Option<ActionConfig> {
        self.config.actions.get(action).cloned()
    }

    pub fn load_bindings(&mut self, config: InputConfig) -> Result<()> {
        config.validate()?;
        self.config = config;
        self.last_frame_pressed_actions.clear();
        Ok(())
    }

    pub fn merge_bindings(&mut self, overlay: InputConfig) -> Result<()> {
        overlay.validate()?;
        self.config.version = overlay.version;
        if !overlay.game_id.trim().is_empty() {
            self.config.game_id = overlay.game_id;
        }
        for (action, cfg) in overlay.actions {
            self.config.actions.insert(action, cfg);
        }
        self.last_frame_pressed_actions
            .retain(|action| self.config.actions.contains_key(action));
        Ok(())
    }

    pub fn rebind_action(
        &mut self,
        action: &str,
        binding: Binding,
        mode: RebindMode,
    ) -> Result<()> {
        validate_binding(&binding, action)?;

        match mode {
            RebindMode::Replace => {
                let entry =
                    self.config
                        .actions
                        .entry(action.into())
                        .or_insert_with(|| ActionConfig {
                            kind: infer_action_kind_for_new_action(&binding),
                            bindings: Vec::new(),
                        });
                entry.bindings = vec![binding];
            }
            RebindMode::Add => {
                let entry =
                    self.config
                        .actions
                        .entry(action.into())
                        .or_insert_with(|| ActionConfig {
                            kind: infer_action_kind_for_new_action(&binding),
                            bindings: Vec::new(),
                        });
                if !entry.bindings.contains(&binding) {
                    entry.bindings.push(binding);
                }
            }
            RebindMode::Swap => {
                for cfg in self.config.actions.values_mut() {
                    cfg.bindings.retain(|b| b != &binding);
                }
                let entry =
                    self.config
                        .actions
                        .entry(action.into())
                        .or_insert_with(|| ActionConfig {
                            kind: infer_action_kind_for_new_action(&binding),
                            bindings: Vec::new(),
                        });
                entry.bindings.clear();
                entry.bindings.push(binding);
            }
        }

        Ok(())
    }

    pub fn clear_action_bindings(&mut self, action: &str) -> bool {
        if let Some(cfg) = self.config.actions.get_mut(action) {
            let had = !cfg.bindings.is_empty();
            cfg.bindings.clear();
            return had;
        }
        false
    }

    pub fn bind_action(&mut self, action: impl Into<String>, keys: Vec<KeyCode>) {
        self.config.actions.insert(
            action.into(),
            ActionConfig {
                kind: ActionKind::Button,
                bindings: keys
                    .into_iter()
                    .map(|key| Binding::Key {
                        code: format!("{key:?}"),
                        scale: 1.0,
                    })
                    .collect(),
            },
        );
    }

    pub fn primary_binding_label(&self, action: &str) -> Option<String> {
        self.config
            .actions
            .get(action)?
            .bindings
            .first()
            .map(binding_label)
    }

    pub fn process_key_down(&mut self, key: KeyCode) {
        if !self.keys_down.contains(&key) {
            self.keys_just_pressed.insert(key);
        }
        self.keys_down.insert(key);
    }

    pub fn process_key_up(&mut self, key: KeyCode) {
        self.keys_down.remove(&key);
        self.keys_just_released.insert(key);
    }

    pub fn process_mouse_button_down(&mut self, button: u32) {
        if !self.mouse_buttons_down.contains(&button) {
            self.mouse_buttons_just_pressed.insert(button);
        }
        self.mouse_buttons_down.insert(button);

        // Mouse-as-touch emulation: left click starts touch finger 0
        if self.emulate_touch_from_mouse && button == 0 {
            self.mouse_touch_active = true;
            self.process_touch_start(0, self.mouse_position.0, self.mouse_position.1);
        }
    }

    pub fn process_mouse_button_up(&mut self, button: u32) {
        self.mouse_buttons_down.remove(&button);
        self.mouse_buttons_just_released.insert(button);

        // Mouse-as-touch emulation: left release ends touch finger 0
        if self.emulate_touch_from_mouse && button == 0 && self.mouse_touch_active {
            self.mouse_touch_active = false;
            self.process_touch_end(0);
        }
    }

    pub fn process_mouse_move(&mut self, x: f64, y: f64) {
        self.mouse_delta.0 += x - self.mouse_position.0;
        self.mouse_delta.1 += y - self.mouse_position.1;
        self.mouse_position = (x, y);

        // Mouse-as-touch emulation: cursor move updates touch finger 0 if active
        if self.emulate_touch_from_mouse && self.mouse_touch_active {
            self.process_touch_move(0, x, y);
        }
    }

    pub fn process_mouse_raw_delta(&mut self, dx: f64, dy: f64) {
        self.raw_mouse_delta.0 += dx;
        self.raw_mouse_delta.1 += dy;
    }

    pub fn process_mouse_wheel(&mut self, dx: f32, dy: f32) {
        self.mouse_wheel_delta.0 += dx;
        self.mouse_wheel_delta.1 += dy;
    }

    // ─── Touch input ──────────────────────────────────────────

    pub fn set_screen_size(&mut self, w: f64, h: f64) {
        self.screen_size = (w.max(1.0), h.max(1.0));
    }

    pub fn screen_size(&self) -> (f64, f64) {
        self.screen_size
    }

    /// Disable mouse-as-touch emulation (called when a real touch event arrives)
    pub fn disable_touch_emulation(&mut self) {
        self.emulate_touch_from_mouse = false;
    }

    pub fn touch_emulation_active(&self) -> bool {
        self.emulate_touch_from_mouse
    }

    fn normalize_touch(&self, x: f64, y: f64) -> (f64, f64) {
        (x / self.screen_size.0, y / self.screen_size.1)
    }

    pub fn process_touch_start(&mut self, id: u64, x: f64, y: f64) {
        let norm = self.normalize_touch(x, y);
        self.touches.insert(
            id,
            TouchPoint {
                id,
                position: norm,
                start_position: norm,
                phase: TouchPhase::Started,
                start_time: Instant::now(),
            },
        );
        self.touches_just_started.insert(id);
    }

    pub fn process_touch_move(&mut self, id: u64, x: f64, y: f64) {
        let norm = self.normalize_touch(x, y);
        if let Some(touch) = self.touches.get_mut(&id) {
            touch.position = norm;
            touch.phase = TouchPhase::Moved;
        }
    }

    pub fn process_touch_end(&mut self, id: u64) {
        if let Some(touch) = self.touches.remove(&id) {
            let elapsed = touch.start_time.elapsed().as_millis();
            let dx = (touch.position.0 - touch.start_position.0) * self.screen_size.0;
            let dy = (touch.position.1 - touch.start_position.1) * self.screen_size.1;
            let dist_sq = dx * dx + dy * dy;

            // DEBUG: remove once swipe is confirmed working
            eprintln!(
                "[input] touch_end id={id} elapsed={elapsed}ms dx={dx:.1} dy={dy:.1} dist={:.1}",
                dist_sq.sqrt()
            );

            if elapsed < 300 && dist_sq < 20.0 * 20.0 {
                // Tap: short duration + small movement
                eprintln!("[input]   -> TAP");
                self.touch_tap_just_fired.push(touch.position);
            } else if elapsed < 500 && dist_sq >= 40.0 * 40.0 {
                // Swipe: fast enough + large enough movement
                let direction = if dx.abs() > dy.abs() {
                    if dx > 0.0 {
                        SwipeDirection::Right
                    } else {
                        SwipeDirection::Left
                    }
                } else if dy > 0.0 {
                    SwipeDirection::Down
                } else {
                    SwipeDirection::Up
                };
                eprintln!("[input]   -> SWIPE {direction:?}");
                self.touch_swipe_just_fired
                    .push((direction, touch.start_position.0, touch.start_position.1));
            } else {
                eprintln!("[input]   -> DEAD ZONE (no gesture)");
            }
        }
        self.touches_just_ended.insert(id);
    }

    pub fn process_touch_cancel(&mut self, id: u64) {
        self.touches.remove(&id);
        self.touches_just_ended.insert(id);
    }

    pub fn touch_count(&self) -> usize {
        self.touches.len()
    }

    pub fn touch_position(&self, id: u64) -> Option<(f64, f64)> {
        self.touches.get(&id).map(|t| t.position)
    }

    pub fn touch_active(&self, id: u64) -> bool {
        self.touches.contains_key(&id)
    }

    pub fn touch_just_started(&self, id: u64) -> bool {
        self.touches_just_started.contains(&id)
    }

    pub fn touch_just_ended(&self, id: u64) -> bool {
        self.touches_just_ended.contains(&id)
    }

    pub fn touch_taps(&self) -> &[(f64, f64)] {
        &self.touch_tap_just_fired
    }

    pub fn touch_swipes(&self) -> &[(SwipeDirection, f64, f64)] {
        &self.touch_swipe_just_fired
    }

    pub fn active_touches(&self) -> &HashMap<u64, TouchPoint> {
        &self.touches
    }

    pub fn touches_just_started_set(&self) -> &HashSet<u64> {
        &self.touches_just_started
    }

    pub fn touches_just_ended_set(&self) -> &HashSet<u64> {
        &self.touches_just_ended
    }

    pub fn process_gamepad_button_down(&mut self, gamepad: u32, button: impl Into<String>) {
        let key = (gamepad, button.into());
        if !self.gamepad_buttons_down.contains(&key) {
            self.gamepad_buttons_just_pressed.insert(key.clone());
        }
        self.gamepad_buttons_down.insert(key);
    }

    pub fn process_gamepad_button_up(&mut self, gamepad: u32, button: impl Into<String>) {
        let key = (gamepad, button.into());
        self.gamepad_buttons_down.remove(&key);
        self.gamepad_button_values.remove(&key);
        self.gamepad_buttons_just_released.insert(key);
    }

    /// Store the analog value for a gamepad button (e.g. trigger pressure).
    /// gilrs fires ButtonChanged events with a 0.0–1.0 value for analog buttons.
    pub fn process_gamepad_button_changed(
        &mut self,
        gamepad: u32,
        button: impl Into<String>,
        value: f32,
    ) {
        self.gamepad_button_values
            .insert((gamepad, button.into()), value.clamp(0.0, 1.0));
    }

    pub fn process_gamepad_axis(&mut self, gamepad: u32, axis: impl Into<String>, value: f32) {
        self.gamepad_axes
            .insert((gamepad, axis.into()), value.clamp(-1.0, 1.0));
    }

    pub fn clear_gamepad(&mut self, gamepad: u32) {
        self.gamepad_buttons_down.retain(|(id, _)| *id != gamepad);
        self.gamepad_buttons_just_pressed
            .retain(|(id, _)| *id != gamepad);
        self.gamepad_buttons_just_released
            .retain(|(id, _)| *id != gamepad);
        self.gamepad_button_values
            .retain(|(id, _), _| *id != gamepad);
        self.gamepad_axes.retain(|(id, _), _| *id != gamepad);
        self.last_frame_gamepad_axes
            .retain(|(id, _), _| *id != gamepad);
    }

    pub fn end_frame(&mut self) {
        self.last_frame_pressed_actions = self.actions_pressed().into_iter().collect();
        self.last_frame_gamepad_axes = self.gamepad_axes.clone();

        self.keys_just_pressed.clear();
        self.keys_just_released.clear();
        self.mouse_buttons_just_pressed.clear();
        self.mouse_buttons_just_released.clear();
        self.gamepad_buttons_just_pressed.clear();
        self.gamepad_buttons_just_released.clear();
        self.mouse_delta = (0.0, 0.0);
        self.raw_mouse_delta = (0.0, 0.0);
        self.mouse_wheel_delta = (0.0, 0.0);
        self.touches_just_started.clear();
        self.touches_just_ended.clear();
        self.touch_tap_just_fired.clear();
        self.touch_swipe_just_fired.clear();
    }

    pub fn is_key_down(&self, key: KeyCode) -> bool {
        self.keys_down.contains(&key)
    }

    pub fn is_key_just_pressed(&self, key: KeyCode) -> bool {
        self.keys_just_pressed.contains(&key)
    }

    pub fn is_action_pressed(&self, action: &str) -> bool {
        self.evaluate_action(action).pressed
    }

    pub fn is_action_just_pressed(&self, action: &str) -> bool {
        self.evaluate_action(action).just_pressed
    }

    pub fn is_action_just_released(&self, action: &str) -> bool {
        self.evaluate_action(action).just_released
    }

    pub fn actions_pressed(&self) -> Vec<String> {
        self.config
            .actions
            .keys()
            .filter(|action| self.is_action_pressed(action))
            .cloned()
            .collect()
    }

    pub fn actions_just_pressed(&self) -> Vec<String> {
        self.config
            .actions
            .keys()
            .filter(|action| self.is_action_just_pressed(action))
            .cloned()
            .collect()
    }

    pub fn actions_just_released(&self) -> Vec<String> {
        self.config
            .actions
            .keys()
            .filter(|action| self.is_action_just_released(action))
            .cloned()
            .collect()
    }

    pub fn action_value(&self, action: &str) -> f32 {
        self.evaluate_action(action).value
    }

    /// Get all registered action names (alias for `all_action_names`)
    pub fn action_names(&self) -> Vec<String> {
        self.all_action_names()
    }

    pub fn mouse_delta(&self) -> (f64, f64) {
        self.mouse_delta
    }

    pub fn raw_mouse_delta(&self) -> (f64, f64) {
        self.raw_mouse_delta
    }

    pub fn is_mouse_button_down(&self, button: u32) -> bool {
        self.mouse_buttons_down.contains(&button)
    }
}

impl InputState {
    fn evaluate_action(&self, action: &str) -> EvaluatedAction {
        let Some(cfg) = self.config.actions.get(action) else {
            return EvaluatedAction::default();
        };

        let mut value: f32 = match cfg.kind {
            ActionKind::Button => 0.0,
            ActionKind::Axis1d => 0.0,
        };
        let mut just_pressed = false;
        let mut just_released = false;

        for binding in &cfg.bindings {
            let binding_value = self.binding_value(binding);
            match cfg.kind {
                ActionKind::Button => {
                    value = value.max(binding_value.abs());
                }
                ActionKind::Axis1d => {
                    value += binding_value;
                }
            }

            if self.binding_just_pressed(binding) {
                just_pressed = true;
            }
            if self.binding_just_released(binding) {
                just_released = true;
            }
        }

        let pressed = match cfg.kind {
            ActionKind::Button => value >= 0.5,
            ActionKind::Axis1d => value.abs() > 0.001,
        };

        // Fallback for non-discrete analog sources.
        let was_pressed = self.last_frame_pressed_actions.contains(action);
        if pressed && !was_pressed {
            just_pressed = true;
        }
        if !pressed && was_pressed {
            just_released = true;
        }

        EvaluatedAction {
            value,
            pressed,
            just_pressed,
            just_released,
        }
    }

    fn binding_value(&self, binding: &Binding) -> f32 {
        match binding {
            Binding::Key { code, scale } => {
                parse_key_code(code)
                    .map(|key| self.keys_down.contains(&key))
                    .unwrap_or(false) as i32 as f32
                    * *scale
            }
            Binding::MouseButton { button } => parse_mouse_button(button)
                .map(|btn| self.mouse_buttons_down.contains(&btn))
                .unwrap_or(false) as i32 as f32,
            Binding::MouseDelta { axis, scale } => match normalize_axis_name(axis).as_deref() {
                Some("x") => self.raw_mouse_delta.0 as f32 * *scale,
                Some("y") => self.raw_mouse_delta.1 as f32 * *scale,
                _ => 0.0,
            },
            Binding::MouseWheel { axis, scale } => match normalize_axis_name(axis).as_deref() {
                Some("x") => self.mouse_wheel_delta.0 * *scale,
                Some("y") => self.mouse_wheel_delta.1 * *scale,
                _ => 0.0,
            },
            Binding::GamepadButton { button, gamepad } => {
                // Return analog value if available (e.g. trigger pressure),
                // otherwise fall back to digital 0/1 from button_down state.
                let analog = self
                    .gamepad_button_values
                    .iter()
                    .filter(|((id, name), _)| selector_matches(*gamepad, *id) && name == button)
                    .map(|(_, v)| *v)
                    .reduce(f32::max);
                if let Some(val) = analog {
                    val
                } else {
                    self.gamepad_buttons_down
                        .iter()
                        .any(|(id, pressed_button)| {
                            selector_matches(*gamepad, *id) && pressed_button == button
                        }) as i32 as f32
                }
            }
            Binding::GamepadAxis {
                axis,
                gamepad,
                deadzone,
                scale,
                invert,
                threshold,
                direction,
            } => {
                let filtered = self.filtered_gamepad_axis_value(
                    &self.gamepad_axes,
                    *gamepad,
                    axis,
                    *deadzone,
                    *invert,
                    *direction,
                );
                if let Some(threshold) = threshold {
                    if filtered.abs() >= *threshold {
                        1.0
                    } else {
                        0.0
                    }
                } else {
                    filtered * *scale
                }
            }
            Binding::TouchZone { zone, scale } => {
                let any_in_zone = self
                    .touches
                    .values()
                    .any(|t| point_in_zone(t.position.0, t.position.1, zone));
                if any_in_zone {
                    *scale
                } else {
                    0.0
                }
            }
            Binding::Swipe { direction } => {
                if self
                    .touch_swipe_just_fired
                    .iter()
                    .any(|(d, _, _)| d == direction)
                {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }

    fn binding_just_pressed(&self, binding: &Binding) -> bool {
        match binding {
            Binding::Key { code, .. } => parse_key_code(code)
                .map(|key| self.keys_just_pressed.contains(&key))
                .unwrap_or(false),
            Binding::MouseButton { button } => parse_mouse_button(button)
                .map(|btn| self.mouse_buttons_just_pressed.contains(&btn))
                .unwrap_or(false),
            Binding::GamepadButton { button, gamepad } => self
                .gamepad_buttons_just_pressed
                .iter()
                .any(|(id, pressed_button)| {
                    selector_matches(*gamepad, *id) && pressed_button == button
                }),
            Binding::GamepadAxis {
                axis,
                gamepad,
                deadzone,
                invert,
                threshold,
                direction,
                ..
            } => {
                let Some(threshold) = threshold else {
                    return false;
                };
                let current = self.filtered_gamepad_axis_value(
                    &self.gamepad_axes,
                    *gamepad,
                    axis,
                    *deadzone,
                    *invert,
                    *direction,
                );
                let previous = self.filtered_gamepad_axis_value(
                    &self.last_frame_gamepad_axes,
                    *gamepad,
                    axis,
                    *deadzone,
                    *invert,
                    *direction,
                );
                current.abs() >= *threshold && previous.abs() < *threshold
            }
            Binding::TouchZone { zone, .. } => self.touches_just_started.iter().any(|id| {
                self.touches
                    .get(id)
                    .map(|t| point_in_zone(t.position.0, t.position.1, zone))
                    .unwrap_or(false)
            }),
            Binding::Swipe { direction } => self
                .touch_swipe_just_fired
                .iter()
                .any(|(d, _, _)| d == direction),
            _ => false,
        }
    }

    fn binding_just_released(&self, binding: &Binding) -> bool {
        match binding {
            Binding::Key { code, .. } => parse_key_code(code)
                .map(|key| self.keys_just_released.contains(&key))
                .unwrap_or(false),
            Binding::MouseButton { button } => parse_mouse_button(button)
                .map(|btn| self.mouse_buttons_just_released.contains(&btn))
                .unwrap_or(false),
            Binding::GamepadButton { button, gamepad } => self
                .gamepad_buttons_just_released
                .iter()
                .any(|(id, released_button)| {
                    selector_matches(*gamepad, *id) && released_button == button
                }),
            Binding::GamepadAxis {
                axis,
                gamepad,
                deadzone,
                invert,
                threshold,
                direction,
                ..
            } => {
                let Some(threshold) = threshold else {
                    return false;
                };
                let current = self.filtered_gamepad_axis_value(
                    &self.gamepad_axes,
                    *gamepad,
                    axis,
                    *deadzone,
                    *invert,
                    *direction,
                );
                let previous = self.filtered_gamepad_axis_value(
                    &self.last_frame_gamepad_axes,
                    *gamepad,
                    axis,
                    *deadzone,
                    *invert,
                    *direction,
                );
                current.abs() < *threshold && previous.abs() >= *threshold
            }
            Binding::TouchZone { zone: _, .. } => {
                // Touch zone "just released" is detected by the evaluate_action
                // fallback: was pressed last frame, not pressed this frame.
                false
            }
            Binding::Swipe { .. } => false,
            _ => false,
        }
    }

    fn filtered_gamepad_axis_value(
        &self,
        axis_map: &HashMap<(u32, String), f32>,
        selector: GamepadSelector,
        axis: &str,
        deadzone: f32,
        invert: bool,
        direction: Option<AxisDirection>,
    ) -> f32 {
        let mut selected: Option<f32> = None;
        for ((id, axis_name), value) in axis_map {
            if axis_name != axis || !selector_matches(selector, *id) {
                continue;
            }
            selected = match selected {
                Some(best) if best.abs() >= value.abs() => Some(best),
                _ => Some(*value),
            };
        }

        let Some(mut value) = selected else {
            return 0.0;
        };
        value = apply_deadzone(value, deadzone);
        if invert {
            value = -value;
        }
        match direction {
            Some(AxisDirection::Positive) => value.max(0.0),
            Some(AxisDirection::Negative) => (-value).max(0.0),
            None => value,
        }
    }
}

#[derive(Debug, Default)]
struct EvaluatedAction {
    value: f32,
    pressed: bool,
    just_pressed: bool,
    just_released: bool,
}

/// Returns the normalized rect (x, y, w, h) for a named touch zone.
fn touch_zone_rect(zone: &str) -> Option<(f64, f64, f64, f64)> {
    match zone {
        "full_screen" => Some((0.0, 0.0, 1.0, 1.0)),
        "left_half" => Some((0.0, 0.0, 0.5, 1.0)),
        "right_half" => Some((0.5, 0.0, 0.5, 1.0)),
        "top_half" => Some((0.0, 0.0, 1.0, 0.5)),
        "bottom_half" => Some((0.0, 0.5, 1.0, 0.5)),
        _ => None,
    }
}

fn point_in_zone(px: f64, py: f64, zone: &str) -> bool {
    if let Some((zx, zy, zw, zh)) = touch_zone_rect(zone) {
        px >= zx && px < zx + zw && py >= zy && py < zy + zh
    } else {
        false
    }
}

fn infer_action_kind_for_new_action(binding: &Binding) -> ActionKind {
    match binding {
        Binding::MouseDelta { .. } | Binding::MouseWheel { .. } => ActionKind::Axis1d,
        Binding::GamepadAxis { threshold, .. } if threshold.is_none() => ActionKind::Axis1d,
        _ => ActionKind::Button,
    }
}

fn apply_deadzone(value: f32, deadzone: f32) -> f32 {
    let abs = value.abs();
    if abs <= deadzone {
        return 0.0;
    }
    let denom = (1.0 - deadzone).max(f32::EPSILON);
    let normalized = (abs - deadzone) / denom;
    normalized.copysign(value)
}

fn selector_matches(selector: GamepadSelector, gamepad_id: u32) -> bool {
    match selector {
        GamepadSelector::Any => true,
        GamepadSelector::Index(index) => index == gamepad_id,
    }
}

fn validate_binding(binding: &Binding, action_name: &str) -> Result<()> {
    match binding {
        Binding::Key { code, .. } => {
            if parse_key_code(code).is_none() {
                return Err(FlintError::RuntimeError(format!(
                    "action '{action_name}' has invalid key code '{code}'"
                )));
            }
        }
        Binding::MouseButton { button } => {
            if parse_mouse_button(button).is_none() {
                return Err(FlintError::RuntimeError(format!(
                    "action '{action_name}' has invalid mouse button '{button}'"
                )));
            }
        }
        Binding::MouseDelta { axis, .. } | Binding::MouseWheel { axis, .. } => {
            if normalize_axis_name(axis).is_none() {
                return Err(FlintError::RuntimeError(format!(
                    "action '{action_name}' has invalid axis '{axis}', expected x or y"
                )));
            }
        }
        Binding::GamepadButton { button, .. } => {
            if button.trim().is_empty() {
                return Err(FlintError::RuntimeError(format!(
                    "action '{action_name}' has empty gamepad button binding"
                )));
            }
        }
        Binding::GamepadAxis {
            axis,
            deadzone,
            scale,
            threshold,
            ..
        } => {
            if axis.trim().is_empty() {
                return Err(FlintError::RuntimeError(format!(
                    "action '{action_name}' has empty gamepad axis binding"
                )));
            }
            if !deadzone.is_finite() || *deadzone < 0.0 || *deadzone >= 1.0 {
                return Err(FlintError::RuntimeError(format!(
                    "action '{action_name}' has invalid deadzone {deadzone}, expected [0, 1)"
                )));
            }
            if !scale.is_finite() {
                return Err(FlintError::RuntimeError(format!(
                    "action '{action_name}' has non-finite scale"
                )));
            }
            if let Some(threshold) = threshold {
                if !threshold.is_finite() || *threshold <= 0.0 || *threshold > 1.0 {
                    return Err(FlintError::RuntimeError(format!(
                        "action '{action_name}' has invalid threshold {threshold}, expected (0, 1]"
                    )));
                }
            }
        }
        Binding::TouchZone { zone, scale } => {
            if touch_zone_rect(zone).is_none() {
                return Err(FlintError::RuntimeError(format!(
                    "action '{action_name}' has unknown touch zone '{zone}', expected one of: full_screen, left_half, right_half, top_half, bottom_half"
                )));
            }
            if !scale.is_finite() {
                return Err(FlintError::RuntimeError(format!(
                    "action '{action_name}' has non-finite touch zone scale"
                )));
            }
        }
        Binding::Swipe { .. } => {
            // SwipeDirection is validated by serde deserialization
        }
    }
    Ok(())
}

fn binding_label(binding: &Binding) -> String {
    match binding {
        Binding::Key { code, .. } => key_code_label(code),
        Binding::MouseButton { button } => match button.as_str() {
            "Left" => "Mouse1".into(),
            "Right" => "Mouse2".into(),
            "Middle" => "Mouse3".into(),
            _ => format!("Mouse:{button}"),
        },
        Binding::MouseDelta { axis, .. } => format!("Mouse {}", axis.to_uppercase()),
        Binding::MouseWheel { axis, .. } => format!("Wheel {}", axis.to_uppercase()),
        Binding::GamepadButton { button, .. } => format!("Pad:{button}"),
        Binding::GamepadAxis {
            axis,
            direction,
            threshold,
            ..
        } => {
            let dir = match direction {
                Some(AxisDirection::Positive) => "+",
                Some(AxisDirection::Negative) => "-",
                None => "",
            };
            if let Some(threshold) = threshold {
                format!("Pad:{axis}{dir}@{threshold:.2}")
            } else {
                format!("Pad:{axis}{dir}")
            }
        }
        Binding::TouchZone { zone, .. } => format!("Touch:{zone}"),
        Binding::Swipe { direction } => format!("Swipe:{direction:?}"),
    }
}

fn key_code_label(code: &str) -> String {
    if let Some(suffix) = code.strip_prefix("Key") {
        return suffix.to_string();
    }
    if let Some(suffix) = code.strip_prefix("Digit") {
        return suffix.to_string();
    }
    match code {
        "ShiftLeft" => "LShift".into(),
        "ShiftRight" => "RShift".into(),
        "ControlLeft" => "LCtrl".into(),
        "ControlRight" => "RCtrl".into(),
        "AltLeft" => "LAlt".into(),
        "AltRight" => "RAlt".into(),
        _ => code.into(),
    }
}

fn normalize_axis_name(axis: &str) -> Option<String> {
    if axis.eq_ignore_ascii_case("x") {
        Some("x".into())
    } else if axis.eq_ignore_ascii_case("y") {
        Some("y".into())
    } else {
        None
    }
}

fn parse_mouse_button(button: &str) -> Option<u32> {
    match button {
        "Left" => Some(0),
        "Right" => Some(1),
        "Middle" => Some(2),
        "Back" => Some(3),
        "Forward" => Some(4),
        _ => None,
    }
}

fn parse_key_code(code: &str) -> Option<KeyCode> {
    // KeyA..KeyZ
    if code.len() == 4 && code.starts_with("Key") {
        return match code.as_bytes()[3] {
            b'A' => Some(KeyCode::KeyA),
            b'B' => Some(KeyCode::KeyB),
            b'C' => Some(KeyCode::KeyC),
            b'D' => Some(KeyCode::KeyD),
            b'E' => Some(KeyCode::KeyE),
            b'F' => Some(KeyCode::KeyF),
            b'G' => Some(KeyCode::KeyG),
            b'H' => Some(KeyCode::KeyH),
            b'I' => Some(KeyCode::KeyI),
            b'J' => Some(KeyCode::KeyJ),
            b'K' => Some(KeyCode::KeyK),
            b'L' => Some(KeyCode::KeyL),
            b'M' => Some(KeyCode::KeyM),
            b'N' => Some(KeyCode::KeyN),
            b'O' => Some(KeyCode::KeyO),
            b'P' => Some(KeyCode::KeyP),
            b'Q' => Some(KeyCode::KeyQ),
            b'R' => Some(KeyCode::KeyR),
            b'S' => Some(KeyCode::KeyS),
            b'T' => Some(KeyCode::KeyT),
            b'U' => Some(KeyCode::KeyU),
            b'V' => Some(KeyCode::KeyV),
            b'W' => Some(KeyCode::KeyW),
            b'X' => Some(KeyCode::KeyX),
            b'Y' => Some(KeyCode::KeyY),
            b'Z' => Some(KeyCode::KeyZ),
            _ => None,
        };
    }

    // Digit0..Digit9
    if code.len() == 6 && code.starts_with("Digit") {
        return match code.as_bytes()[5] {
            b'0' => Some(KeyCode::Digit0),
            b'1' => Some(KeyCode::Digit1),
            b'2' => Some(KeyCode::Digit2),
            b'3' => Some(KeyCode::Digit3),
            b'4' => Some(KeyCode::Digit4),
            b'5' => Some(KeyCode::Digit5),
            b'6' => Some(KeyCode::Digit6),
            b'7' => Some(KeyCode::Digit7),
            b'8' => Some(KeyCode::Digit8),
            b'9' => Some(KeyCode::Digit9),
            _ => None,
        };
    }

    match code {
        "Space" => Some(KeyCode::Space),
        "Tab" => Some(KeyCode::Tab),
        "Enter" => Some(KeyCode::Enter),
        "Backspace" => Some(KeyCode::Backspace),
        "Escape" => Some(KeyCode::Escape),
        "ShiftLeft" => Some(KeyCode::ShiftLeft),
        "ShiftRight" => Some(KeyCode::ShiftRight),
        "ControlLeft" => Some(KeyCode::ControlLeft),
        "ControlRight" => Some(KeyCode::ControlRight),
        "AltLeft" => Some(KeyCode::AltLeft),
        "AltRight" => Some(KeyCode::AltRight),
        "ArrowUp" => Some(KeyCode::ArrowUp),
        "ArrowDown" => Some(KeyCode::ArrowDown),
        "ArrowLeft" => Some(KeyCode::ArrowLeft),
        "ArrowRight" => Some(KeyCode::ArrowRight),
        "F1" => Some(KeyCode::F1),
        "F2" => Some(KeyCode::F2),
        "F3" => Some(KeyCode::F3),
        "F4" => Some(KeyCode::F4),
        "F5" => Some(KeyCode::F5),
        "F6" => Some(KeyCode::F6),
        "F7" => Some(KeyCode::F7),
        "F8" => Some(KeyCode::F8),
        "F9" => Some(KeyCode::F9),
        "F10" => Some(KeyCode::F10),
        "F11" => Some(KeyCode::F11),
        "F12" => Some(KeyCode::F12),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_transitions() {
        let mut input = InputState::new();

        input.process_key_down(KeyCode::KeyW);
        assert!(input.is_key_down(KeyCode::KeyW));
        assert!(input.is_key_just_pressed(KeyCode::KeyW));

        input.end_frame();
        assert!(input.is_key_down(KeyCode::KeyW));
        assert!(!input.is_key_just_pressed(KeyCode::KeyW));

        input.process_key_up(KeyCode::KeyW);
        assert!(!input.is_key_down(KeyCode::KeyW));
    }

    #[test]
    fn test_default_action_map() {
        let mut input = InputState::new();
        assert!(!input.is_action_pressed("move_forward"));

        input.process_key_down(KeyCode::KeyW);
        assert!(input.is_action_pressed("move_forward"));
        assert!(input.is_action_just_pressed("move_forward"));

        input.end_frame();
        assert!(input.is_action_pressed("move_forward"));
        assert!(!input.is_action_just_pressed("move_forward"));
    }

    #[test]
    fn test_actions_just_released() {
        let mut input = InputState::new();
        input.process_key_down(KeyCode::KeyW);
        input.end_frame();

        input.process_key_up(KeyCode::KeyW);
        assert!(input.is_action_just_released("move_forward"));
    }

    #[test]
    fn test_custom_binding_compat_api() {
        let mut input = InputState::new();
        input.bind_action("fire", vec![KeyCode::KeyF, KeyCode::ControlLeft]);

        input.process_key_down(KeyCode::KeyF);
        assert!(input.is_action_pressed("fire"));

        input.process_key_up(KeyCode::KeyF);
        input.end_frame();
        input.process_key_down(KeyCode::ControlLeft);
        assert!(input.is_action_pressed("fire"));
    }

    #[test]
    fn test_parse_input_config() {
        let config = InputConfig::from_toml_str(
            r#"
version = 1
game_id = "doom_fps"

[actions.fire]
kind = "button"
[[actions.fire.bindings]]
type = "mouse_button"
button = "Left"
"#,
        )
        .unwrap();

        assert_eq!(config.game_id, "doom_fps");
        assert!(config.actions.contains_key("fire"));
    }

    #[test]
    fn test_invalid_key_rejected() {
        let config = InputConfig::from_toml_str(
            r#"
version = 1

[actions.fire]
kind = "button"
[[actions.fire.bindings]]
type = "key"
code = "NotARealKey"
"#,
        );
        assert!(config.is_err());
    }

    #[test]
    fn test_gamepad_button_binding() {
        let mut input = InputState::new();
        input
            .load_bindings(
                InputConfig::from_toml_str(
                    r#"
version = 1
[actions.fire]
kind = "button"
[[actions.fire.bindings]]
type = "gamepad_button"
button = "RightTrigger"
gamepad = "any"
"#,
                )
                .unwrap(),
            )
            .unwrap();

        input.process_gamepad_button_down(0, "RightTrigger");
        assert!(input.is_action_pressed("fire"));
        assert!(input.is_action_just_pressed("fire"));
        input.process_gamepad_button_up(0, "RightTrigger");
        assert!(input.is_action_just_released("fire"));
    }

    #[test]
    fn test_axis_value_mouse_delta() {
        let mut input = InputState::new();
        input
            .load_bindings(
                InputConfig::from_toml_str(
                    r#"
version = 1
[actions.look_x]
kind = "axis1d"
[[actions.look_x.bindings]]
type = "mouse_delta"
axis = "x"
scale = 2.0
"#,
                )
                .unwrap(),
            )
            .unwrap();

        input.process_mouse_raw_delta(3.0, 0.0);
        assert!((input.action_value("look_x") - 6.0).abs() < 1e-10);
    }

    #[test]
    fn test_touch_start_end() {
        let mut input = InputState::new();
        input.set_screen_size(1000.0, 1000.0);

        input.process_touch_start(1, 500.0, 500.0);
        assert!(input.touch_active(1));
        assert!(input.touch_just_started(1));
        assert_eq!(input.touch_count(), 1);

        let pos = input.touch_position(1).unwrap();
        assert!((pos.0 - 0.5).abs() < 1e-10);
        assert!((pos.1 - 0.5).abs() < 1e-10);

        input.end_frame();
        assert!(input.touch_active(1));
        assert!(!input.touch_just_started(1));

        input.process_touch_end(1);
        assert!(!input.touch_active(1));
        assert!(input.touch_just_ended(1));

        input.end_frame();
        assert!(!input.touch_just_ended(1));
    }

    #[test]
    fn test_touch_zone_binding() {
        let mut input = InputState::new();
        input.set_screen_size(1000.0, 1000.0);
        input
            .load_bindings(
                InputConfig::from_toml_str(
                    r#"
version = 1
[actions.go_left]
kind = "button"
[[actions.go_left.bindings]]
type = "touch_zone"
zone = "left_half"
"#,
                )
                .unwrap(),
            )
            .unwrap();

        // Touch in the left half (x=200 → 0.2 normalized)
        input.process_touch_start(1, 200.0, 500.0);
        assert!(input.is_action_pressed("go_left"));
        assert!(input.is_action_just_pressed("go_left"));

        input.end_frame();
        assert!(input.is_action_pressed("go_left"));
        assert!(!input.is_action_just_pressed("go_left"));

        // Touch in the right half should NOT trigger left action
        input.process_touch_end(1);
        input.end_frame();
        input.process_touch_start(2, 800.0, 500.0);
        assert!(!input.is_action_pressed("go_left"));
    }

    #[test]
    fn test_touch_tap_detection() {
        let mut input = InputState::new();
        input.set_screen_size(1000.0, 1000.0);

        // Quick tap: start + end with no movement
        input.process_touch_start(1, 500.0, 500.0);
        input.process_touch_end(1);

        assert_eq!(input.touch_taps().len(), 1);
        let tap = input.touch_taps()[0];
        assert!((tap.0 - 0.5).abs() < 1e-10);
        assert!((tap.1 - 0.5).abs() < 1e-10);

        input.end_frame();
        assert!(input.touch_taps().is_empty());
    }

    #[test]
    fn test_touch_tap_no_fire_on_drag() {
        let mut input = InputState::new();
        input.set_screen_size(1000.0, 1000.0);

        // Drag: start, move far, end
        input.process_touch_start(1, 100.0, 500.0);
        input.process_touch_move(1, 500.0, 500.0); // 400px movement
        input.process_touch_end(1);

        assert!(input.touch_taps().is_empty(), "drag should not fire tap");
    }

    #[test]
    fn test_mouse_touch_emulation() {
        let mut input = InputState::new();
        input.set_screen_size(1000.0, 1000.0);
        assert!(input.touch_emulation_active());

        // Move mouse to position first
        input.process_mouse_move(300.0, 500.0);

        // Mouse left click should create touch finger 0
        input.process_mouse_button_down(0);
        assert!(input.touch_active(0));
        assert!(input.touch_just_started(0));

        let pos = input.touch_position(0).unwrap();
        assert!((pos.0 - 0.3).abs() < 1e-10);

        // Mouse move updates touch
        input.process_mouse_move(600.0, 500.0);
        let pos2 = input.touch_position(0).unwrap();
        assert!((pos2.0 - 0.6).abs() < 1e-10);

        // Mouse release ends touch
        input.process_mouse_button_up(0);
        assert!(!input.touch_active(0));
        assert!(input.touch_just_ended(0));
    }

    #[test]
    fn test_multi_touch() {
        let mut input = InputState::new();
        input.set_screen_size(1000.0, 1000.0);

        input.process_touch_start(1, 100.0, 100.0);
        input.process_touch_start(2, 900.0, 900.0);

        assert_eq!(input.touch_count(), 2);
        assert!(input.touch_active(1));
        assert!(input.touch_active(2));

        let p1 = input.touch_position(1).unwrap();
        let p2 = input.touch_position(2).unwrap();
        assert!((p1.0 - 0.1).abs() < 1e-10);
        assert!((p2.0 - 0.9).abs() < 1e-10);

        input.process_touch_end(1);
        assert_eq!(input.touch_count(), 1);
        assert!(!input.touch_active(1));
        assert!(input.touch_active(2));
    }

    #[test]
    fn test_touch_zone_validation() {
        // Valid zone
        let config = InputConfig::from_toml_str(
            r#"
version = 1
[actions.test]
kind = "button"
[[actions.test.bindings]]
type = "touch_zone"
zone = "left_half"
"#,
        );
        assert!(config.is_ok());

        // Invalid zone
        let config = InputConfig::from_toml_str(
            r#"
version = 1
[actions.test]
kind = "button"
[[actions.test.bindings]]
type = "touch_zone"
zone = "invalid_zone"
"#,
        );
        assert!(config.is_err());
    }

    #[test]
    fn test_swipe_up() {
        let mut input = InputState::new();
        input.set_screen_size(1000.0, 1000.0);

        // Swipe upward: start low, move up (lower Y in screen coords = upward)
        input.process_touch_start(1, 500.0, 600.0);
        input.process_touch_move(1, 500.0, 500.0); // 100px up
        input.process_touch_end(1);

        assert_eq!(input.touch_swipes().len(), 1);
        assert_eq!(input.touch_swipes()[0].0, SwipeDirection::Up);
        assert!(input.touch_taps().is_empty(), "swipe should not fire tap");

        input.end_frame();
        assert!(input.touch_swipes().is_empty());
    }

    #[test]
    fn test_swipe_down() {
        let mut input = InputState::new();
        input.set_screen_size(1000.0, 1000.0);

        input.process_touch_start(1, 500.0, 400.0);
        input.process_touch_move(1, 500.0, 500.0); // 100px down
        input.process_touch_end(1);

        assert_eq!(input.touch_swipes().len(), 1);
        assert_eq!(input.touch_swipes()[0].0, SwipeDirection::Down);
    }

    #[test]
    fn test_swipe_horizontal() {
        let mut input = InputState::new();
        input.set_screen_size(1000.0, 1000.0);

        // Swipe right
        input.process_touch_start(1, 400.0, 500.0);
        input.process_touch_move(1, 500.0, 500.0); // 100px right
        input.process_touch_end(1);

        assert_eq!(input.touch_swipes().len(), 1);
        assert_eq!(input.touch_swipes()[0].0, SwipeDirection::Right);

        input.end_frame();

        // Swipe left
        input.process_touch_start(2, 600.0, 500.0);
        input.process_touch_move(2, 500.0, 500.0); // 100px left
        input.process_touch_end(2);

        assert_eq!(input.touch_swipes().len(), 1);
        assert_eq!(input.touch_swipes()[0].0, SwipeDirection::Left);
    }

    #[test]
    fn test_short_movement_is_tap_not_swipe() {
        let mut input = InputState::new();
        input.set_screen_size(1000.0, 1000.0);

        // 10px movement = tap (< 20px threshold)
        input.process_touch_start(1, 500.0, 500.0);
        input.process_touch_move(1, 510.0, 500.0);
        input.process_touch_end(1);

        assert_eq!(input.touch_taps().len(), 1);
        assert!(input.touch_swipes().is_empty());
    }

    #[test]
    fn test_medium_movement_fires_neither() {
        let mut input = InputState::new();
        input.set_screen_size(1000.0, 1000.0);

        // 30px movement = dead zone (>= 20px for tap, < 40px for swipe)
        input.process_touch_start(1, 500.0, 500.0);
        input.process_touch_move(1, 530.0, 500.0);
        input.process_touch_end(1);

        assert!(input.touch_taps().is_empty(), "dead zone should not fire tap");
        assert!(
            input.touch_swipes().is_empty(),
            "dead zone should not fire swipe"
        );
    }

    #[test]
    fn test_swipe_binding_triggers_action() {
        let mut input = InputState::new();
        input.set_screen_size(1000.0, 1000.0);
        input
            .load_bindings(
                InputConfig::from_toml_str(
                    r#"
version = 1
[actions.jump]
kind = "button"
[[actions.jump.bindings]]
type = "swipe"
direction = "up"
"#,
                )
                .unwrap(),
            )
            .unwrap();

        // No swipe yet
        assert!(!input.is_action_pressed("jump"));
        assert!(!input.is_action_just_pressed("jump"));

        // Perform a swipe up
        input.process_touch_start(1, 500.0, 600.0);
        input.process_touch_move(1, 500.0, 500.0);
        input.process_touch_end(1);

        assert!(input.is_action_pressed("jump"));
        assert!(input.is_action_just_pressed("jump"));

        input.end_frame();
        assert!(!input.is_action_pressed("jump"));
        assert!(!input.is_action_just_pressed("jump"));
    }
}
