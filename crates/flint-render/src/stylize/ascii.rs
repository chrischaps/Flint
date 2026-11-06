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
//! Colour modes (#276): `source` (the cell's hue), `tint` (one colour),
//! `raw` (the cell's colour as it is), `mono` (white on grey) and `palette`
//! (a [`Palette`]: nearest ink, as a CGA screen, or a tone ladder, as a
//! phosphor tube).
//!
//! It draws in two passes. The cell pass writes one texel a cell (the
//! cell's colour and its ramp step) into a small state texture; the glyph
//! pass draws each cell's glyph from it. The state is kept from frame to
//! frame for temporal stability (#277): a cell keeps part of last frame's
//! colour (`smoothing`) and last frame's glyph while its tone stays within
//! `hysteresis` ramp steps of it, and an optional grid-fixed ordered
//! `dither` breaks gradients into a stable pattern. `temporal = "off"`
//! draws every frame as if it were the first. History counts only when
//! the last frame the renderer drew was the one before and nothing about
//! the grid, ramp or [`StyleFrame::history_epoch`] changed, so a single
//! headless render is exactly the stateless style's, and a
//! `--frame-step` capture gives it the same frames every run.
//!
//! Edge glyphs (#267) and per-entity selection (#266) build on it.

use super::kit;
use super::{
    StyleDescriptor, StyleFrame, StyleHint, StyleInput, StyleOption, StyleParam, StylePass,
    StyleStage, StyleTexture,
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
            name: "smoothing",
            default: 0.6,
            min: 0.0,
            max: 0.95,
        },
        StyleParam {
            name: "hysteresis",
            default: 0.35,
            min: 0.0,
            max: 1.0,
        },
        StyleParam {
            name: "dither",
            default: 0.0,
            min: 0.0,
            max: 1.0,
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
            name: "temporal",
            default: "on",
            choices: TEMPORAL_MODES,
        },
    ],
    enabled: None,
    uses_mask: false,
    hints: &[
        StyleHint::new("font", "Font", "builtin, or a fonts/<name>.glyphs.toml set beside the scene"),
        StyleHint::new("ramp", "Ramp", "Which of the font's ramps maps tone to glyphs; (default) is the font's own"),
        StyleHint::new("color", "Colour mode", "source: the cell's hue; tint: one colour; raw: the cell's colour; mono: white; palette: a palette's inks"),
        StyleHint::new("palette", "Palette", "For colour mode palette: a built-in, or palettes/<name>.palette.toml"),
        StyleHint::new("ground", "Ground", "Behind the glyphs: dim scene, black, or inverse (ink fills the cell, the glyph is cut out)"),
        StyleHint::new("cell", "Cell size (pt)", "Cell width in points; the height follows the font").integer(),
        StyleHint::new("brightness", "Brightness", "Shifts the tone before a glyph is picked"),
        StyleHint::new("contrast", "Contrast", "Spreads the tone around mid-grey before a glyph is picked"),
        StyleHint::new("background", "Background", "How much of the scene shows behind the glyphs (dim ground)"),
        StyleHint::new("ink", "Ink", "Multiplies the glyph colour"),
        StyleHint::new("levels", "Tone bands", "Quantise tone to this many bands; 0 = smooth").integer(),
        StyleHint::new("footprint", "Footprint (cells)", "How many cells wide each cell's colour is averaged over").integer(),
        StyleHint::new(
            "temporal",
            "Temporal",
            "on: cells keep part of last frame so glyphs don't shimmer in motion; off: every frame stands alone",
        ),
        StyleHint::new(
            "smoothing",
            "Smoothing",
            "How much of last frame's colour a cell keeps; a cell whose colour jumps takes the new one at once",
        ),
        StyleHint::new(
            "hysteresis",
            "Hysteresis",
            "How far, in ramp steps, a cell's tone may stray before its glyph changes",
        ),
        StyleHint::new(
            "dither",
            "Dither",
            "Ordered 4x4 dither before the ramp, fixed to the grid: gradients become a still pattern",
        ),
        StyleHint::new("tint","Tint", "The ink for colour mode tint"),
        StyleHint::new("glow", "Glow", "A halo around bright glyphs, inside each cell"),
    ],
    create: |d| {
        let device = d.device;
        let uniform_bgl = kit::uniform_layout(device, "ASCII Uniform BGL");
        let input_bgl = kit::sampled_layout(device, "ASCII Input BGL");
        let cells_bgl = kit::uint_layout(device, "ASCII Cells BGL");
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
        Box::new(Ascii {
            cells_pipeline: kit::fullscreen_pipeline(
                device,
                "ASCII Cells Pipeline",
                &kit::shader(device, "ASCII Cells Shader", CELLS_SHADER),
                &[&uniform_bgl, &input_bgl, &cells_bgl],
                CELL_FORMAT,
            ),
            pipeline: kit::fullscreen_pipeline(
                device,
                "ASCII Pipeline",
                &kit::shader(device, "ASCII Shader", GLYPH_SHADER),
                &[&uniform_bgl, &cells_bgl, &atlas_bgl],
                d.format,
            ),
            uniforms: kit::uniform_buffer::<AsciiUniforms>(device, "ASCII Uniforms"),
            uniform_bgl,
            input_bgl,
            cells_bgl,
            atlas_bgl,
            fonts: Mutex::new(Fonts::default()),
            history: Mutex::new(History::default()),
        })
    },
};

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct AsciiUniforms {
    cell: [f32; 2],
    origin: [f32; 2],
    glyph: [f32; 2],
    size: [f32; 2],
    tint: [f32; 3],
    color_mode: u32,
    brightness: f32,
    contrast: f32,
    background: f32,
    ink: f32,
    levels: f32,
    footprint: f32,
    ramp_len: u32,
    srgb: u32,
    glow: f32,
    ground: u32,
    palette_len: u32,
    palette_match: u32,
    palette_bg: [f32; 3],
    _pad: u32,
    smoothing: f32,
    hysteresis: f32,
    dither: f32,
    history: u32,
    ramp: [[u32; 4]; MAX_RAMP / 2],
    palette: [[f32; 4]; MAX_COLORS],
}

