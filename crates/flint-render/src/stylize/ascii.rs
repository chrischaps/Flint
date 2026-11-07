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
//! and dither apply to the whole frame's cells). With a `footprint` of 2
//! or more those samples are summed once a cell (#313):
//! `ascii_variant_cells.wgsl` gives each grid a band of rows (variants
//! with the same cell, origin and footprint share one) and stores the
//! first two of a band's variants met in each cell; a pixel of a third,
//! of a variant past [`MAX_BANDS`] grids, or of a footprint-1 variant
//! (whose loads hit the cache) sums for itself. `grid = "object"` pins a
//! variant's cells to its root's projected origin. The frame's ramps and
//! palettes are packed into one table each.
//!
//! A variant may name a font of its own (#335). The frame's glyph sets
//! (the whole frame's first, up to [`MAX_FONTS`]) are stacked in one atlas
//! texture, [`GpuGlyphStack`], uploaded again only when the sets change;
//! the ramp table's origins point into the stack, and each variant row
//! carries its font's glyph cell and where its four edge glyphs sit in the
//! ramp table, so its cell takes that font's aspect and its outlines that
//! font's line glyphs.
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
use super::text::{self, GlyphCell, GlyphText};
use super::{
    choices_with, ChoiceScope, StyleDescriptor, StyleFrame, StyleHint, StyleInput, StyleOption,
    StyleParam, StylePass, StyleStage, StyleTexture,
};
use crate::glyph_atlas::{GlyphSet, GlyphSetCache, GpuGlyphStack, BUILTIN_SET, GLYPHS_SUFFIX};
use crate::palette::{Palette, PaletteCache, BUILTIN_PALETTES, MAX_COLORS, PALETTE_SUFFIX};
use bytemuck::{Pod, Zeroable};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
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
pub(super) fn palette_choices(scope: &ChoiceScope<'_>) -> Vec<String> {
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
            name: "chroma",
            default: 1.0,
            min: 0.0,
            max: 4.0,
        },
        StyleParam {
            name: "palette_dither",
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
            "chroma",
            "Palette chroma",
            "For a nearest-ink palette: scales the cell's saturation before matching, so a muted scene reaches the palette's colours; 1 = as it is",
        ),
        StyleHint::new(
            "palette_dither",
            "Palette dither",
            "For a nearest-ink palette: mixes the two nearest inks by a 4x4 Bayer pattern on the cells, so gradients step through inks instead of banding",
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
    feeds_bloom: false,
    create: |d| {
        let device = d.device;
        let uniform_bgl = kit::uniform_layout(device, "ASCII Uniform BGL");
        // Group 0 of every pass over ascii_common.wgsl (#336): the uniform,
        // the variant rows and each mask id's slot
        let table_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ASCII Table BGL"),
            entries: &[uniform_entry(), storage_entry(1), storage_entry(2)],
        });
        // The edge pass's: its uniform and the slots, which say whose ids
        // draw outlines
        let edge_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ASCII Edge BGL"),
            entries: &[uniform_entry(), storage_entry(1)],
        });
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
                // The variant cells' two entries (#313), likewise
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
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
            &[&table_bgl, &input_bgl, &cells_bgl, &edges_in_bgl],
            CELL_FORMAT,
        );
        // Tone statistics (#289): per lattice row, then per slot
        let stats_pipeline = kit::fullscreen_pipeline(
            device,
            "ASCII Stats Pipeline",
            &kit::shader(device, "ASCII Stats Shader", STATS_SHADER),
            &[&table_bgl, &stats_in_bgl],
            STATS_FORMAT,
        );
        // Per-entity variant cells (#313): the picture and the mask in, as
        // the tone statistics read them
        let variant_cells_pipeline = kit::fullscreen_pipeline_targets(
            device,
            "ASCII Variant Cells Pipeline",
            &kit::shader(device, "ASCII Variant Cells Shader", VARIANT_CELLS_SHADER),
            &[&table_bgl, &stats_in_bgl],
            &[VARIANT_CELL_FORMAT; 2],
        );
        let stats_resolve_pipeline = kit::fullscreen_pipeline(
            device,
            "ASCII Stats Resolve Pipeline",
            &kit::shader(device, "ASCII Stats Resolve Shader", STATS_RESOLVE_SHADER),
            &[&table_bgl, &edges_in_bgl],
            STATS_FORMAT,
        );
        let glyph_module = kit::shader(device, "ASCII Shader", GLYPH_SHADER);
        Box::new(Ascii {
            edge_pipeline: kit::fullscreen_pipeline(
                device,
                "ASCII Edge Pipeline",
                &kit::shader(
                    device,
                    "ASCII Edge Shader",
                    include_str!("ascii_edges.wgsl"),
                ),
                &[&edge_bgl, &depth_bgl, &mask_bgl],
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
            variant_cells_pipeline,
            variant_cells: Mutex::new([None, None]),
            stats_pipeline,
            stats_resolve_pipeline,
            stats_in_bgl,
            stats_rows: Mutex::new(None),
            stats: Mutex::new(None),
            pipeline: kit::fullscreen_pipeline(
                device,
                "ASCII Pipeline",
                &glyph_module,
                &[&table_bgl, &cells_bgl, &atlas_bgl, &entity_bgl],
                d.format,
            ),
            text_pipeline: kit::fullscreen_pipeline_entry(
                device,
                "ASCII Text Pipeline",
                &glyph_module,
                "fs_text",
                &[&table_bgl, &cells_bgl, &atlas_bgl, &entity_bgl],
                TEXT_FORMAT,
            ),
            capture: AtomicBool::new(false),
            text: Mutex::new(None),
            entity_bgl,
            _no_mask: no_mask,
            no_mask_view,
            uniforms: kit::uniform_buffer::<AsciiUniforms>(device, "ASCII Uniforms"),
            uniform_bgl,
            table_bgl,
            edge_bgl,
            table: Mutex::new(None),
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
    /// The whole frame's palette chroma and ink dither (#320).
    chroma: f32,
    palette_dither: f32,
    _pad_palette: [u32; 2],
    /// The four edge glyphs' atlas origins, in `EdgeGlyphs::all()` order,
    /// two a vec4.
    edge_glyphs: [[u32; 4]; 2],
    /// Every variant's ramp, one after another (the whole frame's first):
    /// each step's atlas cell origin, two steps a vec4.
    ramp: [[u32; 4]; MAX_RAMP / 2],
    /// Every variant's palette inks, one palette after another (the whole
    /// frame's first; display values, rgb, unused w).
    palette: [[f32; 4]; MAX_COLORS],
    /// The same inks in OKLab (#320; L, a, b, unused w), for matching.
    palette_lab: [[f32; 4]; MAX_COLORS],
    /// x: rows of `variants` in use (1 = the whole frame alone); y: 1 when
    /// the whole frame passes through (only entities asked, #266).
    variant_meta: [u32; 4],
    /// The variant cell state's bands (#313), `variant_meta[2]` of them:
    /// each grid's cell and origin, and its (first row, rows, columns,
    /// footprint's f32 bits).
    bands: [[f32; 4]; MAX_BANDS],
    band_rows: [[u32; 4]; MAX_BANDS],
}

/// One per-entity variant's settings (#266), a row of the variant table's
/// storage buffer (#336). Row 0 is unused: the whole frame draws from the
/// uniform's own fields and the cell state.
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
    /// The first row + 1 of the variant's band in the variant cell state
    /// (#313); 0 when it has none and the glyph pass samples for itself.
    cell_row: u32,
    /// Silhouette outline width in output pixels (#288; 0 = off).
    outline: f32,
    /// The least ink of a cell this variant only grazes (#288).
    min_coverage: f32,
    /// Ordered dither between the two nearest inks, 0..1 (#320).
    palette_dither: f32,
    /// Palette chroma before matching (#320).
    chroma: f32,
    /// The variant's font's glyph cell in atlas pixels (#335).
    glyph: [f32; 2],
    /// Where the variant's four edge glyphs sit in the ramp table, or
    /// [`NO_EDGE_START`] for the whole frame's (a variant in its font).
    edge_start: u32,
    _pad_font: u32,
}

/// A variant whose edge glyphs are the uniform's own; `NO_EDGE_START` in
/// ascii_common.wgsl.
const NO_EDGE_START: u32 = u32::MAX;

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
    /// The plain `ascii` id (0 = none); the variants' ids are the ones the
    /// variant table gives a slot (#336).
    plain_id: u32,
    _pad1: [u32; 3],
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

/// How many entries a variant table buffer holds to fit `needed`: what it
/// holds already when that's enough, else the next power of two (#336).
fn table_capacity(capacity: usize, needed: usize) -> usize {
    if needed <= capacity {
        capacity
    } else {
        needed.next_power_of_two().max(MIN_TABLE)
    }
}

/// The variant table (#336): every variant's row and every mask id's slot,
/// in storage buffers that grow with the frame and never shrink.
/// `generation` keys the bind groups over them.
struct VariantTable {
    rows: wgpu::Buffer,
    slots: wgpu::Buffer,
    row_capacity: usize,
    slot_capacity: usize,
    generation: u64,
}

impl VariantTable {
    fn storage(device: &wgpu::Device, label: &str, bytes: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: bytes as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// `slot`'s table, remade bigger when `rows` or `slots` don't fit.
    fn ensure<'s>(
        slot: &'s mut Option<VariantTable>,
        device: &wgpu::Device,
        rows: usize,
        slots: usize,
    ) -> &'s VariantTable {
        let (have_rows, have_slots, generation) = slot.as_ref().map_or((0, 0, 0), |t| {
            (t.row_capacity, t.slot_capacity, t.generation + 1)
        });
        let row_capacity = table_capacity(have_rows, rows);
        let slot_capacity = table_capacity(have_slots, slots);
        if slot.is_none() || row_capacity != have_rows || slot_capacity != have_slots {
            *slot = Some(VariantTable {
                rows: Self::storage(
                    device,
                    "ASCII Variant Rows",
                    row_capacity * std::mem::size_of::<VariantUniform>(),
                ),
                slots: Self::storage(device, "ASCII Variant Slots", slot_capacity * 4),
                row_capacity,
                slot_capacity,
                generation,
            });
        }
        slot.as_ref().expect("made above")
    }
}

