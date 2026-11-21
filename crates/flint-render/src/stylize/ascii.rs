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
//! edge_corner = 0.9     # #348: coherence under which a cell can draw ┌ ├ ┼ (0: no corners, #637's line ends and gaps too)
//! edge_ink = "cell"     # cell | color | hue: the line glyphs' ink (#351)
//! edge_color = [0.85, 0.82, 0.74]  # for edge_ink = "color"; or "#d9d1bd"
//! edge_tint = 0.0       # for edge_ink = "color": toward the near side's hue
//! outline = 1.0         # per-entity: ink this many points along the silhouette (#288)
//! min_coverage = 0.5    # per-entity: least ink in a cell the entity only grazes
//! silhouette = "pixel"  # pixel | cell (whole glyphs at the edge, #338; whole frame: at other styles' entities too, #534)
//! cell_share = 0.5      # cell silhouette: share of a cell that is the entity's (whole frame: not another style's)
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
//! colour (`smoothing`, per 60 Hz frame: [`StyleFrame::dt`] scales it so a
//! change settles in the same game time at any rate, #342) and last frame's glyph while its tone stays within
//! `hysteresis` ramp steps of it, and an optional grid-fixed ordered
//! `dither` breaks gradients into a stable pattern. The edge glyphs keep
//! a history of their own (#477): the edge passes read a copy of last
//! frame's edge cells and slots (`EdgePast`), so a cell's edge length,
//! direction and place are smoothed (by the square root of `smoothing`)
//! and its decisions (draw a line or not, which line, which of two cells
//! a line near their border belongs to) are held by `hysteresis`, and a
//! cell drawing the same line changes its corner only when two frames
//! agree. `temporal = "off"` draws every frame as if it were the first. History counts only when
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
//! instead of a tone glyph. The edge pixels come from geometry at the
//! depth's resolution: depth steps and creases from the shared edge pass
//! (`edges.wgsl`, every line style's since #743, scaled by this style's
//! `edge_depth` / `edge_normal`), and the style mask's outlines
//! (`ascii_edge_source.wgsl`). `ascii_edge_cells.wgsl` reduces each glyph
//! cell to one line glyph by the summed structure tensor of every edge
//! pixel in it. The glyph pass draws
//! it over the tone glyph in the one cell the line passes within half a
//! cell of (#352: the cell pass records where the line crosses the cell's
//! centre column or row; `ascii_edge_thin.wgsl` keeps it there). Per
//! entity that cell can be one the style doesn't draw, just outside a
//! silhouette that runs along a cell border, so the thin pass gives the
//! line to the drawn cell beside it (#493).
//! Which glyph each edge cell draws, its thinned line or the corner its
//! neighbours' lines meet in (#348, #637), is decided once per cell in two
//! more passes, `ascii_edge_thin.wgsl` and `ascii_edge_corners.wgsl`
//! (#910): the glyph pass reads one texel of it. Decided per pixel, it
//! inlined under FXC into a glyph shader that took up to a minute to
//! compile on DX12.
//! A mask outline fires wherever the id changes and either side is one of
//! this style's ids (the plain `ascii` id or any per-entity variant's), so
//! it is what separates one styled entity from another, and it is always
//! drawn. Edges follow the screen grid: per-entity variants on an object
//! grid keep their tone glyphs.
//!
//! Line glyphs are inked as the cell's tone glyph by default. Along a
//! silhouette that colour is a mix of the object and what's behind it, so
//! `edge_ink` (#351) gives them an ink of their own: `color` (one colour,
//! mixed toward the near side's hue by `edge_tint`) or `hue` (the near
//! side's own colour, inked as the colour mode inks a cell). The edge cell
//! pass averages the picture under the edge pixels that decided a cell's
//! line, which lie on the near side, and stores the hue, 7 bits a channel,
//! above the outline and cross-axis bits of its texel's z (the texture is
//! `Rgba32Float`, so z is an exact integer).
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
//!
//! `silhouette = "cell"` (#338) is the other reading: the silhouette made
//! of whole glyphs, a cell all glyph or all scene. A cell of the variant's
//! grid is the variant's when at least `cell_share` of its 16 lattice
//! samples are; then every pixel of it draws as the variant, the scene's
//! pixels in it included, and the variant's own pixels in a cell it
//! doesn't hold draw as the whole frame (or pass through). The snap owner
//! pass (#929, `ascii_snap_owner.wgsl`) decides it once a cell of each
//! snapping grid, in a band of rows over the grid's variants' bounds
//! ([`OwnerBand`]), and the glyph pass reads a texel a grid: the pixel's
//! own grid, then each of the snapping variants' grids (up to
//! [`MAX_SNAP_GRIDS`]; a variant on a grid past them still drops its own
//! minority cells, but fills no others; one whose grid gets no band, past
//! [`MAX_BAND_TEXELS`] rows, keeps its pixel silhouette). The pixel `outline` is
//! off on a snapped variant: its edge is the cells'. A variant that cuts
//! to the pixel keeps its own pixels inside a snapped neighbour's cell.
//!
//! For the whole frame (#534), `silhouette = "cell"` also snaps the edge
//! of another style's entity: `StylePass::ownership_cells` hands the
//! framework the whole frame's grid, and the ownership cut gives each cell
//! along a pencil prop wholly to the glyph (at least `cell_share` of it
//! not the prop's) or wholly to the prop (its scene pixels plain). See
//! [`super::ownership`].

use crate::gpu_counts::CountedDevice;
use super::kit;
use super::variant_table::{report_drop, storage_entry, VariantTable};
use super::text::{self, GlyphCell, GlyphText};
use super::{
    choices_with, ChoiceScope, MaskUse, StyleDescriptor, StyleDevice, StyleFrame, StyleInput, StyleOption, StyleParam, StylePass, StyleStage, StyleTexture,
};
use crate::glyph_atlas::{
    calibrated_bounds, GlyphRamp, GlyphSet, GlyphSetCache, GpuGlyphStack, RampMap, BUILTIN_SET,
};
use crate::palette::{Palette, PaletteCache, BUILTIN_PALETTES, MAX_COLORS};
use bytemuck::{Pod, Zeroable};
use flint_core::project::AssetKind;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

mod uniforms;
mod cells;
mod state;
mod pass;
pub use uniforms::*;
pub use cells::*;
pub use state::*;
use pass::*;

/// Ramp steps the uniform holds; a longer ramp is resampled to this many.
pub const MAX_RAMP: usize = 128;

/// The colour modes, by their option spelling.
pub const COLOR_MODES: &[&str] = &["source", "tint", "raw", "mono", "palette"];

/// What inks the edges' line glyphs (#351), by their option spelling:
/// as the cell's tone glyph, one colour, or the near side's hue.
pub const EDGE_INKS: &[&str] = &["cell", "color", "hue"];

/// The edge ink's index in [`EDGE_INKS`] (0, `cell`, for an unknown one).
fn edge_ink_mode(name: &str) -> u32 {
    EDGE_INKS.iter().position(|m| *m == name).unwrap_or(0) as u32
}

/// The ground (background fill) modes, by their option spelling.
pub const GROUND_MODES: &[&str] = &["dim", "black", "inverse"];

/// The palette `color = "palette"` uses when the scene names none.
pub const DEFAULT_PALETTE: &str = "phosphor";

/// The built-in glyph set's ramps, in the order the menu offers them.
pub const RAMPS: &[&str] = &[
    "classic",
    "blocks",
    "minimal",
    "detailed",
    "quadrants",
    "braille",
];

/// The fonts to offer (#322): the built-in set, then every glyph set the
/// scene can see (`fonts/*.glyphs.toml`, by the project's search).
fn font_choices(scope: &ChoiceScope<'_>) -> Vec<String> {
    choices_with(&[BUILTIN_SET], scope.files(AssetKind::GlyphSets))
}

/// The palettes to offer (#322): the built-ins, then every palette the
/// scene can see (`palettes/*.palette.toml`, by the project's search).
pub(super) fn palette_choices(scope: &ChoiceScope<'_>) -> Vec<String> {
    choices_with(BUILTIN_PALETTES, scope.files(AssetKind::Palettes))
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
    let Some(assets) = scope.assets else {
        return builtin();
    };
    match GlyphSetCache::new(assets.clone()).get(&font) {
        Ok(set) => set.ramps().map(|r| r.name().to_string()).collect(),
        Err(_) => builtin(),
    }
}

