//! Watercolor (#272): the picture as transparent pigment on paper. Colour
//! bleeds into soft washes, their borders wobble, pigment pools darker
//! along the edges of each wash and settles into the paper's grain.
//!
//! ```toml
//! [post_process.styles.watercolor]
//! wash = 4.0          # how far pigment diffuses, in points (the blur radius)
//! bleed = 0.55        # how much of the colour is the diffused wash
//! wobble = 2.0        # how far wash borders wander, in points
//! wobble_scale = 40   # the wander's wavelength, in points
//! edge = 1.0          # pigment pooling where colour changes (edge darkening)
//! edge_depth = 0.8    # pooling along silhouettes, from depth
//! edge_width = 1.5    # how wide the pooled rim is, in points
//! turbulence = 0.35   # low-frequency blotches of thicker and thinner pigment
//! granulation = 0.5   # pigment settling into the paper's valleys
//! grain = 0.35        # the paper's own relief, shading even bare paper
//! grain_scale = 1.0   # the paper's tooth (bigger = coarser)
//! pigment = 1.0       # overall density: < 1 dilutes, > 1 deepens
//! layers = 4          # glazes: tone steps, each edged darker (0 = smooth)
//! paper_r = 0.97      # the paper's colour (a component's `paper = [r, g, b]`)
//! scope = "screen"    # or "entities": a preset for watercolor_style entities
//! ```
//!
//! It runs at the display stage, on display-referred colour, because it
//! paints on paper: the paper and its grain must not be fogged, bloomed or
//! tonemapped as if they were in the world. Three passes: the wash (a
//! separable Gaussian of the picture, horizontal then vertical, in two
//! targets of its own), then the paint pass, which wobbles where it reads
//! the picture, the wash and the mask, mixes the wash in, darkens pigment
//! at colour and depth edges, modulates its density with turbulence and
//! granulation (Bousseau et al.'s `C - (C - C^2)(d - 1)`), and lays the
//! result on the paper.
//!
//! The paper lives in screen space, in points, so its grain stays put as
//! the camera moves (the paper doesn't travel with the world), and the
//! same size to the eye on a high-DPI screen.
//!
//! ## Per entity
//!
//! An entity with a `watercolor_style` component (or a `style_mask` naming
//! `watercolor`) is painted with no table at all, the rest of the frame
//! passing through, exactly as ASCII does it (#266); `scope = "entities"`
//! keeps a table as their preset. The mask is read at the wobbled position,
//! so a painted entity's silhouette wanders out over its surroundings like
//! a real wash's. The wash radius is one per frame, the largest any row
//! asks for; every other setting is per entity.
//!
//! ## With Kuwahara
//!
//! They chain: Kuwahara runs at the HDR stage and flattens the scene into
//! brush pools, watercolor then paints those pools on paper. Neither needs
//! the other, and they share nothing (Kuwahara's structure tensor is an
//! HDR-stage intermediate; the wash is the only smoothing watercolor does).

use super::kit;
use super::{
    StyleDescriptor, StyleFrame, StyleHint, StyleInput, StyleOption, StyleParam, StylePass,
    StyleStage,
};
use bytemuck::{Pod, Zeroable};
use std::sync::Arc;

/// Variant rows the uniform holds; row 0 is the scene's.
pub const MAX_VARIANTS: usize = 16;

/// Mask ids the slot table covers.
pub const MAX_IDS: usize = 256;

/// A row's flag: leave these pixels alone (an opt-out variant).
const PASS_THROUGH: u32 = 1;

/// The scopes, by their option spelling.
pub const SCOPES: &[&str] = &["screen", "entities"];

/// The largest wash radius, in pixels, the blur covers.
const MAX_WASH_PX: f32 = 48.0;

const fn param(name: &'static str, default: f32, min: f32, max: f32) -> StyleParam {
    StyleParam {
        name,
        default,
        min,
        max,
    }
}

