//! Windowed front end for `flint play-chart` — a bare wgpu window over the
//! same [`ChartSession`] the console loop drives.
//!
//! Exists for two reasons. Practical: gamepad-to-keyboard mappers (Legion
//! Space, Steam desktop layouts) inject keystrokes at the OS level, and a
//! focused game window absorbs them harmlessly instead of letting them type
//! into the terminal mid-session. Design: it carries the first wordless
//! visual cues — the GDD forbids meters/icons/grades, so everything here is
//! field and glow, nothing is a readout:
//!
//! - a warm glow drifts where the chart wants the lean (the "somewhere to
//!   sway toward" that is inaudible in a stem-only build),
//! - a cooler, smaller glow echoes the player's actual lean — togetherness
//!   reads as the two lights merging, not as a score,
//! - the whole field breathes with the beat and bar from the Conductor,
//! - a pulse press blooms a brief expanding ring from the player's light,
//! - low coherence drains saturation (foreshadowing the disintegration
//!   language), high coherence lets the palette warm.
//!
//! Debug numbers stay on stdout, exactly as in console mode. Keys: `r`
//! reloads the coherence config, `q`/Escape quits.

use anyhow::{Context, Result};
use flint_render::RenderContext;
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use super::play_chart::{ChartSession, Tick};

pub fn run_windowed(session: ChartSession) -> Result<()> {
    let event_loop = EventLoop::new().context("creating event loop")?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        session: Some(session),
        gfx: None,
        start: Instant::now(),
        error: None,
    };
    event_loop.run_app(&mut app).context("event loop")?;
    if let Some(err) = app.error.take() {
        return Err(err);
    }
    match app.session.take() {
        Some(session) => session.finish(),
        None => Ok(()),
    }
}

struct App {
    session: Option<ChartSession>,
    gfx: Option<Gfx>,
    start: Instant,
    error: Option<anyhow::Error>,
}

