//! Flint on the web: the M0 spike (#95) and the M2 shim check (#100).
//! See docs/design/web.md.
//!
//! winit makes a canvas, wgpu opens a device on the WebGPU backend, and every
//! frame clears it to the sky, which drifts through dawn, noon, dusk and
//! night. On top of that it runs the three shims a real scene needs:
//!
//! - **Time**: the day runs on `web_time::Instant`, the engine's clock type,
//!   where `std::time::Instant` would panic.
//! - **Gamepads**: gilrs on the browser Gamepad API. The left stick scrubs
//!   the day, and `pause_day` (Space or the pad's South button) holds it,
//!   through the same `InputState` the player uses.
//! - **Settings**: R and then any key or pad button rebinds `pause_day`. The
//!   rebind is saved through `flint_player::user_files`, which is
//!   localStorage here, so it is still there after a reload.
//!
//! No scene, scripts or assets yet; those wait on the vfs and async startup
//! tickets. Build and serve from `crates/flint-player`:
//!   trunk serve            # http://127.0.0.1:8080
//!   trunk build --release  # dist/, wasm-opt -Oz applied

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("flint-web is the browser entry; run `trunk serve` in crates/flint-player");
}

#[cfg(target_arch = "wasm32")]
fn main() {
    web::run();
}

#[cfg(target_arch = "wasm32")]
mod web {
    use flint_player::user_files;
    use flint_runtime::{
        ActionConfig, ActionKind, Binding, GamepadSelector, InputConfig, InputState, RebindMode,
    };
    use gilrs::{Axis, EventType, Gilrs};
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::Arc;
    use web_time::Instant;
    use winit::application::ApplicationHandler;
    use winit::event::{ElementState, KeyEvent, WindowEvent};
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::keyboard::{KeyCode, PhysicalKey};
    use winit::platform::web::{EventLoopExtWebSys, WindowAttributesExtWebSys};
    use winit::window::{Window, WindowId};

    /// One day of sky, in sRGB, at evenly spaced stops; the last wraps to the
    /// first. Night, dawn, morning, noon, afternoon, dusk.
    const SKY: [[f64; 3]; 6] = [
        [0.05, 0.07, 0.16],
        [0.93, 0.62, 0.52],
        [0.62, 0.80, 0.95],
        [0.45, 0.70, 0.96],
        [0.70, 0.78, 0.90],
        [0.96, 0.50, 0.36],
    ];
    /// Seconds for the sky to go round once.
    const DAY_SECONDS: f64 = 60.0;
    /// Start a little after dawn, so the first frame is already sky blue.
    const START_PHASE: f64 = 2.0 / 6.0;
    /// How many times faster than real time a full stick deflection scrubs.
    const SCRUB_RATE: f64 = 12.0;
    /// The page's game id, so its rebinds have a file of their own.
    const GAME_ID: &str = "flint_web";
    const PAUSE: &str = "pause_day";

    struct Gpu {
        surface: wgpu::Surface<'static>,
        device: wgpu::Device,
        queue: wgpu::Queue,
        config: wgpu::SurfaceConfiguration,
    }

    /// The day's clock: real time, held while paused, pushed by the stick.
    struct Day {
        last: Instant,
        /// Seconds into the day cycle.
        t: f64,
        paused: bool,
    }

    impl Day {
        fn new() -> Self {
            Self {
                last: Instant::now(),
                t: 0.0,
                paused: false,
            }
        }

        fn step(&mut self, scrub: f64) {
            let now = Instant::now();
            let dt = now.duration_since(self.last).as_secs_f64();
            self.last = now;
            let rate = if self.paused { 0.0 } else { 1.0 } + scrub * SCRUB_RATE;
            self.t = (self.t + dt * rate).rem_euclid(DAY_SECONDS);
        }

        /// Position in the day, 0..1, with the start offset applied.
        fn phase(&self) -> f64 {
            (self.t / DAY_SECONDS + START_PHASE).fract()
        }

