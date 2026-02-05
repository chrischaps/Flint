//! Generate procedural texture PNGs for the Luminarium demo scene.
//!
//! Run with: cargo run -p flint-render --example gen_textures

use image::{Rgba, RgbaImage};
use std::path::Path;

fn main() {
    let out_dir = Path::new("demo/textures");
    std::fs::create_dir_all(out_dir).expect("Failed to create demo/textures");

    generate_stone_wall(out_dir);
    generate_wood_planks(out_dir);
    generate_floor_tiles(out_dir);

    println!("Generated textures in {}", out_dir.display());
}

/// Simple pseudo-random hash for deterministic noise
fn hash(x: u32, y: u32, seed: u32) -> f32 {
    let n = x.wrapping_mul(374761393)
        .wrapping_add(y.wrapping_mul(668265263))
        .wrapping_add(seed.wrapping_mul(1274126177));
    let n = (n ^ (n >> 13)).wrapping_mul(1103515245);
    let n = n ^ (n >> 16);
    (n & 0x7FFFFFFF) as f32 / 0x7FFFFFFF as f32
}

/// Smooth noise via bilinear interpolation of hash values
fn smooth_noise(x: f32, y: f32, seed: u32) -> f32 {
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let fx = x - x.floor();
    let fy = y - y.floor();

    // Smoothstep
    let fx = fx * fx * (3.0 - 2.0 * fx);
    let fy = fy * fy * (3.0 - 2.0 * fy);

    let n00 = hash(ix as u32, iy as u32, seed);
    let n10 = hash((ix + 1) as u32, iy as u32, seed);
    let n01 = hash(ix as u32, (iy + 1) as u32, seed);
    let n11 = hash((ix + 1) as u32, (iy + 1) as u32, seed);

    let nx0 = n00 + (n10 - n00) * fx;
    let nx1 = n01 + (n11 - n01) * fx;
    nx0 + (nx1 - nx0) * fy
}

