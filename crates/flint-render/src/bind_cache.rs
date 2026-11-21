//! Bind groups and uniform slots a frame used to make again every frame
//! (#139). On the web each `createBindGroup` and `writeBuffer` crosses
//! from wasm into JS and costs far more than it does natively, so what a
//! frame binds is made once and kept for as long as what it binds.
//!
//! A [`BindCache`] lives inside the thing whose resources its bind groups
//! point at (the post-process chain's [`PostProcessResources`], rebuilt on
//! every resize), so it is dropped with them and never binds a stale view.
//! A bind group over something owned elsewhere (a shadow map, a fog map, a
//! buffer that grows, the caller's depth view) is keyed on that object
//! itself ([`BindCache::get_bound`], [`DepthKey`]): since wgpu 24 every
//! handle is `Clone`, and wgpu 30 compares and hashes them by identity, so
//! the cache holds a clone beside the group and makes the group again only
//! when it's handed another object (#960). On wgpu 23 those keys were
//! proxies (an id taken when the owner was made, a size, a depth id the
//! caller had to pass), and a bare depth view's groups were made every
//! frame.
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

use crate::gpu_counts::{CountedDevice, CountedQueue};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// A process-wide id for a cache as a writer ([`BindCache::first_write`]).
fn writer_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Whose depth view a frame's passes bind (#379, #960).
///
/// A bind group over the chain's own depth lives as long as the cache. One
/// over the caller's is kept for as long as the caller hands the same view:
/// wgpu 30 compares views by identity (natively wgpu-core's id, an index
/// with an epoch it never hands out again; on the web a counter), so a new
/// depth texture is a new view and its groups are made again, the old
/// view's dropped. wgpu 23 gave a view no identity, so the caller vouched
/// for it with an id, and a bare view's groups were made every frame.
#[derive(Clone, Copy, Debug)]
pub enum DepthKey<'a> {
    /// The chain's own (the render-scale path's scaled depth), which lives
    /// as long as the cache.
    Owned,
    /// The caller's view.
    Caller(&'a wgpu::TextureView),
}

/// The depth buffer a host hands [`SceneRenderer::encode_to`]. Bind groups
/// over it are kept for as long as the host hands the same view (#379,
/// #960); a bare `&TextureView` converts to one.
///
/// [`SceneRenderer::encode_to`]: crate::SceneRenderer::encode_to
#[derive(Clone, Copy)]
pub struct DepthTarget<'a> {
    pub view: &'a wgpu::TextureView,
}

impl<'a> DepthTarget<'a> {
    pub fn new(view: &'a wgpu::TextureView) -> Self {
        Self { view }
    }
}

impl<'a> From<&'a wgpu::TextureView> for DepthTarget<'a> {
    fn from(view: &'a wgpu::TextureView) -> Self {
        Self { view }
    }
}