/// The cell pass's output: one texel a cell, the cell's colour as 16-bit
/// unorm in RGB and its ramp step in A.
pub const CELL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Uint;

const CELLS_SHADER: &str = concat!(
    include_str!("ascii_common.wgsl"),
    include_str!("ascii_cells.wgsl")
);
const GLYPH_SHADER: &str = concat!(
    include_str!("ascii_common.wgsl"),
    include_str!("ascii.wgsl")
);

/// The `temporal` option's spellings: `off` draws every frame as if it
/// were the first.
pub const TEMPORAL_MODES: &[&str] = &["on", "off"];

/// The cell state texture's size for a `size` frame: every whole and part
/// cell, plus the one texel left of (above) the origin the shaders offset
/// by (`CELL_OFFSET`).
pub fn cell_grid(size: [u32; 2], cell: [f32; 2], origin: [f32; 2]) -> [u32; 2] {
    let n = |len: u32, c: f32, o: f32| ((len as f32 - o) / c).ceil().max(0.0) as u32 + 1;
    [
        n(size[0], cell[0], origin[0]),
        n(size[1], cell[1], origin[1]),
    ]
}

/// What last frame's cells must share with this frame's to be its past:
/// the same grid over the same picture size, the same ramp (a step means
/// a glyph only within one ramp) and the same history epoch.
#[derive(Clone, PartialEq, Debug)]
struct HistoryKey {
    epoch: u64,
    size: [u32; 2],
    grid: [u32; 2],
    cell: [u32; 2],
    origin: [u32; 2],
    font: String,
    ramp: String,
    ramp_len: u32,
}

/// The two cell state textures the style ping-pongs and what it last drew.
#[derive(Default)]
struct History {
    /// Held so a texture's address can't come back as another's while a
    /// bind group over it is cached; `generation` keys those bind groups.
    textures: Option<[Arc<StyleTexture>; 2]>,
    generation: u64,
    /// Which texture this frame writes.
    parity: usize,
    /// The key and frame number of the last frame drawn.
    last: Option<(HistoryKey, u64)>,
}

