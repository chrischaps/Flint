//! The GPU pass a user style draws with, and its images (moved from
//! `custom.rs`, #742).

use super::*;
use crate::gpu_counts::{CountedDevice, CountedQueue};

// ---------------------------------------------------------------------------
// The pass
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(super) struct FlintFrameUniforms {
    pub(super) inv_view_proj: [[f32; 4]; 4],
    pub(super) resolution: [f32; 2],
    pub(super) texel: [f32; 2],
    pub(super) depth_size: [f32; 2],
    pub(super) time: f32,
    pub(super) frame: u32,
    pub(super) near: f32,
    pub(super) far: f32,
    pub(super) srgb: f32,
    pub(super) pixels_per_point: f32,
    pub(super) camera_pos: [f32; 3],
    pub(super) mask_id: u32,
    pub(super) pass_size: [f32; 2],
    pub(super) history: f32,
    pub(super) pass_index: u32,
}

pub(super) struct Build {
    /// The user's pipeline, or `None` while it has never compiled.
    pub(super) pipeline: Option<wgpu::RenderPipeline>,
    /// A build waiting for the compiler's verdict before it draws (#415).
    pub(super) pending: Option<Pending>,
    /// The WGSL file's modified time when last read.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) modified: Option<std::time::SystemTime>,
    /// Frames since the file was last looked at.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) since_poll: u32,
    /// The [`poll_files_now`] count at that look.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) polled_at: u32,
}

/// A pipeline made but not yet trusted: it draws once both of the GPU's
/// answers say it compiled (#415). Natively both are ready as soon as it
/// is made; on the web they are browser promises, settled a frame or two
/// later, and polled each frame until then (an invalid pipeline in a pass
/// would lose the whole frame, so the last good build draws meanwhile).
pub(super) struct Pending {
    pub(super) pipeline: wgpu::RenderPipeline,
    pub(super) composed: Composed,
    /// The shader compiler's messages, with composed-module lines.
    pub(super) info: Settling<wgpu::CompilationInfo>,
    /// What the validation error scope caught (the shader, or the
    /// pipeline against its layout).
    pub(super) scope: Settling<Option<wgpu::Error>>,
}

/// A future polled by hand once a frame, never awaited: nothing in a frame
/// may block on the browser.
pub(super) enum Settling<T> {
    Waiting(std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>),
    Done(T),
}

impl<T> Settling<T> {
    pub(super) fn new(future: impl std::future::Future<Output = T> + Send + 'static) -> Self {
        Settling::Waiting(Box::pin(future))
    }

    /// Poll once if still waiting; whether it has settled.
    pub(super) fn poll(&mut self) -> bool {
        if let Settling::Waiting(future) = self {
            let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
            match future.as_mut().poll(&mut cx) {
                std::task::Poll::Ready(value) => *self = Settling::Done(value),
                std::task::Poll::Pending => return false,
            }
        }
        true
    }

    pub(super) fn take(self) -> Option<T> {
        match self {
            Settling::Done(value) => Some(value),
            Settling::Waiting(_) => None,
        }
    }
}

/// One pass of a user style on the GPU (#412).
pub(super) struct PassGpu {
    /// What it writes: its target's format, or the stage's.
    pub(super) format: wgpu::TextureFormat,
    /// Group 3 of a pass that reads targets: the style's images, then the
    /// targets it reads. A pass that reads none binds the images' own group.
    pub(super) targets_bgl: Option<wgpu::BindGroupLayout>,
    pub(super) layout: wgpu::PipelineLayout,
    /// Its own frame uniform: `pass_size` and `pass_index` differ by pass,
    /// and a buffer holds one value a frame.
    pub(super) frame_buffer: wgpu::Buffer,
    pub(super) build: Mutex<Build>,
    pub(super) error: Mutex<Option<String>>,
}

/// What the last frame drew, for history targets (#277's rule): trusted
/// only on the frame after, in the same epoch, into the same textures.
#[derive(Default)]
pub(super) struct History {
    /// Which of each history target's two textures this frame writes.
    pub(super) parity: usize,
    pub(super) last: Option<LastFrame>,
}

pub(super) struct LastFrame {
    pub(super) frame: u64,
    pub(super) epoch: u64,
    /// Every target texture it used; held, so an address can't be reused.
    /// A pair a target, the same texture twice for one with no history.
    pub(super) textures: Vec<Arc<super::super::StyleTexture>>,
    /// Whether it read its history targets' last frame (#1136).
    pub(super) trusted: bool,
    /// What a frame capture keys the history on (#1136).
    pub(super) key: serde_json::Value,
}

pub(super) struct CustomPass {
    pub(super) loaded: Arc<Loaded>,
    pub(super) frame_bgl: wgpu::BindGroupLayout,
    pub(super) input_bgl: wgpu::BindGroupLayout,
    pub(super) extra_bgl: Option<wgpu::BindGroupLayout>,
    /// The settings table, shared by every pass (#740: the framework's).
    pub(super) table: Mutex<VariantTable>,
    /// The passes in order (one for a style with a top-level `shader`).
    pub(super) passes: Vec<PassGpu>,
    /// Draws the input unchanged, while a pass can't.
    pub(super) passthrough: wgpu::RenderPipeline,
    /// The style's images (#413), group 3.
    pub(super) images: Option<Mutex<ImageSet>>,
    /// Group 2 when a style binds images or reads targets but reads none of
    /// the optional buffers: an empty group keeps group 3 at 3.
    pub(super) empty: Option<wgpu::BindGroup>,
    /// The history targets' parity and what the last frame drew (#412).
    pub(super) history: Mutex<History>,
    /// History taken from a frame capture (#1136), until the next frame.
    pub(super) imported: super::super::history::PendingHistory,
}