/// A GPU object a bind group binds that its cache doesn't own, which
/// [`BindCache::get_bound`] keys the group on by identity (#960).
#[derive(Clone, Copy, Debug)]
pub enum Bound<'a> {
    View(&'a wgpu::TextureView),
    Buffer(&'a wgpu::Buffer),
    Sampler(&'a wgpu::Sampler),
}

/// A [`Bound`] the cache holds beside the group made over it. Holding it
/// keeps nothing alive that the group doesn't: a bind group holds what it
/// binds.
#[derive(Debug)]
enum Held {
    View(wgpu::TextureView),
    Buffer(wgpu::Buffer),
    Sampler(wgpu::Sampler),
}

impl Held {
    fn of(bound: &Bound<'_>) -> Self {
        match *bound {
            Bound::View(v) => Held::View(v.clone()),
            Bound::Buffer(b) => Held::Buffer(b.clone()),
            Bound::Sampler(s) => Held::Sampler(s.clone()),
        }
    }

    /// The same object: wgpu 30's handles compare by identity.
    fn is(&self, bound: &Bound<'_>) -> bool {
        match (self, bound) {
            (Held::View(a), Bound::View(b)) => a == *b,
            (Held::Buffer(a), Bound::Buffer(b)) => a == *b,
            (Held::Sampler(a), Bound::Sampler(b)) => a == *b,
            _ => false,
        }
    }
}

/// A kept group and the objects it was made over.
struct Kept {
    over: Vec<Held>,
    group: Arc<wgpu::BindGroup>,
}

impl Kept {
    fn made_over(&self, over: &[Bound<'_>]) -> bool {
        self.over.len() == over.len() && self.over.iter().zip(over).all(|(h, b)| h.is(b))
    }
}

type Groups = HashMap<(&'static str, u64), Kept>;

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

/// Bind groups by a name and a variant (which views were chosen), and by
/// the identity of what they bind that the cache doesn't own
/// ([`get_bound`](Self::get_bound)), made on first use; see the module docs.
pub struct BindCache {
    groups: Mutex<Groups>,
    /// Groups over the caller's depth view, and the view they were made
    /// over (`None`: none yet).
    caller: Mutex<(Option<wgpu::TextureView>, Groups)>,
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
            writer: AtomicU64::new(writer_id()),
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
    /// Everything it binds must live as long as the cache (the chain's own
    /// targets); one over something owned elsewhere is
    /// [`get_bound`](Self::get_bound).
    pub fn get(
        &self,
        name: &'static str,
        variant: u64,
        make: impl FnOnce() -> wgpu::BindGroup,
    ) -> Arc<wgpu::BindGroup> {
        self.get_bound(name, variant, &[], make)
    }

    /// The bind group `name` in `variant` over `over` (what it binds that
    /// the cache doesn't own: a shadow map, a fog map, a buffer that grows),
    /// made with `make` the first time and again whenever one of `over` is
    /// another object than the group was made over (#960). The new group
    /// takes the old one's place, so an object made again leaves no group
    /// behind over the old one.
    pub fn get_bound(
        &self,
        name: &'static str,
        variant: u64,
        over: &[Bound<'_>],
        make: impl FnOnce() -> wgpu::BindGroup,
    ) -> Arc<wgpu::BindGroup> {
        let mut groups = self.groups.lock().unwrap_or_else(|e| e.into_inner());
        self.kept_in(&mut groups, name, variant, over, make)
    }

    /// The bind group `name` in `variant` over the frame's depth view (and
    /// `over`, as [`get_bound`](Self::get_bound)), kept as long as the view
    /// lives (#379, #960): with the cache when the chain owns it, and while
    /// the caller hands the same view when it's the caller's. A new caller
    /// view drops every group over the old one, so an old depth texture
    /// isn't held alive.
    pub fn get_over(
        &self,
        depth: DepthKey<'_>,
        name: &'static str,
        variant: u64,
        over: &[Bound<'_>],
        make: impl FnOnce() -> wgpu::BindGroup,
    ) -> Arc<wgpu::BindGroup> {
        match depth {
            DepthKey::Owned => self.get_bound(name, variant, over, make),
            DepthKey::Caller(view) => {
                let mut caller = self.caller.lock().unwrap_or_else(|e| e.into_inner());
                if caller.0.as_ref() != Some(view) {
                    caller.0 = Some(view.clone());
                    caller.1.clear();
                }
                self.kept_in(&mut caller.1, name, variant, over, make)
            }
        }
    }

    fn kept_in(
        &self,
        groups: &mut Groups,
        name: &'static str,
        variant: u64,
        over: &[Bound<'_>],
        make: impl FnOnce() -> wgpu::BindGroup,
    ) -> Arc<wgpu::BindGroup> {
        match groups.entry((name, variant)) {
            std::collections::hash_map::Entry::Occupied(mut kept) => {
                if !kept.get().made_over(over) {
                    kept.insert(Kept {
                        over: over.iter().map(Held::of).collect(),
                        group: self.make(make),
                    });
                }
                kept.get().group.clone()
            }
            std::collections::hash_map::Entry::Vacant(slot) => slot
                .insert(Kept {
                    over: over.iter().map(Held::of).collect(),
                    group: self.make(make),
                })
                .group
                .clone(),
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
        caller.0 = None;
        caller.1.clear();
        drop(caller);
        // A new writer: every first write is due again
        self.writer.store(writer_id(), Ordering::Relaxed);
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
        let mut drop_gone = |groups: &mut Groups| {
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
                    buffer: Arc::new(device.create_buffer_counted(&wgpu::BufferDescriptor {
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
                queue.write_buffer_counted(
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
        let staging = device.create_buffer_counted(&wgpu::BufferDescriptor {
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
            device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &layout,
                entries: &[],
            })
        };
        let depth = depth_view(&device);
        // A frame's binds: a built-in's, then the style's (the depth group
        // over the caller's depth, as `depth_bind` keeps it)
        let draw = |cache: &BindCache, names: &[&'static str; 4]| {
            cache.evict_retired();
            cache.get("bloom 427", 0, group);
            for (i, &name) in names.iter().enumerate() {
                if i == 1 {
                    cache.get_over(DepthKey::Caller(&depth), name, 0, &[], group);
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
    fn writer_ids_are_never_reused() {
        let a = writer_id();
        let b = writer_id();
        assert_ne!(a, b);
    }

    fn depth_view(device: &wgpu::Device) -> wgpu::TextureView {
        device
            .create_texture_counted(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 4,
                    height: 4,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default())
    }

    fn uniform(device: &wgpu::Device) -> wgpu::Buffer {
        device.create_buffer_counted(&wgpu::BufferDescriptor {
            label: None,
            size: 256,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        })
    }

    /// #960: a group over an object owned elsewhere is kept while it's
    /// handed the same object, a clone included, made again when handed
    /// another (what a size or an id taken at creation stood for on wgpu
    /// 23), and the new group takes the old one's place.
    #[test]
    fn a_bound_group_follows_the_objects_it_binds() {
        let Some((device, _queue)) = device() else {
            crate::gpu_select::skip_without_gpu("no GPU adapter");
            return;
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[],
        });
        let group = || {
            device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &layout,
                entries: &[],
            })
        };
        let cache = BindCache::default();
        let (a, b) = (uniform(&device), uniform(&device));
        let first = cache.get_bound("shadow 960", 0, &[Bound::Buffer(&a)], group);
        let again = cache.get_bound("shadow 960", 0, &[Bound::Buffer(&a.clone())], group);
        assert!(Arc::ptr_eq(&first, &again), "a clone is the same buffer");
        assert_eq!(cache.made(), 1);

        // A buffer made again, as a grown one is: one new group, in the
        // old one's place
        let grown = cache.get_bound("shadow 960", 0, &[Bound::Buffer(&b)], group);
        assert!(!Arc::ptr_eq(&first, &grown));
        assert_eq!((cache.made(), cache.len()), (2, 1));
        // Another kind of object in the same place is another object too
        let view = depth_view(&device);
        cache.get_bound("shadow 960", 0, &[Bound::View(&view)], group);
        assert_eq!((cache.made(), cache.len()), (3, 1));

        // The caller's depth: kept frame to frame for one view, the old
        // view's groups dropped for a new one
        let (d1, d2) = (depth_view(&device), depth_view(&device));
        for _ in 0..3 {
            cache.get_over(DepthKey::Caller(&d1), "ssao 960", 0, &[], group);
            cache.get_over(
                DepthKey::Caller(&d1),
                "mask 960",
                0,
                &[Bound::Buffer(&a)],
                group,
            );
        }
        assert_eq!((cache.made(), cache.len()), (5, 3));
        cache.get_over(DepthKey::Caller(&d2), "ssao 960", 0, &[], group);
        assert_eq!(
            (cache.made(), cache.len()),
            (6, 2),
            "the first view's groups went with it"
        );
        let first_view_again = d1.clone();
        cache.get_over(
            DepthKey::Caller(&first_view_again),
            "ssao 960",
            0,
            &[],
            group,
        );
        assert_eq!(cache.made(), 7, "made again: the old groups were dropped");
    }
}