        /// The clock face for the caption: midnight at the night stop.
        fn clock(&self) -> String {
            let minutes = (self.phase() * 24.0 * 60.0) as u32;
            format!("{:02}:{:02}", minutes / 60, minutes % 60)
        }
    }

    struct Controls {
        input: InputState,
        gilrs: Option<Gilrs>,
        /// The person's rebinds, as the player keeps them: only the actions
        /// they changed.
        user: InputConfig,
        user_path: Option<PathBuf>,
        rebinding: bool,
        pad_name: Option<String>,
    }

    impl Controls {
        fn new() -> Self {
            let mut input = InputState::new();
            let game = InputConfig {
                version: 1,
                game_id: GAME_ID.into(),
                actions: [(
                    PAUSE.to_string(),
                    ActionConfig {
                        kind: ActionKind::Button,
                        bindings: vec![
                            Binding::Key {
                                code: "Space".into(),
                                scale: 1.0,
                            },
                            Binding::GamepadButton {
                                button: "South".into(),
                                gamepad: GamepadSelector::Any,
                            },
                        ],
                    },
                )]
                .into(),
            };
            input.load_bindings(game).expect("pause_day config");

            // The player's own layout: <config>/flint/input_<game_id>.toml,
            // which on the web is a localStorage key.
            let user_path = user_files::config_dir()
                .map(|dir| dir.join("flint").join(format!("input_{GAME_ID}.toml")));
            let mut user = InputConfig {
                version: 1,
                game_id: GAME_ID.into(),
                actions: Default::default(),
            };
            if let Some(path) = &user_path {
                match user_files::read(path).map(|t| t.map(|t| InputConfig::from_toml_str(&t))) {
                    Ok(Some(Ok(cfg))) => {
                        log(&format!(
                            "[flint-web] rebinds loaded from {}",
                            user_files::storage_key(path)
                        ));
                        user = cfg.clone();
                        if let Err(e) = input.merge_bindings(cfg) {
                            log(&format!("[flint-web] rebinds ignored: {e}"));
                        }
                    }
                    Ok(Some(Err(e))) => log(&format!("[flint-web] rebinds unreadable: {e}")),
                    Ok(None) => {}
                    Err(e) => log(&format!("[flint-web] {e}")),
                }
            }

            let gilrs = match Gilrs::new() {
                Ok(g) => Some(g),
                Err(e) => {
                    log(&format!("[flint-web] no gamepad backend: {e}"));
                    None
                }
            };
            Self {
                input,
                gilrs,
                user,
                user_path,
                rebinding: false,
                pad_name: None,
            }
        }

        /// A key or pad button while R is armed becomes `pause_day`'s binding.
        fn capture(&mut self, binding: Binding) -> bool {
            if !self.rebinding {
                return false;
            }
            self.rebinding = false;
            if let Err(e) = self
                .input
                .rebind_action(PAUSE, binding, RebindMode::Replace)
            {
                log(&format!("[flint-web] rebind refused: {e}"));
                return true;
            }
            if let Some(cfg) = self.input.action_config(PAUSE) {
                self.user.actions.insert(PAUSE.into(), cfg);
            }
            if let Some(path) = &self.user_path {
                let saved = toml::to_string_pretty(&self.user)
                    .map_err(|e| e.to_string())
                    .and_then(|text| user_files::write(path, &text).map_err(|e| e.to_string()));
                match saved {
                    Ok(()) => log(&format!(
                        "[flint-web] {PAUSE} -> {}, saved to {}",
                        self.label(),
                        user_files::storage_key(path)
                    )),
                    Err(e) => log(&format!("[flint-web] rebind not saved: {e}")),
                }
            }
            true
        }

