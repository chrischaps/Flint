//! GPU adapter selection.
//!
//! Resolution order for the adapter used by both the windowed and headless
//! contexts:
//!
//! 1. A name set programmatically via [`set_gpu_override`] (the `--gpu` CLI
//!    flag on `play` / `render`).
//! 2. `WGPU_ADAPTER_NAME` — case-insensitive substring match against the
//!    adapter name (e.g. `NVIDIA`, `Intel`).
//! 3. `WGPU_POWER_PREF` — `high`, `low`, or `none`.
//! 4. Default: `PowerPreference::HighPerformance`, so dual-GPU laptops pick
//!    the discrete card rather than whichever adapter enumerates first.
//!
//! The chosen adapter is logged at `info` level so the pick is visible.

use std::sync::Mutex;

static GPU_OVERRIDE: Mutex<Option<String>> = Mutex::new(None);

/// Set a process-wide adapter name filter (substring, case-insensitive).
/// Call before creating any render context.
pub fn set_gpu_override(name: Option<String>) {
    *GPU_OVERRIDE.lock().unwrap() = name.filter(|s| !s.trim().is_empty());
}

fn gpu_override() -> Option<String> {
    GPU_OVERRIDE.lock().unwrap().clone()
}

fn name_filter() -> Option<String> {
    gpu_override().or_else(|| std::env::var("WGPU_ADAPTER_NAME").ok().filter(|s| !s.is_empty()))
}

fn power_preference() -> wgpu::PowerPreference {
    wgpu::util::power_preference_from_env().unwrap_or(wgpu::PowerPreference::HighPerformance)
}

/// Pick an adapter honouring the override/env/default chain described in
/// the module docs. `compatible_surface` is `None` for headless use.
pub async fn request_adapter(
    instance: &wgpu::Instance,
    compatible_surface: Option<&wgpu::Surface<'_>>,
) -> Option<wgpu::Adapter> {
    let adapter = match name_filter() {
        Some(filter) => select_by_name(instance, compatible_surface, &filter).await,
        None => {
            instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: power_preference(),
                    compatible_surface,
                    force_fallback_adapter: false,
                })
                .await
        }
    }?;

    let info = adapter.get_info();
    tracing::info!(
        "GPU adapter: {} ({:?}, {:?}, driver: {} {})",
        info.name,
        info.device_type,
        info.backend,
        info.driver,
        info.driver_info
    );
    Some(adapter)
}

async fn select_by_name(
    instance: &wgpu::Instance,
    compatible_surface: Option<&wgpu::Surface<'_>>,
    filter: &str,
) -> Option<wgpu::Adapter> {
    let filter = filter.to_lowercase();
    let adapters = instance.enumerate_adapters(wgpu::Backends::all());
    let available: Vec<String> = adapters.iter().map(|a| a.get_info().name).collect();

    let matched = adapters.into_iter().find(|a| {
        a.get_info().name.to_lowercase().contains(&filter)
            && compatible_surface.map_or(true, |s| a.is_surface_supported(s))
    });

    if matched.is_none() {
        tracing::warn!(
            "No GPU adapter matching {:?}; available: {:?}. Falling back to power preference.",
            filter,
            available
        );
        return instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: power_preference(),
                compatible_surface,
                force_fallback_adapter: false,
            })
            .await;
    }
    matched
}

/// List available adapters as `(name, device type, backend)` strings.
pub fn list_adapters() -> Vec<String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });
    instance
        .enumerate_adapters(wgpu::Backends::all())
        .iter()
        .map(|a| {
            let i = a.get_info();
            format!("{} [{:?}, {:?}]", i.name, i.device_type, i.backend)
        })
        .collect()
}