pub(super) fn create(d: &StyleDevice) -> Box<dyn StylePass> {
    Box::new(build(d))
}

pub(super) fn build(d: &StyleDevice) -> CustomPass {
    let loaded = d
        .style
        .loaded()
        .cloned()
        .expect("a user style's descriptor comes with what was loaded for it");
    let device = d.device;
    let source = &loaded.source;
    let frame_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("User Style Frame BGL"),
        entries: &[uniform_entry(0), storage_entry(1), storage_entry(2)],
    });
    let input_bgl = kit::sampled_layout(device, "User Style Input BGL");
    let images = ImageSet::new(device, source);
    let reads_targets = source.passes.iter().any(|p| !p.reads.is_empty());
    let extra_bgl = (source.inputs.any() || images.is_some() || reads_targets).then(|| {
        let mut entries = Vec::new();
        if source.inputs.depth {
            entries.push(texture_entry(0, wgpu::TextureSampleType::Depth));
        }
        if source.inputs.mask {
            entries.push(texture_entry(1, wgpu::TextureSampleType::Uint));
        }
        if source.inputs.edges {
            entries.push(texture_entry(
                2,
                wgpu::TextureSampleType::Float { filterable: true },
            ));
        }
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("User Style Depth/Mask BGL"),
            entries: &entries,
        })
    });
    let empty = extra_bgl
        .as_ref()
        .filter(|_| !source.inputs.any())
        .map(|bgl| {
            device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                label: Some("User Style Empty BG"),
                layout: bgl,
                entries: &[],
            })
        });
    let passes: Vec<PassGpu> = source
        .passes
        .iter()
        .map(|p| {
            // The images first (bindings 0..n, as #413 laid them out), then
            // the targets this pass reads
            let targets_bgl = (!p.reads.is_empty()).then(|| {
                let image_entries = (0..source.images.len()).map(|i| {
                    texture_entry(
                        i as u32,
                        wgpu::TextureSampleType::Float { filterable: true },
                    )
                });
                let target_entries = p.reads.iter().enumerate().map(|(b, r)| {
                    texture_entry(
                        (source.images.len() + b) as u32,
                        wgpu::TextureSampleType::Float {
                            filterable: source.targets[r.target].format.filterable(),
                        },
                    )
                });
                let entries: Vec<wgpu::BindGroupLayoutEntry> =
                    image_entries.chain(target_entries).collect();
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("User Style Images + Targets BGL"),
                    entries: &entries,
                })
            });
            let mut layouts = vec![&frame_bgl, &input_bgl];
            let group3 = targets_bgl.as_ref().or(images.as_ref().map(|i| &i.layout));
            if let Some(extra) = &extra_bgl {
                if source.inputs.any() || group3.is_some() {
                    layouts.push(extra);
                }
            }
            if let Some(g) = group3 {
                layouts.push(g);
            }
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("User Style Layout"),
                bind_group_layouts: &layouts.iter().copied().map(Some).collect::<Vec<_>>(),
                immediate_size: 0,
            });
            PassGpu {
                format: p
                    .target
                    .map_or(d.format, |t| source.targets[t].format.wgpu()),
                targets_bgl,
                layout,
                frame_buffer: kit::uniform_buffer::<FlintFrameUniforms>(device, "User Style Frame"),
                build: Mutex::new(Build {
                    pipeline: None,
                    pending: None,
                    #[cfg(not(target_arch = "wasm32"))]
                    modified: None,
                    #[cfg(not(target_arch = "wasm32"))]
                    since_poll: 0,
                    #[cfg(not(target_arch = "wasm32"))]
                    polled_at: polls_asked(),
                }),
                error: Mutex::new(None),
            }
        })
        .collect();
    // The passthrough is the last pass's prelude with no inputs, images or
    // targets: always compiles
    let last = source.passes[source.last_pass()].clone();
    let bare = StyleSource {
        inputs: Inputs::default(),
        images: Vec::new(),
        passes: vec![PassSource {
            reads: Vec::new(),
            target: None,
            ..last
        }],
        targets: Vec::new(),
        ..source.clone()
    };
    let passthrough_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("User Style Passthrough Layout"),
        bind_group_layouts: &[Some(&frame_bgl), Some(&input_bgl)],
        immediate_size: 0,
    });
    let (_, passthrough) = pipeline(
        device,
        &passthrough_layout,
        &compose(&bare, PASSTHROUGH).text,
        d.format,
    );
    let pass = CustomPass {
        table: Mutex::new(VariantTable::new(
            device,
            "User Style",
            params_size(source.uniform_fields()),
        )),
        frame_bgl,
        input_bgl,
        extra_bgl,
        passes,
        passthrough,
        images: images.map(Mutex::new),
        empty,
        history: Mutex::new(History::default()),
        imported: Default::default(),
        loaded,
    };
    for i in 0..pass.passes.len() {
        pass.rebuild(device, i);
    }
    pass
}