pub(super) static STYLE: StyleDescriptor = StyleDescriptor {
    name: "watercolor",
    label: "Watercolor",
    stage: StyleStage::Display,
    params: &[
        param("wash", 4.0, 0.0, 16.0),
        param("bleed", 0.55, 0.0, 1.0),
        param("wobble", 2.0, 0.0, 8.0),
        param("wobble_scale", 40.0, 8.0, 200.0),
        param("edge", 1.0, 0.0, 4.0),
        param("edge_depth", 0.8, 0.0, 2.0),
        param("edge_width", 1.5, 0.5, 6.0),
        param("turbulence", 0.35, 0.0, 1.0),
        param("granulation", 0.5, 0.0, 2.0),
        param("grain", 0.35, 0.0, 1.0),
        param("grain_scale", 1.0, 0.25, 4.0),
        param("pigment", 1.0, 0.2, 2.0),
        param("layers", 4.0, 0.0, 12.0),
        param("paper_r", 0.97, 0.0, 1.0),
        param("paper_g", 0.94, 0.0, 1.0),
        param("paper_b", 0.87, 0.0, 1.0),
    ],
    options: &[StyleOption {
        name: "scope",
        default: "screen",
        choices: SCOPES,
    }],
    enabled: None,
    uses_mask: false,
    hints: &[
        StyleHint::new("scope", "Scope", "screen: the table paints the whole frame; entities: it is only the preset for watercolor_style entities"),
        StyleHint::new("wash", "Wash (pt)", "How far pigment diffuses, in points; one radius serves the frame, the largest any entity asks for"),
        StyleHint::new("bleed", "Bleed", "How much of the colour is the diffused wash (0 crisp, 1 all wash)"),
        StyleHint::new("layers", "Glazes", "The tone in this many steps, each edged darker where it dried; 0 is one smooth wash").integer(),
        StyleHint::new("wobble", "Wobble (pt)", "How far borders and silhouettes wander"),
        StyleHint::new("wobble_scale", "Wobble wavelength (pt)", "The wander's wavelength"),
        StyleHint::new("edge", "Edge pooling", "Pigment pooled darker at colour edges and glaze borders"),
        StyleHint::new("edge_depth", "Silhouette pooling", "Pigment pooled on the near side of silhouettes, from depth"),
        StyleHint::new("edge_width", "Silhouette rim (pt)", "How wide that rim is"),
        StyleHint::new("turbulence", "Turbulence", "Low-frequency blotches of thicker and thinner pigment"),
        StyleHint::new("granulation", "Granulation", "Pigment settling into the paper's valleys, strongest in dark washes"),
        StyleHint::new("grain", "Paper relief", "The paper's relief, shading even bare paper"),
        StyleHint::new("grain_scale", "Paper tooth", "Bigger is coarser"),
        StyleHint::new("pigment", "Pigment", "Overall density: below 1 dilutes, above 1 deepens"),
        StyleHint::new("paper", "Paper", "The paper's colour"),
    ],
    component: Some("watercolor_style"),
    create: |d| Box::new(Watercolor::new(d)),
};

/// One blur direction's uniforms.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct WashUniforms {
    texel: [f32; 2],
    dir: [f32; 2],
    radius: f32,
    _pad: [f32; 3],
}

