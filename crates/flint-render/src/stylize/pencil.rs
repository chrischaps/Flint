//! Pencil sketch (#273): the scene drawn in graphite on paper. Tone
//! becomes layers of cross-hatching, the scene's depth and creases become
//! wobbling contour lines, and both sit on a paper with grain.
//!
//! ```toml
//! [post_process.styles.pencil]
//! spacing = 6.0          # hatch line spacing, points
//! layers = 3             # hatch layers (0-4); each joins as the tone darkens
//! direction = "surface"  # surface | screen | tensor
//! angle = 50.0           # the base angle, degrees
//! contour = 1.0          # outline strength
//! grazing = 0.0          # 1 spares planes seen edge-on from depth lines (#404)
//! clutter = 0.0          # 1 drops lines in a field of small silhouettes (#883)
//! tone = "picture"       # picture | light: hatch the darks, or the shadows (#869)
//! wobble = 1.5           # how far lines wander, points
//! sketchy = 0.6          # a second, offset outline
//! grain = 0.5            # paper grain, and how much it breaks strokes
//! paper = [0.95, 0.93, 0.88]   # a 3-array fills paper_r/g/b
//! lead = [0.16, 0.16, 0.19]
//! color = 0.0            # 1 draws over the scene's colour instead of paper
//! seed = 0               # a different hand; the same seed draws the same picture
//! boil = 0.0             # redraws a second (0 = still)
//! paper_image = "papers/cold_press.png"  # a scanned paper (#390); "none" = procedural
//! paper_scale = 1.0      # points a texel of it covers
//! stroke_image = "graphite"   # strokes/graphite.png: a strip of real strokes
//! stroke_rows = 4        # strokes stacked in the strip, one picked a dash
//! ```
//!
//! It runs at the display stage, on the finished picture at output
//! resolution, so strokes stay crisp under a render scale; sizes are in
//! points (times [`StyleFrame::pixels_per_point`]). Two passes: the first
//! writes each pixel's tone, contour strength and local stroke direction
//! into a scratch target; the second blurs the direction into a flow and
//! draws strokes, outlines and paper from it. While a variant hatches by
//! `tensor`, two more passes between them (`pencil_flow.wgsl`, #392) blur
//! its direction wide, separably and at half resolution, each pixel
//! weighted by its contour strength; the second pass then reads that.
//!
//! Stroke direction (`direction`): `surface` runs strokes along the
//! surface's iso-depth lines (round a barrel, across a floor), taken from
//! the depth buffer; `tensor` runs them along the picture's luminance
//! edges (its structure tensor); `screen` hatches every surface at
//! `angle`. Where a surface or tensor direction is unsure (a face turned
//! to the camera, a flat colour), `angle` shows through.
//!
//! Output is a pure function of the picture, the depth, the settings and
//! `seed` (with `boil`, also the post clock, which `--style-time` sets):
//! every random choice is an integer hash.
//!
//! ## Images (#390)
//!
//! `paper_image` names a paper image beside the scene (as glyph sets are
//! found: the scene's folder, then its parent; a bare name also tries
//! `papers/<name>.png`), tiled in points from the screen's corner. Its
//! luma, measured against its own mean and spread, is the paper's height:
//! the tone under the strokes and the tooth that breaks them, as much as
//! `grain` says; its hue tints the `paper` colour. `stroke_image` names a
//! strip of `stroke_rows` strokes stacked top to bottom, each drawn left
//! to right (`strokes/<name>.png` for a bare name); each dash is one of
//! them, stretched along the dash, its ink the strip's dark or opaque
//! texels. Both are read through [`images`](super::images) with no
//! sampler; with none named the procedural paper and dashes draw exactly
//! as before. Each entity may name its own (#547): the papers a frame
//! uses are stacked as layers of one texture array, and the strips as
//! another, up to [`MAX_LAYERS`] of each, each row carrying the layer it
//! reads; a row naming one more draws procedurally (warned once, naming
//! its entity, #740).
//!
//! ## Tone from the light (#869)
//!
//! The picture's luminance is albedo times light, so with `tone =
//! "picture"` (the default) a dark material is hatched as if it were in
//! shadow. `tone = "light"` divides the picture by the style albedo
//! ([`StyleFrame::albedo`]: each opaque mesh's own colour, which the
//! renderer draws after the scene pass only while a row asks for it), both
//! in display values, so a brown floor in the sun reads as lit and a white
//! prop in shadow as dark: the hatching follows where the light falls off.
//! Where no mesh was drawn (terrain, grass, water) or the paint is nearly
//! black, the picture's own luminance stands. The sky (depth cleared to 1)
//! is paper (#890): nothing there for the light to fall on, so a dark sky
//! is left blank as a traced drawing leaves it; `tone = "picture"` still
//! hatches one.
//!
//! ## Per entity
//!
//! As ASCII (#266): an entity with a `pencil_style` component, or a
//! `style_mask { style = "pencil" }`, is drawn in pencil while the rest of
//! the frame passes through, and each distinct set of settings is a row of
//! the variant table the passes index by mask id. `scope = "entities"`
//! keeps a scene table as those entities' preset, and `grid = "object"`
//! pins an entity's hatching to its projected origin, so its strokes
//! travel with it rather than swimming over it.
//!
//! ## On the surfaces
//!
//! `grid = "world"` (#389) lays the hatching on the surfaces: each pixel's
//! world position, rebuilt from the depth, is hatched on the three axis
//! planes (triplanar, weighted by the normal), so as the camera moves the
//! strokes stay on the props instead of the props sliding under them.
//! Spacing comes in octaves of metres (2^k), the one nearest `spacing` on
//! screen cross-faded with the next as a mip chain does, so strokes keep
//! their size on screen at any distance; `direction` is not used (the sky
//! keeps the screen's hatching), `angle` turns the strokes in each plane.
//!
//! World strokes stay put while the camera moves, so a prop moving through
//! the world (a rolling cask) slides under them. `grid = "local"` (#544)
//! hatches the same stripes on the styled root's own position: its row
//! carries the root's world-to-local matrix (the held one under a motion
//! hold), and the draw pass takes the world position, its derivatives and
//! the normal into that space first. A rigid prop's strokes ride on it
//! exactly, its children's too while they move with it; a skinned mesh's
//! surfaces move within that frame, so its strokes still swim (that would
//! need a rest-pose position from the scene pass). The whole frame's row
//! has no root, so a table's `local` is `world`.

use crate::gpu_counts::{CountedDevice, CountedQueue};
pub use super::images::{names_image, NO_IMAGE};
use super::images::{ImageStack, ImageUse, LayerInfo, Stacked, StackedImages, MAX_LAYERS};
use super::kit;
use super::variant_table::{report_drop, storage_entry, VariantTable};
use super::{
    StyleDescriptor, StyleDevice, StyleFrame, StyleInput, StyleOption, StyleParam,
    StylePass, StyleStage,
};
use bytemuck::{Pod, Zeroable};
use std::sync::{Arc, Mutex};

/// The stroke direction modes, by their option spelling.
pub const DIRECTIONS: &[&str] = &["surface", "screen", "tensor"];

/// What a `[post_process.styles.pencil]` table covers (as ASCII's).
pub const SCOPES: &[&str] = &["screen", "entities"];

/// Where a variant's hatching is anchored: ASCII's two grids, `world`,
/// strokes laid on the surfaces themselves (#389), and `local`, laid on
/// them in the entity's own space, so they ride on a moving prop (#544).
pub const GRIDS: &[&str] = &["screen", "object", "world", "local"];

/// Where the tone comes from (#869): the picture's luminance, or the light
/// alone (the picture over its albedo).
pub const TONES: &[&str] = &["picture", "light"];

/// How a blend below 1 shows the drawing (#879): a cross-fade of every
/// pixel, or drawn on, outlines first, then hatching, then the paper.
pub const TRANSITIONS: &[&str] = &["fade", "draw"];