/// A read-only fragment storage buffer at `binding`.
fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// A fragment uniform buffer at binding 0.
fn uniform_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
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
pub(crate) const GLYPH_SHADER: &str = concat!(
    include_str!("ascii_common.wgsl"),
    include_str!("../palette.wgsl"),
    include_str!("ascii.wgsl")
);
const VARIANT_CELLS_SHADER: &str = concat!(
    include_str!("ascii_common.wgsl"),
    include_str!("ascii_variant_cells.wgsl")
);
const STATS_SHADER: &str = concat!(
    include_str!("ascii_common.wgsl"),
    include_str!("ascii_stats.wgsl")
);
const STATS_RESOLVE_SHADER: &str = concat!(
    include_str!("ascii_common.wgsl"),
    include_str!("ascii_stats_resolve.wgsl")
);

/// The variant cell state (#313): two textures, a texel a cell in each,
/// each texel a variant's colour sum (f32 bits in RGB) and its slot,
/// sample count and overflow bit in A.
const VARIANT_CELL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Uint;

/// Grids the variant cell state holds (#313), each a band of rows; a
/// variant on another grid samples for itself. `MAX_BANDS` in
/// ascii_common.wgsl.
pub const MAX_BANDS: usize = 8;

/// The least footprint a variant's cells are summed in the pre-pass at
/// (#313). At 1 the glyph pass's 16 loads a pixel all land on the 16
/// texels its cell shares, so they come from the texture cache, and the
/// pre-pass (a whole screen of cells a band) cost more than it saved: at
/// 4K on an RTX 4070, ASCII went 0.285 -> 0.326 ms on
/// `demo/ascii_entities`. At 2 the lattice spreads over four cells' worth
/// of picture and the pre-pass wins (0.485 -> 0.405 ms close up).
const MIN_BAND_FOOTPRINT: f32 = 2.0;

/// The variant cell state's widest and tallest, inside the least texture
/// size every backend offers.
const MAX_BAND_TEXELS: u32 = 8192;

/// One grid's rows in the variant cell state (#313).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Band {
    cell: [f32; 2],
    origin: [f32; 2],
    footprint: f32,
    /// The band's first row, its rows and its cells across.
    row: u32,
    rows: u32,
    cols: u32,
}

