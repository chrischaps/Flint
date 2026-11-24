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
//! paper_image = "cold_press"  # papers/cold_press.png (#390); "none" = procedural
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
//! `paper_image` (#390) lays the paint on a paper image instead of the
//! procedural one, found and measured as pencil's is
//! ([`images`](super::images)): one texel every `grain_scale` points, its
//! luma against its own mean and spread the paper's height (so `grain` and
//! `granulation` read it as they read the noise), its hue tinting `paper`.
//! Each entity may paint on its own (#547): up to
//! [`MAX_LAYERS`](super::images::MAX_LAYERS) papers a frame, stacked in
//! one texture array; a row naming one more paints on the procedural one.
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

use crate::gpu_counts::{CountedDevice, CountedQueue};
use super::images::{ImageStack, ImageUse, LayerInfo, StackedImages, MAX_LAYERS};
use super::kit;
use super::variant_table::{report_drop, storage_entry, VariantTable};
use super::{
    StyleDescriptor, StyleDevice, StyleFrame, StyleInput, StyleOption, StyleParam,
    StylePass, StyleStage,
};
use bytemuck::{Pod, Zeroable};
use std::sync::{Arc, Mutex};

/// A row's flag: leave these pixels alone (an opt-out variant).
const PASS_THROUGH: u32 = 1;

/// The scopes, by their option spelling.
pub const SCOPES: &[&str] = &["screen", "entities"];

/// The largest wash radius, in pixels, the blur covers.
const MAX_WASH_PX: f32 = 48.0;

const PARAMS: &[StyleParam] = &[
    StyleParam::new("wash", 4.0, 0.0..=16.0)
        .label("Wash (pt)")
        .help("How far pigment diffuses, in points; one radius serves the frame, the largest any entity asks for"),
    StyleParam::new("bleed", 0.55, 0.0..=1.0)
        .label("Bleed")
        .help("How much of the colour is the diffused wash (0 crisp, 1 all wash)"),
    StyleParam::new("wobble", 2.0, 0.0..=8.0)
        .label("Wobble (pt)")
        .help("How far borders and silhouettes wander"),
    StyleParam::new("wobble_scale", 40.0, 8.0..=200.0)
        .label("Wobble wavelength (pt)")
        .help("The wander's wavelength"),
    StyleParam::new("edge", 1.0, 0.0..=4.0)
        .label("Edge pooling")
        .help("Pigment pooled darker at colour edges and glaze borders"),
    StyleParam::new("edge_depth", 0.8, 0.0..=2.0)
        .label("Silhouette pooling")
        .help("Pigment pooled on the near side of silhouettes, from depth"),
    StyleParam::new("edge_width", 1.5, 0.5..=6.0)
        .label("Silhouette rim (pt)")
        .help("How wide that rim is"),
    StyleParam::new("turbulence", 0.35, 0.0..=1.0)
        .label("Turbulence")
        .help("Low-frequency blotches of thicker and thinner pigment"),
    StyleParam::new("granulation", 0.5, 0.0..=2.0)
        .label("Granulation")
        .help("Pigment settling into the paper's valleys, strongest in dark washes"),
    StyleParam::new("grain", 0.35, 0.0..=1.0)
        .label("Paper relief")
        .help("The paper's relief, shading even bare paper"),
    StyleParam::new("grain_scale", 1.0, 0.25..=4.0).label("Paper tooth").help("Bigger is coarser"),
    StyleParam::new("pigment", 1.0, 0.2..=2.0)
        .label("Pigment")
        .help("Overall density: below 1 dilutes, above 1 deepens"),
    StyleParam::new("layers", 4.0, 0.0..=12.0)
        .label("Glazes")
        .help("The tone in this many steps, each edged darker where it dried; 0 is one smooth wash")
        .integer(),
    StyleParam::new("paper_r", 0.97, 0.0..=1.0).label("Paper").help("The paper's colour"),
    StyleParam::new("paper_g", 0.94, 0.0..=1.0),
    StyleParam::new("paper_b", 0.87, 0.0..=1.0),
];

