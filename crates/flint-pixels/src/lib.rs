//! The CPU loops over every texel of a texture on its way to the GPU: widening
//! a decoded image to RGBA8 ([`to_rgba8`]) and box-filtering its mip chain
//! ([`mip_chain`]).
//!
//! They live in a crate of their own so the dev profile can build them
//! optimized (#326). Unoptimized, they were three quarters of what a debug
//! `flint render` spent loading a scene with two large models: 13.6 s of
//! 18.6 s for `examples/style_mask.scene.toml`'s eight 2K and 4K textures,
//! against 4 s for decoding the files. Optimized, the eight cost 1.6 s.
//! `flint-render`, which calls them, is the crate agents edit most, so it
//! stays unoptimized and quick to rebuild; this one has no dependencies and
//! rebuilds in a second or two at any opt-level. Rust does not reassociate
//! or fuse float maths at any opt-level, so a debug build's chains are
//! byte-identical to a release build's.
//!
//! Anything else that walks every texel of a big image on load belongs here
//! too, for the same reason.

use std::collections::HashMap;
use std::sync::Mutex;

/// RGBA8 pixels from a decoded image in `format` (`"rgba8"`, `"rgb8"`,
/// `"rg8"` or `"r8"`, as `flint_import::ImportedTexture::format` names
/// them): a missing alpha is opaque, a missing blue is 0, one grey channel
/// fills red, green and blue. Any other format is white, `width` x `height`.
pub fn to_rgba8(data: &[u8], format: &str, width: u32, height: u32) -> Vec<u8> {
    let pixel_count = (width * height) as usize;
    match format {
        "rgba8" => data.to_vec(),
        "rgb8" => data
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[r, g, b]| [r, g, b, 255])
            .collect(),
        "rg8" => data
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[r, g]| [r, g, 0, 255])
            .collect(),
        "r8" => data.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        _ => vec![255u8; pixel_count * 4],
    }
}

/// Number of mip levels down to 1x1 for a `width` x `height` image.
pub fn mip_level_count(width: u32, height: u32) -> u32 {
    32 - width.max(height).max(1).leading_zeros()
}

/// A full mip chain for RGBA8 pixels: level 0 (the input) first, then each
/// half-size level, all concatenated in the order
/// `create_texture_with_data` expects. Each level is a 2x2 box filter of the
/// one above (an odd edge repeats its last texel). With `srgb` the colour
/// channels are averaged in linear light, so thin bright paint fades into
/// the ground at a distance instead of darkening it; alpha is always linear.
/// Returns the bytes and the level count.
pub fn mip_chain(rgba: &[u8], width: u32, height: u32, srgb: bool) -> (Vec<u8>, u32) {
    let key = {
        let memo = MIP_MEMO.lock().unwrap_or_else(|e| e.into_inner());
        if memo.is_none() {
            drop(memo);
            return build_mip_chain(rgba, width, height, srgb);
        }
        let key = (width, height, srgb, rgba.len(), fingerprint(rgba));
        if let Some(hit) = memo.as_ref().and_then(|m| m.get(&key)) {
            return hit.clone();
        }
        key
    };
    let built = build_mip_chain(rgba, width, height, srgb);
    if let Some(memo) = MIP_MEMO.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        memo.insert(key, built.clone());
    }
    built
}

/// Width, height, sRGB, byte count and [`fingerprint`].
type MipKey = (u32, u32, bool, usize, u64);
type MipMemo = HashMap<MipKey, (Vec<u8>, u32)>;

/// Mip chains kept while [`memoize_mip_chains`] is on.
static MIP_MEMO: Mutex<Option<MipMemo>> = Mutex::new(None);

/// Keep every [`mip_chain`] built, by its pixels, and hand out copies
/// (#329), or (`false`) drop them. For a process that uploads the same
/// textures renderer after renderer: `flint render-suite` builds a fresh
/// renderer a case, and a debug build's box filter was most of what a
/// model's load cost once its file decode was memoized. Off by default.
pub fn memoize_mip_chains(on: bool) {
    *MIP_MEMO.lock().unwrap_or_else(|e| e.into_inner()) = on.then(Default::default);
}