const PARAMS: &[StyleParam] = &[
    StyleParam::new("cell", 8.0, 4.0..=32.0)
        .label("Cell size (pt)")
        .help("Cell width in points; the height follows the font")
        .integer(),
    StyleParam::new("brightness", 0.0, -1.0..=1.0)
        .label("Brightness")
        .help("Shifts the tone before a glyph is picked"),
    StyleParam::new("contrast", 1.0, 0.0..=4.0)
        .label("Contrast")
        .help("Spreads the tone around mid-grey before a glyph is picked"),
    StyleParam::new("background", 0.15, 0.0..=1.0)
        .label("Background")
        .help("How much of the scene shows behind the glyphs (dim ground)")
        .when("ground", &["dim", "inverse"])
        .unless("color", &["palette"]),
    StyleParam::new("ink", 1.0, 0.0..=2.0).label("Ink").help("Multiplies the glyph colour"),
    StyleParam::new("levels", 0.0, 0.0..=16.0)
        .label("Tone bands")
        .help("Quantise tone to this many bands; 0 = smooth")
        .integer(),
    StyleParam::new("footprint", 1.0, 1.0..=4.0)
        .label("Footprint (cells)")
        .help("How many cells wide each cell's colour is averaged over")
        .integer(),
    StyleParam::new("smoothing", 0.6, 0.0..=0.95)
        .label("Smoothing")
        .help("How much of last frame's colour a cell keeps, per 60 Hz frame (the same settling time at any frame rate); a cell whose colour jumps takes the new one at once. Edge cells keep its square root of their line's length, direction and place"),
    StyleParam::new("hysteresis", 0.35, 0.0..=1.0)
        .label("Hysteresis")
        .help("How far, in ramp steps, a cell's tone may stray before its glyph changes; and how far an edge's length, direction and place may stray before its line glyph changes"),
    StyleParam::new("dither", 0.0, 0.0..=1.0)
        .label("Dither")
        .help("Ordered 4x4 dither before the ramp, fixed to the grid: gradients become a still pattern"),
    StyleParam::new("chroma", 1.0, 0.0..=4.0)
        .label("Palette chroma")
        .help("For a nearest-ink palette: scales the cell's saturation before matching, so a muted scene reaches the palette's colours; 1 = as it is")
        .when("color", &["palette"]),
    StyleParam::new("palette_dither", 0.0, 0.0..=1.0)
        .label("Palette dither")
        .help("For a nearest-ink palette: mixes the two nearest inks by a 4x4 Bayer pattern on the cells, so gradients step through inks instead of banding")
        .when("color", &["palette"]),
    StyleParam::new("exposure", 0.0, -4.0..=4.0)
        .label("Exposure (stops)")
        .help("Brightens (or darkens) the cell's colour before its glyph is picked: +1 doubles it in linear light"),
    StyleParam::new("normalize", 0.0, 0.0..=1.0)
        .label("Normalize")
        .help("Stretches this style's own tone range (the whole frame's, or each styled entity's) over the glyph ramp, so a dark prop draws its full ramp; 0 off, 1 full"),
    StyleParam::new("tint_r", 0.35, 0.0..=1.0)
        .label("Tint")
        .help("The ink for colour mode tint")
        .when("color", &["tint"]),
    StyleParam::new("tint_g", 1.0, 0.0..=1.0),
    StyleParam::new("tint_b", 0.45, 0.0..=1.0),
    StyleParam::new("glow", 0.0, 0.0..=2.0)
        .label("Glow")
        .help("A halo around bright glyphs, inside each cell"),
    StyleParam::new("edges", 1.0, 0.0..=1.0)
        .label("Edges")
        .help("Line glyphs along silhouettes, creases and styled outlines, over the tone glyph; 0 = off"),
    StyleParam::new("edge_threshold", 0.5, 0.05..=2.0)
        .label("Edge threshold")
        .help("Cell widths of edge a cell needs to draw a line glyph; lower finds more"),
    StyleParam::new("edge_max", 3.0, 0.5..=8.0)
        .label("Edge max")
        .help("More edge than this (in cell widths) is texture, like grass, and keeps its tone glyph"),
    StyleParam::new("edge_depth", 0.6, 0.05..=4.0)
        .label("Edge depth")
        .help("The relative view-distance step that counts as a silhouette; lower finds more"),
    StyleParam::new("edge_normal", 2.5, 0.2..=8.0)
        .label("Edge crease")
        .help("The normal change that counts as a crease; lower finds more"),
    // Corners (#348): a cell whose edge pixels run in no one direction
    // (the structure tensor's coherence under this) draws the corner or tee
    // its neighbours' line glyphs meet in, where its font has one
    StyleParam::new("edge_corner", 0.9, 0.0..=1.0)
        .label("Edge corners")
        .help("Where a cell's edge turns (its direction this uncertain or less, 0..1), it draws the corner or tee its neighbours' lines meet in: ┌ ├ ┼; so do a line ending beside a perpendicular one and a gap between two (#637); 0 = every corner off"),
    // The edge ink (#351): `edge_ink = "color"` inks line glyphs in
    // this colour (the ASCII spike's warm off-white), mixed toward the
    // near side's hue by `edge_tint`
    StyleParam::new("edge_color_r", 0.85, 0.0..=1.0)
        .label("Edge colour")
        .help("The line glyphs' ink for edge ink color"),
    StyleParam::new("edge_color_g", 0.82, 0.0..=1.0),
    StyleParam::new("edge_color_b", 0.74, 0.0..=1.0),
    StyleParam::new("edge_tint", 0.0, 0.0..=1.0)
        .label("Edge tint")
        .help("For edge ink color: mixes the colour toward the near side's hue; 0 = the colour alone, 1 = the hue"),
    StyleParam::new("outline", 1.0, 0.0..=4.0)
        .label("Silhouette (pt)")
        .help("Per-entity: inks the entity's own pixels this wide along its silhouette, so legs and rails thinner than a cell keep their shape; 0 = off"),
    StyleParam::new("min_coverage", 0.5, 0.0..=1.0)
        .label("Min coverage")
        .help("Per-entity: the least ink in a cell the entity only grazes (a thin feature), 0..1"),
    StyleParam::new("cell_share", 0.5, 0.0625..=1.0)
        .label("Cell share")
        .help("Silhouette cell: how much of a cell the entity must cover for the cell to be drawn as its glyph; lower keeps thinner features. For the whole frame, how much of a cell must not be another style's entity for it to stay a glyph"),
];

const OPTIONS: &[StyleOption] = &[
    StyleOption::new("font", "builtin", &[])
        .discovered(font_choices)
        .label("Font")
        .help("builtin, or a fonts/<name>.glyphs.toml set beside the scene"),
    StyleOption::new("ramp", "", RAMPS)
        .discovered(ramp_choices)
        .label("Ramp")
        .help("Which of the font's ramps maps tone to glyphs; (default) is the font's own"),
    StyleOption::new("color", "source", COLOR_MODES)
        .label("Colour mode")
        .help("source: the cell's hue; tint: one colour; raw: the cell's colour; mono: white; palette: a palette's inks"),
    StyleOption::new("palette", DEFAULT_PALETTE, BUILTIN_PALETTES)
        .discovered(palette_choices)
        .label("Palette")
        .help("For colour mode palette: a built-in, or palettes/<name>.palette.toml")
        .when("color", &["palette"]),
    StyleOption::new("ground", "dim", GROUND_MODES)
        .label("Ground")
        .help("Behind the glyphs: dim scene, black, or inverse (ink fills the cell, the glyph is cut out)"),
    StyleOption::new("temporal", "on", TEMPORAL_MODES)
        .label("Temporal")
        .help("on: cells keep part of last frame so glyphs don't shimmer in motion; reproject: each cell reads its past where its surface was last frame, so glyphs travel with a moving camera; off: every frame stands alone"),
    StyleOption::new("scope", "screen", SCOPES)
        .label("Scope")
        .help("screen: the whole frame in glyphs; entities: only ascii_style entities, starting from this table"),
    StyleOption::new("grid", "screen", GRIDS)
        .label("Grid")
        .help("screen: one grid centred on the frame; object: cells pinned to each styled entity's origin"),
    StyleOption::new("transition", "fade", TRANSITIONS)
        .label("Transition")
        .help("How an entity with blend below 1 shows the glyphs: fade cross-fades every pixel; dissolve turns that share of its cells, in patches"),
    StyleOption::new("edge_ink", "cell", EDGE_INKS)
        .label("Edge ink")
        .help("What inks the line glyphs: cell (as the cell's tone glyph), color (one colour, Edge colour), or hue (the near side's own colour, not the cell's mix of it and what's behind)"),
    StyleOption::new("silhouette", "pixel", SILHOUETTES)
        .label("Silhouette")
        .help("pixel cuts the glyphs at the entity's exact edge; cell draws each edge cell whole or not at all, by how much of it the entity covers. For the whole frame, cell also snaps the edge of another style's entity (a pencil prop) to whole glyphs"),
];

