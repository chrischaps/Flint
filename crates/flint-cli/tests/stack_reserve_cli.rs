//! `flint` links an 8 MB main-thread stack on Windows (#990): what the
//! exe's PE header asks the loader to reserve, read off the built binary.
//! `build.rs` asks for it, so a game building the engine from its own root
//! (where the engine's `.cargo/config.toml` is never read) gets it too.
//! flint-player holds its binary to the same (`tests/stack_reserve.rs`).

#[cfg(windows)]
#[test]
fn flint_reserves_an_8_mb_main_thread_stack() {
    let exe = env!("CARGO_BIN_EXE_flint");
    let bytes = std::fs::read(exe).unwrap();
    assert_eq!(
        stack_reserve(&bytes),
        8 << 20,
        "{exe}: the main thread's stack reserve (build.rs)"
    );
}

/// `SizeOfStackReserve` from a PE image's optional header: the `PE\0\0`
/// signature at the offset `0x3c` holds, the 20-byte COFF header, then the
/// field at 72 bytes in (a u64 in PE32+, a u32 in PE32).
#[cfg(windows)]
fn stack_reserve(image: &[u8]) -> u64 {
    let u16_at = |at: usize| u16::from_le_bytes([image[at], image[at + 1]]);
    let u32_at = |at: usize| u32::from_le_bytes(image[at..at + 4].try_into().unwrap());
    let pe = u32_at(0x3c) as usize;
    assert_eq!(&image[pe..pe + 4], b"PE\0\0", "not a PE image");
    let optional = pe + 4 + 20;
    match u16_at(optional) {
        0x20b => u64::from_le_bytes(image[optional + 72..optional + 80].try_into().unwrap()),
        0x10b => u64::from(u32_at(optional + 72)),
        magic => panic!("optional header magic {magic:#x}"),
    }
}
