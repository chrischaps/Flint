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
//!
//! # DX12's shader compiler (#912)
//!
//! On DX12 every shader goes from naga's HLSL to bytecode through one of
//! two Microsoft compilers. Flint uses **DXC** by default and **FXC** only
//! as the fallback. FXC is about 10x slower on Flint's style shaders (the
//! ASCII glyph shader: 25 s with `/Od` in a debug build and 66 s optimized,
//! against DXC's 2.5 s) and is where Flint's DX12-only miscompiles came from
//! (#314). With DXC, wgpu also picks the device's shader model, up to 6.7;
//! FXC stops at 5.1.
//!
//! wgpu 23 doesn't link DXC in: it loads `dxcompiler.dll` and `dxil.dll` at
//! instance creation. [`instance`] passes both by full path from one folder,
//! [`dxc_dir`]: `FLINT_DXC_DIR` if set, else the running executable's
//! folder. A full path means a copy elsewhere on `PATH` is never loaded
//! (with no path, wgpu would take whichever `dxcompiler.dll` Windows' search
//! order finds). `flint dxc install` puts the pinned release's two DLLs
//! beside `flint.exe` and the test binaries.
//!
//! - `WGPU_DX12_COMPILER=fxc` forces FXC, the escape hatch for a driver or
//!   DXC bug. `dxc` (or unset) is the default.
//! - If a DLL isn't in the folder or doesn't load, the instance gets FXC,
//!   as wgpu itself would fall back, and [`request_adapter`] warns once when
//!   it picks a DX12 adapter. A shader DXC rejects is not retried with FXC.
//! - [`dx12_shaders`] says which one, and why: `DXC (<dir>)`,
//!   `FXC (fallback: dxcompiler.dll not in <dir>)` or
//!   `FXC (WGPU_DX12_COMPILER=fxc)`. [`adapter_line`] adds it to an adapter
//!   on DX12, for `flint render`, `flint goldens` and the player's F3 and
//!   `--perf` lines.
//!
//! The check is the one wgpu-hal 23 makes (`DxcLib::new`: load the file,
//! then look up `DxcCreateInstance`), made first so the report is what the
//! instance got. It reads the disk with `std::fs`, outside the vfs: a DLL
//! beside the executable isn't game data, and DX12 is Windows only, so the
//! code is `cfg(windows)`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
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
    gpu_override().or_else(|| {
        std::env::var("WGPU_ADAPTER_NAME")
            .ok()
            .filter(|s| !s.is_empty())
    })
}

/// Backends to enumerate: `WGPU_BACKEND` env var, else all.
pub fn backends() -> wgpu::Backends {
    wgpu::Backends::from_env().unwrap_or(wgpu::Backends::all())
}

/// Create a wgpu instance over [`backends`], with DX12 compiling shaders
/// through [`dx12_shaders`]' choice (DXC unless it's missing or refused).
pub fn instance() -> wgpu::Instance {
    let backends = backends();
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        backend_options: wgpu::BackendOptions {
            dx12: wgpu::Dx12BackendOptions {
                shader_compiler: dx12_compiler(backends),
                ..Default::default()
            },
            ..Default::default()
        },
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    })
}

/// The DLLs DXC needs, in the order wgpu-hal 23 loads them.
pub const DXC_DLLS: [&str; 2] = ["dxcompiler.dll", "dxil.dll"];

/// Which compiler DX12 turns shaders into bytecode with, and why (#912).
/// Its `Display` is the report: `DXC (<dir>)`, `FXC (WGPU_DX12_COMPILER=fxc)`
/// or `FXC (fallback: <why>)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dx12Shaders {
    /// DXC, both DLLs loaded from `dir`.
    Dxc { dir: PathBuf },
    /// DXC linked into the binary (the `static-dxc` feature, #934).
    StaticDxc,
    /// FXC, because `WGPU_DX12_COMPILER=fxc` asked for it.
    FxcAsked,
    /// FXC, because DXC couldn't be loaded; `why` names the DLL and folder.
    FxcFallback { why: String },
}