/// The scratch target pass 1 writes: tone, contour, flow.
pub const PREP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The tensor flow blur's two targets (#392): the blurred double angle.
pub const FLOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;

/// The direction mode the flow blur runs for.
const TENSOR: f32 = 2.0;

const PARAMS: &[StyleParam] = &[
    StyleParam::new("spacing", 6.0, 2.0..=24.0)
        .label("Spacing (pt)")
        .help("Hatch line spacing in points"),
    StyleParam::new("thickness", 1.0, 0.25..=4.0)
        .label("Stroke width (pt)")
        .help("Hatch stroke width in points"),
    StyleParam::new("layers", 3.0, 0.0..=4.0)
        .label("Hatch layers")
        .help("Up to four; each joins, crossing the last, as the tone darkens")
        .integer(),
    StyleParam::new("angle", 50.0, 0.0..=180.0)
        .label("Angle (deg)")
        .help("The base hatch angle; where surface or tensor directions are unsure it shows through"),
    StyleParam::new("brightness", 0.0, -1.0..=1.0)
        .label("Brightness")
        .help("Shifts the tone before it picks hatch layers"),
    StyleParam::new("contrast", 1.2, 0.0..=4.0)
        .label("Contrast")
        .help("Spreads the tone around mid-grey before it picks hatch layers"),
    StyleParam::new("darkness", 0.85, 0.0..=1.0)
        .label("Stroke darkness")
        .help("How dark one hatch stroke is"),
    StyleParam::new("shade", 0.25, 0.0..=1.0)
        .label("Rubbed shade")
        .help("Soft graphite under the strokes, in the darks"),
    StyleParam::new("contour", 1.0, 0.0..=2.0).label("Outline").help("Outline strength; 0 = none"),
    StyleParam::new("contour_width", 1.25, 0.5..=4.0)
        .label("Outline width (pt)")
        .help("Outline width in points"),
    StyleParam::new("depth", 1.0, 0.0..=4.0)
        .label("Silhouettes")
        .help("Outline sensitivity to depth steps"),
    StyleParam::new("grazing", 0.0, 0.0..=1.0)
        .label("Spare grazing planes")
        .help("1 draws a silhouette only at a true step, so a floor seen edge-on doesn't ink the horizon in a band; 0 = every depth step"),
    StyleParam::new("clutter", 0.0, 0.0..=1.0)
        .label("Spare clutter")
        .help("1 drops the lines of a field of small silhouettes (a meadow's blades) where they would merge into a black fill, keeping near blades and true outlines; 0 = every line"),
    StyleParam::new("crease", 1.0, 0.0..=4.0)
        .label("Creases")
        .help("Outline sensitivity to where the surface turns"),
    StyleParam::new("detail", 0.35, 0.0..=2.0)
        .label("Texture lines")
        .help("Outlines from the picture's own luminance edges"),
    StyleParam::new("wobble", 1.5, 0.0..=8.0)
        .label("Wobble (pt)")
        .help("How far lines wander off true"),
    StyleParam::new("sketchy", 0.6, 0.0..=1.0)
        .label("Sketchy")
        .help("A second, offset outline beside the first"),
    StyleParam::new("grain", 0.5, 0.0..=1.0)
        .label("Paper grain")
        .help("Paper grain, and how much its tooth breaks the strokes"),
    StyleParam::new("color", 0.0, 0.0..=1.0)
        .label("Colour")
        .help("0 draws on paper; 1 draws over the scene's own colour (coloured pencil)"),
    StyleParam::new("seed", 0.0, 0.0..=9999.0)
        .label("Seed")
        .help("Another whole number is another hand; the same seed draws the same picture")
        .integer(),
    StyleParam::new("boil", 0.0, 0.0..=24.0)
        .label("Boil (per s)")
        .help("Redraws a second, for animated lines; 0 = still"),
    StyleParam::new("paper_r", 0.95, 0.0..=1.0)
        .label("Paper")
        .help("Paper colour (display values)"),
    StyleParam::new("paper_g", 0.93, 0.0..=1.0),
    StyleParam::new("paper_b", 0.88, 0.0..=1.0),
    StyleParam::new("lead_r", 0.16, 0.0..=1.0)
        .label("Lead")
        .help("Graphite colour (display values)"),
    StyleParam::new("lead_g", 0.16, 0.0..=1.0),
    StyleParam::new("lead_b", 0.19, 0.0..=1.0),
    StyleParam::new("paper_scale", 1.0, 0.25..=8.0)
        .label("Paper scale (pt)")
        .help("Points one texel of the paper image covers"),
    StyleParam::new("stroke_rows", 1.0, 1.0..=16.0)
        .label("Strokes in strip")
        .help("How many strokes the strip stacks, top to bottom")
        .integer(),
];

/// The papers to offer: none, then every paper the scene can see
/// (`papers/`, by the project's search).
pub(super) fn paper_choices(scope: &super::ChoiceScope<'_>) -> Vec<String> {
    super::choices_with(
        &[NO_IMAGE],
        scope.files(flint_core::project::AssetKind::Papers),
    )
}

/// The stroke strips to offer: none, then every one in a `strokes/`.
fn stroke_choices(scope: &super::ChoiceScope<'_>) -> Vec<String> {
    super::choices_with(
        &[NO_IMAGE],
        scope.files(flint_core::project::AssetKind::Strokes),
    )
}

const OPTIONS: &[StyleOption] = &[
    StyleOption::new("direction", "surface", DIRECTIONS)
        .label("Stroke direction")
        .help("surface: round the form, along iso-depth lines; screen: all at the angle; tensor: along the picture's edges"),
    StyleOption::new("scope", "screen", SCOPES)
        .label("Scope")
        .help("screen: the table draws the whole frame; entities: it is only the preset for pencil_style entities"),
    StyleOption::new("grid", "screen", GRIDS)
        .label("Hatch anchor")
        .help("screen: hatching fixed to the screen; object: pinned to each entity's origin, so strokes travel with it; world: laid on the surfaces, so they hold as the camera moves; local: laid on each entity's surfaces in its own space, so they ride on it as it moves and turns too"),
    StyleOption::new("tone", "picture", TONES)
        .label("Tone from")
        .help("picture: hatch where the picture is dark, dark paint included; light: hatch where the light falls off (the picture over each mesh's own colour), so a dark floor in the sun stays blank and a white prop in shadow is hatched"),
    StyleOption::new("transition", "fade", TRANSITIONS)
        .label("Transition")
        .help("How a blend below 1 shows the drawing: fade cross-fades every pixel, faint all over; draw draws it on, the outlines growing along their length up to the blend, the hatching a stroke at a time after them, the paper's wash last"),
    StyleOption::new("paper_image", NO_IMAGE, &[])
        .discovered(paper_choices)
        .label("Paper image")
        .help("A scanned paper (papers/<name>.png): its relief is the tooth, its hue tints the paper; none = procedural"),
    StyleOption::new("stroke_image", NO_IMAGE, &[])
        .discovered(stroke_choices)
        .label("Stroke image")
        .help("A strip of real strokes (strokes/<name>.png), one stretched along each dash; none = drawn bars"),
];

pub(super) static STYLE: StyleDescriptor =
    StyleDescriptor::new("pencil", "Pencil", StyleStage::Display, create)
        .params(PARAMS)
        .options(OPTIONS)
        .uses_albedo(super::MaskUse::When(reads_albedo))
        // The contour's depth steps and creases (#743)
        .needs(&[super::StyleNeed::Edges])
        .draws_blend(draws_on)
        .component("pencil_style");

/// Whether pencil draws this blend on itself (#879): the whole frame's
/// (`None`) when its table says `transition = "draw"`, an entity variant's
/// when the variant does (its own, else the table's). The stage's mix and
/// the cross-fade leave those blends to the draw pass.
fn draws_on(
    config: &crate::postprocess::PostProcessConfig,
    variant: Option<&crate::style_mask::StyleVariant>,
) -> bool {
    let transition = match variant {
        None => STYLE.option(config, "transition"),
        Some(v) => v.option(&STYLE, config, "transition"),
    };
    transition == "draw"
}

