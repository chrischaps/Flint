//! Images a style draws with (#390): a scanned paper the pencil and
//! watercolor styles lay their marks on, and a strip of real pencil strokes
//! pencil samples along each dash.
//!
//! A style names an image with a string option (`paper_image =
//! "papers/cold_press.png"`, or just `"cold_press"`). It is looked for the
//! way glyph sets and palettes are: beside the scene, then in the scene's
//! parent, then in `.`; a bare name also tries `<folder>/<name>.png` in
//! each. The file is read through the vfs ([`open_image`]), so a game
//! bundle carries it to the web.
//!
//! The image is kept with the style ([`ImageSlot`]) as an `Rgba8Unorm`
//! texture of display values with a mip chain made on the CPU, so a
//! shader reads it with `textureLoad` at a level it picks: tiling a paper
//! wraps by integer arithmetic, and no style needs a sampler of its own
//! (the DX12 heap rule). Each upload has a generation of its own, for the
//! kept bind groups over it. Natively the file is watched like a palette
//! (#323): an edited paper shows while the viewer is open.
//!
//! What the shaders need beyond the texels is measured once at load
//! ([`ImageStats`]): a paper's mean and spread of luma, so any scan's tooth
//! comes out at the same strength and `grain` alone says how much shows;
//! a stroke strip's ink, so a light scan still draws a full-strength line.

use crate::file_watch::FileStamps;
use crate::texture_cache::open_image;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// The texture format of every style image: the file's bytes as they are,
/// display values, read without decoding.
pub const IMAGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The most mip levels kept (level 0 included).
pub const MAX_LEVELS: u32 = 8;

/// A longer side is scaled down to this on load.
pub const MAX_SIDE: u32 = 2048;

/// Where a bare paper name is looked for, and the F4 menu lists papers from.
pub const PAPERS: &str = "papers";

/// Where a bare stroke name is looked for.
pub const STROKES: &str = "strokes";

/// What an image is for, which decides what is measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageUse {
    /// A paper: tiled under the picture; its luma is the paper's height.
    Paper,
    /// A strip of strokes: dark (or opaque) marks on light (or clear).
    Stroke,
}

impl ImageUse {
    fn folder(self) -> &'static str {
        match self {
            Self::Paper => PAPERS,
            Self::Stroke => STROKES,
        }
    }
}

/// What a shader reads beside the texels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageStats {
    /// The mean luma (display values), 0..1.
    pub mean: f32,
    /// The standard deviation of luma, at least a small floor.
    pub spread: f32,
    /// A stroke's ink: the 99th percentile of `alpha * (1 - luma)`, at
    /// least a small floor, so coverage is that over this.
    pub ink: f32,
}

impl ImageStats {
    /// The stand-in's: white, flat, no ink.
    pub const BLANK: Self = Self {
        mean: 1.0,
        spread: 1.0,
        ink: 1.0,
    };

    /// A paper's mean and spread with a stroke strip's ink, as one vec4
    /// (the pencil's `img`): mean, spread, ink, 0.
    pub fn paper_and(self, stroke: ImageStats) -> [f32; 4] {
        [self.mean, self.spread, stroke.ink, 0.0]
    }
}

