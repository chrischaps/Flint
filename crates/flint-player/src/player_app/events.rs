//! Input and window event handling for `PlayerApp` — code-motion sibling
//! of `mod.rs` (player_app decomposition; see the decomposition ADR).
//! Owns the winit `ApplicationHandler` impl, gamepad polling, rebind
//! capture/persistence, cursor capture, and the Android gamepad tracking
//! that exists only to serve these handlers.

use super::egui_input::{Held, Routed};
use super::input_script::{PointerEvent, ScriptEvent};
use super::PlayerApp;
#[cfg(feature = "debug-hud")]
use super::music_guide_panel;
#[cfg(feature = "debug-hud")]
use super::timeline_panel;
use super::input_config::{
    fallback_user_override_path, gamepad_id_to_u32, user_override_save_path,
    write_user_override_file, PendingRebind,
};
use anyhow::Result;
use flint_runtime::{Binding, RebindMode};
use gilrs::EventType;
use std::path::Path;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
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
    /// True once any device has been registered as a gamepad.
    #[cfg(target_os = "android")]
    fn any_registered(&self) -> bool {
        !self.device_slots.is_empty()
    }

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

/// Pixels of a touchpad's smooth scroll that make one wheel line, the
/// figure egui-winit uses too.
const PIXELS_PER_LINE: f32 = 50.0;

