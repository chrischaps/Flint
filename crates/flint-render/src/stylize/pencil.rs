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
//! as before. One paper and one strip a frame: the scene table's, else the
//! first entity's that names one; a row naming another draws procedurally
//! (warned once).
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

use super::images::{ImageSlot, ImageUse, StyleImage};
use super::kit;
use super::{
    StyleDescriptor, StyleDevice, StyleFrame, StyleHint, StyleInput, StyleOption, StyleParam,
    StylePass, StyleStage,
};
use bytemuck::{Pod, Zeroable};
use std::sync::{Arc, Mutex};

/// The stroke direction modes, by their option spelling.
pub const DIRECTIONS: &[&str] = &["surface", "screen", "tensor"];

/// What a `[post_process.styles.pencil]` table covers (as ASCII's).
pub const SCOPES: &[&str] = &["screen", "entities"];

/// Where a variant's hatching is anchored: ASCII's two grids, and `world`,
/// strokes laid on the surfaces themselves (#389).
pub const GRIDS: &[&str] = &["screen", "object", "world"];

/// Rows one frame's variant table holds, the whole frame's included.
pub const MAX_VARIANTS: usize = 64;

/// Mask ids the table covers.
pub const MAX_IDS: usize = 256;

/// The scratch target pass 1 writes: tone, contour, flow.
pub const PREP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The tensor flow blur's two targets (#392): the blurred double angle.
pub const FLOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;

/// The direction mode the flow blur runs for.
const TENSOR: f32 = 2.0;

const PARAMS: &[StyleParam] = &[
    StyleParam::new("spacing", 6.0, 2.0..=24.0),
    StyleParam::new("thickness", 1.0, 0.25..=4.0),
    StyleParam::new("layers", 3.0, 0.0..=4.0),
    StyleParam::new("angle", 50.0, 0.0..=180.0),
    StyleParam::new("brightness", 0.0, -1.0..=1.0),
    StyleParam::new("contrast", 1.2, 0.0..=4.0),
    StyleParam::new("darkness", 0.85, 0.0..=1.0),
    StyleParam::new("shade", 0.25, 0.0..=1.0),
    StyleParam::new("contour", 1.0, 0.0..=2.0),
    StyleParam::new("contour_width", 1.25, 0.5..=4.0),
    StyleParam::new("depth", 1.0, 0.0..=4.0),
    StyleParam::new("grazing", 0.0, 0.0..=1.0),
    StyleParam::new("crease", 1.0, 0.0..=4.0),
    StyleParam::new("detail", 0.35, 0.0..=2.0),
    StyleParam::new("wobble", 1.5, 0.0..=8.0),
    StyleParam::new("sketchy", 0.6, 0.0..=1.0),
    StyleParam::new("grain", 0.5, 0.0..=1.0),
    StyleParam::new("color", 0.0, 0.0..=1.0),
    StyleParam::new("seed", 0.0, 0.0..=9999.0),
    StyleParam::new("boil", 0.0, 0.0..=24.0),
    StyleParam::new("paper_r", 0.95, 0.0..=1.0),
    StyleParam::new("paper_g", 0.93, 0.0..=1.0),
    StyleParam::new("paper_b", 0.88, 0.0..=1.0),
    StyleParam::new("lead_r", 0.16, 0.0..=1.0),
    StyleParam::new("lead_g", 0.16, 0.0..=1.0),
    StyleParam::new("lead_b", 0.19, 0.0..=1.0),
    StyleParam::new("paper_scale", 1.0, 0.25..=8.0),
    StyleParam::new("stroke_rows", 1.0, 1.0..=16.0),
];

/// The value of `paper_image` / `stroke_image` that names no image.
pub const NO_IMAGE: &str = "none";

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

/// Whether an image option's value names an image.
pub fn names_image(value: &str) -> bool {
    !value.is_empty() && value != NO_IMAGE
}

const OPTIONS: &[StyleOption] = &[
    StyleOption::new("direction", "surface", DIRECTIONS),
    StyleOption::new("scope", "screen", SCOPES),
    StyleOption::new("grid", "screen", GRIDS),
    StyleOption::new("paper_image", NO_IMAGE, &[]).discovered(paper_choices),
    StyleOption::new("stroke_image", NO_IMAGE, &[]).discovered(stroke_choices),
];

