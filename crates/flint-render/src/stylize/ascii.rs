//! ASCII (#265): the full-screen, grid-aligned glyph style. Every cell of
//! one screen grid becomes a glyph from a [`GlyphSet`] (#264), picked by
//! the cell's luminance, inked in the cell's colour or a tint.
//!
//! ```toml
//! [post_process.styles.ascii]
//! cell = 8              # cell width in points; the height follows the font
//! font = "builtin"      # or a fonts/<name>.glyphs.toml set
//! ramp = "classic"      # a ramp of that set
//! color = "source"      # source | tint | raw | mono | palette
//! palette = "phosphor"  # for color = "palette": built-in or palettes/<name>.palette.toml
//! ground = "dim"        # dim | black | inverse
//! glow = 0.0            # halo around bright glyphs
//! ```
//!
//! It runs at the display stage: on the finished picture at output
//! resolution, after the render-scale upscale, so glyphs stay crisp at any
//! scale (the ASCII spike's finding 7, #261). The cell is in points, times
//! the host's [`StyleFrame::pixels_per_point`], so it is the same size to
//! the eye on a high-DPI screen. The grid is centred on the frame.
//!
//! ## Per entity (#266)
//!
//! An entity with an `ascii_style` component (or a `style_mask` naming
//! `ascii`) is drawn in glyphs with no table at all, the rest of the frame
//! passing through; `scope = "entities"` keeps a table as the preset those
//! entities start from. Each pixel's mask id picks a row of
//! [`AsciiUniforms::variants`] (slot 0 is the table's own), so props can
//! differ in cell, ramp, colour and grid in one pass. `grid = "object"`
//! pins a variant's cells to its root's projected origin.
//!
//! Colour modes (#276): `source` (the cell's hue), `tint` (one colour),
//! `raw` (the cell's colour as it is), `mono` (white on grey) and `palette`
//! (a [`Palette`]: nearest ink, as a CGA screen, or a tone ladder, as a
//! phosphor tube). They are per variant too: the frame's palettes are
//! packed into one table, as its ramps are. Edge glyphs (#267) build on it.

use super::kit;
use super::{
    StyleDescriptor, StyleFrame, StyleInput, StyleOption, StyleParam, StylePass, StyleStage,
};
use crate::glyph_atlas::{GlyphSet, GlyphSetCache, GpuGlyphAtlas};
use crate::palette::{Palette, PaletteCache, BUILTIN_PALETTES, MAX_COLORS};
use bytemuck::{Pod, Zeroable};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Ramp steps the uniform holds; a longer ramp is resampled to this many.
pub const MAX_RAMP: usize = 128;

/// The colour modes, by their option spelling.
pub const COLOR_MODES: &[&str] = &["source", "tint", "raw", "mono", "palette"];

/// The ground (background fill) modes, by their option spelling.
pub const GROUND_MODES: &[&str] = &["dim", "black", "inverse"];

/// The palette `color = "palette"` uses when the scene names none.
pub const DEFAULT_PALETTE: &str = "phosphor";

/// What a `[post_process.styles.ascii]` table covers: the whole frame, or
/// only the entities that ask (#266), the table then being their preset.
pub const SCOPES: &[&str] = &["screen", "entities"];

/// Where a variant's cells line up: one grid centred on the frame, or a
/// grid pinned to each styled root's projected origin (#266).
pub const GRIDS: &[&str] = &["screen", "object"];

/// Variants one frame can hold, the whole-frame one included; any more
/// are drawn as the whole frame is (logged once).
pub const MAX_VARIANTS: usize = 64;

/// Mask ids the variant table covers; a variant with a higher id is drawn
/// as the whole frame is (logged once).
pub const MAX_IDS: usize = 256;

/// A variant's flag: leave its pixels as they are.
const PASS_THROUGH: u32 = 1;

