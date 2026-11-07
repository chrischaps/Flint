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
//! tint = [0.35, 1.0, 0.45]  # for color = "tint"; or "#59ff73" (#315)
//! palette = "phosphor"  # for color = "palette": built-in or palettes/<name>.palette.toml
//! ground = "dim"        # dim | black | inverse
//! glow = 0.0            # halo around bright glyphs
//! edges = 1.0           # line glyphs on edges: 0 off .. 1 full (#267)
//! edge_threshold = 0.5  # cell widths of edge a cell needs to draw one
//! edge_max = 3.0        # more than this is texture (grass), not a line
//! edge_depth = 0.6      # relative view-distance step that is an edge
//! edge_normal = 2.5     # normal change (Sobel) that is a crease
//! outline = 1.0         # per-entity: ink this many points along the silhouette (#288)
//! min_coverage = 0.5    # per-entity: least ink in a cell the entity only grazes
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
//! ## Per entity (#266)
//!
//! An entity with an `ascii_style` component (or a `style_mask` naming
//! `ascii`) is drawn in glyphs with no table at all, the rest of the frame
//! passing through; `scope = "entities"` keeps a table as the preset those
//! entities start from. Each pixel's mask id picks a row of the variant
//! table (slot 0 is the table's own, drawn from the cell state above), so
//! props can differ in cell, ramp, colour, palette and grid in one pass.
//! Per-entity rows are drawn per pixel in the glyph pass, from their own
//! samples only, and don't keep history yet (#277's smoothing, hysteresis
//! and dither apply to the whole frame's cells). `grid = "object"` pins a
//! variant's cells to its root's projected origin. The frame's ramps and
//! palettes are packed into one table each.
//!
//! ## Edges (#267)
//!
//! Luminance alone loses silhouettes, so cells along the scene's edges
//! draw the glyph set's line glyphs (`─ │ ╱ ╲` in the built-in set)
//! instead of a tone glyph. Two more passes, before the glyph pass:
//! `ascii_edges.wgsl` finds edge pixels from geometry at the depth's
//! resolution (depth, normals rebuilt from it, and the style mask), and
//! `ascii_edge_cells.wgsl` reduces each glyph cell to one line glyph by the
//! summed structure tensor of every edge pixel in it. The glyph pass draws
//! it over the tone glyph, thinning a line that splits between two cells.
//! A mask outline fires wherever the id changes and either side is one of
//! this style's ids (the plain `ascii` id or any per-entity variant's), so
//! it is what separates one styled entity from another, and it is always
//! drawn. Edges follow the screen grid: per-entity variants on an object
//! grid keep their tone glyphs.
//!
//! ## Silhouettes (#288)
//!
//! A per-entity variant cuts its silhouette to the pixel but fills it
//! with cells: a leg thinner than a cell holds a sliver of one glyph,
//! mostly ground, and reads as a smudge. Two ink floors keep thin features
//! there. `outline` (points) inks the variant's own pixels that lie within
//! that distance of another mask slot, so the silhouette is drawn as a
//! line inside it, and a feature two outlines wide is solid ink.
//! `min_coverage` floors the ink of every pixel in a cell the variant only
//! grazes (under a quarter of the cell's colour samples, easing out by
//! five eighths). Both are floors on the glyph's coverage, inked as the
//! glyph is, so a palette or tint holds; the whole frame's cells (slot 0)
//! have no silhouette and are untouched. `ground = "inverse"` already
//! fills the cell with ink and ignores them.

use super::kit;
use super::{
    choices_with, ChoiceScope, StyleDescriptor, StyleFrame, StyleHint, StyleInput, StyleOption,
    StyleParam, StylePass, StyleStage, StyleTexture,
};
use crate::glyph_atlas::{GlyphSet, GlyphSetCache, GpuGlyphAtlas, BUILTIN_SET, GLYPHS_SUFFIX};
use crate::palette::{Palette, PaletteCache, BUILTIN_PALETTES, MAX_COLORS, PALETTE_SUFFIX};
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

/// The built-in glyph set's ramps, in the order the menu offers them.
pub const RAMPS: &[&str] = &["classic", "blocks", "minimal", "detailed"];

/// The fonts to offer (#322): the built-in set, then every
/// `fonts/*.glyphs.toml` beside the scene or in its parent.
fn font_choices(scope: &ChoiceScope<'_>) -> Vec<String> {
    choices_with(&[BUILTIN_SET], scope.files("fonts", GLYPHS_SUFFIX))
}

/// The palettes to offer (#322): the built-ins, then every
/// `palettes/*.palette.toml` beside the scene or in its parent.
fn palette_choices(scope: &ChoiceScope<'_>) -> Vec<String> {
    choices_with(BUILTIN_PALETTES, scope.files("palettes", PALETTE_SUFFIX))
}

