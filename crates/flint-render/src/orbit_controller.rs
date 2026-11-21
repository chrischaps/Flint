//! The orbit camera every `flint edit` editor shares (#712, #1153): drag to
//! orbit, right-drag to pan, scroll to zoom, WASD/QE held to orbit and zoom,
//! the O turntable, and a view cube's eased snap onto an axis view.
//!
//! The editors' host (`flint_viewer::host`) feeds it each editor's window
//! events and steps it once a frame, so orbit speeds, pan, zoom and the
//! turntable feel the same in the scene viewer, the track editor and the
//! previewers. An editor that shares the mouse with its own tools tells it
//! so: [`orbit_button`](OrbitCameraController::orbit_button) moves orbiting
//! off the left button (the track editor picks points with it), and
//! [`set_tool_under_pointer`](OrbitCameraController::set_tool_under_pointer)
//! gives a press to a tool (the scene viewer's transform gizmo) instead.

use std::collections::HashSet;

use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::Camera;

/// Orbit camera controller handling mouse drag and keyboard held-key input.
pub struct OrbitCameraController {
    /// The orbit button is down on a press that orbits (not a tool's)
    pub orbit_pressed: bool,
    /// The right button is down: drags pan
    pub pan_pressed: bool,
    pub last_mouse_pos: Option<(f64, f64)>,
    /// The button whose drag orbits: left, or middle in an editor whose
    /// left button picks and drags its own handles
    pub orbit_button: MouseButton,
    /// A tool under the pointer takes the orbit button's next press
    tool_under_pointer: bool,

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

    /// The view (yaw, pitch) the camera is easing onto, from [`Self::snap_to`]
    snap: Option<(f32, f32)>,
    /// The last snap made the camera orthographic, so orbiting away from
    /// the axis view makes it perspective again
    snapped_ortho: bool,
}

impl Default for OrbitCameraController {
    fn default() -> Self {
        Self::new()
    }
}

impl OrbitCameraController {
    pub fn new() -> Self {
        Self {
            orbit_pressed: false,
            pan_pressed: false,
            last_mouse_pos: None,
            orbit_button: MouseButton::Left,
            tool_under_pointer: false,
            held_keys: HashSet::new(),
            orbit_sensitivity: 0.005,
            zoom_scroll_factor: 0.1,
            key_orbit_speed: 2.0,
            key_zoom_speed: 2.0,
            auto_orbit: false,
            auto_orbit_speed: 0.5,
            snap: None,
            snapped_ortho: false,
        }
    }

    /// Whether a tool (a gizmo's handle) is under the pointer. Said before
    /// each press of the orbit button reaches [`Self::handle_event`]: a
    /// press the tool takes doesn't orbit, however far it drags, until the
    /// button is released.
    pub fn set_tool_under_pointer(&mut self, under: bool) {
        self.tool_under_pointer = under;
    }

    /// Process a winit `WindowEvent`, updating mouse/key state and applying
    /// immediate camera transforms (drag orbit, drag pan, scroll zoom).
    ///
    /// Returns `true` if the event was consumed by the controller.
    /// `egui_wants_input` should be `true` when egui's pointer is over its UI;
    /// a press there is the panel's, but a release always ends a drag.
    pub fn handle_event(
        &mut self,
        event: &WindowEvent,
        camera: &mut Camera,
        egui_wants_input: bool,
    ) -> bool {
        match event {
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = *state == ElementState::Pressed;
                if egui_wants_input && pressed {
                    return false;
                }
                if *button == self.orbit_button {
                    self.orbit_pressed = pressed && !self.tool_under_pointer;
                } else if *button == MouseButton::Right {
                    self.pan_pressed = pressed;
                } else {
                    return false;
                }
                if !pressed {
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

                    if self.orbit_pressed {
                        if dx != 0.0 || dy != 0.0 {
                            self.orbit_by(
                                camera,
                                -dx * self.orbit_sensitivity,
                                dy * self.orbit_sensitivity,
                            );
                        }
                    } else if self.pan_pressed {
                        // Pan using Camera::pan() for correct right/up vectors
                        let pan_factor = camera.distance * 0.002;
                        camera.pan(-dx * pan_factor, dy * pan_factor);
                        self.snap = None;
                    }
                }
                self.last_mouse_pos = Some((position.x, position.y));
                self.orbit_pressed || self.pan_pressed
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
            self.hold_key(code, event.state == ElementState::Pressed);
        }
    }

