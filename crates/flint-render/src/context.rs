//! wgpu render context setup

use std::sync::Arc;
use thiserror::Error;
use winit::window::Window;

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("Failed to create surface: {0}")]
    SurfaceCreation(String),
    #[error("Failed to get adapter")]
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
}

impl RenderContext {
    /// Create a new render context for a window
    pub async fn new(window: Arc<Window>) -> Result<Self, RenderError> {
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
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("Flint Device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: limits,
                    memory_hints: Default::default(),
                },
                None,
            )
            .await
            .map_err(|e| RenderError::DeviceCreation(e.to_string()))?;

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);

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

        // FLINT_VSYNC picks the present mode: unset/"1" = vsync (Fifo),
        // "0"/"off"/"immediate" = unlocked (real frame times for headless
        // perf runs), "mailbox" = uncapped rendering with the newest frame
        // shown at each refresh (no tearing, no Fifo stall). On this Optimus
        // laptop Fifo has been seen pacing at ~7 Hz against a virtual
        // display; mailbox sidesteps it.
        let present_mode = match std::env::var("FLINT_VSYNC")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "0" | "off" | "immediate" => wgpu::PresentMode::AutoNoVsync,
            "mailbox" => wgpu::PresentMode::Mailbox,
            _ => wgpu::PresentMode::AutoVsync,
        };
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
            alpha_mode: surface_caps.alpha_modes[0],
            // A non-sRGB view of the swapchain for the HUD pass: egui expects to
            // write sRGB-encoded values itself, so blending a translucent panel
            // against the scene happens in display space, not linear space.
            view_formats: if surface_format.remove_srgb_suffix() != surface_format {
                vec![surface_format.remove_srgb_suffix()]
            } else {
                vec![]
            },
            desired_maximum_frame_latency: 2,
        };
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
        })
    }

    /// Read a surface-sized texture back as tightly packed RGBA8 bytes.
    ///
    /// Blocks on the GPU (`Maintain::Wait`). The texture must have been
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

        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
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
            wgpu::ImageCopyTexture {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &staging,
                layout: wgpu::ImageDataLayout {
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
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .map_err(|e| RenderError::BufferReadFailed(e.to_string()))?
            .map_err(|e| RenderError::BufferReadFailed(e.to_string()))?;

        let data = slice.get_mapped_range();
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
            self.size = new_size;
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);

            let (depth_texture, depth_view) = create_depth_texture(&self.device, &self.config);
            self.depth_texture = depth_texture;
            self.depth_view = depth_view;
        }
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
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    };

    let texture = device.create_texture(&desc);
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    (texture, view)
}