const HINTS: &[StyleHint] = &[
    StyleHint::new("direction", "Stroke direction", "surface: round the form, along iso-depth lines; screen: all at the angle; tensor: along the picture's edges"),
    StyleHint::new("scope", "Scope", "screen: the table draws the whole frame; entities: it is only the preset for pencil_style entities"),
    StyleHint::new("grid", "Hatch anchor", "screen: hatching fixed to the screen; object: pinned to each entity's origin, so strokes travel with it; world: laid on the surfaces, so they hold as the camera moves"),
    StyleHint::new("spacing", "Spacing (pt)", "Hatch line spacing in points"),
    StyleHint::new("thickness", "Stroke width (pt)", "Hatch stroke width in points"),
    StyleHint::new("layers", "Hatch layers", "Up to four; each joins, crossing the last, as the tone darkens").integer(),
    StyleHint::new("angle", "Angle (deg)", "The base hatch angle; where surface or tensor directions are unsure it shows through"),
    StyleHint::new("brightness", "Brightness", "Shifts the tone before it picks hatch layers"),
    StyleHint::new("contrast", "Contrast", "Spreads the tone around mid-grey before it picks hatch layers"),
    StyleHint::new("darkness", "Stroke darkness", "How dark one hatch stroke is"),
    StyleHint::new("shade", "Rubbed shade", "Soft graphite under the strokes, in the darks"),
    StyleHint::new("contour", "Outline", "Outline strength; 0 = none"),
    StyleHint::new("contour_width", "Outline width (pt)", "Outline width in points"),
    StyleHint::new("depth", "Silhouettes", "Outline sensitivity to depth steps"),
    StyleHint::new("grazing", "Spare grazing planes", "1 draws a silhouette only at a true step, so a floor seen edge-on doesn't ink the horizon in a band; 0 = every depth step"),
    StyleHint::new("crease", "Creases", "Outline sensitivity to where the surface turns"),
    StyleHint::new("detail", "Texture lines", "Outlines from the picture's own luminance edges"),
    StyleHint::new("wobble", "Wobble (pt)", "How far lines wander off true"),
    StyleHint::new("sketchy", "Sketchy", "A second, offset outline beside the first"),
    StyleHint::new("grain", "Paper grain", "Paper grain, and how much its tooth breaks the strokes"),
    StyleHint::new("color", "Colour", "0 draws on paper; 1 draws over the scene's own colour (coloured pencil)"),
    StyleHint::new("seed", "Seed", "Another whole number is another hand; the same seed draws the same picture").integer(),
    StyleHint::new("boil", "Boil (per s)", "Redraws a second, for animated lines; 0 = still"),
    StyleHint::new("paper", "Paper", "Paper colour (display values)"),
    StyleHint::new("lead", "Lead", "Graphite colour (display values)"),
    StyleHint::new("paper_image", "Paper image", "A scanned paper (papers/<name>.png): its relief is the tooth, its hue tints the paper; none = procedural"),
    StyleHint::new("paper_scale", "Paper scale (pt)", "Points one texel of the paper image covers"),
    StyleHint::new("stroke_image", "Stroke image", "A strip of real strokes (strokes/<name>.png), one stretched along each dash; none = drawn bars"),
    StyleHint::new("stroke_rows", "Strokes in strip", "How many strokes the strip stacks, top to bottom").integer(),
];

pub(super) static STYLE: StyleDescriptor =
    StyleDescriptor::new("pencil", "Pencil", StyleStage::Display, create)
        .params(PARAMS)
        .options(OPTIONS)
        .hints(HINTS)
        .component("pencil_style");

fn create(d: &StyleDevice) -> Box<dyn StylePass> {
    let device = d.device;
    let uniform_bgl = kit::uniform_layout(device, "Pencil Uniform BGL");
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
            image_entry(2),
            image_entry(3),
        ],
    });
    // Bound while no entity asks for the style: one unstyled pixel
    let no_mask = device.create_texture(&wgpu::TextureDescriptor {
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
        uniforms: kit::uniform_buffer::<PencilUniforms>(device, "Pencil Uniforms"),
        uniform_bgl,
        input_bgl,
        prep_bgl,
        geometry_bgl,
        _no_mask: no_mask,
        no_mask_view,
        blit,
        warned: Mutex::new(false),
        images: Mutex::new(Images {
            paper: ImageSlot::new(ImageUse::Paper, "pencil paper"),
            stroke: ImageSlot::new(ImageUse::Stroke, "pencil stroke"),
            warned: false,
        }),
    })
}