        fn key(&mut self, code: KeyCode, state: ElementState, repeat: bool) {
            match state {
                ElementState::Pressed if !repeat => {
                    if self.capture(Binding::Key {
                        code: format!("{code:?}"),
                        scale: 1.0,
                    }) {
                        return;
                    }
                    if code == KeyCode::KeyR {
                        self.rebinding = true;
                        return;
                    }
                    self.input.process_key_down(code);
                }
                ElementState::Released => self.input.process_key_up(code),
                _ => {}
            }
        }

        /// Drain gilrs into the input state, as the player's
        /// `poll_gamepad_events` does. Returns the left stick's X.
        fn poll_pads(&mut self) -> f64 {
            let Some(gilrs) = &mut self.gilrs else {
                return 0.0;
            };
            let mut events = Vec::new();
            while let Some(event) = gilrs.next_event() {
                events.push(event);
            }
            self.pad_name = gilrs.gamepads().next().map(|(_, g)| g.name().to_string());
            self.input.set_gamepad_connected(self.pad_name.is_some());
            let stick = gilrs
                .gamepads()
                .next()
                .map_or(0.0, |(_, g)| g.value(Axis::LeftStickX) as f64);

            for event in events {
                let pad: usize = event.id.into();
                let pad = pad as u32;
                match event.event {
                    EventType::Connected => log(&format!(
                        "[flint-web] gamepad connected: {:?}",
                        self.pad_name
                    )),
                    EventType::ButtonPressed(button, _) => {
                        let name = format!("{button:?}");
                        let binding = Binding::GamepadButton {
                            button: name.clone(),
                            gamepad: GamepadSelector::Any,
                        };
                        if !self.capture(binding) {
                            self.input.process_gamepad_button_down(pad, name);
                        }
                    }
                    EventType::ButtonReleased(button, _) => {
                        self.input
                            .process_gamepad_button_up(pad, format!("{button:?}"));
                    }
                    EventType::Disconnected => self.input.clear_gamepad(pad),
                    _ => {}
                }
            }
            // The stick's own deadzone, so a resting pad doesn't creep.
            if stick.abs() < 0.15 {
                0.0
            } else {
                stick
            }
        }

        fn label(&self) -> String {
            self.input
                .primary_binding_label(PAUSE)
                .unwrap_or_else(|| "?".into())
        }
    }

    #[derive(Default)]
    struct App {
        window: Option<Arc<Window>>,
        gpu: Rc<RefCell<Option<Gpu>>>,
        state: Option<(Day, Controls)>,
        caption: String,
    }

    pub fn run() {
        console_error_panic_hook::set_once();
        let event_loop = EventLoop::new().expect("event loop");
        event_loop.spawn_app(App::default());
    }

    fn log(msg: &str) {
        web_sys::console::log_1(&msg.into());
    }

