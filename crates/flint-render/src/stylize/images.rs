//! Images a style draws with (#390): a scanned paper the pencil and
//! watercolor styles lay their marks on, and a strip of real pencil strokes
//! pencil samples along each dash.
//!
//! A style names an image with a string option (`paper_image =
//! "papers/cold_press.png"`, or just `"cold_press"`). It is found the way
//! glyph sets and palettes are, by the project's one asset search (ADR
//! 0114; [`AssetKind::Papers`] / [`AssetKind::Strokes`]): `papers/<name>.png`
//! (then `.jpg`, `.jpeg`), or the name as a path, in the scene's directory,
//! each directory above it up to the project root, then the mounted engine.
//! The file is read through the vfs ([`open_image`]), so a game bundle
//! carries it to the web.
//!
//! Each image is read once and kept with the style ([`ImageStack`]), and
//! the images a frame draws with (up to [`MAX_LAYERS`] of each use: two
//! props on two papers, charcoal on one and HB on another, #547) are
//! stacked as layers of one `Rgba8Unorm` `texture_2d_array` of display
//! values, each layer with its own mip chain made on the CPU. A row of a
//! style's variant table carries the layer it reads in place of an on/off
//! flag, and the shader reads it with `textureLoad` at a level it picks:
//! tiling a paper wraps by integer arithmetic, and no style needs a
//! sampler of its own (the DX12 heap rule). Each stack has a generation of
//! its own, for the kept bind groups over it. Natively every file is
//! watched like a palette (#323): an edited paper shows while the viewer
//! is open.
//!
//! What the shaders need beyond the texels is measured once at load
//! ([`ImageStats`]): a paper's mean and spread of luma, so any scan's tooth
//! comes out at the same strength and `grain` alone says how much shows;
//! a stroke strip's ink, so a light scan still draws a full-strength line.

