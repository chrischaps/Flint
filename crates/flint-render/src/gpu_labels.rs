//! Names for GPU debuggers (#1083).
//!
//! RenderDoc, PIX and Nsight show a frame as the commands it recorded. Left
//! alone, a Flint frame there is a long unnamed run of draws; with these
//! names it reads as the same parts the F2 overlay's GPU rows time.
//!
//! - **Debug groups** bracket each part of the frame the GPU timer names
//!   (`Grass`, `Shadows`, `Scene`, each effect and style, `Composite`,
//!   `World lines`, the player's `UI`): [`begin`] and [`end`] open both the
//!   group and the timer's scope, so the two can't drift apart. Inside the
//!   main and shadow passes a group holds each layer of the draw list
//!   (`Terrain`, `Opaque static`, ...), and each shadow map layer is a pass
//!   of its own name (`Shadow cascade 0`, `Spot shadow`).
//! - **A marker per draw** names the entity it draws and its kind
//!   (`Barrel · Static`), formatted into one buffer reused for the frame.
//!
//! Which of those a frame records is the process's [`level`]:
//!
//! | `FLINT_GPU_LABELS` | Level |
//! |---|---|
//! | unset | [`GpuLabels::Draws`] under a capture tool ([`capture_tool`]), else [`GpuLabels::Groups`] natively and [`GpuLabels::Off`] on the web |
//! | `0` or `off` | [`GpuLabels::Off`] |
//! | `groups` | [`GpuLabels::Groups`] |
//! | `1`, `on` or `draws` | [`GpuLabels::Draws`] |
//! | anything else | warned about once, then as unset |
//!
//! A group names a part, not a draw, so it costs a few calls a frame and no
//! formatting: natively they are always on. On the web each is a WebGPU
//! call, so a frame there makes none unless [`set_level`] asks (the
//! player's `?gpu_labels=1`), and `?perf=1`'s call counts stay as they were.
//! Draw markers format a string a draw, so they are off unless asked for or
//! a capture tool is attached.
//!
//! At [`GpuLabels::Draws`] the wgpu instance is also made with
//! `InstanceFlags::DEBUG` ([`crate::gpu_select::instance`]): Vulkan passes
//! no label to the driver without it in a release build, and with it
//! shaders carry their debug info, so a capture shows their source.

use std::sync::atomic::{AtomicU8, Ordering};

use crate::gpu_timer::{GpuScope, GpuTimer};

/// How much of the frame is named for GPU debuggers; see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GpuLabels {
    /// Nothing beyond the labels objects and passes are made with.
    Off,
    /// A debug group round each part of the frame and each draw-list layer.
    Groups,
    /// Groups, and a marker per draw naming its entity.
    Draws,
}

const UNSET: u8 = u8::MAX;
static LEVEL: AtomicU8 = AtomicU8::new(UNSET);

impl GpuLabels {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => GpuLabels::Off,
            1 => GpuLabels::Groups,
            _ => GpuLabels::Draws,
        }
    }

    /// What `FLINT_GPU_LABELS` set to `value` asks for, `None` when unset
    /// or empty.
    pub fn parse(value: Option<&str>) -> Option<Self> {
        match value.map(str::trim) {
            None | Some("") => None,
            Some(v) if v == "0" || v.eq_ignore_ascii_case("off") => Some(GpuLabels::Off),
            Some(v) if v.eq_ignore_ascii_case("groups") => Some(GpuLabels::Groups),
            Some(_) => Some(GpuLabels::Draws),
        }
    }
}

/// The level this process names its frames at: [`set_level`]'s, else
/// `FLINT_GPU_LABELS`', else what the module docs' table says. Read once.
pub fn level() -> GpuLabels {
    let v = LEVEL.load(Ordering::Relaxed);
    if v != UNSET {
        return GpuLabels::from_u8(v);
    }
    let level = default_level(flint_core::env::GPU_LABELS.choice(), capture_tool());
    // A set_level that raced this one wins
    let _ = LEVEL.compare_exchange(UNSET, level as u8, Ordering::Relaxed, Ordering::Relaxed);
    GpuLabels::from_u8(LEVEL.load(Ordering::Relaxed))
}

/// Name frames at `level` from now on. For a host with no environment to
/// read (the web player's `?gpu_labels=`); before the renderer is made, so
/// the instance is made to match.
pub fn set_level(level: GpuLabels) {
    LEVEL.store(level as u8, Ordering::Relaxed);
}

/// Whether frames carry their debug groups.
pub fn groups() -> bool {
    level() >= GpuLabels::Groups
}

/// Whether each draw carries a marker naming its entity.
pub fn draws() -> bool {
    level() >= GpuLabels::Draws
}

fn default_level(env: Option<&str>, tool: Option<&'static str>) -> GpuLabels {
    if let Some(level) = GpuLabels::parse(env) {
        return level;
    }
    if tool.is_some() {
        GpuLabels::Draws
    } else if cfg!(target_arch = "wasm32") {
        GpuLabels::Off
    } else {
        GpuLabels::Groups
    }
}

/// The frame debugger this process was launched under, if one is known:
/// `RenderDoc` (its Vulkan layer's switch, `ENABLE_VULKAN_RENDERDOC_CAPTURE`,
/// or its library loaded into the process) or `PIX` (its capturer loaded).
/// A tool attached after start-up is not seen.
pub fn capture_tool() -> Option<&'static str> {
    if std::env::var("ENABLE_VULKAN_RENDERDOC_CAPTURE").is_ok_and(|v| v.trim() == "1") {
        return Some("RenderDoc");
    }
    loaded_capture_library()
}