/// The transition's shader number (#879): 0 fade, 1 draw; an unknown name
/// is `fade`.
pub fn transition_mode(name: &str) -> f32 {
    TRANSITIONS.iter().position(|m| *m == name).unwrap_or(0) as f32
}

/// Whether this frame draws with the draw-on pass (#879): the whole frame's
/// front is below 1, or an entity row draws on while the mask may hold a
/// blend below 1 (`mask_fades`). Otherwise every pixel is whole and the
/// plain pass draws it, as it did before the transition existed.
pub fn draws_on_now(rows: &[PencilRow], front: f32, mask_fades: bool) -> bool {
    front < 1.0
        || (mask_fades
            && rows
                .iter()
                .skip(1)
                .any(|r| r.v[8][2] > 0.5 && (r.v[6][2] as u32) & 1 == 0))
}

/// The whole frame's draw front (#879): the table's blend while the table
/// draws on, else 1, when the stage mixes the blend instead.
pub fn whole_frame_front(config: &crate::postprocess::PostProcessConfig) -> f32 {
    if draws_on(config, None) {
        config.styles.blend(STYLE.name)
    } else {
        1.0
    }
}

/// Whether some row this frame takes its tone from the light (#869): the
/// scene's table over the frame, or an entity's variant, so the renderer
/// draws the style albedo only then.
fn reads_albedo(
    config: &crate::postprocess::PostProcessConfig,
    ids: &crate::style_mask::StyleIds,
) -> bool {
    (STYLE.covers_frame(config) && STYLE.option(config, "tone") == "light")
        || ids
            .variants_of(STYLE.name)
            .any(|(_, v)| !v.off && v.option(&STYLE, config, "tone") == "light")
}

fn create(d: &StyleDevice) -> Box<dyn StylePass> {
    let device = d.device;
    // The uniform, then the variant table's rows and slots (#740)
    let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Pencil Uniform BGL"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            storage_entry(1),
            storage_entry(2),
        ],
    });
    let input_bgl = kit::sampled_layout(device, "Pencil Input BGL");
    let prep_bgl = prep_layout(device);
    let geometry_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Pencil Geometry BGL"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
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
            // The paper and stroke stacks (#547)
            image_stack_entry(2),
            image_stack_entry(3),
            // The style albedo (#869), or the blank stand-in
            image_entry(4),
            // The shared edges (#743), or the blank stand-in
            image_entry(5),
        ],
    });
    // Bound while no entity asks for the style: one unstyled pixel
    let no_mask = device.create_texture_counted(&wgpu::TextureDescriptor {
        label: Some("Pencil No Mask"),
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
    // Bound while no row reads the light: one texel where no mesh was drawn
    let no_albedo = device.create_texture_counted(&wgpu::TextureDescriptor {
        label: Some("Pencil No Albedo"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: crate::style_albedo::ALBEDO_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let no_albedo_view = no_albedo.create_view(&Default::default());
    // Bound if the shared edge pass couldn't be built: one texel of no edge
    let no_edges = device.create_texture_counted(&wgpu::TextureDescriptor {
        label: Some("Pencil No Edges"),
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
    let blit = kit::Blit::new(device, "Pencil Blit", &input_bgl, d.format);
    let flow_shader = kit::shader(device, "Pencil Flow Shader", FLOW_SHADER);
    let flow_pipeline = |label: &str, entry: &str| {
        kit::fullscreen_pipeline_entry(
            device,
            label,
            &flow_shader,
            entry,
            &[&uniform_bgl, &input_bgl, &geometry_bgl],
            FLOW_FORMAT,
        )
    };
    Box::new(Pencil {
        flow_across: flow_pipeline("Pencil Flow Across Pipeline", "fs_across"),
        flow_down: flow_pipeline("Pencil Flow Down Pipeline", "fs_down"),
        prep_pipeline: kit::fullscreen_pipeline(
            device,
            "Pencil Prep Pipeline",
            &kit::shader(device, "Pencil Prep Shader", PREP_SHADER),
            &[&uniform_bgl, &input_bgl, &geometry_bgl],
            PREP_FORMAT,
        ),
        draw_pipeline: kit::fullscreen_pipeline(
            device,
            "Pencil Pipeline",
            &kit::shader(device, "Pencil Shader", DRAW_SHADER),
            &[&uniform_bgl, &input_bgl, &geometry_bgl, &prep_bgl],
            d.format,
        ),
        draw_on_pipeline: kit::fullscreen_pipeline(
            device,
            "Pencil Draw On Pipeline",
            &kit::shader(device, "Pencil Draw On Shader", DRAW_ON_SHADER),
            &[&uniform_bgl, &input_bgl, &geometry_bgl, &prep_bgl],
            d.format,
        ),
        uniforms: kit::uniform_buffer::<PencilUniforms>(device, "Pencil Uniforms"),
        uniform_bgl,
        input_bgl,
        prep_bgl,
        geometry_bgl,
        _no_mask: no_mask,
        no_mask_view,
        _no_albedo: no_albedo,
        no_albedo_view,
        _no_edges: no_edges,
        no_edges_view,
        blit,
        table: Mutex::new(VariantTable::new(
            device,
            "Pencil",
            std::mem::size_of::<PencilRow>(),
        )),
        images: Mutex::new(Images {
            paper: ImageStack::new(ImageUse::Paper, "pencil paper"),
            stroke: ImageStack::new(ImageUse::Stroke, "pencil stroke"),
        }),
    })
}

/// A stack of style images' binding (#547, [`StackedImages`]): a
/// `texture_2d_array` read with `textureLoad`, no sampler.
pub(super) fn image_stack_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        count: None,
    }
}

/// A texture read with `textureLoad`, no sampler.
pub(super) fn image_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

const PREP_SHADER: kit::Wgsl = kit::wgsl!(
    "prelude_core.wgsl",
    "edges_read.wgsl",
    "style_image_layer.wgsl",
    "mask_read.wgsl",
    "mask_slot.wgsl",
    "pencil_common.wgsl",
    "pencil_prep.wgsl"
);
const DRAW_SHADER: kit::Wgsl = kit::wgsl!(
    "prelude_core.wgsl",
    "pencil_fade.wgsl",
    "style_image_layer.wgsl",
    "noise.wgsl",
    "mask_read.wgsl",
    "mask_slot.wgsl",
    "pencil_common.wgsl",
    "style_image.wgsl",
    "pencil.wgsl"
);
/// The drawing pass with the draw-on code in (#879): its own module, so the
/// one every other frame draws with compiles exactly as before (a draw-on
/// path in the same function moved pencil-full's pixels by a level, the
/// driver reassociating the strokes' products round it).
const DRAW_ON_SHADER: kit::Wgsl = kit::wgsl!(
    "prelude_core.wgsl",
    "pencil_draw_on.wgsl",
    "style_image_layer.wgsl",
    "noise.wgsl",
    "mask_read.wgsl",
    "mask_slot.wgsl",
    "pencil_common.wgsl",
    "style_image.wgsl",
    "pencil.wgsl"
);
const FLOW_SHADER: kit::Wgsl = kit::wgsl!(
    "prelude_core.wgsl",
    "style_image_layer.wgsl",
    "mask_read.wgsl",
    "mask_slot.wgsl",
    "pencil_common.wgsl",
    "pencil_flow.wgsl"
);

/// The drawing pass's group 3: the prep target and its sampler, and the
/// blurred tensor flow (#392; the prep again while no row hatches by
/// tensor, which the shader then never reads).
fn prep_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let texture = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Pencil Prep BGL"),
        entries: &[
            texture(0),
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            texture(2),
        ],
    })
}

/// Whether any row in use hatches along the picture's edges, so the flow
/// blur has to run. A row with no hatch layers draws no strokes (#404).
pub fn any_tensor(rows: &[PencilRow]) -> bool {
    rows.iter().any(|r| {
        r.v[6][2] != PASS_THROUGH && r.v[4][3] == TENSOR && (r.v[0][2] + 0.5) as u32 > 0
    })
}

/// One variant's settings, thirteen vec4s (see `PencilRow` in the shader):
/// ten of settings, then the world-to-local rows of a `grid = "local"`
/// variant (#544; the identity otherwise).
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug, PartialEq)]
pub struct PencilRow {
    pub v: [[f32; 4]; 13],
}