use crate::file_watch::FileStamps;
use crate::gpu_counts::{CountedDevice, CountedQueue};
use crate::texture_cache::open_image;
use bytemuck::{Pod, Zeroable};
use flint_core::project::{shown, AssetKind, AssetScope};
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
    /// The asset kind an image of this use is searched as.
    pub fn kind(self) -> AssetKind {
        match self {
            Self::Paper => AssetKind::Papers,
            Self::Stroke => AssetKind::Strokes,
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

/// The most images of one use a style draws with in a frame (#547): the
/// layers of its stack. A row naming one more draws procedurally, and the
/// first entity past the cap is warned about.
pub const MAX_LAYERS: usize = 4;

/// The value of an image option that names no image.
pub const NO_IMAGE: &str = "none";

/// Whether an image option's value names an image.
pub fn names_image(value: &str) -> bool {
    !value.is_empty() && value != NO_IMAGE
}

/// One layer of a stack as the shader reads it (`StyleImageLayer` in
/// `style_image.wgsl`): what was measured, and how big the image is, since
/// the layers of one stack differ in size.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug, PartialEq)]
pub struct LayerInfo {
    /// The image's mean and spread of luma and its ink, then 0.
    pub stats: [f32; 4],
    /// Its level-0 width and height, its mip levels, then 0.
    pub size: [u32; 4],
}

impl LayerInfo {
    /// A layer no row reads: the stand-in's one white texel.
    pub const BLANK: Self = Self {
        stats: [
            ImageStats::BLANK.mean,
            ImageStats::BLANK.spread,
            ImageStats::BLANK.ink,
            0.0,
        ],
        size: [1, 1, 1, 0],
    };

    fn of(stats: ImageStats, levels: &[image::RgbaImage]) -> Self {
        let (w, h) = levels[0].dimensions();
        Self {
            stats: [stats.mean, stats.spread, stats.ink, 0.0],
            size: [w, h, levels.len() as u32, 0],
        }
    }
}

/// The images of one use a frame draws with, on the GPU as one
/// `texture_2d_array` (#547): each image in a layer of its own, at its own
/// size in the layer's top-left corner and with its own mip chain, so a
/// layer reads exactly as the image alone would. The array is as big as
/// the largest of them.
pub struct StackedImages {
    _texture: wgpu::Texture,
    /// A `D2Array` view of every layer.
    pub view: wgpu::TextureView,
    /// Each layer's measures and size; [`LayerInfo::BLANK`] past the last.
    pub layers: [LayerInfo; MAX_LAYERS],
    /// Unique to this upload, for bind groups kept over [`view`](Self::view).
    pub generation: u64,
}

static GENERATION: AtomicU64 = AtomicU64::new(1);

/// Upload `images` (each a mip chain) as one array, a layer each. On GL a
/// texture of one layer is a plain 2D texture, which a `texture_2d_array`
/// binding can't view, so there the array has at least two.
fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    images: &[(&[image::RgbaImage], ImageStats)],
    min_layers: u32,
) -> Arc<StackedImages> {
    let width = images.iter().map(|(l, _)| l[0].width()).max().unwrap_or(1);
    let height = images.iter().map(|(l, _)| l[0].height()).max().unwrap_or(1);
    let levels = images
        .iter()
        .map(|(l, _)| l.len() as u32)
        .max()
        .unwrap_or(1);
    let texture = device.create_texture_counted(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: (images.len() as u32).max(min_layers).max(1),
        },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: IMAGE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut layers = [LayerInfo::BLANK; MAX_LAYERS];
    for (layer, (chain, stats)) in images.iter().enumerate() {
        layers[layer] = LayerInfo::of(*stats, chain);
        for (level, img) in chain.iter().enumerate() {
            let (lw, lh) = img.dimensions();
            queue.write_texture_counted(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: layer as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                img.as_raw(),
                wgpu::TexelCopyBufferLayout {
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
    }
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some(label),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    Arc::new(StackedImages {
        _texture: texture,
        view,
        layers,
        generation: GENERATION.fetch_add(1, Ordering::Relaxed),
    })
}

/// Which layer each row draws from, given which names load: a layer each
/// distinct name that loads, in the order rows first name them, up to
/// [`MAX_LAYERS`].
#[derive(Debug, PartialEq)]
pub struct Assignment {
    /// Each row's layer plus one; 0 draws procedurally (no image named,
    /// it didn't load, or the stack was full).
    pub layer: Vec<u32>,
    /// The names stacked, a layer each.
    pub stacked: Vec<String>,
    /// The first row whose image didn't fit.
    pub over_cap: Option<usize>,
}

/// Assign `names` (one a row) to layers; `loads` says whether a name's
/// image could be read.
pub fn assign_layers(names: &[String], loads: &mut dyn FnMut(&str) -> bool) -> Assignment {
    let mut stacked: Vec<String> = Vec::new();
    let mut over_cap = None;
    let layer = names
        .iter()
        .enumerate()
        .map(|(row, name)| {
            if !names_image(name) {
                return 0;
            }
            if let Some(at) = stacked.iter().position(|n| n == name) {
                return at as u32 + 1;
            }
            if !loads(name) {
                return 0;
            }
            if stacked.len() == MAX_LAYERS {
                over_cap = over_cap.or(Some(row));
                return 0;
            }
            stacked.push(name.clone());
            stacked.len() as u32
        })
        .collect();
    Assignment {
        layer,
        stacked,
        over_cap,
    }
}

/// An image read from its file, kept on the CPU so a stack can be built
/// again when the set of images a frame draws with changes.
struct Loaded {
    scope: AssetScope,
    name: String,
    /// The mip chain; `None` while the name finds nothing that loads.
    levels: Option<Arc<Vec<image::RgbaImage>>>,
    stats: ImageStats,
    stamps: FileStamps,
    /// Unique to this read, so a stack over an edited file is rebuilt.
    generation: u64,
    /// When a frame last asked for it, for evicting the oldest.
    used: u64,
}

/// What [`ImageStack::get`] hands a style: the frame's stack, and the
/// layer each row draws from.
pub struct Stacked {
    pub images: Arc<StackedImages>,
    /// See [`Assignment::layer`].
    pub layer: Vec<u32>,
    /// See [`Assignment::over_cap`].
    pub over_cap: Option<usize>,
}

/// The images of one use a style draws with, kept with the style (#547;
/// one image a frame since #390): each name's image read once, cached by
/// name and read again when its file changes, and the frame's set of them
/// stacked into one array, rebuilt only when that set (or a file) changes.
/// A name that finds no image is warned about once and draws procedurally.
pub struct ImageStack {
    use_: ImageUse,
    label: &'static str,
    loaded: Vec<Loaded>,
    clock: u64,
    /// The stack in use, with the reads (generations) it was built from.
    current: Option<(Vec<u64>, Arc<StackedImages>)>,
    blank: Option<Arc<StackedImages>>,
    /// The fewest layers an array has here: 2 on GL, else 1.
    min_layers: Option<u32>,
    warned: Vec<String>,
}

/// How many images a stack keeps read beyond those a frame uses.
const KEPT: usize = 2 * MAX_LAYERS;

impl ImageStack {
    pub fn new(use_: ImageUse, label: &'static str) -> Self {
        Self {
            use_,
            label,
            loaded: Vec::new(),
            clock: 0,
            current: None,
            blank: None,
            min_layers: None,
            warned: Vec::new(),
        }
    }

    fn min_layers(&mut self, device: &wgpu::Device) -> u32 {
        *self.min_layers.get_or_insert_with(|| {
            if device.adapter_info().backend == wgpu::Backend::Gl {
                2
            } else {
                1
            }
        })
    }

    /// The stand-in while no row's image loads: one white, opaque texel.
    fn blank(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Arc<StackedImages> {
        let min_layers = self.min_layers(device);
        let label = self.label;
        self.blank
            .get_or_insert_with(|| {
                let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([255; 4]));
                // Its one layer measures as LayerInfo::BLANK
                upload(
                    device,
                    queue,
                    label,
                    &[(std::slice::from_ref(&img), ImageStats::BLANK)],
                    min_layers,
                )
            })
            .clone()
    }

    /// The frame's stack for `names` (each row's), found by the project's
    /// search from `scope`. With `poll` set, a watched file that changed
    /// (an image, or a nearer place one has appeared at) is read again.
    pub fn get(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scope: &AssetScope,
        names: &[String],
        poll: bool,
    ) -> Stacked {
        self.clock += 1;
        let clock = self.clock;
        let min_layers = self.min_layers(device);
        let mut polled: Vec<String> = Vec::new();
        let assignment = assign_layers(names, &mut |name| {
            let at = self.find_or_load(scope, name, poll && !polled.iter().any(|p| p == name));
            polled.push(name.to_string());
            self.loaded[at].used = clock;
            self.loaded[at].levels.is_some()
        });
        let chosen: Vec<&Loaded> = assignment
            .stacked
            .iter()
            .map(|name| {
                self.loaded
                    .iter()
                    .find(|l| l.scope == *scope && l.name == *name)
                    .expect("loaded above")
            })
            .collect();
        let generations: Vec<u64> = chosen.iter().map(|l| l.generation).collect();
        let images = if chosen.is_empty() {
            self.blank(device, queue)
        } else if let Some((_, stack)) = self.current.as_ref().filter(|(g, _)| *g == generations) {
            stack.clone()
        } else {
            let chains: Vec<(&[image::RgbaImage], ImageStats)> = chosen
                .iter()
                .map(|l| (l.levels.as_ref().expect("loads").as_slice(), l.stats))
                .collect();
            let stack = upload(device, queue, self.label, &chains, min_layers);
            self.current = Some((generations, stack.clone()));
            stack
        };
        self.evict();
        Stacked {
            images,
            layer: assignment.layer,
            over_cap: assignment.over_cap,
        }
    }

    /// Forget the images read longest ago past [`KEPT`], never one this
    /// frame uses.
    fn evict(&mut self) {
        if self.loaded.len() <= KEPT {
            return;
        }
        self.loaded.sort_by_key(|l| std::cmp::Reverse(l.used));
        self.loaded
            .truncate(KEPT.max(self.loaded.iter().filter(|l| l.used == self.clock).count()));
    }

    /// The index of `name`'s read in [`loaded`](Self::loaded), reading it
    /// first if it isn't there yet or (`poll`) its files changed.
    fn find_or_load(&mut self, scope: &AssetScope, name: &str, poll: bool) -> usize {
        let at = self
            .loaded
            .iter()
            .position(|l| l.scope == *scope && l.name == name);
        match at {
            Some(i) if !(poll && self.loaded[i].stamps.changed()) => i,
            Some(i) => {
                self.loaded[i] = self.read(scope, name);
                i
            }
            None => {
                let read = self.read(scope, name);
                self.loaded.push(read);
                self.loaded.len() - 1
            }
        }
    }

    fn read(&mut self, scope: &AssetScope, name: &str) -> Loaded {
        let mut stamps = FileStamps::default();
        let answer = scope.explain(self.use_.kind(), name);
        if let Some(change) = &answer.order_change {
            change.log();
        }
        stamps.watch_all(answer.watched());
        let mut loaded = Loaded {
            scope: scope.clone(),
            name: name.to_string(),
            levels: None,
            stats: ImageStats::BLANK,
            stamps,
            generation: GENERATION.fetch_add(1, Ordering::Relaxed),
            used: 0,
        };
        let Some(path) = answer.found else {
            self.warn(name, &not_found_text(&answer.tried));
            return loaded;
        };
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
                loaded.levels = Some(Arc::new(levels));
                loaded.stats = stats;
            }
            Err(e) => self.warn(name, &format!("{}: {e}", path.display())),
        }
        loaded
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

/// What a missing image's warning says: that there is no built-in image
/// (the style draws its procedural one), and every path tried.
fn not_found_text(tried: &[std::path::PathBuf]) -> String {
    let mut text = String::from("not found (no image is built in). Tried:");
    for path in tried {
        text.push_str("\n  ");
        text.push_str(&shown(path));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::vfs::{self, MemorySource};
    use std::path::{Path, PathBuf};

    /// ADR 0114: a bare name is `papers/<name>.png` in the scene's folder
    /// and each above it up to the project root, a path is taken from each
    /// of them, and a miss says every place tried.
    #[test]
    fn an_image_is_found_from_a_nested_scene_and_a_miss_says_where_it_looked() {
        let mut files = MemorySource::new();
        files.insert("game/flint.toml", "");
        files.insert("game/papers/cold_press.png", "");
        files.insert("game/strokes/hb.png", "");
        files.insert("game/scans/linen.jpg", "");
        files.insert("game/levels/act1/cave/a.scene.toml", "");
        vfs::scoped(Arc::new(files), || {
            let scope = AssetScope::for_dir(Path::new("game/levels/act1/cave"));
            let found = |use_: ImageUse, name: &str| scope.explain(use_.kind(), name).found;
            assert_eq!(
                found(ImageUse::Paper, "cold_press"),
                Some(PathBuf::from("game/papers/cold_press.png"))
            );
            assert_eq!(
                found(ImageUse::Paper, "papers/cold_press.png"),
                Some(PathBuf::from("game/papers/cold_press.png"))
            );
            assert_eq!(
                found(ImageUse::Paper, "scans/linen.jpg"),
                Some(PathBuf::from("game/scans/linen.jpg"))
            );
            assert_eq!(
                found(ImageUse::Stroke, "hb"),
                Some(PathBuf::from("game/strokes/hb.png"))
            );
            // A paper is not a stroke
            assert_eq!(found(ImageUse::Stroke, "cold_press"), None);
            let missing = scope.explain(AssetKind::Papers, "vellum");
            let text = not_found_text(&missing.tried);
            let lines: Vec<&str> = text.lines().map(str::trim).collect();
            assert_eq!(lines[0], "not found (no image is built in). Tried:");
            assert_eq!(lines[1], "game/levels/act1/cave/papers/vellum.png");
            assert!(lines.contains(&"game/papers/vellum.png"), "{text}");
            assert!(lines.contains(&"game/vellum.jpg"), "{text}");
        });
    }

    /// #547: each distinct name that loads gets a layer, in the order rows
    /// first name it; a row repeating a name shares its layer, and past the
    /// cap (or for a name that won't load) a row draws procedurally.
    #[test]
    fn rows_share_a_layer_by_name_up_to_the_cap() {
        let names: Vec<String> = [
            "none",
            "cold_press",
            "kraft",
            "",
            "cold_press",
            "missing",
            "washi",
            "cotton",
            "linen",
            "kraft",
            "linen",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut asked = Vec::new();
        let a = assign_layers(&names, &mut |n| {
            asked.push(n.to_string());
            n != "missing"
        });
        assert_eq!(a.layer, vec![0, 1, 2, 0, 1, 0, 3, 4, 0, 2, 0]);
        assert_eq!(a.stacked, ["cold_press", "kraft", "washi", "cotton"]);
        assert_eq!(a.over_cap, Some(8), "linen is the fifth paper");
        assert!(
            !asked.iter().any(|n| n == "none" || n.is_empty()),
            "no image is looked for: {asked:?}"
        );
        // One image, whoever names it, is one layer
        let one = assign_layers(&["".into(), "hb".into(), "hb".into()], &mut |_| true);
        assert_eq!((one.layer, one.stacked.len()), (vec![0, 1, 1], 1));
        assert_eq!(std::mem::size_of::<LayerInfo>(), 32);
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
