//! What each adapter on this machine offers of wgpu 30's newer features
//! (spike #963): `cargo run -p flint-render --example gpu_features`.
//!
//! Lists every adapter wgpu finds (all backends, or `WGPU_BACKEND`; an
//! adapter listed twice on one backend, as DX12 lists one a display output,
//! is printed once), and for each the features and limits the spike weighed:
//! the one Flint asks for today (`TIMESTAMP_QUERY`, `context.rs`) and the
//! ones it could. A browser's answer is `navigator.gpu`'s, not this
//! (docs/design/wgpu-30-spike.md has Chrome's and Firefox's).

#![allow(clippy::print_stdout)]

use wgpu::Features as F;

/// The features the spike looked at, in the order its table lists them.
const WATCHED: &[(&str, F)] = &[
    ("TIMESTAMP_QUERY", F::TIMESTAMP_QUERY),
    (
        "TIMESTAMP_QUERY_INSIDE_ENCODERS",
        F::TIMESTAMP_QUERY_INSIDE_ENCODERS,
    ),
    (
        "TIMESTAMP_QUERY_INSIDE_PASSES",
        F::TIMESTAMP_QUERY_INSIDE_PASSES,
    ),
    ("PIPELINE_STATISTICS_QUERY", F::PIPELINE_STATISTICS_QUERY),
    ("PIPELINE_CACHE", F::PIPELINE_CACHE),
    ("SUBGROUP", F::SUBGROUP),
    ("SUBGROUP_BARRIER", F::SUBGROUP_BARRIER),
    ("SHADER_F16", F::SHADER_F16),
    ("DUAL_SOURCE_BLENDING", F::DUAL_SOURCE_BLENDING),
    ("IMMEDIATES", F::IMMEDIATES),
    ("FLOAT32_FILTERABLE", F::FLOAT32_FILTERABLE),
    ("FLOAT32_BLENDABLE", F::FLOAT32_BLENDABLE),
    ("RG11B10UFLOAT_RENDERABLE", F::RG11B10UFLOAT_RENDERABLE),
    ("CLIP_DISTANCES", F::CLIP_DISTANCES),
    ("PRIMITIVE_INDEX", F::PRIMITIVE_INDEX),
    ("SHADER_BARYCENTRICS", F::SHADER_BARYCENTRICS),
    ("MULTIVIEW", F::MULTIVIEW),
    ("SELECTIVE_MULTIVIEW", F::SELECTIVE_MULTIVIEW),
    ("MULTI_DRAW_INDIRECT_COUNT", F::MULTI_DRAW_INDIRECT_COUNT),
    ("TEXTURE_BINDING_ARRAY", F::TEXTURE_BINDING_ARRAY),
    (
        "SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING",
        F::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING,
    ),
    (
        "PARTIALLY_BOUND_BINDING_ARRAY",
        F::PARTIALLY_BOUND_BINDING_ARRAY,
    ),
    ("EXPERIMENTAL_RAY_QUERY", F::EXPERIMENTAL_RAY_QUERY),
    ("EXPERIMENTAL_MESH_SHADER", F::EXPERIMENTAL_MESH_SHADER),
    (
        "EXPERIMENTAL_COOPERATIVE_MATRIX",
        F::EXPERIMENTAL_COOPERATIVE_MATRIX,
    ),
    ("TEXTURE_COMPRESSION_BC", F::TEXTURE_COMPRESSION_BC),
    ("TEXTURE_COMPRESSION_ASTC", F::TEXTURE_COMPRESSION_ASTC),
    ("TEXTURE_COMPRESSION_ETC2", F::TEXTURE_COMPRESSION_ETC2),
];

fn main() {
    let instance = flint_render::gpu_select::instance();
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    let mut seen = std::collections::HashSet::new();
    for adapter in adapters {
        let info = adapter.get_info();
        if !seen.insert((info.name.clone(), info.backend, info.device)) {
            continue;
        }
        let features = adapter.features();
        let limits = adapter.limits();
        println!(
            "== {} ({:?}, {:?})",
            info.name, info.backend, info.device_type
        );
        println!("   driver: {} {}", info.driver, info.driver_info);
        println!(
            "   subgroup size: {}..{}",
            info.subgroup_min_size, info.subgroup_max_size
        );
        for (name, flag) in WATCHED {
            let mark = if features.contains(*flag) {
                "yes"
            } else {
                " - "
            };
            println!("   {mark}  {name}");
        }
        println!("   max_immediate_size: {}", limits.max_immediate_size);
        println!(
            "   max_binding_array_elements_per_shader_stage: {}",
            limits.max_binding_array_elements_per_shader_stage
        );
        println!(
            "   max_multiview_view_count: {}",
            limits.max_multiview_view_count
        );
        println!(
            "   max_color_attachment_bytes_per_sample: {}",
            limits.max_color_attachment_bytes_per_sample
        );
        println!(
            "   transient attachments save memory: {:?}",
            info.transient_saves_memory
        );
    }
}
