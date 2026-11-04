//! Per-cell memo for values derived from a cell ID (#187).
//!
//! Per-cell variation (height, tint, roughness, noise offset) is a pure
//! function of the cell ID and a seed, drawn from a fresh [`SeededRng`]
//! (ChaCha8) per call. Seeding ChaCha8 costs far more than the rest of the
//! per-pixel work, and a texture has a few dozen cells over hundreds of
//! thousands of pixels, so the ops compute each cell's value once and look it
//! up afterwards. Results are identical to calling the function per pixel.
//!
//! [`SeededRng`]: crate::SeededRng

use std::collections::HashMap;

/// Caches `f(cell_id)` for a single op invocation.
///
/// Neighbouring pixels usually share a cell, so the last hit is checked before
/// the map.
pub(crate) struct CellMemo<T: Copy, F: Fn(u32) -> T> {
    f: F,
    last: Option<(u32, T)>,
    seen: HashMap<u32, T>,
}

impl<T: Copy, F: Fn(u32) -> T> CellMemo<T, F> {
    pub(crate) fn new(f: F) -> Self {
        Self {
            f,
            last: None,
            seen: HashMap::new(),
        }
    }

    /// The value for `cell_id`, computed on first sight.
    #[inline]
    pub(crate) fn get(&mut self, cell_id: u32) -> T {
        if let Some((id, v)) = self.last {
            if id == cell_id {
                return v;
            }
        }
        let f = &self.f;
        let v = *self.seen.entry(cell_id).or_insert_with(|| f(cell_id));
        self.last = Some((cell_id, v));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn computes_each_cell_once_and_matches_the_function() {
        let calls = Cell::new(0);
        let f = |id: u32| {
            calls.set(calls.get() + 1);
            id * 3 + 1
        };
        let mut memo = CellMemo::new(f);
        for &id in &[4, 4, 4, 7, 7, 4, 9, 7, 4] {
            assert_eq!(memo.get(id), id * 3 + 1);
        }
        assert_eq!(calls.get(), 3);
    }
}
