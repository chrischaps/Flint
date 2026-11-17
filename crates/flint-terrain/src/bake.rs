//! A terrain spec baked to its two maps, with no window (#719).
//!
//! [`bake`] is the one place a spec becomes a heightmap and a splat map: the
//! terrain editor's regenerate and `flint gen x.terrain.toml` both call it,
//! and [`encode_heightmap_png`] / [`encode_splat_png`] are the one encoding,
//! so the editor's Ctrl+E export and the CLI write the same bytes.

use crate::procgen::generate_heightmap;
use crate::spec::TerrainSpec;
use crate::splatgen::generate_splat_map;
use image::ImageEncoder;

/// A spec's heightmap and splat map, at the spec's resolution.
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainMaps {
    /// Row-major heights in [0, 1], `resolution * resolution` of them.
    pub heights: Vec<f32>,
    /// RGBA8 layer weights (R = layer 0 .. A = layer 3), summing to about 255
    /// a texel, `resolution * resolution * 4` bytes.
    pub splat: Vec<u8>,
    /// Texels a side of both maps.
    pub resolution: u32,
    /// The seed both were made from.
    pub seed: u64,
}

/// The side both maps are made at: the spec's resolution, at least 2, as
/// [`generate_heightmap`] has always clamped it.
pub fn map_resolution(spec: &TerrainSpec) -> u32 {
    spec.geometry.resolution.max(2)
}

/// Run the spec's height layers, then its splat rules over the result. The
/// same spec and seed give the same maps, bit for bit.
pub fn bake(spec: &TerrainSpec, seed: u64) -> TerrainMaps {
    let resolution = map_resolution(spec);
    let heights = generate_heightmap(spec, seed);
    let geo = &spec.geometry;
    let splat = generate_splat_map(
        &heights,
        resolution,
        resolution, // the splat map is made at the heightmap's size
        &spec.splat_rules,
        geo.width,
        geo.depth,
        geo.height_scale,
        seed,
    );
    TerrainMaps {
        heights,
        splat,
        resolution,
        seed,
    }
}

impl TerrainMaps {
    /// The heightmap as a 16-bit greyscale PNG.
    pub fn heightmap_png(&self) -> Result<Vec<u8>, String> {
        encode_heightmap_png(&self.heights, self.resolution, self.resolution)
    }

    /// The splat map as an 8-bit RGBA PNG.
    pub fn splat_png(&self) -> Result<Vec<u8>, String> {
        encode_splat_png(&self.splat, self.resolution, self.resolution)
    }

    /// How much of the terrain each layer covers: the mean of its channel
    /// over every texel, in [0, 1]. The four sum to about 1.
    pub fn splat_coverage(&self) -> [f64; 4] {
        let mut sums = [0u64; 4];
        for texel in self.splat.as_chunks::<4>().0 {
            for (sum, &w) in sums.iter_mut().zip(texel) {
                *sum += u64::from(w);
            }
        }
        let texels = (self.splat.len() / 4).max(1) as f64;
        sums.map(|s| s as f64 / 255.0 / texels)
    }
}

/// Heights in [0, 1] as a 16-bit greyscale PNG, the format
/// [`Heightmap::from_png`](crate::Heightmap::from_png) reads back. Values
/// outside [0, 1] are clamped.
pub fn encode_heightmap_png(heights: &[f32], width: u32, depth: u32) -> Result<Vec<u8>, String> {
    let expected = width as usize * depth as usize;
    if heights.len() != expected {
        return Err(format!(
            "heightmap has {} values, not {width}x{depth} = {expected}",
            heights.len()
        ));
    }
    // image 0.25 takes 16-bit samples in native byte order and writes PNG's
    // big-endian itself. Handing it big-endian bytes (what the editor's
    // export did until #719) swapped every sample's bytes on a little-endian
    // machine: the heightmap read back as static.
    let mut samples = Vec::with_capacity(expected * 2);
    for &h in heights {
        let val = (h.clamp(0.0, 1.0) * 65535.0) as u16;
        samples.extend_from_slice(&val.to_ne_bytes());
    }
    encode_png(&samples, width, depth, image::ExtendedColorType::L16)
}

