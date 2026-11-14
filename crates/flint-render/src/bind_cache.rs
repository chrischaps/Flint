//! Bind groups and uniform slots a frame used to make again every frame
//! (#139). On the web each `createBindGroup` and `writeBuffer` crosses
//! from wasm into JS and costs far more than it does natively, so what a
//! frame binds is made once and kept for as long as what it binds.
//!
//! A [`BindCache`] lives inside the thing whose resources its bind groups
//! point at (the post-process chain's [`PostProcessResources`], rebuilt on
//! every resize), so it is dropped with them and never binds a stale view.
//! A bind group over something owned elsewhere carries that owner's
//! [`gpu_id`] in its key.
//!
//! A [`TransformArena`] holds the per-draw transform uniforms of cached
//! draws in pages of 256-byte slots: each draw's bind group binds its own
//! slot, and a frame uploads each page in one write instead of a write per
//! draw. Slots never move, so a bind group made for one stays good until
//! the slot is freed.
//!
//! A name can outlive what it binds while the cache lives: a user style
//! replaced by a `.shader.toml` edit (#275) caches its new pass's groups
//! under new names, and the old ones would wait for the next resize. The
//! old names are [`retire`]d into a process-wide log, and each cache drops
//! them at its next [`evict_retired`](BindCache::evict_retired) (#427), so
//! a long editing session holds one set of groups per style, not one per
//! save, in the window's chain and in every comic panel's alike.
//!
//! [`PostProcessResources`]: crate::postprocess::PostProcessResources

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// A process-wide id for a GPU object that bind groups are cached over,
/// such as a shadow pass or a ground fog map: wgpu 23 gives its objects
/// no identity, and an address can be reused once one is dropped.
pub(crate) fn gpu_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Who vouches for the depth view a frame's passes bind (#379).
///
/// wgpu 23 gives a `TextureView` no identity, so a bind group over the
/// caller's depth buffer could only be kept when the caller says how long
/// that view lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepthKey {
    /// The chain's own (the render-scale path's scaled depth), which lives
    /// as long as the cache.
    Owned,
    /// The caller's, under an id it changes whenever it makes a new depth
    /// texture ([`new_depth_id`]; `RenderContext::depth_id`).
    Caller(u64),
    /// The caller's, with no id: bind groups over it last one frame.
    Frame,
}

impl DepthKey {
    /// The key for the caller's view under `id` (`None`: no id).
    pub fn caller(id: Option<u64>) -> Self {
        id.map_or(DepthKey::Frame, DepthKey::Caller)
    }
}

/// A fresh id for a depth texture a host hands the renderer
/// ([`DepthKey::Caller`]): take a new one each time the texture is made.
pub fn new_depth_id() -> u64 {
    gpu_id()
}

/// The depth buffer a host hands [`SceneRenderer::encode_to`], with the id
/// that lets the renderer keep bind groups over it (#379). A bare
/// `&TextureView` converts to one with no id, whose bind groups are made
/// every frame; `RenderContext::depth_target` carries its id.
///
/// [`SceneRenderer::encode_to`]: crate::SceneRenderer::encode_to
#[derive(Clone, Copy)]
pub struct DepthTarget<'a> {
    pub view: &'a wgpu::TextureView,
    /// A [`new_depth_id`] taken when the view's texture was made, changed
    /// whenever it is made again. Reusing one for a new texture would bind
    /// the old texture.
    pub id: Option<u64>,
}

impl<'a> DepthTarget<'a> {
    pub fn new(view: &'a wgpu::TextureView, id: u64) -> Self {
        Self { view, id: Some(id) }
    }
}

impl<'a> From<&'a wgpu::TextureView> for DepthTarget<'a> {
    fn from(view: &'a wgpu::TextureView) -> Self {
        Self { view, id: None }
    }
}

