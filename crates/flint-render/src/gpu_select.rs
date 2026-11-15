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
//!    wgpu 23 hid WARP on a machine with a GPU; wgpu 30 lists it there too,
//!    so the same name runs CI's adapter locally.
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
//! # DX12's shader compiler (#912, #947)
//!
//! On DX12 every shader goes from naga's HLSL to bytecode through one of
//! two Microsoft compilers. Flint uses **DXC** by default and **FXC** only
//! as the fallback. FXC is about 10x slower on Flint's style shaders (the
//! ASCII glyph shader: 25 s with `/Od` in a debug build and 66 s optimized,
//! against DXC's 2.5 s) and is where Flint's DX12-only miscompiles came from
//! (#314). With DXC, wgpu also picks the device's shader model, up to 6.7;
//! FXC stops at 5.1.
//!
//! **DXC is linked into the binary** (#947). flint-render's default
//! `static-dxc` feature turns on wgpu's, and wgpu-hal's call into
//! `mach_dxcompiler_rs` lands in `vendor/static-dxc` (the root Cargo.toml's
//! `[patch.crates-io]`): Microsoft's DXC, built from its own source as one
//! static library by Flint's workflow (`tools/dxc-static`). No
//! `dxcompiler.dll` or `dxil.dll` ships; DXC signs DXIL itself. wgpu-hal
//! links it on x86_64 Windows with MSVC only ([`STATIC_DXC`]).
//!
//! [`instance`] passes wgpu's own default, `Dx12Compiler::Auto`: the linked
//! DXC; in a build without it, `dxcompiler.dll` loaded by name (the
//! executable's folder first, then the system folders and `PATH`); FXC if
//! that doesn't load. On top of it Flint keeps:
//!
//! - **The override:** `WGPU_DX12_COMPILER`, read by wgpu's own
//!   `Dx12Compiler::from_env`: `fxc` (the escape hatch for a driver or DXC
//!   bug), `dxc` or `dynamicdxc` (the DLL by name), `staticdxc`, `auto`. A
//!   `staticdxc` in a build without it, or a DLL that doesn't load, gets FXC
//!   and a warning, where wgpu would panic or fail the DX12 instance.
//! - **The report:** [`dx12_shaders`] says which compiler and why:
//!   `DXC (static, v1.9.2609)`, `DXC (<path of dxcompiler.dll>)`,
//!   `FXC (WGPU_DX12_COMPILER=fxc)` or `FXC (fallback: <why>)`.
//!   [`adapter_line`] adds it to an adapter on DX12, for `flint render`,
//!   `flint goldens`, `flint dxc status` and the player's F3 and `--perf`
//!   lines. Static DXC is a fact of the build. For the DLL, Flint loads
//!   `dxcompiler.dll` by name first, as wgpu-hal is about to, and asks
//!   Windows which file that was; wgpu-hal's load then gets the same module.
//!   A shader DXC rejects is not retried with FXC.
//!
//! The DLL lookup is `cfg(windows)`: DX12 is Windows only.

use std::path::PathBuf;
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
/// through [`dx12_shaders`]' choice: wgpu's `Auto` unless
/// `WGPU_DX12_COMPILER` says otherwise.
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

/// Whether this build links DXC (#947): flint-render's `static-dxc`
/// feature, on the one target wgpu-hal and `vendor/static-dxc` link it on.
pub const STATIC_DXC: bool = cfg!(all(
    feature = "static-dxc",
    windows,
    target_arch = "x86_64",
    target_env = "msvc"
));

/// The linked DXC: its release (`v1.9.2609`), its licence and notices
/// (which a Windows build carrying it ships beside it: `flint dxc notices`),
/// and how it was built. `None` in a build without it.
#[cfg(all(
    feature = "static-dxc",
    windows,
    target_arch = "x86_64",
    target_env = "msvc"
))]
pub fn static_dxc() -> Option<StaticDxcInfo> {
    Some(StaticDxcInfo {
        version: mach_dxcompiler_rs::DXC_VERSION,
        commit: mach_dxcompiler_rs::DXC_COMMIT,
        release: mach_dxcompiler_rs::RELEASE_TAG,
        notices: mach_dxcompiler_rs::NOTICES,
        build_info: mach_dxcompiler_rs::BUILD_INFO,
    })
}