impl History {
    /// Take this frame's textures; true when they're not last frame's
    /// (made again on a resize or cell change), so nothing in them is
    /// history.
    fn adopt(&mut self, textures: [Arc<StyleTexture>; 2]) -> bool {
        let same = self
            .textures
            .as_ref()
            .is_some_and(|t| Arc::ptr_eq(&t[0], &textures[0]) && Arc::ptr_eq(&t[1], &textures[1]));
        if !same {
            self.textures = Some(textures);
            self.generation += 1;
            self.parity = 0;
        }
        !same
    }

    /// Whether last frame's cells hold this frame's past, and note this
    /// frame as drawn.
    fn advance(&mut self, key: HistoryKey, frame: u64, temporal: bool) -> bool {
        let follows = self
            .last
            .as_ref()
            .is_some_and(|(k, f)| *k == key && f.wrapping_add(1) == frame);
        self.last = Some((key, frame));
        temporal && follows
    }
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
    cells_pipeline: wgpu::RenderPipeline,
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    uniform_bgl: wgpu::BindGroupLayout,
    input_bgl: wgpu::BindGroupLayout,
    cells_bgl: wgpu::BindGroupLayout,
    atlas_bgl: wgpu::BindGroupLayout,
    fonts: Mutex<Fonts>,
    history: Mutex<History>,
}

