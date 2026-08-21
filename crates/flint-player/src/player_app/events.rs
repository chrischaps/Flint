//! Input and window event handling for `PlayerApp` — code-motion sibling
//! of `mod.rs` (player_app decomposition; see the decomposition ADR).
//! Owns the winit `ApplicationHandler` impl, gamepad polling, rebind
//! capture/persistence, cursor capture, and the Android gamepad tracking
//! that exists only to serve these handlers.

use super::PlayerApp;
#[cfg(feature = "debug-hud")]
use super::music_guide_panel;
#[cfg(feature = "debug-hud")]
use super::timeline_panel;
use super::input_config::{
    fallback_user_override_path, gamepad_id_to_u32, write_user_override_file, PendingRebind,
};
use anyhow::Result;
use flint_runtime::{Binding, RebindMode};
use gilrs::EventType;
use std::path::{Path, PathBuf};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
#[cfg(target_os = "android")]
use winit::keyboard::NativeKeyCode;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, WindowId};
#[cfg(target_os = "android")]
use std::collections::HashMap;

/// Tracks Android device IDs that produce gamepad button keycodes,
/// assigning each a stable u32 slot for InputState's gamepad API.
#[cfg(target_os = "android")]
pub(super) struct AndroidGamepadTracker {
    known_device_ids: std::collections::HashSet<winit::event::DeviceId>,
    device_slots: HashMap<winit::event::DeviceId, u32>,
    next_slot: u32,
}

#[cfg(target_os = "android")]
impl AndroidGamepadTracker {
    pub(super) fn new() -> Self {
        Self {
            known_device_ids: std::collections::HashSet::new(),
            device_slots: HashMap::new(),
            next_slot: 0,
        }
    }

    /// Register a device as a gamepad and return its slot index.
    fn register_device(&mut self, device_id: winit::event::DeviceId) -> u32 {
        self.known_device_ids.insert(device_id);
        *self.device_slots.entry(device_id).or_insert_with(|| {
            let slot = self.next_slot;
            self.next_slot += 1;
            slot
        })
    }

    fn is_gamepad(&self, device_id: winit::event::DeviceId) -> bool {
        self.known_device_ids.contains(&device_id)
    }

    fn slot_for(&self, device_id: winit::event::DeviceId) -> Option<u32> {
        self.device_slots.get(&device_id).copied()
    }
}