/// Bind group names retired for good (#427), each with its place in the
/// log, so a cache asks only for what it hasn't dropped yet.
struct Retired {
    /// The place the next retired name takes.
    next: u64,
    /// The last [`RETIRED_KEPT`] names, oldest first.
    names: VecDeque<(u64, &'static str)>,
}

static RETIRED: Mutex<Retired> = Mutex::new(Retired {
    next: 0,
    names: VecDeque::new(),
});

/// [`Retired::next`], read without the lock: a frame with nothing newly
/// retired costs one atomic load.
static RETIRED_NEXT: AtomicU64 = AtomicU64::new(0);

/// Names the log keeps. A cache that falls further behind (a comic panel
/// not drawn through dozens of saves) can't know which names it missed,
/// so it forgets everything, as a resize would.
const RETIRED_KEPT: usize = 256;

/// Retire `names` in every cache in the process: what they bound is gone
/// and nothing will ask for them again (a replaced user style's
/// per-install names, #427). Each cache drops them at its next
/// [`BindCache::evict_retired`].
pub(crate) fn retire(names: impl IntoIterator<Item = &'static str>) {
    let mut log = RETIRED.lock().unwrap_or_else(|e| e.into_inner());
    for name in names {
        let at = log.next;
        log.names.push_back((at, name));
        log.next += 1;
    }
    while log.names.len() > RETIRED_KEPT {
        log.names.pop_front();
    }
    RETIRED_NEXT.store(log.next, Ordering::Release);
}

type CallerGroups = (u64, HashMap<(&'static str, u64), Arc<wgpu::BindGroup>>);

/// Bind groups by a name and a variant (which views were chosen, whose
/// objects they bind), made on first use; see the module docs.
pub struct BindCache {
    groups: Mutex<HashMap<(&'static str, u64), Arc<wgpu::BindGroup>>>,
    /// Groups over the caller's depth view, and the depth id they were
    /// made for (0: none yet; ids start at 1).
    caller: Mutex<CallerGroups>,
    /// Bind groups made through it, kept or not: what a frame costs the
    /// web in `createBindGroup`s, for tests.
    made: AtomicU64,
    /// Who this cache is to [`first_write`](Self::first_write): a
    /// [`gpu_id`], a new one on [`clear`](Self::clear).
    writer: AtomicU64,
    /// How far into the [`retire`] log this cache has dropped names.
    retired_seen: AtomicU64,
}

impl Default for BindCache {
    fn default() -> Self {
        Self {
            groups: Default::default(),
            caller: Default::default(),
            made: AtomicU64::new(0),
            writer: AtomicU64::new(gpu_id()),
            // Names retired before it was made were never in it
            retired_seen: AtomicU64::new(RETIRED_NEXT.load(Ordering::Acquire)),
        }
    }
}

/// The cache that last wrote each write-once uniform (#880). The buffer is
/// the pipeline's, shared by every cache that draws through it: the
/// window's chain and each comic panel's. Process-wide, since a name is one
/// buffer per renderer and another renderer's write only costs a rewrite.
static LAST_WRITER: Mutex<Option<HashMap<&'static str, u64>>> = Mutex::new(None);

impl BindCache {
    /// The bind group `name` in `variant`, made with `make` the first time.
    pub fn get(
        &self,
        name: &'static str,
        variant: u64,
        make: impl FnOnce() -> wgpu::BindGroup,
    ) -> Arc<wgpu::BindGroup> {
        let mut groups = self.groups.lock().unwrap_or_else(|e| e.into_inner());
        groups
            .entry((name, variant))
            .or_insert_with(|| self.make(make))
            .clone()
    }

    /// The bind group `name` in `variant` over the frame's depth view,
    /// kept as long as `depth` says the view lives (#379): with the cache
    /// when the chain owns it, until the id changes when it's the caller's
    /// (the caller's groups are dropped then, so an old depth texture isn't
    /// held alive), and for this frame only when nothing vouches for it.
    pub fn get_over(
        &self,
        depth: DepthKey,
        name: &'static str,
        variant: u64,
        make: impl FnOnce() -> wgpu::BindGroup,
    ) -> Arc<wgpu::BindGroup> {
        match depth {
            DepthKey::Owned => self.get(name, variant, make),
            DepthKey::Caller(id) => {
                let mut caller = self.caller.lock().unwrap_or_else(|e| e.into_inner());
                if caller.0 != id {
                    caller.0 = id;
                    caller.1.clear();
                }
                caller
                    .1
                    .entry((name, variant))
                    .or_insert_with(|| self.make(make))
                    .clone()
            }
            DepthKey::Frame => self.make(make),
        }
    }

    fn make(&self, make: impl FnOnce() -> wgpu::BindGroup) -> Arc<wgpu::BindGroup> {
        self.made.fetch_add(1, Ordering::Relaxed);
        Arc::new(make())
    }

    /// How many bind groups it has made, kept or for one frame.
    pub fn made(&self) -> u64 {
        self.made.load(Ordering::Relaxed)
    }

    /// True when `name`'s upload is due: the first time this cache asks,
    /// and again whenever another cache asked since. For an upload whose
    /// data doesn't change while the cache lives (a texel size) into a
    /// buffer the pipeline shares, so a comic panel's chain, which writes
    /// its own size there, doesn't leave it for the window's (#880).
    pub fn first_write(&self, name: &'static str) -> bool {
        let me = self.writer.load(Ordering::Relaxed);
        let mut last = LAST_WRITER.lock().unwrap_or_else(|e| e.into_inner());
        last.get_or_insert_with(HashMap::new).insert(name, me) != Some(me)
    }

    /// Forget every group and every first write, as a new cache would
    /// (the count of groups made goes on): for when what they bind was
    /// made again (a style rebuilt from a changed shader, #418).
    pub fn clear(&self) {
        self.groups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        let mut caller = self.caller.lock().unwrap_or_else(|e| e.into_inner());
        caller.0 = 0;
        caller.1.clear();
        drop(caller);
        // A new writer: every first write is due again
        self.writer.store(gpu_id(), Ordering::Relaxed);
        // Holding nothing, it holds no retired name
        self.retired_seen
            .store(RETIRED_NEXT.load(Ordering::Acquire), Ordering::Relaxed);
    }

    /// Drop the groups whose names were [`retire`]d since this cache last
    /// asked (#427), returning how many went. Callers ask once a frame,
    /// before anything binds: with nothing new retired it is one atomic
    /// load. A cache too far behind for the log forgets everything.
    pub fn evict_retired(&self) -> usize {
        let seen = self.retired_seen.load(Ordering::Relaxed);
        if RETIRED_NEXT.load(Ordering::Acquire) == seen {
            return 0;
        }
        let log = RETIRED.lock().unwrap_or_else(|e| e.into_inner());
        let next = log.next;
        let oldest = log.names.front().map_or(next, |&(at, _)| at);
        if seen < oldest {
            drop(log);
            let held = self.len();
            self.clear();
            self.retired_seen.store(next, Ordering::Relaxed);
            return held;
        }
        let gone: HashSet<&str> = log
            .names
            .iter()
            .filter(|&&(at, _)| at >= seen)
            .map(|&(_, name)| name)
            .collect();
        drop(log);
        let mut evicted = 0;
        let mut drop_gone = |groups: &mut HashMap<(&'static str, u64), Arc<wgpu::BindGroup>>| {
            let before = groups.len();
            groups.retain(|&(name, _), _| !gone.contains(name));
            evicted += before - groups.len();
        };
        drop_gone(&mut self.groups.lock().unwrap_or_else(|e| e.into_inner()));
        drop_gone(&mut self.caller.lock().unwrap_or_else(|e| e.into_inner()).1);
        self.retired_seen.store(next, Ordering::Relaxed);
        evicted
    }

    /// How many bind groups it holds.
    pub fn len(&self) -> usize {
        self.groups.lock().map_or(0, |g| g.len()) + self.caller.lock().map_or(0, |c| c.1.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Bytes between slots: wgpu's uniform offset alignment on every backend.
pub(crate) const SLOT_STRIDE: u64 = 256;

/// Slots in a page: 32 KB a page.
pub(crate) const PAGE_SLOTS: u32 = 128;

/// One draw's place in a [`TransformArena`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ArenaSlot {
    pub page: u32,
    pub index: u32,
}

impl ArenaSlot {
    pub fn offset(self) -> u64 {
        self.index as u64 * SLOT_STRIDE
    }
}

struct Page {
    buffer: Arc<wgpu::Buffer>,
    /// The page's bytes as they'll be uploaded.
    staging: Vec<u8>,
    /// Slots written since the last upload: the first and one past the last.
    dirty: Option<(u32, u32)>,
}

/// Uniform slots for cached draws, uploaded a page at a time; see the
/// module docs.
#[derive(Default)]
pub(crate) struct TransformArena {
    pages: Vec<Page>,
    free: Vec<ArenaSlot>,
    /// Slots handed out of the last page so far.
    tail: u32,
}

impl TransformArena {
    /// A free slot, adding a page when every page is full. Returns the
    /// slot and the buffer it lives in.
    pub fn alloc(&mut self, device: &wgpu::Device) -> (ArenaSlot, Arc<wgpu::Buffer>) {
        let slot = if let Some(slot) = self.free.pop() {
            slot
        } else {
            if self.pages.is_empty() || self.tail == PAGE_SLOTS {
                let size = PAGE_SLOTS as u64 * SLOT_STRIDE;
                self.pages.push(Page {
                    buffer: Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("Transform Arena Page"),
                        size,
                        usage: wgpu::BufferUsages::UNIFORM
                            | wgpu::BufferUsages::COPY_DST
                            | wgpu::BufferUsages::COPY_SRC,
                        mapped_at_creation: false,
                    })),
                    staging: vec![0; size as usize],
                    dirty: None,
                });
                self.tail = 0;
            }
            let slot = ArenaSlot {
                page: self.pages.len() as u32 - 1,
                index: self.tail,
            };
            self.tail += 1;
            slot
        };
        (slot, self.pages[slot.page as usize].buffer.clone())
    }

    /// Give `slot` back. Its bind group must go with it.
    pub fn free(&mut self, slot: ArenaSlot) {
        self.free.push(slot);
    }

    /// Put `bytes` in `slot`, to go up with its page at [`upload`](Self::upload).
    pub fn stage(&mut self, slot: ArenaSlot, bytes: &[u8]) {
        self.stage_at(slot, 0, bytes);
    }

    /// Put `bytes` in `slot`, `offset` bytes in.
    pub fn stage_at(&mut self, slot: ArenaSlot, offset: usize, bytes: &[u8]) {
        debug_assert!((offset + bytes.len()) as u64 <= SLOT_STRIDE);
        let Some(page) = self.pages.get_mut(slot.page as usize) else {
            return;
        };
        let at = slot.offset() as usize + offset;
        page.staging[at..at + bytes.len()].copy_from_slice(bytes);
        page.dirty = Some(match page.dirty {
            Some((a, b)) => (a.min(slot.index), b.max(slot.index + 1)),
            None => (slot.index, slot.index + 1),
        });
    }

    /// Write each page's staged slots in one `write_buffer`.
    pub fn upload(&mut self, queue: &wgpu::Queue) {
        for page in &mut self.pages {
            if let Some((a, b)) = page.dirty.take() {
                let (from, to) = (a as u64 * SLOT_STRIDE, b as u64 * SLOT_STRIDE);
                queue.write_buffer(
                    &page.buffer,
                    from,
                    &page.staging[from as usize..to as usize],
                );
            }
        }
    }

    /// Slots in use.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn len(&self) -> usize {
        let made = self.pages.len().saturating_sub(1) * PAGE_SLOTS as usize + self.tail as usize;
        made - self.free.len()
    }

    /// Pages allocated.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn pages(&self) -> usize {
        self.pages.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_write_is_true_once_per_name() {
        let cache = BindCache::default();
        assert!(cache.first_write("bloom"));
        assert!(!cache.first_write("bloom"));
        assert!(cache.first_write("fxaa"));
    }

    fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
        pollster::block_on(async {
            let instance = crate::gpu_select::instance();
            let adapter = crate::gpu_select::request_adapter(&instance, None).await?;
            adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .ok()
        })
    }

    fn read(device: &wgpu::Device, queue: &wgpu::Queue, buffer: &wgpu::Buffer) -> Vec<u8> {
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: buffer.size(),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, buffer.size());
        queue.submit([encoder.finish()]);
        staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = crate::wait_for_gpu(device);
        let bytes = staging
            .slice(..)
            .get_mapped_range()
            .expect("mapped buffer range")
            .to_vec();
        bytes
    }

