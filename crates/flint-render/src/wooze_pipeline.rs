//! Shallow wooze (#215): pipeline variants that displace a draw's vertices through
//! world-space noise on the wooze field's clock, for meshes with no poses of their
//! own to drift between.
//!
//! A scene that doesn't use it runs today's shaders and pays for none of this:
//! nothing here is compiled until an entity's draw asks for it, and ordinary draws
//! have no new uniform or branch in any shader. Only the vertex stage is new. It
//! pairs with the fragment stages already compiled, so a woozy surface shades as it
//! always has, in whichever blend mode it uses.
//!
//! The drift rides in the padding of the slot a draw already has: the 48 bytes past
//! [`TransformUniforms`] in a [`TransformArena`](crate::bind_cache) slot, and past
//! [`ShadowDrawUniforms`](crate::shadow::ShadowDrawUniforms) in the shadow pass's.
//! Normals aren't bent: at a few centimetres over a metre, it's about a degree or
//! two of shading for three times the noise.

use bytemuck::{Pod, Zeroable};

use crate::pipeline::{BlendMode, RenderPipeline, ADDITIVE_BLEND, MULTIPLY_BLEND};
use crate::primitives::{SkinnedVertex, Vertex};
use crate::skinned_pipeline::SkinnedPipeline;

/// The shallow wooze vertex stages, with the noise they share.
pub(crate) const VERTEX_SOURCE: &str = concat!(
    include_str!("wooze_noise.wgsl"),
    "\n",
    include_str!("wooze_vertex.wgsl")
);

/// The shadow pass's, with the same noise, so a shadow moves with what casts it.
pub(crate) const SHADOW_SOURCE: &str = concat!(
    include_str!("wooze_noise.wgsl"),
    "\n",
    include_str!("wooze_shadow_shader.wgsl")
);

/// The shortest wavelength, in metres. Under this it stops reading as drift and
/// starts tearing a mesh's triangles apart.
pub const WAVELENGTH_MIN: f32 = 0.05;

/// How far the noise's domain is wrapped, in lattice cells: far more than the
/// jump between targets, and small enough to keep its precision in an `f32`.
const DOMAIN: f64 = 256.0;

/// Where target `index` lives in the noise's domain. The R3 low-discrepancy
/// sequence, so no two targets lie close, and neighbours are many cells apart,
/// which makes each one a fresh shape. A pure function of the index, so everything
/// on the same beat reads the same field.
fn domain_offset(index: u64) -> [f32; 3] {
    const G: f64 = 1.220_744_084_605_759_5;
    let at = |divisor: f64| ((index as f64 / divisor).fract() * DOMAIN) as f32;
    [at(G), at(G * G), at(G * G * G)]
}

/// What one entity's draws do this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShallowWoozeDraw {
    /// Metres at full swing on each axis: what's authored, times the field's
    /// amplitude where the entity stands.
    pub amplitude: f32,
    /// Metres from one swell of the noise to the next. Far above the mesh's size it
    /// moves as a whole; near it, it changes shape.
    pub wavelength: f32,
    /// The target being held or left.
    pub index: u64,
    /// How far toward the next target, 0 to 1.
    pub weight: f32,
}

impl ShallowWoozeDraw {
    pub(crate) fn uniforms(&self) -> ShallowWoozeUniforms {
        let [ax, ay, az] = domain_offset(self.index);
        let [bx, by, bz] = domain_offset(self.index.wrapping_add(1));
        ShallowWoozeUniforms {
            leaving: [ax, ay, az, self.amplitude],
            arriving: [bx, by, bz, self.weight],
            params: [1.0 / self.wavelength.max(WAVELENGTH_MIN), 0.0, 0.0, 0.0],
        }
    }
}

/// `ShallowWooze` in the wooze shaders.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct ShallowWoozeUniforms {
    pub leaving: [f32; 4],
    pub arriving: [f32; 4],
    pub params: [f32; 4],
}

impl ShallowWoozeUniforms {
    /// How far any vertex can move along any axis: each axis of the noise stays
    /// within -1 to 1, so this is what a draw's bounds grow by.
    pub fn reach(&self) -> f32 {
        self.leaving[3].abs()
    }
}

/// Draw `(min, max)` bounds grown to hold whatever `uniforms` can do to a vertex.
pub(crate) fn grow_bounds(bounds: &mut ([f32; 3], [f32; 3]), uniforms: &ShallowWoozeUniforms) {
    let reach = uniforms.reach();
    for axis in 0..3 {
        bounds.0[axis] -= reach;
        bounds.1[axis] += reach;
    }
}