    /// Write the caption under the Flint mark, only when it changes.
    fn set_caption(last: &mut String, text: String) {
        if *last == text {
            return;
        }
        if let Some(el) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("status"))
        {
            el.set_text_content(Some(&text));
        }
        *last = text;
    }

    fn srgb_to_linear(c: f64) -> f64 {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    /// The sky at `phase` (0..1 of a day), eased between stops, as a clear
    /// colour; linear when the surface is sRGB, so the GPU encodes it back.
    fn sky_at(phase: f64, srgb_surface: bool) -> wgpu::Color {
        let day = phase * SKY.len() as f64;
        let i = day.floor() as usize % SKY.len();
        let f = day.fract();
        let f = f * f * (3.0 - 2.0 * f);
        let (a, b) = (SKY[i], SKY[(i + 1) % SKY.len()]);
        let mix = |k: usize| {
            let c = a[k] + (b[k] - a[k]) * f;
            if srgb_surface {
                srgb_to_linear(c)
            } else {
                c
            }
        };
        wgpu::Color {
            r: mix(0),
            g: mix(1),
            b: mix(2),
            a: 1.0,
        }
    }

    async fn init_gpu(window: Arc<Window>) -> Result<Gpu, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU,
            ..Default::default()
        });
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("create_surface: {e}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                // Chrome on Windows ignores the preference and says so in
                // the console; leave it unset.
                power_preference: wgpu::PowerPreference::None,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .ok_or("no WebGPU adapter (is WebGPU enabled in this browser?)")?;
        let info = adapter.get_info();
        log(&format!(
            "[flint-web] adapter: {:?} via {:?}",
            info.name, info.backend
        ));
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("flint-web"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::default(),
                },
                None,
            )
            .await
            .map_err(|e| format!("request_device: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        log(&format!(
            "[flint-web] surface {}x{} {:?}",
            config.width, config.height, config.format
        ));
        Ok(Gpu {
            surface,
            device,
            queue,
            config,
        })
    }

    impl Gpu {
        fn resize(&mut self, width: u32, height: u32) {
            self.config.width = width.max(1);
            self.config.height = height.max(1);
            self.surface.configure(&self.device, &self.config);
        }

        fn draw(&mut self, phase: f64) {
            let frame = match self.surface.get_current_texture() {
                Ok(frame) => frame,
                Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                    self.surface.configure(&self.device, &self.config);
                    return;
                }
                Err(e) => {
                    log(&format!("[flint-web] frame skipped: {e}"));
                    return;
                }
            };
            let view = frame.texture.create_view(&Default::default());
            let mut encoder = self.device.create_command_encoder(&Default::default());
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sky clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(sky_at(phase, self.config.format.is_srgb())),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            self.queue.submit([encoder.finish()]);
            frame.present();
        }
    }

    impl App {
        fn frame(&mut self) {
            let Some((day, controls)) = &mut self.state else {
                return;
            };
            let scrub = controls.poll_pads();
            if controls.input.is_action_just_pressed(PAUSE) {
                day.paused = !day.paused;
            }
            controls.input.end_frame();
            day.step(scrub);

            let held = if day.paused { " · held" } else { "" };
            let hint = if controls.rebinding {
                "press a key or pad button for pause".to_string()
            } else {
                format!("{} pauses · R rebinds", controls.label())
            };
            let pad = controls
                .pad_name
                .as_deref()
                .unwrap_or("press a pad button to wake it");
            set_caption(
                &mut self.caption,
                format!("{}{held} · {hint} · pad: {pad}", day.clock()),
            );

            if let Some(gpu) = self.gpu.borrow_mut().as_mut() {
                gpu.draw(day.phase());
            }
        }
    }

    impl ApplicationHandler for App {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let attrs = Window::default_attributes()
                .with_title("Flint")
                .with_append(true);
            let window = Arc::new(event_loop.create_window(attrs).expect("canvas"));
            self.window = Some(window.clone());
            self.state = Some((Day::new(), Controls::new()));

            // wgpu's device request is a JS promise: the event loop keeps
            // running while it resolves, and the first redraw follows it.
            let slot = self.gpu.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match init_gpu(window.clone()).await {
                    Ok(gpu) => {
                        *slot.borrow_mut() = Some(gpu);
                        window.request_redraw();
                    }
                    Err(e) => web_sys::console::error_1(&format!("[flint-web] {e}").into()),
                }
            });
        }

        fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
            match event {
                WindowEvent::Resized(size) => {
                    if let Some(gpu) = self.gpu.borrow_mut().as_mut() {
                        gpu.resize(size.width, size.height);
                    }
                }
                WindowEvent::KeyboardInput {
                    event:
                        KeyEvent {
                            physical_key: PhysicalKey::Code(code),
                            state,
                            repeat,
                            ..
                        },
                    ..
                } => {
                    if let Some((_, controls)) = &mut self.state {
                        controls.key(code, state, repeat);
                    }
                }
                WindowEvent::RedrawRequested => {
                    self.frame();
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                _ => {}
            }
        }
    }
}