pub(super) static STYLE: StyleDescriptor = StyleDescriptor {
    name: "ascii",
    label: "ASCII",
    stage: StyleStage::Display,
    params: &[
        StyleParam {
            name: "cell",
            default: 8.0,
            min: 4.0,
            max: 32.0,
        },
        StyleParam {
            name: "brightness",
            default: 0.0,
            min: -1.0,
            max: 1.0,
        },
        StyleParam {
            name: "contrast",
            default: 1.0,
            min: 0.0,
            max: 4.0,
        },
        StyleParam {
            name: "background",
            default: 0.15,
            min: 0.0,
            max: 1.0,
        },
        StyleParam {
            name: "ink",
            default: 1.0,
            min: 0.0,
            max: 2.0,
        },
        StyleParam {
            name: "levels",
            default: 0.0,
            min: 0.0,
            max: 16.0,
        },
        StyleParam {
            name: "footprint",
            default: 1.0,
            min: 1.0,
            max: 4.0,
        },
        StyleParam {
            name: "tint_r",
            default: 0.35,
            min: 0.0,
            max: 1.0,
        },
        StyleParam {
            name: "tint_g",
            default: 1.0,
            min: 0.0,
            max: 1.0,
        },
        StyleParam {
            name: "tint_b",
            default: 0.45,
            min: 0.0,
            max: 1.0,
        },
        StyleParam {
            name: "glow",
            default: 0.0,
            min: 0.0,
            max: 2.0,
        },
    ],
    options: &[
        StyleOption {
            name: "font",
            default: "builtin",
            choices: &[],
        },
        StyleOption {
            name: "ramp",
            default: "",
            choices: &["classic", "blocks", "minimal", "detailed"],
        },
        StyleOption {
            name: "color",
            default: "source",
            choices: COLOR_MODES,
        },
        StyleOption {
            name: "palette",
            default: DEFAULT_PALETTE,
            choices: BUILTIN_PALETTES,
        },
        StyleOption {
            name: "ground",
            default: "dim",
            choices: GROUND_MODES,
        },
        StyleOption {
            name: "scope",
            default: "screen",
            choices: SCOPES,
        },
        StyleOption {
            name: "grid",
            default: "screen",
            choices: GRIDS,
        },
    ],
    enabled: None,
    uses_mask: false,
    component: Some("ascii_style"),
    create: |d| {
        let device = d.device;
        let uniform_bgl = kit::uniform_layout(device, "ASCII Uniform BGL");
        let input_bgl = kit::sampled_layout(device, "ASCII Input BGL");
        let atlas_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ASCII Atlas BGL"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let mask_bgl = kit::mask_layout(device, "ASCII Mask BGL");
        // Bound while no entity asks for the style: one unstyled pixel
        let no_mask = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ASCII No Mask"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: crate::style_mask::STYLE_MASK_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let no_mask_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ASCII No Mask BG"),
            layout: &mask_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(
                    &no_mask.create_view(&Default::default()),
                ),
            }],
        });
        Box::new(Ascii {
            pipeline: kit::fullscreen_pipeline(
                device,
                "ASCII Pipeline",
                &kit::shader(device, "ASCII Shader", include_str!("ascii.wgsl")),
                &[&uniform_bgl, &input_bgl, &atlas_bgl, &mask_bgl],
                d.format,
            ),
            mask_bgl,
            _no_mask: no_mask,
            no_mask_bind,
            uniforms: kit::uniform_buffer::<AsciiUniforms>(device, "ASCII Uniforms"),
            uniform_bgl,
            input_bgl,
            atlas_bgl,
            fonts: Mutex::new(Fonts::default()),
        })
    },
};