/// The woozy twin of each solid and blended pipeline of one kind of mesh.
pub(crate) struct WoozePipelineSet {
    pub opaque: wgpu::RenderPipeline,
    pub alpha_back: wgpu::RenderPipeline,
    pub alpha_front: wgpu::RenderPipeline,
    pub additive: wgpu::RenderPipeline,
    pub multiply: wgpu::RenderPipeline,
}

struct Stages<'a> {
    label: &'static str,
    layout: &'a wgpu::PipelineLayout,
    vertex_module: &'a wgpu::ShaderModule,
    vertex_entry: &'static str,
    vertex_layout: wgpu::VertexBufferLayout<'static>,
    fragment_module: &'a wgpu::ShaderModule,
    fragment_entry: &'static str,
    format: wgpu::TextureFormat,
    sample_count: u32,
}

impl Stages<'_> {
    fn pipeline(
        &self,
        device: &wgpu::Device,
        blend: wgpu::BlendState,
        cull_mode: Option<wgpu::Face>,
        depth_write_enabled: bool,
    ) -> wgpu::RenderPipeline {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(self.label),
            layout: Some(self.layout),
            vertex: wgpu::VertexState {
                module: self.vertex_module,
                entry_point: Some(self.vertex_entry),
                buffers: std::slice::from_ref(&self.vertex_layout),
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: self.fragment_module,
                entry_point: Some(self.fragment_entry),
                targets: &[Some(wgpu::ColorTargetState {
                    format: self.format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: self.sample_count,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        })
    }

    fn set(&self, device: &wgpu::Device) -> WoozePipelineSet {
        let alpha = wgpu::BlendState::ALPHA_BLENDING;
        WoozePipelineSet {
            opaque: self.pipeline(device, alpha, Some(wgpu::Face::Back), true),
            alpha_back: self.pipeline(device, alpha, Some(wgpu::Face::Front), false),
            alpha_front: self.pipeline(device, alpha, Some(wgpu::Face::Back), false),
            additive: self.pipeline(device, ADDITIVE_BLEND, None, false),
            multiply: self.pipeline(device, MULTIPLY_BLEND, None, false),
        }
    }
}

fn vertex_module(device: &wgpu::Device) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Shallow Wooze Vertex Shader"),
        source: wgpu::ShaderSource::Wgsl(VERTEX_SOURCE.into()),
    })
}

impl WoozePipelineSet {
    /// The set for standard meshes, over `base`'s fragment stage and layout.
    pub fn standard(device: &wgpu::Device, base: &RenderPipeline) -> Self {
        Stages {
            label: "Shallow Wooze Pipeline",
            layout: &base.pipeline_layout,
            vertex_module: &vertex_module(device),
            vertex_entry: "vs_wooze",
            vertex_layout: Vertex::desc(),
            fragment_module: &base.shader,
            fragment_entry: "fs_main",
            format: base.format,
            sample_count: base.sample_count,
        }
        .set(device)
    }

    /// The set for skinned meshes, over `skinned`'s fragment stage and layout.
    pub fn skinned(
        device: &wgpu::Device,
        base: &RenderPipeline,
        skinned: &SkinnedPipeline,
    ) -> Self {
        Stages {
            label: "Shallow Wooze Skinned Pipeline",
            layout: &skinned.pipeline_layout,
            vertex_module: &vertex_module(device),
            vertex_entry: "vs_skinned_wooze",
            vertex_layout: SkinnedVertex::desc(),
            fragment_module: &skinned.shader,
            fragment_entry: "fs_skinned",
            format: base.format,
            sample_count: base.sample_count,
        }
        .set(device)
    }