/// The ramps the chosen font has (#322): a project's set names its own,
/// which needn't be the built-in four. A font that doesn't load draws with
/// the built-in set, so that set's ramps are offered then.
fn ramp_choices(scope: &ChoiceScope<'_>) -> Vec<String> {
    // The built-in set's four, in the descriptor's order
    let builtin = || RAMPS.iter().map(|r| r.to_string()).collect();
    let font = (scope.option)("font");
    if font.is_empty() || font == BUILTIN_SET {
        return builtin();
    }
    match GlyphSetCache::new(scope.search()).get(&font) {
        Ok(set) => set.ramps().map(|r| r.name().to_string()).collect(),
        Err(_) => builtin(),
    }
}

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
            name: "exposure",
            default: 0.0,
            min: -4.0,
            max: 4.0,
        },
        StyleParam {
            name: "normalize",
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
        StyleParam {
            name: "edges",
            default: 1.0,
            min: 0.0,
            max: 1.0,
        },
        StyleParam {
            name: "edge_threshold",
            default: 0.5,
            min: 0.05,
            max: 2.0,
        },
        StyleParam {
            name: "edge_max",
            default: 3.0,
            min: 0.5,
            max: 8.0,
        },
        StyleParam {
            name: "edge_depth",
            default: 0.6,
            min: 0.05,
            max: 4.0,
        },
        StyleParam {
            name: "edge_normal",
            default: 2.5,
            min: 0.2,
            max: 8.0,
        },
        StyleParam {
            name: "outline",
            default: 1.0,
            min: 0.0,
            max: 4.0,
        },
        StyleParam {
            name: "min_coverage",
            default: 0.5,
            min: 0.0,
            max: 1.0,
        },
    ],
    options: &[
        StyleOption {
            name: "font",
            default: "builtin",
            choices: &[],
            discover: Some(font_choices),
        },
        StyleOption {
            name: "ramp",
            default: "",
            choices: RAMPS,
            discover: Some(ramp_choices),
        },
        StyleOption {
            name: "color",
            default: "source",
            choices: COLOR_MODES,
            discover: None,
        },
        StyleOption {
            name: "palette",
            default: DEFAULT_PALETTE,
            choices: BUILTIN_PALETTES,
            discover: Some(palette_choices),
        },
        StyleOption {
            name: "ground",
            default: "dim",
            choices: GROUND_MODES,
            discover: None,
        },
        StyleOption {
            name: "temporal",
            default: "on",
            choices: TEMPORAL_MODES,
            discover: None,
        },
        StyleOption {
            name: "scope",
            default: "screen",
            choices: SCOPES,
            discover: None,
        },
        StyleOption {
            name: "grid",
            default: "screen",
            choices: GRIDS,
            discover: None,
        },
        StyleOption {
            name: "transition",
            default: "fade",
            choices: TRANSITIONS,
            discover: None,
        },
    ],
    enabled: None,
    // The mask gives styled entities their outlines (#267); with no entity
    // asking for a style it is one cleared target and an empty pass
    uses_mask: true,
    needs: &[],
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
        StyleHint::new(
            "exposure",
            "Exposure (stops)",
            "Brightens (or darkens) the cell's colour before its glyph is picked: +1 doubles it in linear light",
        ),
        StyleHint::new(
            "normalize",
            "Normalize",
            "Stretches this style's own tone range (the whole frame's, or each styled entity's) over the glyph ramp, so a dark prop draws its full ramp; 0 off, 1 full",
        ),
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
        StyleHint::new(
            "scope",
            "Scope",
            "screen: the whole frame in glyphs; entities: only ascii_style entities, starting from this table",
        ),
        StyleHint::new(
            "grid",
            "Grid",
            "screen: one grid centred on the frame; object: cells pinned to each styled entity's origin",
        ),
        StyleHint::new(
            "transition",
            "Transition",
            "How an entity with blend below 1 shows the glyphs: fade cross-fades every pixel; dissolve turns that share of its cells, in patches",
        ),
        StyleHint::new("tint", "Tint", "The ink for colour mode tint"),
        StyleHint::new("glow", "Glow", "A halo around bright glyphs, inside each cell"),
        StyleHint::new(
            "edges",
            "Edges",
            "Line glyphs along silhouettes, creases and styled outlines, over the tone glyph; 0 = off",
        ),
        StyleHint::new(
            "edge_threshold",
            "Edge threshold",
            "Cell widths of edge a cell needs to draw a line glyph; lower finds more",
        ),
        StyleHint::new(
            "edge_max",
            "Edge max",
            "More edge than this (in cell widths) is texture, like grass, and keeps its tone glyph",
        ),
        StyleHint::new(
            "edge_depth",
            "Edge depth",
            "The relative view-distance step that counts as a silhouette; lower finds more",
        ),
        StyleHint::new(
            "edge_normal",
            "Edge crease",
            "The normal change that counts as a crease; lower finds more",
        ),
        StyleHint::new(
            "outline",
            "Silhouette (pt)",
            "Per-entity: inks the entity's own pixels this wide along its silhouette, so legs and rails thinner than a cell keep their shape; 0 = off",
        ),
        StyleHint::new(
            "min_coverage",
            "Min coverage",
            "Per-entity: the least ink in a cell the entity only grazes (a thin feature), 0..1",
        ),
    ],
    component: Some("ascii_style"),
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
        // The glyph pass's per-entity inputs (#266): the picture, for the
        // per-pixel variants and the pixels that pass through, and the mask
        let entity_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ASCII Entity BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // The edge cells (#267): every bind group is taken, so they
                // ride with the entity inputs
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // Each slot's tone range (#289), likewise
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        // The tone statistics' inputs (#289): the picture and the mask
        let stats_in_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ASCII Stats In BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let edges_in_bgl = load_layout(device, "ASCII Edges In BGL");
        let depth_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ASCII Depth BGL"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
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
        let no_mask_view = no_mask.create_view(&Default::default());
        let no_mask_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ASCII No Mask BG"),
            layout: &mask_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&no_mask_view),
            }],
        });
        let cells_pipeline = kit::fullscreen_pipeline(
            device,
            "ASCII Cells Pipeline",
            &kit::shader(device, "ASCII Cells Shader", CELLS_SHADER),
            &[&uniform_bgl, &input_bgl, &cells_bgl, &edges_in_bgl],
            CELL_FORMAT,
        );
        // Tone statistics (#289): per lattice row, then per slot
        let stats_pipeline = kit::fullscreen_pipeline(
            device,
            "ASCII Stats Pipeline",
            &kit::shader(device, "ASCII Stats Shader", STATS_SHADER),
            &[&uniform_bgl, &stats_in_bgl],
            STATS_FORMAT,
        );
        let stats_resolve_pipeline = kit::fullscreen_pipeline(
            device,
            "ASCII Stats Resolve Pipeline",
            &kit::shader(device, "ASCII Stats Resolve Shader", STATS_RESOLVE_SHADER),
            &[&uniform_bgl, &edges_in_bgl],
            STATS_FORMAT,
        );
        Box::new(Ascii {
            edge_pipeline: kit::fullscreen_pipeline(
                device,
                "ASCII Edge Pipeline",
                &kit::shader(
                    device,
                    "ASCII Edge Shader",
                    include_str!("ascii_edges.wgsl"),
                ),
                &[&uniform_bgl, &depth_bgl, &mask_bgl],
                EDGE_FORMAT,
            ),
            edge_cell_pipeline: kit::fullscreen_pipeline(
                device,
                "ASCII Edge Cell Pipeline",
                &kit::shader(
                    device,
                    "ASCII Edge Cell Shader",
                    include_str!("ascii_edge_cells.wgsl"),
                ),
                &[&uniform_bgl, &edges_in_bgl],
                EDGE_CELL_FORMAT,
            ),
            edge_uniforms: kit::uniform_buffer::<EdgeUniforms>(device, "ASCII Edge Uniforms"),
            edge_cell_uniforms: kit::uniform_buffer::<EdgeCellUniforms>(
                device,
                "ASCII Edge Cell Uniforms",
            ),
            edges_in_bgl,
            depth_bgl,
            mask_bgl,
            no_mask_bind,
            edges: Mutex::new(None),
            edge_cells: Mutex::new(None),
            cells_pipeline,
            stats_pipeline,
            stats_resolve_pipeline,
            stats_in_bgl,
            stats_rows: Mutex::new(None),
            stats: Mutex::new(None),
            pipeline: kit::fullscreen_pipeline(
                device,
                "ASCII Pipeline",
                &kit::shader(device, "ASCII Shader", GLYPH_SHADER),
                &[&uniform_bgl, &cells_bgl, &atlas_bgl, &entity_bgl],
                d.format,
            ),
            entity_bgl,
            _no_mask: no_mask,
            no_mask_view,
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
    /// Line glyphs' strength over the tone glyph (#267).
    edges: f32,
    /// Whether the edge passes ran this frame.
    edge_on: u32,
    /// The whole frame's exposure in stops and tone stretch (#289).
    exposure: f32,
    stretch: f32,
    /// The four edge glyphs' atlas origins, in `EdgeGlyphs::all()` order,
    /// two a vec4.
    edge_glyphs: [[u32; 4]; 2],
    /// Every variant's ramp, one after another (the whole frame's first):
    /// each step's atlas cell origin, two steps a vec4.
    ramp: [[u32; 4]; MAX_RAMP / 2],
    /// Every variant's palette inks, one palette after another (the whole
    /// frame's first; display values, rgb, unused w).
    palette: [[f32; 4]; MAX_COLORS],
    /// x: rows of `variants` in use (1 = the whole frame alone); y: 1 when
    /// the whole frame passes through (only entities asked, #266).
    variant_meta: [u32; 4],
    variants: [VariantUniform; MAX_VARIANTS],
    /// The variant slot of each mask id, four ids a vec4.
    slots: [[u32; 4]; MAX_IDS / 4],
}

/// One per-entity variant's settings (#266). Row 0 is unused: the whole
/// frame draws from the uniform's own fields and the cell state.
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
    /// Exposure in stops and tone stretch, 0..1 (#289).
    exposure: f32,
    stretch: f32,
    _pad: u32,
    /// Silhouette outline width in output pixels (#288; 0 = off).
    outline: f32,
    /// The least ink of a cell this variant only grazes (#288).
    min_coverage: f32,
    _pad_silhouette: [u32; 2],
}