/// One variant's settings: row 0 the scene's, then one a per-entity set.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable, PartialEq)]
struct RowUniform {
    wobble: f32,
    wobble_scale: f32,
    bleed: f32,
    edge: f32,
    edge_depth: f32,
    edge_width: f32,
    turbulence: f32,
    granulation: f32,
    grain: f32,
    grain_scale: f32,
    pigment: f32,
    flags: u32,
    paper: [f32; 3],
    layers: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct PaintUniforms {
    size: [f32; 2],
    pixels_per_point: f32,
    srgb: u32,
    /// Rows of `rows` in use.
    row_count: u32,
    /// 1 when the whole frame passes through (only entities asked).
    passthrough: u32,
    near: f32,
    far: f32,
    rows: [RowUniform; MAX_VARIANTS],
    /// The row of each mask id, four ids a vec4.
    slots: [[u32; 4]; MAX_IDS / 4],
}

struct Watercolor {
    wash_pipeline: wgpu::RenderPipeline,
    paint_pipeline: wgpu::RenderPipeline,
    uniform_bgl: wgpu::BindGroupLayout,
    sampled_bgl: wgpu::BindGroupLayout,
    entity_bgl: wgpu::BindGroupLayout,
    wash_h: wgpu::Buffer,
    wash_v: wgpu::Buffer,
    uniforms: wgpu::Buffer,
    _no_mask: wgpu::Texture,
    no_mask_view: wgpu::TextureView,
    warned: std::sync::atomic::AtomicBool,
}

impl Watercolor {
    fn new(d: &super::StyleDevice) -> Self {
        let device = d.device;
        let uniform_bgl = kit::uniform_layout(device, "Watercolor Uniform BGL");
        let sampled_bgl = kit::sampled_layout(device, "Watercolor Texture BGL");
        // The paint pass's mask and depth
        let entity_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Watercolor Entity BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let module = kit::shader(device, "Watercolor Shader", include_str!("watercolor.wgsl"));
        let wash_module = kit::shader(
            device,
            "Watercolor Wash Shader",
            include_str!("watercolor_wash.wgsl"),
        );
        // A one-texel mask of id 0, bound while no entity asks
        let no_mask = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Watercolor No Mask"),
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
        Self {
            wash_pipeline: kit::fullscreen_pipeline(
                device,
                "Watercolor Wash Pipeline",
                &wash_module,
                &[&uniform_bgl, &sampled_bgl],
                d.format,
            ),
            paint_pipeline: kit::fullscreen_pipeline(
                device,
                "Watercolor Pipeline",
                &module,
                &[&uniform_bgl, &sampled_bgl, &sampled_bgl, &entity_bgl],
                d.format,
            ),
            wash_h: kit::uniform_buffer::<WashUniforms>(device, "Watercolor Wash H Uniforms"),
            wash_v: kit::uniform_buffer::<WashUniforms>(device, "Watercolor Wash V Uniforms"),
            uniforms: kit::uniform_buffer::<PaintUniforms>(device, "Watercolor Uniforms"),
            uniform_bgl,
            sampled_bgl,
            entity_bgl,
            _no_mask: no_mask,
            no_mask_view,
            warned: Default::default(),
        }
    }

    /// The mask (or the stand-in) and the depth, bound together.
    fn entity_bind(&self, frame: &StyleFrame<'_>, with_mask: bool) -> Arc<wgpu::BindGroup> {
        let mask = frame.mask.filter(|_| with_mask);
        frame.depth_bind("Watercolor Entity BG", mask.is_some() as u64, || {
            frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Watercolor Entity BG"),
                layout: &self.entity_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(
                            mask.unwrap_or(&self.no_mask_view),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(frame.depth),
                    },
                ],
            })
        })
    }
}

/// A row from a settings source: the scene's (`param` = the frame's) or a
/// variant's.
fn row(param: impl Fn(&str) -> f32, off: bool) -> RowUniform {
    RowUniform {
        wobble: param("wobble").clamp(0.0, 16.0),
        wobble_scale: param("wobble_scale").max(1.0),
        bleed: param("bleed").clamp(0.0, 1.0),
        edge: param("edge").max(0.0),
        edge_depth: param("edge_depth").max(0.0),
        edge_width: param("edge_width").clamp(0.25, 16.0),
        turbulence: param("turbulence").clamp(0.0, 1.0),
        granulation: param("granulation").max(0.0),
        grain: param("grain").clamp(0.0, 1.0),
        grain_scale: param("grain_scale").clamp(0.05, 16.0),
        pigment: param("pigment").clamp(0.0, 4.0),
        flags: if off { PASS_THROUGH } else { 0 },
        paper: [
            param("paper_r").clamp(0.0, 1.0),
            param("paper_g").clamp(0.0, 1.0),
            param("paper_b").clamp(0.0, 1.0),
        ],
        layers: param("layers").clamp(0.0, 32.0),
    }
}