    /// The front-facing pipeline for a blended draw in `mode`.
    pub fn blended(&self, mode: BlendMode) -> &wgpu::RenderPipeline {
        match mode {
            BlendMode::Alpha => &self.alpha_front,
            BlendMode::Additive => &self.additive,
            BlendMode::Multiply => &self.multiply,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bind_cache::SLOT_STRIDE;
    use crate::pipeline::TransformUniforms;
    use crate::shadow::{ShadowDrawUniforms, SHADOW_DRAW_STRIDE};
    use std::mem::size_of;

    fn validated(source: &str, name: &str) -> naga::Module {
        let module = naga::front::wgsl::parse_str(source).expect(name);
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect(name);
        module
    }

    fn entry_stage_io(module: &naga::Module, entry: &str) -> Vec<(u32, naga::Scalar)> {
        let ep = module
            .entry_points
            .iter()
            .find(|e| e.name == entry)
            .unwrap_or_else(|| panic!("no entry point {entry}"));
        let ty = ep.function.result.as_ref().map(|r| r.ty);
        let mut io = Vec::new();
        let Some(ty) = ty else { return io };
        if let naga::TypeInner::Struct { members, .. } = &module.types[ty].inner {
            for member in members {
                if let Some(naga::Binding::Location { location, .. }) = member.binding {
                    if let Some(scalar) = module.types[member.ty].inner.scalar() {
                        io.push((location, scalar));
                    }
                }
            }
        }
        io
    }

    #[test]
    fn the_shaders_validate() {
        validated(VERTEX_SOURCE, "wooze_vertex.wgsl");
        validated(SHADOW_SOURCE, "wooze_shadow_shader.wgsl");
    }

    /// The variants hand their outputs to fragment stages they don't own.
    #[test]
    fn a_woozy_vertex_stage_hands_over_what_its_plain_twin_does() {
        let woozy = validated(VERTEX_SOURCE, "wooze_vertex.wgsl");
        let plain = naga::front::wgsl::parse_str(include_str!("shader.wgsl")).unwrap();
        let skinned = naga::front::wgsl::parse_str(include_str!("skinned_shader.wgsl")).unwrap();
        assert_eq!(
            entry_stage_io(&woozy, "vs_wooze"),
            entry_stage_io(&plain, "vs_main")
        );
        assert_eq!(
            entry_stage_io(&woozy, "vs_skinned_wooze"),
            entry_stage_io(&skinned, "vs_skinned")
        );
    }

    #[test]
    fn the_drift_fits_in_the_padding_of_the_slots_draws_already_have() {
        assert_eq!(size_of::<ShallowWoozeUniforms>(), 48);
        assert!(
            (size_of::<TransformUniforms>() + size_of::<ShallowWoozeUniforms>()) as u64
                <= SLOT_STRIDE
        );
        assert!(
            (size_of::<ShadowDrawUniforms>() + size_of::<ShallowWoozeUniforms>()) as u64
                <= SHADOW_DRAW_STRIDE
        );
        // WGSL aligns a struct of vec4s to 16 bytes, so it starts right past these.
        assert_eq!(size_of::<TransformUniforms>() % 16, 0);
        assert_eq!(size_of::<ShadowDrawUniforms>() % 16, 0);
    }

    #[test]
    fn every_target_is_somewhere_new_and_the_same_every_time() {
        for index in [0u64, 1, 7, 1000, 1 << 40] {
            let here = domain_offset(index);
            assert_eq!(here, domain_offset(index), "pure in its index");
            let next = domain_offset(index + 1);
            let far = (0..3).any(|axis| (here[axis] - next[axis]).abs() >= 2.0);
            assert!(far, "{here:?} then {next:?} share a lattice cell");
            assert!(here.iter().all(|v| (0.0..DOMAIN as f32).contains(v)));
        }
    }

    #[test]
    fn a_draw_says_what_its_noise_needs() {
        let draw = ShallowWoozeDraw {
            amplitude: 0.04,
            wavelength: 2.0,
            index: 5,
            weight: 0.25,
        };
        let uniforms = draw.uniforms();
        assert_eq!(uniforms.leaving[3], 0.04);
        assert_eq!(uniforms.arriving[3], 0.25);
        assert_eq!(uniforms.params[0], 0.5);
        assert_eq!(uniforms.leaving[..3], domain_offset(5));
        assert_eq!(uniforms.arriving[..3], domain_offset(6));
        // The target it arrives at is the one the next beat leaves from.
        let next = ShallowWoozeDraw { index: 6, ..draw }.uniforms();
        assert_eq!(uniforms.arriving[..3], next.leaving[..3]);
    }

    #[test]
    fn a_wavelength_too_short_to_draw_is_held_at_the_shortest() {
        let draw = ShallowWoozeDraw {
            amplitude: 0.04,
            wavelength: 0.0,
            index: 0,
            weight: 0.0,
        };
        assert_eq!(draw.uniforms().params[0], 1.0 / WAVELENGTH_MIN);
    }

    #[test]
    fn bounds_grow_by_as_far_as_a_vertex_can_go() {
        let uniforms = ShallowWoozeDraw {
            amplitude: 0.05,
            wavelength: 1.0,
            index: 3,
            weight: 0.5,
        }
        .uniforms();
        let mut bounds = ([0.0, 1.0, 2.0], [3.0, 4.0, 5.0]);
        grow_bounds(&mut bounds, &uniforms);
        assert_eq!(bounds.0, [-0.05, 0.95, 1.95]);
        assert_eq!(bounds.1, [3.05, 4.05, 5.05]);
    }

    // ----- On a GPU. Without one each test prints a note and passes. -----

    use crate::scene_renderer::{RendererConfig, SceneRenderer};
    use crate::{Camera, Vertex as MeshVertex};
    use flint_core::EntityId;
    use flint_ecs::FlintWorld;
    use flint_import::ImportedMaterial;

    const SIZE: u32 = 128;
    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

    struct Rig {
        device: wgpu::Device,
        queue: wgpu::Queue,
        color: wgpu::Texture,
        color_view: wgpu::TextureView,
        depth_view: wgpu::TextureView,
    }

    fn rig() -> Option<Rig> {
        pollster::block_on(async {
            let instance = crate::gpu_select::instance();
            let adapter = crate::gpu_select::request_adapter(&instance, None).await?;
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default(), None)
                .await
                .ok()?;
            let target = |format, usage| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: SIZE,
                        height: SIZE,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
            };
            let attach = wgpu::TextureUsages::RENDER_ATTACHMENT;
            let color = target(FORMAT, attach | wgpu::TextureUsages::COPY_SRC);
            let depth = target(
                wgpu::TextureFormat::Depth32Float,
                attach | wgpu::TextureUsages::TEXTURE_BINDING,
            );
            Some(Rig {
                color_view: color.create_view(&Default::default()),
                depth_view: depth.create_view(&Default::default()),
                color,
                device,
                queue,
            })
        })
    }

