//! Input state management

use std::collections::{HashMap, HashSet};
use winit::keyboard::KeyCode;

/// Tracks keyboard and mouse input state per frame
pub struct InputState {
    /// Keys currently held down
    keys_down: HashSet<KeyCode>,
    /// Keys pressed this frame
    keys_just_pressed: HashSet<KeyCode>,
    /// Keys released this frame
    keys_just_released: HashSet<KeyCode>,

    /// Mouse button state (button index -> pressed)
    mouse_buttons_down: HashSet<u32>,
    /// Mouse buttons pressed this frame
    mouse_buttons_just_pressed: HashSet<u32>,

    /// Current mouse position in window pixels
    pub mouse_position: (f64, f64),
    /// Mouse movement delta this frame
    mouse_delta: (f64, f64),
    /// Raw accumulated mouse delta (for cursor-locked mode)
    raw_mouse_delta: (f64, f64),

    /// Action map: action name -> list of key bindings
    action_map: HashMap<String, Vec<KeyCode>>,

    /// Mouse button action map: action name -> list of mouse button indices
    mouse_button_map: HashMap<String, Vec<u32>>,
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
            mouse_position: (0.0, 0.0),
            mouse_delta: (0.0, 0.0),
            raw_mouse_delta: (0.0, 0.0),
            action_map: Self::default_action_map(),
            mouse_button_map: Self::default_mouse_button_map(),
        }
    }

    fn default_action_map() -> HashMap<String, Vec<KeyCode>> {
        let mut map = HashMap::new();
        // FPS defaults
        map.insert("move_forward".into(), vec![KeyCode::KeyW]);
        map.insert("move_backward".into(), vec![KeyCode::KeyS]);
        map.insert("move_left".into(), vec![KeyCode::KeyA]);
        map.insert("move_right".into(), vec![KeyCode::KeyD]);
        map.insert("jump".into(), vec![KeyCode::Space]);
        map.insert("interact".into(), vec![KeyCode::KeyE]);
        map.insert("sprint".into(), vec![KeyCode::ShiftLeft]);
        map.insert("weapon_1".into(), vec![KeyCode::Digit1]);
        map.insert("weapon_2".into(), vec![KeyCode::Digit2]);
        map.insert("reload".into(), vec![KeyCode::KeyR]);
        // Kart defaults (overlap with FPS keys is intentional — game scripts use whichever they need)
        map.insert("accelerate".into(), vec![KeyCode::KeyW, KeyCode::ArrowUp]);
        map.insert("brake".into(), vec![KeyCode::KeyS, KeyCode::ArrowDown]);
        map.insert("steer_left".into(), vec![KeyCode::KeyA, KeyCode::ArrowLeft]);
        map.insert("steer_right".into(), vec![KeyCode::KeyD, KeyCode::ArrowRight]);
        map.insert("restart".into(), vec![KeyCode::KeyR]);
        map
    }

    fn default_mouse_button_map() -> HashMap<String, Vec<u32>> {
        let mut map = HashMap::new();
        map.insert("fire".into(), vec![0]); // Left mouse button
        map
    }

    /// Bind an action to one or more keys
    pub fn bind_action(&mut self, action: impl Into<String>, keys: Vec<KeyCode>) {
        self.action_map.insert(action.into(), keys);
    }

    /// Process a key press event
    pub fn process_key_down(&mut self, key: KeyCode) {
        if !self.keys_down.contains(&key) {
            self.keys_just_pressed.insert(key);
        }
        self.keys_down.insert(key);
    }

    /// Process a key release event
    pub fn process_key_up(&mut self, key: KeyCode) {
        self.keys_down.remove(&key);
        self.keys_just_released.insert(key);
    }

    /// Process mouse button press
    pub fn process_mouse_button_down(&mut self, button: u32) {
        if !self.mouse_buttons_down.contains(&button) {
            self.mouse_buttons_just_pressed.insert(button);
        }
        self.mouse_buttons_down.insert(button);
    }

    /// Process mouse button release
    pub fn process_mouse_button_up(&mut self, button: u32) {
        self.mouse_buttons_down.remove(&button);
    }

    /// Process mouse movement (cursor position mode)
    pub fn process_mouse_move(&mut self, x: f64, y: f64) {
        self.mouse_delta.0 += x - self.mouse_position.0;
        self.mouse_delta.1 += y - self.mouse_position.1;
        self.mouse_position = (x, y);
    }

    /// Process raw mouse delta (device motion, for locked cursor)
    pub fn process_mouse_raw_delta(&mut self, dx: f64, dy: f64) {
        self.raw_mouse_delta.0 += dx;
        self.raw_mouse_delta.1 += dy;
    }

    /// Call at end of frame to clear per-frame state
    pub fn end_frame(&mut self) {
        self.keys_just_pressed.clear();
        self.keys_just_released.clear();
        self.mouse_buttons_just_pressed.clear();
        self.mouse_delta = (0.0, 0.0);
        self.raw_mouse_delta = (0.0, 0.0);
    }

    // --- Query methods ---

    /// Is a key currently held down?
    pub fn is_key_down(&self, key: KeyCode) -> bool {
        self.keys_down.contains(&key)
    }

    /// Was a key pressed this frame?
    pub fn is_key_just_pressed(&self, key: KeyCode) -> bool {
        self.keys_just_pressed.contains(&key)
    }

    /// Is an action currently held? (any bound key or mouse button is down)
    pub fn is_action_pressed(&self, action: &str) -> bool {
        let key_match = self.action_map
            .get(action)
            .map(|keys| keys.iter().any(|k| self.keys_down.contains(k)))
            .unwrap_or(false);
        let mouse_match = self.mouse_button_map
            .get(action)
            .map(|btns| btns.iter().any(|b| self.mouse_buttons_down.contains(b)))
            .unwrap_or(false);
        key_match || mouse_match
    }

    /// Was an action just pressed this frame?
    pub fn is_action_just_pressed(&self, action: &str) -> bool {
        let key_match = self.action_map
            .get(action)
            .map(|keys| keys.iter().any(|k| self.keys_just_pressed.contains(k)))
            .unwrap_or(false);
        let mouse_match = self.mouse_button_map
            .get(action)
            .map(|btns| btns.iter().any(|b| self.mouse_buttons_just_pressed.contains(b)))
            .unwrap_or(false);
        key_match || mouse_match
    }

    /// Get all actions that were just pressed this frame
    pub fn actions_just_pressed(&self) -> Vec<String> {
        let mut result: Vec<String> = self.action_map
            .iter()
            .filter(|(_, keys)| keys.iter().any(|k| self.keys_just_pressed.contains(k)))
            .map(|(action, _)| action.clone())
            .collect();
        // Also check mouse button actions
        for (action, btns) in &self.mouse_button_map {
            if btns.iter().any(|b| self.mouse_buttons_just_pressed.contains(b)) {
                if !result.contains(action) {
                    result.push(action.clone());
                }
            }
        }
        result
    }

    /// Get all registered action names (keyboard + mouse button maps)
    pub fn action_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.action_map.keys().cloned().collect();
        for name in self.mouse_button_map.keys() {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names
    }

    /// Get the mouse movement delta this frame
    pub fn mouse_delta(&self) -> (f64, f64) {
        self.mouse_delta
    }

    /// Get the raw mouse delta (accumulated device motion)
    pub fn raw_mouse_delta(&self) -> (f64, f64) {
        self.raw_mouse_delta
    }

    /// Is a mouse button currently held?
    pub fn is_mouse_button_down(&self, button: u32) -> bool {
        self.mouse_buttons_down.contains(&button)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_transitions() {
        let mut input = InputState::new();

        // Press W
        input.process_key_down(KeyCode::KeyW);
        assert!(input.is_key_down(KeyCode::KeyW));
        assert!(input.is_key_just_pressed(KeyCode::KeyW));

        // End frame clears just_pressed
        input.end_frame();
        assert!(input.is_key_down(KeyCode::KeyW));
        assert!(!input.is_key_just_pressed(KeyCode::KeyW));

        // Release W
        input.process_key_up(KeyCode::KeyW);
        assert!(!input.is_key_down(KeyCode::KeyW));
    }

    #[test]
    fn test_action_map() {
        let mut input = InputState::new();

        // W is bound to "move_forward" by default
        assert!(!input.is_action_pressed("move_forward"));

        input.process_key_down(KeyCode::KeyW);
        assert!(input.is_action_pressed("move_forward"));
        assert!(input.is_action_just_pressed("move_forward"));

        input.end_frame();
        assert!(input.is_action_pressed("move_forward"));
        assert!(!input.is_action_just_pressed("move_forward"));
    }

    #[test]
    fn test_custom_binding() {
        let mut input = InputState::new();
        input.bind_action("fire", vec![KeyCode::KeyF, KeyCode::ControlLeft]);

        input.process_key_down(KeyCode::KeyF);
        assert!(input.is_action_pressed("fire"));

        input.process_key_up(KeyCode::KeyF);
        input.process_key_down(KeyCode::ControlLeft);
        assert!(input.is_action_pressed("fire"));
    }

    #[test]
    fn test_mouse_delta() {
        let mut input = InputState::new();

        input.process_mouse_move(100.0, 200.0);
        input.process_mouse_move(110.0, 205.0);

        let delta = input.mouse_delta();
        assert!((delta.0 - 110.0).abs() < 1e-10); // from (0,0) to 110
        assert!((delta.1 - 205.0).abs() < 1e-10);

        input.end_frame();
        let delta = input.mouse_delta();
        assert!((delta.0).abs() < 1e-10);
        assert!((delta.1).abs() < 1e-10);
    }

    #[test]
    fn test_mouse_buttons() {
        let mut input = InputState::new();

        input.process_mouse_button_down(0);
        assert!(input.is_mouse_button_down(0));

        input.process_mouse_button_up(0);
        assert!(!input.is_mouse_button_down(0));
    }
}