pub(super) static STYLE: StyleDescriptor =
    StyleDescriptor::new("ascii", "ASCII", StyleStage::Display, create)
        .params(PARAMS)
        .options(OPTIONS)
        // The mask gives styled entities their outlines (#267), so only while
        // edges are on and something is styled (#350)
        .uses_mask(MaskUse::When(reads_mask))
        // Its line glyphs are drawn from the shared edges (#743), so only
        // while edges are on or the edge view shows them
        .needs_edges_when(needs_edges)
        .component("ascii_style")
        // Its fade mixes in display space and its dissolve turns whole cells (#278)
        .blends_itself();

fn create(d: &StyleDevice) -> Box<dyn StylePass> {
    let device = d.device;
    let uniform_bgl = kit::uniform_layout(device, "ASCII Uniform BGL");
    // Group 0 of every pass over ascii_common.wgsl (#336): the uniform,
    // the variant rows and each mask id's slot
    let table_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ASCII Table BGL"),
        entries: &[uniform_entry(), storage_entry(1), storage_entry(2)],
    });
    // The edge cell pass's and the edge view's group 0: its uniform and
    // the slots, which say whose ids draw outlines (ascii_edge_source.wgsl)
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
            // The dissolving variants' cell ranks (#398), likewise
            wgpu::BindGroupLayoutEntry {
                binding: 6,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            // Each edge cell's glyph, decided once per cell (#910), likewise
            texture_entry(7, wgpu::TextureSampleType::Uint),
            // Which snapping variant holds each cell, decided once per cell
            // (#929), likewise
            texture_entry(8, wgpu::TextureSampleType::Uint),
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
    // What the edge pixels are read from (#743, ascii_edge_source.wgsl):
    // the shared edges, the style mask and the scene's depth
    let source_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ASCII Edge Source BGL"),
        entries: &[
            texture_entry(0, wgpu::TextureSampleType::Float { filterable: false }),
            texture_entry(1, wgpu::TextureSampleType::Uint),
            texture_entry(2, wgpu::TextureSampleType::Depth),
        ],
    });
    // Bound while no entity asks for the style: one unstyled pixel
    let no_mask = device.create_texture_counted(&wgpu::TextureDescriptor {
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
    // Bound if the shared edge pass couldn't be built: no edge anywhere, so
    // only the mask's outlines are found
    let no_edges = device.create_texture_counted(&wgpu::TextureDescriptor {
        label: Some("ASCII No Edges"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: super::edges::EDGE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let no_edges_view = no_edges.create_view(&Default::default());
    // The cell pass's past (#277, #344): last frame's cells and their
    // view depths, and the scene depth to reproject through
    let history_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ASCII History BGL"),
        entries: &[
            texture_entry(0, wgpu::TextureSampleType::Uint),
            texture_entry(1, wgpu::TextureSampleType::Float { filterable: false }),
            texture_entry(2, wgpu::TextureSampleType::Depth),
        ],
    });
    let cells_pipeline = kit::fullscreen_pipeline_targets(
        device,
        "ASCII Cells Pipeline",
        &kit::shader(device, "ASCII Cells Shader", CELLS_SHADER),
        &[&table_bgl, &input_bgl, &history_bgl, &edges_in_bgl],
        &[CELL_FORMAT, CELL_DEPTH_FORMAT],
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
    // Which snapping variant holds each cell (#929): the mask in, as the
    // variant cells read it, a band a snapping grid, each drawn with its
    // first variant's slot as the instance
    let snap_owner_pipeline = kit::fullscreen_pipeline_vertex(
        device,
        "ASCII Snap Owner Pipeline",
        &kit::shader(device, "ASCII Snap Owner Shader", SNAP_OWNER_SHADER),
        "vs_owner",
        &[&table_bgl, &stats_in_bgl],
        SNAP_OWNER_FORMAT,
    );
    // The dissolve's order and rank (#398): the mask in, then the orders
    let dissolve_order_pipeline = kit::fullscreen_pipeline(
        device,
        "ASCII Dissolve Order Pipeline",
        &kit::shader(device, "ASCII Dissolve Order Shader", DISSOLVE_ORDER_SHADER),
        &[&table_bgl, &stats_in_bgl],
        DISSOLVE_FORMAT,
    );
    let dissolve_rank_pipeline = kit::fullscreen_pipeline(
        device,
        "ASCII Dissolve Rank Pipeline",
        &kit::shader(device, "ASCII Dissolve Rank Shader", DISSOLVE_RANK_SHADER),
        &[&table_bgl, &edges_in_bgl],
        DISSOLVE_FORMAT,
    );
    let stats_resolve_module =
        kit::shader(device, "ASCII Stats Resolve Shader", STATS_RESOLVE_SHADER);
    let stats_resolve_pipeline = kit::fullscreen_pipeline(
        device,
        "ASCII Stats Resolve Pipeline",
        &stats_resolve_module,
        &[&table_bgl, &edges_in_bgl],
        STATS_FORMAT,
    );
    // Each row's runs summed into the row (#337)
    let stats_rows_pipeline = kit::fullscreen_pipeline_with(
        device,
        "ASCII Stats Rows Pipeline",
        &stats_resolve_module,
        "fs_rows",
        &[&table_bgl, &edges_in_bgl],
        STATS_FORMAT,
        None,
    );
    let glyph_module = kit::shader(device, "ASCII Shader", GLYPH_SHADER);
    // The edge view's last group (#349, #910): the edge cells and their
    // decided slots
    let edge_view_cells_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ASCII Edge View Cells BGL"),
        entries: &[
            texture_entry(0, wgpu::TextureSampleType::Float { filterable: false }),
            texture_entry(1, wgpu::TextureSampleType::Uint),
            // The mask's outlines (#743)
            texture_entry(2, wgpu::TextureSampleType::Float { filterable: false }),
        ],
    });
    // The edge cell pass's last group: last frame's edge cells (#477) and
    // this frame's outlines (#743)
    let past_outline_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ASCII Past Cells And Outlines BGL"),
        entries: &[
            texture_entry(0, wgpu::TextureSampleType::Float { filterable: false }),
            texture_entry(1, wgpu::TextureSampleType::Float { filterable: false }),
        ],
    });
    // The edge debug view (#349): the edge pixels and the edge cells in
    let edge_view_pipeline = kit::fullscreen_pipeline(
        device,
        "ASCII Edge View Pipeline",
        &kit::shader(device, "ASCII Edge View Shader", EDGE_VIEW_SHADER),
        // The edge pixels' table and inputs first, as the cell pass's
        &[&edge_bgl, &source_bgl, &input_bgl, &edge_view_cells_bgl],
        d.format,
    );
    // The thin pass's last group: last frame's edge slots (#477) and the
    // style mask, which with the table says which cells are drawn (#493)
    let thin_in_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ASCII Edge Thin Inputs BGL"),
        entries: &[
            texture_entry(0, wgpu::TextureSampleType::Uint),
            texture_entry(1, wgpu::TextureSampleType::Uint),
        ],
    });
    // Each edge cell's glyph, once per cell (#910): thinned lines and
    // marked corners' arms, then the glyph every cell draws
    let edge_thin_pipeline = kit::fullscreen_pipeline(
        device,
        "ASCII Edge Thin Pipeline",
        &kit::shader(device, "ASCII Edge Thin Shader", EDGE_THIN_SHADER),
        // The variant table, the edge cells, its uniform, then last
        // frame's edge slots (#477) and the mask (#493)
        &[&table_bgl, &edges_in_bgl, &uniform_bgl, &thin_in_bgl],
        EDGE_SLOT_FORMAT,
    );
    let edge_corner_pipeline = kit::fullscreen_pipeline(
        device,
        "ASCII Edge Corner Pipeline",
        &kit::shader(device, "ASCII Edge Corner Shader", EDGE_CORNER_SHADER),
        // The thin pass's uniform and last frame's slots (#477)
        &[&cells_bgl, &edges_in_bgl, &uniform_bgl, &cells_bgl],
        EDGE_SLOT_FORMAT,
    );
    let blit = kit::Blit::new(device, "ASCII Blit", &input_bgl, d.format);
    Box::new(Ascii {
        outline_pipeline: kit::fullscreen_pipeline(
            device,
            "ASCII Edge Outline Pipeline",
            &kit::shader(device, "ASCII Edge Outline Shader", EDGE_OUTLINE_SHADER),
            // The edge cell pass's table (its uniform starts with the edge
            // source) and the edge pixels' inputs
            &[&edge_bgl, &source_bgl],
            super::edges::EDGE_FORMAT,
        ),
        edge_cell_pipeline: kit::fullscreen_pipeline(
            device,
            "ASCII Edge Cell Pipeline",
            &kit::shader(
                device,
                "ASCII Edge Cell Shader",
                EDGE_CELLS_SHADER,
            ),
            // The edge table and the edge pixels' inputs (#743), the picture
            // (#351: the near side's hue), last frame's edge cells (#477) and
            // the outlines
            &[&edge_bgl, &source_bgl, &stats_in_bgl, &past_outline_bgl],
            EDGE_CELL_FORMAT,
        ),
        edge_cell_uniforms: kit::uniform_buffer::<EdgeCellUniforms>(
            device,
            "ASCII Edge Cell Uniforms",
        ),
        edge_thin_uniforms: kit::uniform_buffer::<EdgeThinUniforms>(
            device,
            "ASCII Edge Thin Uniforms",
        ),
        edge_view_pipeline,
        edge_view_cells_bgl,
        edge_thin_pipeline,
        edge_corner_pipeline,
        thin_in_bgl,
        edge_view_uniforms: kit::uniform_buffer::<EdgeViewUniforms>(
            device,
            "ASCII Edge View Uniforms",
        ),
        edges_in_bgl,
        source_bgl,
        past_outline_bgl,
        outlines: Mutex::new(None),
        _no_edges: no_edges,
        no_edges_view,
        edge_cells: Mutex::new(None),
        edge_thin: Mutex::new(None),
        edge_slots: Mutex::new(None),
        edge_past: Mutex::new(EdgePast::default()),
        cells_pipeline,
        variant_cells_pipeline,
        variant_cells: Mutex::new([None, None]),
        snap_owner_pipeline,
        snap_owners: Mutex::new(None),
        dissolve_order_pipeline,
        dissolve_rank_pipeline,
        dissolve: Mutex::new([None, None]),
        stats_pipeline,
        stats_resolve_pipeline,
        stats_rows_pipeline,
        stats_in_bgl,
        stats_runs: Mutex::new(None),
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
        blit,
        uniforms: kit::uniform_buffer::<AsciiUniforms>(device, "ASCII Uniforms"),
        uniform_bgl,
        table_bgl,
        edge_bgl,
        table: Mutex::new(VariantTable::new(device, "ASCII", std::mem::size_of::<VariantUniform>())),
        input_bgl,
        cells_bgl,
        history_bgl,
        atlas_bgl,
        fonts: Mutex::new(Fonts::default()),
        history: Mutex::new(History::default()),
        imported: Default::default(),
    })
}

