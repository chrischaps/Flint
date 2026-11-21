//! The per-entity variant table, and what a cap left out (#740).
//!
//! A per-entity style draws each mask id with a row of settings: row 0 the
//! whole frame's, then one a [`StyleVariant`](crate::style_mask::StyleVariant),
//! with a slot a mask id saying which row it takes. ASCII kept its table
//! in two storage buffers that grow with the frame (#336); pencil,
//! watercolor and user styles kept fixed arrays in their uniforms (64, 16
//! and 16 rows, 256 ids), and so did the ownership pass (256 ids), each
//! dropping what didn't fit, most of them silently. They all use
//! [`VariantTable`] now, and the owner tables a [`GrowingStorage`]: one
//! read-only storage array that grows to the next power of two a frame
//! needs and never shrinks, so a still scene writes the same buffers and
//! makes no bind group, and a crowd of 300 props on object grids keeps a
//! row and an owner for every prop.
//!
//! In WGSL a table is two bindings, read bounds-checked:
//!
//! ```wgsl
//! @group(0) @binding(1) var<storage, read> rows: array<Row>;
//! @group(0) @binding(2) var<storage, read> slots: array<u32>;
//! fn slot_of(id: u32) -> u32 {
//!     if id >= arrayLength(&slots) { return 0u; }
//!     return slots[id];
//! }
//! ```
//!
//! A cap that remains (ASCII's tone-statistics width, its dissolve bands,
//! snap grids and fonts, the papers and strips a frame stacks) reports what it
//! drops through [`report_drop`]: one warning per style and cap, naming the
//! first entity past it, kept in [`dropped`] for hosts to show
//! (`flint render --report json`'s `style_drops`).

use crate::gpu_counts::{CountedDevice, CountedQueue};
use std::sync::Mutex;

/// Entries a table starts with; it doubles past them.
pub const MIN_ENTRIES: usize = 64;

/// The entries a buffer of `capacity` must hold for `needed`: as it is
/// while they fit, else the next power of two (at least [`MIN_ENTRIES`]).
pub fn grown_capacity(capacity: usize, needed: usize) -> usize {
    if needed <= capacity {
        capacity
    } else {
        needed.next_power_of_two().max(MIN_ENTRIES)
    }
}

/// A read-only fragment storage buffer at `binding`, as every table is.
pub fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// One storage array of `stride`-byte entries that grows with what a frame
/// writes and never shrinks. Its [`generation`](Self::generation) moves
/// when the buffer is remade, which is what a bind group over it is kept
/// under.
pub struct GrowingStorage {
    label: String,
    stride: usize,
    buffer: wgpu::Buffer,
    capacity: usize,
    generation: u64,
}

impl GrowingStorage {
    /// A buffer of [`MIN_ENTRIES`] entries of `stride` bytes.
    pub fn new(device: &wgpu::Device, label: &str, stride: usize) -> Self {
        assert!(
            stride > 0 && stride.is_multiple_of(4),
            "a storage stride of {stride} bytes"
        );
        Self {
            label: label.to_string(),
            stride,
            buffer: Self::make(device, label, stride * MIN_ENTRIES),
            capacity: MIN_ENTRIES,
            generation: 0,
        }
    }

    fn make(device: &wgpu::Device, label: &str, bytes: usize) -> wgpu::Buffer {
        device.create_buffer_counted(&wgpu::BufferDescriptor {
            label: Some(label),
            size: bytes as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Remade bigger, empty, when `entries` don't fit. Returns whether it was.
    pub fn reserve(&mut self, device: &wgpu::Device, entries: usize) -> bool {
        let capacity = grown_capacity(self.capacity, entries);
        if capacity == self.capacity {
            return false;
        }
        self.buffer = Self::make(device, &self.label, self.stride * capacity);
        self.capacity = capacity;
        self.generation += 1;
        true
    }

    /// Write `bytes` (whole entries) from the start, growing first.
    pub fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8]) {
        debug_assert!(bytes.len().is_multiple_of(self.stride));
        self.reserve(device, bytes.len() / self.stride);
        if !bytes.is_empty() {
            queue.write_buffer_counted(&self.buffer, 0, bytes);
        }
    }

    /// Write `words` (a 4-byte stride) and `fill` in every entry past them,
    /// so an entry an earlier frame wrote reads `fill` again.
    pub fn write_words(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        words: &[u32],
        fill: u32,
    ) {
        debug_assert_eq!(self.stride, 4);
        self.reserve(device, words.len());
        let mut all = Vec::with_capacity(self.capacity);
        all.extend_from_slice(words);
        all.resize(self.capacity, fill);
        queue.write_buffer_counted(&self.buffer, 0, bytemuck::cast_slice(&all));
    }

    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    /// The entries the buffer holds.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Bumped each time the buffer is remade.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// A style's variant table: its rows (one a variant, row 0 the whole
/// frame's, laid out as the style's shader declares a row) and a slot for
/// every mask id. Both grow with the frame (see the module).
pub struct VariantTable {
    rows: GrowingStorage,
    slots: GrowingStorage,
}

impl VariantTable {
    /// A table whose rows are `row_size` bytes, buffers named after `label`.
    pub fn new(device: &wgpu::Device, label: &str, row_size: usize) -> Self {
        Self {
            rows: GrowingStorage::new(device, &format!("{label} Variant Rows"), row_size),
            slots: GrowingStorage::new(device, &format!("{label} Variant Slots"), 4),
        }
    }

    /// Write this frame's `rows` and `slots` (indexed by mask id), growing
    /// either buffer first; every slot past `slots` reads 0.
    pub fn upload<R: bytemuck::Pod>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rows: &[R],
        slots: &[u32],
    ) {
        self.upload_bytes(device, queue, bytemuck::cast_slice(rows), slots);
    }

    /// [`upload`](Self::upload) of rows already laid out as bytes.
    pub fn upload_bytes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rows: &[u8],
        slots: &[u32],
    ) {
        self.rows.write(device, queue, rows);
        self.slots.write_words(device, queue, slots, 0);
    }

