//! The scene's lights, owned by [`LightState`] (ADR 0118, #736): the
//! light uniforms every lit pipeline reads (group 2), their buffer and bind
//! group (which also carry the shadow maps and the material sampler), and
//! the lighting levers a scene's `[environment]` sets (ambient, diffuse
//! wrap, Oren-Nayar, sheen, shadows, default lights).

use crate::motion_hold::MotionHold;
use crate::pipeline::{
    DirectionalLight, LightUniforms, PointLight, SceneLayouts, SpotLight, MAX_DIRECTIONAL_LIGHTS,
    MAX_POINT_LIGHTS, MAX_SPOT_LIGHTS,
};
use crate::shadow::ShadowPass;
use flint_core::components as comp;
use flint_core::toml_util::toml_f32;
use flint_core::{EntityId, Vec3};
use flint_ecs::FlintWorld;
use wgpu::util::DeviceExt;

/// Snapshot of the runtime lighting levers (ambient hemisphere, diffuse
/// wrap, Oren-Nayar blend, sheen). Values are the active overrides when set,
/// else the built-in neutral defaults — suitable for seeding UI controls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightingLevers {
    /// Hemisphere ambient, sky half (linear RGB)
    pub ambient_sky: [f32; 3],
    /// Hemisphere ambient, ground half (linear RGB)
    pub ambient_ground: [f32; 3],
    /// Diffuse terminator wrap (0 = physically sharp / legacy)
    pub diffuse_wrap: f32,
    /// Lambert → Oren-Nayar blend (0 = exact legacy shading)
    pub oren_nayar: f32,
    /// Charlie-sheen rim tint (linear RGB)
    pub sheen_color: [f32; 3],
    /// Sheen strength, 0..~0.3 (0 = exact legacy shading)
    pub sheen_strength: f32,
}

/// The light uniforms, their GPU binding, and the scene's lighting levers.
pub(crate) struct LightState {
    /// The uniforms' buffer, written once a frame by [`Self::upload`].
    pub(crate) buffer: wgpu::Buffer,
    /// Group 2 of every lit pipeline: the uniforms, the shadow maps and
    /// their sampler, and the material sampler.
    pub(crate) bind_group: wgpu::BindGroup,
    /// This frame's lights and ambient. The sky writes a `sky` component's
    /// ambient in here too.
    pub(crate) uniforms: LightUniforms,
    /// Scene-authored hemisphere ambient override (sky, ground). None keeps
    /// the built-in default; stored so the zero-light fallback in
    /// [`Self::extract`] cannot stomp it.
    ambient_override: Option<([f32; 3], [f32; 3])>,
    /// The scene's `[environment] shadows` (#627): false keeps the shadow
    /// pass from drawing whatever the quality tier or F4 say. True unless
    /// a scene says otherwise.
    scene_shadows: bool,
    /// The scene's `[environment] default_lights` (#627): false lights a
    /// scene with no light entities by its ambient alone, instead of the
    /// built-in key and fill. True unless a scene says otherwise.
    default_lights: bool,
    /// Scene-authored diffuse terminator wrap. Rides ambient_sky.w encoded as
    /// (1 + wrap) so legacy writes of 1.0 decode to wrap = 0 in the shader.
    diffuse_wrap_override: Option<f32>,
    oren_nayar_override: Option<f32>,
    sheen_override: Option<([f32; 3], f32)>,
}

impl LightState {
    /// The built-in key and fill lights, bound with `shadow_pass`'s maps.
    pub(crate) fn new(
        device: &wgpu::Device,
        layouts: &SceneLayouts,
        shadow_pass: &ShadowPass,
    ) -> Self {
        let uniforms = LightUniforms::default_scene_lights();
        let (buffer, bind_group) = Self::bind(device, layouts, &uniforms, shadow_pass);
        Self {
            buffer,
            bind_group,
            uniforms,
            ambient_override: None,
            scene_shadows: true,
            default_lights: true,
            diffuse_wrap_override: None,
            oren_nayar_override: None,
            sheen_override: None,
        }
    }