impl PlayerApp {
    /// Map Android AKEYCODE_BUTTON_* values to gilrs-format button names.
    #[cfg(target_os = "android")]
    fn android_keycode_to_gamepad_button(keycode: u32) -> Option<&'static str> {
        match keycode {
            96 => Some("South"),          // AKEYCODE_BUTTON_A
            97 => Some("East"),           // AKEYCODE_BUTTON_B
            99 => Some("West"),           // AKEYCODE_BUTTON_X
            100 => Some("North"),         // AKEYCODE_BUTTON_Y
            102 => Some("LeftTrigger"),   // AKEYCODE_BUTTON_L1
            103 => Some("RightTrigger"),  // AKEYCODE_BUTTON_R1
            104 => Some("LeftTrigger2"),  // AKEYCODE_BUTTON_L2
            105 => Some("RightTrigger2"), // AKEYCODE_BUTTON_R2
            106 => Some("LeftThumb"),     // AKEYCODE_BUTTON_THUMBL
            107 => Some("RightThumb"),    // AKEYCODE_BUTTON_THUMBR
            108 => Some("Start"),         // AKEYCODE_BUTTON_START
            109 => Some("Select"),        // AKEYCODE_BUTTON_SELECT
            110 => Some("Mode"),          // AKEYCODE_BUTTON_MODE
            _ => None,
        }
    }

    /// Start capture mode for "press next control to bind" remapping.
    pub fn begin_rebind_capture(&mut self, action: impl Into<String>, mode: RebindMode) {
        self.pending_rebind = Some(PendingRebind {
            action: action.into(),
            mode,
        });
    }

    pub(super) fn poll_gamepad_events(&mut self) {
        let mut events = Vec::new();
        if let Some(gilrs) = &mut self.gilrs {
            while let Some(event) = gilrs.next_event() {
                events.push(event);
            }
        }

        for event in events {
            let gamepad = gamepad_id_to_u32(event.id);
            match event.event {
                EventType::ButtonPressed(button, _) => {
                    let name = format!("{button:?}");
                    if self.try_capture_rebind(Binding::GamepadButton {
                        button: name.clone(),
                        gamepad: flint_runtime::GamepadSelector::Any,
                    }) {
                        continue;
                    }
                    self.input.process_gamepad_button_down(gamepad, name);
                }
                EventType::ButtonReleased(button, _) => {
                    let name = format!("{button:?}");
                    self.input.process_gamepad_button_up(gamepad, name);
                }
                EventType::ButtonChanged(button, value, _) => {
                    let name = format!("{button:?}");
                    self.input
                        .process_gamepad_button_changed(gamepad, name, value);
                }
                EventType::AxisChanged(axis, value, _) => {
                    let name = format!("{axis:?}");
                    if self.pending_rebind.is_some() && value.abs() >= 0.45 {
                        let direction = if value < 0.0 {
                            Some(flint_runtime::AxisDirection::Negative)
                        } else {
                            Some(flint_runtime::AxisDirection::Positive)
                        };
                        if self.try_capture_rebind(Binding::GamepadAxis {
                            axis: name.clone(),
                            gamepad: flint_runtime::GamepadSelector::Any,
                            deadzone: 0.15,
                            scale: 1.0,
                            invert: false,
                            threshold: Some(0.35),
                            direction,
                        }) {
                            continue;
                        }
                    }
                    self.input.process_gamepad_axis(gamepad, name, value);
                }
                EventType::Disconnected => {
                    self.input.clear_gamepad(gamepad);
                }
                _ => {}
            }
        }

        // Android: poll trigger/right-stick axes from JNI bridge
        #[cfg(target_os = "android")]
        if let Some(reader) = self.android_axis_reader {
            let axes = reader();
            let [lt, rt, rs_x, rs_y] = axes;
            // Use slot 0 — the JNI bridge doesn't distinguish multiple gamepads
            let slot = 0u32;
            // Feed triggers as LeftZ/RightZ to match existing input.toml bindings
            // (accelerate = gamepad_axis "RightZ", brake = gamepad_axis "LeftZ")
            self.input.process_gamepad_axis(slot, "LeftZ", lt);
            self.input.process_gamepad_axis(slot, "RightZ", rt);
            // Right stick axes for future use
            self.input.process_gamepad_axis(slot, "RightStickX", rs_x);
            self.input.process_gamepad_axis(slot, "RightStickY", rs_y);
        }
    }

    fn try_capture_rebind(&mut self, binding: Binding) -> bool {
        let Some(pending) = self.pending_rebind.take() else {
            return false;
        };

        if let Err(e) = self
            .input
            .rebind_action(&pending.action, binding, pending.mode)
        {
            tracing::warn!("Failed to rebind action '{}': {:?}", pending.action, e);
            return true;
        }

        if let Some(action_cfg) = self.input.action_config(&pending.action) {
            self.user_override_config
                .actions
                .insert(pending.action.clone(), action_cfg);
        }
        if self.user_override_config.game_id.trim().is_empty() {
            self.user_override_config.game_id = self.input.config().game_id.clone();
        }

        if let Err(e) = self.persist_user_overrides() {
            tracing::warn!("Failed to save input overrides: {e:#}");
        }

        true
    }

    fn persist_user_overrides(&mut self) -> Result<()> {
        let Some(paths) = &mut self.input_config_paths else {
            return Ok(());
        };

        let mut target = paths.user_override.clone().unwrap_or_else(|| {
            fallback_user_override_path(Path::new(&self.scene_path), &self.input.config().game_id)
                .unwrap_or_else(|| PathBuf::from(".flint/input.user.toml"))
        });

        if let Err(err) = write_user_override_file(&target, &self.user_override_config) {
            let Some(fallback) = fallback_user_override_path(
                Path::new(&self.scene_path),
                &self.input.config().game_id,
            ) else {
                return Err(err);
            };
            if fallback != target {
                write_user_override_file(&fallback, &self.user_override_config)?;
                target = fallback;
            } else {
                return Err(err);
            }
        }

        paths.user_override = Some(target);
        Ok(())
    }

    pub(super) fn capture_cursor(&mut self) {
        if let Some(window) = &self.window {
            #[cfg(not(target_os = "android"))]
            {
                // Try confined first, then locked
                let _ = window
                    .set_cursor_grab(CursorGrabMode::Confined)
                    .or_else(|_| window.set_cursor_grab(CursorGrabMode::Locked));
                window.set_cursor_visible(false);
            }
            self.cursor_captured = true;
        }
    }

    pub(super) fn release_cursor(&mut self) {
        if let Some(window) = &self.window {
            #[cfg(not(target_os = "android"))]
            {
                let _ = window.set_cursor_grab(CursorGrabMode::None);
                window.set_cursor_visible(true);
            }
            self.cursor_captured = false;
        }
    }
}