/// One variant's settings: the whole frame's (slot 0) or one per-entity
/// variant's (#266).
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct VariantUniform {
    cell: [f32; 2],
    origin: [f32; 2],
    tint: [f32; 3],
    color_mode: u32,
    brightness: f32,
    contrast: f32,
    background: f32,
    ink: f32,
    levels: f32,
    footprint: f32,
    ramp_start: u32,
    ramp_len: u32,
    flags: u32,
    glow: f32,
    ground: u32,
    palette_start: u32,
    palette_bg: [f32; 3],
    palette_len: u32,
    palette_match: u32,
    _pad: [u32; 3],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct AsciiUniforms {
    glyph: [f32; 2],
    size: [f32; 2],
    srgb: u32,
    variant_count: u32,
    _pad: [u32; 2],
    variants: [VariantUniform; MAX_VARIANTS],
    /// The variant slot of each mask id, four ids a vec4.
    slots: [[u32; 4]; MAX_IDS / 4],
    /// Every variant's ramp, one after another: each step's atlas cell
    /// origin, two steps a vec4.
    ramp: [[u32; 4]; MAX_RAMP / 2],
    /// Every variant's palette inks, one palette after another (display
    /// values; rgb, unused w).
    palette: [[f32; 4]; MAX_COLORS],
}

/// The cell, in output pixels, for a cell `points` wide at
/// `pixels_per_point`, with a font whose glyphs are `glyph` pixels: whole
/// pixels, at least 2 wide, the height keeping the glyph's aspect.
pub fn cell_pixels(points: f32, pixels_per_point: f32, glyph: [u32; 2]) -> [f32; 2] {
    let ppp = if pixels_per_point.is_finite() && pixels_per_point > 0.0 {
        pixels_per_point
    } else {
        1.0
    };
    let w = (points.max(1.0) * ppp).round().max(2.0);
    let aspect = glyph[1].max(1) as f32 / glyph[0].max(1) as f32;
    [w, (w * aspect).round().max(2.0)]
}

/// Where the grid starts so that the part cells at the borders match: half
/// of what's left over on each axis.
pub fn grid_origin(size: [u32; 2], cell: [f32; 2]) -> [f32; 2] {
    let left = |n: u32, c: f32| ((n as f32) % c) * 0.5;
    [
        left(size[0], cell[0]).floor(),
        left(size[1], cell[1]).floor(),
    ]
}

/// The colour mode's shader number; an unknown name is source colour.
pub fn color_mode(name: &str) -> u32 {
    COLOR_MODES.iter().position(|m| *m == name).unwrap_or(0) as u32
}

/// The ground mode's shader number; an unknown name is `dim`.
pub fn ground_mode(name: &str) -> u32 {
    GROUND_MODES.iter().position(|m| *m == name).unwrap_or(0) as u32
}

/// Up to [`MAX_RAMP`] steps of `indices`, evenly resampled when longer.
fn fit_ramp(indices: &[u32]) -> Vec<u32> {
    if indices.len() <= MAX_RAMP {
        return indices.to_vec();
    }
    (0..MAX_RAMP)
        .map(|i| indices[i * (indices.len() - 1) / (MAX_RAMP - 1)])
        .collect()
}

/// The set in use and its atlas on the GPU.
struct LoadedFont {
    name: String,
    set: Arc<GlyphSet>,
    _gpu: GpuGlyphAtlas,
    bind: wgpu::BindGroup,
}

#[derive(Default)]
struct Fonts {
    search: Vec<PathBuf>,
    cache: GlyphSetCache,
    palettes: PaletteCache,
    loaded: Option<LoadedFont>,
    /// The last (font, ramp) a warning was given for, so it's said once.
    warned: Option<(String, String)>,
    /// Per-entity warnings already given (too many variants, a font of
    /// its own, a ramp that doesn't fit), so each is said once.
    warned_variants: std::collections::HashSet<String>,
}

impl Fonts {
    /// Look in the scene's folder and its parent, as the player finds
    /// `fonts/`, then the working directory.
    fn search_for(scene_dir: Option<&Path>) -> Vec<PathBuf> {
        let mut search = Vec::new();
        if let Some(dir) = scene_dir {
            search.push(dir.to_path_buf());
            if let Some(parent) = dir.parent() {
                search.push(parent.to_path_buf());
            }
        }
        search.push(PathBuf::from("."));
        search
    }
}

struct Ascii {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    uniform_bgl: wgpu::BindGroupLayout,
    input_bgl: wgpu::BindGroupLayout,
    atlas_bgl: wgpu::BindGroupLayout,
    mask_bgl: wgpu::BindGroupLayout,
    _no_mask: wgpu::Texture,
    no_mask_bind: wgpu::BindGroup,
    fonts: Mutex<Fonts>,
}

/// A variant's settings, read through the getters of the whole frame or of
/// one per-entity variant.
fn variant_uniform(
    param: &dyn Fn(&str) -> f32,
    option: &dyn Fn(&str) -> String,
    cell: [f32; 2],
    origin: [f32; 2],
    ramp: (u32, u32),
    palette: &PalettePlace,
    flags: u32,
) -> VariantUniform {
    VariantUniform {
        cell,
        origin,
        tint: [param("tint_r"), param("tint_g"), param("tint_b")],
        color_mode: color_mode(&option("color")),
        brightness: param("brightness"),
        contrast: param("contrast").max(0.0),
        background: param("background").clamp(0.0, 1.0),
        ink: param("ink").max(0.0),
        levels: param("levels").max(0.0),
        footprint: param("footprint").clamp(1.0, 8.0),
        ramp_start: ramp.0,
        ramp_len: ramp.1,
        flags,
        glow: param("glow").max(0.0),
        ground: ground_mode(&option("ground")),
        palette_start: palette.start,
        palette_bg: palette.background,
        palette_len: palette.len,
        palette_match: palette.matching,
        _pad: [0; 3],
    }
}

/// Where one palette sits in the uniform's packed inks.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct PalettePlace {
    start: u32,
    len: u32,
    matching: u32,
    background: [f32; 3],
}