/// A style's `reach` (points) in pixels at `pixels_per_point`, rounded
/// out, plus one for the filtered read of a pixel at its edge (#538).
pub(super) fn reach_pixels(reach: f32, pixels_per_point: f32) -> u32 {
    let ppp = if pixels_per_point.is_finite() && pixels_per_point > 0.0 {
        pixels_per_point
    } else {
        1.0
    };
    (reach.max(0.0) * ppp).ceil() as u32 + 1
}

pub(super) fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

pub(super) fn texture_entry(
    binding: u32,
    sample_type: wgpu::TextureSampleType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

pub(super) fn pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    text: &str,
    format: wgpu::TextureFormat,
) -> (wgpu::ShaderModule, wgpu::RenderPipeline) {
    // User WGSL, so it keeps every runtime check wgpu has (loop bounding
    // included, after #945) and never goes through `TrustedShader` (#944).
    // The passthrough is composed from the user's source too.
    crate::gpu_counts::count_shader_module();
    #[allow(clippy::disallowed_methods)]
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("User Style Shader"),
        source: wgpu::ShaderSource::Wgsl(text.into()),
    });
    let pipeline = device.create_render_pipeline_counted(&wgpu::RenderPipelineDescriptor {
        label: Some("User Style Pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    (module, pipeline)
}

/// How many frames pass between looks at the WGSL file's modified time:
/// about four times a second at 60 fps, and one `stat` each.
#[cfg(not(target_arch = "wasm32"))]
pub(super) const POLL_FRAMES: u32 = 15;

/// How many times [`poll_files_now`] was called.
#[cfg(not(target_arch = "wasm32"))]
pub(super) static POLLS_ASKED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn polls_asked() -> u32 {
    POLLS_ASKED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Have every user style's pass look at its WGSL and image files at its
/// next draw, whatever its frame count (#432). A window draws sixty frames
/// a second and a pass looks every [`POLL_FRAMES`]; a host that draws one
/// frame a change (`flint render --watch`) calls this before the frame, so
/// the save it was woken for is the one it draws. Natively; on the web the
/// files never change and this does nothing.
pub fn poll_files_now() {
    #[cfg(not(target_arch = "wasm32"))]
    POLLS_ASKED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Images (#413)
// ---------------------------------------------------------------------------

/// One image binding and what it holds now.
pub(super) struct ImageSlot {
    pub(super) source: ImageSource,
    /// An image option's choices and default (`None` for a fixed image).
    pub(super) option: Option<OptionSource>,
    /// The file loaded now (relative to the style's folder) and its view.
    pub(super) loaded: Option<(String, wgpu::TextureView)>,
    /// That file's modified time when read, for the poll.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) modified: Option<std::time::SystemTime>,
    /// A file that failed, with its modified time then: not tried again
    /// until it changes, and its error.
    pub(super) failed: Option<(String, Option<std::time::SystemTime>, String)>,
}

/// A style's images: decoded through the vfs when first drawn or chosen,
/// read again when their files change (natively), bound in group 3.
pub(super) struct ImageSet {
    pub(super) dir: PathBuf,
    pub(super) slots: Vec<ImageSlot>,
    pub(super) layout: wgpu::BindGroupLayout,
    /// Made again whenever a slot's view changes.
    pub(super) bind: Option<wgpu::BindGroup>,
    /// Bumped whenever a slot's view changes: the key of the bind groups a
    /// pass that reads targets makes over the images and its targets (#412).
    pub(super) generation: u64,
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) since_poll: u32,
    /// The [`poll_files_now`] count at the last look.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) polled_at: u32,
}

/// What one [`ImageSet::update`] did.
#[derive(Debug, Default, PartialEq)]
pub(super) struct ImageUpdate {
    /// Files read (first loads, new choices and reloads).
    pub(super) loaded: Vec<String>,
    /// Every slot has an image to bind.
    pub(super) ready: bool,
    /// The slots' errors, one line each.
    pub(super) error: Option<String>,
}

impl ImageSet {
    pub(super) fn new(device: &wgpu::Device, source: &StyleSource) -> Option<Self> {
        if source.images.is_empty() {
            return None;
        }
        let entries: Vec<wgpu::BindGroupLayoutEntry> = (0..source.images.len())
            .map(|i| {
                texture_entry(
                    i as u32,
                    wgpu::TextureSampleType::Float { filterable: true },
                )
            })
            .collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("User Style Images BGL"),
            entries: &entries,
        });
        let slots = source
            .images
            .iter()
            .map(|image| ImageSlot {
                option: source
                    .options
                    .iter()
                    .find(|(n, _)| *n == image.name)
                    .map(|(_, o)| o.clone()),
                source: image.clone(),
                loaded: None,
                #[cfg(not(target_arch = "wasm32"))]
                modified: None,
                failed: None,
            })
            .collect();
        Some(Self {
            dir: source.dir().to_path_buf(),
            slots,
            layout,
            bind: None,
            generation: 0,
            #[cfg(not(target_arch = "wasm32"))]
            since_poll: 0,
            #[cfg(not(target_arch = "wasm32"))]
            polled_at: polls_asked(),
        })
    }

    /// Load what each slot should hold: its file, or for an option the
    /// file `choose(name)` names (its default when that isn't a choice).
    /// With `poll`, natively, a file whose modified time moved is read
    /// again. A file that fails keeps the slot's last good image.
    pub(super) fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        choose: &dyn Fn(&str) -> String,
        poll: bool,
    ) -> ImageUpdate {
        let mut update = ImageUpdate::default();
        for slot in &mut self.slots {
            let want = match (&slot.source.file, &slot.option) {
                (Some(file), _) => file.clone(),
                (None, Some(option)) => {
                    let chosen = choose(&slot.source.name);
                    if option.choices.contains(&chosen) {
                        chosen
                    } else {
                        option.default.clone()
                    }
                }
                (None, None) => continue,
            };
            let path = self.dir.join(&want);
            #[cfg(not(target_arch = "wasm32"))]
            let now = modified(&path);
            #[cfg(target_arch = "wasm32")]
            let now: Option<std::time::SystemTime> = None;
            let is_loaded = slot.loaded.as_ref().is_some_and(|(f, _)| *f == want);
            #[cfg(not(target_arch = "wasm32"))]
            let stale = is_loaded && poll && now.is_some() && now != slot.modified;
            #[cfg(target_arch = "wasm32")]
            let stale = {
                let _ = poll;
                false
            };
            let tried = slot
                .failed
                .as_ref()
                .is_some_and(|(f, at, _)| *f == want && *at == now);
            if is_loaded && !stale {
                // Back on a file that reads: an old choice's failure is past
                if slot.failed.as_ref().is_some_and(|(f, _, _)| *f != want) {
                    slot.failed = None;
                }
                continue;
            }
            if tried {
                continue;
            }
            let (srgb, mips) = (slot.source.srgb, slot.source.mips);
            match upload_image(device, queue, &path, srgb, mips, &slot.source.name) {
                Ok(view) => {
                    slot.loaded = Some((want.clone(), view));
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        slot.modified = now;
                    }
                    slot.failed = None;
                    self.bind = None;
                    self.generation += 1;
                    update.loaded.push(want);
                }
                Err(e) => slot.failed = Some((want, now, e)),
            }
        }
        // A slot showing another file than it was asked for (a new choice
        // that failed) keeps drawing its last good one; the error says so
        let errors: Vec<&str> = self
            .slots
            .iter()
            .filter_map(|s| s.failed.as_ref().map(|(_, _, e)| e.as_str()))
            .collect();
        update.error = (!errors.is_empty()).then(|| errors.join("\n"));
        update.ready = self.slots.iter().all(|s| s.loaded.is_some());
        update
    }

    /// The group 3 bind group over every slot's view (all loaded).
    pub(super) fn bind_group(&mut self, device: &wgpu::Device) -> &wgpu::BindGroup {
        let slots = &self.slots;
        let layout = &self.layout;
        self.bind.get_or_insert_with(|| {
            let entries: Vec<wgpu::BindGroupEntry> = slots
                .iter()
                .enumerate()
                .map(|(i, s)| wgpu::BindGroupEntry {
                    binding: i as u32,
                    resource: wgpu::BindingResource::TextureView(
                        &s.loaded.as_ref().expect("every slot is loaded").1,
                    ),
                })
                .collect();
            device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                label: Some("User Style Images BG"),
                layout,
                entries: &entries,
            })
        })
    }
}

