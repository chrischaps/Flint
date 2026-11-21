//! wgpu render context setup

use crate::gpu_counts::CountedDevice;
use std::sync::Arc;
use thiserror::Error;
use winit::window::Window;

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("Failed to create surface: {0}")]
    SurfaceCreation(String),
    // `flint goldens` and its test read "Failed to get adapter" as no GPU:
    // keep those words first (#220 added the hint after them)
    #[error("Failed to get adapter: {}", crate::gpu_select::no_adapter_hint())]
    AdapterNotFound,
    #[error("Failed to create device: {0}")]
    DeviceCreation(String),
    #[error("Surface error: {0}")]
    SurfaceError(String),
    #[error("Failed to read render buffer: {0}")]
    BufferReadFailed(String),
}

/// wgpu render context containing device, queue, and surface
pub struct RenderContext {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub size: winit::dpi::PhysicalSize<u32>,
    pub depth_texture: wgpu::Texture,
    /// The renderer keeps bind groups over it for as long as it is the same
    /// view (#379, #960): a texture made again, here or by a host, is a new
    /// view and gets new groups.
    pub depth_view: wgpu::TextureView,
    // Retained for surface recreation on Android resume
    instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    /// True when the swapchain was configured with `COPY_SRC`, so the
    /// presented frame can be read back via [`Self::read_surface_rgba`].
    pub surface_copyable: bool,
    /// Format for the HUD (egui) pass view: the swapchain format without its
    /// sRGB suffix, so egui's own gamma handling applies and alpha blends in
    /// display space.
    pub hud_format: wgpu::TextureFormat,
    /// Format the scene's last pass writes the swapchain in: always sRGB
    /// when the surface can give an sRGB view. A desktop surface is sRGB
    /// itself; a browser canvas offers only `Bgra8Unorm`/`Rgba8Unorm`, so
    /// it is configured plain with an sRGB view format (#102) and the frame
    /// is drawn through that view (see [`Self::frame_view`]).
    pub view_format: wgpu::TextureFormat,
    /// The present modes the surface offers, kept so
    /// [`Self::present_mode_in_use`] can say which one an `Auto` mode
    /// became (#380).
    pub supported_present_modes: Vec<wgpu::PresentMode>,
}

/// The present mode wgpu configures for `requested` on a surface offering
/// `supported`: an `Auto` mode is the first of its list the surface has,
/// as wgpu-core picks it (`AutoVsync`: FifoRelaxed, Fifo; `AutoNoVsync`:
/// Immediate, Mailbox, Fifo; `device/surface_config.rs` in wgpu-core 30,
/// the same lists as 23's). Any other mode is used as asked.
pub fn resolve_present_mode(
    requested: wgpu::PresentMode,
    supported: &[wgpu::PresentMode],
) -> wgpu::PresentMode {
    use wgpu::PresentMode as P;
    let order: &[P] = match requested {
        P::AutoVsync => &[P::FifoRelaxed, P::Fifo],
        P::AutoNoVsync => &[P::Immediate, P::Mailbox, P::Fifo],
        other => return other,
    };
    order
        .iter()
        .copied()
        .find(|m| supported.contains(m))
        .unwrap_or(P::Fifo)
}

/// How frames reach the display (#1149): `flint play --vsync`, or its
/// alias `FLINT_VSYNC`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Vsync {
    /// Wait for vsync (Fifo): the default.
    #[default]
    Fifo,
    /// Unlocked (Immediate where the surface has it): real frame times for
    /// perf runs, with tearing.
    Off,
    /// Render uncapped and show the newest frame at each refresh: no
    /// tearing, no Fifo stall. On an Optimus laptop Fifo has been seen
    /// pacing at ~7 Hz against a virtual display; mailbox sidesteps it.
    Mailbox,
}