    /// An orbit key (WASD/QE) went down or up; other keys are ignored.
    pub fn hold_key(&mut self, code: KeyCode, pressed: bool) {
        match code {
            KeyCode::KeyW
            | KeyCode::KeyA
            | KeyCode::KeyS
            | KeyCode::KeyD
            | KeyCode::KeyQ
            | KeyCode::KeyE => {
                if pressed {
                    self.held_keys.insert(code);
                    self.auto_orbit = false;
                } else {
                    self.held_keys.remove(&code);
                }
            }
            _ => {}
        }
    }

    /// Turn the camera by hand, radians: what a drag does, and what a view
    /// cube's drag asks. It stops the turntable and a snap under way, and
    /// leaves a snapped axis view for perspective.
    pub fn orbit_by(&mut self, camera: &mut Camera, dyaw: f32, dpitch: f32) {
        self.auto_orbit = false;
        self.snap = None;
        self.leave_snapped_ortho(camera);
        camera.yaw += dyaw;
        camera.pitch = (camera.pitch + dpitch).clamp(-Camera::PITCH_LIMIT, Camera::PITCH_LIMIT);
        camera.update_orbit();
    }

    /// Ease the camera onto the axis view (yaw, pitch), radians, over the
    /// next frames, orthographic at once (a view cube's axis click). An
    /// orbit by hand afterwards ends the snap and goes back to perspective.
    pub fn snap_to(&mut self, camera: &mut Camera, yaw: f32, pitch: f32) {
        self.auto_orbit = false;
        self.snap = Some((yaw, pitch));
        camera.orthographic = true;
        self.snapped_ortho = true;
    }

    /// Whether a snap is still easing.
    pub fn is_snapping(&self) -> bool {
        self.snap.is_some()
    }

    /// Drop a snap under way, where the camera is now (the camera was
    /// reset or framed anew).
    pub fn cancel_snap(&mut self) {
        self.snap = None;
    }

    /// Back to perspective (a view cube's background click).
    pub fn perspective(&mut self, camera: &mut Camera) {
        camera.orthographic = false;
        self.snapped_ortho = false;
    }

    fn leave_snapped_ortho(&mut self, camera: &mut Camera) {
        if self.snapped_ortho {
            self.perspective(camera);
        }
    }