/// Decode the image at `path` (through the vfs) into a texture of its own,
/// 8 bits a channel: `Rgba8UnormSrgb` with `srgb` (read back as linear
/// light), else `Rgba8Unorm` (the stored values). One mip, or with `mips`
/// the whole chain down to 1x1 (#558), each level a box filter of the one
/// above, averaged in linear light for an sRGB image and in the stored
/// values otherwise, as the texture cache's are.
pub(super) fn upload_image(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    path: &Path,
    srgb: bool,
    mips: bool,
    name: &str,
) -> Result<wgpu::TextureView, String> {
    upload_texture(device, queue, path, srgb, mips, name)
        .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()))
}

/// [`upload_image`]'s texture.
pub(super) fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    path: &Path,
    srgb: bool,
    mips: bool,
    name: &str,
) -> Result<wgpu::Texture, String> {
    let image = crate::open_image(path)
        .map_err(|e| format!("{}: {e}", shown(path)))?
        .to_rgba8();
    let (width, height) = image.dimensions();
    let max = device.limits().max_texture_dimension_2d;
    if width == 0 || height == 0 || width > max || height > max {
        return Err(format!(
            "{}: {width}x{height} is not a size this GPU binds (1..{max} a side)",
            shown(path)
        ));
    }
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    // Level 0 is the file's bytes either way, so an image with mips reads
    // texel for texel as one without wherever it is drawn at its size
    let (data, mip_level_count) = if mips {
        crate::texture_cache::mip_chain(image.as_raw(), width, height, srgb)
    } else {
        (image.into_raw(), 1)
    };
    let texture = device.create_texture_with_data_counted(
        queue,
        &wgpu::TextureDescriptor {
            label: Some(&format!("User Style Image {name}")),
            size,
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: if srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            // COPY_SRC for the test that reads a level back
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &data,
    );
    Ok(texture)
}

/// Clear `view` to transparent black: a history target whose past isn't
/// this picture's reads as nothing (#412).
pub(super) fn clear(encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
    let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("User Style History Clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
}

impl CustomPass {
    /// Record pass `pass`'s error (or that it builds), and the style's
    /// error as all of its passes' together.
    pub(super) fn set_error(&self, pass: usize, error: Option<String>) {
        let name = &self.loaded.source.name;
        {
            let mut slot = self.passes[pass]
                .error
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(e) = &error {
                if slot.as_ref() != Some(e) {
                    tracing::warn!("style '{name}': {e}");
                }
            } else if slot.is_some() {
                if self.passes.len() > 1 {
                    tracing::info!(
                        "style '{name}': pass '{}' compiles again",
                        self.loaded.source.passes[pass].name
                    );
                } else {
                    tracing::info!("style '{name}' compiles again");
                }
            }
            *slot = error;
        }
        let all: Vec<Option<String>> = self
            .passes
            .iter()
            .map(|p| p.error.lock().unwrap_or_else(|e| e.into_inner()).clone())
            .collect();
        *self.loaded.error.lock().unwrap_or_else(|e| e.into_inner()) = join_errors(&all);
    }

    /// Read, check and build pass `i`'s shader; on failure keep what was
    /// drawing (its last good build, or the passthrough).
    pub(super) fn rebuild(&self, device: &wgpu::Device, i: usize) {
        let source = &self.loaded.source;
        let path = &source.passes[i].wgsl_path;
        let gpu = &self.passes[i];
        #[cfg(not(target_arch = "wasm32"))]
        let modified = modified(path);
        let user = match flint_core::vfs::read_to_string(path) {
            Ok(user) => user,
            Err(e) => {
                self.set_error(i, Some(format!("{}: {e}", shown(path))));
                return;
            }
        };
        let mut build = gpu.build.lock().unwrap_or_else(|e| e.into_inner());
        #[cfg(not(target_arch = "wasm32"))]
        {
            build.modified = modified;
        }
        let report = match check_pass(source, i, &user) {
            Ok(report) => report,
            Err(e) => {
                drop(build);
                self.set_error(i, Some(e));
                return;
            }
        };
        // naga has passed it natively; the compiler's messages and the error
        // scope catch what the backend's own compiler still refuses (and, on
        // the web, everything). Neither is awaited: the build waits as
        // `pending` until both settle (#415)
        //
        // The guarded module when it has loops (#411), else the text as
        // composed. naga wrote the guarded one, so its lines aren't the
        // user's: a compiler message on it says "in the generated code"
        let composed = match report.guarded {
            Some(text) => Composed {
                text,
                prelude_lines: 0,
                user_lines: 0,
            },
            None => compose_pass(source, i, &user),
        };
        let guard = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let (module, made) = pipeline(device, &gpu.layout, &composed.text, gpu.format);
        let scope = Settling::new(guard.pop());
        let info = Settling::new(module.get_compilation_info());
        build.pending = Some(Pending {
            pipeline: made,
            composed,
            info,
            scope,
        });
        self.settle(i, &mut build);
    }

    /// Poll pass `i`'s pending build; once the GPU has answered, draw it,
    /// or keep what was drawing and say why. Natively the answer is in on
    /// the first poll, inside [`Self::rebuild`].
    pub(super) fn settle(&self, i: usize, build: &mut Build) {
        let Some(pending) = build.pending.as_mut() else {
            return;
        };
        let info_in = pending.info.poll();
        if !(pending.scope.poll() && info_in) {
            return;
        }
        let Some(pending) = build.pending.take() else {
            return;
        };
        let source = &self.loaded.source;
        let info = pending.info.take();
        let refused = pending.scope.take().flatten();
        let compiled = info
            .as_ref()
            .and_then(|info| compiler_error_pass(source, i, &pending.composed, info));
        match (compiled, refused) {
            (Some(e), _) => self.set_error(i, Some(e)),
            (None, Some(e)) => self.set_error(
                i,
                Some(format!(
                    "{}: the GPU refused it: {e}",
                    shown(&source.passes[i].wgsl_path)
                )),
            ),
            (None, None) => {
                build.pipeline = Some(pending.pipeline);
                self.set_error(i, None);
            }
        }
    }

    /// Bring the images up to date for this frame (#413): the scene's
    /// choices, and natively a look at the files' modified times every
    /// [`POLL_FRAMES`]. Records their error, logged when it changes.
    /// Whether every image has something to bind.
    pub(super) fn update_images(&self, frame: &StyleFrame<'_>) -> bool {
        let Some(images) = &self.images else {
            return true;
        };
        let mut images = images.lock().unwrap_or_else(|e| e.into_inner());
        #[cfg(not(target_arch = "wasm32"))]
        let poll = {
            images.since_poll += 1;
            let asked = polls_asked();
            let due = images.since_poll >= POLL_FRAMES || images.polled_at != asked;
            if due {
                images.since_poll = 0;
                images.polled_at = asked;
            }
            due
        };
        #[cfg(target_arch = "wasm32")]
        let poll = false;
        let update = images.update(
            frame.device,
            frame.queue,
            &|name| frame.option(name).to_string(),
            poll,
        );
        let name = &self.loaded.source.name;
        for file in &update.loaded {
            tracing::debug!("style '{name}': image {file} read");
        }
        let mut slot = self
            .loaded
            .image_error
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *slot != update.error {
            match &update.error {
                Some(e) => tracing::warn!("style '{name}': {e}"),
                None => tracing::info!("style '{name}': its images read again"),
            }
            *slot = update.error;
        }
        update.ready
    }

    /// Rebuild each pass whose WGSL file changed on disk (native only).
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn poll(&self, device: &wgpu::Device) {
        for (i, pass) in self.loaded.source.passes.iter().enumerate() {
            let changed = {
                let mut build = self.passes[i]
                    .build
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                build.since_poll += 1;
                let asked = polls_asked();
                if build.since_poll < POLL_FRAMES && build.polled_at == asked {
                    continue;
                }
                build.since_poll = 0;
                build.polled_at = asked;
                let now = modified(&pass.wgsl_path);
                now.is_some() && now != build.modified
            };
            if changed {
                self.rebuild(device, i);
            }
        }
    }

    /// The frame uniform of pass `i`, drawing `size`.
    pub(super) fn uniforms(
        &self,
        frame: &StyleFrame<'_>,
        i: usize,
        size: [u32; 2],
        history: bool,
    ) -> FlintFrameUniforms {
        let camera = frame.camera;
        FlintFrameUniforms {
            inv_view_proj: camera.inv_view_proj,
            resolution: [frame.width as f32, frame.height as f32],
            texel: frame.texel_size(),
            depth_size: [frame.depth_size[0] as f32, frame.depth_size[1] as f32],
            time: frame.time,
            frame: frame.frame as u32,
            near: camera.near,
            far: camera.far,
            srgb: if frame.srgb() { 1.0 } else { 0.0 },
            pixels_per_point: frame.pixels_per_point,
            camera_pos: camera.position,
            mask_id: u32::from(
                frame
                    .style_id(self.loaded.source.name.as_str())
                    .unwrap_or(0),
            ),
            pass_size: [size[0] as f32, size[1] as f32],
            history: if history { 1.0 } else { 0.0 },
            pass_index: i as u32,
        }
    }

    /// Pass `i`'s group 0: its frame uniform and the shared settings table.
    pub(super) fn frame_bind(&self, frame: &StyleFrame<'_>, i: usize) -> Arc<wgpu::BindGroup> {
        let name = self.loaded.names.passes[i].frame;
        let table = self.table.lock().unwrap_or_else(|e| e.into_inner());
        let [rows, slots] = table.entries(1);
        frame.bind(name, table.generation(), || {
            frame
                .device
                .create_bind_group_counted(&wgpu::BindGroupDescriptor {
                    label: Some(name),
                    layout: &self.frame_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: self.passes[i].frame_buffer.as_entire_binding(),
                        },
                        rows,
                        slots,
                    ],
                })
        })
    }

    /// The input unchanged, through the last pass's frame uniform.
    pub(super) fn draw_passthrough(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input_bg: &wgpu::BindGroup,
        output: &wgpu::TextureView,
    ) {
        let last = self.passes.len() - 1;
        let uniforms = self.uniforms(frame, last, [frame.width, frame.height], false);
        frame.queue.write_buffer_counted(
            &self.passes[last].frame_buffer,
            0,
            bytemuck::cast_slice(&[uniforms]),
        );
        let frame_bg = self.frame_bind(frame, last);
        kit::fullscreen_pass(
            encoder,
            self.loaded.names.passes[last].pass,
            output,
            &self.passthrough,
            &[&frame_bg, input_bg],
        );
    }
}