impl Vsync {
    /// The words `--vsync` and `FLINT_VSYNC` take, in `flint env`'s order.
    pub const WORDS: [&'static str; 7] = ["1", "on", "fifo", "0", "off", "immediate", "mailbox"];

    /// Read one of [`Self::WORDS`], any case.
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "1" | "on" | "fifo" => Some(Self::Fifo),
            "0" | "off" | "immediate" => Some(Self::Off),
            "mailbox" => Some(Self::Mailbox),
            _ => None,
        }
    }

    /// `FLINT_VSYNC`, the flag's alias: `None` unset (or a value it
    /// refuses, which it says once).
    pub fn from_env() -> Option<Self> {
        flint_core::env::VSYNC.choice().and_then(Self::parse)
    }

    /// The mode the swapchain is configured with.
    pub fn present_mode(self) -> wgpu::PresentMode {
        match self {
            Self::Fifo => wgpu::PresentMode::AutoVsync,
            Self::Off => wgpu::PresentMode::AutoNoVsync,
            Self::Mailbox => wgpu::PresentMode::Mailbox,
        }
    }
}

/// What a window's swapchain is configured with (#1149): the present mode
/// and how many frames the CPU may queue ahead of the display. A host
/// passes it to [`RenderContext::with_present`]; [`RenderContext::new`]
/// takes [`PresentOptions::from_env`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentOptions {
    pub vsync: Vsync,
    /// 1 to 3 (clamped).
    pub frame_latency: u32,
}

impl PresentOptions {
    /// The frame latency with nothing asked: 2.
    pub const DEFAULT_FRAME_LATENCY: u32 = 2;
    /// The most frames the CPU may queue ahead (and the fewest is 1).
    pub const MAX_FRAME_LATENCY: u32 = 3;

    /// A flag's values over their aliases (`FLINT_VSYNC`,
    /// `FLINT_FRAME_LATENCY`), over the defaults: a flag wins.
    pub fn resolve(vsync: Option<Vsync>, frame_latency: Option<u32>) -> Self {
        Self {
            vsync: vsync.or_else(Vsync::from_env).unwrap_or_default(),
            frame_latency: frame_latency
                .or_else(|| flint_core::env::FRAME_LATENCY.u32())
                .unwrap_or(Self::DEFAULT_FRAME_LATENCY)
                .clamp(1, Self::MAX_FRAME_LATENCY),
        }
    }

    /// The environment's choice alone, for a host with no flags.
    pub fn from_env() -> Self {
        Self::resolve(None, None)
    }
}

impl Default for PresentOptions {
    /// Vsync and a frame latency of 2, whatever the environment says.
    fn default() -> Self {
        Self {
            vsync: Vsync::Fifo,
            frame_latency: Self::DEFAULT_FRAME_LATENCY,
        }
    }
}

impl RenderContext {
    /// Create a new render context for a window, presenting as the
    /// environment says ([`PresentOptions::from_env`]): the editors' and
    /// tools' windows, which take no present flags.
    pub async fn new(window: Arc<Window>) -> Result<Self, RenderError> {
        Self::with_present(window, PresentOptions::from_env()).await
    }

    /// Create a new render context for a window presenting with `present`
    /// (`flint play --vsync`, `--frame-latency`).
    pub async fn with_present(
        window: Arc<Window>,
        present: PresentOptions,
    ) -> Result<Self, RenderError> {
        let size = window.inner_size();

        let instance = crate::gpu_select::instance();

        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| RenderError::SurfaceCreation(e.to_string()))?;

        let adapter = crate::gpu_select::request_adapter(&instance, Some(&surface))
            .await
            .ok_or(RenderError::AdapterNotFound)?;