/// A style image's binding: read with `textureLoad`, no sampler.
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
    "pencil_common.wgsl",
    "pencil_prep.wgsl"
);
const DRAW_SHADER: kit::Wgsl = kit::wgsl!(
    "pencil_common.wgsl",
    "style_image.wgsl",
    "pencil.wgsl"
);
const FLOW_SHADER: kit::Wgsl = kit::wgsl!(
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

/// One variant's settings, nine vec4s (see `PencilRow` in the shader).
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug, PartialEq)]
pub struct PencilRow {
    pub v: [[f32; 4]; 9],
}

/// The paper and stroke images, kept with the style (#390).
struct Images {
    paper: ImageSlot,
    stroke: ImageSlot,
    /// Whether the two-images-in-one-frame warning has been given.
    warned: bool,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct PencilUniforms {
    inv_view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 3],
    pixels_per_point: f32,
    size: [f32; 2],
    srgb: f32,
    _pad0: f32,
    forward: [f32; 3],
    _pad1: f32,
    info: [u32; 4],
    img: [f32; 4],
    rows: [PencilRow; MAX_VARIANTS],
    slots: [[u32; 4]; MAX_IDS / 4],
}

/// A row's flag: leave its pixels as they are.
const PASS_THROUGH: f32 = 1.0;

/// A row's flag (added to the others): its hatching is on the world's
/// surfaces, `grid = "world"` (#389).
const WORLD_GRID: f32 = 2.0;

/// The slot of a mask id another registered style owns: its pixels are
/// that style's (an ASCII prop in a pencil frame), so pencil leaves them.
pub const FOREIGN: u32 = 255;

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

/// The seed a frame draws with: `seed`, moved on `boil` times a second of
/// post time, so a still drawing (boil 0) never changes and a boiling one
/// changes in steps, the same steps at the same `--style-time`. The steps
/// are [`motion_hold::tick`](crate::motion_hold::tick)'s, so a pose held at
/// `rate = boil` moves in the same frames the lines redraw (#405).
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
                flags
                    + if option("grid") == "world" {
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
            [param("grazing").clamp(0.0, 1.0), 0.0, 0.0, 0.0],
        ],
    }
}

/// The one image a frame draws with in `lane` (2 paper, 3 stroke) of the
/// rows' last vec4, from each row's name for it: the scene table's (row 0)
/// when it names one, else the first row's that does. A row naming
/// another loses its flag and draws procedurally. Returns the name and
/// whether any row was refused so.
pub fn choose_image(names: &[String], rows: &mut [PencilRow], lane: usize) -> (String, bool) {
    let chosen = names
        .iter()
        .find(|n| names_image(n))
        .cloned()
        .unwrap_or_default();
    let mut refused = false;
    for (name, row) in names.iter().zip(rows.iter_mut()) {
        if row.v[7][lane] > 0.5 && *name != chosen {
            row.v[7][lane] = 0.0;
            refused = true;
        }
    }
    (chosen, refused)
}

/// How far, in pixels, a styled pixel's draw reads the prep texture
/// (#385), for the rows `rows`: the surface flow's 5x5 taps over
/// `1.5 * spacing` points, the tensor flow's blur (#392, `pencil_flow.wgsl`)
/// over `3 * spacing` along each axis, or a contour's wobble
/// (`1.5 * wobble`) plus half its width; and four pixels for the bilinear
/// taps' footprint and the flow's half-resolution texels.
fn prep_reach(rows: &[PencilRow], pixels_per_point: f32) -> u32 {
    let points = rows
        .iter()
        .map(|r| {
            let [spacing, ..] = r.v[0];
            let contour_width = r.v[1][3];
            let wobble = r.v[2][3];
            let flow = if r.v[4][3] == TENSOR { 3.0 } else { 1.5 };
            (flow * spacing).max(1.5 * wobble + 0.5 * contour_width.max(1.0))
        })
        .fold(0.0f32, f32::max);
    (points * pixels_per_point).ceil() as u32 + 4
}