#[cfg(windows)]
fn loaded_capture_library() -> Option<&'static str> {
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> isize;
    }
    let loaded = |name: &str| {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: a NUL-terminated UTF-16 name; the handle isn't kept, and
        // GetModuleHandleW takes no reference on the module.
        unsafe { GetModuleHandleW(wide.as_ptr()) != 0 }
    };
    [
        ("renderdoc.dll", "RenderDoc"),
        ("WinPixGpuCapturer.dll", "PIX"),
    ]
    .into_iter()
    .find(|(dll, _)| loaded(dll))
    .map(|(_, tool)| tool)
}

#[cfg(target_os = "linux")]
fn loaded_capture_library() -> Option<&'static str> {
    // The process's own memory map, not a game file: past the vfs
    #[allow(clippy::disallowed_methods)] // reads /proc, not an asset
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    maps.contains("librenderdoc.so").then_some("RenderDoc")
}

#[cfg(not(any(windows, target_os = "linux")))]
fn loaded_capture_library() -> Option<&'static str> {
    None
}

/// A part of the frame [`begin`] opened, to hand back to [`end`].
#[derive(Debug)]
#[must_use = "a part of the frame is closed only by end"]
pub struct FrameScope {
    timed: Option<GpuScope>,
    grouped: bool,
}

impl FrameScope {
    /// The timer's scope, when a timer was handed to [`begin`].
    pub fn timed(&self) -> Option<GpuScope> {
        self.timed
    }
}

/// Open a part of the frame named `label` in `encoder`: a debug group when
/// [`groups`] are on and, with a timer, a timed scope inside it (ADR 0108).
pub fn begin(
    timer: Option<&mut GpuTimer>,
    encoder: &mut wgpu::CommandEncoder,
    label: &'static str,
) -> FrameScope {
    let grouped = groups();
    if grouped {
        encoder.push_debug_group(label);
    }
    FrameScope {
        timed: timer.map(|t| t.begin(encoder, label)),
        grouped,
    }
}

/// Close `scope` in the encoder [`begin`] opened it in.
pub fn end(timer: Option<&mut GpuTimer>, encoder: &mut wgpu::CommandEncoder, scope: FrameScope) {
    if let (Some(timer), Some(timed)) = (timer, scope.timed) {
        timer.end(encoder, timed);
    }
    if scope.grouped {
        encoder.pop_debug_group();
    }
}

/// A debug group round the draws of one part of a render pass, popped on
/// [`close`](PassGroup::close). Nothing when [`groups`] are off.
#[must_use = "a pass group is popped only by close"]
pub struct PassGroup(bool);

impl PassGroup {
    pub fn open(pass: &mut wgpu::RenderPass<'_>, label: &str) -> Self {
        let on = groups();
        if on {
            pass.push_debug_group(label);
        }
        PassGroup(on)
    }

    pub fn close(self, pass: &mut wgpu::RenderPass<'_>) {
        if self.0 {
            pass.pop_debug_group();
        }
    }
}

/// The buffer draw markers are written into, kept for a pass, so naming
/// its draws allocates at most once (and only at [`GpuLabels::Draws`]:
/// callers ask [`draws`] once a pass and make no marker when it's off).
#[derive(Default)]
pub struct DrawLabel(String);

impl DrawLabel {
    /// Mark the next draw in `pass` with `name`.
    pub fn mark(&mut self, pass: &mut wgpu::RenderPass<'_>, name: std::fmt::Arguments<'_>) {
        use std::fmt::Write;
        self.0.clear();
        let _ = self.0.write_fmt(name);
        pass.insert_debug_marker(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_reads_as_documented() {
        assert_eq!(GpuLabels::parse(None), None);
        assert_eq!(GpuLabels::parse(Some(" ")), None);
        assert_eq!(GpuLabels::parse(Some("0")), Some(GpuLabels::Off));
        assert_eq!(GpuLabels::parse(Some("OFF")), Some(GpuLabels::Off));
        assert_eq!(GpuLabels::parse(Some("groups")), Some(GpuLabels::Groups));
        assert_eq!(GpuLabels::parse(Some("1")), Some(GpuLabels::Draws));
        assert_eq!(GpuLabels::parse(Some("draws")), Some(GpuLabels::Draws));
    }

    #[test]
    fn a_capture_tool_names_draws_unless_told_otherwise() {
        assert_eq!(default_level(None, Some("RenderDoc")), GpuLabels::Draws);
        assert_eq!(default_level(Some("0"), Some("RenderDoc")), GpuLabels::Off);
        assert_eq!(
            default_level(Some("groups"), Some("PIX")),
            GpuLabels::Groups
        );
        let alone = default_level(None, None);
        if cfg!(target_arch = "wasm32") {
            assert_eq!(alone, GpuLabels::Off);
        } else {
            assert_eq!(alone, GpuLabels::Groups);
        }
    }

    #[test]
    fn levels_order_so_draws_include_groups() {
        assert!(GpuLabels::Draws > GpuLabels::Groups);
        assert!(GpuLabels::Groups > GpuLabels::Off);
    }
}