fn luma(p: &image::Rgba<u8>) -> f32 {
    (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0
}

/// Measure `img` (see [`ImageStats`]).
pub fn measure(img: &image::RgbaImage) -> ImageStats {
    let n = (img.width() as usize * img.height() as usize).max(1);
    let (mut sum, mut sum2) = (0.0f64, 0.0f64);
    let mut inks: Vec<u16> = Vec::with_capacity(n);
    for p in img.pixels() {
        let l = luma(p) as f64;
        sum += l;
        sum2 += l * l;
        let ink = (p[3] as f32 / 255.0) * (1.0 - luma(p));
        inks.push((ink.clamp(0.0, 1.0) * 65535.0).round() as u16);
    }
    let mean = sum / n as f64;
    let var = (sum2 / n as f64 - mean * mean).max(0.0);
    inks.sort_unstable();
    let p99 = inks[((n - 1) as f64 * 0.99) as usize] as f32 / 65535.0;
    ImageStats {
        mean: mean as f32,
        spread: (var.sqrt() as f32).max(0.004),
        ink: p99.max(0.02),
    }
}

/// Take a paper's overall cast out: each channel divided by the image's
/// mean colour over its mean luma, so the average texel is grey at the
/// same brightness. The style's `paper` colour then says what colour the
/// paper is, and the scan adds its relief and its local variations of hue
/// (a fibre, a fleck), not a second tint over the first.
pub fn neutralize(img: &mut image::RgbaImage) {
    let n = (img.width() as f64 * img.height() as f64).max(1.0);
    let mut sum = [0.0f64; 3];
    for p in img.pixels() {
        for (c, s) in sum.iter_mut().enumerate() {
            *s += p[c] as f64;
        }
    }
    let mean = sum.map(|s| s / n);
    let l = 0.2126 * mean[0] + 0.7152 * mean[1] + 0.0722 * mean[2];
    if l <= 0.5 {
        return;
    }
    let gain = mean.map(|m| if m > 0.5 { l / m } else { 1.0 });
    for p in img.pixels_mut() {
        for c in 0..3 {
            p[c] = (p[c] as f64 * gain[c]).round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// The paths `name` may be at, in order: `name` itself under each search
/// directory (or as given when absolute), then, for a bare name,
/// `<folder>/<name>.png` under each.
pub fn candidates(search: &[PathBuf], name: &str, use_: ImageUse) -> Vec<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() {
        return vec![path.to_path_buf()];
    }
    let mut out: Vec<PathBuf> = search.iter().map(|d| d.join(path)).collect();
    let bare = !name.contains('/') && !name.contains('\\') && path.extension().is_none();
    if bare {
        let file = format!("{name}.png");
        out.extend(search.iter().map(|d| d.join(use_.folder()).join(&file)));
    }
    out
}

/// The mip chain of `img`: level 0 at most [`MAX_SIDE`] on its longer
/// side, each level half the last (at least 1), up to [`MAX_LEVELS`].
pub fn mip_chain(img: image::RgbaImage) -> Vec<image::RgbaImage> {
    use image::imageops::{resize, FilterType};
    let (w, h) = img.dimensions();
    let longest = w.max(h).max(1);
    let base = if longest > MAX_SIDE {
        let s = MAX_SIDE as f32 / longest as f32;
        let (nw, nh) = (
            ((w as f32 * s).round() as u32).max(1),
            ((h as f32 * s).round() as u32).max(1),
        );
        resize(&img, nw, nh, FilterType::Triangle)
    } else {
        img
    };
    let mut levels = vec![base];
    while levels.len() < MAX_LEVELS as usize {
        let (w, h) = levels.last().unwrap().dimensions();
        if w == 1 && h == 1 {
            break;
        }
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let next = resize(levels.last().unwrap(), nw, nh, FilterType::Triangle);
        levels.push(next);
    }
    levels
}

/// One image on the GPU.
pub struct StyleImage {
    _texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub stats: ImageStats,
    /// Unique to this upload, for bind groups kept over [`view`](Self::view).
    pub generation: u64,
    /// Whether this is a loaded file (false: the blank stand-in).
    pub found: bool,
}

static GENERATION: AtomicU64 = AtomicU64::new(1);

fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    levels: &[image::RgbaImage],
    stats: ImageStats,
    found: bool,
) -> Arc<StyleImage> {
    let (w, h) = levels[0].dimensions();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: IMAGE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, img) in levels.iter().enumerate() {
        let (lw, lh) = img.dimensions();
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            img.as_raw(),
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * lw),
                rows_per_image: Some(lh),
            },
            wgpu::Extent3d {
                width: lw,
                height: lh,
                depth_or_array_layers: 1,
            },
        );
    }
    let view = texture.create_view(&Default::default());
    Arc::new(StyleImage {
        _texture: texture,
        view,
        stats,
        generation: GENERATION.fetch_add(1, Ordering::Relaxed),
        found,
    })
}

/// One image option of a style, kept with the style: the image its name
/// finds, loaded once and again when the file changes, or a blank stand-in
/// while it names none or can't be read (warned once a name).
pub struct ImageSlot {
    use_: ImageUse,
    label: &'static str,
    /// What the current image was loaded for: the search path and name.
    key: Option<(Vec<PathBuf>, String)>,
    current: Option<Arc<StyleImage>>,
    blank: Option<Arc<StyleImage>>,
    stamps: FileStamps,
    warned: Vec<String>,
}

impl ImageSlot {
    pub fn new(use_: ImageUse, label: &'static str) -> Self {
        Self {
            use_,
            label,
            key: None,
            current: None,
            blank: None,
            stamps: FileStamps::default(),
            warned: Vec::new(),
        }
    }

    /// The blank stand-in: one white, opaque texel.
    pub fn blank(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Arc<StyleImage> {
        self.blank
            .get_or_insert_with(|| {
                let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([255; 4]));
                upload(device, queue, self.label, &[img], ImageStats::BLANK, false)
            })
            .clone()
    }

    /// The image `name` names, looked for from `search` (see
    /// [`candidates`]); the blank stand-in for "" or a file that won't
    /// load. With `poll` set, a watched file that changed is read again.
    pub fn get(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        search: &[PathBuf],
        name: &str,
        poll: bool,
    ) -> Arc<StyleImage> {
        if name.is_empty() {
            return self.blank(device, queue);
        }
        let key = (search.to_vec(), name.to_string());
        let stale = self.key.as_ref() != Some(&key) || (poll && self.stamps.changed());
        if stale || self.current.is_none() {
            self.stamps.clear();
            self.key = Some(key);
            self.current = Some(self.load(device, queue, search, name));
        }
        self.current.clone().unwrap()
    }