/// A 64-bit FNV-1a over the pixels, eight bytes a step: the memo's key
/// with the size and colour space, cheap next to the chain it saves.
fn fingerprint(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let (words, rest) = bytes.as_chunks::<8>();
    for w in words {
        h ^= u64::from_le_bytes(*w);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    for &b in rest {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn build_mip_chain(rgba: &[u8], width: u32, height: u32, srgb: bool) -> (Vec<u8>, u32) {
    let levels = mip_level_count(width, height);
    let to_linear: Vec<f32> = (0..256)
        .map(|i| {
            let c = i as f32 / 255.0;
            if !srgb {
                c
            } else if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        })
        .collect();
    let to_byte = |v: f32, colour: bool| -> u8 {
        let v = v.clamp(0.0, 1.0);
        let e = if colour && srgb {
            if v <= 0.003_130_8 {
                v * 12.92
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            }
        } else {
            v
        };
        (e * 255.0 + 0.5) as u8
    };

    let mut out = rgba.to_vec();
    let mut cur: Vec<f32> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, a]| {
            [
                to_linear[r as usize],
                to_linear[g as usize],
                to_linear[b as usize],
                a as f32 / 255.0,
            ]
        })
        .collect();
    let (mut w, mut h) = (width as usize, height as usize);
    for _ in 1..levels {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut next = vec![0.0f32; nw * nh * 4];
        for y in 0..nh {
            let (y0, y1) = ((2 * y).min(h - 1), (2 * y + 1).min(h - 1));
            for x in 0..nw {
                let (x0, x1) = ((2 * x).min(w - 1), (2 * x + 1).min(w - 1));
                for c in 0..4 {
                    next[(y * nw + x) * 4 + c] = 0.25
                        * (cur[(y0 * w + x0) * 4 + c]
                            + cur[(y0 * w + x1) * 4 + c]
                            + cur[(y1 * w + x0) * 4 + c]
                            + cur[(y1 * w + x1) * 4 + c]);
                }
            }
        }
        out.extend(next.as_chunks::<4>().0.iter().flat_map(|&[r, g, b, a]| {
            [
                to_byte(r, true),
                to_byte(g, true),
                to_byte(b, true),
                to_byte(a, false),
            ]
        }));
        cur = next;
        w = nw;
        h = nh;
    }
    (out, levels)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The memo (#329) hands back what building would, and two textures
    /// differing in one byte don't share a chain.
    #[test]
    fn memoized_chains_match_built_ones() {
        let mut a = vec![0u8; 8 * 4 * 4];
        for (i, b) in a.iter_mut().enumerate() {
            *b = (i * 37 % 251) as u8;
        }
        let mut b = a.clone();
        b[77] ^= 0x40;
        let built = (
            build_mip_chain(&a, 8, 4, true),
            build_mip_chain(&b, 8, 4, true),
        );
        memoize_mip_chains(true);
        let first = mip_chain(&a, 8, 4, true);
        let again = mip_chain(&a, 8, 4, true);
        let other = mip_chain(&b, 8, 4, true);
        let linear = mip_chain(&a, 8, 4, false);
        memoize_mip_chains(false);
        assert_eq!(first, built.0);
        assert_eq!(again, built.0);
        assert_eq!(other, built.1);
        assert_eq!(linear, build_mip_chain(&a, 8, 4, false));
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn level_counts() {
        assert_eq!(mip_level_count(1, 1), 1);
        assert_eq!(mip_level_count(1024, 1024), 11);
        assert_eq!(mip_level_count(1024, 256), 11);
        assert_eq!(mip_level_count(5, 3), 3);
    }

    #[test]
    fn chain_sizes_add_up() {
        let (w, h) = (6u32, 3u32);
        let (data, levels) = mip_chain(&vec![200u8; (w * h * 4) as usize], w, h, true);
        // 6x3, 3x1, 1x1
        assert_eq!(levels, 3);
        assert_eq!(data.len(), (18 + 3 + 1) * 4);
        // a flat colour stays the same colour all the way down
        assert!(data.iter().all(|&b| b == 200));
    }

    #[test]
    fn srgb_average_is_in_linear_light() {
        // black and white average to linear 0.5, which is sRGB 188
        let px = [0, 0, 0, 255, 255, 255, 255, 255];
        let (data, _) = mip_chain(&px, 2, 1, true);
        assert_eq!(&data[8..11], &[188, 188, 188]);
        assert_eq!(data[11], 255);
        let (data, _) = mip_chain(&px, 2, 1, false);
        assert_eq!(data[8], 128);
    }

    #[test]
    fn widening_to_rgba() {
        assert_eq!(to_rgba8(&[1, 2, 3], "rgb8", 1, 1), [1, 2, 3, 255]);
        assert_eq!(to_rgba8(&[1, 2], "rg8", 1, 1), [1, 2, 0, 255]);
        assert_eq!(to_rgba8(&[7], "r8", 1, 1), [7, 7, 7, 255]);
        assert_eq!(to_rgba8(&[1, 2, 3, 4], "rgba8", 1, 1), [1, 2, 3, 4]);
        assert_eq!(to_rgba8(&[0; 12], "rgb32f", 1, 1), [255; 4]);
    }
}