/// A kept bind group of one cell state texture, keyed by the history's
/// generation and which of the two it is.
fn cells_bind(
    frame: &StyleFrame<'_>,
    name: &'static str,
    layout: &wgpu::BindGroupLayout,
    texture: &StyleTexture,
    variant: u64,
) -> Arc<wgpu::BindGroup> {
    frame.bind(name, variant, || {
        frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(name),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&texture.view),
            }],
        })
    })
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
        let ramp = match set.ramp(&ramp_name) {
            Some(r) => r,
            None => {
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
                set.default_ramp()
            }
        };
        let atlas = set.atlas();
        let glyph = atlas.cell_size();
        let steps = fit_ramp(ramp.atlas_indices());
        let mut ramp_origins = [[0u32; 4]; MAX_RAMP / 2];
        for (i, index) in steps.iter().enumerate() {
            let [x, y] = atlas.cell_origin(*index);
            let slot = &mut ramp_origins[i / 2];
            slot[(i % 2) * 2] = x;
            slot[(i % 2) * 2 + 1] = y;
        }
        let palette = if frame.option("color") == "palette" {
            let name = match frame.option("palette") {
                "" => DEFAULT_PALETTE,
                n => n,
            };
            fonts.palettes.get(name).ok()
        } else {
            None
        }
        .unwrap_or_else(|| Palette::builtin(DEFAULT_PALETTE).expect("built-in"));
        let cell = cell_pixels(frame.param("cell"), frame.pixels_per_point, glyph);
        let size = [frame.width, frame.height];
        let origin = grid_origin(size, cell);
        let grid = cell_grid(size, cell, origin);

        // Last frame's cells and this frame's, ping-ponged (#277)
        let mut history = self.history.lock().unwrap_or_else(|e| e.into_inner());
        let textures = [
            frame.target_sized("ascii.cells.a", CELL_FORMAT, grid[0], grid[1]),
            frame.target_sized("ascii.cells.b", CELL_FORMAT, grid[0], grid[1]),
        ];
        let fresh = history.adopt(textures);
        let key = HistoryKey {
            epoch: frame.history_epoch,
            size,
            grid,
            cell: cell.map(f32::to_bits),
            origin: origin.map(f32::to_bits),
            font: set.name().to_string(),
            ramp: ramp.name().to_string(),
            ramp_len: steps.len() as u32,
        };
        let temporal = frame.option("temporal") != "off";
        let past = history.advance(key, frame.frame, temporal) && !fresh;
        let (write, read) = (history.parity, 1 - history.parity);
        history.parity = read;
        let generation = history.generation;
        let pair = history.textures.clone().expect("adopted above");
        drop(history);

        let uniforms = AsciiUniforms {
            cell,
            origin,
            glyph: [glyph[0] as f32, glyph[1] as f32],
            size: [frame.width as f32, frame.height as f32],
            tint: [
                frame.param("tint_r"),
                frame.param("tint_g"),
                frame.param("tint_b"),
            ],
            color_mode: color_mode(frame.option("color")),
            brightness: frame.param("brightness"),
            contrast: frame.param("contrast").max(0.0),
            background: frame.param("background").clamp(0.0, 1.0),
            ink: frame.param("ink").max(0.0),
            levels: frame.param("levels").max(0.0),
            footprint: frame.param("footprint").clamp(1.0, 8.0),
            ramp_len: steps.len() as u32,
            srgb: frame.srgb() as u32,
            glow: frame.param("glow").max(0.0),
            ground: ground_mode(frame.option("ground")),
            palette_len: palette.colors().len() as u32,
            palette_match: palette.matching().index(),
            palette_bg: palette.background(),
            _pad: 0,
            smoothing: frame.param("smoothing").clamp(0.0, 0.99),
            hysteresis: frame.param("hysteresis").max(0.0),
            dither: frame.param("dither").max(0.0),
            history: past as u32,
            ramp: ramp_origins,
            palette: palette.uniform_colors(),
        };
        frame
            .queue
            .write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&[uniforms]));
        let uniform = frame.uniform("ASCII Uniform BG", &self.uniform_bgl, &self.uniforms);
        let variant = |i: usize| generation * 2 + i as u64;

        // Cell pass: each cell's colour and glyph, from the picture and
        // (when it follows on) last frame's cells
        kit::fullscreen_pass(
            encoder,
            "ASCII Cells",
            &pair[write].view,
            &self.cells_pipeline,
            &[
                &uniform,
                &frame.sampled_input("ASCII Input BG", &self.input_bgl, input),
                &cells_bind(
                    frame,
                    "ASCII History BG",
                    &self.cells_bgl,
                    &pair[read],
                    variant(read),
                ),
            ],
        );

        // Glyph pass: draw the cells
        let font = fonts.loaded.as_ref().expect("loaded by font()");
        kit::fullscreen_pass(
            encoder,
            "ASCII Pass",
            output,
            &self.pipeline,
            &[
                &uniform,
                &cells_bind(
                    frame,
                    "ASCII Cells BG",
                    &self.cells_bgl,
                    &pair[write],
                    variant(write),
                ),
                &font.bind,
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniforms_match_the_shader_layout() {
        // 128 bytes of scalars, 64 vec4<u32> of ramp, 32 vec4<f32> of palette
        assert_eq!(
            std::mem::size_of::<AsciiUniforms>(),
            128 + 64 * 16 + MAX_COLORS * 16
        );
    }

    #[test]
    fn the_cell_grid_covers_every_part_cell_and_the_offset() {
        // 1280 / 8 = 160 whole cells, plus the offset texel
        assert_eq!(cell_grid([1280, 720], [8.0, 8.0], [0.0, 0.0]), [161, 91]);
        // 1283 leaves 3 px: 1 left of the origin, 2 at the right, so 162
        let origin = grid_origin([1283, 725], [8.0, 8.0]);
        assert_eq!(cell_grid([1283, 725], [8.0, 8.0], origin), [162, 92]);
    }

    fn key(epoch: u64) -> HistoryKey {
        HistoryKey {
            epoch,
            size: [64, 64],
            grid: [9, 9],
            cell: [8, 8],
            origin: [0, 0],
            font: "builtin".into(),
            ramp: "classic".into(),
            ramp_len: 10,
        }
    }

    #[test]
    fn history_counts_only_when_the_last_frame_drawn_was_the_one_before() {
        let mut h = History::default();
        // A first frame has no past
        assert!(!h.advance(key(0), 10, true));
        assert!(h.advance(key(0), 11, true));
        // A frame drawn without the style in between starts it over
        assert!(!h.advance(key(0), 13, true));
        assert!(h.advance(key(0), 14, true));
        // So does a new epoch (a scene change), or any change of the key
        assert!(!h.advance(key(1), 15, true));
        let mut other = key(1);
        other.ramp = "blocks".into();
        assert!(!h.advance(other, 16, true));
        // And `temporal = "off"` never reads it
        assert!(!h.advance(key(1), 17, true));
        assert!(!h.advance(key(1), 18, false));
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
}