/// The cell pass's output: one texel a cell, the cell's colour as 16-bit
/// unorm in RGB and its ramp step in A.
/// The most of last frame's cell colour one frame may keep (#342): a
/// 0.95 `smoothing` at 240 Hz is 0.987 a frame, and at 0.999 a 16-bit
/// channel still moves by a level for a change over ~0.008. The cells
/// shader holds the same cap.
const SMOOTHING_MAX: f32 = 0.999;

pub const CELL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Uint;

/// The cell pass's second output (#344): each cell centre's view depth
/// (clip w), which the next frame's reprojection tests its past against.
const CELL_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;

/// A fragment-visible, unarrayed, single-sample texture binding.
fn texture_entry(binding: u32, sample_type: wgpu::TextureSampleType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

const CELLS_SHADER: kit::Wgsl = kit::wgsl!(
    "prelude_core.wgsl",
    "ascii_common.wgsl",
    "ascii_cells.wgsl"
);
pub(crate) const GLYPH_SHADER: kit::Wgsl = kit::wgsl!(
    "prelude_core.wgsl",
    "ascii_common.wgsl",
    "../palette.wgsl",
    "ascii_dissolve.wgsl",
    "ascii.wgsl",
    "ascii_edge_slot.wgsl"
);
const EDGE_OUTLINE_SHADER: kit::Wgsl = kit::wgsl!(
    "prelude_core.wgsl",
    "ascii_edge_source.wgsl",
    "ascii_edge_outline.wgsl"
);
const EDGE_CELLS_SHADER: kit::Wgsl = kit::wgsl!(
    "prelude_core.wgsl",
    "ascii_edge_source.wgsl",
    "ascii_edge_cells.wgsl"
);
const EDGE_VIEW_SHADER: kit::Wgsl = kit::wgsl!(
    "prelude_core.wgsl",
    "ascii_edge_source.wgsl",
    "ascii_edge_view.wgsl",
    "ascii_edge_slot.wgsl"
);
const EDGE_THIN_SHADER: kit::Wgsl = kit::wgsl!(
    "prelude_core.wgsl",
    "ascii_common.wgsl",
    "ascii_edge_slot.wgsl",
    "ascii_edge_thin.wgsl"
);
const EDGE_CORNER_SHADER: kit::Wgsl = kit::wgsl!(
    "ascii_edge_slot.wgsl",
    "ascii_edge_corners.wgsl"
);
const DISSOLVE_ORDER_SHADER: kit::Wgsl = kit::wgsl!(
    "prelude_core.wgsl",
    "ascii_common.wgsl",
    "ascii_dissolve.wgsl",
    "ascii_dissolve_order.wgsl"
);
const DISSOLVE_RANK_SHADER: kit::Wgsl = kit::wgsl!(
    "prelude_core.wgsl",
    "ascii_common.wgsl",
    "ascii_dissolve.wgsl",
    "ascii_dissolve_rank.wgsl"
);
const VARIANT_CELLS_SHADER: kit::Wgsl = kit::wgsl!(
    "prelude_core.wgsl",
    "ascii_common.wgsl",
    "ascii_variant_cells.wgsl"
);
const SNAP_OWNER_SHADER: kit::Wgsl = kit::wgsl!(
    "prelude_core.wgsl",
    "ascii_common.wgsl",
    "ascii_snap_owner.wgsl"
);
const STATS_SHADER: kit::Wgsl = kit::wgsl!(
    "prelude_core.wgsl",
    "ascii_common.wgsl",
    "ascii_stats.wgsl"
);
const STATS_RESOLVE_SHADER: kit::Wgsl = kit::wgsl!(
    "prelude_core.wgsl",
    "ascii_common.wgsl",
    "ascii_stats_resolve.wgsl"
);

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::project::AssetScope;
    use std::path::Path;

    #[test]
    fn edge_regions_hold_the_cells_under_a_rect_their_neighbours_and_their_pixels() {
        // 8 px cells from origin 4 on a 100 x 50 frame: cell k is pixels
        // 4 + 8k .. 12 + 8k, texel k + 1
        let size = [100, 50];
        let (cell, origin) = ([8.0, 8.0], [4.0, 4.0]);
        let grid = cell_grid(size, cell, origin);
        let glyph = kit::PixelRect {
            x: 20,
            y: 12,
            width: 8,
            height: 1,
        };
        let r = edge_regions(glyph, cell, origin, size, grid, size);
        // Pixels 20..28 are cell 2, texel 3; with neighbours texels 2..=4
        assert_eq!((r.edge_cells.x, r.edge_cells.width), (2, 3));
        // Pixel row 12 is cell 1, texel 2; texels 1..=3
        assert_eq!((r.edge_cells.y, r.edge_cells.height), (1, 3));
        // Texels 2..=4 are cells 1..=3, pixels 12..36, padded a pixel
        assert_eq!((r.edge_pixels.x, r.edge_pixels.width), (11, 26));
        // At half the depth size: pixels 6..18, padded
        let half = edge_regions(glyph, cell, origin, size, grid, [50, 25]);
        assert_eq!((half.edge_pixels.x, half.edge_pixels.width), (5, 14));
        // At the frame's corner the regions clamp to the textures
        let corner = edge_regions(
            kit::PixelRect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
            cell,
            origin,
            size,
            grid,
            size,
        );
        assert_eq!((corner.edge_cells.x, corner.edge_cells.y), (0, 0));
        assert_eq!((corner.edge_pixels.x, corner.edge_pixels.y), (0, 0));
        assert!(
            edge_regions(kit::PixelRect::EMPTY, cell, origin, size, grid, size)
                .edge_pixels
                .is_empty()
        );
    }

    /// #930: every edge cell a drawn cell's glyph reads this frame is one
    /// the scissored edge cell pass writes. The glyph pass draws an
    /// entity's cells, those its screen bounds touch; its rect is those
    /// bounds grown by two of the largest cell (`encode`'s
    /// `styled_rect`, `GLYPH_RECT_CELLS` of them), and the edge regions are
    /// the cells under the rect and one more. Whatever the cell size, its
    /// fraction of a pixel, the frame's leftover origin and where the
    /// bounds fall, the cells within `EDGE_CORNER_REACH` of a drawn cell
    /// lie inside them, clamped to the grid.
    #[test]
    fn edge_regions_hold_every_cell_a_drawn_cells_corner_reads() {
        let mut checked = 0;
        for size in [[256u32, 256u32], [333, 187], [1920, 1080]] {
            for cell_px in [4.0f32, 6.0, 8.0, 12.0, 13.5, 16.0, 32.0] {
                let cell = [cell_px, cell_px * 1.5];
                let origin = grid_origin(size, cell);
                let grid = cell_grid(size, cell, origin);
                let largest = cell[0].max(cell[1]);
                // Bounds from a sliver to a dozen cells, all over the frame,
                // the frame's edges included
                for x0 in (0..size[0]).step_by(37) {
                    for w in [1u32, 3, 9, 41] {
                        let y0 = (x0 * 7 + w * 13) % size[1];
                        let bounds = kit::PixelRect {
                            x: x0,
                            y: y0,
                            width: w.min(size[0] - x0),
                            height: (w * 2).min(size[1] - y0),
                        };
                        let pad = (GLYPH_RECT_CELLS as f32 * largest).ceil() as u32;
                        let glyph = bounds.padded(pad, size[0], size[1]);
                        let r = edge_regions(glyph, cell, origin, size, grid, size);
                        // The texels the bounds' pixels lie in (texel t is
                        // cell t - 1, `CELL_OFFSET`)
                        let texel =
                            |p: u32, c: f32, o: f32| ((p as f32 - o) / c).floor() as i64 + 1;
                        let span = |p0: u32, len: u32, c: f32, o: f32, n: u32| {
                            let reach = EDGE_CORNER_REACH as i64;
                            let lo = (texel(p0, c, o) - reach).max(0);
                            let hi = (texel(p0 + len - 1, c, o) + reach).min(n as i64 - 1);
                            (lo as u32, hi as u32)
                        };
                        let (tx0, tx1) = span(bounds.x, bounds.width, cell[0], origin[0], grid[0]);
                        let (ty0, ty1) = span(bounds.y, bounds.height, cell[1], origin[1], grid[1]);
                        let e = r.edge_cells;
                        assert!(
                            e.x <= tx0
                                && e.x + e.width > tx1
                                && e.y <= ty0
                                && e.y + e.height > ty1,
                            "{size:?}, cell {cell:?}, bounds {bounds:?}: the corners read \
                             texels {tx0}..={tx1} x {ty0}..={ty1}, the edge cell pass writes {e:?}"
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 1000, "{checked}");
    }

    #[test]
    fn uniforms_match_the_shader_layout() {
        // 160 bytes of scalars (#267's edge strength, #320's palette look), 7
        // vec4<u32> of edge glyphs (#348's corners), 64 vec4<u32> of ramp, 2 x 32 vec4<f32> of
        // palette (inks, then their OKLab), then the variant table's 16-byte
        // header (its rows, and its slots, are storage buffers since #336:
        // a row is 144 bytes, #288's silhouette floors and #320's palette
        // look, then #335's glyph cell and edge start), and the variant cell
        // bands' two vec4 each (#313), reprojection's two mat4 and its
        // vec4<u32> (#344), #351's edge colour, then #398's dissolve bands
        // (a vec4 header and MAX_DISSOLVE vec4s), then #507's mask rect, then
        // #929's snap owner band a snap grid. A row is 192 bytes since
        // #929's owner band, after #507's bounds and #398's dissolve row
        // padded to 16
        assert_eq!(std::mem::size_of::<VariantUniform>(), 192);
        assert_eq!(std::mem::offset_of!(VariantUniform, glyph), 128);
        assert_eq!(std::mem::offset_of!(VariantUniform, owner), 176);
        assert_eq!(
            std::mem::size_of::<AsciiUniforms>(),
            160 + 7 * 16
                + 64 * 16
                + 2 * MAX_COLORS * 16
                + 16
                + MAX_BANDS * 32
                + MAX_SNAP_GRIDS * 16
                + 16
                + 2 * 64
                + 16
                + 16
                + MAX_DISSOLVE * 16
                + MAX_RAMP * 4
                + 16
                + MAX_SNAP_GRIDS * 16
        );
        assert_eq!(
            std::mem::offset_of!(AsciiUniforms, inv_view_proj) % 16,
            0,
            "a mat4x4 is 16-aligned in WGSL"
        );
        // #351's edge colour, #398's dissolve bands, #281's ramp tones, then
        // #507's mask rect, then #929's snap owner bands, last
        assert_eq!(
            std::mem::offset_of!(AsciiUniforms, edge_color),
            std::mem::size_of::<AsciiUniforms>()
                - 32
                - MAX_DISSOLVE * 16
                - MAX_RAMP * 4
                - 16
                - MAX_SNAP_GRIDS * 16
        );
        assert_eq!(std::mem::offset_of!(AsciiUniforms, edges), 128);
        assert_eq!(std::mem::offset_of!(AsciiUniforms, edge_glyphs), 160);
        // #743's edge source: the camera, the plain id, the thresholds' scales
        assert_eq!(std::mem::size_of::<EdgeSource>(), 96);
        // #477's history, hold and smoothing, after the edge source
        assert_eq!(std::mem::size_of::<EdgeCellUniforms>(), 96 + 64);
        // #493's grid and who draws
        assert_eq!(std::mem::size_of::<EdgeThinUniforms>(), 48);
    }

    #[test]
    fn the_edge_shaders_validate_and_their_uniforms_match_rust() {
        for (name, src, size) in [
            (
                "ascii_edge_cells.wgsl",
                EDGE_CELLS_SHADER.text,
                std::mem::size_of::<EdgeCellUniforms>(),
            ),
            (
                "ascii_edge_view.wgsl",
                EDGE_VIEW_SHADER.text,
                std::mem::size_of::<EdgeViewUniforms>(),
            ),
        ] {
            let module = naga::front::wgsl::parse_str(src).expect(name);
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                // the edge cells' f16 packing (#945)
                naga::valid::Capabilities::default()
                    | naga::valid::Capabilities::SHADER_FLOAT16_IN_FLOAT32,
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

    /// The edge thin and corner passes (#910) validate, and the glyph and
    /// edge view shaders no longer carry the per-pixel decision: inlined
    /// by FXC, it made the glyph shader take up to a minute on DX12.
    #[test]
    fn edge_glyphs_are_decided_once_per_cell() {
        for (name, src) in [
            ("ascii_edge_thin.wgsl", EDGE_THIN_SHADER.text),
            ("ascii_edge_corners.wgsl", EDGE_CORNER_SHADER.text),
        ] {
            let module = naga::front::wgsl::parse_str(src).expect(name);
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .expect(name);
            // Both read the thin pass's uniform (#477), each declaring it
            let (_, t) = module
                .global_variables
                .iter()
                .find(|(_, v)| v.name.as_deref() == Some("t"))
                .expect("uniform t");
            assert_eq!(
                module.types[t.ty].inner.size(module.to_ctx()) as usize,
                std::mem::size_of::<EdgeThinUniforms>(),
                "{name}"
            );
        }
        for (name, src) in [
            ("glyph", GLYPH_SHADER.text),
            ("edge view", EDGE_VIEW_SHADER.text),
        ] {
            for decision in ["fn thinned_slot", "fn cornered_slot"] {
                assert!(!src.contains(decision), "the {name} shader has {decision}");
            }
        }
    }

    #[test]
    fn the_variant_table_grows_to_the_next_power_of_two_and_keeps_it() {
        // Starts at MIN_ENTRIES, doubles past it, never shrinks (#336)
        use super::super::variant_table::{grown_capacity, MIN_ENTRIES};
        assert_eq!(grown_capacity(MIN_ENTRIES, 501), 512);
        assert_eq!(grown_capacity(512, 3), 512);
        assert_eq!(grown_capacity(512, 513), 1024);
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
            "edge_corner",
        ] {
            let p = STYLE.params.iter().find(|p| p.name == name).unwrap();
            assert!(p.min <= p.default && p.default <= p.max, "{name}");
        }
        let edges = STYLE.params.iter().find(|p| p.name == "edges").unwrap();
        assert_eq!(edges.default, 1.0);
        assert!(STYLE.uses_mask.ever(), "outlines read the style mask");
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
        for choice in &*palette.choices {
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
            let assets = AssetScope::for_dir(Path::new("game/levels"));
            let scope = ChoiceScope::defaults(Some(&assets));
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

    /// ADR 0114: the lists are the resolver's. A scene two folders down is
    /// offered the project's palettes and fonts, a nearer file of a name
    /// hides the project's, and every name offered is one the caches find.
    #[test]
    fn the_lists_are_what_the_search_finds_from_a_nested_scene() {
        let mut files = game_files();
        files.insert("game/flint.toml", "");
        files.insert("game/levels/act1/cave/a.scene.toml", "");
        files.insert(
            "game/levels/act1/palettes/dusk.palette.toml",
            "colors = [\"#101010\", \"#202020\", \"#303030\"]\n",
        );
        files.insert(
            "game/engine/palettes/stock.palette.toml",
            "colors = [\"#ffffff\"]\n",
        );
        flint_core::vfs::scoped(Arc::new(files), || {
            let assets = AssetScope::for_dir(Path::new("game/levels/act1/cave"));
            let scope = ChoiceScope::defaults(Some(&assets));
            let palettes = option("palette").choices_in(&scope);
            let mut want: Vec<String> = BUILTIN_PALETTES.iter().map(|s| s.to_string()).collect();
            want.extend(["dusk", "stock", "tidepool"].map(String::from));
            assert_eq!(palettes, want);
            assert_eq!(
                option("font").choices_in(&scope),
                ["builtin", "plain", "terminal"]
            );
            let mut cache = PaletteCache::new(assets.clone());
            for name in &palettes {
                assert!(cache.get(name).is_ok(), "{name} is offered and found");
            }
            // The nearer dusk, three inks, not the project's
            assert_eq!(cache.get("dusk").unwrap().colors().len(), 3);
            let mut fonts = GlyphSetCache::new(assets.clone());
            for name in option("font").choices_in(&scope) {
                assert!(fonts.get(&name).is_ok(), "{name} is offered and found");
            }
        });
    }

    #[test]
    fn the_ramp_list_follows_the_chosen_font() {
        flint_core::vfs::scoped(Arc::new(game_files()), || {
            let assets = AssetScope::for_dir(Path::new("game/levels"));
            let font = |name: &'static str| {
                move |o: &str| match o {
                    "font" => name.to_string(),
                    _ => String::new(),
                }
            };
            let plain = font("plain");
            let scope = ChoiceScope {
                assets: Some(&assets),
                option: &plain,
            };
            let mut ramps = option("ramp").choices_in(&scope);
            ramps.sort();
            assert_eq!(ramps, ["bars", "dots"]);
            // The built-in font (or one that won't load) has the built-in four
            let missing = font("nonesuch");
            for f in [&font("builtin") as &dyn Fn(&str) -> String, &missing] {
                let scope = ChoiceScope {
                    assets: Some(&assets),
                    option: f,
                };
                let ramps = option("ramp").choices_in(&scope);
                for r in &*option("ramp").choices {
                    assert!(ramps.iter().any(|x| x == r), "{r} in {ramps:?}");
                }
            }
        });
    }

    #[test]
    fn found_options_stay_open_to_the_command_line() {
        // `--style ascii:palette=tidepool` names a file the style resolves
        let mut config = crate::PostProcessConfig::default();
        assert!(super::super::apply_style_spec(&mut config, "ascii:palette=tidepool", &crate::stylize::StyleRegistry::new()).is_ok());
        assert!(super::super::apply_style_spec(&mut config, "ascii:color=plaid", &crate::stylize::StyleRegistry::new()).is_err());
    }

    #[test]
    fn long_ramps_are_resampled_end_to_end() {
        let short: Vec<usize> = (0..10).collect();
        assert_eq!(fit_positions(10), short);
        let fit = fit_positions(300);
        assert_eq!(fit.len(), MAX_RAMP);
        assert_eq!(fit[0], 0);
        assert_eq!(*fit.last().unwrap(), 299);
        assert!(fit.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn a_calibrated_ramp_packs_its_bands_and_an_equal_one_the_sentinel() {
        // #281: the shader's `ramp_tone` table
        let set = GlyphSet::builtin();
        let classic = set.ramp("classic").unwrap();
        let fit = fit_ramp(classic);
        assert_eq!(fit.len(), classic.len());
        assert!(fit.iter().all(|(_, t)| *t == EQUAL_BANDS));
        let calibrated = classic.calibrated();
        let fit = fit_ramp(&calibrated);
        let tones: Vec<f32> = fit.iter().map(|(_, t)| *t).collect();
        assert_eq!(tones, calibrated.bounds());
        assert_eq!(tones[0], 0.0);
        assert!(tones.windows(2).all(|w| w[0] <= w[1]));
        let mut ramps = Ramps::default();
        let at = ramps.pack(0, "c", fit.clone()).unwrap();
        ramps.pack(0, "e", vec![(0, EQUAL_BANDS); 4]).unwrap();
        assert_eq!(ramps.tones.len(), ramps.steps.len());
        assert_eq!(
            &ramps.tones[at.0 as usize..(at.0 + at.1) as usize],
            &tones[..]
        );
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
            .iter()
        {
            assert!(
                set.ramp(choice).is_some(),
                "built-in set has no ramp {choice}"
            );
        }
    }
    #[test]
    fn the_shader_validates_and_its_uniforms_match_rust() {
        let module = naga::front::wgsl::parse_str(GLYPH_SHADER.text).expect("parse");
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
            ("ascii_snap_owner.wgsl", SNAP_OWNER_SHADER),
            ("ascii_dissolve_order.wgsl", DISSOLVE_ORDER_SHADER),
            ("ascii_dissolve_rank.wgsl", DISSOLVE_RANK_SHADER),
        ] {
            let module = naga::front::wgsl::parse_str(src.text).expect(name);
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
        assert!(include_str!("ascii_common.wgsl")
            .contains(&format!("const STATS_CHUNKS: u32 = {STATS_CHUNKS}u;")));
        // And so are the variant cell state's bands (#313)
        assert!(include_str!("ascii_common.wgsl")
            .contains(&format!("const MAX_BANDS: u32 = {MAX_BANDS}u;")));
        // And the dissolving variants (#398)
        assert!(include_str!("ascii_common.wgsl")
            .contains(&format!("const MAX_DISSOLVE: u32 = {MAX_DISSOLVE}u;")));
        // And the snapping grids and flag (#338)
        assert!(include_str!("ascii_common.wgsl")
            .contains(&format!("const MAX_SNAP_GRIDS: u32 = {MAX_SNAP_GRIDS}u;")));
        assert!(include_str!("ascii_common.wgsl").contains(&format!("const SNAP: u32 = {SNAP}u;")));
    }

    #[test]
    fn snapping_variants_list_each_grid_once_and_skip_the_rest() {
        let rows = vec![
            VariantUniform::zeroed(),
            variant_row(8.0, [0.0, 0.0], 1.0, SNAP),
            // The same grid: listed once
            variant_row(8.0, [0.0, 0.0], 2.0, SNAP | DISSOLVE),
            // Cut to the pixel: not listed
            variant_row(12.0, [0.0, 0.0], 1.0, 0),
            // Passes through: nothing to snap
            variant_row(16.0, [0.0, 0.0], 1.0, SNAP | PASS_THROUGH),
            // An object grid
            variant_row(8.0, [3.0, 5.0], 1.0, SNAP),
        ];
        assert_eq!(
            snap_grids(&rows),
            vec![[8.0, 8.0, 0.0, 0.0], [8.0, 8.0, 3.0, 5.0]]
        );
        let many: Vec<_> = std::iter::once(VariantUniform::zeroed())
            .chain((0..MAX_SNAP_GRIDS + 3).map(|i| variant_row(8.0, [i as f32, 0.0], 1.0, SNAP)))
            .collect();
        assert_eq!(snap_grids(&many).len(), MAX_SNAP_GRIDS);
    }

    fn snapping_row(cell: f32, origin: [f32; 2], bounds: [f32; 4]) -> VariantUniform {
        VariantUniform {
            bounds,
            ..variant_row(cell, origin, 1.0, SNAP)
        }
    }

    #[test]
    fn each_snapping_grid_gets_a_band_over_its_variants_bounds() {
        let size = [1280, 720];
        let mut rows = vec![
            VariantUniform::zeroed(),
            snapping_row(8.0, [0.0, 0.0], [100.0, 40.0, 140.0, 80.0]),
            // Not snapping, and passing through: no band
            VariantUniform {
                bounds: [0.0, 0.0, 1280.0, 720.0],
                ..variant_row(8.0, [0.0, 0.0], 1.0, 0)
            },
            VariantUniform {
                bounds: [0.0, 0.0, 1280.0, 720.0],
                ..variant_row(8.0, [0.0, 0.0], 1.0, SNAP | PASS_THROUGH)
            },
            // Row 1's grid: its band grows over both
            snapping_row(8.0, [0.0, 0.0], [300.0, 10.0, 310.0, 20.0]),
            // Its own grid (an object grid), and off screen
            snapping_row(10.0, [3.0, 5.0], [600.0, 600.0, 640.0, 700.0]),
            snapping_row(10.0, [7.0, 7.0], [1300.0, 0.0, 1400.0, 100.0]),
        ];
        let bands = assign_owner_bands(&mut rows, size);
        assert_eq!(bands.len(), 2);
        // Cells 12..17 and 37..38 across, 1..10 down, one more each side
        assert_eq!(bands[0].slot, 1);
        assert_eq!(bands[0].first, [11, 0]);
        assert_eq!([bands[0].cols, bands[0].rows], [29, 12]);
        assert_eq!(bands[0].row, 0);
        assert_eq!(rows[1].owner, bands[0].packed());
        assert_eq!(rows[4].owner, bands[0].packed());
        assert_eq!(rows[2].owner, [0; 4]);
        assert_eq!(rows[3].owner, [0; 4]);
        // Stacked under the first
        assert_eq!(bands[1].slot, 5);
        assert_eq!(bands[1].row, 12);
        assert_eq!(bands[1].first, [58, 58]);
        assert_eq!(rows[5].owner, [58, 58, 13, (bands[1].cols | bands[1].rows << 16) as i32]);
        assert_eq!(rows[6].owner, [0; 4]);
        // Every pixel of a variant's bounds lands in its band, as the
        // shader finds its cell
        for (row, band) in [(1, 0), (4, 0), (5, 1)] {
            let r = rows[row];
            let b = bands[band];
            let [x0, y0, x1, y1] = r.bounds;
            for p in [[x0 + 0.5, y0 + 0.5], [x1 - 0.5, y1 - 0.5]] {
                let c = [0, 1].map(|i| ((p[i] - r.origin[i]) / r.cell[i]).floor() as i32 - b.first[i]);
                assert!(c[0] >= 1 && (c[0] as u32) < b.cols - 1, "{row}: {c:?}");
                assert!(c[1] >= 1 && (c[1] as u32) < b.rows - 1, "{row}: {c:?}");
            }
        }
        // The uniform's bands follow `snap_grids`
        let snapping = snap_grids(&rows);
        let held = snap_bands(&snapping, &bands);
        assert_eq!(held[0], bands[0].packed());
        assert_eq!(held[1], bands[1].packed());
        // The third grid is off screen: no band
        assert_eq!(held[2], [0; 4]);
        assert_eq!(owner_texture_size(&bands, None), [29.max(bands[1].cols), 12 + bands[1].rows]);
        assert_eq!(owner_texture_size(&[], Some([40, 30])), [40, 30]);
    }

    #[test]
    fn a_snapping_grid_past_the_owner_states_rows_gets_no_band() {
        // Whole-frame grids of 1-pixel cells: 723 rows each (720 and one
        // more each side), so the twelfth passes 8192 rows
        let size = [1000, 720];
        let mut rows: Vec<_> = std::iter::once(VariantUniform::zeroed())
            .chain((0..12).map(|i| snapping_row(1.0, [i as f32 * 0.01, 0.0], [0.0, 0.0, 1000.0, 720.0])))
            .collect();
        let bands = assign_owner_bands(&mut rows, size);
        assert_eq!(bands.len(), 11);
        assert!(bands.iter().all(|b| b.rows == 723));
        assert_eq!(rows[12].owner, [0; 4]);
        assert!(rows[11].owner[2] > 0);
    }

    #[test]
    fn silhouette_is_an_option_and_cell_share_a_param() {
        assert_eq!(option("silhouette").default, "pixel");
        assert_eq!(*option("silhouette").choices, *SILHOUETTES);
        let share = STYLE
            .params
            .iter()
            .find(|p| p.name == "cell_share")
            .unwrap();
        assert_eq!(share.default, 0.5);
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
    fn each_dissolving_variant_gets_its_own_band_up_to_the_cap() {
        let size = [1280, 720];
        let mut rows = vec![
            VariantUniform::zeroed(),
            variant_row(8.0, [0.0, 0.0], 1.0, DISSOLVE),
            // Fades, and one that passes through: no band
            variant_row(8.0, [0.0, 0.0], 1.0, 0),
            variant_row(8.0, [0.0, 0.0], 1.0, DISSOLVE | PASS_THROUGH),
            // Same grid as row 1, but its own band: ranks count one slot
            variant_row(8.0, [0.0, 0.0], 1.0, DISSOLVE | SNAP),
            variant_row(16.0, [3.0, 5.0], 1.0, DISSOLVE),
        ];
        let bands = assign_dissolve(&mut rows, size);
        let grid = cell_grid(size, [8.0, 8.0], [0.0, 0.0]);
        let wide = cell_grid(size, [16.0, 16.0], [3.0, 5.0]);
        assert_eq!(
            bands,
            vec![
                [1, 0, grid[1], grid[0]],
                [4, grid[1], grid[1], grid[0]],
                [5, 2 * grid[1], wide[1], wide[0]],
            ]
        );
        let at: Vec<u32> = rows.iter().map(|r| r.dissolve_row).collect();
        assert_eq!(at, vec![0, 1, 0, 0, grid[1] + 1, 2 * grid[1] + 1]);
        assert_eq!(
            dissolve_texture_size(&bands, None),
            [grid[0], 2 * grid[1] + wide[1]]
        );
        // A frame without a dissolve keeps the textures it has
        assert_eq!(dissolve_texture_size(&[], Some([90, 46])), [90, 46]);
        assert_eq!(dissolve_texture_size(&[], None), [1, 1]);

        let mut many = vec![VariantUniform::zeroed()];
        many.extend((0..MAX_DISSOLVE + 2).map(|_| variant_row(8.0, [0.0, 0.0], 1.0, DISSOLVE)));
        assert_eq!(assign_dissolve(&mut many, size).len(), MAX_DISSOLVE);
        assert!(many[MAX_DISSOLVE + 1..].iter().all(|r| r.dissolve_row == 0));
    }

    #[test]
    fn bind_keys_never_wrap_onto_each_other() {
        // A generation shifted into a 6-bit field wrapped after 64 (#398)
        let keys: std::collections::HashSet<u64> = (0..4096u64)
            .flat_map(|g| [bind_key(&[0, 0, 0, g, 7]), bind_key(&[0, 0, g, 0, 7])])
            .collect();
        assert_eq!(keys.len(), 2 * 4096 - 1);
    }

    fn edge_key(epoch: u64) -> EdgeKey {
        EdgeKey {
            epoch,
            size: [1280, 720],
            depth_size: [1280, 720],
            cell: [8.0f32.to_bits(), 16.0f32.to_bits()],
            origin: [0, 0],
            textures: [0, 0],
        }
    }

    #[test]
    fn edge_history_follows_only_the_frame_before_with_the_same_key() {
        let mut past = EdgePast::default();
        // The first frame has no past; the next one does
        assert!(!past.advance(edge_key(0), 10, true));
        assert!(past.advance(edge_key(0), 11, true));
        // A skipped frame (edges off for a frame) leaves nothing to trust
        assert!(!past.advance(edge_key(0), 13, true));
        assert!(past.advance(edge_key(0), 14, true));
        // A camera cut's new epoch, and a resized texture, start over
        assert!(!past.advance(edge_key(1), 15, true));
        let mut resized = edge_key(1);
        resized.textures = [1, 1];
        assert!(!past.advance(resized.clone(), 16, true));
        assert!(past.advance(resized.clone(), 17, true));
        // temporal = "off" never trusts it, but still notes the frame
        assert!(!past.advance(resized.clone(), 18, false));
        assert!(past.advance(resized, 19, true));
    }

    #[test]
    fn edge_smoothing_is_the_square_root_of_the_tones() {
        let at_60 = edge_smoothing(0.6, 1.0 / 60.0);
        assert!((at_60 - 0.6f32.sqrt()).abs() < 1e-6, "{at_60}");
        // Per 60 Hz frame, as the tone's: two 120 Hz frames keep as much
        let at_120 = edge_smoothing(0.6, 1.0 / 120.0);
        assert!((at_120 * at_120 - at_60).abs() < 1e-6);
        // No smoothing is none, and the most is capped like the tone's
        assert_eq!(edge_smoothing(0.0, 1.0 / 60.0), 0.0);
        assert!(edge_smoothing(0.99, 1.0 / 240.0) <= SMOOTHING_MAX);
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
        let bands = assign_bands(&mut rows, size, MIN_BAND_FOOTPRINT);
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
            band_texture_size(&bands, None),
            [grid[0].max(wide[0]), 2 * grid[1] + wide[1]]
        );
        // With no bands the texture is a stand-in texel
        assert_eq!(band_texture_size(&[], None), [1, 1]);
        // A smaller grid, or none, keeps the textures it has
        assert_eq!(band_texture_size(&bands[..1], Some([400, 300])), [400, 300]);
        assert_eq!(band_texture_size(&[], Some([90, 46])), [90, 46]);
    }

    #[test]
    fn a_slot_past_what_a_band_texel_holds_samples_for_itself() {
        // One grid, 300 variants on it: the texel names a slot in 8 bits
        let mut rows = vec![VariantUniform::zeroed()];
        rows.extend((0..300).map(|_| variant_row(8.0, [0.0, 0.0], 2.0, 0)));
        let bands = assign_bands(&mut rows, [1280, 720], MIN_BAND_FOOTPRINT);
        assert_eq!(bands.len(), 1);
        assert!(rows[1..=MAX_BAND_SLOT].iter().all(|r| r.cell_row == 1));
        assert!(rows[MAX_BAND_SLOT + 1..].iter().all(|r| r.cell_row == 0));
    }

    #[test]
    fn a_footprint_of_one_takes_a_band_only_when_the_ids_have_bounds() {
        let size = [1280, 720];
        let mut rows = vec![VariantUniform::zeroed(), variant_row(8.0, [0.0, 0.0], 1.0, 0)];
        assert!(assign_bands(&mut rows, size, MIN_BAND_FOOTPRINT).is_empty());
        assert_eq!(rows[1].cell_row, 0);
        let bands = assign_bands(&mut rows, size, MIN_BAND_FOOTPRINT_BOUNDED);
        assert_eq!(bands.len(), 1);
        assert_eq!(rows[1].cell_row, 1);
    }

    #[test]
    fn a_bands_drawn_cells_hold_every_cell_whose_lattice_meets_the_rect() {
        // Every cell a pass would write anything into: one of its 16 lattice
        // samples (ascii_variant_cells.wgsl) is a pixel of the rect
        let size = [1280u32, 720u32];
        for (cell, origin, footprint) in [
            (8.0, grid_origin(size, [8.0, 8.0]), 1.0),
            (8.0, grid_origin(size, [8.0, 8.0]), 2.0),
            (12.0, [3.0, 5.0], 1.0),
            (10.0, [-4.5, 7.25], 3.5),
        ] {
            let grid = cell_grid(size, [cell; 2], origin);
            let band = Band {
                cell: [cell; 2],
                origin,
                footprint,
                row: 40,
                rows: grid[1],
                cols: grid[0],
            };
            let rects = [
                kit::PixelRect { x: 0, y: 0, width: 7, height: 5 },
                kit::PixelRect { x: 300, y: 200, width: 41, height: 63 },
                kit::PixelRect { x: 1270, y: 715, width: 10, height: 5 },
                kit::PixelRect::full(size[0], size[1]),
            ];
            for rect in rects {
                let drawn = band_cells_in(&band, rect);
                assert!(drawn.x + drawn.width <= band.cols);
                assert!(drawn.y >= band.row && drawn.y + drawn.height <= band.row + band.rows);
                let inside = |t: u32, lo: u32, n: u32| t >= lo && t < lo + n;
                for ty in 0..band.rows {
                    for tx in 0..band.cols {
                        let c = [tx as f32 - 1.0, ty as f32 - 1.0];
                        let centre = [
                            c[0] * cell + origin[0] + 0.5 * cell,
                            c[1] * cell + origin[1] + 0.5 * cell,
                        ];
                        let foot = cell * footprint;
                        let meets = (0..16).any(|k| {
                            let q = [
                                (centre[0] + (((k & 3) as f32 + 0.5) / 4.0 - 0.5) * foot).floor(),
                                (centre[1] + (((k >> 2) as f32 + 0.5) / 4.0 - 0.5) * foot).floor(),
                            ];
                            q[0] >= rect.x as f32
                                && q[0] < (rect.x + rect.width) as f32
                                && q[1] >= rect.y as f32
                                && q[1] < (rect.y + rect.height) as f32
                        });
                        if meets {
                            assert!(
                                inside(tx, drawn.x, drawn.width)
                                    && inside(band.row + ty, drawn.y, drawn.height),
                                "cell {c:?} of grid {cell} @ {origin:?} x{footprint} meets \
                                 {rect:?} but {drawn:?} leaves it out"
                            );
                        }
                    }
                }
            }
            assert!(band_cells_in(&band, kit::PixelRect::EMPTY).is_empty());
        }
    }

    #[test]
    fn grids_past_the_band_limit_sample_for_themselves() {
        let mut rows = vec![VariantUniform::zeroed()];
        for i in 0..MAX_BANDS + 2 {
            rows.push(variant_row(8.0 + i as f32, [0.0, 0.0], 2.0, 0));
        }
        let bands = assign_bands(&mut rows, [640, 360], MIN_BAND_FOOTPRINT);
        assert_eq!(bands.len(), MAX_BANDS);
        assert!(rows[1..=MAX_BANDS].iter().all(|r| r.cell_row > 0));
        assert!(rows[MAX_BANDS + 1..].iter().all(|r| r.cell_row == 0));
        // 2 px cells at 4K are 1081 rows a band: the eighth won't fit
        let mut tall = vec![VariantUniform::zeroed()];
        for footprint in 2..=MAX_BANDS + 1 {
            tall.push(variant_row(2.0, [0.0, 0.0], footprint as f32, 0));
        }
        let bands = assign_bands(&mut tall, [3840, 2160], MIN_BAND_FOOTPRINT);
        let [w, h] = band_texture_size(&bands, None);
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

    /// The glyph pass's arms-to-corner table (#348) is `EdgeGlyphs::for_arms`,
    /// slot for slot, and the edge view's inverse table undoes it.
    #[test]
    fn the_shaders_corner_table_matches_for_arms() {
        let table = |src: &str, name: &str| -> Vec<String> {
            let start = src.find(&format!("const {name} = array<u32,")).expect(name);
            let body = &src[start..];
            let body = &body[body.find('(').unwrap() + 1..body.find(");").unwrap()];
            body.lines()
                .map(|l| l.split("//").next().unwrap())
                .flat_map(|l| l.split(','))
                .map(|t| t.trim().trim_end_matches('u').to_string())
                .filter(|t| !t.is_empty())
                .collect()
        };
        let arms = table(include_str!("ascii_edge_corners.wgsl"), "ARM_GLYPHS");
        assert_eq!(arms.len(), 16);
        let edges = GlyphSet::builtin().edges();
        let slots = edges.slots();
        assert_eq!(slots.len(), EDGE_SLOTS);
        for (mask, entry) in arms.iter().enumerate() {
            let want = edges.for_arms(mask & 1 != 0, mask & 2 != 0, mask & 4 != 0, mask & 8 != 0);
            let got = (entry != "NO_EDGE").then(|| slots[entry.parse::<usize>().unwrap()]);
            assert_eq!(got, want, "arms {mask:04b}");
        }
        let back = table(include_str!("ascii_edge_view.wgsl"), "SLOT_ARMS");
        for (i, mask) in back.iter().enumerate() {
            assert_eq!(arms[mask.parse::<usize>().unwrap()], (i + 4).to_string());
        }
    }

    #[test]
    fn a_fonts_ramp_and_edges_pack_together_or_not_at_all() {
        let set = GlyphSet::builtin();
        let mut ramps = Ramps::default();
        let (ramp, edges) = ramps.place_with_edges(1, &set, "classic").unwrap();
        assert_eq!(ramp.0, 0);
        assert_eq!(edges, ramp.1, "the edges follow the ramp");
        let atlas = set.atlas();
        let packed: Vec<u32> = ramps.steps[edges as usize..edges as usize + EDGE_SLOTS]
            .iter()
            .map(|(_, i)| *i)
            .collect();
        let want: Vec<u32> = set
            .edges()
            .slots()
            .iter()
            .map(|c| atlas.index_of(*c).unwrap())
            .collect();
        assert_eq!(packed, want, "in EdgeGlyphs::slots() order");
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
        let slots = set.edges().slots().len();
        assert_eq!(
            slots, EDGE_SLOTS,
            "the built-in set has every corner (#348)"
        );
        ramps.steps.resize(MAX_RAMP - blocks - slots + 1, (0, 0));
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
