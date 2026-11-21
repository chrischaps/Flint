//! The sea and the game's lakes and rivers, owned by [`WaterRenderer`]
//! (ADR 0118, #736): the ocean's uniforms and transform, the wave spectrum
//! and look a scene's `ocean` gives, the contact-foam hulls, water bodies
//! (ADR 0094), and the refraction grab the water reads (an opaque-scene
//! snapshot, kept per view so a comic panel has its own). The pipelines are
//! in `ScenePipelines`; the host's clock is still `SceneRenderer::ocean_time`.

use super::helpers::identity_matrix;
use crate::gpu_counts::{CountedDevice, CountedQueue};
use crate::ocean_pipeline::OceanPipeline;
use crate::pipeline::TransformUniforms;
use crate::postprocess::HDR_FORMAT;
use crate::sky_pipeline::SkyParams;
use flint_ecs::FlintWorld;

/// The water's GPU resources and what the scene says about it.
pub(crate) struct WaterRenderer {
    uniform_buffer: Option<wgpu::Buffer>,
    uniform_bind_group: Option<wgpu::BindGroup>,
    transform_buffer: Option<wgpu::Buffer>,
    transform_bind_group: Option<wgpu::BindGroup>,
    /// The sea's own body block (group 1, binding 1): its colours, full
    /// waves, no flow (ADR 0094).
    sea_body_buffer: Option<wgpu::Buffer>,
    /// Water bodies a game set at run time, by its id (ADR 0094).
    bodies: std::collections::BTreeMap<String, crate::water_body::GpuWaterBody>,
    /// Water bodies from `water_body` components, by entity name, with the
    /// component they were built from (rebuilt only when it changes). `None`
    /// when the component didn't describe a mesh; the warning said why.
    scene_bodies:
        std::collections::BTreeMap<String, (toml::Value, Option<crate::water_body::GpuWaterBody>)>,
    spectrum: Option<flint_geometry::ocean::WaveSpectrum>,
    visuals: crate::ocean_pipeline::OceanVisuals,
    /// The scene has a sea this frame (and its pipeline built).
    ocean_active: bool,
    /// Every `ocean_contact` hull this frame, as drawn, in name order
    /// (#749). `upload` foams round the `MAX_CONTACTS` nearest the camera.
    contacts: Vec<ContactHull>,
    /// Each hull's motion across frames, for the velocity a host didn't
    /// give: by entity, dropped with the hull.
    contact_motion: std::collections::HashMap<flint_core::EntityId, ContactMotion>,
    /// The velocities the host gave this frame (`FrameInputs::contact_velocities`).
    contact_velocities: std::collections::HashMap<flint_core::EntityId, [f32; 3]>,
    /// The host's tint over the component's this frame
    /// (`FrameInputs::ocean_tint`, #746).
    frame_tint: Option<crate::ocean_pipeline::OceanTint>,
    /// Hulls already warned about being left out, by name.
    contact_warned: std::cell::RefCell<std::collections::BTreeSet<String>>,
    /// Grab pass (refraction): opaque scene color+depth snapshots, at the
    /// view's size.
    pub(crate) grab: OceanGrab,
    grab_dummy_bind_group: Option<wgpu::BindGroup>,
    /// The grab runs this frame: water, the HDR chain, normal shading.
    pub(crate) grab_this_frame: bool,
    /// The camera's near and far planes, which the water linearises the
    /// grabbed depth with.
    pub(crate) camera_near_far: (f32, f32),
}

/// The refraction grab at one view's size. A comic panel's `ViewSlot`
/// holds its own and swaps it in around its draw.
#[derive(Default)]
pub(crate) struct OceanGrab {
    pub(crate) color: Option<(wgpu::Texture, wgpu::TextureView)>,
    pub(crate) depth: Option<(wgpu::Texture, wgpu::TextureView)>,
    size: (u32, u32),
    pub(crate) bind_group: Option<wgpu::BindGroup>,
}

/// One `ocean_contact` hull as the frame draws it (#749).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ContactHull {
    pub(crate) id: flint_core::EntityId,
    pub(crate) name: String,
    /// The entity's origin in the world, its parents and pose applied.
    pub(crate) position: [f32; 3],
    /// (cos yaw, sin yaw) of its heading on the water.
    pub(crate) heading: [f32; 2],
    /// Half extents in metres, the world scale applied.
    pub(crate) half: [f32; 2],
    /// Its velocity: the host's, or its drawn position differenced.
    pub(crate) velocity: [f32; 3],
}

impl ContactHull {
    fn to_gpu(&self) -> crate::ocean_pipeline::ContactHullGpu {
        crate::ocean_pipeline::ContactHullGpu {
            pose: [
                self.position[0],
                self.position[2],
                self.heading[0],
                self.heading[1],
            ],
            extent: [self.half[0], self.half[1], 0.0, 0.0],
            velocity: [self.velocity[0], self.velocity[1], self.velocity[2], 0.0],
        }
    }
}