/// A wheel event in lines (notches): up and right are positive, as winit
/// reports them. A touchpad's pixel scroll is scaled to lines, so wheel
/// bindings and `mouse_wheel_*()` feel alike on either.
pub(super) fn wheel_lines(delta: MouseScrollDelta) -> (f32, f32) {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => (x, y),
        MouseScrollDelta::PixelDelta(p) => {
            (p.x as f32 / PIXELS_PER_LINE, p.y as f32 / PIXELS_PER_LINE)
        }
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

        // gamepad_connected(): gilrs knows on desktop; on Android (no gilrs
        // backend) a pad exists once any gamepad-shaped event has been seen.
        #[cfg(not(target_os = "android"))]
        {
            let connected = self
                .gilrs
                .as_ref()
                .map(|g| g.gamepads().next().is_some())
                .unwrap_or(false);
            self.input.set_gamepad_connected(connected);
        }
        #[cfg(target_os = "android")]
        {
            if self.android_gamepad.any_registered() || !events.is_empty() {
                self.input.set_gamepad_connected(true);
            }
        }

        for event in events {
            let gamepad = gamepad_id_to_u32(event.id);
            self.real_pad_slots.insert(gamepad);
            // A real pad is ignored while the window is unfocused (ADR 0110),
            // and by a capture, which hears its script's pad alone.
            let deaf = !self.window_focused || self.capture.is_capture();
            if deaf && event.event != EventType::Disconnected {
                continue;
            }
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

        let scene_path = Path::new(&self.scene_path);
        let game_id = &self.input.config().game_id;
        let mut target =
            user_override_save_path(scene_path, game_id, paths.user_override.as_deref());

        if let Err(err) = write_user_override_file(&target, &self.user_override_config) {
            let fallback = fallback_user_override_path(scene_path, game_id);
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

    /// Capture the cursor for mouse-look (ADR 0110). While it's captured the
    /// cursor reads as the window centre, its moves reach no one, and its
    /// buttons and wheel reach the game but not egui. Without a window
    /// (headless tests) only the state changes.
    pub(super) fn capture_cursor(&mut self) {
        if let Some(window) = &self.window {
            // A browser locks the pointer only from a gesture: the lock is
            // asked for once a frame while one is live (`sync_pointer_lock`).
            #[cfg(target_arch = "wasm32")]
            window.set_cursor_visible(false);
            #[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
            {
                // Try confined first, then locked
                let _ = window
                    .set_cursor_grab(CursorGrabMode::Confined)
                    .or_else(|_| window.set_cursor_grab(CursorGrabMode::Locked));
                window.set_cursor_visible(false);
            }
        }
        self.cursor_captured = true;
        self.centre_the_cursor();
        // egui stops hearing the pointer, so it mustn't think it's still
        // hovering whatever it was over.
        if let Some(egui_winit) = &mut self.egui_winit {
            egui_winit.egui_input_mut().events.push(egui::Event::PointerGone);
        }
    }

    pub(super) fn release_cursor(&mut self) {
        if let Some(window) = &self.window {
            #[cfg(not(target_os = "android"))]
            {
                let _ = window.set_cursor_grab(CursorGrabMode::None);
                window.set_cursor_visible(true);
                // The cursor comes back where it read while captured, so
                // nothing jumps under it.
                let (w, h) = self.input.screen_size();
                let _ = window.set_cursor_position(winit::dpi::PhysicalPosition::new(
                    w / 2.0,
                    h / 2.0,
                ));
            }
        }
        self.cursor_captured = false;
    }

    /// Whether the cursor is captured now (ADR 0110). A script's request
    /// takes effect at the end of the frame it's made in.
    pub fn is_cursor_captured(&self) -> bool {
        self.cursor_captured
    }

    /// Put the cursor `InputState` reports at the window centre, without a
    /// delta, as it reads while captured.
    fn centre_the_cursor(&mut self) {
        let (w, h) = self.input.screen_size();
        self.input.mouse_position = (w / 2.0, h / 2.0);
    }

    /// Whether a capture drops this real window event: it hears its
    /// script alone. The script's events take `taken_before_the_game`
    /// directly, never `window_event`, so they're never asked.
    pub(super) fn capture_drops(&self, event: &WindowEvent) -> bool {
        self.capture.is_capture() && super::egui_input::is_device_input(event)
    }

    /// Whether an egui text field has the keyboard (the text-focus gate of
    /// ADR 0073): keys, pads and raw mouse motion stay out of the game. Only
    /// a `TextEdit` counts, not any focused widget (ADR 0110's amendment).
    pub(super) fn text_has_focus(&self) -> bool {
        self.egui_sees_input() && super::egui_input::text_field_focused(&self.egui_ctx)
    }

    /// Hold the pads while a text field has focus (ADR 0110). Called at the
    /// top of each frame, before the pads are polled.
    pub(super) fn gate_gamepads(&mut self) {
        let held = self.text_has_focus();
        self.input.hold_gamepads(held);
    }

    /// The window gained or lost focus. Losing it releases every real pad,
    /// and their events are dropped until it's back; then the sticks are
    /// read again where they stand. Buttons need a fresh press. The input
    /// script's pad is left alone: captures run unfocused.
    pub(super) fn window_focus_changed(&mut self, focused: bool) {
        self.window_focused = focused;
        if !focused {
            for slot in self.real_pad_slots.iter().copied() {
                self.input.clear_gamepad(slot);
            }
        }
        let Some(gilrs) = &mut self.gilrs else {
            return;
        };
        // Drain what came in before the change, so it isn't replayed.
        while gilrs.next_event().is_some() {}
        if !focused || self.capture.is_capture() {
            return;
        }
        for (id, pad) in gilrs.gamepads() {
            let slot = gamepad_id_to_u32(id);
            self.real_pad_slots.insert(slot);
            for axis in STICKS {
                let value = pad.value(axis);
                if value != 0.0 {
                    self.input
                        .process_gamepad_axis(slot, format!("{axis:?}"), value);
                }
            }
        }
    }
}

/// The axes read again when the window regains focus.
const STICKS: [gilrs::Axis; 8] = [
    gilrs::Axis::LeftStickX,
    gilrs::Axis::LeftStickY,
    gilrs::Axis::LeftZ,
    gilrs::Axis::RightStickX,
    gilrs::Axis::RightStickY,
    gilrs::Axis::RightZ,
    gilrs::Axis::DPadX,
    gilrs::Axis::DPadY,
];

impl ApplicationHandler for PlayerApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // No window yet means first launch. (On the web the window exists
        // while the device is still on its way; don't start twice.)
        if self.window.is_none() && self.render_context.is_none() {
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
        // Not ready: the device is still on its way (web startup, #99), so
        // there is nothing to tick, draw or steer. Each event is a chance to
        // pick it up.
        if self.render_context.is_none() {
            #[cfg(target_arch = "wasm32")]
            self.take_ready_render_context(event_loop);
            if self.render_context.is_none() {
                return;
            }
        }
        if let WindowEvent::Focused(focused) = event {
            self.window_focus_changed(focused);
        }
        if self.capture_drops(&event) {
            return;
        }
        if self.taken_before_the_game(&event) {
            return;
        }

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            WindowEvent::Resized(new_size) => {
                self.handle_resize(new_size);
            }

            WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.update_vsync_pacing();
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

            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = wheel_lines(delta);
                self.input.process_mouse_wheel(dx, dy);
            }

            WindowEvent::Touch(touch) => {
                self.handle_touch(touch);
            }

            WindowEvent::RedrawRequested => {
                // A refresh the frame-rate pacer skips (#140) does nothing.
                #[cfg(target_arch = "wasm32")]
                if !self.web_frame_begin() {
                    return;
                }
                self.tick();
                self.render();
                #[cfg(target_arch = "wasm32")]
                self.web_frame_end();
                // The frame's end, for the profiler (ADR 0109).
                profiling::finish_frame!();
                if self.music_exit_requested || self.exit_requested {
                    event_loop.exit();
                }
            }

            _ => {}
        }
    }

    /// The web's device request landed (`set_event_proxy`).
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: ()) {
        #[cfg(target_arch = "wasm32")]
        self.take_ready_render_context(_event_loop);
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        // Raw motion flows while the cursor is captured, the window has
        // focus and no text field has the keyboard (ADR 0110), and never
        // into a capture (`look` is the script's).
        if !self.cursor_captured
            || !self.window_focused
            || self.text_has_focus()
            || self.capture.is_capture()
        {
            return;
        }

        // On the web the pointer moves the view only once the browser has
        // locked it; before the first click it is just crossing the page.
        #[cfg(target_arch = "wasm32")]
        if !self.pointer_lock.held {
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

    /// Every exit path (close button, Escape, --exit-after, script-requested,
    /// init failure) funnels through here: last chance to write the store.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.flush_persistent_store(true, "exit");
    }
}