        #[cfg(target_os = "android")]
        let limits = {
            let mut lim = wgpu::Limits::downlevel_defaults();
            // Use the adapter's actual max texture size so the surface can
            // match the device's full screen resolution (often > 2048px).
            let adapter_limits = adapter.limits();
            lim.max_texture_dimension_2d = adapter_limits.max_texture_dimension_2d;
            lim
        };
        #[cfg(not(target_os = "android"))]
        let limits = wgpu::Limits::default();

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Flint Device"),
                // Timestamps where the adapter has them, so the
                // renderer's GPU timing can be turned on (ADR 0108).
                required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
                required_limits: limits,
                memory_hints: Default::default(),
                ..Default::default()
            })
            .await
            .map_err(|e| RenderError::DeviceCreation(e.to_string()))?;

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);
        // Without an sRGB surface (WebGPU canvases), draw through an sRGB
        // view of the plain one, so the encode happens on write exactly as
        // it does on the desktop. Without it the web frame came out darker
        // and more saturated (docs/design/web.md).
        let view_format = surface_format.add_srgb_suffix();

        // Ask for COPY_SRC on the swapchain when the surface allows it so the
        // player can screenshot the presented frame (HUD included). Some
        // backends (notably GL/Android) only permit RENDER_ATTACHMENT.
        let surface_copyable = surface_caps.usages.contains(wgpu::TextureUsages::COPY_SRC);
        let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        if surface_copyable {
            usage |= wgpu::TextureUsages::COPY_SRC;
        } else {
            tracing::warn!(
                "surface does not support COPY_SRC; --screenshot is unavailable on this adapter"
            );
        }

        // The host's choice (`--vsync`, else `FLINT_VSYNC`; see `Vsync`)
        let present_mode = present.vsync.present_mode();
        tracing::info!(
            "surface present modes: {:?}; using {:?}",
            surface_caps.present_modes,
            present_mode
        );

        let config = wgpu::SurfaceConfiguration {
            usage,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            // sRGB, as every wgpu before 30 chose (#945)
            color_space: wgpu::SurfaceColorSpace::Auto,
            alpha_mode: surface_caps.alpha_modes[0],
            // A non-sRGB view of the swapchain for the HUD pass: egui expects to
            // write sRGB-encoded values itself, so blending a translucent panel
            // against the scene happens in display space, not linear space.
            view_formats: if surface_format.remove_srgb_suffix() != surface_format {
                vec![surface_format.remove_srgb_suffix()]
            } else if view_format != surface_format {
                vec![view_format]
            } else {
                vec![]
            },
            desired_maximum_frame_latency: present
                .frame_latency
                .clamp(1, PresentOptions::MAX_FRAME_LATENCY),
        };
        tracing::info!(
            "surface frame latency: {}",
            config.desired_maximum_frame_latency
        );
        surface.configure(&device, &config);

        let (depth_texture, depth_view) = create_depth_texture(&device, &config);

        Ok(Self {
            surface,
            device,
            queue,
            config,
            size,
            depth_texture,
            depth_view,
            instance,
            adapter,
            surface_copyable,
            hud_format: surface_format.remove_srgb_suffix(),
            view_format,
            supported_present_modes: surface_caps.present_modes.clone(),
        })
    }

    /// The view of a swapchain texture that scene passes draw into, in
    /// [`Self::view_format`].
    pub fn frame_view(&self, texture: &wgpu::Texture) -> wgpu::TextureView {
        texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.view_format),
            ..Default::default()
        })
    }

    /// The view of a swapchain texture that an egui pass draws into, in
    /// [`Self::hud_format`]. egui-wgpu picks its shader by the target's
    /// format: into this plain view it writes gamma-encoded colour and the
    /// blend happens in display space, as egui's colours are authored. Into
    /// an sRGB view it writes linear colour and the GPU blends in linear
    /// light, which lifts every translucent dark panel towards grey (#428:
    /// the style-error card read lighter in the scene viewer than in the
    /// player). Build the egui renderer for `hud_format` too.
    pub fn hud_view(&self, texture: &wgpu::Texture) -> wgpu::TextureView {
        texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.hud_format),
            ..Default::default()
        })
    }

    /// Read a surface-sized texture back as tightly packed RGBA8 bytes.
    ///
    /// Blocks on the GPU ([`crate::wait_for_gpu`]). The texture must have been
    /// created with `COPY_SRC` and be in the surface format; Bgra formats are
    /// swizzled to RGBA on the way out. Mirrors `HeadlessContext::read_pixels`.
    pub fn read_surface_rgba(&self, texture: &wgpu::Texture) -> Result<Vec<u8>, RenderError> {
        if !self.surface_copyable {
            return Err(RenderError::BufferReadFailed(
                "surface was not configured with COPY_SRC".into(),
            ));
        }
        let width = texture.width();
        let height = texture.height();
        let bytes_per_pixel = 4u32;
        let unpadded_bytes_per_row = width * bytes_per_pixel;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(align) * align;

        let staging = self.device.create_buffer_counted(&wgpu::BufferDescriptor {
            label: Some("Surface Readback Buffer"),
            size: (padded_bytes_per_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Surface Readback Encoder"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(std::iter::once(encoder.finish()));

        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let _ = crate::wait_for_gpu(&self.device);
        rx.recv()
            .map_err(|e| RenderError::BufferReadFailed(e.to_string()))?
            .map_err(|e| RenderError::BufferReadFailed(e.to_string()))?;

        let data = slice.get_mapped_range().expect("mapped buffer range");
        let mut pixels = Vec::with_capacity((width * height * bytes_per_pixel) as usize);
        for row in 0..height {
            let start = (row * padded_bytes_per_row) as usize;
            pixels.extend_from_slice(&data[start..start + unpadded_bytes_per_row as usize]);
        }
        drop(data);
        staging.unmap();

        let is_bgra = matches!(
            texture.format(),
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        if is_bgra {
            for px in pixels.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
        }
        // The swapchain alpha channel is undefined for opaque surfaces.
        for px in pixels.chunks_exact_mut(4) {
            px[3] = 255;
        }
        Ok(pixels)
    }

    /// Recreate the surface from a new window handle.
    ///
    /// On Android, the native surface is destroyed when the app is paused and a
    /// new one is provided when resumed. This method creates a fresh surface,
    /// reconfigures it with the existing device/format settings, and rebuilds
    /// the depth texture.
    pub fn recreate_surface(&mut self, window: Arc<Window>) -> Result<(), RenderError> {
        let size = window.inner_size();

        let surface = self
            .instance
            .create_surface(window)
            .map_err(|e| RenderError::SurfaceCreation(e.to_string()))?;

        // Reconfigure with current format but new dimensions
        let _note = flint_core::frame_notes::begin(
            flint_core::frame_notes::NoteKind::SurfaceReconfigure,
            || format!("{}x{}, a new surface", size.width, size.height),
        );
        self.config.width = size.width.max(1);
        self.config.height = size.height.max(1);
        surface.configure(&self.device, &self.config);

        let (depth_texture, depth_view) = create_depth_texture(&self.device, &self.config);

        self.surface = surface;
        self.size = size;
        self.depth_texture = depth_texture;
        self.depth_view = depth_view;

        Ok(())
    }

    /// Resize the surface
    pub fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            // The swapchain made again is a likely hitch (#1077)
            let _note = flint_core::frame_notes::begin(
                flint_core::frame_notes::NoteKind::SurfaceReconfigure,
                || format!("{}x{}", new_size.width, new_size.height),
            );
            self.size = new_size;
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);

            let (depth_texture, depth_view) = create_depth_texture(&self.device, &self.config);
            self.depth_texture = depth_texture;
            self.depth_view = depth_view;
        }
    }

    /// True when every presented frame waits for a vblank (Fifo), so frame
    /// times can be paced to the display's refresh (`GameClock`).
    pub fn presents_on_vblank(&self) -> bool {
        matches!(
            self.config.present_mode,
            wgpu::PresentMode::AutoVsync | wgpu::PresentMode::Fifo | wgpu::PresentMode::FifoRelaxed
        )
    }

    /// The present mode the swapchain runs in: `config.present_mode` with
    /// an `Auto` mode resolved as wgpu resolves it. The perf readout names
    /// it, since under Fifo an idle GPU clocks down and its pass times read
    /// high (#380).
    pub fn present_mode_in_use(&self) -> wgpu::PresentMode {
        resolve_present_mode(self.config.present_mode, &self.supported_present_modes)
    }

    /// The depth view, for
    /// [`SceneRenderer::encode_to`](crate::SceneRenderer::encode_to).
    pub fn depth_target(&self) -> crate::bind_cache::DepthTarget<'_> {
        crate::bind_cache::DepthTarget::new(&self.depth_view)
    }

    /// Get aspect ratio
    pub fn aspect_ratio(&self) -> f32 {
        self.size.width as f32 / self.size.height as f32
    }
}