/// The palettes in use, packed one after another into the uniform.
#[derive(Default)]
struct Palettes {
    inks: Vec<[f32; 4]>,
    placed: Vec<(String, PalettePlace)>,
}

impl Palettes {
    /// Where `palette` sits, packing it on first use; `None` when its inks
    /// don't fit in what's left.
    fn place(&mut self, palette: &Palette) -> Option<PalettePlace> {
        if let Some((_, at)) = self.placed.iter().find(|(n, _)| n == palette.name()) {
            return Some(*at);
        }
        let colors = palette.colors();
        if colors.is_empty() || self.inks.len() + colors.len() > MAX_COLORS {
            return None;
        }
        let at = PalettePlace {
            start: self.inks.len() as u32,
            len: colors.len() as u32,
            matching: palette.matching().index(),
            background: palette.background(),
        };
        self.inks
            .extend(colors.iter().map(|c| [c[0], c[1], c[2], 1.0]));
        self.placed.push((palette.name().to_string(), at));
        Some(at)
    }
}

/// Where an anchored grid starts: a cell centred on `anchor`, folded into
/// the first cell so the shader's arithmetic stays small.
pub fn anchored_origin(anchor: [f32; 2], cell: [f32; 2]) -> [f32; 2] {
    [
        (anchor[0] - 0.5 * cell[0]).rem_euclid(cell[0]).round(),
        (anchor[1] - 0.5 * cell[1]).rem_euclid(cell[1]).round(),
    ]
}

/// The ramps in use, packed one after another into the uniform.
#[derive(Default)]
struct Ramps {
    steps: Vec<u32>,
    placed: Vec<(String, (u32, u32))>,
}

impl Ramps {
    /// Where ramp `name` of `set` sits, packing it on first use; `None`
    /// when it doesn't fit in what's left.
    fn place(&mut self, set: &GlyphSet, name: &str) -> Option<(u32, u32)> {
        if let Some((_, at)) = self.placed.iter().find(|(n, _)| n == name) {
            return Some(*at);
        }
        let ramp = set.ramp(name).unwrap_or_else(|| set.default_ramp());
        let fit = fit_ramp(ramp.atlas_indices());
        if self.steps.len() + fit.len() > MAX_RAMP {
            return None;
        }
        let at = (self.steps.len() as u32, fit.len() as u32);
        self.steps.extend(fit);
        self.placed.push((name.to_string(), at));
        Some(at)
    }
}

impl Ascii {
    /// The font the config names, uploaded; the built-in set when it can't
    /// be loaded (the cache has warned).
    fn font<'f>(&self, frame: &StyleFrame<'_>, fonts: &'f mut Fonts) -> &'f LoadedFont {
        let search = Fonts::search_for(frame.scene_dir);
        if fonts.search != search {
            fonts.cache = GlyphSetCache::new(search.clone());
            fonts.palettes = PaletteCache::new(search.clone());
            fonts.search = search;
            fonts.loaded = None;
        }
        let name = frame.option("font").to_string();
        let set = fonts
            .cache
            .get(&name)
            .unwrap_or_else(|_| GlyphSet::builtin());
        let stale = fonts
            .loaded
            .as_ref()
            .is_none_or(|l| l.name != name || !Arc::ptr_eq(&l.set, &set));
        if stale {
            let gpu = GpuGlyphAtlas::upload(frame.device, frame.queue, set.atlas(), "ASCII Atlas");
            let bind = frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ASCII Atlas BG"),
                layout: &self.atlas_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&gpu.view),
                }],
            });
            fonts.loaded = Some(LoadedFont {
                name,
                set,
                _gpu: gpu,
                bind,
            });
        }
        fonts.loaded.as_ref().expect("loaded above")
    }
}