struct Pencil {
    prep_pipeline: wgpu::RenderPipeline,
    /// The tensor flow blur along x, from the prep target (#392)
    flow_across: wgpu::RenderPipeline,
    /// And along y, from the across pass's result
    flow_down: wgpu::RenderPipeline,
    draw_pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    uniform_bgl: wgpu::BindGroupLayout,
    input_bgl: wgpu::BindGroupLayout,
    prep_bgl: wgpu::BindGroupLayout,
    geometry_bgl: wgpu::BindGroupLayout,
    _no_mask: wgpu::Texture,
    no_mask_view: wgpu::TextureView,
    /// The input through, outside the styled entities' rect (#385).
    blit: kit::Blit,
    /// Whether the too-many-variants warning has been given.
    warned: Mutex<bool>,
    images: Mutex<Images>,
}

impl Pencil {
    /// The depth and the mask (or the one-pixel stand-in).
    fn geometry_bind(
        &self,
        frame: &StyleFrame<'_>,
        with_mask: bool,
        paper: &StyleImage,
        stroke: &StyleImage,
    ) -> Arc<wgpu::BindGroup> {
        let mask = frame.mask.filter(|_| with_mask);
        // Keyed by both images' uploads, so a new paper gets a new group
        let key = mask.is_some() as u64 | paper.generation << 1 | stroke.generation << 32;
        frame.depth_bind("Pencil Geometry BG", key, || {
            frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
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
                ],
            })
        })
    }

    /// Row 0 (the whole frame's) and a row for each per-entity variant,
    /// with the row of every mask id.
    /// Each row's image names (paper, stroke) come back beside it.
    #[allow(clippy::type_complexity)]
    fn variant_table(
        &self,
        frame: &StyleFrame<'_>,
    ) -> (Vec<PencilRow>, [u32; MAX_IDS], Vec<[String; 2]>) {
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
        let mut slots = [0u32; MAX_IDS];
        // Ids another running style draws are that style's pixels (#384:
        // the framework puts the input back there too; the slot keeps
        // pencil's contours and hatching from reading them as its own)
        for id in frame.foreign_ids() {
            if (id as usize) < MAX_IDS {
                slots[id as usize] = FOREIGN;
            }
        }
        for (id, v) in frame.variants() {
            if rows.len() == MAX_VARIANTS || id as usize >= MAX_IDS {
                let mut warned = self.warned.lock().unwrap_or_else(|e| e.into_inner());
                if !*warned {
                    tracing::warn!(
                        "pencil: more per-entity variants than the {} (or ids past {MAX_IDS}) \
                         one frame holds; the rest are drawn like the whole frame",
                        MAX_VARIANTS - 1
                    );
                    *warned = true;
                }
                continue;
            }
            let origin = v
                .anchor
                .and_then(|a| frame.project(a))
                .map_or([0.0, 0.0], |at| [at[0].round(), at[1].round()]);
            let flags = if v.off { PASS_THROUGH } else { 0.0 };
            slots[id as usize] = rows.len() as u32;
            names.push(image_names(&|n| frame.variant_option(v, n).to_string()));
            rows.push(row(
                &|n| frame.variant_param(v, n),
                &|n| frame.variant_option(v, n).to_string(),
                frame.time,
                origin,
                flags,
            ));
        }
        (rows, slots, names)
    }

    /// The frame's paper and stroke images, clearing the flag of any row
    /// that names another (see [`choose_image`]) or whose image didn't load.
    fn images(
        &self,
        frame: &StyleFrame<'_>,
        rows: &mut [PencilRow],
        names: &[[String; 2]],
    ) -> (Arc<StyleImage>, Arc<StyleImage>) {
        let papers: Vec<String> = names.iter().map(|n| n[0].clone()).collect();
        let strokes: Vec<String> = names.iter().map(|n| n[1].clone()).collect();
        let (paper_name, refused_paper) = choose_image(&papers, rows, 2);
        let (stroke_name, refused_stroke) = choose_image(&strokes, rows, 3);
        let search = frame.assets;
        let poll = frame.frame.is_multiple_of(15);
        let mut images = self.images.lock().unwrap_or_else(|e| e.into_inner());
        if (refused_paper || refused_stroke) && !images.warned {
            tracing::warn!(
                "pencil: one paper and one stroke image a frame (\"{paper_name}\", \
                 \"{stroke_name}\"); entities naming others draw procedurally"
            );
            images.warned = true;
        }
        let paper = images
            .paper
            .get(frame.device, frame.queue, search, &paper_name, poll);
        let stroke = images
            .stroke
            .get(frame.device, frame.queue, search, &stroke_name, poll);
        for row in rows.iter_mut() {
            if !paper.found {
                row.v[7][2] = 0.0;
            }
            if !stroke.found {
                row.v[7][3] = 0.0;
            }
        }
        (paper, stroke)
    }
}