// window_event's per-arm handlers (Stage 2 of the player_app decomposition):
// each is the former match-arm body, extracted verbatim. A `return` in a
// handler ends that event's handling exactly as it did in the arm — nothing
// follows the match in window_event.
impl PlayerApp {
    pub(super) fn handle_resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
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
            if self.cursor_captured {
                self.centre_the_cursor();
            }
    }

    /// Pace the clock to the window's monitor when frames wait for vblank
    /// (#406 in The Capitol). On start, and again when the window moves, since
    /// another monitor can have another refresh. `FLINT_PACING=0` turns it off.
    pub(super) fn update_vsync_pacing(&mut self) {
        // On the web the frame-rate pacer sets the period (#140).
        if self.frame_pacer.is_some() {
            return;
        }
        let on_vblank = self
            .render_context
            .as_ref()
            .is_some_and(|c| c.presents_on_vblank());
        let disabled = std::env::var("FLINT_PACING").is_ok_and(|v| v == "0");
        let period = (on_vblank && !disabled)
            .then(|| self.window.as_ref()?.current_monitor()?.refresh_rate_millihertz())
            .flatten()
            .filter(|&mhz| mhz > 0)
            .map(|mhz| 1000.0 / mhz as f64);
        if period != self.clock.vsync_period() {
            tracing::info!(
                "frame pacing: {}",
                period.map_or("off".to_string(), |p| format!("{:.2} Hz", 1.0 / p))
            );
            self.clock.set_vsync_period(period);
        }
    }

    fn handle_keyboard_input(
        &mut self,
        event_loop: &ActiveEventLoop,
        device_id: winit::event::DeviceId,
        event: winit::event::KeyEvent,
    ) {
            // Suppress unused variable warning on non-Android platforms
            let _ = &device_id;
            #[cfg(target_arch = "wasm32")]
            let _ = &event_loop;

            if let PhysicalKey::Code(key_code) = event.physical_key {
                match event.state {
                    ElementState::Pressed => {
                        // Handle escape to toggle cursor capture
                        if key_code == KeyCode::Escape {
                            if self.cursor_captured {
                                self.release_cursor();
                            } else {
                                // A page has nowhere to exit to.
                                #[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
                                event_loop.exit();
                            }
                            return;
                        }

                        // Debug keys, shared with `--input-script` (H9)
                        self.handle_debug_key(key_code);

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

    /// The player's debug keys: F2 stats overlay, F3/F4 panels, the music
    /// guide and timeline, F9 force-fail, F11 fullscreen. Called for a real
    /// key press and for one the input script presses, so a capture can
    /// open the overlay (H9). Escape stays window-only: it exits.
    pub(super) fn handle_debug_key(&mut self, key_code: KeyCode) {
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
            // Debug force-fail: trigger the full
            // reintegration (rewind → seam → reassembly)
            // without having to play to a fail — seam
            // iteration must not depend on first clearing
            // the arming threshold (ADR 0064 follow-up).
            #[cfg(feature = "debug-hud")]
            KeyCode::F9 => {
                if let Some(ms) = self.music_session.as_mut() {
                    ms.debug_force_fail();
                    println!(
                        "[music] DEBUG: full-fail forced — rewind incoming"
                    );
                } else {
                    println!(
                        "[music] no music session running — nothing to fail"
                    );
                }
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
    }

    /// Whether egui or an extension takes a window event before the player
    /// and scripts see it.
    fn taken_before_the_game(&mut self, event: &WindowEvent) -> bool {
        if let WindowEvent::ModifiersChanged(modifiers) = event {
            self.key_modifiers = modifiers.state();
        }
        // A captured cursor's moves reach no one, real or scripted: it reads
        // as the window centre (ADR 0110).
        if self.cursor_captured && matches!(event, WindowEvent::CursorMoved { .. }) {
            return true;
        }
        // Input egui wants stops here (see egui_input for the rules). A
        // captured cursor's buttons and wheel skip egui, but for the release
        // of a press egui took before the capture (ADR 0110).
        let skip_egui = self.cursor_captured && self.egui_input.skips_captured(event);
        if self.egui_sees_input() && !skip_egui {
            if let (Some(egui_winit), Some(window)) = (&mut self.egui_winit, &self.window) {
                let response = egui_winit.on_window_event(window, event);
                let wants_keyboard = super::egui_input::text_field_focused(&self.egui_ctx);
                let routed = super::egui_input::classify(event);
                if self
                    .egui_input
                    .consumes(routed, response.consumed, wants_keyboard)
                {
                    return true;
                }
            }
        }

        // Game extensions next; they may consume input but never lifecycle
        // events (see PlayerExtension::on_window_event and on_key).
        crate::extension::dispatch_window_event(&mut self.extensions, event, self.key_modifiers)
    }

    /// Whether egui sees input before the game: whenever it can have
    /// interactive UI on screen, an open debug panel or an extension's ui().
    fn egui_sees_input(&self) -> bool {
        #[cfg(feature = "debug-hud")]
        let any_panel_open = self.debug_panels.iter().any(|p| p.is_open());
        #[cfg(not(feature = "debug-hud"))]
        let any_panel_open = false;
        any_panel_open || !self.extensions.is_empty()
    }

    /// An event from `--input-script`, routed as a real device's would be.
    pub(super) fn scripted_event(&mut self, event: ScriptEvent) {
        match event {
            ScriptEvent::Pointer(pointer) => self.scripted_pointer_event(pointer),
            ScriptEvent::Key(key, state) => self.scripted_key(key, state),
            ScriptEvent::Modifiers(state) => {
                // A real `ModifiersChanged`: egui-winit keeps the modifiers
                // for the events after it, extensions see it too, and so
                // does the next key's `KeyInput`.
                self.taken_before_the_game(&WindowEvent::ModifiersChanged(state.into()));
            }
            // Raw motion takes a real mouse's gate (ADR 0110), but for the
            // window's focus: captures run unfocused.
            ScriptEvent::Look(dx, dy) => {
                if self.cursor_captured && !self.text_has_focus() {
                    self.input.process_mouse_raw_delta(dx, dy);
                }
            }
            ScriptEvent::Text(text) => {
                if self.egui_sees_input() {
                    if let Some(egui_winit) = &mut self.egui_winit {
                        egui_winit
                            .egui_input_mut()
                            .events
                            .push(egui::Event::Text(text));
                    }
                }
            }
        }
    }

    /// A key from `--input-script` (ADR 0097). egui gets what egui-winit
    /// would have made of it, and the same gate as a real key decides
    /// whether the game does: a key typed into a focused field stops there.
    /// Then extensions' `on_key` (ADR 0103), which may take it. Their
    /// `on_window_event` never sees one: it takes a winit `KeyboardInput`,
    /// whose `KeyEvent` can't be built outside winit.
    fn scripted_key(&mut self, key: KeyCode, state: ElementState) {
        let pressed = state == ElementState::Pressed;
        if self.egui_sees_input() {
            if let Some(egui_winit) = &mut self.egui_winit {
                let raw = egui_winit.egui_input_mut();
                let events = super::egui_input::scripted_key_events(key, pressed, raw.modifiers);
                raw.events.extend(events);
            }
            let held = Held::Key(PhysicalKey::Code(key));
            let routed = if pressed {
                Routed::Press(held)
            } else {
                Routed::Release(held)
            };
            let wants_keyboard = super::egui_input::text_field_focused(&self.egui_ctx);
            if self.egui_input.consumes(routed, false, wants_keyboard) {
                return;
            }
        }
        let mut buf = [0; 4];
        let character = super::egui_input::typed_char(key, self.key_modifiers.shift_key())
            .map(|c| &*c.encode_utf8(&mut buf));
        let input = crate::extension::KeyInput {
            key,
            state,
            repeat: false,
            character,
            modifiers: self.key_modifiers,
            scripted: true,
        };
        if crate::extension::dispatch_key(&mut self.extensions, &input) {
            return;
        }
        // As a real key, through the debug keys (H9), but Esc doesn't quit.
        if pressed {
            self.handle_debug_key(key);
            self.input.process_key_down(key);
        } else {
            self.input.process_key_up(key);
        }
    }

    /// A pointer event from `--input-script` (ADR 0082). It takes the path a
    /// real one does: egui, then extensions, then `InputState`.
    pub(super) fn scripted_pointer_event(&mut self, event: PointerEvent) {
        let event = event.to_window_event();
        if self.taken_before_the_game(&event) {
            return;
        }
        match event {
            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_input(state, button);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.input.process_mouse_move(position.x, position.y);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = wheel_lines(delta);
                self.input.process_mouse_wheel(dx, dy);
            }
            _ => {}
        }
    }

    fn handle_mouse_input(&mut self, state: ElementState, button: MouseButton) {
            // Mouse button state always reaches InputState so scripts can
            // read it (`is_mouse_pressed`, menus, screen-space UI) and the
            // mouse-as-touch emulation keeps working. FPS scenes (a player
            // entity exists) additionally treat the first left click as
            // click-to-capture, which hides the cursor for mouse-look.
            if let Some(btn) = match button {
                MouseButton::Left => Some(0),
                MouseButton::Right => Some(1),
                MouseButton::Middle => Some(2),
                _ => None,
            } {
                match state {
                    ElementState::Pressed => self.input.process_mouse_button_down(btn),
                    ElementState::Released => self.input.process_mouse_button_up(btn),
                }
            }

            if !self.cursor_captured
                && self.physics.has_player_entity()
                && state == ElementState::Pressed
                && button == MouseButton::Left
            {
                #[cfg(feature = "debug-hud")]
                let panel_open = self.debug_panels.iter().any(|p| p.is_open());
                #[cfg(not(feature = "debug-hud"))]
                let panel_open = false;
                if !panel_open {
                    self.capture_cursor();
                }
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

#[cfg(test)]
mod wheel_tests {
    use super::wheel_lines;
    use winit::dpi::PhysicalPosition;
    use winit::event::MouseScrollDelta;

    #[test]
    fn a_notch_is_a_line_and_fifty_pixels_are_another() {
        assert_eq!(wheel_lines(MouseScrollDelta::LineDelta(0.0, 1.0)), (0.0, 1.0));
        assert_eq!(wheel_lines(MouseScrollDelta::LineDelta(-2.0, 0.0)), (-2.0, 0.0));
        let px = MouseScrollDelta::PixelDelta(PhysicalPosition::new(25.0, -100.0));
        assert_eq!(wheel_lines(px), (0.5, -2.0));
    }
}