impl PencilRow {
    /// Hatch on the surfaces in the space `rows` take a world point into
    /// (#544): the world-to-local rows of [`LocalFrame::inverse_rows`].
    /// Only a world-grid row takes it, so a local grid is a world one plus
    /// the frame.
    ///
    /// [`LocalFrame::inverse_rows`]: crate::style_mask::LocalFrame::inverse_rows
    pub fn set_local(&mut self, rows: [[f32; 4]; 3]) {
        let flags = self.v[6][2] as u32;
        if flags & WORLD_GRID as u32 == 0 {
            return;
        }
        if flags & LOCAL_GRID as u32 == 0 {
            self.v[6][2] += LOCAL_GRID;
        }
        self.v[10..13].copy_from_slice(&rows);
    }
}

/// The rows a row's local frame holds while it has none: the identity.
const NO_LOCAL: [[f32; 4]; 3] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

/// The paper and stroke images, kept with the style (#390), a stack of
/// each a frame (#547).
struct Images {
    paper: ImageStack,
    stroke: ImageStack,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct PencilUniforms {
    inv_view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 3],
    pixels_per_point: f32,
    size: [f32; 2],
    srgb: f32,
    /// The whole frame's draw front (#879, [`whole_frame_front`])
    front: f32,
    forward: [f32; 3],
    _pad1: f32,
    info: [u32; 4],
    /// The shared edges' thresholds (#743): `edge_depth`, `edge_normal`
    edge: [f32; 4],
    /// Each layer of the paper and stroke stacks (#547): its measures and size
    papers: [LayerInfo; MAX_LAYERS],
    strokes: [LayerInfo; MAX_LAYERS],
}

/// A row's flag: leave its pixels as they are.
const PASS_THROUGH: f32 = 1.0;

/// A row's flag (added to the others): its hatching is on the world's
/// surfaces, `grid = "world"` (#389).
const WORLD_GRID: f32 = 2.0;

/// A row's flag (with [`WORLD_GRID`]): the world strokes are laid in the
/// styled root's own space, `grid = "local"` (#544), through the row's
/// last three vec4s.
const LOCAL_GRID: f32 = 4.0;

/// The slot of a mask id another registered style owns: its pixels are
/// that style's (an ASCII prop in a pencil frame), so pencil leaves them.
/// No row's index: the table grows past 255 rows (#740).
pub const FOREIGN: u32 = u32::MAX;