/// The edge pass's uniforms (#267), `ascii_edges.wgsl`.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct EdgeUniforms {
    inv_view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 3],
    _pad0: u32,
    _pad: [f32; 2],
    depth_threshold: f32,
    normal_threshold: f32,
    /// One bit for each mask id below [`MAX_IDS`] drawn in this style.
    ours: [[u32; 4]; 2],
}

/// The edge cell pass's uniforms (#267), `ascii_edge_cells.wgsl`.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct EdgeCellUniforms {
    cell: [f32; 2],
    origin: [f32; 2],
    size: [f32; 2],
    threshold: f32,
    max_len: f32,
    edge_size: [f32; 2],
    _pad: [f32; 2],
}

/// Edge pixels: the unit gradient's structure tensor and the edge's kind.
const EDGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// One texel a glyph cell: its line glyph's slot + 1, edge length, outline.
const EDGE_CELL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The bit set of this style's mask ids for the edge pass: the plain
/// `ascii` id, and every id the variant table gives a slot of its own.
fn our_ids(style_id: Option<u16>, slots: &[u32; MAX_IDS]) -> [[u32; 4]; 2] {
    let mut bits = [[0u32; 4]; 2];
    let mut set = |id: usize| {
        if id > 0 && id < MAX_IDS {
            bits[id / 128][(id / 32) % 4] |= 1 << (id % 32);
        }
    };
    if let Some(id) = style_id {
        set(id as usize);
    }
    for (id, slot) in slots.iter().enumerate() {
        if *slot != 0 {
            set(id);
        }
    }
    bits
}