/// The linked DXC; `None` in a build without it (this one).
#[cfg(not(all(
    feature = "static-dxc",
    windows,
    target_arch = "x86_64",
    target_env = "msvc"
)))]
pub fn static_dxc() -> Option<StaticDxcInfo> {
    None
}

/// What [`static_dxc`] reports about the DXC linked in.
#[derive(Clone, Copy, Debug)]
pub struct StaticDxcInfo {
    /// The DXC release, `v1.9.2609`.
    pub version: &'static str,
    /// Its commit in microsoft/DirectXShaderCompiler.
    pub commit: &'static str,
    /// The chrischaps/Flint release whose archive was linked.
    pub release: &'static str,
    /// DXC's `LICENSE.TXT` and `ThirdPartyNotices.txt`.
    pub notices: &'static str,
    /// The archive's BUILD-INFO.txt.
    pub build_info: &'static str,
}

/// Which compiler DX12 turns shaders into bytecode with, and why (#912,
/// #947). Its `Display` is the report: `DXC (static, v1.9.2609)`,
/// `DXC (<dll path>)`, `FXC (WGPU_DX12_COMPILER=fxc)` or `FXC (fallback: <why>)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dx12Shaders {
    /// DXC linked into the binary.
    StaticDxc { version: &'static str },
    /// DXC from `dxcompiler.dll`, at `path` (a build without static DXC,
    /// or `WGPU_DX12_COMPILER=dxc`).
    DllDxc { path: PathBuf },
    /// FXC, because `WGPU_DX12_COMPILER=fxc` asked for it.
    FxcAsked,
    /// FXC, because no DXC could be had; `why` says what was missing.
    FxcFallback { why: String },
}

impl Dx12Shaders {
    /// One word, for a line with no room for the rest (the `--perf`
    /// line): `DXC`, `FXC` or `FXC fallback`.
    pub fn short(&self) -> &'static str {
        match self {
            Dx12Shaders::StaticDxc { .. } | Dx12Shaders::DllDxc { .. } => "DXC",
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
            Dx12Shaders::StaticDxc { version } => write!(f, "DXC (static, {version})"),
            Dx12Shaders::DllDxc { path } => write!(f, "DXC ({})", path.display()),
            Dx12Shaders::FxcAsked => write!(f, "FXC (WGPU_DX12_COMPILER=fxc)"),
            Dx12Shaders::FxcFallback { why } => write!(f, "FXC (fallback: {why})"),
        }
    }
}

/// The decision without the environment or the disk: what
/// `WGPU_DX12_COMPILER` asked for (wgpu's parse of it), the linked DXC's
/// version if any, and `load` finding a DLL by name (its path, or why not).
/// Gives the report and what to hand wgpu.
fn choose_dx12(
    asked: Option<wgpu::Dx12Compiler>,
    linked: Option<&'static str>,
    load: impl FnOnce(&str) -> Result<PathBuf, String>,
) -> (Dx12Shaders, wgpu::Dx12Compiler) {
    use wgpu::Dx12Compiler as C;
    let asked = match asked {
        Some(C::StaticDxc) if linked.is_none() => {
            warn_once(&UNLINKED_STATIC_WARNED, || {
                "WGPU_DX12_COMPILER=staticdxc, but this build doesn't link DXC; \
                 using the default"
                    .into()
            });
            C::Auto
        }
        Some(c) => c,
        None => C::Auto,
    };
    match (asked, linked) {
        (C::Fxc, _) => (Dx12Shaders::FxcAsked, C::Fxc),
        (C::DynamicDxc { dxc_path }, _) => match load(&dxc_path) {
            Ok(path) => (Dx12Shaders::DllDxc { path }, C::DynamicDxc { dxc_path }),
            Err(why) => (Dx12Shaders::FxcFallback { why }, C::Fxc),
        },
        (c @ (C::StaticDxc | C::Auto), Some(version)) => (Dx12Shaders::StaticDxc { version }, c),
        (_, None) => match load("dxcompiler.dll") {
            Ok(path) => (Dx12Shaders::DllDxc { path }, C::Auto),
            Err(why) => (
                Dx12Shaders::FxcFallback {
                    why: format!("this build doesn't link DXC, and {why}"),
                },
                C::Fxc,
            ),
        },
    }
}

