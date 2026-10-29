//! Flint on the web, M0 spike (#95, docs/design/web.md).
//!
//! The smallest proof that the player's stack reaches a browser: winit makes
//! a canvas, wgpu opens a device on the WebGPU backend, and every frame
//! clears it to the sky. The sky drifts slowly through dawn, noon, dusk and
//! night so a still page is visibly alive. No scene, scripts or assets yet;
//! those wait on the vfs and async startup tickets.
//!
//! Build and serve from `crates/flint-player`:
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
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;
    use winit::application::ApplicationHandler;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
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

    struct Gpu {
        surface: wgpu::Surface<'static>,
        device: wgpu::Device,
        queue: wgpu::Queue,
        config: wgpu::SurfaceConfiguration,
    }

    #[derive(Default)]
    struct App {
        window: Option<Arc<Window>>,
        gpu: Rc<RefCell<Option<Gpu>>>,
    }

    pub fn run() {
        console_error_panic_hook::set_once();
        let event_loop = EventLoop::new().expect("event loop");
        event_loop.spawn_app(App::default());
    }

    fn log(msg: &str) {
        web_sys::console::log_1(&msg.into());
    }

    fn now_seconds() -> f64 {
        web_sys::window()
            .and_then(|w| w.performance())
            .map_or(0.0, |p| p.now() / 1000.0)
    }

    fn srgb_to_linear(c: f64) -> f64 {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    /// The sky at `t` seconds, eased between stops, as a linear clear colour
    /// (the surface is sRGB, so the GPU encodes it back on write).
    fn sky_at(t: f64, srgb_surface: bool) -> wgpu::Color {
        let day = (t / DAY_SECONDS + START_PHASE).fract() * SKY.len() as f64;
        let i = day.floor() as usize;
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
        wgpu::Color { r: mix(0), g: mix(1), b: mix(2), a: 1.0 }
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
        log(&format!("[flint-web] adapter: {:?} via {:?}", info.name, info.backend));
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
        Ok(Gpu { surface, device, queue, config })
    }

    impl Gpu {
        fn resize(&mut self, width: u32, height: u32) {
            self.config.width = width.max(1);
            self.config.height = height.max(1);
            self.surface.configure(&self.device, &self.config);
        }

        fn draw(&mut self) {
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
                        load: wgpu::LoadOp::Clear(sky_at(
                            now_seconds(),
                            self.config.format.is_srgb(),
                        )),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            self.queue.submit([encoder.finish()]);
            frame.present();
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
                WindowEvent::RedrawRequested => {
                    if let Some(gpu) = self.gpu.borrow_mut().as_mut() {
                        gpu.draw();
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                _ => {}
            }
        }
    }
}
