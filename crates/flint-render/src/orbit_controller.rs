//! Shared orbit camera controller for mouse + keyboard input.
//!
//! Used by `gen-preview`, `preview`, and `serve`/viewer to provide a
//! consistent orbit-camera experience with left-drag orbit, right-drag pan,
//! scroll zoom, and WASD/QE keyboard controls.

use std::collections::HashSet;

use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::Camera;

/// Orbit camera controller handling mouse drag and keyboard held-key input.
pub struct OrbitCameraController {
    // Mouse state
    pub left_pressed: bool,
    pub right_pressed: bool,
    pub last_mouse_pos: Option<(f64, f64)>,

    // Keyboard held-key tracking (only WASD/QE)
    held_keys: HashSet<KeyCode>,

    // Sensitivity
    pub orbit_sensitivity: f32,
    pub zoom_scroll_factor: f32,
    pub key_orbit_speed: f32,
    pub key_zoom_speed: f32,

    // Auto-orbit
    pub auto_orbit: bool,
    pub auto_orbit_speed: f32,
}

impl Default for OrbitCameraController {
    fn default() -> Self {
        Self::new()
    }
}

impl OrbitCameraController {
    pub fn new() -> Self {
        Self {
            left_pressed: false,
            right_pressed: false,
            last_mouse_pos: None,
            held_keys: HashSet::new(),
            orbit_sensitivity: 0.005,
            zoom_scroll_factor: 0.1,
            key_orbit_speed: 2.0,
            key_zoom_speed: 2.0,
            auto_orbit: false,
            auto_orbit_speed: 0.5,
        }
    }

    /// Process a winit `WindowEvent`, updating mouse/key state and applying
    /// immediate camera transforms (drag orbit, drag pan, scroll zoom).
    ///
    /// Returns `true` if the event was consumed by the controller.
    /// `egui_wants_input` should be `true` when egui's pointer is over its UI.
    pub fn handle_event(
        &mut self,
        event: &WindowEvent,
        camera: &mut Camera,
        egui_wants_input: bool,
    ) -> bool {
        match event {
            WindowEvent::MouseInput { state, button, .. } => {
                if egui_wants_input {
                    return false;
                }
                match button {
                    MouseButton::Left => {
                        self.left_pressed = *state == ElementState::Pressed;
                        if self.left_pressed {
                            self.auto_orbit = false;
                        }
                    }
                    MouseButton::Right => {
                        self.right_pressed = *state == ElementState::Pressed;
                    }
                    _ => return false,
                }
                if *state == ElementState::Released {
                    self.last_mouse_pos = None;
                }
                true
            }

            WindowEvent::CursorMoved { position, .. } => {
                if egui_wants_input {
                    return false;
                }
                if let Some((lx, ly)) = self.last_mouse_pos {
                    let dx = (position.x - lx) as f32;
                    let dy = (position.y - ly) as f32;

                    if self.left_pressed {
                        // Orbit
                        camera.yaw -= dx * self.orbit_sensitivity;
                        camera.pitch += dy * self.orbit_sensitivity;
                        camera.pitch = camera.pitch.clamp(-1.4, 1.4);
                        camera.update_orbit();
                    } else if self.right_pressed {
                        // Pan using Camera::pan() for correct right/up vectors
                        let pan_factor = camera.distance * 0.002;
                        camera.pan(-dx * pan_factor, dy * pan_factor);
                    }
                }
                self.last_mouse_pos = Some((position.x, position.y));
                self.left_pressed || self.right_pressed
            }

            WindowEvent::MouseWheel { delta, .. } => {
                if egui_wants_input {
                    return false;
                }
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    MouseScrollDelta::PixelDelta(pos) => pos.y as f32 * 0.01,
                };
                camera.distance *= 1.0 - scroll * self.zoom_scroll_factor;
                camera.distance = camera.distance.max(0.1);
                camera.update_orbit();
                true
            }

            _ => false,
        }
    }

    /// Track WASD/QE held state from a `KeyEvent`.
    /// Call this from your `KeyboardInput` handler so the controller knows
    /// which orbit keys are held. Does not consume the event.
    pub fn handle_key_event(&mut self, event: &winit::event::KeyEvent) {
        if let PhysicalKey::Code(code) = event.physical_key {
            match code {
                KeyCode::KeyW
                | KeyCode::KeyA
                | KeyCode::KeyS
                | KeyCode::KeyD
                | KeyCode::KeyQ
                | KeyCode::KeyE => {
                    if event.state == ElementState::Pressed {
                        self.held_keys.insert(code);
                        self.auto_orbit = false;
                    } else {
                        self.held_keys.remove(&code);
                    }
                }
                _ => {}
            }
        }
    }

    /// Apply held-key orbit/zoom and auto-orbit each frame. Call once per
    /// frame with the frame delta in seconds.
    pub fn update(&self, camera: &mut Camera, dt: f32) {
        let mut yaw_delta = 0.0f32;
        let mut pitch_delta = 0.0f32;
        let mut zoom_factor = 1.0f32;

        // Auto-orbit: continuous yaw rotation
        if self.auto_orbit {
            yaw_delta += self.auto_orbit_speed * dt;
        }

        if self.held_keys.contains(&KeyCode::KeyA) {
            yaw_delta += self.key_orbit_speed * dt;
        }
        if self.held_keys.contains(&KeyCode::KeyD) {
            yaw_delta -= self.key_orbit_speed * dt;
        }
        if self.held_keys.contains(&KeyCode::KeyW) {
            pitch_delta += self.key_orbit_speed * dt;
        }
        if self.held_keys.contains(&KeyCode::KeyS) {
            pitch_delta -= self.key_orbit_speed * dt;
        }
        if self.held_keys.contains(&KeyCode::KeyQ) {
            zoom_factor *= 1.0 + self.key_zoom_speed * dt;
        }
        if self.held_keys.contains(&KeyCode::KeyE) {
            zoom_factor *= 1.0 - self.key_zoom_speed * dt;
        }

        if yaw_delta != 0.0 || pitch_delta != 0.0 || zoom_factor != 1.0 {
            camera.yaw += yaw_delta;
            camera.pitch = (camera.pitch + pitch_delta).clamp(-1.4, 1.4);
            camera.distance = (camera.distance * zoom_factor).max(0.1);
            camera.update_orbit();
        }
    }

    /// Multiply the auto-orbit speed by `factor`, clamped to 0.1–5.0 rad/s.
    pub fn adjust_auto_orbit_speed(&mut self, factor: f32) {
        self.auto_orbit_speed = (self.auto_orbit_speed * factor).clamp(0.1, 5.0);
    }

    /// Returns true if any orbit key is currently held.
    pub fn has_held_keys(&self) -> bool {
        !self.held_keys.is_empty()
    }

    /// Check if a specific key is currently held.
    pub fn is_key_held(&self, key: KeyCode) -> bool {
        self.held_keys.contains(&key)
    }
}
