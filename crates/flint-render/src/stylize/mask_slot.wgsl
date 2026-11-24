// The slot table's reader (#1256), beside `mask_read.wgsl`: which of a
// style's settings rows each mask id takes (#336, #414; the framework's
// variant table, #740). Each shader that includes this declares the table,
// at its own group and binding, under this name:
//
//     @group(G) @binding(B) var<storage, read> flint_slots: array<u32>;
//
// A style that gates the table (pencil while it has no per-entity rows,
// watercolor past its row count, ASCII sparing id 0 the read) wraps this.

// Mask id -> its slot, 0 past the table
fn flint_slot(id: u32) -> u32 {
    if (id >= arrayLength(&flint_slots)) {
        return 0u;
    }
    return flint_slots[id];
}