    /// Make the buffer and bind group again over a new shadow pass (a
    /// shadow resolution change), from the uniforms as they stand.
    pub(crate) fn rebind(
        &mut self,
        device: &wgpu::Device,
        layouts: &SceneLayouts,
        shadow_pass: &ShadowPass,
    ) {
        let (buffer, bind_group) = Self::bind(device, layouts, &self.uniforms, shadow_pass);
        self.buffer = buffer;
        self.bind_group = bind_group;
    }

    /// Write this frame's uniforms to the buffer.
    pub(crate) fn upload(&self, queue: &wgpu::Queue) {
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&[self.uniforms]));
    }

    /// The scene's `[environment] shadows` (#627).
    pub(crate) fn set_scene_shadows(&mut self, on: bool) {
        self.scene_shadows = on;
    }

    /// Whether the scene lets the shadow pass draw.
    pub(crate) fn scene_shadows(&self) -> bool {
        self.scene_shadows
    }

    /// The scene's `[environment] default_lights` (#627).
    pub(crate) fn set_default_lights(&mut self, on: bool) {
        self.default_lights = on;
    }

    /// The light the cascades follow, directional 0 after the sort that
    /// puts the strongest first, gives light or god rays.
    pub(crate) fn sun_casts(&self) -> bool {
        self.uniforms.directional_count > 0 && {
            let sun = &self.uniforms.directional_lights[0];
            sun.intensity > 0.0 || sun.volumetric_intensity > 0.0
        }
    }

    fn bind(
        device: &wgpu::Device,
        layouts: &SceneLayouts,
        uniforms: &LightUniforms,
        shadow_pass: &ShadowPass,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Light Uniform Buffer"),
            contents: bytemuck::cast_slice(&[*uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &layouts.light_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&shadow_pass.shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&shadow_pass.shadow_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: shadow_pass.shadow_uniforms_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&layouts.material_sampler),
                },
            ],
            label: Some("Light Bind Group"),
        });

        (buffer, bind_group)
    }

    /// Override the hemisphere ambient colors (linear RGB). Scenes set this
    /// via `[environment] ambient_sky / ambient_ground`; when never called,
    /// the built-in default ambient is byte-identical to prior behavior.
    pub(crate) fn set_ambient(&mut self, sky: [f32; 3], ground: [f32; 3]) {
        self.ambient_override = Some((sky, ground));
        self.apply_ambient_override();
    }

    /// Clear any scene-authored ambient override, restoring the built-in
    /// default (needed on scene transitions so an old scene's ambient never
    /// leaks into a scene that doesn't author one).
    pub(crate) fn reset_ambient(&mut self) {
        self.ambient_override = None;
        self.diffuse_wrap_override = None;
        self.oren_nayar_override = None;
        self.sheen_override = None;
        let [sr, sg, sb] = LightUniforms::DEFAULT_AMBIENT_SKY;
        let [gr, gg, gb] = LightUniforms::DEFAULT_AMBIENT_GROUND;
        self.uniforms.ambient_sky = [sr, sg, sb, 1.0];
        self.uniforms.ambient_ground = [gr, gg, gb, 1.0];
        self.uniforms.sheen_color_strength = [0.0; 4];
    }

    /// Soften the diffuse terminator (0 = physically sharp / legacy shading).
    /// Scenes set this via `[environment] diffuse_wrap`.
    pub(crate) fn set_diffuse_wrap(&mut self, wrap: f32) {
        self.diffuse_wrap_override = Some(wrap.max(0.0));
        self.apply_ambient_override();
    }

    /// Blend the diffuse term from Lambert toward the Fujii Oren-Nayar
    /// approximation (0 = exact legacy shading, 1 = full Oren-Nayar; sigma
    /// comes from material roughness). Scenes set this via
    /// `[environment] oren_nayar` (ADR 0048).
    pub(crate) fn set_oren_nayar(&mut self, blend: f32) {
        self.oren_nayar_override = Some(blend.max(0.0));
        self.apply_ambient_override();
    }

    /// Tinted Charlie-sheen rim: `color` is a linear RGB tint, `strength`
    /// 0..~0.3 (0 = exact legacy shading). Scenes set this via
    /// `[environment] sheen_color / sheen_strength` (ADR 0048).
    pub(crate) fn set_sheen(&mut self, color: [f32; 3], strength: f32) {
        self.sheen_override = Some((color, strength.max(0.0)));
        self.apply_ambient_override();
    }

    /// Snapshot of the current lighting-lever state (overrides if set, else
    /// the built-in neutral defaults). Read by the render debug panel.
    pub(crate) fn levers(&self) -> LightingLevers {
        let (ambient_sky, ambient_ground) = self.ambient_override.unwrap_or((
            LightUniforms::DEFAULT_AMBIENT_SKY,
            LightUniforms::DEFAULT_AMBIENT_GROUND,
        ));
        let (sheen_color, sheen_strength) = self.sheen_override.unwrap_or(([0.0; 3], 0.0));
        LightingLevers {
            ambient_sky,
            ambient_ground,
            diffuse_wrap: self.diffuse_wrap_override.unwrap_or(0.0),
            oren_nayar: self.oren_nayar_override.unwrap_or(0.0),
            sheen_color,
            sheen_strength,
        }
    }

    fn apply_ambient_override(&mut self) {
        if let Some((sky, ground)) = self.ambient_override {
            self.uniforms.ambient_sky = [sky[0], sky[1], sky[2], 1.0];
            self.uniforms.ambient_ground = [ground[0], ground[1], ground[2], 1.0];
        }
        if let Some(wrap) = self.diffuse_wrap_override {
            // Shader decodes wrap = ambient_sky.w - 1.0
            self.uniforms.ambient_sky[3] = 1.0 + wrap;
        }
        if let Some(oren) = self.oren_nayar_override {
            // Shader decodes oren = ambient_ground.w - 1.0
            self.uniforms.ambient_ground[3] = 1.0 + oren;
        }
        if let Some((color, strength)) = self.sheen_override {
            self.uniforms.sheen_color_strength = [color[0], color[1], color[2], strength];
        }
    }

    /// Extract light entities from the world into the light uniforms,
    /// or the built-in key and fill when it has none and the scene
    /// allows them. Uploaded by [`Self::upload`].
    pub(crate) fn extract(&mut self, world: &FlintWorld, motion_hold: &mut MotionHold) {
        let mut dir_count = 0u32;
        let mut point_count = 0u32;
        let mut spot_count = 0u32;
        let mut directionals = [DirectionalLight::default(); MAX_DIRECTIONAL_LIGHTS];
        let mut points = [PointLight::default(); MAX_POINT_LIGHTS];
        let mut spots = [SpotLight::default(); MAX_SPOT_LIGHTS];

        // Deterministic iteration: the component index is a HashSet whose
        // order changes per process, which silently reassigned the one
        // shadow-casting directional slot (index 0) between runs. Sort by
        // entity name (then id) so light order is stable and authored.
        let mut light_entities: Vec<_> = world
            .entities_with_component(comp::LIGHT)
            .iter()
            .copied()
            .collect();
        // A hidden entity's light goes out with it (#628)
        let hidden = world.hidden_entities();
        if !hidden.is_empty() {
            light_entities.retain(|id| !hidden.contains(id));
        }
        light_entities.sort_by(|a, b| {
            world
                .get_name(*a)
                .unwrap_or("")
                .cmp(world.get_name(*b).unwrap_or(""))
                .then(a.cmp(b))
        });

        for entity_id in light_entities {
            let light_component = world
                .get_components(entity_id)
                .and_then(|components| components.get(comp::LIGHT).cloned());

            if let Some(light) = light_component {
                let light_type = light
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("directional");

                let color = light_vec3(&light, "color").unwrap_or([1.0, 1.0, 1.0]);
                let intensity = light.get("intensity").and_then(toml_f32).unwrap_or(1.0);

                match light_type {
                    "directional" => {
                        if (dir_count as usize) < MAX_DIRECTIONAL_LIGHTS {
                            let direction =
                                light_vec3(&light, "direction").unwrap_or([0.0, -1.0, 0.0]);
                            let volumetric_intensity = light
                                .get("volumetric_intensity")
                                .and_then(toml_f32)
                                .unwrap_or(0.0);
                            let volumetric_color =
                                light_vec3(&light, "volumetric_color").unwrap_or(color);
                            // Apparent source angular size, authored in
                            // degrees (sun ~0.5, softbox 2-5); drives PCSS
                            // penumbra (ADR 0056). Stored in radians.
                            let angular_size = light
                                .get("angular_size")
                                .and_then(toml_f32)
                                .unwrap_or(0.0)
                                .to_radians();
                            directionals[dir_count as usize] = DirectionalLight {
                                direction,
                                volumetric_intensity,
                                color,
                                intensity,
                                volumetric_color,
                                angular_size,
                            };
                            dir_count += 1;
                        }
                    }
                    "point" => {
                        if (point_count as usize) < MAX_POINT_LIGHTS {
                            let light_pos = light_position(motion_hold, world, entity_id);
                            let radius = light
                                .get("range")
                                .or_else(|| light.get("radius"))
                                .and_then(toml_f32)
                                .unwrap_or(10.0);
                            let source_radius =
                                light.get("source_radius").and_then(toml_f32).unwrap_or(0.0);
                            points[point_count as usize] = PointLight {
                                position: [light_pos.x, light_pos.y, light_pos.z],
                                radius,
                                color,
                                intensity,
                                source_radius,
                                _pad0: 0.0,
                                _pad1: 0.0,
                                _pad2: 0.0,
                            };
                            point_count += 1;
                        }
                    }
                    "spot" => {
                        if (spot_count as usize) < MAX_SPOT_LIGHTS {
                            let light_pos = light_position(motion_hold, world, entity_id);
                            let direction =
                                light_vec3(&light, "direction").unwrap_or([0.0, -1.0, 0.0]);
                            let radius = light
                                .get("range")
                                .or_else(|| light.get("radius"))
                                .and_then(toml_f32)
                                .unwrap_or(10.0);
                            let inner_angle =
                                light.get("inner_angle").and_then(toml_f32).unwrap_or(0.3);
                            let outer_angle =
                                light.get("outer_angle").and_then(toml_f32).unwrap_or(0.5);
                            let source_radius =
                                light.get("source_radius").and_then(toml_f32).unwrap_or(0.0);
                            spots[spot_count as usize] = SpotLight {
                                position: [light_pos.x, light_pos.y, light_pos.z],
                                radius,
                                direction,
                                inner_angle,
                                color,
                                outer_angle,
                                intensity,
                                source_radius,
                                _pad1: 0.0,
                                _pad2: 0.0,
                            };
                            spot_count += 1;
                        }
                    }
                    _ => {}
                }
            }
        }

        // Strongest directional casts the shadows: only index 0 gets CSM, so
        // put the highest-intensity directional first (sun over fill),
        // independent of entity naming. Stable sort keeps name order on ties.
        directionals[..dir_count as usize].sort_by(|a, b| {
            b.intensity
                .partial_cmp(&a.intensity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // If no lights found in scene, use defaults
        if dir_count == 0 && point_count == 0 && spot_count == 0 && self.default_lights {
            self.uniforms = LightUniforms::default_scene_lights();
            self.apply_ambient_override();
        } else {
            self.uniforms.directional_lights = directionals;
            self.uniforms.point_lights = points;
            self.uniforms.spot_lights = spots;
            self.uniforms.directional_count = dir_count;
            self.uniforms.point_count = point_count;
            self.uniforms.spot_count = spot_count;
        }
    }
}

/// Where a point or spot light shines from: its world position, or,
/// carried by a held entity, where it was when the tick began (#873), so
/// the pool of light steps with the drawing that carries it. The held
/// matrix is the one the entity's own draw reads later this frame.
fn light_position(motion_hold: &mut MotionHold, world: &FlintWorld, entity: EntityId) -> Vec3 {
    let Some(live) = world.get_world_matrix(entity) else {
        return Vec3::ZERO;
    };
    let m = motion_hold.matrix(entity, live);
    Vec3::new(m[3][0], m[3][1], m[3][2])
}

fn light_vec3(table: &toml::Value, key: &str) -> Option<[f32; 3]> {
    flint_core::toml_util::toml_vec3(table.get(key)?)
}
