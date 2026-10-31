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
//! [`PostProcessResources`]: crate::postprocess::PostProcessResources

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// A process-wide id for a GPU object that bind groups are cached over,
/// such as a shadow pass or a ground fog map: wgpu 23 gives its objects
/// no identity, and an address can be reused once one is dropped.
pub(crate) fn gpu_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Bind groups by a name and a variant (which views were chosen, whose
/// objects they bind), made on first use; see the module docs.
#[derive(Default)]
pub struct BindCache {
    groups: Mutex<HashMap<(&'static str, u64), Arc<wgpu::BindGroup>>>,
    /// Uniforms whose contents last as long as the cache (texel sizes),
    /// written once.
    written: Mutex<Vec<&'static str>>,
}

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
            .or_insert_with(|| Arc::new(make()))
            .clone()
    }

    /// [`get`](Self::get) when `keep`, else a bind group for this frame
    /// only: for one over a view the cache can't vouch for, such as the
    /// caller's depth buffer.
    pub fn get_if(
        &self,
        keep: bool,
        name: &'static str,
        variant: u64,
        make: impl FnOnce() -> wgpu::BindGroup,
    ) -> Arc<wgpu::BindGroup> {
        if keep {
            self.get(name, variant, make)
        } else {
            Arc::new(make())
        }
    }

    /// True the first time it's asked about `name`: for an upload whose
    /// data doesn't change while the cache lives.
    pub fn first_write(&self, name: &'static str) -> bool {
        let mut written = self.written.lock().unwrap_or_else(|e| e.into_inner());
        if written.contains(&name) {
            false
        } else {
            written.push(name);
            true
        }
    }

    /// How many bind groups it holds.
    pub fn len(&self) -> usize {
        self.groups.lock().map_or(0, |g| g.len())
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
        debug_assert!(bytes.len() as u64 <= SLOT_STRIDE);
        let Some(page) = self.pages.get_mut(slot.page as usize) else {
            return;
        };
        let at = slot.offset() as usize;
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
                .request_device(&wgpu::DeviceDescriptor::default(), None)
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
        device.poll(wgpu::Maintain::Wait);
        let bytes = staging.slice(..).get_mapped_range().to_vec();
        bytes
    }

    #[test]
    fn the_arena_pages_its_slots_and_uploads_them_together() {
        let Some((device, queue)) = device() else {
            eprintln!("no GPU adapter: skipped");
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

    #[test]
    fn ids_are_never_reused() {
        let a = gpu_id();
        let b = gpu_id();
        assert_ne!(a, b);
    }
}