    #[test]
    fn the_arena_pages_its_slots_and_uploads_them_together() {
        let Some((device, queue)) = device() else {
            crate::gpu_select::skip_without_gpu("no GPU adapter");
            return;
        };
        let mut arena = TransformArena::default();
        let slots: Vec<_> = (0..PAGE_SLOTS + 2).map(|_| arena.alloc(&device)).collect();
        assert_eq!(arena.pages(), 2, "a page holds {PAGE_SLOTS}");
        assert_eq!(arena.len(), PAGE_SLOTS as usize + 2);
        assert_eq!(
            slots[PAGE_SLOTS as usize].0,
            ArenaSlot { page: 1, index: 0 }
        );
        assert!(Arc::ptr_eq(&slots[0].1, &slots[1].1), "one buffer a page");

        // A freed slot is the next one handed out, and no page is added
        let freed = slots[3].0;
        arena.free(freed);
        assert_eq!(arena.len(), PAGE_SLOTS as usize + 1);
        assert_eq!(arena.alloc(&device).0, freed);
        assert_eq!(arena.pages(), 2);

        arena.stage(slots[2].0, &[7; 16]);
        arena.stage(slots[5].0, &[9; 16]);
        arena.upload(&queue);
        let page = read(&device, &queue, &slots[0].1);
        let at = |slot: ArenaSlot| slot.offset() as usize;
        assert_eq!(&page[at(slots[2].0)..at(slots[2].0) + 16], &[7; 16]);
        assert_eq!(&page[at(slots[5].0)..at(slots[5].0) + 16], &[9; 16]);
        assert!(page[at(slots[1].0)..at(slots[1].0) + 16]
            .iter()
            .all(|&b| b == 0));
    }

