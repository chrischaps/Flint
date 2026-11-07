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
//! wobble = 1.5           # how far lines wander, points
//! sketchy = 0.6          # a second, offset outline
//! grain = 0.5            # paper grain, and how much it breaks strokes
//! paper = [0.95, 0.93, 0.88]   # a 3-array fills paper_r/g/b
//! lead = [0.16, 0.16, 0.19]
//! color = 0.0            # 1 draws over the scene's colour instead of paper
//! seed = 0               # a different hand; the same seed draws the same picture
//! boil = 0.0             # redraws a second (0 = still)
//! ```
//!
//! It runs at the display stage, on the finished picture at output
//! resolution, so strokes stay crisp under a render scale; sizes are in
//! points (times [`StyleFrame::pixels_per_point`]). Two passes: the first
//! writes each pixel's tone, contour strength and local stroke direction
//! into a scratch target; the second blurs the direction into a flow and
//! draws strokes, outlines and paper from it.
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
//! ## Per entity
//!
//! As ASCII (#266): an entity with a `pencil_style` component, or a
//! `style_mask { style = "pencil" }`, is drawn in pencil while the rest of
//! the frame passes through, and each distinct set of settings is a row of
//! the variant table the passes index by mask id. `scope = "entities"`
//! keeps a scene table as those entities' preset, and `grid = "object"`
//! pins an entity's hatching to its projected origin, so its strokes
//! travel with it rather than swimming over it.

use super::kit;
use super::{
    StyleDescriptor, StyleFrame, StyleHint, StyleInput, StyleOption, StyleParam, StylePass,
    StyleStage,
};
use bytemuck::{Pod, Zeroable};
use std::sync::{Arc, Mutex};

/// The stroke direction modes, by their option spelling.
pub const DIRECTIONS: &[&str] = &["surface", "screen", "tensor"];

/// What a `[post_process.styles.pencil]` table covers (as ASCII's).
pub const SCOPES: &[&str] = &["screen", "entities"];

/// Where a variant's hatching is anchored (as ASCII's grids).
pub const GRIDS: &[&str] = &["screen", "object"];

/// Rows one frame's variant table holds, the whole frame's included.
pub const MAX_VARIANTS: usize = 64;

/// Mask ids the table covers.
pub const MAX_IDS: usize = 256;

/// The scratch target pass 1 writes: tone, contour, flow.
pub const PREP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

const fn param(name: &'static str, default: f32, min: f32, max: f32) -> StyleParam {
    StyleParam {
        name,
        default,
        min,
        max,
    }
}

pub(super) static STYLE: StyleDescriptor = StyleDescriptor {
    name: "pencil",
    label: "Pencil",
    stage: StyleStage::Display,
    params: &[
        param("spacing", 6.0, 2.0, 24.0),
        param("thickness", 1.0, 0.25, 4.0),
        param("layers", 3.0, 0.0, 4.0),
        param("angle", 50.0, 0.0, 180.0),
        param("brightness", 0.0, -1.0, 1.0),
        param("contrast", 1.2, 0.0, 4.0),
        param("darkness", 0.85, 0.0, 1.0),
        param("shade", 0.25, 0.0, 1.0),
        param("contour", 1.0, 0.0, 2.0),
        param("contour_width", 1.25, 0.5, 4.0),
        param("depth", 1.0, 0.0, 4.0),
        param("crease", 1.0, 0.0, 4.0),
        param("detail", 0.35, 0.0, 2.0),
        param("wobble", 1.5, 0.0, 8.0),
        param("sketchy", 0.6, 0.0, 1.0),
        param("grain", 0.5, 0.0, 1.0),
        param("color", 0.0, 0.0, 1.0),
        param("seed", 0.0, 0.0, 9999.0),
        param("boil", 0.0, 0.0, 24.0),
        param("paper_r", 0.95, 0.0, 1.0),
        param("paper_g", 0.93, 0.0, 1.0),
        param("paper_b", 0.88, 0.0, 1.0),
        param("lead_r", 0.16, 0.0, 1.0),
        param("lead_g", 0.16, 0.0, 1.0),
        param("lead_b", 0.19, 0.0, 1.0),
    ],
    options: &[
        StyleOption {
            name: "direction",
            default: "surface",
            choices: DIRECTIONS,
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
    needs: &[],
    hints: &[
        StyleHint::new("direction", "Stroke direction", "surface: round the form, along iso-depth lines; screen: all at the angle; tensor: along the picture's edges"),
        StyleHint::new("scope", "Scope", "screen: the table draws the whole frame; entities: it is only the preset for pencil_style entities"),
        StyleHint::new("grid", "Hatch anchor", "screen: hatching fixed to the screen; object: pinned to each entity, so strokes travel with it"),
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
    ],
    component: Some("pencil_style"),
    create: |d| {
        let device = d.device;
        let uniform_bgl = kit::uniform_layout(device, "Pencil Uniform BGL");
        let input_bgl = kit::sampled_layout(device, "Pencil Input BGL");
        let prep_bgl = kit::sampled_layout(device, "Pencil Prep BGL");
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
        Box::new(Pencil {
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
            warned: Mutex::new(false),
        })
    },
};

const PREP_SHADER: &str = concat!(
    include_str!("pencil_common.wgsl"),
    include_str!("pencil_prep.wgsl")
);
const DRAW_SHADER: &str = concat!(
    include_str!("pencil_common.wgsl"),
    include_str!("pencil.wgsl")
);

/// One variant's settings, seven vec4s (see `PencilRow` in the shader).
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug, PartialEq)]
pub struct PencilRow {
    pub v: [[f32; 4]; 7],
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
    rows: [PencilRow; MAX_VARIANTS],
    slots: [[u32; 4]; MAX_IDS / 4],
}

