//! Each render pipeline's description, kept so a debug view can build its
//! twin (#1203): the same vertex stage, layout, primitive and depth test,
//! with a fragment stage of its own.
//!
//! The overdraw view draws the scene pass's draws again, each adding one
//! to the pixels it shades. Writing an overdraw variant of every scene
//! pipeline by hand would be a second copy of every vertex stage (static,
//! skinned, woozy, echo, billboard, terrain, particles, grass, sprites)
//! that a new pipeline could forget. Instead, while [`keep`] is on, every
//! pipeline made through `create_render_pipeline_counted` (all of this
//! crate's) leaves its description here, keyed by the pipeline itself
//! (wgpu 30 compares and hashes a handle by identity), and [`twin`] makes
//! the variant from it on demand.
//!
//! Off by default and in every game: one relaxed load a pipeline built. A
//! replay (`flint frame replay`) turns it on before its renderer is made.
//! What is kept holds the pipeline's layout and shader module alive, so a
//! process keeping them should be one that builds a bounded set.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static KEEPING: AtomicBool = AtomicBool::new(false);
static KEPT: Mutex<Option<HashMap<wgpu::RenderPipeline, Kept>>> = Mutex::new(None);

/// Keep every render pipeline's description from now on (or stop). A
/// pipeline built while off has no twin.
pub fn keep(on: bool) {
    KEEPING.store(on, Ordering::Relaxed);
}

/// Whether descriptions are being kept.
pub fn keeping() -> bool {
    KEEPING.load(Ordering::Relaxed)
}

/// A pipeline's description, owned.
#[derive(Clone)]
struct Kept {
    label: Option<String>,
    layout: Option<wgpu::PipelineLayout>,
    module: wgpu::ShaderModule,
    entry_point: Option<String>,
    constants: Vec<(String, f64)>,
    zero_initialize_workgroup_memory: bool,
    buffers: Vec<Option<(u64, wgpu::VertexStepMode, Vec<wgpu::VertexAttribute>)>>,
    primitive: wgpu::PrimitiveState,
    depth_stencil: Option<wgpu::DepthStencilState>,
}

/// Note `pipeline`, just built from `desc`, while keeping is on.
pub(crate) fn note(desc: &wgpu::RenderPipelineDescriptor<'_>, pipeline: &wgpu::RenderPipeline) {
    if !keeping() {
        return;
    }
    let v = &desc.vertex;
    let kept = Kept {
        label: desc.label.map(String::from),
        layout: desc.layout.cloned(),
        module: v.module.clone(),
        entry_point: v.entry_point.map(String::from),
        constants: v
            .compilation_options
            .constants
            .iter()
            .map(|(k, x)| (k.to_string(), *x))
            .collect(),
        zero_initialize_workgroup_memory: v.compilation_options.zero_initialize_workgroup_memory,
        buffers: v
            .buffers
            .iter()
            .map(|b| {
                b.as_ref()
                    .map(|b| (b.array_stride, b.step_mode, b.attributes.to_vec()))
            })
            .collect(),
        primitive: desc.primitive,
        depth_stencil: desc.depth_stencil.clone(),
    };
    if let Ok(mut map) = KEPT.lock() {
        map.get_or_insert_with(HashMap::new)
            .insert(pipeline.clone(), kept);
    }
}

/// What a twin's fragment stage is.
#[derive(Clone)]
pub(crate) struct TwinFragment<'a> {
    pub module: &'a wgpu::ShaderModule,
    pub entry_point: &'a str,
    /// Its one colour target. `write_mask` empty draws depth alone.
    pub target: wgpu::ColorTargetState,
    /// The depth target's format, for a pipeline that drew with none.
    pub depth_format: wgpu::TextureFormat,
}

/// `pipeline`'s twin: its vertex stage, layout, primitive and depth test
/// (depth writes included, so later draws are tested as they were), one
/// sample, and `fragment` for the fragment stage. `None` when `pipeline`
/// was built while [`keep`] was off, or its layout was the shader's own
/// (a twin's layout would differ, and the draw's bind groups wouldn't fit).
pub(crate) fn twin(
    device: &wgpu::Device,
    pipeline: &wgpu::RenderPipeline,
    fragment: &TwinFragment<'_>,
) -> Option<wgpu::RenderPipeline> {
    use crate::gpu_counts::CountedDevice;
    // Out of the lock first: building the twin notes it here too
    let kept = KEPT.lock().ok()?.as_ref()?.get(pipeline)?.clone();
    let layout = kept.layout.as_ref()?;
    let attributes: Vec<Option<wgpu::VertexBufferLayout<'_>>> = kept
        .buffers
        .iter()
        .map(|b| {
            b.as_ref()
                .map(|(stride, step, attrs)| wgpu::VertexBufferLayout {
                    array_stride: *stride,
                    step_mode: *step,
                    attributes: attrs,
                })
        })
        .collect();
    let constants: Vec<(&str, f64)> = kept
        .constants
        .iter()
        .map(|(k, x)| (k.as_str(), *x))
        .collect();
    let label = format!("{} (twin)", kept.label.as_deref().unwrap_or("pipeline"));
    let depth_stencil = kept.depth_stencil.clone().or_else(|| {
        // Drawn with no depth (a 2D sprite): test nothing, write nothing
        Some(wgpu::DepthStencilState {
            format: fragment.depth_format,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        })
    });
    Some(
        device.create_render_pipeline_counted(&wgpu::RenderPipelineDescriptor {
            label: Some(&label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: &kept.module,
                entry_point: kept.entry_point.as_deref(),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &constants,
                    zero_initialize_workgroup_memory: kept.zero_initialize_workgroup_memory,
                },
                buffers: &attributes,
            },
            primitive: kept.primitive,
            depth_stencil,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: fragment.module,
                entry_point: Some(fragment.entry_point),
                compilation_options: Default::default(),
                targets: &[Some(fragment.target.clone())],
            }),
            multiview_mask: None,
            cache: None,
        }),
    )
}

/// How many pipelines are kept (tests).
pub fn kept_count() -> usize {
    KEPT.lock()
        .ok()
        .and_then(|m| m.as_ref().map(HashMap::len))
        .unwrap_or(0)
}