fn create_depth_texture(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
) -> (wgpu::Texture, wgpu::TextureView) {
    let size = wgpu::Extent3d {
        width: config.width.max(1),
        height: config.height.max(1),
        depth_or_array_layers: 1,
    };

    let desc = wgpu::TextureDescriptor {
        label: Some("Depth Texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        // COPY_SRC: a frame capture reads it as its pass left it (#1086)
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    };

    let texture = device.create_texture_counted(&desc);
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    (texture, view)
}

#[cfg(test)]
mod tests {
    use super::{resolve_present_mode, PresentOptions, Vsync};

    #[test]
    fn vsync_takes_every_word_flint_vsync_does() {
        assert_eq!(Vsync::WORDS, flint_core::env::VSYNC_WORDS);
        for word in Vsync::WORDS {
            assert!(Vsync::parse(word).is_some(), "{word}");
            assert!(flint_core::env::VSYNC.parse(word).is_ok(), "{word}");
        }
        assert_eq!(Vsync::parse("Mailbox"), Some(Vsync::Mailbox));
        assert_eq!(Vsync::parse("immediate"), Some(Vsync::Off));
        assert_eq!(Vsync::parse("ON"), Some(Vsync::Fifo));
        assert_eq!(Vsync::parse("fasle"), None);
        use wgpu::PresentMode as P;
        assert_eq!(Vsync::Fifo.present_mode(), P::AutoVsync);
        assert_eq!(Vsync::Off.present_mode(), P::AutoNoVsync);
        assert_eq!(Vsync::Mailbox.present_mode(), P::Mailbox);
    }

    /// A flag wins over its alias, which wins over the default. The test
    /// asks nothing of the environment it can't assume: with FLINT_VSYNC or
    /// FLINT_FRAME_LATENCY set in the shell only the flag rows are checked.
    #[test]
    fn a_flag_wins_over_the_environment() {
        let both = PresentOptions::resolve(Some(Vsync::Mailbox), Some(1));
        assert_eq!(both.vsync, Vsync::Mailbox);
        assert_eq!(both.frame_latency, 1);
        assert_eq!(PresentOptions::resolve(None, Some(9)).frame_latency, 3);
        assert_eq!(PresentOptions::resolve(None, Some(0)).frame_latency, 1);
        if !flint_core::env::VSYNC.is_set() && !flint_core::env::FRAME_LATENCY.is_set() {
            assert_eq!(PresentOptions::from_env(), PresentOptions::default());
        }
    }
    use wgpu::PresentMode as P;

    #[test]
    fn auto_modes_resolve_as_wgpu_picks_them() {
        // Windows DX12/Vulkan offer all three; vsync is Fifo there.
        let all = [P::Fifo, P::Mailbox, P::Immediate];
        assert_eq!(resolve_present_mode(P::AutoVsync, &all), P::Fifo);
        assert_eq!(resolve_present_mode(P::AutoNoVsync, &all), P::Immediate);
        // Relaxed Fifo is wgpu's first choice for vsync where it exists.
        let relaxed = [P::Fifo, P::FifoRelaxed];
        assert_eq!(resolve_present_mode(P::AutoVsync, &relaxed), P::FifoRelaxed);
        // No Immediate: unlocked falls to Mailbox, then to Fifo.
        assert_eq!(
            resolve_present_mode(P::AutoNoVsync, &[P::Fifo, P::Mailbox]),
            P::Mailbox
        );
        assert_eq!(resolve_present_mode(P::AutoNoVsync, &[P::Fifo]), P::Fifo);
        // A named mode is used as asked.
        assert_eq!(resolve_present_mode(P::Mailbox, &all), P::Mailbox);
    }
}