impl ApplicationHandler for PlayerApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.render_context.is_none() {
            // First launch — full initialization
            if let Err(e) = self.initialize(event_loop) {
                tracing::error!("Failed to initialize player: {e:#}");
                event_loop.exit();
            }
        } else {
            // Subsequent resume (Android returning from background).
            // Recreate window and surface — GPU context is still valid.
            #[cfg(target_os = "android")]
            {
                let window_attrs = Window::default_attributes().with_title("Flint Player");
                match event_loop.create_window(window_attrs) {
                    Ok(window) => {
                        let window = Arc::new(window);
                        self.window = Some(window.clone());
                        if let Some(ctx) = &mut self.render_context {
                            if let Err(e) = ctx.recreate_surface(window.clone()) {
                                tracing::error!("Failed to recreate surface: {e}");
                                return;
                            }
                            self.camera.aspect = ctx.aspect_ratio();
                            let size = ctx.size;
                            if let Some(renderer) = &mut self.scene_renderer {
                                renderer.resize_postprocess(&ctx.device, size.width, size.height);
                            }
                            self.input
                                .set_screen_size(size.width as f64, size.height as f64);
                        }
                        self.cursor_captured = true;
                    }
                    Err(e) => {
                        tracing::error!("Failed to recreate window on resume: {e}");
                    }
                }
            }
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // On Android, the native surface is destroyed when the app is suspended.
        // Drop the window reference — a new one will be created in resumed().
        #[cfg(target_os = "android")]
        {
            self.window = None;
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // Forward window events to egui when a debug panel is open
        // so interactive widgets (sliders, buttons) receive mouse input.
        #[cfg(feature = "debug-hud")]
        let any_panel_open = self.debug_panels.iter().any(|p| p.is_open());
        #[cfg(not(feature = "debug-hud"))]
        let any_panel_open = false;
        if any_panel_open {
            if let (Some(egui_winit), Some(window)) = (&mut self.egui_winit, &self.window) {
                let response = egui_winit.on_window_event(window, &event);
                if response.consumed {
                    return;
                }
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            WindowEvent::Resized(new_size) => {
                self.handle_resize(new_size);
            }

            WindowEvent::KeyboardInput {
                device_id, event, ..
            } => {
                self.handle_keyboard_input(event_loop, device_id, event);
            }

            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_input(state, button);
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.input.process_mouse_move(position.x, position.y);
            }

            WindowEvent::Touch(touch) => {
                self.handle_touch(touch);
            }

            WindowEvent::RedrawRequested => {
                self.tick();
                self.render();
                if self.music_exit_requested {
                    event_loop.exit();
                }
            }

            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        if !self.cursor_captured {
            return;
        }

        if let DeviceEvent::MouseMotion { delta } = event {
            self.input.process_mouse_raw_delta(delta.0, delta.1);
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}


// window_event's per-arm handlers (Stage 2 of the player_app decomposition):
// each is the former match-arm body, extracted verbatim. A `return` in a
// handler ends that event's handling exactly as it did in the arm — nothing
// follows the match in window_event.
impl PlayerApp {
    fn handle_resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
            if let Some(context) = &mut self.render_context {
                context.resize(new_size);
                self.camera.aspect = context.aspect_ratio();
                // Resize post-processing HDR buffer and bloom chain
                if let Some(renderer) = &mut self.scene_renderer {
                    renderer.resize_postprocess(
                        &context.device,
                        new_size.width,
                        new_size.height,
                    );
                }
            }
            self.input
                .set_screen_size(new_size.width as f64, new_size.height as f64);
    }

    fn handle_keyboard_input(
        &mut self,
        event_loop: &ActiveEventLoop,
        device_id: winit::event::DeviceId,
        event: winit::event::KeyEvent,
    ) {
            // Suppress unused variable warning on non-Android platforms
            let _ = &device_id;

            if let PhysicalKey::Code(key_code) = event.physical_key {
                match event.state {
                    ElementState::Pressed => {
                        // Handle escape to toggle cursor capture
                        if key_code == KeyCode::Escape {
                            if self.cursor_captured {
                                self.release_cursor();
                            } else {
                                #[cfg(not(target_os = "android"))]
                                event_loop.exit();
                            }
                            return;
                        }

                        // Debug keys
                        match key_code {
                            KeyCode::F2 => {
                                self.show_stats = !self.show_stats;
                            }
                            #[cfg(feature = "debug-hud")]
                            KeyCode::F3 => {
                                // Toggle the scene-component debug panels
                                // (grass, ocean, ...). The Rendering &
                                // Effects panel is excluded — it has its
                                // own key (F4) and would otherwise flip
                                // out of phase with it.
                                let scene_panels: Vec<usize> = self
                                    .debug_panels
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, p)| {
                                        p.name() != flint_debug_ui::RENDER_DEBUG_PANEL
                                    })
                                    .map(|(i, _)| i)
                                    .collect();
                                if scene_panels.is_empty() {
                                    tracing::info!("No debug panels in current scene");
                                } else {
                                    // Toggle the panels, then adjust cursor outside the borrow
                                    for i in scene_panels {
                                        self.debug_panels[i].toggle();
                                    }
                                    // Cursor follows ALL panels (incl. the
                                    // render panel, which F3 leaves alone).
                                    let any_open =
                                        self.debug_panels.iter().any(|p| p.is_open());
                                    if any_open {
                                        self.release_cursor();
                                    } else if self.physics.has_player_entity() {
                                        self.capture_cursor();
                                    }
                                }
                            }
                            // Music Guide overlay (ADR 0035): the Phase 4
                            // debug-surface key — all F-keys are taken.
                            #[cfg(feature = "debug-hud")]
                            KeyCode::Backquote => {
                                self.toggle_named_panel(
                                    music_guide_panel::MUSIC_GUIDE_PANEL,
                                    "no music session running — nothing to guide",
                                );
                            }
                            // Manifest Map timeline strip: Backslash
                            // (unbound elsewhere; Backquote is the guide's).
                            #[cfg(feature = "debug-hud")]
                            KeyCode::Backslash => {
                                self.toggle_named_panel(
                                    timeline_panel::MANIFEST_MAP_PANEL,
                                    "no music session running — nothing to map",
                                );
                            }
                            // Rendering & Effects menu (ADR 0053): the
                            // one home for every render/post debug
                            // control the old F1/F4-F10/F12 keys used to
                            // flip blindly, plus their non-binary params.
                            #[cfg(feature = "debug-hud")]
                            KeyCode::F4 => {
                                self.toggle_named_panel(
                                    flint_debug_ui::RENDER_DEBUG_PANEL,
                                    "no renderer active — nothing to tune",
                                );
                            }
                            KeyCode::F11 => {
                                if let Some(window) = &self.window {
                                    if window.fullscreen().is_some() {
                                        window.set_fullscreen(None);
                                    } else {
                                        window.set_fullscreen(Some(
                                            winit::window::Fullscreen::Borderless(None),
                                        ));
                                    }
                                }
                            }
                            _ => {}
                        }

                        self.input.process_key_down(key_code);

                        // On Android, DPad maps to ArrowKeys — also fire gamepad
                        // button events so configs with gamepad DPad bindings work.
                        #[cfg(target_os = "android")]
                        {
                            let dpad_button = match key_code {
                                KeyCode::ArrowUp => Some("DPadUp"),
                                KeyCode::ArrowDown => Some("DPadDown"),
                                KeyCode::ArrowLeft => Some("DPadLeft"),
                                KeyCode::ArrowRight => Some("DPadRight"),
                                _ => None,
                            };
                            if let Some(btn) = dpad_button {
                                let slot = self.android_gamepad.register_device(device_id);
                                self.input.process_gamepad_button_down(slot, btn);
                            }
                        }
                    }
                    ElementState::Released => {
                        self.input.process_key_up(key_code);

                        #[cfg(target_os = "android")]
                        {
                            let dpad_button = match key_code {
                                KeyCode::ArrowUp => Some("DPadUp"),
                                KeyCode::ArrowDown => Some("DPadDown"),
                                KeyCode::ArrowLeft => Some("DPadLeft"),
                                KeyCode::ArrowRight => Some("DPadRight"),
                                _ => None,
                            };
                            if let Some(btn) = dpad_button {
                                if let Some(slot) = self.android_gamepad.slot_for(device_id) {
                                    self.input.process_gamepad_button_up(slot, btn);
                                }
                            }
                        }
                    }
                }
            }

            // Android: intercept gamepad buttons delivered as unidentified native keycodes
            #[cfg(target_os = "android")]
            if let PhysicalKey::Unidentified(NativeKeyCode::Android(keycode)) =
                event.physical_key
            {
                if let Some(button_name) = Self::android_keycode_to_gamepad_button(keycode) {
                    let slot = self.android_gamepad.register_device(device_id);
                    match event.state {
                        ElementState::Pressed => {
                            self.input.process_gamepad_button_down(slot, button_name);
                        }
                        ElementState::Released => {
                            self.input.process_gamepad_button_up(slot, button_name);
                        }
                    }
                }
            }
    }

    fn handle_mouse_input(&mut self, state: ElementState, button: MouseButton) {
            // FPS scenes gate mouse input behind click-to-capture (hides the
            // cursor for mouse-look). 2D / UI scenes have no player entity, so
            // keep the cursor visible and forward mouse buttons directly — this
            // lets screen-space UI (e.g. card games) be clicked and dragged.
            if !self.cursor_captured && self.physics.has_player_entity() {
                if state == ElementState::Pressed && button == MouseButton::Left {
                    #[cfg(feature = "debug-hud")]
                    let panel_open = self.debug_panels.iter().any(|p| p.is_open());
                    #[cfg(not(feature = "debug-hud"))]
                    let panel_open = false;
                    if !panel_open {
                        self.capture_cursor();
                    }
                }
                return;
            }

            let btn = match button {
                MouseButton::Left => 0,
                MouseButton::Right => 1,
                MouseButton::Middle => 2,
                _ => return,
            };

            match state {
                ElementState::Pressed => self.input.process_mouse_button_down(btn),
                ElementState::Released => self.input.process_mouse_button_up(btn),
            }
    }

    fn handle_touch(&mut self, touch: winit::event::Touch) {
            // Android: joystick MotionEvents arrive as Touch with axis values
            // in [-1, 1]. Route these to gamepad axis input instead of touch.
            #[cfg(target_os = "android")]
            {
                let x = touch.location.x;
                let y = touch.location.y;

                // If this device was already identified as a gamepad via button
                // presses, treat its touch events as stick axis data.
                if self.android_gamepad.is_gamepad(touch.device_id) {
                    if let Some(slot) = self.android_gamepad.slot_for(touch.device_id) {
                        self.input
                            .process_gamepad_axis(slot, "LeftStickX", x as f32);
                        self.input
                            .process_gamepad_axis(slot, "LeftStickY", y as f32);
                        return;
                    }
                }

                // Heuristic: if coordinates are in joystick range [-1.5, 1.5]
                // and this touch id has no prior Started event (process_touch_move
                // would silently drop it anyway), treat it as a joystick from an
                // unregistered gamepad device.
                if x.abs() <= 1.5 && y.abs() <= 1.5 && !self.input.has_active_touch(touch.id) {
                    let slot = self.android_gamepad.register_device(touch.device_id);
                    self.input
                        .process_gamepad_axis(slot, "LeftStickX", x as f32);
                    self.input
                        .process_gamepad_axis(slot, "LeftStickY", y as f32);
                    return;
                }
            }

            // Normal touch processing
            self.input.disable_touch_emulation();

            let id = touch.id;
            let x = touch.location.x;
            let y = touch.location.y;
            match touch.phase {
                winit::event::TouchPhase::Started => {
                    self.input.process_touch_start(id, x, y);
                }
                winit::event::TouchPhase::Moved => {
                    self.input.process_touch_move(id, x, y);
                }
                winit::event::TouchPhase::Ended => {
                    // Update position from the End event before gesture detection —
                    // fast swipes may have few/no Move events between Start and End
                    self.input.process_touch_move(id, x, y);
                    self.input.process_touch_end(id);
                }
                winit::event::TouchPhase::Cancelled => {
                    self.input.process_touch_cancel(id);
                }
            }
    }
}
