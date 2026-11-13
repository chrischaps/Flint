//! Pipelines kept across renderers on one device (#911).
//!
//! `flint goldens` draws every case in one process on one device (#329),
//! but each case builds its own `SceneRenderer`, and building one built
//! every pipeline it draws with again: the scene's (PBR, skinned, ocean,
//! grass...), the post-process chain's and each style's. Compiling those
//! was most of a case's time (67% on Vulkan, 88% on DX12 with DXC, 97% with
//! FXC, measured on an RTX 4070 debug build), and ASCII's glyph pipeline
//! was compiled 41 times a run.
//!
//! With the cache on ([`set_enabled`]), what a renderer builds through
//! [`shared`] is kept under what it was built from, and the next renderer
//! on the same device takes it instead of building it again. Only
//! immutable objects are kept: pipelines, with their layouts, shader
//! modules and constant meshes. Everything a frame writes (uniform buffers,
//! history textures, bind groups, a style's state) stays the renderer's,
//! so nothing one case draws can reach the next.
//!
//! A key names everything the object depends on that can change in one
//! process: the device, the type, a label and the caller's parts (formats,
//! sample counts, and for a style the hash of its WGSL text, so a shader
//! swapped by `FLINT_STYLE_SHADER_DIR` or a user style's edit is a new
//! key, never a stale pipeline). Code that is fixed for the binary (an
//! `include_str!` shader, a layout written in Rust) needs no key of its
//! own.
//!
//! Bind groups a later renderer makes still fit a kept pipeline: wgpu
//! pools bind group layouts by their entries (and WebGPU calls equal
//! layouts the same), so a layout made again equal is the one the kept
//! pipeline holds.
//!
//! Off by default, so a host that doesn't ask (the player, the viewer,
//! `flint render`) builds as it always did. [`counts`] is kept either way:
//! how many of each label were built and how many reused.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

static ENABLED: AtomicBool = AtomicBool::new(false);

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

#[derive(Default)]
struct Cache {
    kept: HashMap<Key, Arc<dyn Any + Send + Sync>>,
    counts: BTreeMap<String, Count>,
}

#[derive(Hash, PartialEq, Eq)]
struct Key {
    device: usize,
    ty: TypeId,
    label: String,
    parts: String,
}

/// How many objects of one label were built, and how many times one was
/// taken from the cache instead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Count {
    pub built: u64,
    pub reused: u64,
}

fn cache() -> MutexGuard<'static, Option<Cache>> {
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Keep what [`shared`] builds for the next renderer on the same device
/// (`flint goldens` in one process), or stop and forget it all.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    if !on {
        clear();
    }
}

/// Whether [`shared`] keeps what it builds.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Forget every kept object (the counts stay). A host calls it when it
/// lets its device go: what was kept holds that device alive, and a new
/// device must never be handed the old one's pipelines.
pub fn clear() {
    if let Some(cache) = cache().as_mut() {
        cache.kept.clear();
    }
}

/// How many of each label were built and reused since the last
/// [`reset_counts`], by label.
pub fn counts() -> BTreeMap<String, Count> {
    cache()
        .as_ref()
        .map(|c| c.counts.clone())
        .unwrap_or_default()
}

/// Start the counts again.
pub fn reset_counts() {
    if let Some(cache) = cache().as_mut() {
        cache.counts.clear();
    }
}

/// The device's address: wgpu 23 gives a device no identity of its own.
/// A device that moves only misses; one made where a dropped one was is
/// why a host calls [`clear`] when it lets its device go.
fn device_id(device: &wgpu::Device) -> usize {
    device as *const wgpu::Device as usize
}

/// Count one `label` built where nothing could be kept.
pub(crate) fn built(label: &str) {
    count(label, false);
}

fn count(label: &str, reused: bool) {
    let mut guard = cache();
    let count = guard
        .get_or_insert_with(Cache::default)
        .counts
        .entry(label.to_string())
        .or_default();
    if reused {
        count.reused += 1;
    } else {
        count.built += 1;
    }
}

/// The object `build` makes for `label` and `parts` on `device`: the one
/// kept from an earlier call with the same key while the cache is on,
/// else built now (and kept, if on). `parts` is everything the object
/// depends on that can differ between calls in one process, by its
/// `Debug` text. A `build` that panics keeps nothing.
pub fn shared<T: Send + Sync + 'static>(
    device: &wgpu::Device,
    label: &str,
    parts: impl Debug,
    build: impl FnOnce() -> T,
) -> Arc<T> {
    shared_some(device, label, parts, || Some(build())).expect("built")
}

/// [`shared`] for a build that may make nothing (a GPU that can't run
/// it): `None` is returned and nothing kept, so the next renderer tries
/// again.
pub fn shared_some<T: Send + Sync + 'static>(
    device: &wgpu::Device,
    label: &str,
    parts: impl Debug,
    build: impl FnOnce() -> Option<T>,
) -> Option<Arc<T>> {
    if !enabled() {
        let made = build().map(Arc::new);
        if made.is_some() {
            count(label, false);
        }
        return made;
    }
    let key = Key {
        device: device_id(device),
        ty: TypeId::of::<T>(),
        label: label.to_string(),
        parts: format!("{parts:?}"),
    };
    let kept = cache()
        .as_ref()
        .and_then(|c| c.kept.get(&key).cloned())
        .and_then(|any| any.downcast::<T>().ok());
    if let Some(kept) = kept {
        count(label, true);
        return Some(kept);
    }
    // Built with the lock released: a build may ask for what it uses
    let made = Arc::new(build()?);
    count(label, false);
    let mut guard = cache();
    let cache = guard.get_or_insert_with(Cache::default);
    cache.kept.entry(key).or_insert_with(|| made.clone());
    Some(made)
}

thread_local! {
    /// The build a [`scoped`] call belongs to, and how many it has made.
    static SCOPE: RefCell<Option<(String, u32)>> = const { RefCell::new(None) };
}

/// Run `f` as the build named `name`: a style's `create`, whose objects
/// [`scoped`] keeps by `name` and their order in it. A style builds the
/// same layouts every time it is created from the same descriptor, so its
/// n-th pipeline of a label and text has the same layouts too, which is
/// what lets [`scoped`] leave the layouts out of its key.
pub(crate) fn in_scope<R>(name: impl Debug, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<(String, u32)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let back = self.0.take();
            SCOPE.with(|s| *s.borrow_mut() = back);
        }
    }
    let before = SCOPE.with(|s| s.borrow_mut().replace((format!("{name:?}"), 0)));
    let _restore = Restore(before);
    f()
}

/// [`shared`] inside an [`in_scope`] build: keyed by the scope's name and
/// this call's place in it as well as `parts`. Outside one, it builds and
/// keeps nothing, since nothing then says the layouts are the same.
pub(crate) fn scoped<T: Send + Sync + 'static>(
    device: &wgpu::Device,
    label: &str,
    parts: impl Debug,
    build: impl FnOnce() -> T,
) -> Arc<T> {
    let place = SCOPE.with(|s| {
        s.borrow_mut().as_mut().map(|(name, next)| {
            *next += 1;
            (name.clone(), *next)
        })
    });
    match place {
        Some(place) => shared(device, label, (place, parts), build),
        None => {
            count(label, false);
            Arc::new(build())
        }
    }
}

/// A WGSL text as a key part: its SHA-256, not the text.
pub(crate) fn text_key(text: &str) -> String {
    flint_core::ContentHash::from_str(text).to_hex()
}