/// A fragment-stage texture at binding 0 read with `textureLoad`.
fn load_layout(device: &wgpu::Device, label: &str) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
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
    })
}

/// A target of the style's own at a size the chain's don't come in: the
/// edge pixels at the depth's size, the edge cells at the grid's. Kept
/// while the size holds; `generation` keys bind groups over it.
struct Owned {
    size: [u32; 2],
    generation: u64,
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
}

impl Owned {
    /// `slot`'s target, remade at `size` when it's another size.
    fn ensure<'s>(
        slot: &'s mut Option<Owned>,
        device: &wgpu::Device,
        label: &'static str,
        format: wgpu::TextureFormat,
        size: [u32; 2],
        layout: &wgpu::BindGroupLayout,
    ) -> &'s Owned {
        let size = [size[0].max(1), size[1].max(1)];
        if slot.as_ref().is_none_or(|o| o.size != size) {
            let generation = slot.as_ref().map_or(0, |o| o.generation + 1);
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                }],
            });
            *slot = Some(Owned {
                size,
                generation,
                _texture: texture,
                view,
                bind,
            });
        }
        slot.as_ref().expect("made above")
    }
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
const STATS_SHADER: &str = concat!(
    include_str!("ascii_common.wgsl"),
    include_str!("ascii_stats.wgsl")
);
const STATS_RESOLVE_SHADER: &str = concat!(
    include_str!("ascii_common.wgsl"),
    include_str!("ascii_stats_resolve.wgsl")
);

/// The tone statistics (#289): a texel per (slot, lattice row) of count,
/// sum and sum of squares, then a texel per slot of its range.
const STATS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

/// The tone statistics lattice's rows; `STATS_ROWS` in ascii_common.wgsl.
pub const STATS_ROWS: u32 = 180;

/// The `temporal` option's spellings: `off` draws every frame as if it
/// were the first.
pub const TEMPORAL_MODES: &[&str] = &["on", "off"];

/// What a `[post_process.styles.ascii]` table covers: the whole frame, or
/// only the entities that ask (#266), the table then being their preset.
pub const SCOPES: &[&str] = &["screen", "entities"];

/// Where a variant's cells line up: one grid centred on the frame, or a
/// grid pinned to each styled root's projected origin (#266).
pub const GRIDS: &[&str] = &["screen", "object"];

/// How an entity part-way into ASCII (`blend` below 1, #278) shows it:
/// every pixel a mix of its shading and its glyph, or that share of its
/// cells turned to glyphs whole, in patches that grow as the blend rises.
pub const TRANSITIONS: &[&str] = &["fade", "dissolve"];

/// Variants one frame can hold, the whole-frame one included; any more
/// are drawn as the whole frame is (logged once).
pub const MAX_VARIANTS: usize = 64;

/// Mask ids the variant table covers; a variant with a higher id is drawn
/// as the whole frame is (logged once).
pub const MAX_IDS: usize = 256;

/// A variant's flag: leave its pixels as they are.
const PASS_THROUGH: u32 = 1;

/// A variant's flag: a blend below 1 turns whole cells (#278).
const DISSOLVE: u32 = 2;

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
    /// Per-entity warnings already given (too many variants, a font of
    /// its own, a ramp or palette that doesn't fit), so each is said once.
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
    cells_pipeline: wgpu::RenderPipeline,
    stats_pipeline: wgpu::RenderPipeline,
    stats_resolve_pipeline: wgpu::RenderPipeline,
    stats_in_bgl: wgpu::BindGroupLayout,
    /// Tone statistics (#289): per (slot, lattice row), then per slot.
    stats_rows: Mutex<Option<Owned>>,
    stats: Mutex<Option<Owned>>,
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    uniform_bgl: wgpu::BindGroupLayout,
    input_bgl: wgpu::BindGroupLayout,
    cells_bgl: wgpu::BindGroupLayout,
    atlas_bgl: wgpu::BindGroupLayout,
    entity_bgl: wgpu::BindGroupLayout,
    _no_mask: wgpu::Texture,
    no_mask_view: wgpu::TextureView,
    fonts: Mutex<Fonts>,
    history: Mutex<History>,
    edge_pipeline: wgpu::RenderPipeline,
    edge_cell_pipeline: wgpu::RenderPipeline,
    edge_uniforms: wgpu::Buffer,
    edge_cell_uniforms: wgpu::Buffer,
    edges_in_bgl: wgpu::BindGroupLayout,
    depth_bgl: wgpu::BindGroupLayout,
    mask_bgl: wgpu::BindGroupLayout,
    no_mask_bind: wgpu::BindGroup,
    edges: Mutex<Option<Owned>>,
    edge_cells: Mutex<Option<Owned>>,
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
        // The caller's, read as one colour (`variant_color`, #315)
        tint: [0.0; 3],
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
        exposure: param("exposure").clamp(-8.0, 8.0),
        stretch: param("normalize").clamp(0.0, 1.0),
        _pad: 0,
        // In points here; the caller scales it to pixels (`outline_pixels`)
        outline: param("outline"),
        min_coverage: param("min_coverage").clamp(0.0, 1.0),
        _pad_silhouette: [0; 2],
    }
}