/// Gives each drawn per-entity variant (rows past 0) with a footprint of
/// at least [`MIN_BAND_FOOTPRINT`] the band of its grid: variants with the
/// same cell, origin and footprint share one, so their cells are summed in
/// one pass. Narrower variants, variants past [`MAX_BANDS`] grids and
/// ones past the texture's size get none and sample in the glyph pass.
fn assign_bands(rows: &mut [VariantUniform], size: [u32; 2]) -> Vec<Band> {
    let mut bands: Vec<Band> = Vec::new();
    let mut next = 0u32;
    for row in rows.iter_mut().skip(1) {
        row.cell_row = 0;
        if row.flags & PASS_THROUGH != 0 || row.footprint < MIN_BAND_FOOTPRINT {
            continue;
        }
        let same = |b: &&Band| {
            b.cell.map(f32::to_bits) == row.cell.map(f32::to_bits)
                && b.origin.map(f32::to_bits) == row.origin.map(f32::to_bits)
                && b.footprint.to_bits() == row.footprint.to_bits()
        };
        if let Some(b) = bands.iter().find(same) {
            row.cell_row = b.row + 1;
            continue;
        }
        let grid = cell_grid(size, row.cell, row.origin);
        if bands.len() == MAX_BANDS || grid[0] > MAX_BAND_TEXELS || next + grid[1] > MAX_BAND_TEXELS
        {
            continue;
        }
        bands.push(Band {
            cell: row.cell,
            origin: row.origin,
            footprint: row.footprint,
            row: next,
            rows: grid[1],
            cols: grid[0],
        });
        row.cell_row = next + 1;
        next += grid[1];
    }
    bands
}

/// The variant cell state's size for `bands`: a texel a cell across the
/// widest band, every band's rows stacked.
fn band_texture_size(bands: &[Band]) -> [u32; 2] {
    let wide = bands.iter().map(|b| b.cols).max().unwrap_or(0);
    let tall = bands.last().map_or(0, |b| b.row + b.rows);
    [wide.max(1), tall.max(1)]
}

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
/// are drawn as the whole frame is (logged once). The table is a storage
/// buffer sized to the frame (#336), so this is only the tone statistics'
/// texture width (a column a variant), well inside every backend's limit.
pub const MAX_VARIANTS: usize = 4096;

/// Rows (and mask id slots) the variant table's buffers start with; they
/// grow to the next power of two when a frame needs more (#336).
const MIN_TABLE: usize = 64;

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

/// The sets in use and their atlases stacked on the GPU (#335).
struct LoadedStack {
    sets: Vec<Arc<GlyphSet>>,
    gpu: GpuGlyphStack,
    bind: wgpu::BindGroup,
}

impl LoadedStack {
    /// Glyph `index` of font `font`'s origin in the stack.
    fn origin(&self, font: u32, index: u32) -> [u32; 2] {
        let font = font as usize;
        let [x, y] = self.sets[font].atlas().cell_origin(index);
        [x, y + self.gpu.tops[font]]
    }
}

#[derive(Default)]
struct Fonts {
    search: Vec<PathBuf>,
    cache: GlyphSetCache,
    palettes: PaletteCache,
    stack: Option<LoadedStack>,
    /// The last (font, ramp) a warning was given for, so it's said once.
    warned: Option<(String, String)>,
    /// Per-entity warnings already given (too many variants or fonts, a
    /// ramp or palette that doesn't fit), so each is said once.
    warned_variants: std::collections::HashSet<String>,
    /// Frames drawn since the caches last looked at the disk (#323).
    since_poll: u32,
}

/// The ASCII style asks its caches whether a palette or glyph set file
/// changed every this many frames: a quarter second at 60 fps.
const POLL_FRAMES: u32 = 15;

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
    /// Per-entity variant cells (#313): their pass and its two entries.
    variant_cells_pipeline: wgpu::RenderPipeline,
    variant_cells: Mutex<[Option<Owned>; 2]>,
    stats_pipeline: wgpu::RenderPipeline,
    stats_resolve_pipeline: wgpu::RenderPipeline,
    stats_in_bgl: wgpu::BindGroupLayout,
    /// Tone statistics (#289): per (slot, lattice row), then per slot.
    stats_rows: Mutex<Option<Owned>>,
    stats: Mutex<Option<Owned>>,
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    uniform_bgl: wgpu::BindGroupLayout,
    /// The uniform and the variant table (#336), and the edge pass's.
    table_bgl: wgpu::BindGroupLayout,
    edge_bgl: wgpu::BindGroupLayout,
    table: Mutex<Option<VariantTable>>,
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
    /// The glyph grid as text (#317): `fs_text` in ascii.wgsl, drawn only
    /// while `capture` is on, into `text` and copied to its read-back.
    text_pipeline: wgpu::RenderPipeline,
    capture: AtomicBool,
    text: Mutex<Option<TextCapture>>,
}

/// The text pass's texel per cell: glyph code, ink, ground, styled.
const TEXT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Uint;
const TEXT_TEXEL_BYTES: u32 = 16;

/// The text pass's target and read-back for one grid size, and what the
/// last frame copied into it needs to become text.
struct TextCapture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    buffer: wgpu::Buffer,
    grid: [u32; 2],
    /// Bytes a row of the read-back, padded to wgpu's copy alignment.
    row_bytes: u32,
    /// The grid's first visible column and row: the offset texel is off
    /// screen when the grid starts at the frame's edge.
    first: [u32; 2],
    /// Each glyph's atlas origin (packed `x | y << 16`) to its character.
    glyphs: Vec<(u32, char)>,
    /// Mask ids a display style after ASCII draws over: their cells show
    /// that style in the finished frame, so they're written as passed.
    overdrawn: Vec<u32>,
    /// Whether a frame has been copied in since the last read.
    copied: bool,
}