impl StylePass for Watercolor {
    fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    ) {
        // The variant table: row 0 the scene's, then each entity variant
        let mut rows = vec![row(|n| frame.param(n), false)];
        let mut slots = [0u32; MAX_IDS];
        let mut wash = frame.param("wash");
        for (id, v) in frame.variants() {
            if rows.len() == MAX_VARIANTS || id as usize >= MAX_IDS {
                if !self.warned.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    tracing::warn!(
                        "watercolor: more per-entity variants than the {} (or ids past \
                         {MAX_IDS}) one frame holds; the rest are painted like the scene",
                        MAX_VARIANTS - 1
                    );
                }
                continue;
            }
            if !v.off {
                wash = wash.max(frame.variant_param(v, "wash"));
            }
            slots[id as usize] = rows.len() as u32;
            rows.push(row(|n| frame.variant_param(v, n), v.off));
        }
        let with_mask = rows.len() > 1;
        let passthrough = !(frame.whole_frame() && frame.option("scope") != "entities");

        // Pass 1 and 2: the wash, a separable Gaussian
        let radius = (wash.max(0.0) * frame.pixels_per_point).min(MAX_WASH_PX);
        let texel = frame.texel_size();
        for (buffer, dir) in [(&self.wash_h, [1.0, 0.0]), (&self.wash_v, [0.0, 1.0])] {
            let u = WashUniforms {
                texel,
                dir,
                radius,
                _pad: [0.0; 3],
            };
            frame.queue.write_buffer(buffer, 0, bytemuck::bytes_of(&u));
        }
        let half = frame.target("watercolor.wash.h", frame.format);
        let washed = frame.target("watercolor.wash", frame.format);
        kit::fullscreen_pass(
            encoder,
            "Watercolor Wash H",
            &half.view,
            &self.wash_pipeline,
            &[
                &frame.uniform("Watercolor Wash H BG", &self.uniform_bgl, &self.wash_h),
                &frame.sampled_input("Watercolor Input BG", &self.sampled_bgl, input),
            ],
        );
        kit::fullscreen_pass(
            encoder,
            "Watercolor Wash V",
            &washed.view,
            &self.wash_pipeline,
            &[
                &frame.uniform("Watercolor Wash V BG", &self.uniform_bgl, &self.wash_v),
                &frame.sampled(
                    "Watercolor Wash H Texture BG",
                    &self.sampled_bgl,
                    &half.view,
                ),
            ],
        );

        // Pass 3: paint
        let mut uniforms = PaintUniforms {
            size: [frame.width as f32, frame.height as f32],
            pixels_per_point: frame.pixels_per_point.max(0.25),
            srgb: frame.srgb() as u32,
            row_count: rows.len() as u32,
            passthrough: passthrough as u32,
            near: frame.camera.near,
            far: frame.camera.far,
            rows: [RowUniform::zeroed(); MAX_VARIANTS],
            slots: [[0; 4]; MAX_IDS / 4],
        };
        uniforms.rows[..rows.len()].copy_from_slice(&rows);
        for (i, slot) in slots.iter().enumerate() {
            uniforms.slots[i / 4][i % 4] = *slot;
        }
        frame
            .queue
            .write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
        kit::fullscreen_pass(
            encoder,
            "Watercolor Pass",
            output,
            &self.paint_pipeline,
            &[
                &frame.uniform("Watercolor Uniform BG", &self.uniform_bgl, &self.uniforms),
                &frame.sampled_input("Watercolor Paint Input BG", &self.sampled_bgl, input),
                &frame.sampled(
                    "Watercolor Wash Texture BG",
                    &self.sampled_bgl,
                    &washed.view,
                ),
                &self.entity_bind(frame, with_mask),
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniforms_match_the_shader_layout() {
        assert_eq!(std::mem::size_of::<RowUniform>(), 64);
        assert_eq!(std::mem::size_of::<WashUniforms>(), 32);
        assert_eq!(
            std::mem::size_of::<PaintUniforms>(),
            32 + MAX_VARIANTS * 64 + MAX_IDS / 4 * 16
        );
    }

    #[test]
    fn rows_read_the_scene_then_the_variant() {
        let mut config = crate::postprocess::PostProcessConfig::default();
        config.styles.set_param("watercolor", "edge", 2.0);
        let mut v = crate::style_mask::StyleVariant::plain("watercolor");
        v.params.insert("paper_r".into(), 0.5);
        let r = row(|n| v.param(&STYLE, &config, n), false);
        assert_eq!(r.edge, 2.0, "the scene's table");
        assert_eq!(r.paper, [0.5, 0.94, 0.87], "the entity's, then defaults");
        assert_eq!(r.flags, 0);
        assert_eq!(row(|n| STYLE.param(&config, n), true).flags, PASS_THROUGH);
    }

    #[test]
    fn a_paper_triple_fills_the_paper_channels() {
        let table: toml::Table = toml::from_str("paper = [0.9, 0.8, 0.7]\nwobble = 3").unwrap();
        let mut v = crate::style_mask::StyleVariant::plain("watercolor");
        crate::style_mask::read_overrides(&STYLE, &table, &mut v);
        assert_eq!(v.params.get("paper_g"), Some(&0.8));
        assert_eq!(v.params.get("wobble"), Some(&3.0));
    }
}