    pub fn rows(&self) -> &wgpu::Buffer {
        self.rows.buffer()
    }

    pub fn slots(&self) -> &wgpu::Buffer {
        self.slots.buffer()
    }

    /// The rows the buffer holds (ASCII's tone statistics give each a column).
    pub fn row_capacity(&self) -> usize {
        self.rows.capacity()
    }

    /// Moves when either buffer is remade: the key of a bind group over them.
    pub fn generation(&self) -> u64 {
        self.rows.generation() + self.slots.generation()
    }

    /// The table's two bind group entries, rows at `binding` and slots at
    /// `binding + 1`.
    pub fn entries(&self, binding: u32) -> [wgpu::BindGroupEntry<'_>; 2] {
        [
            wgpu::BindGroupEntry {
                binding,
                resource: self.rows().as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: binding + 1,
                resource: self.slots().as_entire_binding(),
            },
        ]
    }
}

/// Something a style's cap left out (#740): the first entity past it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Dropped {
    /// The style that dropped it.
    pub style: String,
    /// What the cap counts (`"dissolve bands"`, `"variants"`...).
    pub cap: String,
    /// How many it holds.
    pub limit: usize,
    /// The first entity past it (an entity of the id that didn't fit).
    pub entity: String,
    /// How that entity is drawn instead.
    pub instead: String,
}

impl Dropped {
    /// The warning's text.
    pub fn message(&self) -> String {
        format!(
            "{}: `{}` is past the {} {} one frame holds, so it is {}",
            self.style, self.entity, self.limit, self.cap, self.instead
        )
    }
}

static DROPPED: Mutex<Vec<Dropped>> = Mutex::new(Vec::new());

/// Say that `style`'s cap on `cap` (holding `limit`) left `entity` out,
/// drawn `instead`. Warns once per style and cap until [`forget_dropped`]
/// (a scene binding calls it), naming the first entity past it.
pub fn report_drop(style: &str, cap: &str, limit: usize, entity: &str, instead: &str) {
    let mut dropped = DROPPED.lock().unwrap_or_else(|e| e.into_inner());
    if dropped.iter().any(|d| d.style == style && d.cap == cap) {
        return;
    }
    let d = Dropped {
        style: style.to_string(),
        cap: cap.to_string(),
        limit,
        entity: entity.to_string(),
        instead: instead.to_string(),
    };
    tracing::warn!("{}", d.message());
    dropped.push(d);
}

/// Every cap that dropped something since the last [`forget_dropped`], in
/// the order they were first met.
pub fn dropped() -> Vec<Dropped> {
    DROPPED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Start over: the next scene warns about its own drops.
pub fn forget_dropped() {
    DROPPED.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_holds_until_it_must_double() {
        assert_eq!(grown_capacity(64, 10), 64);
        assert_eq!(grown_capacity(64, 64), 64);
        assert_eq!(grown_capacity(64, 65), 128);
        assert_eq!(grown_capacity(128, 300), 512);
        assert_eq!(grown_capacity(0, 1), MIN_ENTRIES);
    }

    #[test]
    fn a_cap_warns_once_naming_the_first_entity_past_it() {
        // Process-wide: names of their own so no other test meets them
        let style = "utest_variant_table";
        report_drop(style, "bands", 8, "barrel", "faded instead");
        report_drop(style, "bands", 8, "cask", "faded instead");
        report_drop(style, "grids", 8, "crate", "cut by pixel");
        let mine: Vec<Dropped> = dropped().into_iter().filter(|d| d.style == style).collect();
        assert_eq!(mine.len(), 2, "{mine:?}");
        assert_eq!(mine[0].entity, "barrel", "the first past the cap is named");
        assert_eq!(
            mine[0].message(),
            "utest_variant_table: `barrel` is past the 8 bands one frame holds, so it is \
             faded instead"
        );
        assert_eq!(mine[1].cap, "grids");
    }
}