impl StylePass for Ascii {
    fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    ) {
        let mut fonts = self.fonts.lock().unwrap_or_else(|e| e.into_inner());
        let ramp_name = frame.option("ramp").to_string();
        let set = self.font(frame, &mut fonts).set.clone();
        if set.ramp(&ramp_name).is_none() {
            let key = (set.name().to_string(), ramp_name.clone());
            if !ramp_name.is_empty() && fonts.warned.as_ref() != Some(&key) {
                tracing::warn!(
                    "ascii: glyph set `{}` has no ramp `{ramp_name}`; using `{}` ({})",
                    set.name(),
                    set.default_ramp().name(),
                    set.ramps().map(|r| r.name()).collect::<Vec<_>>().join(", ")
                );
                fonts.warned = Some(key);
            }
        }
        let atlas = set.atlas();
        let glyph = atlas.cell_size();
        let mut ramps = Ramps::default();
        let base_ramp = ramps
            .place(&set, &ramp_name)
            .expect("the first ramp is resampled to fit");
        // Palettes: the scene's first, so a variant's that can't fit falls
        // back on it. One that can't be loaded is the default, as on #276.
        let mut palettes = Palettes::default();
        let palette_for =
            |fonts: &mut Fonts, color: &str, name: &str| -> Option<Arc<Palette>> {
                if color != "palette" {
                    return None;
                }
                let name = if name.is_empty() {
                    DEFAULT_PALETTE
                } else {
                    name
                };
                Some(
                    fonts
                        .palettes
                        .get(name)
                        .ok()
                        .unwrap_or_else(|| Palette::builtin(DEFAULT_PALETTE).expect("built-in")),
                )
            };
        let base_palette = palette_for(&mut fonts, frame.option("color"), frame.option("palette"))
            .and_then(|p| palettes.place(&p))
            .unwrap_or_default();

        // Slot 0: the whole frame, glyphized when the scene's table covers
        // it, left alone when only entities ask for the style (#266)
        let whole = frame.whole_frame() && frame.option("scope") != "entities";
        let cell = cell_pixels(frame.param("cell"), frame.pixels_per_point, glyph);
        let mut variants = vec![variant_uniform(
            &|n| frame.param(n),
            &|n| frame.option(n).to_string(),
            cell,
            grid_origin([frame.width, frame.height], cell),
            base_ramp,
            &base_palette,
            if whole { 0 } else { PASS_THROUGH },
        )];
        let mut slots = [0u32; MAX_IDS];
        let base_font = frame.option("font").to_string();
        for (id, v) in frame.variants() {
            let mut warn_once = |key: String, text: String| {
                if fonts.warned_variants.insert(key) {
                    tracing::warn!("ascii: {text}");
                }
            };
            if variants.len() == MAX_VARIANTS || id as usize >= MAX_IDS {
                warn_once(
                    "overflow".into(),
                    format!(
                        "more per-entity variants than the {} (or ids past {MAX_IDS}) one \
                         frame holds; the rest are drawn like the whole frame",
                        MAX_VARIANTS - 1
                    ),
                );
                continue;
            }
            let param = |n: &str| frame.variant_param(v, n);
            let option = |n: &str| frame.variant_option(v, n).to_string();
            if option("font") != base_font {
                warn_once(
                    format!("font {}", option("font")),
                    format!(
                        "per-entity fonts aren't supported yet; `{}` is drawn in `{base_font}`",
                        option("font")
                    ),
                );
            }
            let ramp = ramps.place(&set, &option("ramp")).unwrap_or_else(|| {
                warn_once(
                    format!("ramp {}", option("ramp")),
                    format!(
                        "ramp `{}` doesn't fit beside the others ({MAX_RAMP} steps in all); \
                         using the scene's",
                        option("ramp")
                    ),
                );
                base_ramp
            });
            let palette = match palette_for(&mut fonts, &option("color"), &option("palette")) {
                None => PalettePlace::default(),
                Some(p) => palettes.place(&p).unwrap_or_else(|| {
                    if fonts
                        .warned_variants
                        .insert(format!("palette {}", p.name()))
                    {
                        tracing::warn!(
                            "ascii: palette `{}` doesn't fit beside the others ({MAX_COLORS} inks \
                             in all); using the scene's",
                            p.name()
                        );
                    }
                    palettes
                        .placed
                        .first()
                        .map(|(_, at)| *at)
                        .unwrap_or_default()
                }),
            };
            let cell = cell_pixels(param("cell"), frame.pixels_per_point, glyph);
            let origin = v.anchor.and_then(|a| frame.project(a)).map_or_else(
                || grid_origin([frame.width, frame.height], cell),
                |at| anchored_origin(at, cell),
            );
            let flags = if v.off { PASS_THROUGH } else { 0 };
            slots[id as usize] = variants.len() as u32;
            variants.push(variant_uniform(
                &param, &option, cell, origin, ramp, &palette, flags,
            ));
        }

        let mut uniforms = AsciiUniforms::zeroed();
        uniforms.glyph = [glyph[0] as f32, glyph[1] as f32];
        uniforms.size = [frame.width as f32, frame.height as f32];
        uniforms.srgb = frame.srgb() as u32;
        uniforms.variant_count = variants.len() as u32;
        uniforms.variants[..variants.len()].copy_from_slice(&variants);
        for (i, slot) in slots.iter().enumerate() {
            uniforms.slots[i / 4][i % 4] = *slot;
        }
        uniforms.palette[..palettes.inks.len()].copy_from_slice(&palettes.inks);
        for (i, index) in ramps.steps.iter().enumerate() {
            let [x, y] = atlas.cell_origin(*index);
            let pair = &mut uniforms.ramp[i / 2];
            pair[(i % 2) * 2] = x;
            pair[(i % 2) * 2 + 1] = y;
        }
        // The mask, when entities ask (it's drawn whenever one names the
        // style); else the one-pixel stand-in, which reads as unstyled
        let mask = frame
            .mask
            .filter(|_| variants.len() > 1)
            .and_then(|_| frame.mask_bind("ASCII Mask BG", &self.mask_bgl));
        frame
            .queue
            .write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
        let font = fonts.loaded.as_ref().expect("loaded by font()");
        kit::fullscreen_pass(
            encoder,
            "ASCII Pass",
            output,
            &self.pipeline,
            &[
                &frame.uniform("ASCII Uniform BG", &self.uniform_bgl, &self.uniforms),
                &frame.sampled_input("ASCII Input BG", &self.input_bgl, input),
                &font.bind,
                mask.as_deref().unwrap_or(&self.no_mask_bind),
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniforms_match_the_shader_layout() {
        // 32 bytes of header, 64 variants of 112, 64 + 64 vec4<u32>, then
        // the palette's 32 vec4<f32>
        assert_eq!(std::mem::size_of::<VariantUniform>(), 112);
        assert_eq!(
            std::mem::size_of::<AsciiUniforms>(),
            32 + MAX_VARIANTS * 112 + 2 * 64 * 16 + MAX_COLORS * 16
        );
    }

    #[test]
    fn cells_scale_with_pixels_per_point_and_keep_the_glyph_aspect() {
        assert_eq!(cell_pixels(8.0, 1.0, [8, 8]), [8.0, 8.0]);
        assert_eq!(cell_pixels(8.0, 2.0, [8, 8]), [16.0, 16.0]);
        assert_eq!(cell_pixels(8.0, 1.5, [8, 8]), [12.0, 12.0]);
        assert_eq!(cell_pixels(8.0, 1.0, [8, 16]), [8.0, 16.0]);
        // Never below 2 px, and a broken scale factor counts as 1
        assert_eq!(cell_pixels(0.5, 1.0, [8, 8]), [2.0, 2.0]);
        assert_eq!(cell_pixels(8.0, f32::NAN, [8, 8]), [8.0, 8.0]);
        assert_eq!(cell_pixels(8.0, 0.0, [8, 8]), [8.0, 8.0]);
    }

    #[test]
    fn the_grid_is_centred() {
        // 1280 / 8 is whole; 720 / 12 is whole; 1283 leaves 3: 1 on the left
        assert_eq!(grid_origin([1280, 720], [8.0, 8.0]), [0.0, 0.0]);
        assert_eq!(grid_origin([1283, 725], [8.0, 8.0]), [1.0, 2.0]);
        assert_eq!(grid_origin([1920, 1080], [12.0, 12.0]), [0.0, 0.0]);
    }

    #[test]
    fn colour_modes_parse_and_fall_back_to_source() {
        assert_eq!(color_mode("source"), 0);
        assert_eq!(color_mode("tint"), 1);
        assert_eq!(color_mode("raw"), 2);
        assert_eq!(color_mode("mono"), 3);
        assert_eq!(color_mode("palette"), 4);
        assert_eq!(color_mode("plaid"), 0);
        assert_eq!(ground_mode("dim"), 0);
        assert_eq!(ground_mode("black"), 1);
        assert_eq!(ground_mode("inverse"), 2);
        assert_eq!(ground_mode(""), 0);
    }

    #[test]
    fn every_offered_palette_is_built_in() {
        let palette = STYLE.options.iter().find(|o| o.name == "palette").unwrap();
        assert!(Palette::builtin(palette.default).is_some());
        for choice in palette.choices {
            assert!(Palette::builtin(choice).is_some(), "{choice}");
        }
    }

    #[test]
    fn palettes_pack_once_each_and_refuse_what_does_not_fit() {
        let mut palettes = Palettes::default();
        let cga = Palette::builtin("cga").unwrap();
        let at = palettes.place(&cga).unwrap();
        assert_eq!((at.start, at.len as usize), (0, cga.colors().len()));
        let amber = Palette::builtin("amber").unwrap();
        let next = palettes.place(&amber).unwrap();
        assert_eq!(next.start, at.len, "packed after the first");
        assert_eq!(palettes.place(&cga), Some(at), "placed once");
        // cga16's 15 inks don't fit in what's left of 32 after more of them
        palettes.inks.resize(MAX_COLORS - 2, [0.0; 4]);
        assert_eq!(palettes.place(&Palette::builtin("cga16").unwrap()), None);
    }

    #[test]
    fn long_ramps_are_resampled_end_to_end() {
        let short: Vec<u32> = (0..10).collect();
        assert_eq!(fit_ramp(&short), short);
        let long: Vec<u32> = (0..300).collect();
        let fit = fit_ramp(&long);
        assert_eq!(fit.len(), MAX_RAMP);
        assert_eq!(fit[0], 0);
        assert_eq!(*fit.last().unwrap(), 299);
        assert!(fit.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn every_builtin_ramp_fits_without_resampling() {
        let set = GlyphSet::builtin();
        for ramp in set.ramps() {
            assert!(ramp.len() <= MAX_RAMP, "{}", ramp.name());
        }
        for choice in STYLE
            .options
            .iter()
            .find(|o| o.name == "ramp")
            .unwrap()
            .choices
        {
            assert!(
                set.ramp(choice).is_some(),
                "built-in set has no ramp {choice}"
            );
        }
    }

    #[test]
    fn the_shader_validates_and_its_uniforms_match_rust() {
        let module = naga::front::wgsl::parse_str(include_str!("ascii.wgsl")).expect("parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("ascii.wgsl validates");
        let (_, u) = module
            .global_variables
            .iter()
            .find(|(_, v)| v.name.as_deref() == Some("u"))
            .expect("uniform u");
        let size = module.types[u.ty].inner.size(module.to_ctx());
        assert_eq!(size as usize, std::mem::size_of::<AsciiUniforms>());
    }

    #[test]
    fn an_anchored_grid_centres_a_cell_on_its_anchor() {
        // A cell centred on (100, 50): its corner is at (96, 46), which
        // folds to (0, 6) for 8 px cells
        assert_eq!(anchored_origin([100.0, 50.0], [8.0, 8.0]), [0.0, 6.0]);
        // Off screen to the left still folds into the first cell
        assert_eq!(anchored_origin([-3.0, 4.0], [8.0, 8.0]), [1.0, 0.0]);
        let o = anchored_origin([1234.5, 777.25], [12.0, 12.0]);
        assert!(o.iter().all(|c| (0.0..=12.0).contains(c)), "{o:?}");
    }

    #[test]
    fn ramps_pack_once_each_and_refuse_what_does_not_fit() {
        let set = GlyphSet::builtin();
        let mut ramps = Ramps::default();
        let classic = ramps.place(&set, "classic").unwrap();
        assert_eq!(classic.0, 0);
        let blocks = ramps.place(&set, "blocks").unwrap();
        assert_eq!(blocks.0, classic.1, "packed after the first");
        assert_eq!(ramps.place(&set, "classic"), Some(classic), "placed once");
        // Fill what's left, then one more doesn't fit
        ramps.steps.resize(MAX_RAMP, 0);
        assert_eq!(ramps.place(&set, "minimal"), None);
    }
}
