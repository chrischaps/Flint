//! Input state management

use flint_core::{FlintError, Result};
use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use winit::keyboard::KeyCode;

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
                    bindings: vec![Binding::Key { code: key.into() }],
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
    gamepad_axes: HashMap<(u32, String), f32>,
    last_frame_gamepad_axes: HashMap<(u32, String), f32>,

    pub mouse_position: (f64, f64),
    mouse_delta: (f64, f64),
    raw_mouse_delta: (f64, f64),
    mouse_wheel_delta: (f32, f32),

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
            gamepad_axes: HashMap::new(),
            last_frame_gamepad_axes: HashMap::new(),
            mouse_position: (0.0, 0.0),
            mouse_delta: (0.0, 0.0),
            raw_mouse_delta: (0.0, 0.0),
            mouse_wheel_delta: (0.0, 0.0),
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

    pub fn rebind_action(&mut self, action: &str, binding: Binding, mode: RebindMode) -> Result<()> {
        validate_binding(&binding, action)?;

        match mode {
            RebindMode::Replace => {
                let entry = self.config.actions.entry(action.into()).or_insert_with(|| ActionConfig {
                    kind: infer_action_kind_for_new_action(&binding),
                    bindings: Vec::new(),
                });
                entry.bindings = vec![binding];
            }
            RebindMode::Add => {
                let entry = self.config.actions.entry(action.into()).or_insert_with(|| ActionConfig {
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
                let entry = self.config.actions.entry(action.into()).or_insert_with(|| ActionConfig {
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
    }

    pub fn process_mouse_button_up(&mut self, button: u32) {
        self.mouse_buttons_down.remove(&button);
        self.mouse_buttons_just_released.insert(button);
    }

    pub fn process_mouse_move(&mut self, x: f64, y: f64) {
        self.mouse_delta.0 += x - self.mouse_position.0;
        self.mouse_delta.1 += y - self.mouse_position.1;
        self.mouse_position = (x, y);
    }

    pub fn process_mouse_raw_delta(&mut self, dx: f64, dy: f64) {
        self.raw_mouse_delta.0 += dx;
        self.raw_mouse_delta.1 += dy;
    }

    pub fn process_mouse_wheel(&mut self, dx: f32, dy: f32) {
        self.mouse_wheel_delta.0 += dx;
        self.mouse_wheel_delta.1 += dy;
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
        self.gamepad_buttons_just_released.insert(key);
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

        let mut value = match cfg.kind {
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
            Binding::Key { code } => parse_key_code(code)
                .map(|key| self.keys_down.contains(&key))
                .unwrap_or(false) as i32 as f32,
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
            Binding::GamepadButton { button, gamepad } => self
                .gamepad_buttons_down
                .iter()
                .any(|(id, pressed_button)| {
                    selector_matches(*gamepad, *id) && pressed_button == button
                }) as i32 as f32,
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
                    if filtered.abs() >= *threshold { 1.0 } else { 0.0 }
                } else {
                    filtered * *scale
                }
            }
        }
    }

    fn binding_just_pressed(&self, binding: &Binding) -> bool {
        match binding {
            Binding::Key { code } => parse_key_code(code)
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
            _ => false,
        }
    }

    fn binding_just_released(&self, binding: &Binding) -> bool {
        match binding {
            Binding::Key { code } => parse_key_code(code)
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
        Binding::Key { code } => {
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
    }
    Ok(())
}

fn binding_label(binding: &Binding) -> String {
    match binding {
        Binding::Key { code } => key_code_label(code),
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
        input.load_bindings(
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
        input.load_bindings(
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
}
