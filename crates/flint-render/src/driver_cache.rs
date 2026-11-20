//! SPIKE #963 prototype: wgpu's driver pipeline cache, kept on disk.
//!
//! `FLINT_SPIKE_PIPELINE_CACHE=<file>` turns it on: the device asks for
//! `Features::PIPELINE_CACHE` (Vulkan only in wgpu-hal 30), the cache is
//! made from the file's bytes if it has any, every pipeline the renderer
//! builds names it, and [`save`] writes `get_data()` back. Unset, every
//! descriptor's `cache` is `None` as before. Not for main as it stands: one
//! process-wide cache for the first device that asks.

#![allow(clippy::disallowed_methods)] // spike: reads its own file with std::fs

use std::sync::OnceLock;

static CACHE: OnceLock<(String, wgpu::PipelineCache)> = OnceLock::new();

fn path() -> Option<String> {
    std::env::var("FLINT_SPIKE_PIPELINE_CACHE")
        .ok()
        .filter(|p| !p.is_empty())
}

/// The features to ask the device for: `PIPELINE_CACHE` when the spike's
/// file is set and the adapter has it.
pub fn features(adapter: &wgpu::Adapter) -> wgpu::Features {
    let mut f = wgpu::Features::empty();
    if path().is_some() {
        f |= adapter.features() & wgpu::Features::PIPELINE_CACHE;
    }
    // FLINT_SPIKE_F16=1 asks for SHADER_F16 too (a shader swapped in by
    // FLINT_STYLE_SHADER_DIR can then `enable f16;`).
    if std::env::var("FLINT_SPIKE_F16").is_ok() {
        f |= adapter.features() & wgpu::Features::SHADER_F16;
    }
    f
}

/// Make the cache for `device` from the file, once a process.
pub fn init(device: &wgpu::Device, info: &wgpu::AdapterInfo) {
    let Some(path) = path() else { return };
    if !device.features().contains(wgpu::Features::PIPELINE_CACHE) {
        eprintln!("spike cache: {:?} has no PIPELINE_CACHE", info.backend);
        return;
    }
    let file = match wgpu::util::pipeline_cache_key(info) {
        Some(key) => format!("{path}.{key}"),
        None => path,
    };
    let data = std::fs::read(&file).ok();
    eprintln!(
        "spike cache: {file}: {}",
        data.as_ref()
            .map_or("empty".to_string(), |d| format!("{} bytes", d.len()))
    );
    // SAFETY: the data is what `get_data` wrote for this adapter's key.
    let cache = unsafe {
        device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor {
            label: Some("spike pipeline cache"),
            data: data.as_deref(),
            fallback: true,
        })
    };
    let _ = CACHE.set((file, cache));
}

/// The cache every pipeline descriptor names, if the spike made one.
pub fn cache() -> Option<&'static wgpu::PipelineCache> {
    CACHE.get().map(|(_, c)| c)
}

/// Write the cache's data back to its file.
pub fn save() {
    if let Some((file, cache)) = CACHE.get() {
        if let Some(data) = cache.get_data() {
            eprintln!("spike cache: saved {} bytes", data.len());
            let _ = std::fs::write(file, data);
        }
    }
}