/// A hull's last drawn position and smoothed velocity.
#[derive(Debug, Clone, Copy)]
struct ContactMotion {
    time: f64,
    position: [f32; 3],
    velocity: [f32; 3],
}

/// Fastest a hull is taken to move (m/s): a teleport or a scene load would
/// otherwise read as a slam.
const CONTACT_MAX_SPEED: f32 = 5.0;

/// Where a hull with world matrix `m` (column-major) lies on the water: its
/// origin, its heading as (cos yaw, sin yaw), and `half` scaled by the
/// world's scale along its local X and Z.
///
/// The heading is the mean of its local Z axis and its local X axis turned
/// a quarter, each laid flat: exactly the yaw for an upright hull, and the
/// line its footprint follows for one that pitches or rolls. The shader's
/// convention is forward = (sin y, cos y), right = (cos y, -sin y).
pub(crate) fn hull_on_water(m: &[[f32; 4]; 4], half: [f32; 2]) -> ([f32; 3], [f32; 2], [f32; 2]) {
    let x_axis = [m[0][0], m[0][1], m[0][2]];
    let z_axis = [m[2][0], m[2][1], m[2][2]];
    let len = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let (sx, sz) = (len(x_axis), len(z_axis));
    let (ix, iz) = (1.0 / sx.max(1e-6), 1.0 / sz.max(1e-6));
    // Forward on the water: (sin y, cos y)
    let f = [
        z_axis[0] * iz - x_axis[2] * ix,
        z_axis[2] * iz + x_axis[0] * ix,
    ];
    let fl = (f[0] * f[0] + f[1] * f[1]).sqrt();
    let heading = if fl > 1e-6 {
        [f[1] / fl, f[0] / fl]
    } else {
        [1.0, 0.0]
    };
    (
        [m[3][0], m[3][1], m[3][2]],
        heading,
        [(half[0] * sx).max(0.01), (half[1] * sz).max(0.01)],
    )
}

/// What the water's uniforms read from the rest of the frame.
pub(crate) struct WaterFrame<'a> {
    /// The wave clock (`SceneRenderer::ocean_time`).
    pub(crate) time: f64,
    pub(crate) tonemapping: u32,
    /// The sky's colours, which the water reflects when the sky is active.
    pub(crate) sky: &'a SkyParams,
    pub(crate) sky_active: bool,
}