impl TextCapture {
    fn new(device: &wgpu::Device, grid: [u32; 2]) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ASCII Text"),
            size: wgpu::Extent3d {
                width: grid[0].max(1),
                height: grid[1].max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let row_bytes = (grid[0].max(1) * TEXT_TEXEL_BYTES).div_ceil(align) * align;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ASCII Text Read-back"),
            size: (row_bytes * grid[1].max(1)) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            texture,
            view,
            buffer,
            grid,
            row_bytes,
            first: [0, 0],
            glyphs: Vec::new(),
            overdrawn: Vec::new(),
            copied: false,
        }
    }

    /// The copied frame as text; waits on the device for the copy.
    fn read(&mut self, device: &wgpu::Device) -> Option<GlyphText> {
        if !self.copied {
            return None;
        }
        let slice = self.buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::Maintain::Wait);
        rx.recv().ok()?.ok()?;
        let data = slice.get_mapped_range();
        let words = |x: u32, y: u32| -> [u32; 4] {
            let at = (y * self.row_bytes + x * TEXT_TEXEL_BYTES) as usize;
            let w: &[u8] = &data[at..at + TEXT_TEXEL_BYTES as usize];
            std::array::from_fn(|i| u32::from_le_bytes(w[i * 4..i * 4 + 4].try_into().unwrap()))
        };
        let [cols, rows] = [
            self.grid[0].saturating_sub(self.first[0]),
            self.grid[1].saturating_sub(self.first[1]),
        ];
        let mut cells = Vec::with_capacity((cols * rows) as usize);
        for y in 0..rows {
            for x in 0..cols {
                let [code, ink, ground, id] = words(x + self.first[0], y + self.first[1]);
                let ours = code != text::TEXT_NONE && !self.overdrawn.contains(&id);
                let glyph = ours.then(|| {
                    self.glyphs
                        .iter()
                        .find(|(o, _)| *o == code)
                        .map_or('?', |(_, c)| *c)
                });
                // Over a cell another style draws, the ground alone
                let ground = text::unpack_rgb(ground);
                let ink = if code != text::TEXT_NONE && !ours {
                    ground
                } else {
                    text::unpack_rgb(ink)
                };
                cells.push(GlyphCell { glyph, ink, ground });
            }
        }
        drop(data);
        self.buffer.unmap();
        self.copied = false;
        Some(GlyphText { cols, rows, cells })
    }
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
        // Given by `assign_bands`
        cell_row: 0,
        // In points here; the caller scales it to pixels (`outline_pixels`)
        outline: param("outline"),
        min_coverage: param("min_coverage").clamp(0.0, 1.0),
        palette_dither: param("palette_dither").clamp(0.0, 1.0),
        chroma: param("chroma").clamp(0.0, 8.0),
        // The caller's, from the variant's font (#335)
        glyph: [0.0; 2],
        edge_start: NO_EDGE_START,
        _pad_font: 0,
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
    /// The inks in OKLab, beside `inks` (#320).
    lab: Vec<[f32; 4]>,
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
        self.lab.resize(at.start as usize, [0.0; 4]);
        self.lab
            .extend(palette.lab_colors().iter().map(|c| [c[0], c[1], c[2], 0.0]));
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

/// The ramps in use, packed one after another into the uniform: each step
/// a (font, atlas index) pair, the font an index into the frame's
/// [`FontsInUse`] (#335), made an origin in the glyph stack at upload.
#[derive(Default)]
struct Ramps {
    steps: Vec<(u32, u32)>,
    placed: Vec<((u32, String), (u32, u32))>,
}

/// The key a font's four edge glyphs are packed under in [`Ramps`]; no
/// ramp is named with a NUL.
const EDGES_KEY: &str = "\0edges";

impl Ramps {
    /// Where ramp `name` of `set` (font `font`) sits, packing it on first
    /// use; `None` when it doesn't fit in what's left.
    fn place(&mut self, font: u32, set: &GlyphSet, name: &str) -> Option<(u32, u32)> {
        let ramp = set.ramp(name).unwrap_or_else(|| set.default_ramp());
        let fit = fit_ramp(ramp.atlas_indices());
        self.pack(font, name, fit)
    }

    /// Ramp `name` of font `font` and the font's edge glyphs (#335), both
    /// or neither: (the ramp's start and length, the edges' start).
    fn place_with_edges(
        &mut self,
        font: u32,
        set: &GlyphSet,
        name: &str,
    ) -> Option<((u32, u32), u32)> {
        let ramp = set.ramp(name).unwrap_or_else(|| set.default_ramp());
        let fit = fit_ramp(ramp.atlas_indices());
        let atlas = set.atlas();
        let edges: Vec<u32> = set
            .edges()
            .all()
            .iter()
            .map(|c| atlas.index_of(*c).unwrap_or(0))
            .collect();
        let need = |key: &str, len: usize| {
            if self.find(font, key).is_some() {
                0
            } else {
                len
            }
        };
        if self.steps.len() + need(name, fit.len()) + need(EDGES_KEY, edges.len()) > MAX_RAMP {
            return None;
        }
        let ramp = self.pack(font, name, fit)?;
        let edges = self.pack(font, EDGES_KEY, edges)?;
        Some((ramp, edges.0))
    }

    fn find(&self, font: u32, name: &str) -> Option<(u32, u32)> {
        self.placed
            .iter()
            .find(|((f, n), _)| *f == font && n == name)
            .map(|(_, at)| *at)
    }

    fn pack(&mut self, font: u32, name: &str, indices: Vec<u32>) -> Option<(u32, u32)> {
        if let Some(at) = self.find(font, name) {
            return Some(at);
        }
        if self.steps.len() + indices.len() > MAX_RAMP {
            return None;
        }
        let at = (self.steps.len() as u32, indices.len() as u32);
        self.steps.extend(indices.into_iter().map(|i| (font, i)));
        self.placed.push(((font, name.to_string()), at));
        Some(at)
    }
}

/// Fonts one frame can draw in (#335), the whole frame's first; a variant
/// naming another past these is drawn in the whole frame's (logged once).
pub const MAX_FONTS: usize = 8;

/// The tallest glyph stack, inside the least texture size every backend
/// offers; a font that would pass it is drawn in the whole frame's.
const MAX_STACK_TEXELS: u32 = 8192;

/// The glyph sets a frame draws in (#335): the whole frame's at 0, then
/// each per-entity variant's own in the order the variants name them.
struct FontsInUse {
    sets: Vec<Arc<GlyphSet>>,
}

impl FontsInUse {
    fn new(base: Arc<GlyphSet>) -> Self {
        Self { sets: vec![base] }
    }