const OPTIONS: &[StyleOption] = &[
    StyleOption::new("scope", "screen", SCOPES)
        .label("Scope")
        .help("screen: the table paints the whole frame; entities: it is only the preset for watercolor_style entities"),
    StyleOption::new("paper_image", super::pencil::NO_IMAGE, &[])
        .discovered(super::pencil::paper_choices)
        .label("Paper image")
        .help("A scanned paper (papers/<name>.png), a texel every grain_scale points: its relief is the grain, its hue tints the paper; none = procedural"),
];

pub(super) static STYLE: StyleDescriptor =
    StyleDescriptor::new("watercolor", "Watercolor", StyleStage::Display, create)
        .params(PARAMS)
        .options(OPTIONS)
        .component("watercolor_style");

fn create(d: &StyleDevice) -> Box<dyn StylePass> {
    Box::new(Watercolor::new(d))
}

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
    /// 1 when this row paints on the paper image (#390).
    image: f32,
    _pad: [f32; 3],
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
    /// Each layer of the paper stack (#547): its measures and size.
    papers: [LayerInfo; MAX_LAYERS],
}

struct Watercolor {
    wash_pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    paint_pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    /// `fs_passthrough` (#538): the paint pass's unpainted pixel, drawn
    /// around the per-entity rect the paint pass is scissored to.
    passthrough_pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    uniform_bgl: wgpu::BindGroupLayout,
    /// The paint pass's group 0: its uniform and the variant table (#740).
    paint_bgl: wgpu::BindGroupLayout,
    sampled_bgl: wgpu::BindGroupLayout,
    entity_bgl: wgpu::BindGroupLayout,
    wash_h: wgpu::Buffer,
    wash_v: wgpu::Buffer,
    uniforms: wgpu::Buffer,
    _no_mask: wgpu::Texture,
    no_mask_view: wgpu::TextureView,
    /// The variant table (#740): grows with the frame.
    table: Mutex<VariantTable>,
    paper: Mutex<ImageStack>,
}

impl Watercolor {
    fn new(d: &super::StyleDevice) -> Self {
        let device = d.device;
        let uniform_bgl = kit::uniform_layout(device, "Watercolor Uniform BGL");
        let paint_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Watercolor Paint BGL"),
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
                super::pencil::image_stack_entry(2),
            ],
        });
        let module = kit::shader(device, "Watercolor Shader", PAINT_SHADER);
        let wash_module = kit::shader(
            device,
            "Watercolor Wash Shader",
            kit::wgsl!("watercolor_wash.wgsl"),
        );
        // A one-texel mask of id 0, bound while no entity asks
        let no_mask = device.create_texture_counted(&wgpu::TextureDescriptor {
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
                &[&paint_bgl, &sampled_bgl, &sampled_bgl, &entity_bgl],
                d.format,
            ),
            passthrough_pipeline: kit::fullscreen_pipeline_entry(
                device,
                "Watercolor Passthrough Pipeline",
                &module,
                "fs_passthrough",
                &[&paint_bgl, &sampled_bgl],
                d.format,
            ),
            wash_h: kit::uniform_buffer::<WashUniforms>(device, "Watercolor Wash H Uniforms"),
            wash_v: kit::uniform_buffer::<WashUniforms>(device, "Watercolor Wash V Uniforms"),
            uniforms: kit::uniform_buffer::<PaintUniforms>(device, "Watercolor Uniforms"),
            uniform_bgl,
            paint_bgl,
            sampled_bgl,
            entity_bgl,
            _no_mask: no_mask,
            no_mask_view,
            table: Mutex::new(VariantTable::new(
                device,
                "Watercolor",
                std::mem::size_of::<RowUniform>(),
            )),
            paper: Mutex::new(ImageStack::new(ImageUse::Paper, "watercolor paper")),
        }
    }

    /// The mask (or the stand-in) and the depth, bound together.
    fn entity_bind(
        &self,
        frame: &StyleFrame<'_>,
        with_mask: bool,
        paper: &StackedImages,
    ) -> Arc<wgpu::BindGroup> {
        let mask = frame.mask.filter(|_| with_mask);
        let key = mask.is_some() as u64 | paper.generation << 1;
        frame.depth_bind("Watercolor Entity BG", key, || {
            frame.device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
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
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&paper.view),
                    },
                ],
            })
        })
    }
}