    macro_rules! rig_or_skip {
        () => {
            match rig() {
                Some(r) => r,
                None => {
                    eprintln!("no GPU adapter; skipping");
                    return;
                }
            }
        };
    }

    impl Rig {
        /// Draw a frame and read it back, tightly packed.
        fn image(&self, renderer: &mut SceneRenderer, world: &FlintWorld) -> Vec<u8> {
            renderer.update_from_world(world, &self.device);
            renderer.render_to(
                &self.device,
                &self.queue,
                &self.depth_view,
                &Camera::default(),
                &self.color_view,
            );
            let row = (SIZE * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
                * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: (row * SIZE) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = self.device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_buffer(
                self.color.as_image_copy(),
                wgpu::ImageCopyBuffer {
                    buffer: &buffer,
                    layout: wgpu::ImageDataLayout {
                        offset: 0,
                        bytes_per_row: Some(row),
                        rows_per_image: None,
                    },
                },
                wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                },
            );
            self.queue.submit([encoder.finish()]);
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, |r| r.unwrap());
            self.device.poll(wgpu::Maintain::Wait);
            let data = buffer.slice(..).get_mapped_range();
            let mut out = Vec::with_capacity((SIZE * SIZE * 4) as usize);
            for y in 0..SIZE {
                let start = (row * y) as usize;
                out.extend_from_slice(&data[start..start + (SIZE * 4) as usize]);
            }
            out
        }
    }

    /// How many pixels of two frames differ by more than a rounding error.
    fn differing(a: &[u8], b: &[u8]) -> usize {
        a.chunks(4)
            .zip(b.chunks(4))
            .filter(|(a, b)| (0..3).any(|i| a[i].abs_diff(b[i]) > 3))
            .count()
    }

    /// A flat, double-sided square of half-width `half` at the origin.
    fn square(half: f32) -> (Vec<MeshVertex>, Vec<u32>) {
        let v = |x: f32, z: f32| MeshVertex {
            position: [x, 0.0, z],
            normal: [0.0, 1.0, 0.0],
            color: [0.5, 0.5, 0.5, 1.0],
            uv: [0.0, 0.0],
        };
        (
            vec![
                v(-half, -half),
                v(-half, half),
                v(half, half),
                v(half, -half),
            ],
            vec![0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2],
        )
    }

    fn material(name: &str) -> ImportedMaterial {
        ImportedMaterial {
            roughness: 1.0,
            metallic: 0.0,
            ..ImportedMaterial::vertex_colored(name)
        }
    }

    fn place(world: &mut FlintWorld, id: EntityId, position: [f32; 3]) {
        let mut transform = toml::Table::new();
        transform.insert(
            "position".into(),
            toml::Value::Array(position.iter().map(|&p| f64::from(p).into()).collect()),
        );
        world
            .set_component(id, "transform", transform.into())
            .unwrap();
    }

    /// Grey ground under a slanting sun, and a sheet 2 m up casting a shadow on the
    /// middle of the view, as in `cast_shadows_false_keeps_a_mesh_out_of_the_shadow_map`.
    fn sheet_over_ground(rig: &Rig, renderer: &mut SceneRenderer) -> (FlintWorld, EntityId) {
        renderer.set_tonemapping(false);
        renderer.set_ambient([0.0; 3], [0.0; 3]);
        let (ground, indices) = square(20.0);
        let (sheet, _) = square(1.5);
        renderer.load_procedural_mesh(&rig.device, "ground", &ground, &indices, material("ground"));
        renderer.load_procedural_mesh(&rig.device, "sheet", &sheet, &indices, material("sheet"));
        let mut world = FlintWorld::new();
        let sun = world.spawn("sun").unwrap();
        let mut light = toml::Table::new();
        light.insert("type".into(), "directional".into());
        light.insert("intensity".into(), 3.0.into());
        light.insert(
            "direction".into(),
            toml::Value::Array(vec![0.5.into(), 1.0.into(), 0.0.into()]),
        );
        world.set_component(sun, "light", light.into()).unwrap();
        let spawn = |world: &mut FlintWorld, name: &str, mesh: &str, at: [f32; 3]| {
            let id = world.spawn(name).unwrap();
            place(world, id, at);
            let mut model = toml::Table::new();
            model.insert("asset".into(), mesh.into());
            model.insert("runtime".into(), true.into());
            world.set_component(id, "model", model.into()).unwrap();
            id
        };
        spawn(&mut world, "ground", "ground", [0.0; 3]);
        let sheet = spawn(&mut world, "sheet", "sheet", [1.0, 2.0, 0.0]);
        (world, sheet)
    }

    /// The shader's noise at a point, in Rust, to say where a vertex should land.
    fn noise_at(p: [f32; 3]) -> [f32; 3] {
        let corner = |c: [i32; 3]| {
            let mut v = c.map(|x| (x as u32).wrapping_mul(1664525).wrapping_add(1013904223));
            let round = |v: &mut [u32; 3]| {
                v[0] = v[0].wrapping_add(v[1].wrapping_mul(v[2]));
                v[1] = v[1].wrapping_add(v[2].wrapping_mul(v[0]));
                v[2] = v[2].wrapping_add(v[0].wrapping_mul(v[1]));
            };
            round(&mut v);
            v = v.map(|x| x ^ (x >> 16));
            round(&mut v);
            v.map(|x| x as f32 / 2_147_483_648.0 - 1.0)
        };
        let cell = p.map(|x| x.floor() as i32);
        let f: [f32; 3] = std::array::from_fn(|i| p[i] - p[i].floor());
        let u = f.map(|t| t * t * t * (t * (t * 6.0 - 15.0) + 10.0));
        let mix3 = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] {
            std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
        };
        let at = |x: i32, y: i32, z: i32| corner([cell[0] + x, cell[1] + y, cell[2] + z]);
        let x00 = mix3(at(0, 0, 0), at(1, 0, 0), u[0]);
        let x10 = mix3(at(0, 1, 0), at(1, 1, 0), u[0]);
        let x01 = mix3(at(0, 0, 1), at(1, 0, 1), u[0]);
        let x11 = mix3(at(0, 1, 1), at(1, 1, 1), u[0]);
        mix3(mix3(x00, x10, u[1]), mix3(x01, x11, u[1]), u[2])
    }

    /// With a wavelength this long a whole sheet sits at one point of the noise, so
    /// it moves as one, by a distance we can work out here.
    fn whole_sheet_draw() -> (ShallowWoozeDraw, [f32; 3]) {
        let draw = ShallowWoozeDraw {
            amplitude: 1.2,
            wavelength: 1.0e6,
            index: 3,
            weight: 0.35,
        };
        let uniforms = draw.uniforms();
        let a = noise_at([
            uniforms.leaving[0],
            uniforms.leaving[1],
            uniforms.leaving[2],
        ]);
        let b = noise_at([
            uniforms.arriving[0],
            uniforms.arriving[1],
            uniforms.arriving[2],
        ]);
        let moved = std::array::from_fn(|i| (a[i] + (b[i] - a[i]) * draw.weight) * draw.amplitude);
        (draw, moved)
    }

    /// The whole contract at once: a woozy sheet lands exactly where a plain sheet
    /// moved by that much would, in the main pass and the shadow pass both.
    #[test]
    fn a_vertex_lands_where_the_noise_says_and_its_shadow_follows() {
        let rig = rig_or_skip!();
        let mut renderer = SceneRenderer::new_headless(
            &rig.device,
            &rig.queue,
            FORMAT,
            SIZE,
            SIZE,
            RendererConfig::default(),
        );
        let (mut world, sheet) = sheet_over_ground(&rig, &mut renderer);
        let (draw, moved) = whole_sheet_draw();
        assert!(
            moved.iter().any(|d| d.abs() > 0.1),
            "it should move: {moved:?}"
        );
        let rest = [1.0_f32, 2.0, 0.0];

        for opacity in [1.0, 0.02] {
            // A faint sheet is the shadow pass on its own: it still casts solid.
            let mut look = toml::Table::new();
            look.insert("opacity".into(), f64::from(opacity).into());
            world.set_component(sheet, "material", look.into()).unwrap();

            renderer.set_shallow_wooze(std::iter::empty());
            place(&mut world, sheet, rest);
            let still = rig.image(&mut renderer, &world);
            let moved_by_hand: [f32; 3] = std::array::from_fn(|i| rest[i] + moved[i]);
            place(&mut world, sheet, moved_by_hand);
            let by_hand = rig.image(&mut renderer, &world);

            place(&mut world, sheet, rest);
            renderer.set_shallow_wooze([(sheet, draw)]);
            let woozy = rig.image(&mut renderer, &world);

            let told_apart = differing(&still, &by_hand);
            assert!(
                told_apart > 40,
                "opacity {opacity}: only {told_apart} moved"
            );
            let off = differing(&woozy, &by_hand);
            assert!(
                off <= 6,
                "opacity {opacity}: {off} of {told_apart} pixels off"
            );
        }
    }

    #[test]
    fn at_no_amplitude_the_variant_draws_what_the_plain_pipeline_does() {
        let rig = rig_or_skip!();
        let mut renderer = SceneRenderer::new_headless(
            &rig.device,
            &rig.queue,
            FORMAT,
            SIZE,
            SIZE,
            RendererConfig::default(),
        );
        let (world, sheet) = sheet_over_ground(&rig, &mut renderer);
        let plain = rig.image(&mut renderer, &world);
        renderer.set_shallow_wooze([(
            sheet,
            ShallowWoozeDraw {
                amplitude: 0.0,
                wavelength: 1.0,
                index: 7,
                weight: 0.5,
            },
        )]);
        let still = rig.image(&mut renderer, &world);
        assert_eq!(differing(&plain, &still), 0);
        assert!(plain == still, "byte for byte");
    }

    #[test]
    fn a_scene_without_it_builds_none_of_it_and_one_with_it_only_what_it_uses() {
        let rig = rig_or_skip!();
        let mut renderer = SceneRenderer::new_headless(
            &rig.device,
            &rig.queue,
            FORMAT,
            SIZE,
            SIZE,
            RendererConfig::default(),
        );
        let (world, sheet) = sheet_over_ground(&rig, &mut renderer);
        rig.image(&mut renderer, &world);
        assert_eq!(renderer.wooze_built(), (false, false, false));

        // Naming something with no draws builds nothing either.
        let draw = whole_sheet_draw().0;
        renderer.set_shallow_wooze([(EntityId::new(), draw)]);
        rig.image(&mut renderer, &world);
        assert_eq!(renderer.wooze_built(), (false, false, false));

        renderer.set_shallow_wooze([(sheet, draw)]);
        rig.image(&mut renderer, &world);
        assert_eq!(renderer.wooze_built(), (true, false, true));
    }

    /// Each blend mode has its own woozy pipeline. A wrong layout or a mismatched
    /// stage panics here, in wgpu's validation, rather than in somebody's scene.
    #[test]
    fn every_blend_mode_draws() {
        let rig = rig_or_skip!();
        let mut renderer = SceneRenderer::new_headless(
            &rig.device,
            &rig.queue,
            FORMAT,
            SIZE,
            SIZE,
            RendererConfig::default(),
        );
        let (mut world, sheet) = sheet_over_ground(&rig, &mut renderer);
        renderer.set_shallow_wooze([(sheet, whole_sheet_draw().0)]);
        for mode in ["alpha", "additive", "multiply"] {
            let mut look = toml::Table::new();
            look.insert("opacity".into(), 0.5.into());
            look.insert("blend_mode".into(), mode.into());
            world.set_component(sheet, "material", look.into()).unwrap();
            rig.image(&mut renderer, &world);
        }
    }
}