impl StylePass for CustomPass {
    fn keeps_history(&self) -> bool {
        self.loaded.source.has_history()
    }

    fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    ) {
        #[cfg(not(target_arch = "wasm32"))]
        self.poll(frame.device);

        let source = &self.loaded.source;
        let names = &self.loaded.names;
        // The settings table (#414): row 0 the scene's, then one a variant
        // Each row is the params, then the options (#413) as u32 indices
        // written as their bits
        let row = |param: &dyn Fn(&str) -> f32, option: &dyn Fn(&str) -> u32| -> Vec<f32> {
            source
                .params
                .iter()
                .map(|(name, p)| param(name).clamp(p.min, p.max))
                .chain(
                    source
                        .options
                        .iter()
                        .map(|(name, _)| f32::from_bits(option(name))),
                )
                .collect()
        };
        let index = |name: &str, value: &str| {
            source
                .options
                .iter()
                .find(|(n, _)| n == name)
                .map_or(0, |(_, o)| o.index(value))
        };
        // (#740: every variant gets a row and every mask id a slot)
        let mut rows = vec![row(&|n| frame.param(n), &|n| index(n, frame.option(n)))];
        let mut slots = Vec::new();
        if source.inputs.mask {
            slots.resize(frame.style_ids.len() + 1, 0u32);
            for (id, v) in frame.variants() {
                if v.off {
                    continue; // an opt-out reads as no entity of this style
                }
                rows.push(row(&|n| frame.variant_param(v, n), &|n| {
                    index(n, frame.variant_option(v, n))
                }));
                slots[id as usize] = rows.len() as u32;
            }
        }
        let words = row_words(source.uniform_fields(), &rows);
        let images_ready = self.update_images(frame);
        self.table
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .upload_bytes(
                frame.device,
                frame.queue,
                bytemuck::cast_slice(&words),
                &slots,
            );
        let input_bg = frame.sampled_input(names.input, &self.input_bgl, input);

        // Every pass must have built once (a build the browser hasn't
        // answered for yet settles here, #415): a chain missing a link draws
        // nothing meaningful, so the style passes its input through
        let mut builds: Vec<std::sync::MutexGuard<'_, Build>> = self
            .passes
            .iter()
            .map(|p| p.build.lock().unwrap_or_else(|e| e.into_inner()))
            .collect();
        for (i, build) in builds.iter_mut().enumerate() {
            self.settle(i, build);
        }
        if builds.iter().any(|b| b.pipeline.is_none()) {
            drop(builds);
            self.draw_passthrough(frame, encoder, &input_bg, output);
            return;
        }
        // An image that never read (a missing file) leaves the style a
        // passthrough with its error, as a shader that doesn't compile does
        if !images_ready {
            drop(builds);
            self.draw_passthrough(frame, encoder, &input_bg, output);
            return;
        }
        // The mask and the edges are there whenever this style is on and
        // reads them; if a host ever draws without one (or the edge pass
        // failed to build), the passthrough keeps the frame whole
        let extra_bg = match &self.extra_bgl {
            None => None,
            Some(_) if !source.inputs.any() => None,
            Some(bgl)
                if (!source.inputs.mask || frame.mask.is_some())
                    && (!source.inputs.edges || frame.edges.is_some()) =>
            {
                // Kept over the edges it reads, by identity (#960)
                let edges = (frame.edges.as_ref())
                    .filter(|_| source.inputs.edges)
                    .map(|e| crate::bind_cache::Bound::View(&e.view));
                Some(frame.depth_bind_over(names.extra, 0, edges.as_slice(), || {
                    let mut entries = Vec::new();
                    if source.inputs.depth {
                        entries.push(wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(frame.depth),
                        });
                    }
                    if let Some(mask) = frame.mask.filter(|_| source.inputs.mask) {
                        entries.push(wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(mask),
                        });
                    }
                    if let Some(edges) = frame.edges.as_ref().filter(|_| source.inputs.edges) {
                        entries.push(wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(&edges.view),
                        });
                    }
                    frame
                        .device
                        .create_bind_group_counted(&wgpu::BindGroupDescriptor {
                            label: Some(names.extra),
                            layout: bgl,
                            entries: &entries,
                        })
                }))
            }
            Some(_) => {
                drop(builds);
                self.draw_passthrough(frame, encoder, &input_bg, output);
                return;
            }
        };

        // This frame's targets (#412): each history target's pair, the
        // others once
        let sizes: Vec<[u32; 2]> = source
            .targets
            .iter()
            .map(|t| t.size(frame.width, frame.height))
            .collect();
        let textures: Vec<[Arc<super::super::StyleTexture>; 2]> = source
            .targets
            .iter()
            .enumerate()
            .map(|(k, t)| {
                let [a, b] = names.targets[k];
                let [w, h] = sizes[k];
                let first = frame.target_sized(a, t.format.wgpu(), w, h);
                let second = if t.history {
                    frame.target_sized(b, t.format.wgpu(), w, h)
                } else {
                    first.clone()
                };
                [first, second]
            })
            .collect();
        let mut history = self.history.lock().unwrap_or_else(|e| e.into_inner());
        let used: Vec<Arc<super::super::StyleTexture>> =
            textures.iter().flatten().cloned().collect();
        let mut trusted = history.last.as_ref().is_some_and(|last| {
            last.frame.wrapping_add(1) == frame.frame
                && last.epoch == frame.history_epoch
                && last.textures.len() == used.len()
                && last
                    .textures
                    .iter()
                    .zip(&used)
                    .all(|(a, b)| Arc::ptr_eq(a, b))
        });
        let write = history.parity;
        let previous = 1 - write;
        // What a frame capture keys the history on (#1136): each history
        // target's name, format and size
        let key = serde_json::json!({
            "targets": source
                .targets
                .iter()
                .zip(&sizes)
                .filter(|(t, _)| t.history)
                .map(|(t, s)| serde_json::json!({
                    "name": t.name,
                    "format": format!("{:?}", t.format.wgpu()),
                    "size": s,
                }))
                .collect::<Vec<_>>(),
        });
        // History a frame capture handed back (#1136): last frame's side of
        // each history target, when the key is the captured frame's
        if let Some(h) = self.imported.take() {
            let style = descriptor_name(&self.loaded);
            if let Some(part) = h.used_part("targets") {
                match super::super::history::key_difference(&part.key, &key) {
                    Some(why) => h.say(format!(
                        "{style}: the captured history was another frame's ({why}): drawn fresh"
                    )),
                    None => {
                        let wrote = source
                            .targets
                            .iter()
                            .enumerate()
                            .filter(|(_, t)| t.history)
                            .try_for_each(|(k, t)| {
                                h.write(frame.queue, part, &t.name, &textures[k][previous].texture)
                            });
                        match wrote {
                            Ok(()) => {
                                trusted = true;
                                h.say(format!("{style}: history taken from the capture"));
                            }
                            Err(why) => h.say(format!("{style}: history not taken: {why}")),
                        }
                    }
                }
            }
        }
        if !trusted {
            for (k, t) in source.targets.iter().enumerate() {
                if t.history {
                    clear(encoder, &textures[k][previous].view);
                }
            }
        }

        let mut images = self
            .images
            .as_ref()
            .map(|i| i.lock().unwrap_or_else(|e| e.into_inner()));
        for (i, pass) in source.passes.iter().enumerate() {
            let size = pass
                .target
                .map_or([frame.width, frame.height], |t| sizes[t]);
            let uniforms = self.uniforms(frame, i, size, trusted);
            frame.queue.write_buffer_counted(
                &self.passes[i].frame_buffer,
                0,
                bytemuck::cast_slice(&[uniforms]),
            );
            let frame_bg = self.frame_bind(frame, i);
            let pick = |r: &ReadSource| if r.previous { previous } else { write };
            // Group 3: the images and the targets this pass reads, keyed by
            // what changes the views (each read's size, which of a history
            // pair it is this frame, and the images' generation); or the
            // images' own group for a pass that reads no target
            let targets_bg = self.passes[i].targets_bgl.as_ref().map(|bgl| {
                let start = images.as_ref().map_or(0, |im| im.generation);
                let key = pass.reads.iter().fold(
                    0xcbf2_9ce4_8422_2325u64 ^ start.wrapping_mul(0x9e37_79b9_7f4a_7c15),
                    |h, r| {
                        let [w, ht] = sizes[r.target];
                        let side = if source.targets[r.target].history {
                            pick(r) as u64 + 1
                        } else {
                            0
                        };
                        let v = (u64::from(w) << 34) ^ (u64::from(ht) << 4) ^ side;
                        (h ^ v).wrapping_mul(0x0100_0000_01b3)
                    },
                );
                frame.bind(names.passes[i].targets, key, || {
                    let mut entries: Vec<wgpu::BindGroupEntry> = images
                        .as_ref()
                        .map(|im| {
                            im.slots
                                .iter()
                                .enumerate()
                                .map(|(b, s)| wgpu::BindGroupEntry {
                                    binding: b as u32,
                                    resource: wgpu::BindingResource::TextureView(
                                        &s.loaded.as_ref().expect("every image is loaded").1,
                                    ),
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let first = source.images.len();
                    entries.extend(pass.reads.iter().enumerate().map(|(b, r)| {
                        wgpu::BindGroupEntry {
                            binding: (first + b) as u32,
                            resource: wgpu::BindingResource::TextureView(
                                &textures[r.target][pick(r)].view,
                            ),
                        }
                    }));
                    frame
                        .device
                        .create_bind_group_counted(&wgpu::BindGroupDescriptor {
                            label: Some(names.passes[i].targets),
                            layout: bgl,
                            entries: &entries,
                        })
                })
            });
            let images_bg: Option<&wgpu::BindGroup> = match (&targets_bg, images.as_mut()) {
                (None, Some(im)) => Some(im.bind_group(frame.device)),
                _ => None,
            };
            let group3: Option<&wgpu::BindGroup> = targets_bg.as_deref().or(images_bg);
            let mut groups: Vec<&wgpu::BindGroup> = vec![&frame_bg, &input_bg];
            if let Some(extra) = &extra_bg {
                groups.push(extra);
            } else if group3.is_some() {
                if let Some(empty) = &self.empty {
                    groups.push(empty);
                }
            }
            if let Some(g) = group3 {
                groups.push(g);
            }
            let out = match pass.target {
                Some(t) => &textures[t][write].view,
                None => output,
            };
            let pipeline = builds[i].pipeline.as_ref().expect("checked above");
            match source.reach.filter(|_| pass.target.is_none()) {
                // A style that says how far past its entities it reaches
                // (#538): the last pass inside that, its passthrough (the
                // same prelude and epilogue around `return p.color`, so
                // the pixels it returns there) around it
                Some(reach) => kit::scissored_pass_around(
                    encoder,
                    names.passes[i].pass,
                    out,
                    [frame.width, frame.height],
                    (pipeline, &groups),
                    frame.styled_rect(reach_pixels(reach, frame.pixels_per_point)),
                    (&self.passthrough, &[&frame_bg, &input_bg]),
                ),
                None => kit::fullscreen_pass(encoder, names.passes[i].pass, out, pipeline, &groups),
            }
        }
        if source.has_history() {
            history.parity = previous;
        }
        history.last = Some(LastFrame {
            frame: frame.frame,
            epoch: frame.history_epoch,
            textures: used,
            trusted,
            key,
        });
    }

    /// Each history target's last-frame side as the captured frame read it
    /// (#1136): the frame read it and wrote the other, so after the frame
    /// it still holds the past.
    fn export_history(&self, out: &mut super::super::history::HistoryExport<'_>) {
        let source = &self.loaded.source;
        if !source.has_history() {
            return;
        }
        let history = self.history.lock().unwrap_or_else(|e| e.into_inner());
        let Some(last) = history.last.as_ref().filter(|l| l.frame == out.frame) else {
            return;
        };
        // The frame flipped the parity to the side it read
        let read = history.parity;
        let textures: Vec<(&str, &wgpu::Texture)> = source
            .targets
            .iter()
            .enumerate()
            .filter(|(_, t)| t.history)
            .filter_map(|(k, t)| Some((t.name.as_str(), &last.textures.get(2 * k + read)?.texture)))
            .collect();
        out.part(
            "targets",
            last.key.clone(),
            last.trusted,
            Default::default(),
            &textures,
        );
    }

    fn import_history(&self, history: super::super::history::ImportedHistory) {
        self.imported.put(history);
    }
}

/// The style's name, for what it says of an imported history.
fn descriptor_name(loaded: &Loaded) -> &str {
    &loaded.source.name
}