    /// `set`'s index, adding it on first use; `None` when the frame holds
    /// [`MAX_FONTS`] already or its atlas would make the stack too tall.
    fn index(&mut self, set: &Arc<GlyphSet>) -> Option<u32> {
        if let Some(i) = self.sets.iter().position(|s| Arc::ptr_eq(s, set)) {
            return Some(i as u32);
        }
        let tall: u32 = self.sets.iter().map(|s| s.atlas().size()[1]).sum();
        let [w, h] = set.atlas().size();
        if self.sets.len() == MAX_FONTS || tall + h > MAX_STACK_TEXELS || w > MAX_STACK_TEXELS {
            return None;
        }
        self.sets.push(set.clone());
        Some(self.sets.len() as u32 - 1)
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
        variant_cells: [&Owned; 2],
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
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&variant_cells[0].view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(&variant_cells[1].view),
                    },
                ],
            })
        };
        // The edge cells are remade when the grid changes size: their
        // generation is part of the key
        let key = (edge_cells.generation << 40)
            ^ (stats.generation << 33)
            ^ (variant_cells[0].generation << 52)
            ^ (input.id * 2 + mask.is_some() as u64);
        frame.bind("ASCII Entity BG", key, || {
            make(mask.unwrap_or(&self.no_mask_view))
        })
    }

    /// The variant table's rows and slots written to its buffers (grown to
    /// fit, #336), and the bind groups over them: the uniform's group 0 of
    /// every ascii_common.wgsl pass, the edge pass's, and the width the
    /// tone statistics give a column each row.
    fn upload_table(
        &self,
        frame: &StyleFrame<'_>,
        rows: &[VariantUniform],
        slots: &mut Vec<u32>,
    ) -> (Arc<wgpu::BindGroup>, Arc<wgpu::BindGroup>, u32) {
        let mut table_slot = self.table.lock().unwrap_or_else(|e| e.into_inner());
        let table = VariantTable::ensure(&mut table_slot, frame.device, rows.len(), slots.len());
        // Every slot is written, so an id a past frame held reads 0 again
        slots.resize(table.slot_capacity, 0);
        frame
            .queue
            .write_buffer(&table.rows, 0, bytemuck::cast_slice(rows));
        frame
            .queue
            .write_buffer(&table.slots, 0, bytemuck::cast_slice(slots));
        let main = frame.bind("ASCII Table BG", table.generation, || {
            frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ASCII Table BG"),
                layout: &self.table_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniforms.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: table.rows.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: table.slots.as_entire_binding(),
                    },
                ],
            })
        });
        let edge = frame.bind("ASCII Edge Table BG", table.generation, || {
            frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ASCII Edge Table BG"),
                layout: &self.edge_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.edge_uniforms.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: table.slots.as_entire_binding(),
                    },
                ],
            })
        });
        let width = table.row_capacity.min(MAX_VARIANTS) as u32;
        (main, edge, width)
    }

    /// Row 0 (the whole frame's, drawn from the cell state) and a row for
    /// each per-entity variant (#266), with the slot of every mask id. A
    /// variant naming a font of its own (#335) adds it to `in_use`.
    fn variant_table(
        &self,
        frame: &StyleFrame<'_>,
        fonts: &mut Fonts,
        in_use: &mut FontsInUse,
        ramps: &mut Ramps,
        palettes: &mut Palettes,
        base_ramp: (u32, u32),
    ) -> (Vec<VariantUniform>, Vec<u32>) {
        let mut rows = vec![VariantUniform::zeroed()];
        // A slot for every id up to the highest this style's variants hold
        let mut slots = vec![
            0u32;
            frame
                .variants()
                .map(|(id, _)| id as usize + 1)
                .max()
                .unwrap_or(1)
        ];
        let base = in_use.sets[0].clone();
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
            if rows.len() == MAX_VARIANTS {
                warn_once(
                    fonts,
                    "overflow".into(),
                    format!(
                        "more per-entity variants than the {} one frame holds; the rest \
                         are drawn like the whole frame",
                        MAX_VARIANTS - 1
                    ),
                );
                continue;
            }
            let param = |n: &str| frame.variant_param(v, n);
            let option = |n: &str| frame.variant_option(v, n).to_string();
            // The variant's own font (#335): its ramp and its edge glyphs
            // packed beside the others, its glyph cell its own. One that
            // doesn't fit is drawn in the whole frame's font
            let font_name = option("font");
            let set = self.set_named(fonts, &font_name);
            let own = if Arc::ptr_eq(&set, &base) {
                None
            } else {
                let placed = in_use.index(&set).and_then(|font| {
                    ramps
                        .place_with_edges(font, &set, &option("ramp"))
                        .map(|(ramp, edges)| (set.atlas().cell_size(), ramp, edges))
                });
                if placed.is_none() {
                    warn_once(
                        fonts,
                        format!("font {font_name}"),
                        format!(
                            "font `{font_name}` doesn't fit beside the others ({MAX_FONTS} \
                             fonts, {MAX_STACK_TEXELS} atlas rows, {MAX_RAMP} ramp steps in \
                             all); drawn in `{}`",
                            base.name()
                        ),
                    );
                }
                placed
            };
            let (glyph, ramp, edge_start) = match own {
                Some(placed) => placed,
                None => {
                    let ramp = ramps.place(0, &base, &option("ramp")).unwrap_or_else(|| {
                        warn_once(
                            fonts,
                            format!("ramp {}", option("ramp")),
                            format!(
                                "ramp `{}` doesn't fit beside the others ({MAX_RAMP} steps in \
                                 all); using the scene's",
                                option("ramp")
                            ),
                        );
                        base_ramp
                    });
                    (base.atlas().cell_size(), ramp, NO_EDGE_START)
                }
            };
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
            row.glyph = [glyph[0] as f32, glyph[1] as f32];
            row.edge_start = edge_start;
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
        table: &wgpu::BindGroup,
    ) {
        let edge_uniforms = EdgeUniforms {
            inv_view_proj: frame.camera.inv_view_proj,
            camera_pos: frame.camera.position,
            _pad0: 0,
            _pad: [0.0; 2],
            depth_threshold: frame.param("edge_depth").max(1e-3),
            normal_threshold: frame.param("edge_normal").max(1e-3),
            plain_id: frame.style_id(STYLE.name).map_or(0, u32::from),
            _pad1: [0; 3],
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
            &[table, &depth, mask.as_deref().unwrap_or(&self.no_mask_bind)],
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
            // A column a slot, as wide as the resolved stats
            [stats.size[0], STATS_ROWS],
            &self.edges_in_bgl,
        );
        let input_bind = self.picture_bind(frame, input, with_mask);
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

    /// The picture and (while any per-entity variant is drawn) the mask,
    /// as the tone statistics and the variant cells read them.
    fn picture_bind(
        &self,
        frame: &StyleFrame<'_>,
        input: &StyleInput<'_>,
        with_mask: bool,
    ) -> Arc<wgpu::BindGroup> {
        let mask = frame.mask.filter(|_| with_mask);
        let key = input.id * 2 + mask.is_some() as u64;
        frame.bind("ASCII Stats In BG", key, || {
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
        })
    }

    /// Once a frame: the caches follow the scene's folder, and every
    /// [`POLL_FRAMES`] a changed palette or glyph set file clears its cache
    /// (#323). A reloaded set is a new Arc, so the stack re-uploads, and
    /// palettes are packed afresh every frame.
    fn refresh_fonts(&self, frame: &StyleFrame<'_>, fonts: &mut Fonts) {
        let search = Fonts::search_for(frame.scene_dir);
        if fonts.search != search {
            fonts.cache = GlyphSetCache::new(search.clone());
            fonts.palettes = PaletteCache::new(search.clone());
            fonts.search = search;
            fonts.stack = None;
        }
        fonts.since_poll += 1;
        if fonts.since_poll >= POLL_FRAMES {
            fonts.since_poll = 0;
            let glyphs = fonts.cache.poll();
            let palettes = fonts.palettes.poll();
            if glyphs || palettes {
                tracing::info!(
                    "ascii: reloading {}",
                    match (glyphs, palettes) {
                        (true, true) => "glyph sets and palettes",
                        (true, false) => "glyph sets",
                        _ => "palettes",
                    }
                );
                fonts.warned = None;
                fonts.warned_variants.clear();
            }
        }
    }

    /// The glyph set `name`, the built-in set when it can't be loaded (the
    /// cache has warned).
    fn set_named(&self, fonts: &mut Fonts, name: &str) -> Arc<GlyphSet> {
        fonts
            .cache
            .get(name)
            .unwrap_or_else(|_| GlyphSet::builtin())
    }

    /// The atlases of `in_use` stacked on the GPU, uploaded again only
    /// when the frame's sets change (#335).
    fn stack<'f>(
        &self,
        frame: &StyleFrame<'_>,
        fonts: &'f mut Fonts,
        in_use: &FontsInUse,
    ) -> &'f LoadedStack {
        let stale = fonts.stack.as_ref().is_none_or(|l| {
            l.sets.len() != in_use.sets.len()
                || l.sets
                    .iter()
                    .zip(&in_use.sets)
                    .any(|(a, b)| !Arc::ptr_eq(a, b))
        });
        if stale {
            let atlases: Vec<_> = in_use.sets.iter().map(|s| &**s.atlas()).collect();
            let gpu = GpuGlyphStack::upload(frame.device, frame.queue, &atlases, "ASCII Atlas");
            let bind = frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ASCII Atlas BG"),
                layout: &self.atlas_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&gpu.view),
                }],
            });
            fonts.stack = Some(LoadedStack {
                sets: in_use.sets.clone(),
                gpu,
                bind,
            });
        }
        fonts.stack.as_ref().expect("loaded above")
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
        self.refresh_fonts(frame, &mut fonts);
        let set = self.set_named(&mut fonts, frame.option("font"));
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
            .place(0, &set, ramp.name())
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
        let mut in_use = FontsInUse::new(set.clone());
        let (mut variants, mut slots) = self.variant_table(
            frame,
            &mut fonts,
            &mut in_use,
            &mut ramps,
            &mut palettes,
            base_ramp,
        );
        let cell = cell_pixels(frame.param("cell"), frame.pixels_per_point, glyph);
        let size = [frame.width, frame.height];
        let origin = grid_origin(size, cell);
        let grid = cell_grid(size, cell, origin);
        // Each per-entity variant's band of cells (#313)
        let bands = assign_bands(&mut variants, size);

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
        // The variant table (#336), written before any pass reads it
        let (uniform, edge_table, stats_width) = self.upload_table(frame, &variants, &mut slots);
        if edges > 0.0 {
            self.encode_edges(frame, encoder, edge_cells, cell, origin, &edge_table);
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
            chroma: frame.param("chroma").clamp(0.0, 8.0),
            palette_dither: frame.param("palette_dither").clamp(0.0, 1.0),
            _pad_palette: [0; 2],
            edge_glyphs,
            ramp: [[0; 4]; MAX_RAMP / 2],
            palette: [[0.0; 4]; MAX_COLORS],
            palette_lab: [[0.0; 4]; MAX_COLORS],
            variant_meta: [
                variants.len() as u32,
                !(frame.whole_frame() && frame.option("scope") != "entities") as u32,
                bands.len() as u32,
                0,
            ],
            bands: [[0.0; 4]; MAX_BANDS],
            band_rows: [[0; 4]; MAX_BANDS],
        };
        let mut uniforms = uniforms;
        // Every font the frame draws in, stacked; the whole frame's is at
        // the top, so its origins are its atlas's own
        let stack = self.stack(frame, &mut fonts, &in_use);
        for (i, (font, index)) in ramps.steps.iter().enumerate() {
            let [x, y] = stack.origin(*font, *index);
            let pair = &mut uniforms.ramp[i / 2];
            pair[(i % 2) * 2] = x;
            pair[(i % 2) * 2 + 1] = y;
        }
        uniforms.palette[..palettes.inks.len()].copy_from_slice(&palettes.inks);
        uniforms.palette_lab[..palettes.lab.len()].copy_from_slice(&palettes.lab);
        for (i, b) in bands.iter().enumerate() {
            uniforms.bands[i] = [b.cell[0], b.cell[1], b.origin[0], b.origin[1]];
            uniforms.band_rows[i] = [b.row, b.rows, b.cols, b.footprint.to_bits()];
        }
        frame
            .queue
            .write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
        let variant = |i: usize| generation * 2 + i as u64;

        // Tone statistics (#289): each slot's luminance range, from this
        // frame's picture, for the slots that normalise
        let mut stats_slot = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        let stats = Owned::ensure(
            &mut stats_slot,
            frame.device,
            "ASCII Tone Stats",
            STATS_FORMAT,
            [stats_width, 1],
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

        // Variant cells (#313): each per-entity variant cell's colour sum,
        // once a cell rather than in each of its pixels
        let mut variant_cells_slot = self.variant_cells.lock().unwrap_or_else(|e| e.into_inner());
        let [first_slot, second_slot] = &mut *variant_cells_slot;
        let band_size = band_texture_size(&bands);
        let variant_cells = [
            Owned::ensure(
                first_slot,
                frame.device,
                "ASCII Variant Cells A",
                VARIANT_CELL_FORMAT,
                band_size,
                &self.cells_bgl,
            ),
            Owned::ensure(
                second_slot,
                frame.device,
                "ASCII Variant Cells B",
                VARIANT_CELL_FORMAT,
                band_size,
                &self.cells_bgl,
            ),
        ];
        if !bands.is_empty() {
            kit::fullscreen_pass_targets(
                encoder,
                "ASCII Variant Cells",
                &[&variant_cells[0].view, &variant_cells[1].view],
                &self.variant_cells_pipeline,
                &[&uniform, &self.picture_bind(frame, input, true)],
            );
        }

        // Glyph pass: draw the cells
        let font = fonts.stack.as_ref().expect("stacked above");
        let cells = cells_bind(
            frame,
            "ASCII Cells BG",
            &self.cells_bgl,
            &pair[write],
            variant(write),
        );
        // A cell whose entity a later display style draws over isn't ours
        // in the finished frame (#317): the text pass reads the mask for it
        let capturing = self.capture.load(Ordering::Relaxed);
        let overdrawn = if capturing {
            overdrawn_ids(frame)
        } else {
            Vec::new()
        };
        let with_mask = variants.len() > 1 || !overdrawn.is_empty();
        let entity = self.entity_bind(frame, input, with_mask, edge_cells, stats, variant_cells);
        let groups = [&*uniform, &*cells, &font.bind, &*entity];
        kit::fullscreen_pass(encoder, "ASCII Pass", output, &self.pipeline, &groups);

        // The same cells as text (#317), when asked
        if capturing {
            let mut slot = self.text.lock().unwrap_or_else(|e| e.into_inner());
            if slot.as_ref().is_none_or(|t| t.grid != grid) {
                *slot = Some(TextCapture::new(frame.device, grid));
            }
            let capture = slot.as_mut().expect("made above");
            kit::fullscreen_pass(
                encoder,
                "ASCII Text",
                &capture.view,
                &self.text_pipeline,
                &groups,
            );
            encoder.copy_texture_to_buffer(
                capture.texture.as_image_copy(),
                wgpu::ImageCopyBuffer {
                    buffer: &capture.buffer,
                    layout: wgpu::ImageDataLayout {
                        offset: 0,
                        bytes_per_row: Some(capture.row_bytes),
                        rows_per_image: None,
                    },
                },
                wgpu::Extent3d {
                    width: grid[0],
                    height: grid[1],
                    depth_or_array_layers: 1,
                },
            );
            capture.first = text_first(origin);
            // Every font's glyphs, at their places in the stack (#335)
            capture.glyphs = font
                .sets
                .iter()
                .enumerate()
                .flat_map(|(f, set)| {
                    set.atlas()
                        .glyphs()
                        .iter()
                        .enumerate()
                        .map(move |(i, ch)| ((f as u32, i as u32), *ch))
                })
                .map(|((f, i), ch)| {
                    let [x, y] = font.origin(f, i);
                    (x | (y << 16), ch)
                })
                .collect();
            capture.overdrawn = overdrawn;
            capture.copied = true;
        }
    }

    fn set_text_capture(&self, on: bool) {
        self.capture.store(on, Ordering::Relaxed);
        if !on {
            *self.text.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }

    fn read_text(&self, device: &wgpu::Device) -> Option<GlyphText> {
        self.text
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()?
            .read(device)
    }
}