    /// Apply held-key orbit/zoom, auto-orbit and a snap's easing each
    /// frame. Call once per frame with the frame delta in seconds.
    pub fn update(&mut self, camera: &mut Camera, dt: f32) {
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

        if yaw_delta != 0.0 || pitch_delta != 0.0 {
            self.snap = None;
            self.leave_snapped_ortho(camera);
        }
        if yaw_delta != 0.0 || pitch_delta != 0.0 || zoom_factor != 1.0 {
            camera.yaw += yaw_delta;
            camera.pitch =
                (camera.pitch + pitch_delta).clamp(-Camera::PITCH_LIMIT, Camera::PITCH_LIMIT);
            camera.distance = (camera.distance * zoom_factor).max(0.1);
            camera.update_orbit();
        }

        if let Some((yaw, pitch)) = self.snap {
            let t = 1.0 - (-12.0 * dt).exp();
            camera.yaw += shortest_angle(camera.yaw, yaw) * t;
            camera.pitch += (pitch - camera.pitch) * t;
            let remaining = shortest_angle(camera.yaw, yaw).abs() + (camera.pitch - pitch).abs();
            if remaining < 0.002 {
                camera.yaw = yaw;
                camera.pitch = pitch;
                self.snap = None;
            }
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

/// `to - from` in radians, the short way round.
fn shortest_angle(from: f32, to: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (to - from + PI).rem_euclid(TAU) - PI
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::dpi::PhysicalPosition;
    use winit::event::DeviceId;

    fn press(button: MouseButton, pressed: bool) -> WindowEvent {
        WindowEvent::MouseInput {
            device_id: DeviceId::dummy(),
            state: if pressed {
                ElementState::Pressed
            } else {
                ElementState::Released
            },
            button,
        }
    }

    fn moved(x: f64, y: f64) -> WindowEvent {
        WindowEvent::CursorMoved {
            device_id: DeviceId::dummy(),
            position: PhysicalPosition::new(x, y),
        }
    }

    /// Press `button` at (0, 0), drag to (dx, dy), release.
    fn drag(
        orbit: &mut OrbitCameraController,
        camera: &mut Camera,
        button: MouseButton,
        dx: f64,
        dy: f64,
    ) {
        orbit.handle_event(&moved(0.0, 0.0), camera, false);
        orbit.handle_event(&press(button, true), camera, false);
        orbit.handle_event(&moved(0.0, 0.0), camera, false);
        orbit.handle_event(&moved(dx, dy), camera, false);
        orbit.handle_event(&press(button, false), camera, false);
    }

    #[test]
    fn a_left_drag_orbits_and_ends_the_turntable() {
        let (mut orbit, mut camera) = (OrbitCameraController::new(), Camera::new());
        orbit.auto_orbit = true;
        let yaw = camera.yaw;
        drag(&mut orbit, &mut camera, MouseButton::Left, 40.0, 0.0);
        assert!((camera.yaw - (yaw - 40.0 * orbit.orbit_sensitivity)).abs() < 1e-6);
        assert!(!orbit.auto_orbit);
        assert!(!orbit.orbit_pressed, "released");
    }

    #[test]
    fn a_press_a_tool_takes_never_orbits() {
        let (mut orbit, mut camera) = (OrbitCameraController::new(), Camera::new());
        orbit.auto_orbit = true;
        let (yaw, pitch) = (camera.yaw, camera.pitch);
        orbit.set_tool_under_pointer(true);
        drag(&mut orbit, &mut camera, MouseButton::Left, 40.0, 25.0);
        assert_eq!((camera.yaw, camera.pitch), (yaw, pitch));
        assert!(
            orbit.auto_orbit,
            "a gizmo drag leaves the turntable turning"
        );
        // The next press, off the tool, orbits again
        orbit.set_tool_under_pointer(false);
        drag(&mut orbit, &mut camera, MouseButton::Left, 40.0, 0.0);
        assert_ne!(camera.yaw, yaw);
    }

    #[test]
    fn a_middle_orbit_button_leaves_the_left_to_the_editor() {
        let (mut orbit, mut camera) = (OrbitCameraController::new(), Camera::new());
        orbit.orbit_button = MouseButton::Middle;
        let yaw = camera.yaw;
        drag(&mut orbit, &mut camera, MouseButton::Left, 40.0, 0.0);
        assert_eq!(camera.yaw, yaw);
        drag(&mut orbit, &mut camera, MouseButton::Middle, 40.0, 0.0);
        assert_ne!(camera.yaw, yaw);
    }

    #[test]
    fn a_release_over_a_panel_still_ends_the_drag() {
        let (mut orbit, mut camera) = (OrbitCameraController::new(), Camera::new());
        orbit.handle_event(&press(MouseButton::Left, true), &mut camera, false);
        orbit.handle_event(&press(MouseButton::Left, false), &mut camera, true);
        assert!(!orbit.orbit_pressed);
    }

    #[test]
    fn a_snap_eases_onto_the_view_orthographic_until_an_orbit() {
        let (mut orbit, mut camera) = (OrbitCameraController::new(), Camera::new());
        orbit.auto_orbit = true;
        orbit.snap_to(&mut camera, 0.0, 1.55);
        assert!(camera.orthographic, "the projection switches at once");
        assert!(!orbit.auto_orbit);
        for _ in 0..120 {
            orbit.update(&mut camera, 1.0 / 60.0);
        }
        assert!(!orbit.is_snapping());
        assert_eq!((camera.yaw, camera.pitch), (0.0, 1.55));
        assert!(camera.orthographic, "settled, still the axis view");
        // A pitch past the old 1.4 clamp survives a zoom key
        orbit.hold_key(KeyCode::KeyQ, true);
        orbit.update(&mut camera, 1.0 / 60.0);
        orbit.hold_key(KeyCode::KeyQ, false);
        assert_eq!(camera.pitch, 1.55);
        assert!(camera.orthographic, "zooming isn't orbiting");
        // An orbit by hand is perspective again
        orbit.orbit_by(&mut camera, 0.1, 0.0);
        assert!(!camera.orthographic);
    }

    #[test]
    fn a_snap_takes_the_short_way_round() {
        let (mut orbit, mut camera) = (OrbitCameraController::new(), Camera::new());
        camera.yaw = 3.0;
        orbit.snap_to(&mut camera, -3.0, 0.0);
        orbit.update(&mut camera, 1.0 / 60.0);
        assert!(
            camera.yaw > 3.0,
            "through pi, not back through 0: {}",
            camera.yaw
        );
    }

    #[test]
    fn keys_end_a_snap_and_its_ortho() {
        let (mut orbit, mut camera) = (OrbitCameraController::new(), Camera::new());
        orbit.snap_to(&mut camera, 0.0, 0.0);
        orbit.hold_key(KeyCode::KeyA, true);
        orbit.update(&mut camera, 1.0 / 60.0);
        assert!(!orbit.is_snapping());
        assert!(!camera.orthographic);
    }

    #[test]
    fn an_ortho_camera_no_snap_made_stays_ortho() {
        let (mut orbit, mut camera) = (OrbitCameraController::new(), Camera::new());
        camera.orthographic = true;
        orbit.orbit_by(&mut camera, 0.1, 0.1);
        assert!(camera.orthographic, "an editor's own ortho view is its own");
    }
}