impl Dx12Shaders {
    /// One word, for a line with no room for the folder (the `--perf`
    /// line): `DXC`, `FXC` or `FXC fallback`.
    pub fn short(&self) -> &'static str {
        match self {
            Dx12Shaders::Dxc { .. } | Dx12Shaders::StaticDxc => "DXC",
            Dx12Shaders::FxcAsked => "FXC",
            Dx12Shaders::FxcFallback { .. } => "FXC fallback",
        }
    }

    pub fn is_fallback(&self) -> bool {
        matches!(self, Dx12Shaders::FxcFallback { .. })
    }
}

impl std::fmt::Display for Dx12Shaders {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Dx12Shaders::Dxc { dir } => write!(f, "DXC ({})", dir.display()),
            Dx12Shaders::StaticDxc => write!(f, "DXC (static)"),
            Dx12Shaders::FxcAsked => write!(f, "FXC (WGPU_DX12_COMPILER=fxc)"),
            Dx12Shaders::FxcFallback { why } => write!(f, "FXC (fallback: {why})"),
        }
    }
}

/// The folder DXC's DLLs load from: `FLINT_DXC_DIR` if set, else the
/// running executable's (for a cargo test, `target/<profile>/deps`).
pub fn dxc_dir() -> Option<PathBuf> {
    match std::env::var_os("FLINT_DXC_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir)),
        None => std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf)),
    }
}

/// What `WGPU_DX12_COMPILER` and [`dxc_dir`] give, with `load` checking a
/// folder's DLLs (Err names what's wrong). The decision without the disk.
fn choose_dx12(
    compiler: Option<&str>,
    dir: Option<PathBuf>,
    load: impl FnOnce(&Path) -> Result<(), String>,
) -> Dx12Shaders {
    if compiler.is_some_and(|c| c.trim().eq_ignore_ascii_case("fxc")) {
        return Dx12Shaders::FxcAsked;
    }
    let Some(dir) = dir else {
        return Dx12Shaders::FxcFallback {
            why: "the executable's folder is unknown; set FLINT_DXC_DIR".into(),
        };
    };
    match load(&dir) {
        Ok(()) => Dx12Shaders::Dxc { dir },
        Err(why) => Dx12Shaders::FxcFallback { why },
    }
}

/// Which compiler an instance made now would give DX12, and why.
pub fn dx12_shaders() -> Dx12Shaders {
    let compiler = std::env::var("WGPU_DX12_COMPILER").ok();
    if let Some(c) = compiler.as_deref().map(str::trim) {
        if !c.is_empty() && !c.eq_ignore_ascii_case("fxc") && !c.eq_ignore_ascii_case("dxc") {
            warn_once(&UNKNOWN_COMPILER_WARNED, || {
                format!("WGPU_DX12_COMPILER={c:?} is neither dxc nor fxc; using the default, DXC")
            });
        }
    }
    let chosen = choose_dx12(compiler.as_deref(), dxc_dir(), load_dxc);
    // Built with DXC inside, every choice but an explicit FXC is that one
    if cfg!(all(
        feature = "static-dxc",
        windows,
        target_arch = "x86_64",
        target_env = "msvc"
    )) && chosen != Dx12Shaders::FxcAsked
    {
        return Dx12Shaders::StaticDxc;
    }
    chosen
}

/// The last choice [`instance`] made for DX12, which is what its adapters
/// compile with.
static LAST_DX12: Mutex<Option<Dx12Shaders>> = Mutex::new(None);
static FALLBACK_WARNED: AtomicBool = AtomicBool::new(false);
static UNKNOWN_COMPILER_WARNED: AtomicBool = AtomicBool::new(false);

fn warn_once(flag: &AtomicBool, message: impl FnOnce() -> String) {
    if !flag.swap(true, Ordering::Relaxed) {
        tracing::warn!("{}", message());
    }
}