/// The style mask ids whose pixels a display style that runs after ASCII
/// draws over (#317): an entity of another style that's on this frame.
fn overdrawn_ids(frame: &StyleFrame<'_>) -> Vec<u32> {
    let chain = super::stage_chain(StyleStage::Display, frame.config);
    let Some(ours) = chain.iter().position(|(d, _)| d.name == STYLE.name) else {
        return Vec::new();
    };
    let later: Vec<&str> = chain[ours + 1..]
        .iter()
        .filter(|(d, _)| d.is_on(frame.config, frame.style_ids))
        .map(|(d, _)| d.name)
        .collect();
    frame
        .style_ids
        .iter()
        .filter(|(id, _)| {
            frame
                .style_ids
                .variant(*id)
                .is_some_and(|v| !v.off && later.contains(&v.style.as_str()))
        })
        .map(|(id, _)| u32::from(id))
        .collect()
}

/// The text grid's first visible texel (#317): the cell state's texel 0
/// holds the part cell left of (above) the origin, which is off screen
/// when the grid starts at the frame's edge.
fn text_first(origin: [f32; 2]) -> [u32; 2] {
    origin.map(|o| u32::from(o <= 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniforms_match_the_shader_layout() {
        // 160 bytes of scalars (#267's edge strength, #320's palette look), 2
        // vec4<u32> of edge glyphs, 64 vec4<u32> of ramp, 2 x 32 vec4<f32> of
        // palette (inks, then their OKLab), then the variant table's 16-byte
        // header (its rows, and its slots, are storage buffers since #336:
        // a row is 144 bytes, #288's silhouette floors and #320's palette
        // look, then #335's glyph cell and edge start), and the variant cell
        // bands' two vec4 each (#313)
        assert_eq!(std::mem::size_of::<VariantUniform>(), 144);
        assert_eq!(std::mem::offset_of!(VariantUniform, glyph), 128);
        assert_eq!(
            std::mem::size_of::<AsciiUniforms>(),
            160 + 2 * 16 + 64 * 16 + 2 * MAX_COLORS * 16 + 16 + MAX_BANDS * 32
        );
        assert_eq!(std::mem::offset_of!(AsciiUniforms, edges), 128);
        assert_eq!(std::mem::offset_of!(AsciiUniforms, edge_glyphs), 160);
        assert_eq!(std::mem::size_of::<EdgeUniforms>(), 112);
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
    fn the_variant_table_grows_to_the_next_power_of_two_and_keeps_it() {
        // Starts at MIN_TABLE, doubles past it, never shrinks (#336)
        assert_eq!(table_capacity(0, 1), MIN_TABLE);
        assert_eq!(table_capacity(MIN_TABLE, MIN_TABLE), MIN_TABLE);
        assert_eq!(table_capacity(MIN_TABLE, 501), 512);
        assert_eq!(table_capacity(512, 3), 512);
        assert_eq!(table_capacity(512, 513), 1024);
        // The tone statistics' column a row stays inside every backend
        assert!(MAX_VARIANTS <= 8192 && MAX_VARIANTS.is_power_of_two());
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
            ("ascii_variant_cells.wgsl", VARIANT_CELLS_SHADER),
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
        // And so are the variant cell state's bands (#313)
        assert!(include_str!("ascii_common.wgsl")
            .contains(&format!("const MAX_BANDS: u32 = {MAX_BANDS}u;")));
    }

    fn variant_row(cell: f32, origin: [f32; 2], footprint: f32, flags: u32) -> VariantUniform {
        VariantUniform {
            cell: [cell, cell],
            origin,
            footprint,
            flags,
            ..VariantUniform::zeroed()
        }
    }

    #[test]
    fn variants_on_one_grid_share_a_band_and_bands_stack() {
        let size = [1280, 720];
        let mut rows = vec![
            VariantUniform::zeroed(),
            variant_row(8.0, [0.0, 0.0], 2.0, 0),
            // Same grid: the same band
            variant_row(8.0, [0.0, 0.0], 2.0, 0),
            // Another footprint lays another lattice: a band of its own
            variant_row(8.0, [0.0, 0.0], 3.0, 0),
            // Passes through: no cells
            variant_row(8.0, [0.0, 0.0], 2.0, PASS_THROUGH),
            // An object grid
            variant_row(12.0, [3.0, 5.0], 2.0, 0),
            // A footprint of 1 samples in the glyph pass, from the cache
            variant_row(8.0, [0.0, 0.0], 1.0, 0),
        ];
        let bands = assign_bands(&mut rows, size);
        assert_eq!(bands.len(), 3);
        let grid = cell_grid(size, [8.0; 2], [0.0; 2]);
        assert_eq!(
            (bands[0].row, bands[0].rows, bands[0].cols),
            (0, grid[1], grid[0])
        );
        assert_eq!(bands[1].row, grid[1]);
        assert_eq!(bands[2].row, 2 * grid[1]);
        let cells: Vec<u32> = rows.iter().map(|r| r.cell_row).collect();
        assert_eq!(cells, [0, 1, 1, grid[1] + 1, 0, 2 * grid[1] + 1, 0]);
        let wide = cell_grid(size, [12.0; 2], [3.0, 5.0]);
        assert_eq!(
            band_texture_size(&bands),
            [grid[0].max(wide[0]), 2 * grid[1] + wide[1]]
        );
        // With no bands the texture is a stand-in texel
        assert_eq!(band_texture_size(&[]), [1, 1]);
    }

    #[test]
    fn grids_past_the_band_limit_sample_for_themselves() {
        let mut rows = vec![VariantUniform::zeroed()];
        for i in 0..MAX_BANDS + 2 {
            rows.push(variant_row(8.0 + i as f32, [0.0, 0.0], 2.0, 0));
        }
        let bands = assign_bands(&mut rows, [640, 360]);
        assert_eq!(bands.len(), MAX_BANDS);
        assert!(rows[1..=MAX_BANDS].iter().all(|r| r.cell_row > 0));
        assert!(rows[MAX_BANDS + 1..].iter().all(|r| r.cell_row == 0));
        // 2 px cells at 4K are 1081 rows a band: the eighth won't fit
        let mut tall = vec![VariantUniform::zeroed()];
        for footprint in 2..=MAX_BANDS + 1 {
            tall.push(variant_row(2.0, [0.0, 0.0], footprint as f32, 0));
        }
        let bands = assign_bands(&mut tall, [3840, 2160]);
        let [w, h] = band_texture_size(&bands);
        assert!(w <= MAX_BAND_TEXELS && h <= MAX_BAND_TEXELS);
        assert_eq!(bands.len(), MAX_BANDS - 1);
        assert_eq!(tall[MAX_BANDS].cell_row, 0);
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
        let classic = ramps.place(0, &set, "classic").unwrap();
        assert_eq!(classic.0, 0);
        let blocks = ramps.place(0, &set, "blocks").unwrap();
        assert_eq!(blocks.0, classic.1, "packed after the first");
        assert_eq!(
            ramps.place(0, &set, "classic"),
            Some(classic),
            "placed once"
        );
        // Another font's ramp of the same name is its own (#335)
        let other = ramps.place(1, &set, "classic").unwrap();
        assert_eq!(other.0, blocks.0 + blocks.1);
        assert!(ramps.steps[other.0 as usize..].iter().all(|(f, _)| *f == 1));
        // Fill what's left, then one more doesn't fit
        ramps.steps.resize(MAX_RAMP, (0, 0));
        assert_eq!(ramps.place(0, &set, "minimal"), None);
    }

    #[test]
    fn a_fonts_ramp_and_edges_pack_together_or_not_at_all() {
        let set = GlyphSet::builtin();
        let mut ramps = Ramps::default();
        let (ramp, edges) = ramps.place_with_edges(1, &set, "classic").unwrap();
        assert_eq!(ramp.0, 0);
        assert_eq!(edges, ramp.1, "the edges follow the ramp");
        let atlas = set.atlas();
        let packed: Vec<u32> = ramps.steps[edges as usize..edges as usize + 4]
            .iter()
            .map(|(_, i)| *i)
            .collect();
        let want: Vec<u32> = set
            .edges()
            .all()
            .iter()
            .map(|c| atlas.index_of(*c).unwrap())
            .collect();
        assert_eq!(packed, want, "in EdgeGlyphs::all() order");
        // Again: placed once, edges shared by the font's ramps
        let len = ramps.steps.len();
        assert_eq!(
            ramps.place_with_edges(1, &set, "classic"),
            Some((ramp, edges))
        );
        let (_, again) = ramps.place_with_edges(1, &set, "blocks").unwrap();
        assert_eq!(again, edges);
        assert!(ramps.steps.len() > len);
        // Room for the ramp but not its edges: neither is packed
        let blocks = set.ramp("minimal").unwrap().len();
        ramps.steps.resize(MAX_RAMP - blocks - 3, (0, 0));
        let before = ramps.steps.len();
        assert_eq!(ramps.place_with_edges(2, &set, "minimal"), None);
        assert_eq!(ramps.steps.len(), before);
    }

    #[test]
    fn fonts_in_use_are_counted_once_and_capped() {
        let base = GlyphSet::builtin();
        let mut in_use = FontsInUse::new(base.clone());
        assert_eq!(in_use.index(&base), Some(0));
        let other =
            |n: &str| Arc::new(GlyphSet::from_toml(n, "", Path::new(".")).expect("built-in cells"));
        let a = other("a");
        assert_eq!(in_use.index(&a), Some(1));
        assert_eq!(in_use.index(&a), Some(1), "the same set once");
        for i in 2..MAX_FONTS {
            assert_eq!(in_use.index(&other(&format!("f{i}"))), Some(i as u32));
        }
        assert_eq!(in_use.index(&other("one too many")), None);
        assert_eq!(in_use.sets.len(), MAX_FONTS);
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
