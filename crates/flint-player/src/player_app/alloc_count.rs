//! Counting allocations, for tests that say a frame allocates nothing
//! (#1078). The unit tests' allocator is the system's with a counter per
//! thread, so a test counts only what its own thread does while it runs
//! a closure, whatever other tests do meanwhile.
#![cfg(test)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    // `const`: the counter itself must not allocate when first touched.
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

struct Counting;

// SAFETY: every call goes straight to `System`, which upholds the
// contract; the counter is a thread-local `Cell` that never allocates.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// What `f` returns, and how many allocations (and reallocations) this
/// thread made while it ran.
pub(crate) fn allocations_in<T>(f: impl FnOnce() -> T) -> (T, u64) {
    let before = ALLOCATIONS.with(Cell::get);
    let out = f();
    (out, ALLOCATIONS.with(Cell::get) - before)
}

#[test]
fn the_counter_sees_an_allocation_and_not_its_absence() {
    let (_, none) = allocations_in(|| 2 + 2);
    assert_eq!(none, 0);
    let (v, some) = allocations_in(|| vec![1u8; 64]);
    assert_eq!(v.len(), 64);
    assert!(some >= 1);
}
