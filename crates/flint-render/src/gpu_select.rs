//! GPU adapter selection.
//!
//! Resolution order for the adapter used by both the windowed and headless
//! contexts:
//!
//! 1. A name set programmatically via [`set_gpu_override`] (the `--gpu` CLI
//!    flag on `play` / `render`).
//! 2. `WGPU_ADAPTER_NAME` — case-insensitive substring match against the
//!    adapter name (e.g. `NVIDIA`, `Intel`). A name nothing matches falls
//!    back to the power preference, except under `FLINT_REQUIRE_GPU`
//!    ([`gpu_required`]), where it finds no adapter: CI asks for WARP by
//!    name, and a run on anything else would test the wrong adapter (#314).
//!    wgpu 23 hides WARP on a machine with a GPU; it appears on a runner
//!    with none.
//! 3. `WGPU_POWER_PREF` — `high`, `low`, or `none`.
//! 4. Default: `PowerPreference::HighPerformance`, so dual-GPU laptops pick
//!    the discrete card rather than whichever adapter enumerates first.
//!
//! The backend set comes from `WGPU_BACKEND` (`vulkan`, `dx12`, `gl`, or a
//! comma list) via [`backends`], defaulting to all. On hybrid laptops whose
//! panel is wired to the integrated GPU, DX12 presents cross-adapter frames
//! far better than Vulkan; `WGPU_BACKEND=dx12` is the first thing to try when
//! the discrete GPU renders fast but the screen updates slowly.
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

/// Backends to enumerate: `WGPU_BACKEND` env var, else all.
pub fn backends() -> wgpu::Backends {
    wgpu::util::backend_bits_from_env().unwrap_or(wgpu::Backends::all())
}

/// Create a wgpu instance over [`backends`].
pub fn instance() -> wgpu::Instance {
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: backends(),
        ..Default::default()
    })
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
    // A browser hands out one adapter through `requestAdapter`; with no list
    // to filter, the web falls back to the power preference below.
    #[cfg(not(target_arch = "wasm32"))]
    let adapters = instance.enumerate_adapters(backends());
    #[cfg(target_arch = "wasm32")]
    let adapters: Vec<wgpu::Adapter> = Vec::new();
    let available: Vec<String> = adapters.iter().map(|a| a.get_info().name).collect();

    let matched = adapters.into_iter().find(|a| {
        a.get_info().name.to_lowercase().contains(&filter)
            && compatible_surface.map_or(true, |s| a.is_surface_supported(s))
    });

    if matched.is_none() {
        // A run that requires its GPU (CI on WARP, #314) asked for that
        // adapter by name: drawing on another would test the wrong one
        if gpu_required() {
            tracing::error!(
                "No GPU adapter matching {:?}; available: {:?}. FLINT_REQUIRE_GPU: no fallback.",
                filter,
                available
            );
            return None;
        }
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

/// Whether GPU tests must find an adapter (#314): `FLINT_REQUIRE_GPU` set
/// to anything but `0` or empty. CI sets it on the job that renders on a
/// software adapter, so a test that can't get one fails there instead of
/// passing without having run.
pub fn gpu_required() -> bool {
    std::env::var("FLINT_REQUIRE_GPU").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// What a GPU test calls when it found no adapter, before it returns
/// (#314). It prints a note and the test passes, as on a machine with no
/// GPU, unless [`gpu_required`], when it panics: a skip there is a failure.
/// `why` names what was missing ("no GPU adapter", "no adapter with
/// timestamp queries").
#[track_caller]
pub fn skip_without_gpu(why: &str) {
    if gpu_required() {
        panic!(
            "{why}, and FLINT_REQUIRE_GPU is set; adapters: {:?}",
            list_adapters()
        );
    }
    eprintln!("{why}; skipping (FLINT_REQUIRE_GPU=1 fails instead)");
}

/// What to do about finding no adapter (#220), for the end of the
/// no-adapter error and for an empty `--list-gpus`. It names what narrowed
/// the search (`--gpu`, `WGPU_ADAPTER_NAME`, `WGPU_BACKEND`), then the
/// platform's software adapter: lavapipe on Linux (a machine with no GPU,
/// such as a container or a cloud agent, has no Vulkan driver until
/// `mesa-vulkan-drivers` is installed), WARP on Windows.
pub fn no_adapter_hint() -> String {
    let mut lines = Vec::new();
    if let Some(name) = name_filter() {
        lines.push(format!(
            "only adapters named like {name:?} were asked for (--gpu or WGPU_ADAPTER_NAME)"
        ));
    }
    if let Ok(b) = std::env::var("WGPU_BACKEND") {
        if !b.is_empty() {
            lines.push(format!(
                "only the {b:?} backend was searched (WGPU_BACKEND)"
            ));
        }
    }
    lines.push(software_adapter_hint().to_string());
    lines.join("; ")
}

#[cfg(target_os = "linux")]
fn software_adapter_hint() -> &'static str {
    "with no GPU, install a software Vulkan driver (lavapipe): \
     `sudo apt-get install mesa-vulkan-drivers` on Debian/Ubuntu, \
     then `flint render --list-gpus` should list \"llvmpipe\""
}

#[cfg(target_os = "windows")]
fn software_adapter_hint() -> &'static str {
    "with no GPU, Windows' software adapter is WARP: \
     WGPU_BACKEND=dx12 WGPU_ADAPTER_NAME=\"Microsoft Basic Render Driver\""
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn software_adapter_hint() -> &'static str {
    "check `flint render --list-gpus` and the GPU driver"
}

/// List available adapters as `(name, device type, backend)` strings.
pub fn list_adapters() -> Vec<String> {
    #[cfg(not(target_arch = "wasm32"))]
    let adapters = instance().enumerate_adapters(backends());
    #[cfg(target_arch = "wasm32")]
    let adapters: Vec<wgpu::Adapter> = Vec::new();
    adapters
        .iter()
        .map(|a| {
            let i = a.get_info();
            format!("{} [{:?}, {:?}]", i.name, i.device_type, i.backend)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::context::RenderError;

    // `render-suite` tells "no GPU" from a failed case by these words
    // (`looks_like_no_gpu`), so the hint goes after them
    #[test]
    fn no_adapter_error_keeps_its_words_and_names_the_fix() {
        let message = RenderError::AdapterNotFound.to_string();
        assert!(message.starts_with("Failed to get adapter: "), "{message}");
        #[cfg(target_os = "linux")]
        assert!(message.contains("mesa-vulkan-drivers"), "{message}");
        #[cfg(target_os = "windows")]
        assert!(
            message.contains("Microsoft Basic Render Driver"),
            "{message}"
        );
    }
}