/// The camera's forward axis from its inverse view-projection: from the
/// centre of the near plane to the centre of the far one.
pub fn forward_axis(inv_view_proj: &[[f32; 4]; 4]) -> [f32; 3] {
    let at = |z: f32| {
        let m = inv_view_proj;
        let row = |r: usize| m[2][r] * z + m[3][r];
        let w = row(3);
        [row(0) / w, row(1) / w, row(2) / w]
    };
    let (near, far) = (at(0.0), at(1.0));
    let d = [far[0] - near[0], far[1] - near[1], far[2] - near[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if len.is_finite() && len > 0.0 {
        d.map(|c| c / len)
    } else {
        [0.0, 0.0, -1.0]
    }
}

/// The direction mode's shader number; an unknown name is `surface`.
pub fn direction_mode(name: &str) -> f32 {
    DIRECTIONS.iter().position(|m| *m == name).unwrap_or(0) as f32
}

/// The tone mode's shader number (#869): 0 picture, 1 light; an unknown
/// name is `picture`.
pub fn tone_mode(name: &str) -> f32 {
    TONES.iter().position(|m| *m == name).unwrap_or(0) as f32
}

/// The seed a frame draws with: `seed`, moved on `boil` times a second of
/// post time, so a still drawing (boil 0) never changes and a boiling one
/// changes in steps, the same steps at the same `--style-time`. The steps
/// are [`motion_hold::tick`](crate::motion_hold::tick)'s, so a pose held at
/// `rate = boil` moves in the same frames the lines redraw (#405), and that
/// is the stepped clock with no glide ([`flint_core::step_clock`], #747), the
/// one a living prop holds and glides on.
pub fn effective_seed(seed: f32, boil: f32, time: f32) -> f32 {
    let base = seed.max(0.0).floor();
    match crate::motion_hold::tick(time, boil) {
        Some(step) => base + step as f32,
        None => base,
    }
}

/// A variant's row from its getters.
pub fn row(
    param: &dyn Fn(&str) -> f32,
    option: &dyn Fn(&str) -> String,
    time: f32,
    origin: [f32; 2],
    flags: f32,
) -> PencilRow {
    PencilRow {
        v: [
            [
                param("spacing").max(0.5),
                param("thickness").max(0.05),
                param("layers").clamp(0.0, 4.0),
                param("angle").to_radians(),
            ],
            [
                param("brightness"),
                param("contrast").max(0.0),
                param("contour").max(0.0),
                param("contour_width").max(0.1),
            ],
            [
                param("depth").max(0.0),
                param("crease").max(0.0),
                param("detail").max(0.0),
                param("wobble").max(0.0),
            ],
            [
                param("sketchy").clamp(0.0, 1.0),
                param("grain").clamp(0.0, 1.0),
                param("color").clamp(0.0, 1.0),
                effective_seed(param("seed"), param("boil"), time),
            ],
            [
                param("paper_r"),
                param("paper_g"),
                param("paper_b"),
                direction_mode(&option("direction")),
            ],
            [
                param("lead_r"),
                param("lead_g"),
                param("lead_b"),
                param("shade").clamp(0.0, 1.0),
            ],
            [
                origin[0],
                origin[1],
                // A local grid is a world one until its frame is set
                flags
                    + if matches!(option("grid").as_str(), "world" | "local") {
                        WORLD_GRID
                    } else {
                        0.0
                    },
                param("darkness").clamp(0.0, 1.0),
            ],
            [
                param("paper_scale").clamp(0.05, 64.0),
                param("stroke_rows").clamp(1.0, 64.0).round(),
                names_image(&option("paper_image")) as u32 as f32,
                names_image(&option("stroke_image")) as u32 as f32,
            ],
            [
                param("grazing").clamp(0.0, 1.0),
                tone_mode(&option("tone")),
                transition_mode(&option("transition")),
                // The draw order's seed holds still while the lines boil
                param("seed").max(0.0).floor(),
            ],
            [param("clutter").clamp(0.0, 1.0), 0.0, 0.0, 0.0],
            NO_LOCAL[0],
            NO_LOCAL[1],
            NO_LOCAL[2],
        ],
    }
}

/// How far, in pixels, a styled pixel's draw reads the prep texture
/// (#385), for the rows `rows`: the surface flow's 5x5 taps over
/// `1.5 * spacing` points, the tensor flow's blur (#392, `pencil_flow.wgsl`)
/// over `3 * spacing` along each axis, or a contour's wobble
/// (`1.5 * wobble`) plus half its width, or with `clutter` (#883) the wide
/// disc it measures (three times `clutter_radius` in `pencil.wgsl`: six
/// contour widths, at least nine pixels); and four pixels for the bilinear
/// taps' footprint and the flow's half-resolution texels.
/// Where the prep pass draws (#385): per entity, their rect grown by how
/// far a styled pixel reads the prep; `None` for the whole frame.
fn prep_rect(
    frame: &StyleFrame<'_>,
    rows: &[PencilRow],
    pixels_per_point: f32,
) -> Option<kit::PixelRect> {
    frame.styled_rect(0).map(|r| {
        r.padded(
            prep_reach(rows, pixels_per_point),
            frame.width,
            frame.height,
        )
    })
}

fn prep_reach(rows: &[PencilRow], pixels_per_point: f32) -> u32 {
    let pixels = rows
        .iter()
        .map(|r| {
            let [spacing, ..] = r.v[0];
            let contour_width = r.v[1][3];
            let wobble = r.v[2][3];
            let flow = if r.v[4][3] == TENSOR { 3.0 } else { 1.5 };
            let points = (flow * spacing).max(1.5 * wobble + 0.5 * contour_width.max(1.0));
            let clutter = if r.v[9][0] > 0.0 {
                3.0 * (2.0 * contour_width * pixels_per_point).max(3.0)
            } else {
                0.0
            };
            (points * pixels_per_point).max(clutter)
        })
        .fold(0.0f32, f32::max);
    pixels.ceil() as u32 + 4
}

struct Pencil {
    prep_pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    /// The tensor flow blur along x, from the prep target (#392)
    flow_across: std::sync::Arc<wgpu::RenderPipeline>,
    /// And along y, from the across pass's result
    flow_down: std::sync::Arc<wgpu::RenderPipeline>,
    draw_pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    /// The drawing pass while some row draws on (#879, [`DRAW_ON_SHADER`])
    draw_on_pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    uniforms: wgpu::Buffer,
    uniform_bgl: wgpu::BindGroupLayout,
    input_bgl: wgpu::BindGroupLayout,
    prep_bgl: wgpu::BindGroupLayout,
    geometry_bgl: wgpu::BindGroupLayout,
    _no_mask: wgpu::Texture,
    no_mask_view: wgpu::TextureView,
    _no_albedo: wgpu::Texture,
    no_albedo_view: wgpu::TextureView,
    _no_edges: wgpu::Texture,
    no_edges_view: wgpu::TextureView,
    /// The input through, outside the styled entities' rect (#385).
    blit: kit::Blit,
    /// The variant table (#740): grows with the frame.
    table: Mutex<VariantTable>,
    images: Mutex<Images>,
}

impl Pencil {
    /// The depth and the mask (or the one-pixel stand-in).
    fn geometry_bind(
        &self,
        frame: &StyleFrame<'_>,
        with_mask: bool,
        paper: &StackedImages,
        stroke: &StackedImages,
    ) -> Arc<wgpu::BindGroup> {
        let mask = frame.mask.filter(|_| with_mask);
        // Kept over the images, the albedo and the edges themselves (#960):
        // a new paper or stroke upload is a new view, and its group takes
        // the old one's place. With and without the mask side by side
        let view = crate::bind_cache::Bound::View;
        let over = [
            view(mask.unwrap_or(&self.no_mask_view)),
            view(&paper.view),
            view(&stroke.view),
            frame.albedo_or(&self.no_albedo_view),
            frame.edges_or(&self.no_edges_view),
        ];
        let variant = mask.is_some() as u64;
        frame.depth_bind_over("Pencil Geometry BG", variant, &over, || {
            frame.device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                label: Some("Pencil Geometry BG"),
                layout: &self.geometry_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(frame.depth),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            mask.unwrap_or(&self.no_mask_view),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&paper.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&stroke.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(
                            frame
                                .albedo
                                .as_ref()
                                .map_or(&self.no_albedo_view, |a| &a.view),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(
                            frame
                                .edges
                                .as_ref()
                                .map_or(&self.no_edges_view, |e| &e.view),
                        ),
                    },
                ],
            })
        })
    }

    /// Row 0 (the whole frame's) and a row for each per-entity variant,
    /// with the row of every mask id (#740: every variant gets one).
    /// Each row's image names (paper, stroke) and mask id come back
    /// beside it.
    fn variant_table(&self, frame: &StyleFrame<'_>) -> Table {
        let whole = row(
            &|n| frame.param(n),
            &|n| frame.option(n).to_string(),
            frame.time,
            [0.0, 0.0],
            0.0,
        );
        let mut rows = vec![whole];
        let image_names = |o: &dyn Fn(&str) -> String| [o("paper_image"), o("stroke_image")];
        let mut names = vec![image_names(&|n| frame.option(n).to_string())];
        let mut ids = vec![0u16];
        let mut slots = vec![0u32; frame.style_ids.len() + 1];
        // Ids another running style draws are that style's pixels (#384:
        // the framework puts the input back there too; the slot keeps
        // pencil's contours and hatching from reading them as its own)
        for id in frame.foreign_ids() {
            slots[id as usize] = FOREIGN;
        }
        for (id, v) in frame.variants() {
            let origin = v
                .anchor
                .and_then(|a| frame.project(a))
                .map_or([0.0, 0.0], |at| [at[0].round(), at[1].round()]);
            let flags = if v.off { PASS_THROUGH } else { 0.0 };
            slots[id as usize] = rows.len() as u32;
            ids.push(id);
            names.push(image_names(&|n| frame.variant_option(v, n).to_string()));
            let mut r = row(
                &|n| frame.variant_param(v, n),
                &|n| frame.variant_option(v, n).to_string(),
                frame.time,
                origin,
                flags,
            );
            // Strokes on the root's own surfaces (#544); a prop scaled to
            // nothing has no frame and keeps the world's
            if let Some(local) = v.local.and_then(|f| f.inverse_rows()) {
                r.set_local(local);
            }
            rows.push(r);
        }
        Table {
            rows,
            slots,
            names,
            ids,
        }
    }

    /// The frame's paper and stroke stacks (#547), each row given the
    /// layer it reads (lanes 2 and 3 of its eighth vec4: the layer plus
    /// one, 0 for the procedural paper or dashes): a layer each image the
    /// rows name, up to [`MAX_LAYERS`]; past it, or for an image that
    /// doesn't load, the row draws procedurally.
    fn images(&self, frame: &StyleFrame<'_>, table: &mut Table) -> (Stacked, Stacked) {
        let papers: Vec<String> = table.names.iter().map(|n| n[0].clone()).collect();
        let strokes: Vec<String> = table.names.iter().map(|n| n[1].clone()).collect();
        let poll = frame.frame.is_multiple_of(15);
        let mut images = self.images.lock().unwrap_or_else(|e| e.into_inner());
        let paper = images
            .paper
            .get(frame.device, frame.queue, frame.assets, &papers, poll);
        let stroke = images
            .stroke
            .get(frame.device, frame.queue, frame.assets, &strokes, poll);
        for (lane, stack, kind) in [(2, &paper, "paper images"), (3, &stroke, "stroke images")] {
            for (row, layer) in table.rows.iter_mut().zip(&stack.layer) {
                row.v[7][lane] = *layer as f32;
            }
            // The first entity past the cap is said (#740)
            if let Some(row) = stack.over_cap {
                let entity = frame.style_ids.entity(table.ids[row]).unwrap_or("?");
                report_drop("pencil", kind, MAX_LAYERS, entity, "drawn procedurally");
            }
        }
        (paper, stroke)
    }
}

/// One frame's variant table, built by [`Pencil::variant_table`].
struct Table {
    rows: Vec<PencilRow>,
    /// Each mask id's row, [`FOREIGN`] for another style's.
    slots: Vec<u32>,
    /// Each row's (paper, stroke) image names.
    names: Vec<[String; 2]>,
    /// Each row's mask id (row 0's is 0).
    ids: Vec<u16>,
}