impl StylePass for Pencil {
    fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    ) {
        let (mut rows, slots, names) = self.variant_table(frame);
        let (paper, stroke) = self.images(frame, &mut rows, &names);
        let with_mask = (rows.len() > 1 || slots.contains(&FOREIGN)) && frame.mask.is_some();
        let mut uniforms = PencilUniforms {
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
            _pad0: 0.0,
            forward: forward_axis(&frame.camera.inv_view_proj),
            _pad1: 0.0,
            info: [
                rows.len() as u32,
                !(frame.whole_frame() && frame.option("scope") != "entities") as u32,
                with_mask as u32,
                0,
            ],
            img: paper.stats.paper_and(stroke.stats),
            rows: [PencilRow::zeroed(); MAX_VARIANTS],
            slots: [[0; 4]; MAX_IDS / 4],
        };
        uniforms.rows[..rows.len()].copy_from_slice(&rows);
        for (i, slot) in slots.iter().enumerate() {
            uniforms.slots[i / 4][i % 4] = *slot;
        }
        frame
            .queue
            .write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));

        let uniform = frame.uniform("Pencil Uniform BG", &self.uniform_bgl, &self.uniforms);
        let input_bind = frame.sampled_input("Pencil Input BG", &self.input_bgl, input);
        let geometry = self.geometry_bind(frame, with_mask, &paper, &stroke);
        let prep = frame.target("pencil.prep", PREP_FORMAT);

        // Per entity, only the entities' pixels change (#385): the draw
        // pass runs inside their rect, and the prep inside it grown by
        // how far a styled pixel reads the prep
        let size = [frame.width, frame.height];
        let draw_rect = frame.styled_rect(0);
        let prep_rect = draw_rect.map(|r| {
            r.padded(
                prep_reach(&rows, uniforms.pixels_per_point),
                size[0],
                size[1],
            )
        });
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
            frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
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
        kit::scissored_pass(
            encoder,
            "Pencil Pass",
            output,
            size,
            &self.draw_pipeline,
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
    fn a_row_is_nine_vec4s() {
        assert_eq!(std::mem::size_of::<PencilRow>(), 144);
        assert_eq!(
            std::mem::size_of::<PencilUniforms>(),
            64 + 16 + 16 + 16 + 16 + 16 + MAX_VARIANTS * 144 + MAX_IDS * 4
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
        for p in STYLE.params {
            assert!(
                reached.iter().any(|r| r == p.name),
                "{} not in the schema",
                p.name
            );
        }
        for o in STYLE.options {
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
    }

    #[test]
    fn one_image_a_frame_the_scene_tables_first() {
        let config = crate::postprocess::PostProcessConfig::default();
        let p = |n: &str| STYLE.param(&config, n);
        let named = |paper: &'static str| {
            let config = crate::postprocess::PostProcessConfig::default();
            move |n: &str| match n {
                "paper_image" => paper.to_string(),
                o => STYLE.option(&config, o).to_string(),
            }
        };
        let mut rows = vec![
            row(&p, &named("none"), 0.0, [0.0; 2], 0.0),
            row(&p, &named("cold_press"), 0.0, [0.0; 2], 0.0),
            row(&p, &named("hot_press"), 0.0, [0.0; 2], 0.0),
        ];
        let names: Vec<String> = ["none", "cold_press", "hot_press"].map(String::from).into();
        let (chosen, refused) = choose_image(&names, &mut rows, 2);
        assert_eq!(
            chosen, "cold_press",
            "the scene names none: the first entity's"
        );
        assert!(refused);
        assert_eq!(
            rows.iter().map(|r| r.v[7][2]).collect::<Vec<_>>(),
            vec![0.0, 1.0, 0.0],
            "the other paper draws procedurally"
        );
        assert!(names_image("cold_press") && !names_image("none") && !names_image(""));
    }
}