    fn load(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        search: &[PathBuf],
        name: &str,
    ) -> Arc<StyleImage> {
        let mut tried = Vec::new();
        for path in candidates(search, name, self.use_) {
            self.stamps.watch(&path);
            if !flint_core::vfs::is_file(&path) {
                tried.push(path);
                continue;
            }
            match open_image(&path) {
                Ok(img) => {
                    let mut img = img.to_rgba8();
                    if self.use_ == ImageUse::Paper {
                        neutralize(&mut img);
                    }
                    let stats = measure(&img);
                    let levels = mip_chain(img);
                    tracing::debug!(
                        "{}: {} ({}x{}, mean {:.3}, spread {:.3}, ink {:.3})",
                        self.label,
                        path.display(),
                        levels[0].width(),
                        levels[0].height(),
                        stats.mean,
                        stats.spread,
                        stats.ink
                    );
                    return upload(device, queue, self.label, &levels, stats, true);
                }
                Err(e) => {
                    self.warn(name, &format!("{}: {e}", path.display()));
                    return self.blank(device, queue);
                }
            }
        }
        let list: Vec<String> = tried.iter().map(|p| p.display().to_string()).collect();
        self.warn(name, &format!("not found (looked at {})", list.join(", ")));
        self.blank(device, queue)
    }

    fn warn(&mut self, name: &str, why: &str) {
        if !self.warned.iter().any(|w| w == name) {
            tracing::warn!(
                "{}: image \"{name}\" {why}; drawing the procedural one",
                self.label
            );
            self.warned.push(name.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_name_also_tries_the_folder() {
        let search = vec![PathBuf::from("demo"), PathBuf::from(".")];
        let c = candidates(&search, "cold_press", ImageUse::Paper);
        assert_eq!(
            c,
            vec![
                PathBuf::from("demo").join("cold_press"),
                PathBuf::from(".").join("cold_press"),
                PathBuf::from("demo").join("papers").join("cold_press.png"),
                PathBuf::from(".").join("papers").join("cold_press.png"),
            ]
        );
        let c = candidates(&search, "strokes/hb.png", ImageUse::Stroke);
        assert_eq!(c.len(), 2, "a path is taken as written");
    }

    #[test]
    fn a_paper_is_measured_by_its_luma() {
        // Half black, half white: mean 0.5, spread 0.5
        let img = image::RgbaImage::from_fn(4, 2, |x, _| {
            image::Rgba(if x < 2 { [0, 0, 0, 255] } else { [255; 4] })
        });
        let s = measure(&img);
        assert!((s.mean - 0.5).abs() < 1e-4);
        assert!((s.spread - 0.5).abs() < 1e-4);
        // A flat paper's spread is floored, so dividing by it is safe
        let flat = image::RgbaImage::from_pixel(3, 3, image::Rgba([200, 200, 200, 255]));
        assert!(measure(&flat).spread > 0.0);
    }

    #[test]
    fn a_stroke_counts_dark_or_opaque_as_ink() {
        // A light grey stroke on white: its ink is its darkness, so a
        // faint scan still draws at full strength once divided by it
        let img = image::RgbaImage::from_fn(10, 10, |x, _| {
            image::Rgba(if x == 5 {
                [153, 153, 153, 255]
            } else {
                [255; 4]
            })
        });
        assert!((measure(&img).ink - 0.4).abs() < 0.01);
        // Black on clear: only the opaque texels are ink
        let clear = image::RgbaImage::from_fn(10, 10, |x, _| {
            image::Rgba(if x == 5 { [0, 0, 0, 255] } else { [0, 0, 0, 0] })
        });
        assert!((measure(&clear).ink - 1.0).abs() < 0.01);
    }

    #[test]
    fn a_papers_cast_is_taken_out_and_its_variation_kept() {
        // A cream paper with one warmer fleck
        let mut img = image::RgbaImage::from_fn(10, 10, |x, _| {
            image::Rgba(if x == 0 {
                [250, 220, 180, 255]
            } else {
                [240, 232, 214, 255]
            })
        });
        neutralize(&mut img);
        let p = img.get_pixel(5, 5);
        assert!(p[0].abs_diff(p[2]) <= 8, "the cast is gone: {p:?}");
        let f = img.get_pixel(0, 0);
        assert!(
            f[0] > f[2] + 30,
            "the fleck stays warmer than the paper: {f:?}"
        );
    }

    #[test]
    fn the_mip_chain_halves_to_one_texel_or_the_cap() {
        let img = image::RgbaImage::new(64, 16);
        let sizes: Vec<_> = mip_chain(img).iter().map(|l| l.dimensions()).collect();
        assert_eq!(
            sizes,
            vec![(64, 16), (32, 8), (16, 4), (8, 2), (4, 1), (2, 1), (1, 1)]
        );
        let big = image::RgbaImage::new(4096, 512);
        let chain = mip_chain(big);
        assert_eq!(chain[0].dimensions(), (MAX_SIDE, 256));
        assert_eq!(chain.len(), MAX_LEVELS as usize);
    }
}