fn dx12_compiler(backends: wgpu::Backends) -> wgpu::Dx12Compiler {
    if !backends.contains(wgpu::Backends::DX12) {
        return wgpu::Dx12Compiler::Fxc;
    }
    let shaders = dx12_shaders();
    *LAST_DX12.lock().unwrap() = Some(shaders.clone());
    match shaders {
        // wgpu >= 27 needs no dxil.dll; a DynamicDxc that fails to load is
        // an instance error in wgpu 30, not a quiet FXC fallback (#934)
        Dx12Shaders::StaticDxc => wgpu::Dx12Compiler::StaticDxc,
        Dx12Shaders::Dxc { dir } => wgpu::Dx12Compiler::DynamicDxc {
            dxc_path: dir.join(DXC_DLLS[0]).to_string_lossy().into_owned(),
        },
        // A missing DLL would make wgpu fall back by itself, but only after
        // a log line from every instance, even one that draws on Vulkan
        _ => wgpu::Dx12Compiler::Fxc,
    }
}

/// What compiles shaders on `backend`: on DX12, the last [`instance`]'s
/// [`Dx12Shaders`] (or one decided now); `None` on any other backend.
pub fn shader_compiler(backend: wgpu::Backend) -> Option<Dx12Shaders> {
    (backend == wgpu::Backend::Dx12).then(|| {
        LAST_DX12
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(dx12_shaders)
    })
}

/// An adapter for a report: `name (DiscreteGpu, Vulkan)`, and on DX12
/// `name (DiscreteGpu, Dx12, shaders DXC (<dir>))`.
pub fn adapter_line(info: &wgpu::AdapterInfo) -> String {
    match shader_compiler(info.backend) {
        Some(s) => format!(
            "{} ({:?}, {:?}, shaders {s})",
            info.name, info.device_type, info.backend
        ),
        None => format!("{} ({:?}, {:?})", info.name, info.device_type, info.backend),
    }
}

/// [`adapter_line`] for a line with no room: `name (Vulkan)`, and on DX12
/// `name (Dx12, DXC)` ([`Dx12Shaders::short`]). The player's F3 and
/// `--perf` lines.
pub fn adapter_short(info: &wgpu::AdapterInfo) -> String {
    match shader_compiler(info.backend) {
        Some(s) => format!("{} ({:?}, {})", info.name, info.backend, s.short()),
        None => format!("{} ({:?})", info.name, info.backend),
    }
}

/// Load `dir`'s two DLLs as wgpu-hal 23 does (`DxcLib::new`, then
/// `DxcCreateInstance`), and keep them loaded: wgpu loads the same files
/// next and holds them for the device's life. Checked once per folder.
#[cfg(windows)]
fn load_dxc(dir: &Path) -> Result<(), String> {
    type Checked = Vec<(PathBuf, Result<(), String>)>;
    static CHECKED: Mutex<Checked> = Mutex::new(Vec::new());
    let mut checked = CHECKED.lock().unwrap();
    if let Some((_, result)) = checked.iter().find(|(d, _)| d == dir) {
        return result.clone();
    }
    let result = DXC_DLLS.iter().try_for_each(|name| {
        let path = dir.join(name);
        if !path.is_file() {
            return Err(format!("{name} not in {}", dir.display()));
        }
        // SAFETY: loading runs the DLL's initialisers; it's the file
        // wgpu-hal loads next, by the same full path.
        let lib = unsafe { libloading::Library::new(&path) }
            .map_err(|e| format!("{} didn't load: {e}", path.display()))?;
        // SAFETY: only looked up, never called here.
        unsafe { lib.get::<unsafe extern "system" fn()>(b"DxcCreateInstance\0") }
            .map_err(|e| format!("{} has no DxcCreateInstance: {e}", path.display()))?;
        std::mem::forget(lib);
        Ok(())
    });
    checked.push((dir.to_path_buf(), result.clone()));
    result
}

#[cfg(not(windows))]
fn load_dxc(_dir: &Path) -> Result<(), String> {
    Err("DX12 is Windows only".into())
}

fn power_preference() -> wgpu::PowerPreference {
    wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::HighPerformance)
}

