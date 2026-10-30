//! The C ABI the `AudioWorkletProcessor` calls (`flint-audio/src/worklet.js`).
//!
//! The module imports nothing, so the processor instantiates it with an
//! empty import object and needs no wasm-bindgen glue (an
//! `AudioWorkletGlobalScope` has no `TextDecoder` for it anyway). Bytes go
//! in through one scratch area: the processor asks for room with
//! [`flint_scratch`], copies into the memory there, then calls
//! [`flint_apply`] or [`flint_load`].

use std::cell::RefCell;

use crate::Mixer;

/// Scratch kept after a load larger than this is given back.
const KEEP_SCRATCH: usize = 1 << 20;

thread_local! {
    static MIXER: RefCell<Option<Mixer>> = const { RefCell::new(None) };
    /// `u32`s so a pointer into it is aligned for `f32` samples.
    static SCRATCH: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
}

fn with_mixer<T>(f: impl FnOnce(&mut Mixer) -> T) -> Option<T> {
    MIXER.with(|m| m.borrow_mut().as_mut().map(f))
}

/// Make the mixer at the context's rate. 1 when it is ready, 0 when kira
/// couldn't be set up.
#[no_mangle]
pub extern "C" fn flint_init(sample_rate: u32) -> u32 {
    let mixer = Mixer::new(sample_rate);
    let ok = mixer.is_some();
    MIXER.with(|m| *m.borrow_mut() = mixer);
    ok as u32
}

/// Room for `bytes` bytes; returns where to write them. The pointer is good
/// until the next call into the module.
#[no_mangle]
pub extern "C" fn flint_scratch(bytes: usize) -> *mut u8 {
    SCRATCH.with(|s| {
        let mut s = s.borrow_mut();
        s.resize(bytes.div_ceil(4), 0);
        s.as_mut_ptr().cast()
    })
}

fn scratch_bytes<T>(len: usize, f: impl FnOnce(&[u8]) -> T) -> T {
    SCRATCH.with(|s| {
        let s = s.borrow();
        let bytes: &[u8] = as_bytes(&s);
        f(&bytes[..len.min(bytes.len())])
    })
}

fn as_bytes(words: &[u32]) -> &[u8] {
    // SAFETY: u32 has no padding, and u8's alignment is 1.
    unsafe { std::slice::from_raw_parts(words.as_ptr().cast(), words.len() * 4) }
}

/// Apply the command batch of `len` bytes in scratch.
#[no_mangle]
pub extern "C" fn flint_apply(len: usize) {
    scratch_bytes(len, |batch| with_mixer(|m| m.apply(batch)));
}

/// Keep the sound of `samples` f32s in scratch under `id`.
#[no_mangle]
pub extern "C" fn flint_load(id: u32, sample_rate: u32, channels: u32, samples: usize) {
    SCRATCH.with(|s| {
        let mut s = s.borrow_mut();
        let n = samples.min(s.len());
        // SAFETY: f32 and u32 share size and alignment; every bit pattern
        // is a valid f32.
        let floats = unsafe { std::slice::from_raw_parts(s.as_ptr().cast::<f32>(), n) };
        with_mixer(|m| m.load_sound(id, sample_rate, channels, floats));
        if s.capacity() > KEEP_SCRATCH / 4 {
            *s = Vec::new();
        }
    });
}

/// Mix `frames` frames; returns where they are, left channel then right.
#[no_mangle]
pub extern "C" fn flint_render(frames: usize) -> *const f32 {
    with_mixer(|m| m.render(frames).as_ptr()).unwrap_or(std::ptr::null())
}

/// Commands that failed so far.
#[no_mangle]
pub extern "C" fn flint_errors() -> u32 {
    with_mixer(|m| m.errors).unwrap_or(0)
}