/// Which compiler an instance made now would give DX12, and why, with what
/// to hand wgpu for it.
fn decide_dx12() -> (Dx12Shaders, wgpu::Dx12Compiler) {
    choose_dx12(
        wgpu::Dx12Compiler::from_env(),
        static_dxc().map(|s| s.version),
        load_dxc_dll,
    )
}

/// Which compiler an instance made now would give DX12, and why.
pub fn dx12_shaders() -> Dx12Shaders {
    decide_dx12().0
}

/// The last choice [`instance`] made for DX12, which is what its adapters
/// compile with.
static LAST_DX12: Mutex<Option<Dx12Shaders>> = Mutex::new(None);
static FALLBACK_WARNED: AtomicBool = AtomicBool::new(false);
static UNLINKED_STATIC_WARNED: AtomicBool = AtomicBool::new(false);

fn warn_once(flag: &AtomicBool, message: impl FnOnce() -> String) {
    if !flag.swap(true, Ordering::Relaxed) {
        tracing::warn!("{}", message());
    }
}

fn dx12_compiler(backends: wgpu::Backends) -> wgpu::Dx12Compiler {
    if !backends.contains(wgpu::Backends::DX12) {
        // No DX12 instance, so nothing to compile with: don't look for DLLs
        return wgpu::Dx12Compiler::Fxc;
    }
    let (shaders, compiler) = decide_dx12();
    *LAST_DX12.lock().unwrap() = Some(shaders);
    compiler
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
/// `name (DiscreteGpu, Dx12, shaders DXC (static, v1.9.2609))`.
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

/// Load `name` (`dxcompiler.dll`, or the path `WGPU_DX12_COMPILER=dxc`
/// gives) the way wgpu-hal does, by Windows' search order, look up
/// `DxcCreateInstance`, and keep it loaded, so wgpu-hal's own load gets this
/// module. Gives the file Windows loaded. Checked once per name.
#[cfg(windows)]
fn load_dxc_dll(name: &str) -> Result<PathBuf, String> {
    type Checked = Vec<(String, Result<PathBuf, String>)>;
    static CHECKED: Mutex<Checked> = Mutex::new(Vec::new());
    let mut checked = CHECKED.lock().unwrap();
    if let Some((_, result)) = checked.iter().find(|(n, _)| n == name) {
        return result.clone();
    }
    let load = || {
        // SAFETY: loading runs the DLL's initialisers; it's the file
        // wgpu-hal loads next, by the same name.
        let lib = unsafe { libloading::os::windows::Library::new(name) }
            .map_err(|e| format!("{name} didn't load ({e})"))?;
        // SAFETY: only looked up, never called here.
        unsafe { lib.get::<unsafe extern "system" fn()>(b"DxcCreateInstance\0") }
            .map_err(|e| format!("{name} has no DxcCreateInstance ({e})"))?;
        // Kept loaded for wgpu-hal: never freed
        let module = lib.into_raw();
        Ok(module_path(module).unwrap_or_else(|| PathBuf::from(name)))
    };
    let result = load();
    checked.push((name.to_string(), result.clone()));
    result
}

#[cfg(not(windows))]
fn load_dxc_dll(_name: &str) -> Result<PathBuf, String> {
    Err("DX12 is Windows only".into())
}

/// The file a loaded module came from.
#[cfg(windows)]
fn module_path(module: isize) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleFileNameW(module: isize, filename: *mut u16, size: u32) -> u32;
    }
    let mut buf = vec![0u16; 1024];
    // SAFETY: `buf` is writable for its length; `module` is a loaded module.
    let n = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (n > 0 && n < buf.len()).then(|| PathBuf::from(std::ffi::OsString::from_wide(&buf[..n])))
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
                 Flint links DXC on x86_64 Windows with MSVC (flint-render's \
                 `static-dxc` feature); in another build, a dxcompiler.dll beside \
                 the executable is found first"
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
    use std::path::PathBuf;
    use wgpu::Dx12Compiler as C;

    fn is_auto(c: &C) -> bool {
        matches!(c, C::Auto)
    }

    // #947: wgpu's Auto, reported as what it gets, with
    // WGPU_DX12_COMPILER (wgpu's parse of it) on top
    #[test]
    fn dx12_takes_wgpus_auto_and_says_which_compiler_it_got() {
        let never = |_: &str| -> Result<PathBuf, String> { panic!("looked for a DLL") };
        let dll = |name: &str| Ok(PathBuf::from(format!("C:\\bin\\{name}")));
        let none = |name: &str| Err(format!("{name} didn't load"));

        // Linked in: Auto, static, and no DLL looked for
        let (s, c) = choose_dx12(None, Some("v1.9.2609"), never);
        assert_eq!(s.to_string(), "DXC (static, v1.9.2609)");
        assert_eq!(s.short(), "DXC");
        assert!(is_auto(&c));
        let (s, c) = choose_dx12(Some(C::StaticDxc), Some("v1.9.2609"), never);
        assert_eq!(
            s,
            Dx12Shaders::StaticDxc {
                version: "v1.9.2609"
            }
        );
        assert!(matches!(c, C::StaticDxc));

        // FXC asked for: never looks for DXC
        let (s, c) = choose_dx12(Some(C::Fxc), Some("v1.9.2609"), never);
        assert_eq!(s.to_string(), "FXC (WGPU_DX12_COMPILER=fxc)");
        assert!(!s.is_fallback());
        assert!(matches!(c, C::Fxc));

        // The DLL asked for, found or not
        let (s, c) = choose_dx12(Some(C::default_dynamic_dxc()), Some("v1.9.2609"), dll);
        assert_eq!(s.to_string(), "DXC (C:\\bin\\dxcompiler.dll)");
        assert!(matches!(c, C::DynamicDxc { .. }));
        let (s, c) = choose_dx12(Some(C::default_dynamic_dxc()), None, none);
        assert_eq!(s.to_string(), "FXC (fallback: dxcompiler.dll didn't load)");
        assert!(matches!(c, C::Fxc));

        // Not linked: Auto finds the DLL by name, else FXC, saying why
        let (s, c) = choose_dx12(None, None, dll);
        assert_eq!(s.short(), "DXC");
        assert!(is_auto(&c));
        let (s, c) = choose_dx12(Some(C::StaticDxc), None, none);
        assert!(s.is_fallback());
        assert_eq!(s.short(), "FXC fallback");
        assert_eq!(
            s.to_string(),
            "FXC (fallback: this build doesn't link DXC, and dxcompiler.dll didn't load)"
        );
        assert!(matches!(c, C::Fxc));
    }

    // A default build on x86_64 Windows with MSVC links DXC (#947)
    #[test]
    #[cfg(all(windows, target_arch = "x86_64", target_env = "msvc"))]
    fn a_default_windows_build_links_dxc() {
        let linked = super::static_dxc().expect("static-dxc is a default feature");
        assert!(linked.version.starts_with('v'), "{}", linked.version);
        assert!(linked.notices.contains("University of Illinois"));
        assert!(linked.build_info.contains(linked.commit));
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