/// RGBA8 splat weights as a PNG.
pub fn encode_splat_png(rgba: &[u8], width: u32, depth: u32) -> Result<Vec<u8>, String> {
    let expected = width as usize * depth as usize * 4;
    if rgba.len() != expected {
        return Err(format!(
            "splat map has {} bytes, not {width}x{depth}x4 = {expected}",
            rgba.len()
        ));
    }
    encode_png(rgba, width, depth, image::ExtendedColorType::Rgba8)
}

fn encode_png(
    samples: &[u8],
    width: u32,
    height: u32,
    color: image::ExtendedColorType,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(samples, width, height, color)
        .map_err(|e| format!("PNG encode failed: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Heightmap;

    fn small_spec() -> TerrainSpec {
        let mut spec: TerrainSpec = toml::from_str(
            r#"
[meta]
name = "small"

[geometry]
resolution = 24

[[height_layers]]
op = "noise"
octaves = 3

[[height_layers]]
op = "erosion"
kind = "hydraulic"
iterations = 40

[[splat_rules]]
layer = 0
height_max = 0.5
noise_amount = 0.2

[[splat_rules]]
layer = 2
height_min = 0.4
"#,
        )
        .unwrap();
        spec.seed.value = 7;
        spec
    }

    #[test]
    fn the_same_seed_bakes_the_same_bytes() {
        let spec = small_spec();
        let (a, b) = (bake(&spec, 7), bake(&spec, 7));
        assert_eq!(a, b);
        assert_eq!(a.heightmap_png().unwrap(), b.heightmap_png().unwrap());
        assert_eq!(a.splat_png().unwrap(), b.splat_png().unwrap());
        assert_ne!(a.heights, bake(&spec, 8).heights);
    }

    #[test]
    fn the_heightmap_png_reads_back_as_the_heights() {
        let maps = bake(&small_spec(), 7);
        let dir = std::env::temp_dir().join(format!("flint_terrain_bake_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("h.png");
        std::fs::write(&file, maps.heightmap_png().unwrap()).unwrap();
        let back = Heightmap::from_png(&file).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!((back.width, back.depth), (24, 24));
        for (i, &h) in maps.heights.iter().enumerate() {
            let (x, z) = (i as u32 % 24, i as u32 / 24);
            let u = x as f32 / 23.0;
            let v = z as f32 / 23.0;
            let got = back.sample(u, v);
            assert!((got - h).abs() < 1e-4, "texel {i}: wrote {h}, read {got}");
        }
    }

    #[test]
    fn coverage_sums_to_one_and_follows_the_rules() {
        let maps = bake(&small_spec(), 7);
        let c = maps.splat_coverage();
        assert!((c.iter().sum::<f64>() - 1.0).abs() < 0.01, "{c:?}");
        assert!(c[0] > 0.0 && c[2] > 0.0, "{c:?}");
        assert_eq!(c[1], 0.0, "no rule paints layer 1: {c:?}");
    }

    /// The editor's export used to save the splat map with `RgbaImage::save`;
    /// the in-memory encoding has to give the bytes it gave.
    #[test]
    fn the_splat_png_is_what_rgba_image_save_writes() {
        let maps = bake(&small_spec(), 7);
        let dir = std::env::temp_dir().join(format!("flint_terrain_splat_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("s.png");
        image::RgbaImage::from_raw(24, 24, maps.splat.clone())
            .unwrap()
            .save(&file)
            .unwrap();
        #[allow(clippy::disallowed_methods)] // a test reading back what it wrote
        let saved = std::fs::read(&file).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(saved, maps.splat_png().unwrap());
    }

    #[test]
    fn a_resolution_under_two_bakes_at_two() {
        let mut spec = small_spec();
        spec.geometry.resolution = 1;
        let maps = bake(&spec, 7);
        assert_eq!(maps.resolution, 2);
        assert_eq!(maps.heights.len(), 4);
        assert_eq!(maps.splat.len(), 16);
    }

    #[test]
    fn a_wrong_sized_buffer_is_refused() {
        assert!(encode_heightmap_png(&[0.5; 3], 2, 2).is_err());
        assert!(encode_splat_png(&[0; 15], 2, 2).is_err());
    }
}