const PAINT_SHADER: kit::Wgsl = kit::wgsl!(
    "prelude_core.wgsl",
    "style_image_layer.wgsl",
    "noise.wgsl",
    "mask_read.wgsl",
    "mask_slot.wgsl",
    "watercolor.wgsl",
    "style_image.wgsl"
);

/// A row from a settings source: the scene's (`param` = the frame's) or a
/// variant's. Its `image` layer is set later, once the frame's papers
/// are stacked.
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
        image: 0.0,
        _pad: [0.0; 3],
    }
}

impl Watercolor {
    /// The paint pass's group 0: its uniform and this frame's variant
    /// table, written first.
    fn paint_bind(
        &self,
        frame: &StyleFrame<'_>,
        rows: &[RowUniform],
        slots: &[u32],
    ) -> Arc<wgpu::BindGroup> {
        let mut table = self.table.lock().unwrap_or_else(|e| e.into_inner());
        table.upload(frame.device, frame.queue, rows, slots);
        let [rows, slots] = table.entries(1);
        frame.bind("Watercolor Uniform BG", table.generation(), || {
            frame.device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                label: Some("Watercolor Uniform BG"),
                layout: &self.paint_bgl,
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

impl StylePass for Watercolor {
    fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    ) {
        // The variant table: row 0 the scene's, then each entity variant
        // (#740: every variant gets a row, every id a slot)
        let mut rows = vec![row(|n| frame.param(n), false)];
        let mut papers = vec![frame.option("paper_image").to_string()];
        let mut ids = vec![0u16];
        let mut slots = vec![0u32; frame.style_ids.len() + 1];
        let mut wash = frame.param("wash");
        for (id, v) in frame.variants() {
            ids.push(id);
            if !v.off {
                wash = wash.max(frame.variant_param(v, "wash"));
            }
            slots[id as usize] = rows.len() as u32;
            rows.push(row(|n| frame.variant_param(v, n), v.off));
            papers.push(frame.variant_option(v, "paper_image").to_string());
        }
        // The frame's papers (#390), a layer each up to MAX_LAYERS (#547): a
        // row paints on its own, past the cap (or a miss) on noise
        let stacked = self.paper.lock().unwrap_or_else(|e| e.into_inner()).get(
            frame.device,
            frame.queue,
            frame.assets,
            &papers,
            frame.frame.is_multiple_of(15),
        );
        for (r, layer) in rows.iter_mut().zip(&stacked.layer) {
            r.image = *layer as f32;
        }
        // The first entity past the cap is said (#740)
        if let Some(i) = stacked.over_cap {
            report_drop(
                "watercolor",
                "paper images",
                MAX_LAYERS,
                frame.style_ids.entity(ids[i]).unwrap_or("?"),
                "painted on procedural paper",
            );
        }
        let paper = stacked.images;
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
            frame.queue.write_buffer_counted(buffer, 0, bytemuck::bytes_of(&u));
        }
        // Per entity (#538), each pass runs only where what follows reads
        // it: the paint inside its entities' rect, the wash inside the
        // paint's reads, the first blur inside the second's
        let ppp = frame.pixels_per_point.max(0.25);
        let reach = Reach::of(&rows, ppp, radius);
        let size = [frame.width, frame.height];
        let paint_rect = frame.styled_rect(reach.paint);
        let wash_rect = paint_rect.map(|r| r.padded(reach.wash, frame.width, frame.height));
        let half_rect = wash_rect.map(|r| r.padded(reach.blur, frame.width, frame.height));
        let half = frame.target("watercolor.wash.h", frame.format);
        let washed = frame.target("watercolor.wash", frame.format);
        kit::fullscreen_pass_in(
            encoder,
            "Watercolor Wash H",
            &half.view,
            size,
            &self.wash_pipeline,
            &[
                &frame.uniform("Watercolor Wash H BG", &self.uniform_bgl, &self.wash_h),
                &frame.sampled_input("Watercolor Input BG", &self.sampled_bgl, input),
            ],
            half_rect,
        );
        kit::fullscreen_pass_in(
            encoder,
            "Watercolor Wash V",
            &washed.view,
            size,
            &self.wash_pipeline,
            &[
                &frame.uniform("Watercolor Wash V BG", &self.uniform_bgl, &self.wash_v),
                &frame.sampled(
                    "Watercolor Wash H Texture BG",
                    &self.sampled_bgl,
                    &half.view,
                ),
            ],
            wash_rect,
        );

        // Pass 3: paint
        let uniforms = PaintUniforms {
            size: [frame.width as f32, frame.height as f32],
            pixels_per_point: frame.pixels_per_point.max(0.25),
            srgb: frame.srgb() as u32,
            row_count: rows.len() as u32,
            passthrough: passthrough as u32,
            near: frame.camera.near,
            far: frame.camera.far,
            papers: paper.layers,
        };
        frame
            .queue
            .write_buffer_counted(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
        let paint_bind = self.paint_bind(frame, &rows, &slots);
        let input_bind =
            frame.sampled_input("Watercolor Paint Input BG", &self.sampled_bgl, input);
        kit::scissored_pass_around(
            encoder,
            "Watercolor Pass",
            output,
            size,
            (
                &self.paint_pipeline,
                &[
                    &paint_bind,
                    &input_bind,
                    &frame.sampled(
                        "Watercolor Wash Texture BG",
                        &self.sampled_bgl,
                        &washed.view,
                    ),
                    &self.entity_bind(frame, with_mask, &paper),
                ],
            ),
            paint_rect,
            (&self.passthrough_pipeline, &[&paint_bind, &input_bind]),
        );
    }
}

/// How far, in pixels, each of watercolor's passes reads past the pixels
/// it serves (#538), so a per-entity frame scissors them without changing
/// a byte.
#[derive(Debug, PartialEq)]
struct Reach {
    /// The paint pass: a pixel takes the paint of the mask read at its
    /// wobbled position, up to `wobble` points away on each axis, so a
    /// painted silhouette wanders that far out over its surroundings.
    paint: u32,
    /// The wash, read by a painted pixel at its own wobbled position and
    /// 1.5 points either side of it (the glazes' slope), filtered.
    wash: u32,
    /// The first blur, read by the second up to two radii away, filtered.
    blur: u32,
}

impl Reach {
    fn of(rows: &[RowUniform], pixels_per_point: f32, radius: f32) -> Self {
        let wobble = rows.iter().map(|r| r.wobble).fold(0.0f32, f32::max);
        let px = |points: f32| (points * pixels_per_point).ceil() as u32;
        Self {
            paint: px(wobble) + 1,
            wash: px(wobble + 1.5) + 2,
            blur: (2.0 * radius).ceil() as u32 + 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniforms_match_the_shader_layout() {
        assert_eq!(std::mem::size_of::<RowUniform>(), 80);
        assert_eq!(std::mem::size_of::<WashUniforms>(), 32);
        assert_eq!(std::mem::size_of::<PaintUniforms>(), 32 + 4 * 32);
    }

    #[test]
    fn the_paint_shader_validates_and_its_uniforms_match_rust() {
        let module = naga::front::wgsl::parse_str(PAINT_SHADER.text)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(PAINT_SHADER.text)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("watercolor shader validates");
        let (_, u) = module
            .global_variables
            .iter()
            .find(|(_, v)| v.name.as_deref() == Some("params"))
            .expect("uniform params");
        let size = module.types[u.ty].inner.size(module.to_ctx());
        assert_eq!(size as usize, std::mem::size_of::<PaintUniforms>());
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

    /// #538: the paint reaches as far as the widest wobble any row has
    /// (an opt-out's and the scene's too: an unpainted pixel wobbles by its
    /// own row), the wash that plus the glazes' 1.5 pt, the first blur two
    /// radii; each rounded out and grown for the filter.
    #[test]
    fn each_pass_reaches_as_far_as_what_reads_it() {
        let config = crate::postprocess::PostProcessConfig::default();
        let with = |wobble: f32| {
            let mut r = row(|n| STYLE.param(&config, n), false);
            r.wobble = wobble;
            r
        };
        let reach = Reach::of(&[with(2.0), with(0.5)], 1.0, 4.0);
        assert_eq!(
            reach,
            Reach {
                paint: 3,
                wash: 6,
                blur: 10
            }
        );
        let wide = Reach::of(&[with(0.0), with(2.5)], 2.0, 9.6);
        assert_eq!(wide.paint, 6);
        assert_eq!(wide.wash, 10);
        assert_eq!(wide.blur, 22);
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