/// Multi-octave fractal noise
fn fbm(x: f32, y: f32, octaves: u32, seed: u32) -> f32 {
    let mut value = 0.0;
    let mut amplitude = 0.5;
    let mut frequency = 1.0;
    for i in 0..octaves {
        value += amplitude * smooth_noise(x * frequency, y * frequency, seed + i * 7);
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    value
}

/// Stone wall: ashlar block pattern with mortar lines, gray-brown tones
fn generate_stone_wall(out_dir: &Path) {
    let size = 256u32;
    let mut img = RgbaImage::new(size, size);

    let block_h = 32.0_f32; // block height in pixels
    let block_w = 64.0_f32; // block width in pixels

    for y in 0..size {
        for x in 0..size {
            let row = (y as f32 / block_h).floor() as i32;
            // Offset every other row for ashlar bond
            let offset = if row % 2 == 0 { 0.0 } else { block_w * 0.5 };
            let bx = ((x as f32 + offset) % block_w) / block_w;
            let by = (y as f32 % block_h) / block_h;

            // Mortar line detection
            let mortar_h = 0.04; // horizontal mortar thickness
            let mortar_v = 0.04; // vertical mortar thickness
            let is_mortar = by < mortar_h || by > (1.0 - mortar_h)
                || bx < mortar_v || bx > (1.0 - mortar_v);

            if is_mortar {
                // Mortar: light gray with subtle variation
                let noise = fbm(x as f32 * 0.1, y as f32 * 0.1, 3, 42);
                let v = (0.6 + noise * 0.15).clamp(0.0, 1.0);
                let r = (v * 200.0) as u8;
                let g = (v * 195.0) as u8;
                let b = (v * 185.0) as u8;
                img.put_pixel(x, y, Rgba([r, g, b, 255]));
            } else {
                // Stone block: warm gray-brown with per-block color variation
                let block_seed = hash(
                    ((x as f32 + offset) / block_w).floor() as u32,
                    row as u32,
                    123,
                );
                let noise = fbm(x as f32 * 0.05, y as f32 * 0.05, 4, 99);
                let detail = fbm(x as f32 * 0.2, y as f32 * 0.2, 2, 77);

                let base = 0.45 + block_seed * 0.2 + noise * 0.15 + detail * 0.05;
                let base = base.clamp(0.0, 1.0);

                // Warm sandstone tint
                let r = (base * 210.0) as u8;
                let g = (base * 195.0) as u8;
                let b = (base * 170.0) as u8;
                img.put_pixel(x, y, Rgba([r, g, b, 255]));
            }
        }
    }

    img.save(out_dir.join("stone_wall.png")).unwrap();
    println!("  stone_wall.png (256x256)");
}

/// Wood planks: vertical planks with grain variation
fn generate_wood_planks(out_dir: &Path) {
    let size = 256u32;
    let mut img = RgbaImage::new(size, size);

    let plank_width = 42.0_f32; // pixel width per plank

    for y in 0..size {
        for x in 0..size {
            let plank = (x as f32 / plank_width).floor() as u32;
            let bx = (x as f32 % plank_width) / plank_width;

            // Gap between planks
            let gap = 0.03;
            let is_gap = bx < gap || bx > (1.0 - gap);

            if is_gap {
                img.put_pixel(x, y, Rgba([40, 30, 20, 255]));
            } else {
                // Per-plank base color variation
                let plank_seed = hash(plank, 0, 555);
                let plank_tone = 0.35 + plank_seed * 0.25;

                // Wood grain: stretched noise along Y axis
                let grain = fbm(x as f32 * 0.02 + plank as f32 * 30.0, y as f32 * 0.15, 4, 200);
                let fine_grain = fbm(x as f32 * 0.1 + plank as f32 * 30.0, y as f32 * 0.5, 2, 300);

                // Ring pattern via sine on the grain
                let ring = ((grain * 12.0).sin() * 0.5 + 0.5) * 0.15;

                let v = (plank_tone + grain * 0.12 + fine_grain * 0.05 + ring).clamp(0.0, 1.0);

                // Warm brown tones
                let r = (v * 195.0 + 30.0).min(255.0) as u8;
                let g = (v * 145.0 + 20.0).min(255.0) as u8;
                let b = (v * 95.0 + 15.0).min(255.0) as u8;
                img.put_pixel(x, y, Rgba([r, g, b, 255]));
            }
        }
    }

    img.save(out_dir.join("wood_planks.png")).unwrap();
    println!("  wood_planks.png (256x256)");
}

/// Floor tiles: 4x4 tile grid with grout lines, cool grays
fn generate_floor_tiles(out_dir: &Path) {
    let size = 256u32;
    let mut img = RgbaImage::new(size, size);

    let tile_size = 64.0_f32; // 256 / 4 = 64px per tile

    for y in 0..size {
        for x in 0..size {
            let tx = (x as f32 % tile_size) / tile_size;
            let ty = (y as f32 % tile_size) / tile_size;

            let tile_ix = (x as f32 / tile_size).floor() as u32;
            let tile_iy = (y as f32 / tile_size).floor() as u32;

            // Grout lines
            let grout = 0.035;
            let is_grout = tx < grout || tx > (1.0 - grout)
                || ty < grout || ty > (1.0 - grout);

            if is_grout {
                let noise = fbm(x as f32 * 0.15, y as f32 * 0.15, 2, 88);
                let v = (0.35 + noise * 0.1).clamp(0.0, 1.0);
                let c = (v * 140.0) as u8;
                img.put_pixel(x, y, Rgba([c, c, (c as f32 * 1.05) as u8, 255]));
            } else {
                // Per-tile color variation
                let tile_seed = hash(tile_ix, tile_iy, 999);
                let base = 0.45 + tile_seed * 0.15;

                // Surface variation
                let noise = fbm(x as f32 * 0.06, y as f32 * 0.06, 4, 150);
                let speckle = hash(x, y, 777) * 0.04;

                let v = (base + noise * 0.12 + speckle).clamp(0.0, 1.0);

                // Cool gray-blue slate tones
                let r = (v * 175.0) as u8;
                let g = (v * 178.0) as u8;
                let b = (v * 190.0) as u8;
                img.put_pixel(x, y, Rgba([r, g, b, 255]));
            }
        }
    }

    img.save(out_dir.join("floor_tiles.png")).unwrap();
    println!("  floor_tiles.png (256x256)");
}