/// The silhouette outline (#288), `points` wide, in output pixels: at
/// least one whole pixel when on, so a 1 pt line never vanishes between
/// samples on a fractional display.
fn outline_pixels(points: f32, pixels_per_point: f32) -> f32 {
    if points.is_nan() || points <= 0.0 {
        return 0.0;
    }
    let ppp = if pixels_per_point.is_finite() && pixels_per_point > 0.0 {
        pixels_per_point
    } else {
        1.0
    };
    (points * ppp).clamp(1.0, 4.0)
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
    /// The glyph pass's picture and mask: the mask while any per-entity
    /// variant is in the table, else the one-pixel stand-in.
    fn entity_bind(
        &self,
        frame: &StyleFrame<'_>,
        input: &StyleInput<'_>,
        with_mask: bool,
        edge_cells: &Owned,
        stats: &Owned,
    ) -> Arc<wgpu::BindGroup> {
        let mask = frame.mask.filter(|_| with_mask);
        let make = |mask: &wgpu::TextureView| {
            frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ASCII Entity BG"),
                layout: &self.entity_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(input.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(mask),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&edge_cells.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&stats.view),
                    },
                ],
            })
        };
        // The edge cells are remade when the grid changes size: their
        // generation is part of the key
        let key = (edge_cells.generation << 40)
            ^ (stats.generation << 33)
            ^ (input.id * 2 + mask.is_some() as u64);
        frame.bind("ASCII Entity BG", key, || {
            make(mask.unwrap_or(&self.no_mask_view))
        })
    }

    /// Row 0 (the whole frame's, drawn from the cell state) and a row for
    /// each per-entity variant (#266), with the slot of every mask id.
    #[allow(clippy::too_many_arguments)]
    fn variant_table(
        &self,
        frame: &StyleFrame<'_>,
        fonts: &mut Fonts,
        set: &GlyphSet,
        glyph: [u32; 2],
        ramps: &mut Ramps,
        palettes: &mut Palettes,
        base_ramp: (u32, u32),
    ) -> (Vec<VariantUniform>, [u32; MAX_IDS]) {
        let mut rows = vec![VariantUniform::zeroed()];
        let mut slots = [0u32; MAX_IDS];
        let base_font = frame.option("font").to_string();
        let palette_for = |fonts: &mut Fonts, color: &str, name: &str| -> Option<Arc<Palette>> {
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
        for (id, v) in frame.variants() {
            let warn_once = |fonts: &mut Fonts, key: String, text: String| {
                if fonts.warned_variants.insert(key) {
                    tracing::warn!("ascii: {text}");
                }
            };
            if rows.len() == MAX_VARIANTS || id as usize >= MAX_IDS {
                warn_once(
                    fonts,
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
                    fonts,
                    format!("font {}", option("font")),
                    format!(
                        "per-entity fonts aren't supported yet; `{}` is drawn in `{base_font}`",
                        option("font")
                    ),
                );
            }
            let ramp = ramps.place(set, &option("ramp")).unwrap_or_else(|| {
                warn_once(
                    fonts,
                    format!("ramp {}", option("ramp")),
                    format!(
                        "ramp `{}` doesn't fit beside the others ({MAX_RAMP} steps in all); \
                         using the scene's",
                        option("ramp")
                    ),
                );
                base_ramp
            });
            let palette = match palette_for(fonts, &option("color"), &option("palette")) {
                None => PalettePlace::default(),
                Some(p) => palettes.place(&p).unwrap_or_else(|| {
                    warn_once(
                        fonts,
                        format!("palette {}", p.name()),
                        format!(
                            "palette `{}` doesn't fit beside the others ({MAX_COLORS} inks \
                             in all); using the scene's",
                            p.name()
                        ),
                    );
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
            let flags = if v.off {
                PASS_THROUGH
            } else if option("transition") == "dissolve" {
                DISSOLVE
            } else {
                0
            };
            slots[id as usize] = rows.len() as u32;
            let mut row = variant_uniform(&param, &option, cell, origin, ramp, &palette, flags);
            row.tint = frame.variant_color(v, "tint");
            // The row holds the outline in points; the shader wants pixels
            row.outline = outline_pixels(row.outline, frame.pixels_per_point);
            rows.push(row);
        }
        (rows, slots)
    }

    /// Edge pixels from depth, normals and the mask, then each cell's line
    /// glyph from them (#267).
    fn encode_edges(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        edge_cells: &Owned,
        cell: [f32; 2],
        origin: [f32; 2],
        ours: [[u32; 4]; 2],
    ) {
        let edge_uniforms = EdgeUniforms {
            inv_view_proj: frame.camera.inv_view_proj,
            camera_pos: frame.camera.position,
            _pad0: 0,
            _pad: [0.0; 2],
            depth_threshold: frame.param("edge_depth").max(1e-3),
            normal_threshold: frame.param("edge_normal").max(1e-3),
            ours,
        };
        frame
            .queue
            .write_buffer(&self.edge_uniforms, 0, bytemuck::bytes_of(&edge_uniforms));
        // At the depth's size, so a line is one depth pixel wide
        let mut edges_slot = self.edges.lock().unwrap_or_else(|e| e.into_inner());
        let edges = Owned::ensure(
            &mut edges_slot,
            frame.device,
            "ASCII Edges",
            EDGE_FORMAT,
            frame.depth_size,
            &self.edges_in_bgl,
        );
        let depth = frame.depth_bind("ASCII Depth BG", 0, || {
            frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ASCII Depth BG"),
                layout: &self.depth_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(frame.depth),
                }],
            })
        });
        let mask = frame.mask_bind("ASCII Mask BG", &self.mask_bgl);
        kit::fullscreen_pass(
            encoder,
            "ASCII Edge Pass",
            &edges.view,
            &self.edge_pipeline,
            &[
                &frame.uniform(
                    "ASCII Edge Uniform BG",
                    &self.uniform_bgl,
                    &self.edge_uniforms,
                ),
                &depth,
                mask.as_deref().unwrap_or(&self.no_mask_bind),
            ],
        );

        let cell_uniforms = EdgeCellUniforms {
            cell,
            origin,
            size: [frame.width as f32, frame.height as f32],
            threshold: frame.param("edge_threshold").max(0.01),
            max_len: frame.param("edge_max"),
            edge_size: [edges.size[0] as f32, edges.size[1] as f32],
            _pad: [0.0; 2],
        };
        frame.queue.write_buffer(
            &self.edge_cell_uniforms,
            0,
            bytemuck::bytes_of(&cell_uniforms),
        );
        kit::fullscreen_pass(
            encoder,
            "ASCII Edge Cell Pass",
            &edge_cells.view,
            &self.edge_cell_pipeline,
            &[
                &frame.uniform(
                    "ASCII Edge Cell Uniform BG",
                    &self.uniform_bgl,
                    &self.edge_cell_uniforms,
                ),
                &edges.bind,
            ],
        );
    }

    /// Each slot's tone range into `stats` (#289): per lattice row, then
    /// summed per slot, both from this frame's picture.
    fn encode_stats(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        uniform: &wgpu::BindGroup,
        with_mask: bool,
        stats: &Owned,
    ) {
        let mut rows_slot = self.stats_rows.lock().unwrap_or_else(|e| e.into_inner());
        let rows = Owned::ensure(
            &mut rows_slot,
            frame.device,
            "ASCII Tone Stat Rows",
            STATS_FORMAT,
            [MAX_VARIANTS as u32, STATS_ROWS],
            &self.edges_in_bgl,
        );
        let mask = frame.mask.filter(|_| with_mask);
        let key = input.id * 2 + mask.is_some() as u64;
        let input_bind = frame.bind("ASCII Stats In BG", key, || {
            frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ASCII Stats In BG"),
                layout: &self.stats_in_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(input.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            mask.unwrap_or(&self.no_mask_view),
                        ),
                    },
                ],
            })
        });
        kit::fullscreen_pass(
            encoder,
            "ASCII Tone Stat Rows",
            &rows.view,
            &self.stats_pipeline,
            &[uniform, &input_bind],
        );
        kit::fullscreen_pass(
            encoder,
            "ASCII Tone Stats",
            &stats.view,
            &self.stats_resolve_pipeline,
            &[uniform, &rows.bind],
        );
    }

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
        // Ramps and palettes are packed, the whole frame's first so its
        // steps and inks sit where the cell state expects them
        let mut ramps = Ramps::default();
        let base_ramp = ramps
            .place(&set, ramp.name())
            .expect("the first ramp is resampled to fit");
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
        let mut palettes = Palettes::default();
        let _ = palettes.place(&palette);
        let (variants, slots) = self.variant_table(
            frame,
            &mut fonts,
            &set,
            glyph,
            &mut ramps,
            &mut palettes,
            base_ramp,
        );
        let cell = cell_pixels(frame.param("cell"), frame.pixels_per_point, glyph);
        let size = [frame.width, frame.height];
        let origin = grid_origin(size, cell);
        let grid = cell_grid(size, cell, origin);

        // Edges (#267): line glyphs from geometry, one texel a cell on the
        // same grid as the cell state
        let mut edge_glyphs = [[0u32; 4]; 2];
        for (i, ch) in set.edges().all().iter().enumerate() {
            // A set's edge glyphs are always in its atlas (GlyphSet checks)
            let [x, y] = atlas
                .index_of(*ch)
                .map_or([0, 0], |index| atlas.cell_origin(index));
            edge_glyphs[i / 2][(i % 2) * 2] = x;
            edge_glyphs[i / 2][(i % 2) * 2 + 1] = y;
        }
        let edges = frame.param("edges").clamp(0.0, 1.0);
        let mut edge_cells_slot = self.edge_cells.lock().unwrap_or_else(|e| e.into_inner());
        let edge_cells = Owned::ensure(
            &mut edge_cells_slot,
            frame.device,
            "ASCII Edge Cells",
            EDGE_CELL_FORMAT,
            grid,
            &self.edges_in_bgl,
        );
        if edges > 0.0 {
            let ours = our_ids(frame.style_id(STYLE.name), &slots);
            self.encode_edges(frame, encoder, edge_cells, cell, origin, ours);
        }

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
            tint: frame.color("tint"),
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
            edges,
            edge_on: u32::from(edges > 0.0),
            exposure: frame.param("exposure").clamp(-8.0, 8.0),
            stretch: frame.param("normalize").clamp(0.0, 1.0),
            edge_glyphs,
            ramp: [[0; 4]; MAX_RAMP / 2],
            palette: [[0.0; 4]; MAX_COLORS],
            variant_meta: [
                variants.len() as u32,
                !(frame.whole_frame() && frame.option("scope") != "entities") as u32,
                0,
                0,
            ],
            variants: [VariantUniform::zeroed(); MAX_VARIANTS],
            slots: [[0; 4]; MAX_IDS / 4],
        };
        let mut uniforms = uniforms;
        for (i, index) in ramps.steps.iter().enumerate() {
            let [x, y] = atlas.cell_origin(*index);
            let pair = &mut uniforms.ramp[i / 2];
            pair[(i % 2) * 2] = x;
            pair[(i % 2) * 2 + 1] = y;
        }
        uniforms.palette[..palettes.inks.len()].copy_from_slice(&palettes.inks);
        uniforms.variants[..variants.len()].copy_from_slice(&variants);
        for (i, slot) in slots.iter().enumerate() {
            uniforms.slots[i / 4][i % 4] = *slot;
        }
        frame
            .queue
            .write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
        let uniform = frame.uniform("ASCII Uniform BG", &self.uniform_bgl, &self.uniforms);
        let variant = |i: usize| generation * 2 + i as u64;

        // Tone statistics (#289): each slot's luminance range, from this
        // frame's picture, for the slots that normalise
        let mut stats_slot = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        let stats = Owned::ensure(
            &mut stats_slot,
            frame.device,
            "ASCII Tone Stats",
            STATS_FORMAT,
            [MAX_VARIANTS as u32, 1],
            &self.edges_in_bgl,
        );
        if uniforms.stretch > 0.0 || variants.iter().any(|v| v.stretch > 0.0) {
            self.encode_stats(frame, encoder, input, &uniform, variants.len() > 1, stats);
        }

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
                &stats.bind,
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
                &self.entity_bind(frame, input, variants.len() > 1, edge_cells, stats),
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniforms_match_the_shader_layout() {
        // 144 bytes of scalars (#267's edge strength included), 2
        // vec4<u32> of edge glyphs, 64 vec4<u32> of ramp, 32 vec4<f32> of
        // palette, then the variant table's 16-byte header, its rows of 128
        // (#288's silhouette floors are the last 16) and its 64 vec4<u32>
        // of slots (#266)
        assert_eq!(std::mem::size_of::<VariantUniform>(), 128);
        assert_eq!(
            std::mem::size_of::<AsciiUniforms>(),
            144 + 2 * 16 + 64 * 16 + MAX_COLORS * 16 + 16 + MAX_VARIANTS * 128 + 64 * 16
        );
        assert_eq!(std::mem::offset_of!(AsciiUniforms, edges), 128);
        assert_eq!(std::mem::offset_of!(AsciiUniforms, edge_glyphs), 144);
        assert_eq!(std::mem::size_of::<EdgeUniforms>(), 128);
        assert_eq!(std::mem::size_of::<EdgeCellUniforms>(), 48);
    }

    #[test]
    fn the_edge_shaders_validate_and_their_uniforms_match_rust() {
        for (name, src, size) in [
            (
                "ascii_edges.wgsl",
                include_str!("ascii_edges.wgsl"),
                std::mem::size_of::<EdgeUniforms>(),
            ),
            (
                "ascii_edge_cells.wgsl",
                include_str!("ascii_edge_cells.wgsl"),
                std::mem::size_of::<EdgeCellUniforms>(),
            ),
        ] {
            let module = naga::front::wgsl::parse_str(src).expect(name);
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .expect(name);
            let (_, u) = module
                .global_variables
                .iter()
                .find(|(_, v)| v.name.as_deref() == Some("u"))
                .expect("uniform u");
            assert_eq!(
                module.types[u.ty].inner.size(module.to_ctx()) as usize,
                size,
                "{name}"
            );
        }
    }

    #[test]
    fn our_ids_hold_the_plain_id_and_every_variant() {
        let mut slots = [0u32; MAX_IDS];
        slots[3] = 1;
        slots[200] = 2;
        let bits = our_ids(Some(5), &slots);
        let has = |id: usize| bits[id / 128][(id / 32) % 4] & (1 << (id % 32)) != 0;
        assert!(has(3) && has(5) && has(200));
        assert!(!has(0) && !has(4) && !has(255));
        assert_eq!(our_ids(None, &[0; MAX_IDS]), [[0; 4]; 2]);
    }

    #[test]
    fn edges_are_on_by_default_and_every_edge_param_is_in_range() {
        for name in [
            "edges",
            "edge_threshold",
            "edge_max",
            "edge_depth",
            "edge_normal",
        ] {
            let p = STYLE.params.iter().find(|p| p.name == name).unwrap();
            assert!(p.min <= p.default && p.default <= p.max, "{name}");
        }
        let edges = STYLE.params.iter().find(|p| p.name == "edges").unwrap();
        assert_eq!(edges.default, 1.0);
        assert!(STYLE.uses_mask, "outlines read the style mask");
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
    fn the_silhouette_outline_is_in_points_and_never_under_a_pixel() {
        assert_eq!(outline_pixels(1.0, 1.0), 1.0);
        assert_eq!(outline_pixels(1.0, 2.0), 2.0);
        // On when asked for, even at half a point on a 1x display
        assert_eq!(outline_pixels(0.5, 1.0), 1.0);
        // Off, and broken input, are off; the ring search stops at 4 px
        assert_eq!(outline_pixels(0.0, 2.0), 0.0);
        assert_eq!(outline_pixels(f32::NAN, 1.0), 0.0);
        assert_eq!(outline_pixels(3.0, 2.0), 4.0);
        assert_eq!(outline_pixels(1.0, f32::NAN), 1.0);
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

    /// A game folder in memory, as a bundle gives it on the web: a level
    /// beside its own palette, and a palette and two fonts one folder up.
    fn game_files() -> flint_core::vfs::MemorySource {
        let mut src = flint_core::vfs::MemorySource::new();
        src.insert("game/levels/a.scene.toml", "[scene]\nname = \"a\"\n");
        src.insert(
            "game/levels/palettes/tidepool.palette.toml",
            "colors = [\"#0a3d62\", \"#3c6382\"]\n",
        );
        src.insert(
            "game/palettes/dusk.palette.toml",
            "colors = [\"#2d1b3d\", \"#f2a65a\"]\n",
        );
        // Not a palette: a stray file in the folder is not offered
        src.insert("game/palettes/readme.txt", "notes");
        src.insert(
            "game/fonts/plain.glyphs.toml",
            "[ramps.dots]\nglyphs = \" .:\"\n\n[ramps.bars]\nglyphs = \" -=#\"\n",
        );
        src.insert(
            "game/fonts/terminal.glyphs.toml",
            "[ramps.x]\nglyphs = \" x\"\n",
        );
        src
    }

    fn option(name: &str) -> &'static StyleOption {
        STYLE.options.iter().find(|o| o.name == name).unwrap()
    }

    #[test]
    fn palettes_and_fonts_beside_the_scene_are_offered() {
        flint_core::vfs::scoped(Arc::new(game_files()), || {
            let dir = Path::new("game/levels");
            let scope = ChoiceScope::defaults(Some(dir));
            let palettes = option("palette").choices_in(&scope);
            let mut want: Vec<String> = BUILTIN_PALETTES.iter().map(|s| s.to_string()).collect();
            want.extend(["dusk".to_string(), "tidepool".to_string()]);
            assert_eq!(palettes, want);
            assert_eq!(
                option("font").choices_in(&scope),
                ["builtin", "plain", "terminal"]
            );
            // With no scene, the built-ins alone
            let none = ChoiceScope::defaults(None);
            assert_eq!(option("palette").choices_in(&none), BUILTIN_PALETTES);
            assert_eq!(option("font").choices_in(&none), ["builtin"]);
        });
    }

    #[test]
    fn the_ramp_list_follows_the_chosen_font() {
        flint_core::vfs::scoped(Arc::new(game_files()), || {
            let dir = Path::new("game/levels");
            let font = |name: &'static str| {
                move |o: &str| match o {
                    "font" => name.to_string(),
                    _ => String::new(),
                }
            };
            let plain = font("plain");
            let scope = ChoiceScope {
                scene_dir: Some(dir),
                option: &plain,
            };
            let mut ramps = option("ramp").choices_in(&scope);
            ramps.sort();
            assert_eq!(ramps, ["bars", "dots"]);
            // The built-in font (or one that won't load) has the built-in four
            let missing = font("nonesuch");
            for f in [&font("builtin") as &dyn Fn(&str) -> String, &missing] {
                let scope = ChoiceScope {
                    scene_dir: Some(dir),
                    option: f,
                };
                let ramps = option("ramp").choices_in(&scope);
                for r in option("ramp").choices {
                    assert!(ramps.iter().any(|x| x == r), "{r} in {ramps:?}");
                }
            }
        });
    }

    #[test]
    fn found_options_stay_open_to_the_command_line() {
        // `--style ascii:palette=tidepool` names a file the style resolves
        let mut config = crate::PostProcessConfig::default();
        assert!(super::super::apply_style_spec(&mut config, "ascii:palette=tidepool").is_ok());
        assert!(super::super::apply_style_spec(&mut config, "ascii:color=plaid").is_err());
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
        let module = naga::front::wgsl::parse_str(GLYPH_SHADER).expect("parse");
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
    fn the_cell_and_tone_stat_shaders_validate() {
        for (name, src) in [
            ("ascii_cells.wgsl", CELLS_SHADER),
            ("ascii_stats.wgsl", STATS_SHADER),
            ("ascii_stats_resolve.wgsl", STATS_RESOLVE_SHADER),
        ] {
            let module = naga::front::wgsl::parse_str(src).expect(name);
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .expect(name);
        }
        // The lattice's rows are one constant in two languages
        assert!(include_str!("ascii_common.wgsl")
            .contains(&format!("const STATS_ROWS: u32 = {STATS_ROWS}u;")));
    }

    #[test]
    fn exposure_and_normalize_are_off_by_default_and_in_range() {
        for (name, lo, hi) in [("exposure", -4.0, 4.0), ("normalize", 0.0, 1.0)] {
            let p = STYLE.params.iter().find(|p| p.name == name).expect(name);
            assert_eq!(p.default, 0.0, "{name} must leave existing scenes alone");
            assert_eq!((p.min, p.max), (lo, hi), "{name}");
        }
        // They sit where the shader's Variant and AsciiUniforms put them
        assert_eq!(std::mem::offset_of!(VariantUniform, exposure), 100);
        assert_eq!(std::mem::offset_of!(AsciiUniforms, exposure), 136);
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
}