impl App {
    /// Run one session tick; on finish or error, leave the loop.
    fn drive(&mut self, event_loop: &ActiveEventLoop) {
        let Some(session) = &mut self.session else {
            return;
        };
        match session.tick() {
            Ok(Tick::Running) => {}
            Ok(Tick::Finished) => event_loop.exit(),
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("star child — feel harness")
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 960.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.error = Some(anyhow::anyhow!("creating window: {e}"));
                event_loop.exit();
                return;
            }
        };
        match pollster::block_on(RenderContext::new(window.clone())) {
            Ok(ctx) => self.gfx = Some(Gfx::new(window, ctx)),
            Err(e) => {
                self.error = Some(anyhow::anyhow!("initializing wgpu: {e}"));
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gfx) = &mut self.gfx {
                    gfx.ctx.resize(size);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed || event.repeat {
                    return;
                }
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::Escape) | PhysicalKey::Code(KeyCode::KeyQ) => {
                        println!("quit requested");
                        event_loop.exit();
                    }
                    PhysicalKey::Code(KeyCode::KeyR) => {
                        if let Some(session) = &mut self.session {
                            if let Err(e) = session.reload_config() {
                                self.error = Some(e);
                                event_loop.exit();
                            }
                        }
                    }
                    // Anything else is likely mapper spam; swallow it here —
                    // that containment is half the point of this window.
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => {
                let (Some(gfx), Some(session)) = (&mut self.gfx, &self.session) else {
                    return;
                };
                let frame = session.visual_frame();
                if let Err(e) = gfx.render(&frame, self.start.elapsed().as_secs_f32()) {
                    self.error = Some(e);
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Vsync paces this loop (~monitor rate): present blocks, so ticking
        // here drains queues often enough. The audio clock owns time.
        self.drive(event_loop);
        if let Some(gfx) = &self.gfx {
            gfx.window.request_redraw();
        }
    }
}

/// 16 f32s, matching `Uni` in the WGSL below (four 16-byte rows).
const UNIFORM_SIZE: u64 = 64;

struct Gfx {
    window: Arc<Window>,
    ctx: RenderContext,
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl Gfx {
    fn new(window: Arc<Window>, ctx: RenderContext) -> Self {
        let device = &ctx.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("play-chart-window"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("play-chart-uniforms"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("play-chart-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE),
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("play-chart-bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("play-chart-pl"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("play-chart-pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: ctx.config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        Self {
            window,
            ctx,
            pipeline,
            uniforms,
            bind_group,
        }
    }

    fn render(&mut self, frame: &super::play_chart::VisualFrame, time_s: f32) -> Result<()> {
        let aspect = self.ctx.aspect_ratio();
        let data: [f32; 16] = [
            frame.lean[0],
            frame.lean[1],
            frame.target[0],
            frame.target[1],
            frame.beat_phase,
            frame.bar_phase,
            frame.coherence,
            frame.pulse_age_s.min(1e3),
            aspect,
            if frame.preroll { 1.0 } else { 0.0 },
            time_s,
            frame.desaturate,
            frame.blur,
            frame.chromatic,
            frame.reassembly,
            0.0,
        ];
        self.ctx
            .queue
            .write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&data));

        let surface = match self.ctx.surface.get_current_texture() {
            Ok(s) => s,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.ctx.resize(self.ctx.size);
                return Ok(()); // retry next frame
            }
            Err(e) => return Err(anyhow::anyhow!("acquiring surface: {e}")),
        };
        let view = surface
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("play-chart-encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("play-chart-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.ctx.queue.submit([encoder.finish()]);
        surface.present();
        Ok(())
    }
}

const SHADER: &str = r#"
struct Uni {
    lean: vec2<f32>,
    tgt: vec2<f32>,
    // beat_phase, bar_phase, coherence, pulse_age_s
    misc0: vec4<f32>,
    // aspect, preroll, time_s, desaturate
    misc1: vec4<f32>,
    // blur, chromatic, reassembly, _pad
    misc2: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uni;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    // Clip-space xy passed through: y is up, matching stick space.
    @location(0) p: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var corners = array<vec2<f32>, 3>(
        vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0),
    );
    var out: VsOut;
    out.pos = vec4(corners[vi], 0.0, 1.0);
    out.p = corners[vi];
    return out;
}

// The whole luminous field at one sample point. Factored out so chromatic
// offset can evaluate it per channel at shifted positions — the image is
// procedural, so "aberration" is re-rendering, not texture sampling.
fn field(q: vec2<f32>) -> vec3<f32> {
    let beat_phase = u.misc0.x;
    let bar_phase = u.misc0.y;
    let pulse_age = u.misc0.w;
    let t = u.misc1.z;
    let blur = u.misc2.x;

    // Defocus: rung blur widens and dims every glow — light losing its
    // edges, not an image losing pixels.
    let soft = 1.0 + blur * 3.0;
    let dim = 1.0 / (1.0 + blur * 0.8);

    // Deep indigo ground with a slow vertical drift.
    var col = mix(
        vec3(0.030, 0.028, 0.070),
        vec3(0.075, 0.050, 0.125),
        0.5 + 0.5 * q.y + 0.06 * sin(t * 0.11),
    );

    // Breathing: a soft swell decaying through each beat, deeper each bar.
    col += vec3(0.045) * exp(-5.0 * beat_phase) + vec3(0.05) * exp(-3.0 * bar_phase);

    // The chart's lean target: a warm light to sway toward.
    let dt = distance(q, u.tgt * 0.6);
    col += vec3(0.85, 0.60, 0.36) * 0.34 * dim * exp(-dt * dt / (0.085 * soft));

    // The player's own lean: a cooler, closer light. Togetherness reads as
    // the two lights merging and brightening — never as a number.
    let dp = distance(q, u.lean * 0.6);
    col += vec3(0.34, 0.52, 0.85) * 0.30 * dim * exp(-dp * dp / (0.032 * soft));

    // Pulse: a ring blooms out from the player's light and fades.
    if (pulse_age < 1.4) {
        let r = pulse_age * 0.85;
        let d = distance(q, u.lean * 0.6) - r;
        let ring = exp(-d * d / (0.0016 * soft)) * exp(-2.6 * pulse_age);
        col += vec3(0.90, 0.84, 0.68) * 0.45 * dim * ring;
    }
    return col;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let coherence = u.misc0.z;
    let aspect = u.misc1.x;
    let preroll = u.misc1.y;
    let t = u.misc1.z;
    let desat = u.misc1.w;
    let blur = u.misc2.x;
    let chroma = u.misc2.y;
    let reassembly = u.misc2.z;

    let p = vec2(in.p.x * aspect, in.p.y);
    // Woozy: the whole field swims a little, always — more as focus goes.
    let wob = vec2(
        sin(t * 0.31 + p.y * 1.7),
        cos(t * 0.23 + p.x * 1.9),
    ) * (0.025 + 0.035 * blur);
    let q = p + wob;

    // Chromatic wobble: color planes drift apart radially, breathing slowly
    // so it reads as instability, not a filter.
    var col: vec3<f32>;
    let ca = chroma * 0.05 * (0.7 + 0.3 * sin(t * 0.9));
    if (ca > 0.0005) {
        col = vec3(
            field(q * (1.0 + ca)).r,
            field(q).g,
            field(q * (1.0 - ca)).b,
        );
    } else {
        col = field(q);
    }

    // Coherence drains or restores the world's color — the disintegration
    // language in miniature. Never fully grey: the field stays alive.
    var grey = vec3(dot(col, vec3(0.299, 0.587, 0.114)));
    col = mix(grey * 0.72, col, 0.30 + 0.70 * coherence);

    // Ladder desaturation stacks on top: the rungs pull further toward ash.
    grey = vec3(dot(col, vec3(0.299, 0.587, 0.114)));
    col = mix(col, grey * 0.62, desat);

    // Reassembly: while the world re-gathers (progress < 1), a warm veil —
    // home's light — suffuses everything and recedes as the field returns.
    if (reassembly < 1.0) {
        let gather = 1.0 - reassembly;
        col += vec3(0.30, 0.20, 0.10) * gather * (0.55 + 0.15 * sin(t * 1.7));
    }

    // Pre-roll: lights low until the suite begins.
    col *= mix(1.0, 0.35, preroll);

    return vec4(col, 1.0);
}
"#;

#[cfg(test)]
mod tests {
    /// The shader only compiles at window creation in normal use; parse and
    /// validate it headlessly so a WGSL slip fails `cargo test`, not a
    /// feel session.
    #[test]
    fn window_shader_parses_and_validates() {
        let module = naga::front::wgsl::parse_str(super::SHADER).expect("WGSL parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("WGSL validation");
    }
}