/// A row's flag: leave its pixels as they are.
const PASS_THROUGH: f32 = 1.0;

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
/// changes in steps, the same steps at the same `--style-time`.
pub fn effective_seed(seed: f32, boil: f32, time: f32) -> f32 {
    let base = seed.max(0.0).floor();
    if boil > 0.0 && time.is_finite() {
        base + (time.max(0.0) * boil).floor()
    } else {
        base
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
                flags,
                param("darkness").clamp(0.0, 1.0),
            ],
        ],
    }
}

struct Pencil {
    prep_pipeline: wgpu::RenderPipeline,
    draw_pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    uniform_bgl: wgpu::BindGroupLayout,
    input_bgl: wgpu::BindGroupLayout,
    prep_bgl: wgpu::BindGroupLayout,
    geometry_bgl: wgpu::BindGroupLayout,
    _no_mask: wgpu::Texture,
    no_mask_view: wgpu::TextureView,
    /// Whether the too-many-variants warning has been given.
    warned: Mutex<bool>,
}

impl Pencil {
    /// The depth and the mask (or the one-pixel stand-in).
    fn geometry_bind(&self, frame: &StyleFrame<'_>, with_mask: bool) -> Arc<wgpu::BindGroup> {
        let mask = frame.mask.filter(|_| with_mask);
        frame.depth_bind("Pencil Geometry BG", mask.is_some() as u64, || {
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
                ],
            })
        })
    }

    /// Row 0 (the whole frame's) and a row for each per-entity variant,
    /// with the row of every mask id.
    fn variant_table(&self, frame: &StyleFrame<'_>) -> (Vec<PencilRow>, [u32; MAX_IDS]) {
        let whole = row(
            &|n| frame.param(n),
            &|n| frame.option(n).to_string(),
            frame.time,
            [0.0, 0.0],
            0.0,
        );
        let mut rows = vec![whole];
        let mut slots = [0u32; MAX_IDS];
        // Ids another style draws are that style's pixels
        for (id, _) in frame.style_ids.iter() {
            let Some(v) = frame.style_ids.variant(id) else {
                continue;
            };
            if (id as usize) < MAX_IDS
                && v.style != STYLE.name
                && !v.off
                && super::find(&v.style).is_some()
            {
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
            rows.push(row(
                &|n| frame.variant_param(v, n),
                &|n| frame.variant_option(v, n).to_string(),
                frame.time,
                origin,
                flags,
            ));
        }
        (rows, slots)
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
        let (rows, slots) = self.variant_table(frame);
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
        let geometry = self.geometry_bind(frame, with_mask);
        let prep = frame.target("pencil.prep", PREP_FORMAT);

        kit::fullscreen_pass(
            encoder,
            "Pencil Prep",
            &prep.view,
            &self.prep_pipeline,
            &[&uniform, &input_bind, &geometry],
        );
        kit::fullscreen_pass(
            encoder,
            "Pencil Pass",
            output,
            &self.draw_pipeline,
            &[
                &uniform,
                &input_bind,
                &geometry,
                &frame.sampled("Pencil Prep BG", &self.prep_bgl, &prep.view),
            ],
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
        for (source, name) in [(PREP_SHADER, "prep"), (DRAW_SHADER, "draw")] {
            let module = validate(source, name);
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
    fn a_row_is_seven_vec4s() {
        assert_eq!(std::mem::size_of::<PencilRow>(), 112);
        assert_eq!(
            std::mem::size_of::<PencilUniforms>(),
            64 + 16 + 16 + 16 + 16 + MAX_VARIANTS * 112 + MAX_IDS * 4
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
    /// parameter, an option, a 3-array of `_r/_g/_b` parameters or
    /// `enabled`; every parameter and option can be set from it; and no
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
            if key == "enabled" {
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
        assert_eq!(&r.v[4][..3], &[0.95, 0.93, 0.88], "paper");
        assert_eq!([r.v[6][0], r.v[6][1], r.v[6][2]], [3.0, 4.0, 1.0]);
    }
}