impl WaterRenderer {
    /// The ocean's uniform/transform buffers and bind groups, when its
    /// pipeline built (`ScenePipelines` leaves it out when its shader fails:
    /// no ocean rather than a crashed renderer).
    pub(crate) fn new(
        device: &wgpu::Device,
        pipeline: Option<&crate::ocean_pipeline::OceanPipeline>,
        transform_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let Some(pipeline) = pipeline else {
            return Self::without_gpu(None, None, None, None, None);
        };

        let uniform_buffer = device.create_buffer_counted(&wgpu::BufferDescriptor {
            label: Some("Ocean Uniform Buffer"),
            size: std::mem::size_of::<crate::ocean_pipeline::OceanUniformsGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sea_body_buffer = device.create_buffer_counted(&wgpu::BufferDescriptor {
            label: Some("Ocean Sea Body Buffer"),
            size: std::mem::size_of::<crate::water_body::WaterBodyUniformsGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind_group = device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
            layout: &pipeline.ocean_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: sea_body_buffer.as_entire_binding(),
                },
            ],
            label: Some("Ocean Uniform Bind Group"),
        });

        let transform_buffer = device.create_buffer_counted(&wgpu::BufferDescriptor {
            label: Some("Ocean Transform Buffer"),
            size: std::mem::size_of::<TransformUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let transform_bind_group = device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
            layout: transform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: transform_buffer.as_entire_binding(),
            }],
            label: Some("Ocean Transform Bind Group"),
        });

        Self::without_gpu(
            Some(uniform_buffer),
            Some(uniform_bind_group),
            Some(transform_buffer),
            Some(transform_bind_group),
            Some(sea_body_buffer),
        )
    }

    /// Everything but the GPU resources, empty.
    fn without_gpu(
        uniform_buffer: Option<wgpu::Buffer>,
        uniform_bind_group: Option<wgpu::BindGroup>,
        transform_buffer: Option<wgpu::Buffer>,
        transform_bind_group: Option<wgpu::BindGroup>,
        sea_body_buffer: Option<wgpu::Buffer>,
    ) -> Self {
        Self {
            uniform_buffer,
            uniform_bind_group,
            transform_buffer,
            transform_bind_group,
            sea_body_buffer,
            bodies: std::collections::BTreeMap::new(),
            scene_bodies: std::collections::BTreeMap::new(),
            spectrum: None,
            visuals: crate::ocean_pipeline::OceanVisuals::default(),
            ocean_active: false,
            contacts: Vec::new(),
            contact_motion: std::collections::HashMap::new(),
            contact_velocities: std::collections::HashMap::new(),
            frame_tint: None,
            contact_warned: Default::default(),
            grab: OceanGrab::default(),
            grab_dummy_bind_group: None,
            grab_this_frame: false,
            camera_near_far: (0.1, 1000.0),
        }
    }

    /// The refraction grab's colour and depth, bytes (#1091).
    pub(crate) fn grab_bytes(&self) -> u64 {
        use crate::gpu_counts::bytes_of;
        self.grab.color.as_ref().map_or(0, |c| bytes_of(&c.0))
            + self.grab.depth.as_ref().map_or(0, |d| bytes_of(&d.0))
    }

    /// Trade this view's grab for a panel's (`panels::ViewSlot`).
    pub(crate) fn swap_grab(&mut self, other: &mut OceanGrab) {
        std::mem::swap(&mut self.grab, other);
    }

    /// Create the placeholder group-3 bind group (1x1 textures) so the ocean
    /// can always bind group 3 even when the grab pass didn't run.
    pub(crate) fn ensure_grab_dummy(
        &mut self,
        device: &wgpu::Device,
        pipeline: Option<&OceanPipeline>,
    ) {
        if self.grab_dummy_bind_group.is_some() {
            return;
        }
        let Some(op) = pipeline else {
            return;
        };
        let dummy_color = crate::ocean_pipeline::create_dummy_grab_texture(
            device,
            HDR_FORMAT,
            "Ocean Grab Dummy Color",
        );
        let dummy_depth = crate::ocean_pipeline::create_dummy_grab_texture(
            device,
            wgpu::TextureFormat::R32Float,
            "Ocean Grab Dummy Depth",
        );
        let cv = dummy_color.create_view(&Default::default());
        let dv = dummy_depth.create_view(&Default::default());
        self.grab_dummy_bind_group = Some(device.create_bind_group_counted(
            &wgpu::BindGroupDescriptor {
                layout: &op.grab_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&cv),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&dv),
                    },
                ],
                label: Some("Ocean Grab Dummy Bind Group"),
            },
        ));
    }

    /// Ensure grab-pass textures + bind groups exist at the given size.
    /// Called only on the ocean+postprocess path; cheap when size unchanged.
    pub(crate) fn ensure_grab(
        &mut self,
        device: &wgpu::Device,
        pipeline: Option<&OceanPipeline>,
        width: u32,
        height: u32,
    ) {
        self.ensure_grab_dummy(device, pipeline);
        let Some(op) = pipeline else {
            return;
        };

        if self.grab.size == (width, height) && self.grab.bind_group.is_some() {
            return;
        }

        let make = |format: wgpu::TextureFormat, label: &str| {
            let tex = device.create_texture_counted(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT
                    // Copied out by a frame capture (#1085)
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = tex.create_view(&Default::default());
            (tex, view)
        };
        let color = make(HDR_FORMAT, "Ocean Grab Color");
        let depth = make(wgpu::TextureFormat::R32Float, "Ocean Grab Depth");

        self.grab.bind_group = Some(
            device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                layout: &op.grab_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&color.1),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&depth.1),
                    },
                ],
                label: Some("Ocean Grab Bind Group"),
            }),
        );
        self.grab.color = Some(color);
        self.grab.depth = Some(depth);
        self.grab.size = (width, height);
    }

    /// Extract the first `ocean` component from the world. The wave spectrum
    /// is regenerated only when simulation params change (cheap: N sin/cos
    /// coefficient sets), so live-tweaking any field takes effect next frame.
    pub(crate) fn extract_ocean(&mut self, world: &FlintWorld, has_pipeline: bool) {
        self.ocean_active = false;
        for &entity_id in world.entities_with_component(flint_geometry::components::OCEAN) {
            let view = world.view(entity_id, flint_geometry::components::OCEAN);
            if !view.exists() {
                continue;
            }

            let params = flint_geometry::ocean::OceanParams::from_component(&view);
            let needs_regen = self
                .spectrum
                .as_ref()
                .map(|s| s.params != params)
                .unwrap_or(true);
            if needs_regen {
                self.spectrum = Some(flint_geometry::ocean::WaveSpectrum::generate(&params));
            }
            self.visuals = crate::ocean_pipeline::OceanVisuals::from_component(&view);
            self.ocean_active = has_pipeline;
            break; // one ocean per scene
        }
    }

    /// Whether any water draws this frame: the sea, or a body.
    pub(crate) fn any_active(&self, has_pipeline: bool) -> bool {
        self.ocean_active || self.has_bodies(has_pipeline)
    }

    fn has_bodies(&self, has_pipeline: bool) -> bool {
        has_pipeline
            && (!self.bodies.is_empty() || self.scene_bodies.values().any(|(_, b)| b.is_some()))
    }

    /// Every body to draw this frame: the scene's, then the game's.
    pub(crate) fn bodies_to_draw(&self) -> impl Iterator<Item = &crate::water_body::GpuWaterBody> {
        self.scene_bodies
            .values()
            .filter_map(|(_, b)| b.as_ref())
            .chain(self.bodies.values())
    }

    fn build_body(
        &self,
        device: &wgpu::Device,
        pipeline: Option<&OceanPipeline>,
        label: &str,
        mesh: &crate::water_body::WaterBodyMesh,
        look: crate::water_body::WaterBodyLook,
    ) -> Result<crate::water_body::GpuWaterBody, String> {
        let (Some(op), Some(ocean_buf)) = (pipeline, &self.uniform_buffer) else {
            return Err("this device has no ocean pipeline".into());
        };
        mesh.validate()?;
        if mesh.indices.is_empty() {
            return Err("water body has no triangles".into());
        }
        Ok(crate::water_body::GpuWaterBody::new(
            device,
            &op.ocean_bind_group_layout,
            ocean_buf,
            label,
            mesh,
            look,
        ))
    }

    /// Build the bodies `water_body` components describe. A body is rebuilt
    /// only when its component changes, and dropped with its entity.
    pub(crate) fn extract_bodies(
        &mut self,
        world: &FlintWorld,
        device: &wgpu::Device,
        pipeline: Option<&OceanPipeline>,
    ) {
        let mut seen = std::collections::BTreeSet::new();
        for &entity_id in world.entities_with_component(flint_geometry::components::WATER_BODY) {
            let Some(components) = world.get_components(entity_id) else {
                continue;
            };
            let Some(value) = components.get(flint_geometry::components::WATER_BODY) else {
                continue;
            };
            let name = world
                .get_name(entity_id)
                .map(str::to_string)
                .unwrap_or_else(|| format!("{entity_id:?}"));
            seen.insert(name.clone());
            if self
                .scene_bodies
                .get(&name)
                .is_some_and(|(v, _)| v == value)
            {
                continue;
            }
            let view = world.view(entity_id, flint_geometry::components::WATER_BODY);
            let look = crate::water_body::WaterBodyLook::from_component(&view);
            let body = crate::water_body::mesh_from_component(&view)
                .and_then(|mesh| self.build_body(device, pipeline, &name, &mesh, look));
            let body = match body {
                Ok(b) => Some(b),
                Err(e) => {
                    tracing::warn!("water_body `{name}`: {e}");
                    None
                }
            };
            self.scene_bodies.insert(name, (value.clone(), body));
        }
        self.scene_bodies.retain(|name, _| seen.contains(name));

        // Bodies take the sea's look and waves; a scene with bodies and no
        // ocean gets the default ocean's.
        if !self.ocean_active && self.has_bodies(pipeline.is_some()) && self.spectrum.is_none() {
            self.spectrum = Some(flint_geometry::ocean::WaveSpectrum::generate(
                &flint_geometry::ocean::OceanParams::default(),
            ));
        }
    }

    /// Draw water on a game's own mesh (ADR 0094): a lake at its level, a
    /// river down a valley. It shades as the sea does, with the body's own
    /// tint, wave scale and flow, and the sea's (or the default ocean's)
    /// foam, refraction and reflection. Replaces any body set with this
    /// `id`. Errs when the mesh's indices are bad or it has no triangles.
    pub(crate) fn set_body(
        &mut self,
        device: &wgpu::Device,
        pipeline: Option<&OceanPipeline>,
        id: &str,
        mesh: &crate::water_body::WaterBodyMesh,
        look: crate::water_body::WaterBodyLook,
    ) -> Result<(), String> {
        let body = self.build_body(device, pipeline, id, mesh, look)?;
        self.bodies.insert(id.to_string(), body);
        if !self.ocean_active && self.spectrum.is_none() {
            self.spectrum = Some(flint_geometry::ocean::WaveSpectrum::generate(
                &flint_geometry::ocean::OceanParams::default(),
            ));
        }
        Ok(())
    }

    /// Change a body's look and keep its mesh. Returns whether it existed.
    pub(crate) fn set_body_look(
        &mut self,
        id: &str,
        look: crate::water_body::WaterBodyLook,
    ) -> bool {
        match self.bodies.get_mut(id) {
            Some(b) => {
                b.look = look;
                true
            }
            None => false,
        }
    }

    /// Remove the body set with `id`. Returns whether it existed.
    pub(crate) fn remove_body(&mut self, id: &str) -> bool {
        self.bodies.remove(id).is_some()
    }

    /// Remove every body a game set (the scene's own stay).
    pub(crate) fn clear_bodies(&mut self) {
        self.bodies.clear();
    }

    /// The hull velocities a host gave this frame (`FrameInputs::contact_velocities`).
    pub(crate) fn set_contact_velocities(&mut self, given: &[(flint_core::EntityId, [f32; 3])]) {
        self.contact_velocities.clear();
        self.contact_velocities.extend(given.iter().copied());
    }

    /// The host's tint for this frame, over the `ocean` component's
    /// (`FrameInputs::ocean_tint`, #746); `None` draws the component's.
    pub(crate) fn set_frame_tint(&mut self, tint: Option<crate::ocean_pipeline::OceanTint>) {
        self.frame_tint = tint;
    }

    /// Every `ocean_contact` entity, as drawn (#749): its world matrix gives
    /// the centre, heading and scaled extents, so a hull under a parent or
    /// posed with `rotation_quat` (as physics writes one) foams where it is.
    /// Its velocity is the host's when it gave one (physics knows it), else
    /// its drawn position differenced across frames on the waves' clock:
    /// drift and heave both count as impact against the moving water.
    pub(crate) fn extract_contact(&mut self, world: &FlintWorld, time: f64) {
        use flint_core::toml_util::toml_f32;
        let key = flint_geometry::components::OCEAN_CONTACT;
        self.contacts.clear();
        for &id in world.entities_with_component(key) {
            let Some(contact) = world.get_component(id, key) else {
                continue;
            };
            let Some(matrix) = world.get_world_matrix(id) else {
                continue;
            };
            let half_x = contact.get("half_x").and_then(toml_f32).unwrap_or(1.0);
            let half_z = contact.get("half_z").and_then(toml_f32).unwrap_or(1.0);
            let (position, heading, half) = hull_on_water(&matrix, [half_x, half_z]);

            let motion = self.contact_motion.entry(id).or_insert(ContactMotion {
                time,
                position,
                velocity: [0.0; 3],
            });
            if let Some(given) = self.contact_velocities.get(&id) {
                motion.velocity = given.map(|v| v.clamp(-CONTACT_MAX_SPEED, CONTACT_MAX_SPEED));
            } else {
                // Clamped against teleports, lightly smoothed against
                // frame jitter.
                let dt = (time - motion.time) as f32;
                if dt > 1e-4 {
                    let k = (10.0 * dt).min(1.0);
                    for ((vel, now), was) in motion
                        .velocity
                        .iter_mut()
                        .zip(position)
                        .zip(motion.position)
                    {
                        let v = ((now - was) / dt).clamp(-CONTACT_MAX_SPEED, CONTACT_MAX_SPEED);
                        *vel += (v - *vel) * k;
                    }
                }
            }
            motion.time = time;
            motion.position = position;

            self.contacts.push(ContactHull {
                id,
                name: world.get_name(id).unwrap_or_default().to_string(),
                position,
                heading,
                half,
                velocity: motion.velocity,
            });
        }
        // Name order, so which hulls foam never rests on a HashMap's
        self.contacts.sort_by(|a, b| a.name.cmp(&b.name));
        let seen: std::collections::HashSet<_> = self.contacts.iter().map(|c| c.id).collect();
        self.contact_motion.retain(|id, _| seen.contains(id));
    }

    /// The hulls the sea foams round this frame: all of them up to
    /// `MAX_CONTACTS`, else the ones nearest `camera_pos`, warning once by
    /// name about each left out.
    pub(crate) fn contacts_to_foam(&self, camera_pos: [f32; 3]) -> Vec<&ContactHull> {
        use crate::ocean_pipeline::MAX_CONTACTS;
        if self.contacts.len() <= MAX_CONTACTS {
            return self.contacts.iter().collect();
        }
        let dist2 = |c: &ContactHull| {
            (0..3)
                .map(|i| (c.position[i] - camera_pos[i]).powi(2))
                .sum::<f32>()
        };
        let mut by_distance: Vec<&ContactHull> = self.contacts.iter().collect();
        by_distance.sort_by(|a, b| dist2(a).total_cmp(&dist2(b)).then(a.name.cmp(&b.name)));
        let mut warned = self.contact_warned.borrow_mut();
        for left_out in &by_distance[MAX_CONTACTS..] {
            if warned.insert(left_out.name.clone()) {
                tracing::warn!(
                    "ocean_contact on '{}': the sea foams round {} hulls at once, the nearest \
                     the camera, and this one is farther; it has no contact foam while it is",
                    left_out.name,
                    MAX_CONTACTS
                );
            }
        }
        by_distance.truncate(MAX_CONTACTS);
        by_distance
    }

    /// Write this frame's ocean uniforms (wave phases in f64, grid follows
    /// the camera), the sea's body block and every body's. Water bodies
    /// read them too (ADR 0094), sea or no sea.
    pub(crate) fn upload(
        &self,
        queue: &wgpu::Queue,
        view_proj: [[f32; 4]; 4],
        camera_pos: [f32; 3],
        frame: &WaterFrame<'_>,
    ) {
        if let (Some(spectrum), Some(ubuf), Some(tbuf)) =
            (&self.spectrum, &self.uniform_buffer, &self.transform_buffer)
        {
            let transform = TransformUniforms {
                view_proj,
                model: identity_matrix(),
                model_inv_transpose: identity_matrix(),
                camera_pos,
                _pad: 0.0,
            };
            queue.write_buffer_counted(tbuf, 0, bytemuck::cast_slice(&[transform]));

            let v = &self.visuals;
            let hulls = self.contacts_to_foam(camera_pos);
            // Snap the grid to inner-cell multiples so wave sampling
            // positions stay world-anchored while the mesh follows us.
            let q = v.snap_quantum();
            let grid_offset = [
                (camera_pos[0] / q).floor() * q,
                (camera_pos[2] / q).floor() * q,
            ];
            let tint = self.frame_tint.unwrap_or(v.tint).to_gpu();
            let uniforms = crate::ocean_pipeline::OceanUniformsGpu {
                waves: spectrum.to_gpu(frame.time),
                deep_color: v.deep_color,
                shallow_color: v.shallow_color,
                foam_color: v.foam_color,
                sss_color: v.sss_color,
                grid_offset,
                num_waves: spectrum.waves.len() as u32,
                enable_tonemapping: frame.tonemapping,
                grid_scale: v.grid_scale,
                fade_start: v.fade_start,
                fade_end: v.fade_end,
                foam_threshold: v.foam_threshold,
                foam_noise_scale: v.foam_noise_scale,
                ramp_steps: v.ramp_steps,
                specular_strength: v.specular_strength,
                time: (frame.time % 100_000.0) as f32,
                absorption_color: v.absorption_color,
                turbidity: v.turbidity,
                refraction_strength: v.refraction_strength,
                camera_near: self.camera_near_far.0,
                camera_far: self.camera_near_far.1,
                grab_enabled: u32::from(self.grab_this_frame),
                sky_zenith: frame.sky.zenith_color,
                sky_horizon: frame.sky.horizon_color,
                sky_haze: frame.sky.haze_color,
                // Reflections need a procedural sky to reflect; scenes
                // with a texture skybox (or none) keep the old look.
                sky_reflection_strength: if frame.sky_active {
                    v.sky_reflection_strength
                } else {
                    0.0
                },
                // Contact foam is off (strength 0) without a hull.
                splash_strength: if hulls.is_empty() {
                    0.0
                } else {
                    v.splash_strength
                },
                splash_width: v.splash_width,
                splash_flicker_speed: v.splash_flicker_speed,
                splash_baseline: v.splash_baseline,
                splash_noise_scale: v.splash_noise_scale,
                splash_response: v.splash_response,
                contact_count: hulls.len() as u32,
                hulls: {
                    let mut gpu = [crate::ocean_pipeline::ContactHullGpu::default();
                        crate::ocean_pipeline::MAX_CONTACTS];
                    for (slot, hull) in gpu.iter_mut().zip(&hulls) {
                        *slot = hull.to_gpu();
                    }
                    gpu
                },
                band_wobble: v.band_wobble,
                band_dither: v.band_dither,
                band_dither_scale: v.band_dither_scale,
                rain_ripple: v.rain_ripple,
                foam_glow: [
                    v.foam_glow_color[0],
                    v.foam_glow_color[1],
                    v.foam_glow_color[2],
                    v.foam_glow,
                ],
                // The tint (#746): the remap happens in-shader AFTER a
                // script's palette lands in these uniforms, so it can never
                // race it
                tint: tint.0,
                tint_flags: tint.1,
                // Shoreline foam reads the grab pass's depth: off
                // without it, whatever the component says.
                shore_foam_width: if self.grab_this_frame {
                    v.shore_foam_width
                } else {
                    0.0
                },
                shore_foam_strength: v.shore_foam_strength,
                shore_foam_color: v.shore_foam_color,
                shore_foam_speed: v.shore_foam_speed,
                shore_foam_reach: v.shore_foam_reach,
                _pad_shore: [0.0; 2],
                tint_deep: tint.2[0],
                tint_shallow: tint.2[1],
                tint_sss: tint.2[2],
                tint_foam: tint.2[3],
                tint_sky: tint.2[4],
            };
            queue.write_buffer_counted(ubuf, 0, bytemuck::cast_slice(&[uniforms]));

            // Each body's look, its missing colours the sea's as they
            // stand this frame (so a time-of-day palette tints them too).
            if let Some(sea_buf) = &self.sea_body_buffer {
                let sea = crate::water_body::WaterBodyUniformsGpu::sea(v);
                queue.write_buffer_counted(sea_buf, 0, bytemuck::cast_slice(&[sea]));
            }
            for b in self.bodies_to_draw() {
                let u = b.look.to_gpu(v, b.flowing);
                queue.write_buffer_counted(&b.uniform_buffer, 0, bytemuck::cast_slice(&[u]));
            }
        }
    }

    /// Draw the sea and then the bodies (after all opaque: writes depth so
    /// fog applies; group 3 holds the grab-pass snapshots, or dummies when
    /// the grab didn't run).
    pub(crate) fn draw<'a>(
        &'a self,
        render_pass: &mut wgpu::RenderPass<'a>,
        pipeline: Option<&'a OceanPipeline>,
        light_bind_group: &'a wgpu::BindGroup,
    ) {
        let grab_bg = if self.grab_this_frame {
            self.grab.bind_group.as_ref()
        } else {
            self.grab_dummy_bind_group.as_ref()
        };
        if let (Some(op), Some(ocean_bg), Some(transform_bg), Some(grab_bg)) = (
            pipeline,
            &self.uniform_bind_group,
            &self.transform_bind_group,
            grab_bg,
        ) {
            render_pass.set_bind_group(0, transform_bg, &[]);
            render_pass.set_bind_group(2, light_bind_group, &[]);
            render_pass.set_bind_group(3, grab_bg, &[]);
            if self.ocean_active {
                render_pass.set_pipeline(&op.pipeline);
                render_pass.set_bind_group(1, ocean_bg, &[]);
                render_pass.set_vertex_buffer(0, op.grid_vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(op.grid_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..op.grid_index_count, 0, 0..1);
            }
            // Lakes and rivers (ADR 0094): after the sea, depth-tested,
            // so one above sea level hides the sea beneath it.
            let mut bodies = self.bodies_to_draw().peekable();
            if bodies.peek().is_some() {
                render_pass.set_pipeline(&op.body_pipeline);
                for b in bodies {
                    render_pass.set_bind_group(1, &b.bind_group, &[]);
                    render_pass.set_vertex_buffer(0, b.vertex_buffer.slice(..));
                    render_pass
                        .set_index_buffer(b.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    render_pass.draw_indexed(0..b.index_count, 0, 0..1);
                }
            }
        }
    }
}

#[cfg(test)]
mod contact_tests {
    //! Contact foam's hulls (#749): read as drawn, several at once, the
    //! host's velocity over the differenced one.
    use super::*;
    use crate::ocean_pipeline::MAX_CONTACTS;
    use flint_core::EntityId;

    fn entity(world: &mut FlintWorld, name: &str, transform: &str) -> EntityId {
        let id = world.spawn(name).unwrap();
        world
            .set_component(id, "transform", toml::from_str(transform).unwrap())
            .unwrap();
        id
    }

    fn hull(world: &mut FlintWorld, name: &str, transform: &str) -> EntityId {
        let id = entity(world, name, transform);
        world
            .set_component(
                id,
                "ocean_contact",
                toml::from_str("half_x = 0.5\nhalf_z = 1.0").unwrap(),
            )
            .unwrap();
        id
    }

    fn water() -> WaterRenderer {
        WaterRenderer::without_gpu(None, None, None, None, None)
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn a_parented_hull_foams_where_its_parent_puts_it() {
        let mut world = FlintWorld::new();
        let boat = entity(
            &mut world,
            "boat",
            "position = [10.0, 0.0, 4.0]\nrotation = [0.0, 90.0, 0.0]",
        );
        let h = hull(&mut world, "hull", "position = [0.0, 0.0, 2.0]");
        world.set_parent(h, boat).unwrap();
        let mut w = water();
        w.extract_contact(&world, 0.0);
        let c = &w.contacts[0];
        // The boat's yaw of 90 turns the hull's +Z offset onto +X
        assert!(
            close(c.position[0], 12.0) && close(c.position[2], 4.0),
            "{c:?}"
        );
        assert!(
            close(c.heading[0], 0.0) && close(c.heading[1], 1.0),
            "{c:?}"
        );
    }

    #[test]
    fn a_quaternion_posed_hull_keeps_its_yaw() {
        let mut world = FlintWorld::new();
        let s = std::f32::consts::FRAC_1_SQRT_2;
        hull(
            &mut world,
            "dinghy",
            &format!("rotation_quat = [0.0, {s}, 0.0, {s}]"),
        );
        let mut w = water();
        w.extract_contact(&world, 0.0);
        let c = &w.contacts[0];
        assert!(
            close(c.heading[0], 0.0) && close(c.heading[1], 1.0),
            "{c:?}"
        );
    }

    #[test]
    fn an_euler_yaw_reads_as_before() {
        // The old reading was rotation[1]'s cos and sin: an upright hull
        // keeps it (bit for bit at 0, Float's raft at rest)
        for yaw in [0.0f32, 20.0, -35.0, 135.0] {
            let mut world = FlintWorld::new();
            hull(&mut world, "raft", &format!("rotation = [0.0, {yaw}, 0.0]"));
            let mut w = water();
            w.extract_contact(&world, 0.0);
            let c = &w.contacts[0];
            let r = yaw.to_radians();
            assert!(
                close(c.heading[0], r.cos()) && close(c.heading[1], r.sin()),
                "{yaw}: {c:?}"
            );
        }
        let mut world = FlintWorld::new();
        hull(
            &mut world,
            "raft",
            "position = [0.0, 0.05, 0.0]\nrotation = [0.0, 0.0, 0.0]",
        );
        let mut w = water();
        w.extract_contact(&world, 0.0);
        assert_eq!(w.contacts[0].heading, [1.0, 0.0]);
        assert_eq!(w.contacts[0].half, [0.5, 1.0]);
    }

    #[test]
    fn a_pitched_and_rolled_hull_keeps_its_heading() {
        // A raft riding the swell: pitch and roll must not swing the ring
        let mut world = FlintWorld::new();
        hull(&mut world, "raft", "rotation = [8.0, 30.0, -6.0]");
        let mut w = water();
        w.extract_contact(&world, 0.0);
        let yaw = w.contacts[0].heading[1]
            .atan2(w.contacts[0].heading[0])
            .to_degrees();
        assert!((yaw - 30.0).abs() < 1.0, "heading {yaw}");
    }

    #[test]
    fn scale_grows_the_footprint() {
        let mut world = FlintWorld::new();
        let parent = entity(&mut world, "barge", "scale = [2.0, 1.0, 3.0]");
        let h = hull(&mut world, "hull", "position = [0.0, 0.0, 0.0]");
        world.set_parent(h, parent).unwrap();
        let mut w = water();
        w.extract_contact(&world, 0.0);
        assert!(close(w.contacts[0].half[0], 1.0) && close(w.contacts[0].half[1], 3.0));
    }

    #[test]
    fn two_hulls_both_foam() {
        let mut world = FlintWorld::new();
        hull(&mut world, "skiff", "position = [-4.0, 0.0, 0.0]");
        hull(&mut world, "tender", "position = [4.0, 0.0, 0.0]");
        let mut w = water();
        w.extract_contact(&world, 0.0);
        let foaming: Vec<&str> = w
            .contacts_to_foam([0.0, 5.0, 10.0])
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(foaming, ["skiff", "tender"]);
    }

    #[test]
    fn past_the_limit_the_nearest_foam() {
        let mut world = FlintWorld::new();
        for i in 0..MAX_CONTACTS + 2 {
            hull(
                &mut world,
                &format!("boat_{i}"),
                &format!("position = [{}.0, 0.0, 0.0]", i * 10),
            );
        }
        let mut w = water();
        w.extract_contact(&world, 0.0);
        // The camera stands by the last two
        let camera = [((MAX_CONTACTS + 1) * 10) as f32, 5.0, 0.0];
        let mut foaming: Vec<String> = w
            .contacts_to_foam(camera)
            .iter()
            .map(|c| c.name.clone())
            .collect();
        foaming.sort();
        assert_eq!(foaming, ["boat_2", "boat_3", "boat_4", "boat_5"]);
        // Each left out is warned about once
        assert_eq!(
            *w.contact_warned.borrow(),
            ["boat_0".to_string(), "boat_1".to_string()]
                .into_iter()
                .collect()
        );
        w.contacts_to_foam(camera);
        assert_eq!(w.contact_warned.borrow().len(), 2);
    }

    #[test]
    fn velocity_is_differenced_per_hull() {
        let mut world = FlintWorld::new();
        let a = hull(&mut world, "a", "position = [0.0, 0.0, 0.0]");
        hull(&mut world, "b", "position = [9.0, 0.0, 0.0]");
        let mut w = water();
        w.extract_contact(&world, 0.0);
        // a moves 1 m along X in 0.5 s; b stays
        world
            .set_component(
                a,
                "transform",
                toml::from_str("position = [1.0, 0.0, 0.0]").unwrap(),
            )
            .unwrap();
        w.extract_contact(&world, 0.5);
        // k = min(10 * 0.5, 1) = 1: the speed in full
        assert!(close(w.contacts[0].velocity[0], 2.0), "{:?}", w.contacts[0]);
        assert_eq!(w.contacts[1].velocity, [0.0; 3]);
    }

    #[test]
    fn the_hosts_velocity_wins() {
        let mut world = FlintWorld::new();
        let a = hull(&mut world, "a", "position = [0.0, 0.0, 0.0]");
        let mut w = water();
        w.set_contact_velocities(&[(a, [0.0, -1.5, 0.75])]);
        w.extract_contact(&world, 0.0);
        assert_eq!(w.contacts[0].velocity, [0.0, -1.5, 0.75]);
        // Clamped as a differenced one is
        w.set_contact_velocities(&[(a, [40.0, 0.0, 0.0])]);
        w.extract_contact(&world, 0.1);
        assert_eq!(w.contacts[0].velocity[0], CONTACT_MAX_SPEED);
    }

    #[test]
    fn no_hull_no_foam() {
        let world = FlintWorld::new();
        let mut w = water();
        w.extract_contact(&world, 0.0);
        assert!(w.contacts_to_foam([0.0; 3]).is_empty());
    }
}