    /// A user style's names for one install, as `custom::install` makes
    /// them: unique per generation, leaked.
    fn install_names(style: &str, generation: u32) -> [&'static str; 4] {
        ["input", "depth/mask", "frame", "pass targets"].map(|what| {
            &*Box::leak(format!("{style} style {what} BG #{generation}").into_boxed_str())
        })
    }

    /// #427: 100 `.shader.toml` saves leave a cache holding what it held
    /// after the first, in the window's chain and in a comic panel's that
    /// only catches up when it's drawn; another style's groups stay.
    #[test]
    fn retired_names_leave_every_cache() {
        let Some((device, _queue)) = device() else {
            crate::gpu_select::skip_without_gpu("no GPU adapter");
            return;
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[],
        });
        let group = || {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &layout,
                entries: &[],
            })
        };
        // A frame's binds: a built-in's, then the style's (the depth group
        // over the caller's depth, as `depth_bind` keeps it)
        let draw = |cache: &BindCache, names: &[&'static str; 4]| {
            cache.evict_retired();
            cache.get("bloom 427", 0, group);
            for (i, &name) in names.iter().enumerate() {
                if i == 1 {
                    cache.get_over(DepthKey::Caller(7), name, 0, group);
                } else {
                    cache.get(name, i as u64, group);
                }
            }
        };
        let (window, panel) = (BindCache::default(), BindCache::default());
        let mut names = install_names("utest_427", 0);
        draw(&window, &names);
        draw(&panel, &names);
        let held = window.len();
        assert_eq!(held, 5);
        let bloom_made = window.made();
        for save in 1..=100 {
            retire(names);
            names = install_names("utest_427", save);
            draw(&window, &names);
            assert_eq!(window.len(), held, "after save {save}");
            if save % 10 == 0 {
                // The panel is drawn now and then: it drops every save's
                // names it missed, not only the last
                draw(&panel, &names);
                assert_eq!(panel.len(), held, "panel after save {save}");
            }
        }
        // Only the style's groups were made again: bloom's was kept
        assert_eq!(window.made() - bloom_made, 4 * 100);

        // A cache behind more than the log keeps forgets everything
        let behind = BindCache::default();
        draw(&behind, &names);
        for _ in 0..=RETIRED_KEPT / 4 {
            retire(names);
            names = install_names("utest_427", 1000);
        }
        assert_eq!(behind.evict_retired(), 5, "cleared, bloom's too");
        assert!(behind.is_empty());
        assert_eq!(behind.evict_retired(), 0, "and caught up");
    }

    #[test]
    fn ids_are_never_reused() {
        let a = gpu_id();
        let b = gpu_id();
        assert_ne!(a, b);
    }
}