impl Pencil {
    /// Group 0: the uniform and this frame's variant table, written first.
    fn uniform_bind(
        &self,
        frame: &StyleFrame<'_>,
        rows: &[PencilRow],
        slots: &[u32],
    ) -> Arc<wgpu::BindGroup> {
        let mut table = self.table.lock().unwrap_or_else(|e| e.into_inner());
        table.upload(frame.device, frame.queue, rows, slots);
        let [rows, slots] = table.entries(1);
        frame.bind("Pencil Uniform BG", table.generation(), || {
            frame.device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                label: Some("Pencil Uniform BG"),
                layout: &self.uniform_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniforms.as_entire_binding(),
                    },
                    rows,
                    slots,
                ],
            })
        })
    }
}

impl StylePass for Pencil {
    /// Per entity (#743), the prep reads the shared edges only where it
    /// draws: [`prep_rect`], in depth pixels.
    fn edges_rect(&self, frame: &StyleFrame<'_>) -> Option<kit::PixelRect> {
        let rows = self.variant_table(frame).rows;
        let ppp = if frame.pixels_per_point.is_finite() && frame.pixels_per_point > 0.0 {
            frame.pixels_per_point
        } else {
            1.0
        };
        let size = [frame.width, frame.height];
        prep_rect(frame, &rows, ppp).map(|r| {
            r.rescaled(size, frame.depth_size)
                .padded(2, frame.depth_size[0], frame.depth_size[1])
        })
    }

    fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    ) {
        let mut table = self.variant_table(frame);
        let (paper, stroke) = self.images(frame, &mut table);
        let (paper, stroke) = (paper.images, stroke.images);
        let Table { rows, slots, .. } = table;
        let with_mask = (rows.len() > 1 || slots.contains(&FOREIGN)) && frame.mask.is_some();
        let uniforms = PencilUniforms {
            inv_view_proj: frame.camera.inv_view_proj,
            camera_pos: frame.camera.position,
            pixels_per_point: if frame.pixels_per_point.is_finite() && frame.pixels_per_point > 0.0
            {
                frame.pixels_per_point
            } else {
                1.0
            },
            size: [frame.width as f32, frame.height as f32],
            srgb: if frame.srgb() { 1.0 } else { 0.0 },
            front: whole_frame_front(frame.config),
            forward: forward_axis(&frame.camera.inv_view_proj),
            _pad1: 0.0,
            info: [
                rows.len() as u32,
                !(frame.whole_frame() && frame.option("scope") != "entities") as u32,
                with_mask as u32,
                0,
            ],
            edge: [
                frame.config.edge_depth.max(super::edges::MIN_THRESHOLD),
                frame.config.edge_normal.max(super::edges::MIN_THRESHOLD),
                0.0,
                0.0,
            ],
            papers: paper.layers,
            strokes: stroke.layers,
        };
        frame
            .queue
            .write_buffer_counted(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
        let uniform = self.uniform_bind(frame, &rows, &slots);
        let input_bind = frame.sampled_input("Pencil Input BG", &self.input_bgl, input);
        let geometry = self.geometry_bind(frame, with_mask, &paper, &stroke);
        let prep = frame.target("pencil.prep", PREP_FORMAT);

        // Per entity, only the entities' pixels change (#385): the draw
        // pass runs inside their rect, and the prep inside it grown by
        // how far a styled pixel reads the prep
        let size = [frame.width, frame.height];
        let draw_rect = frame.styled_rect(0);
        let prep_rect = prep_rect(frame, &rows, uniforms.pixels_per_point);
        kit::fullscreen_pass_in(
            encoder,
            "Pencil Prep",
            &prep.view,
            size,
            &self.prep_pipeline,
            &[&uniform, &input_bind, &geometry],
            prep_rect,
        );
        // The tensor flow: blurred wide in two passes (#392), only when a
        // row asks for it. Group 3 holds the prep and the flow, the prep
        // twice while no row does.
        let flow = any_tensor(&rows).then(|| {
            // At half resolution: the flow is smooth over a dozen pixels
            let (w, h) = (frame.width.div_ceil(2), frame.height.div_ceil(2));
            let across = frame.target_sized("pencil.flow_across", FLOW_FORMAT, w, h);
            let flow = frame.target_sized("pencil.flow", FLOW_FORMAT, w, h);
            kit::fullscreen_pass(
                encoder,
                "Pencil Flow Across",
                &across.view,
                &self.flow_across,
                &[
                    &uniform,
                    &frame.sampled("Pencil Flow Prep BG", &self.input_bgl, &prep.view),
                    &geometry,
                ],
            );
            kit::fullscreen_pass(
                encoder,
                "Pencil Flow Down",
                &flow.view,
                &self.flow_down,
                &[
                    &uniform,
                    &frame.sampled("Pencil Flow Across BG", &self.input_bgl, &across.view),
                    &geometry,
                ],
            );
            flow
        });
        let flow_view = flow.as_ref().map_or(&prep.view, |f| &f.view);
        let prep_bind = frame.bind("Pencil Prep BG", flow.is_some() as u64, || {
            frame.device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                label: Some("Pencil Prep BG"),
                layout: &self.prep_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&prep.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(frame.linear_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(flow_view),
                    },
                ],
            })
        });
        // Drawing on (#879) takes the other module, only while some front
        // is below 1: the whole frame's, or a drawing row's entity at a
        // mask blend under 1
        let pipeline = if draws_on_now(&rows, uniforms.front, frame.mask_fades && with_mask) {
            &self.draw_on_pipeline
        } else {
            &self.draw_pipeline
        };
        kit::scissored_pass(
            encoder,
            "Pencil Pass",
            output,
            size,
            pipeline,
            &[&uniform, &input_bind, &geometry, &prep_bind],
            draw_rect,
            &self.blit,
            &input_bind,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validate(source: &str, name: &str) -> naga::Module {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name} validates: {e:?}"));
        module
    }

    #[test]
    fn both_shaders_validate_and_their_uniforms_match_rust() {
        for (source, name) in [
            (PREP_SHADER, "prep"),
            (DRAW_SHADER, "draw"),
            (DRAW_ON_SHADER, "draw on"),
            (FLOW_SHADER, "flow"),
        ] {
            let module = validate(source.text, name);
            let (_, u) = module
                .global_variables
                .iter()
                .find(|(_, v)| v.name.as_deref() == Some("u"))
                .expect("uniform u");
            let size = module.types[u.ty].inner.size(module.to_ctx());
            assert_eq!(
                size as usize,
                std::mem::size_of::<PencilUniforms>(),
                "{name}"
            );
        }
    }

    #[test]
    fn a_row_is_thirteen_vec4s() {
        // Ten of settings, three of a local grid's frame (#544)
        assert_eq!(std::mem::size_of::<PencilRow>(), 208);
        assert_eq!(
            std::mem::size_of::<PencilUniforms>(),
            // ... #743's edge thresholds, then the stacks' layers (#547)
            64 + 16 + 16 + 16 + 16 + 16 + 2 * 4 * 32
        );
    }

    /// `tone` rides in the row's ninth vec4 (#869), and the style albedo is
    /// asked for only while some row reads the light: the whole frame's
    /// table, or one entity's variant, an opt-out's never.
    #[test]
    fn the_albedo_is_drawn_only_while_a_row_takes_its_tone_from_the_light() {
        use crate::postprocess::PostProcessConfig;
        use crate::style_mask::{StyleIds, StyleVariant};
        let row_for = |tone: &str| {
            let tone = tone.to_string();
            row(
                &|n| STYLE.param(&PostProcessConfig::default(), n),
                &move |n| {
                    if n == "tone" {
                        tone.clone()
                    } else {
                        String::new()
                    }
                },
                0.0,
                [0.0, 0.0],
                0.0,
            )
        };
        assert_eq!(row_for("picture").v[8][1], 0.0);
        assert_eq!(row_for("light").v[8][1], 1.0);
        assert_eq!(row_for("lamp").v[8][1], 0.0, "an unknown name is picture");

        let none = StyleIds::default();
        let mut config = PostProcessConfig::default();
        config.styles.set_enabled("pencil", true);
        assert!(!crate::stylize::StyleRegistry::new().any_uses_albedo(&config, &none), "picture is the default");
        config.styles.set_option("pencil", "tone", "light");
        assert!(crate::stylize::StyleRegistry::new().any_uses_albedo(&config, &none));
        // A preset for entities draws nothing by itself
        config.styles.set_option("pencil", "scope", "entities");
        assert!(!crate::stylize::StyleRegistry::new().any_uses_albedo(&config, &none));

        // An entity's variant: its own tone, else the preset's
        let mut ids = StyleIds::default();
        ids.set(vec![("pencil".to_string(), StyleVariant::plain("pencil"))]);
        assert!(crate::stylize::StyleRegistry::new().any_uses_albedo(&config, &ids), "inherits light");
        let mut own = StyleVariant::plain("pencil");
        own.options.insert("tone".into(), "picture".into());
        ids.set(vec![("pencil{tone=picture}".to_string(), own)]);
        assert!(!crate::stylize::StyleRegistry::new().any_uses_albedo(&config, &ids));
        let mut off = StyleVariant::plain("pencil");
        off.off = true;
        ids.set(vec![("pencil{off}".to_string(), off)]);
        assert!(!crate::stylize::StyleRegistry::new().any_uses_albedo(&config, &ids), "an opt-out");
        // Post-processing off draws no style at all
        let mut ids = StyleIds::default();
        ids.set(vec![("pencil".to_string(), StyleVariant::plain("pencil"))]);
        config.enabled = false;
        assert!(!crate::stylize::StyleRegistry::new().any_uses_albedo(&config, &ids));
    }

    /// `transition = "draw"` (#879): the row carries it in `i.z` and the
    /// unboiled seed in `i.w`; pencil draws the whole frame's blend itself
    /// only while the table draws, and a variant's while the variant does
    /// (its own, else the table's); the cross-fade's owner table then gives
    /// that variant to nobody.
    #[test]
    fn a_draw_transition_takes_its_blends_from_the_framework() {
        use crate::postprocess::PostProcessConfig;
        use crate::style_mask::{StyleIds, StyleVariant};
        let mut config = PostProcessConfig::default();
        let p = |c: &PostProcessConfig, n: &str| STYLE.param(c, n);
        let r = row(
            &|n| p(&config, n),
            &|n| match n {
                "transition" => "draw".into(),
                o =>STYLE.option(&config, o).to_string(),
            },
            1.0,
            [0.0, 0.0],
            0.0,
        );
        assert_eq!(r.v[8][2], 1.0, "draw");
        // The draw-on pass is taken only while some front can be below 1
        let mut fade_row = r;
        fade_row.v[8][2] = 0.0;
        let mut opted_out = r;
        opted_out.v[6][2] = PASS_THROUGH + WORLD_GRID;
        assert!(!draws_on_now(&[fade_row, r], 1.0, false), "no blend below 1");
        assert!(draws_on_now(&[fade_row, r], 1.0, true), "an entity drawing on");
        assert!(!draws_on_now(&[r, fade_row], 1.0, true), "row 0 is the whole frame's");
        assert!(!draws_on_now(&[fade_row, opted_out], 1.0, true), "an opt-out draws nothing");
        assert!(draws_on_now(&[fade_row], 0.5, false), "the whole frame's front");
        assert_eq!(transition_mode("fade"), 0.0);
        assert_eq!(transition_mode("smear"), 0.0, "an unknown name is fade");

        // Boil moves the hatching's seed, never the order's
        let boiling = row(
            &|n| match n {
                "seed" => 7.0,
                "boil" => 4.0,
                n => p(&config, n),
            },
            &|n| STYLE.option(&config, n).to_string(),
            1.0,
            [0.0, 0.0],
            0.0,
        );
        assert_eq!(boiling.v[3][3], 11.0, "the strokes' seed boils");
        assert_eq!(boiling.v[8][3], 7.0, "the order's doesn't");

        config.styles.set_enabled("pencil", true);
        config.styles.set_blend("pencil", Some(0.4));
        assert!(!STYLE.draws_own_blend(&config, None), "fade is the default");
        assert_eq!(whole_frame_front(&config), 1.0, "the stage mixes it");
        config.styles.set_option("pencil", "transition", "draw");
        assert!(STYLE.draws_own_blend(&config, None));
        assert_eq!(whole_frame_front(&config), 0.4);

        // A variant inherits the table's draw unless it says otherwise
        let inherits = StyleVariant::plain("pencil");
        let mut fades = StyleVariant::plain("pencil");
        fades.options.insert("transition".into(), "fade".into());
        assert!(STYLE.draws_own_blend(&config, Some(&inherits)));
        assert!(!STYLE.draws_own_blend(&config, Some(&fades)));

        let mut ids = StyleIds::default();
        ids.set(vec![
            ("pencil".into(), inherits),
            ("pencil{transition=fade}".into(), fades),
        ]);
        let registry = crate::stylize::StyleRegistry::new();
        let table = super::super::fade::owner_table(&config, &ids, &registry);
        let me = registry.iter().position(|d| d.name == "pencil").unwrap() as u32 + 1;
        assert_eq!(table[ids.id("pencil").unwrap() as usize], 0, "drawn by pencil");
        assert_eq!(
            table[ids.id("pencil{transition=fade}").unwrap() as usize],
            me,
            "cross-faded by the framework"
        );
    }

    #[test]
    fn the_seed_holds_still_unless_it_boils() {
        assert_eq!(effective_seed(7.0, 0.0, 12.3), 7.0);
        assert_eq!(effective_seed(7.6, 0.0, 0.0), 7.0, "whole seeds");
        // Four redraws a second: the same step at the same time
        assert_eq!(effective_seed(7.0, 4.0, 0.0), 7.0);
        assert_eq!(effective_seed(7.0, 4.0, 0.2), 7.0);
        assert_eq!(effective_seed(7.0, 4.0, 0.25), 8.0);
        assert_eq!(effective_seed(7.0, 4.0, 1.0), 11.0);
        assert_eq!(effective_seed(7.0, 4.0, f32::NAN), 7.0);
        assert_eq!(effective_seed(-3.0, 0.0, 0.0), 0.0);
    }

    #[test]
    fn the_boil_steps_on_the_living_props_clock() {
        // The seed's step is the stepped clock with no glide (#747): the beat
        // `warp` gives a living prop counting 1/boil seconds a beat.
        let mut t = 0.0f32;
        for _ in 0..600 {
            t += 1.0 / 60.0;
            let beats = f64::from(t * 12.0 + flint_core::step_clock::TICK_EARLY);
            let (beat, _) = flint_core::step_clock::warp(beats, 0.0);
            assert_eq!(effective_seed(3.0, 12.0, t), 3.0 + beat as f32, "t {t}");
        }
    }

    /// The `pencil_style` schema and the descriptor agree: every field is a
    /// parameter, an option, a 3-array of `_r/_g/_b` parameters, or one
    /// every style component takes (`enabled`, `blend`, `blend_source`); every parameter and option can be set from it; and no
    /// field has a default, which the loader would fill in over the
    /// scene's table (the #266 gotcha).
    #[test]
    fn the_component_schema_matches_the_descriptor() {
        let schema: toml::Table = toml::from_str(include_str!(
            "../../../../schemas/components/pencil_style.toml"
        ))
        .unwrap();
        let fields = schema["component"]["pencil_style"]["fields"]
            .as_table()
            .unwrap();
        let is_param = |n: &str| STYLE.params.iter().any(|p| p.name == n);
        let is_option = |n: &str| STYLE.options.iter().any(|o| o.name == n);
        let mut reached: Vec<String> = Vec::new();
        for (key, field) in fields {
            assert!(field.get("default").is_none(), "{key} has a default");
            if [
                "enabled",
                crate::style_mask::BLEND_FIELD,
                crate::style_mask::BLEND_SOURCE_FIELD,
            ]
            .contains(&key.as_str())
            {
                continue;
            }
            if is_param(key) || is_option(key) {
                reached.push(key.clone());
            } else {
                let channels = ["r", "g", "b"].map(|c| format!("{key}_{c}"));
                assert!(
                    channels.iter().all(|c| is_param(c)),
                    "{key} is no parameter, option or colour of pencil"
                );
                assert_eq!(field["type"].as_str(), Some("vec3"), "{key}");
                reached.extend(channels);
            }
        }
        for p in &STYLE.params {
            assert!(
                reached.iter().any(|r| r == p.name),
                "{} not in the schema",
                p.name
            );
        }
        for o in &STYLE.options {
            // scope is the scene table's, not an entity's
            if o.name != "scope" {
                assert!(
                    reached.iter().any(|r| r == o.name),
                    "{} not in the schema",
                    o.name
                );
            }
        }
    }

    /// The flow blur (#392) runs only for a drawn row that hatches by tensor.
    #[test]
    fn the_flow_blur_runs_only_for_tensor_rows() {
        let config = crate::postprocess::PostProcessConfig::default();
        let p = |n: &str| STYLE.param(&config, n);
        let with = |dir: &'static str, flags: f32| {
            row(
                &p,
                &|n: &str| {
                    if n == "direction" {
                        dir.into()
                    } else {
                        STYLE.option(&config, n).to_string()
                    }
                },
                0.0,
                [0.0, 0.0],
                flags,
            )
        };
        let surface = with("surface", 0.0);
        let screen = with("screen", 0.0);
        let tensor = with("tensor", 0.0);
        let tensor_off = with("tensor", PASS_THROUGH);
        assert!(!any_tensor(&[surface, screen]));
        assert!(any_tensor(&[surface, tensor]));
        assert!(
            !any_tensor(&[surface, tensor_off]),
            "an opt-out row draws nothing"
        );
        let mut lines_only = tensor;
        lines_only.v[0][2] = 0.0;
        assert!(
            !any_tensor(&[surface, lines_only]),
            "a row with no hatch layers draws no strokes (#404)"
        );
    }

    #[test]
    fn a_world_grid_sets_its_flag_beside_pass_through() {
        let p = |n: &str| STYLE.params.iter().find(|q| q.name == n).unwrap().default;
        let world = |n: &str| {
            if n == "grid" {
                "world".to_string()
            } else {
                String::new()
            }
        };
        assert_eq!(row(&p, &world, 0.0, [0.0, 0.0], 0.0).v[6][2], WORLD_GRID);
        assert_eq!(
            row(&p, &world, 0.0, [0.0, 0.0], PASS_THROUGH).v[6][2],
            PASS_THROUGH + WORLD_GRID
        );
        assert!(GRIDS.contains(&"world"));
    }

    /// `grid = "local"` (#544) is a world grid until its frame is set,
    /// then carries the frame's rows and its flag; any other row keeps the
    /// identity and refuses a frame.
    #[test]
    fn a_local_grid_is_a_world_grid_with_its_roots_frame() {
        let p = |n: &str| STYLE.params.iter().find(|q| q.name == n).unwrap().default;
        let grid = |g: &'static str| {
            move |n: &str| {
                if n == "grid" {
                    g.to_string()
                } else {
                    String::new()
                }
            }
        };
        let frame = [
            [0.0, 0.0, -1.0, 2.0],
            [0.0, 1.0, 0.0, -3.0],
            [1.0, 0.0, 0.0, 0.5],
        ];
        let mut local = row(&p, &grid("local"), 0.0, [0.0, 0.0], 0.0);
        assert_eq!(local.v[6][2], WORLD_GRID, "no frame yet: the world's");
        assert_eq!(&local.v[10..13], &NO_LOCAL, "the identity");
        local.set_local(frame);
        assert_eq!(local.v[6][2], WORLD_GRID + LOCAL_GRID);
        assert_eq!(&local.v[10..13], &frame);
        local.set_local(frame);
        assert_eq!(local.v[6][2], WORLD_GRID + LOCAL_GRID, "set twice, one flag");

        let mut screen = row(&p, &grid("screen"), 0.0, [0.0, 0.0], 0.0);
        screen.set_local(frame);
        assert_eq!(screen.v[6][2], 0.0, "a screen grid takes no frame");
        assert_eq!(&screen.v[10..13], &NO_LOCAL);
        let mut off = row(&p, &grid("local"), 0.0, [0.0, 0.0], PASS_THROUGH);
        off.set_local(frame);
        assert_eq!(off.v[6][2], PASS_THROUGH + WORLD_GRID + LOCAL_GRID);
        assert!(GRIDS.contains(&"local"));
    }

    #[test]
    fn directions_parse_and_fall_back_to_surface() {
        assert_eq!(direction_mode("surface"), 0.0);
        assert_eq!(direction_mode("screen"), 1.0);
        assert_eq!(direction_mode("tensor"), 2.0);
        assert_eq!(direction_mode("diagonal"), 0.0);
    }

    #[test]
    fn a_row_carries_its_settings_where_the_shader_reads_them() {
        let config = crate::postprocess::PostProcessConfig::default();
        let p = |n: &str| STYLE.param(&config, n);
        let o = |n: &str| STYLE.option(&config, n).to_string();
        let r = row(&p, &o, 0.0, [3.0, 4.0], PASS_THROUGH);
        assert_eq!(r.v[0][0], 6.0, "spacing");
        assert!(
            (r.v[0][3] - 50f32.to_radians()).abs() < 1e-6,
            "angle in radians"
        );
        assert_eq!(r.v[4][3], 0.0, "surface by default");
        assert_eq!(r.v[6][2], PASS_THROUGH, "no world grid by default");
        assert_eq!(&r.v[4][..3], &[0.95, 0.93, 0.88], "paper");
        assert_eq!([r.v[6][0], r.v[6][1], r.v[6][2]], [3.0, 4.0, 1.0]);
        assert_eq!(r.v[7], [1.0, 1.0, 0.0, 0.0], "no images by default");
        assert_eq!(r.v[9][0], 0.0, "no clutter by default");
    }

    /// `clutter` (#883) rides in the tenth vec4's first lane, clamped to
    /// 0..1, and the scissor reaches as far as the wide disc it measures:
    /// six contour widths, wider than a thin line's wobble.
    #[test]
    fn clutter_rides_in_the_row_and_widens_the_reach() {
        let config = crate::postprocess::PostProcessConfig::default();
        let cfg = &config;
        let o = |n: &str| STYLE.option(cfg, n).to_string();
        let with = |clutter: f32| {
            let p = move |n: &str| match n {
                "clutter" => clutter,
                "contour_width" => 4.0,
                "wobble" => 0.0,
                "spacing" => 1.0,
                _ => STYLE.param(cfg, n),
            };
            row(&p, &o, 0.0, [0.0, 0.0], 0.0)
        };
        assert_eq!(with(1.0).v[9][0], 1.0);
        assert_eq!(with(3.0).v[9][0], 1.0, "clamped");
        // 1.5 spacing, or half a 4 pt outline: 2 pt; the wide disc: 24 pt
        assert_eq!(prep_reach(&[with(0.0)], 1.0), 2 + 4);
        assert_eq!(prep_reach(&[with(1.0)], 1.0), 24 + 4);
        assert_eq!(prep_reach(&[with(0.0), with(0.5)], 2.0), 48 + 4);
    }

    #[test]
    fn a_rows_image_flags_say_whether_it_names_one() {
        // Provisional, until the stacks give each row its layer (#547)
        let config = crate::postprocess::PostProcessConfig::default();
        let p = |n: &str| STYLE.param(&config, n);
        let named = |n: &str| match n {
            "paper_image" => "cold_press".to_string(),
            o => STYLE.option(&config, o).to_string(),
        };
        assert_eq!(row(&p, &named, 0.0, [0.0; 2], 0.0).v[7][2..], [1.0, 0.0]);
        assert!(names_image("cold_press") && !names_image("none") && !names_image(""));
    }
}