/// Pick an adapter honouring the override/env/default chain described in
/// the module docs. `compatible_surface` is `None` for headless use.
pub async fn request_adapter(
    instance: &wgpu::Instance,
    compatible_surface: Option<&wgpu::Surface<'_>>,
) -> Option<wgpu::Adapter> {
    let adapter = match name_filter() {
        Some(filter) => select_by_name(instance, compatible_surface, &filter).await,
        None => instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: power_preference(),
                compatible_surface,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .ok(),
    }?;

    let info = adapter.get_info();
    tracing::info!(
        "GPU adapter: {}, driver: {} {}",
        adapter_line(&info),
        info.driver,
        info.driver_info
    );
    if let Some(shaders) = shader_compiler(info.backend).filter(Dx12Shaders::is_fallback) {
        warn_once(&FALLBACK_WARNED, || {
            format!(
                "DX12 compiles shaders with {shaders}, about 10x slower than DXC. \
                 `flint dxc install` puts dxcompiler.dll and dxil.dll beside the \
                 executable, or FLINT_DXC_DIR names a folder holding them"
            )
        });
    }
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
    let adapters = instance.enumerate_adapters(backends()).await;
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
                apply_limit_buckets: false,
            })
            .await
            .ok();
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
    let adapters = pollster::block_on(instance().enumerate_adapters(backends()));
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
    use super::{choose_dx12, Dx12Shaders};
    use crate::context::RenderError;
    use std::path::{Path, PathBuf};

    // #912: DXC from the folder unless FXC is asked for or a DLL is
    // missing, and each says why in the words the adapter line prints
    #[test]
    fn dx12_takes_dxc_from_the_folder_and_says_why_when_it_doesnt() {
        let dir = PathBuf::from("bin");
        let ok = |_: &Path| Ok(());
        let missing = |d: &Path| Err(format!("dxcompiler.dll not in {}", d.display()));
        let dxc = choose_dx12(None, Some(dir.clone()), ok);
        assert_eq!(dxc, Dx12Shaders::Dxc { dir: dir.clone() });
        assert_eq!(dxc.to_string(), "DXC (bin)");
        assert_eq!(choose_dx12(Some("dxc"), Some(dir.clone()), ok), dxc);
        // Asked for, FXC never looks at the folder
        let never = |_: &Path| -> Result<(), String> { panic!("looked at the DLLs") };
        let fxc = choose_dx12(Some(" FXC "), Some(dir.clone()), never);
        assert_eq!(fxc.to_string(), "FXC (WGPU_DX12_COMPILER=fxc)");
        assert!(!fxc.is_fallback());
        let fallback = choose_dx12(None, Some(dir.clone()), missing);
        assert_eq!(
            fallback.to_string(),
            "FXC (fallback: dxcompiler.dll not in bin)"
        );
        assert!(fallback.is_fallback());
        assert_eq!(fallback.short(), "FXC fallback");
        assert!(choose_dx12(None, None, never).is_fallback());
    }

    // The report names DX12's compiler, and only on DX12
    #[test]
    fn the_adapter_line_names_the_compiler_on_dx12_only() {
        let info = |backend| wgpu::AdapterInfo {
            name: "GPU".into(),
            ..wgpu::AdapterInfo::new(wgpu::DeviceType::DiscreteGpu, backend)
        };
        assert_eq!(
            super::adapter_line(&info(wgpu::Backend::Vulkan)),
            "GPU (DiscreteGpu, Vulkan)"
        );
        let dx12 = super::adapter_line(&info(wgpu::Backend::Dx12));
        assert!(
            dx12.starts_with("GPU (DiscreteGpu, Dx12, shaders "),
            "{dx12}"
        );
        assert!(dx12.contains("XC ("), "{dx12}");
        assert_eq!(
            super::adapter_short(&info(wgpu::Backend::Vulkan)),
            "GPU (Vulkan)"
        );
        let short = super::adapter_short(&info(wgpu::Backend::Dx12));
        assert!(
            short.starts_with("GPU (Dx12, ") && short.contains("XC"),
            "{short}"
        );
    }

    // `flint goldens` tells "no GPU" from a failed case by these words
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
