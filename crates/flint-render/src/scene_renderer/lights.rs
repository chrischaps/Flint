//! The scene's lights, owned by [`LightState`] (ADR 0118, #736): the
//! light uniforms every lit pipeline reads (group 2), their buffer and bind
//! group (which also carry the shadow maps and the material sampler), and
//! the lighting levers a scene's `[environment]` sets (ambient, diffuse
//! wrap, Oren-Nayar, sheen, shadows, default lights).

use crate::gpu_counts::{CountedDevice, CountedQueue};
use crate::motion_hold::MotionHold;
use crate::pipeline::{
    DirectionalLight, LightUniforms, PointLight, SceneLayouts, SpotLight, MAX_DIRECTIONAL_LIGHTS,
    MAX_POINT_LIGHTS, MAX_SPOT_LIGHTS,
};
use crate::shadow::ShadowPass;
use flint_core::components as comp;
use flint_core::diagnostic::{self, codes, Diagnostic};
use flint_ecs::{ComponentView, FlintWorld};
use std::collections::BTreeSet;

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
    /// This frame's point and spot lights, every one, in name order:
    /// [`Self::upload`] keeps the nearest its camera when there are more
    /// than the shader takes (#682).
    points: Vec<Found<PointLight>>,
    spots: Vec<Found<SpotLight>>,
    /// This frame's directional lights, strongest first: the first
    /// `MAX_DIRECTIONAL_LIGHTS` are drawn. Kept (they were made anyway) for
    /// a frame's description (#1084).
    directionals: Vec<Found<DirectionalLight>>,
    /// Where the camera the lights were last placed for stood.
    placed_for: [f32; 3],
    /// The built-in key and fill stand in this frame.
    defaulted: bool,
    /// Lights already said to be past their type's cap, by name; and
    /// whether the fallback to the built-ins was said. A new scene speaks
    /// again ([`Self::forget_warnings`]).
    cap_warned: BTreeSet<String>,
    said_fallback: bool,
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
            points: Vec::new(),
            spots: Vec::new(),
            directionals: Vec::new(),
            placed_for: [0.0; 3],
            defaulted: true,
            cap_warned: BTreeSet::new(),
            said_fallback: false,
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

    /// Choose the point and spot lights `camera_pos` sees best (all of
    /// them when they fit) and write this frame's uniforms to the buffer.
    pub(crate) fn upload(&mut self, queue: &wgpu::Queue, camera_pos: [f32; 3]) {
        self.place(camera_pos);
        queue.write_buffer_counted(&self.buffer, 0, bytemuck::cast_slice(&[self.uniforms]));
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

    /// The spot whose shadow map this camera draws (#504): the one
    /// `cast_shadows` chose, if it is among the spots uploaded and gives any
    /// light. Read after [`Self::upload`].
    pub(crate) fn shadow_spot(&self) -> Option<&SpotLight> {
        let count = (self.uniforms.spot_count as usize).min(MAX_SPOT_LIGHTS);
        casting_spot(&self.uniforms.spot_lights[..count])
    }

    /// The point light whose cube shadow map this camera draws (#1112), as
    /// [`Self::shadow_spot`] finds the spot's. Read after [`Self::upload`].
    pub(crate) fn shadow_point(&self) -> Option<&PointLight> {
        let count = (self.uniforms.point_count as usize).min(MAX_POINT_LIGHTS);
        casting_point(&self.uniforms.point_lights[..count])
    }

    fn bind(
        device: &wgpu::Device,
        layouts: &SceneLayouts,
        uniforms: &LightUniforms,
        shadow_pass: &ShadowPass,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer_init_counted(&wgpu::util::BufferInitDescriptor {
            label: Some("Light Uniform Buffer"),
            contents: bytemuck::cast_slice(&[*uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
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

    /// Read the light entities from the world, or stand the built-in key
    /// and fill in when it has none the renderer can use and the scene
    /// allows them. The directional lights go into the uniforms now; the
    /// point and spot lights wait for a camera, since past the shader's
    /// cap the nearest are the ones drawn ([`Self::upload`], #682).
    pub(crate) fn extract(&mut self, world: &FlintWorld, motion_hold: &mut MotionHold) {
        let found = gather(world, motion_hold);
        let usable =
            !found.directionals.is_empty() || !found.points.is_empty() || !found.spots.is_empty();

        if !usable && self.default_lights {
            // Light entities that all fail to light (an unknown `type`, a
            // component that is not a table) would leave the scene lit by
            // the built-ins with no word: each was reported, and this says
            // what came of it, once a scene.
            if found.unusable > 0 && !self.said_fallback {
                self.said_fallback = true;
                tracing::warn!(
                    "{} light {} but none the renderer can use: the built-in key and fill \
                     light the scene (each light's problem is above)",
                    found.unusable,
                    if found.unusable == 1 {
                        "entity"
                    } else {
                        "entities"
                    },
                );
            }
            self.uniforms = LightUniforms::default_scene_lights();
            self.apply_ambient_override();
            self.defaulted = true;
            self.points.clear();
            self.spots.clear();
            self.directionals.clear();
            return;
        }
        self.defaulted = false;

        // Strongest directional casts the shadows: only index 0 gets CSM, so
        // put the highest-intensity directional first (sun over fill),
        // independent of entity naming. Stable sort keeps name order on ties,
        // and past the cap the weakest are the ones left out.
        let mut directionals = found.directionals;
        directionals.sort_by(|a, b| {
            b.light
                .intensity
                .partial_cmp(&a.light.intensity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for left_out in directionals.iter().skip(MAX_DIRECTIONAL_LIGHTS) {
            say_past_cap(
                &mut self.cap_warned,
                &left_out.name,
                "directional",
                MAX_DIRECTIONAL_LIGHTS,
                "the strongest by `intensity`",
            );
        }
        let mut uniform = [DirectionalLight::default(); MAX_DIRECTIONAL_LIGHTS];
        for (slot, found) in uniform.iter_mut().zip(&directionals) {
            *slot = found.light;
        }
        self.uniforms.directional_lights = uniform;
        self.uniforms.directional_count = directionals.len().min(MAX_DIRECTIONAL_LIGHTS) as u32;
        self.points = found.points;
        self.spots = found.spots;
        self.directionals = directionals;
    }

    /// Which lights the last placed camera drew and which it left out
    /// (#1084): `(kind, entity name, casts a shadow)`, then the left out as
    /// `(kind, entity name)`, then whether the built-ins stood in.
    #[allow(clippy::type_complexity)]
    pub(crate) fn described(
        &self,
    ) -> (
        Vec<(&'static str, String, bool)>,
        Vec<(&'static str, String)>,
        bool,
    ) {
        let mut kept = Vec::new();
        let mut left = Vec::new();
        if self.defaulted {
            return (kept, left, true);
        }
        for (i, d) in self.directionals.iter().enumerate() {
            if i < MAX_DIRECTIONAL_LIGHTS {
                kept.push(("directional", d.name.clone(), i == 0));
            } else {
                left.push(("directional", d.name.clone()));
            }
        }
        let (_, _, out) = nearest::<_, MAX_POINT_LIGHTS>(&self.points, self.placed_for);
        for p in &self.points {
            if out.contains(&p.name.as_str()) {
                left.push(("point", p.name.clone()));
            } else {
                kept.push(("point", p.name.clone(), p.light.shadow > 0.5));
            }
        }
        let (_, _, out) = nearest::<_, MAX_SPOT_LIGHTS>(&self.spots, self.placed_for);
        for s in &self.spots {
            if out.contains(&s.name.as_str()) {
                left.push(("spot", s.name.clone()));
            } else {
                kept.push(("spot", s.name.clone(), s.light.shadow > 0.5));
            }
        }
        (kept, left, false)
    }

    /// Choose this camera's point and spot lights: all of them when they
    /// fit, else the nearest `camera_pos` (#682), each one left out said
    /// once by name. A comic panel's camera chooses its own.
    fn place(&mut self, camera_pos: [f32; 3]) {
        if self.defaulted {
            return;
        }
        self.placed_for = camera_pos;
        let (points, point_count, left_out) =
            nearest::<_, MAX_POINT_LIGHTS>(&self.points, camera_pos);
        for name in left_out {
            say_past_cap(
                &mut self.cap_warned,
                name,
                "point",
                MAX_POINT_LIGHTS,
                "the nearest the camera",
            );
        }
        let (spots, spot_count, left_out) = nearest::<_, MAX_SPOT_LIGHTS>(&self.spots, camera_pos);
        for name in left_out {
            say_past_cap(
                &mut self.cap_warned,
                name,
                "spot",
                MAX_SPOT_LIGHTS,
                "the nearest the camera",
            );
        }
        self.uniforms.point_lights = points;
        self.uniforms.point_count = point_count;
        self.uniforms.spot_lights = spots;
        self.uniforms.spot_count = spot_count;
    }

    /// A new scene: what was said about the last one's lights may be said
    /// again.
    pub(crate) fn forget_warnings(&mut self) {
        self.cap_warned.clear();
        self.said_fallback = false;
    }
}

/// What a light entity's `type` may be.
const LIGHT_TYPES: [&str; 3] = ["directional", "point", "spot"];

/// The spot cone's half-angles when neither form is written, in radians
/// (about 17.2 and 28.6 degrees).
const INNER_ANGLE: f32 = 0.3;
const OUTER_ANGLE: f32 = 0.5;

/// One light as the shader takes it, with the name it is reported by and,
/// for a point or spot light, where it stands.
#[derive(Debug, Clone)]
pub(crate) struct Found<T> {
    pub(crate) name: String,
    pub(crate) position: [f32; 3],
    pub(crate) light: T,
}

/// Every light the renderer can use this frame, in entity-name order.
#[derive(Debug, Default)]
pub(crate) struct Gathered {
    pub(crate) directionals: Vec<Found<DirectionalLight>>,
    pub(crate) points: Vec<Found<PointLight>>,
    pub(crate) spots: Vec<Found<SpotLight>>,
    /// Light entities that light nothing (an unknown `type`), each reported.
    pub(crate) unusable: usize,
}

/// Read every visible light entity through its typed view (ADR 0115), so a
/// wrong value or an unknown `type` is said once, by entity name, instead
/// of lighting nothing in silence (#682).
pub(crate) fn gather(world: &FlintWorld, motion_hold: &mut MotionHold) -> Gathered {
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

    let mut found = Gathered::default();
    // The first spot by name that asks casts (#504), and the first point
    // light (#1112); the rest are said once
    let mut spot_casts = None::<String>;
    let mut point_casts = None::<String>;
    for entity_id in light_entities {
        let light = world.view(entity_id, comp::LIGHT);
        // A component that is no table has no fields (the view said so)
        let is_table = world
            .get_component(entity_id, comp::LIGHT)
            .is_some_and(toml::Value::is_table);
        let Some(kind) = is_table.then(|| light_type(&light)).flatten() else {
            found.unusable += 1;
            continue;
        };
        let name = world.get_name(entity_id).unwrap_or("?").to_string();
        // The matrix its draw reads: held on a tick by motion on twos
        // (#873), so the pool of light steps with the drawing that carries
        // it. No transform: at the origin, unturned.
        let matrix = world
            .get_world_matrix(entity_id)
            .map(|live| motion_hold.matrix(entity_id, live))
            .unwrap_or(IDENTITY);
        let position = [matrix[3][0], matrix[3][1], matrix[3][2]];

        let color = light.vec3("color", [1.0, 1.0, 1.0]);
        let intensity = light.f32("intensity", 1.0);
        // `range` over its older name `radius`
        let range = light
            .opt_f32("range")
            .or_else(|| light.opt_f32("radius"))
            .unwrap_or(10.0);
        let source_radius = light.f32("source_radius", 0.0);
        let cast_shadows = light.opt_bool("cast_shadows");
        let spot_shadow = kind == "spot"
            && cast_shadows == Some(true)
            && casts_first(&light, "spot", &name, &mut spot_casts);
        let point_shadow = kind == "point"
            && cast_shadows == Some(true)
            && casts_first(&light, "point", &name, &mut point_casts);
        if kind == "directional" {
            say_shadow_not_taken(&light, cast_shadows);
        }

        match kind {
            "directional" => found.directionals.push(Found {
                name,
                position,
                light: DirectionalLight {
                    // Toward the light: with no `direction`, the light's
                    // rays run down the entity's own -Y, so it is the
                    // entity's +Y (a sun overhead when unturned)
                    direction: light
                        .opt_vec3("direction")
                        .unwrap_or_else(|| turned(&matrix, [0.0, 1.0, 0.0])),
                    volumetric_intensity: light.f32("volumetric_intensity", 0.0),
                    color,
                    intensity,
                    volumetric_color: light.vec3("volumetric_color", color),
                    // Apparent source angular size, authored in degrees
                    // (sun ~0.5, softbox 2-5); drives PCSS penumbra (ADR
                    // 0056). Stored in radians.
                    angular_size: light.f32("angular_size", 0.0).to_radians(),
                },
            }),
            "point" => found.points.push(Found {
                name,
                position,
                light: PointLight {
                    position,
                    radius: range,
                    color,
                    intensity,
                    source_radius,
                    shadow: if point_shadow { 1.0 } else { 0.0 },
                    _pad1: 0.0,
                    _pad2: 0.0,
                },
            }),
            _ => found.spots.push(Found {
                name,
                position,
                light: SpotLight {
                    position,
                    radius: range,
                    // The way it shines: with no `direction`, down the
                    // entity's own -Y, so a spot on a turned parent turns
                    direction: light
                        .opt_vec3("direction")
                        .unwrap_or_else(|| turned(&matrix, [0.0, -1.0, 0.0])),
                    inner_angle: cone_angle(&light, "inner_angle", INNER_ANGLE),
                    color,
                    outer_angle: cone_angle(&light, "outer_angle", OUTER_ANGLE),
                    intensity,
                    source_radius,
                    shadow: if spot_shadow { 1.0 } else { 0.0 },
                    _pad2: 0.0,
                },
            }),
        }
    }
    found
}

/// Of the spots a camera's frame uploaded, the one whose shadow is drawn:
/// the one marked (there is one at most) when it gives light.
pub(crate) fn casting_spot(uploaded: &[SpotLight]) -> Option<&SpotLight> {
    uploaded
        .iter()
        .find(|spot| spot.shadow > 0.5 && spot.intensity > 0.0)
}

/// Of the point lights a camera's frame uploaded, the one whose cube map is
/// drawn (#1112): the one marked (there is one at most) when it gives light.
pub(crate) fn casting_point(uploaded: &[PointLight]) -> Option<&PointLight> {
    uploaded
        .iter()
        .find(|point| point.shadow > 0.5 && point.intensity > 0.0)
}

/// Whether this light is the one of its `kind` (spot or point) that casts:
/// the first by name to ask. A later one is said once, naming the light that
/// does (#504, #1112).
fn casts_first(
    light: &ComponentView<'_>,
    kind: &str,
    name: &str,
    first: &mut Option<String>,
) -> bool {
    let Some(caster) = first.as_deref() else {
        *first = Some(name.to_string());
        return true;
    };
    light.report("cast_shadows", || {
        Diagnostic::warning(
            codes::SHADOW_CAP,
            format!(
                "one {kind} light casts a shadow, the first by name: '{caster}' does, \
                 so this one lights without one"
            ),
        )
        .hint(if kind == "spot" {
            "keep `cast_shadows` on the key light alone"
        } else {
            "keep `cast_shadows` on the lamp whose shadow matters most"
        })
    });
    false
}

/// A `false` on the strongest directional light is said once, not passed
/// over (#504): it casts the cascades whatever it says.
fn say_shadow_not_taken(light: &ComponentView<'_>, cast_shadows: Option<bool>) {
    if cast_shadows == Some(false) {
        light.report("cast_shadows", || {
            Diagnostic::warning(
                codes::SHADOW_CAP,
                "the strongest directional light casts the cascades whatever \
                 `cast_shadows` says",
            )
            .hint(
                "`material.cast_shadows = false` on what shouldn't cast, or \
                 `[environment] shadows = false` for none at all",
            )
        });
    }
}

const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// The light's kind, or `None` when it lights nothing: an unknown `type`,
/// said once with the nearest kind. A light with no `type` is directional,
/// said once too, since a lamp that forgot its `type` would otherwise be a
/// sun with no word (#682).
fn light_type(light: &ComponentView<'_>) -> Option<&'static str> {
    if !light.exists() {
        return None;
    }
    let Some(written) = light.raw("type") else {
        light.report("type", || {
            Diagnostic::warning(
                codes::MISSING_FIELD,
                "a light with no `type` is a directional light",
            )
            .hint("write type = \"directional\", \"point\" or \"spot\"")
        });
        return Some("directional");
    };
    if let Some(kind) = written
        .as_str()
        .and_then(|w| LIGHT_TYPES.iter().find(|k| **k == w))
    {
        return Some(kind);
    }
    light.report("type", || {
        let shown = written
            .as_str()
            .map(|w| format!("\"{w}\""))
            .unwrap_or_else(|| written.to_string());
        let mut hint = String::new();
        if let Some(near) = written
            .as_str()
            .and_then(|w| diagnostic::did_you_mean(w, LIGHT_TYPES.iter().copied()))
        {
            hint.push_str(&near);
            hint.push(' ');
        }
        hint.push_str("a light is directional, point or spot");
        Diagnostic::error(
            codes::BAD_ENUM_VALUE,
            format!("{shown} is not a light type, so this light is off"),
        )
        .hint(hint)
    });
    None
}

/// A spot cone's half-angle in radians, from `<key>_deg` (degrees, as
/// transforms and `angular_size` are) or else `<key>` (radians, the older
/// form). Both written: the degrees win, said once. A radian value past a
/// quarter turn is almost surely degrees written in the wrong key: said
/// once, with the key it belongs in (#682).
fn cone_angle(light: &ComponentView<'_>, key: &str, default: f32) -> f32 {
    let deg_key = format!("{key}_deg");
    if let Some(degrees) = light.opt_f32(&deg_key) {
        if light.has(key) {
            light.report(key, || {
                Diagnostic::warning(
                    codes::TWO_FORMS,
                    format!("`{deg_key}` is written too, and wins"),
                )
                .hint(format!("keep one: `{deg_key}` in degrees"))
            });
        }
        return degrees.to_radians();
    }
    let radians = light.f32(key, default);
    if radians > std::f32::consts::FRAC_PI_2 {
        light.report(key, || {
            Diagnostic::warning(
                codes::OUT_OF_RANGE,
                format!("{radians} is past a quarter turn, and `{key}` is in radians"),
            )
            .hint(format!(
                "for {radians} degrees, write `{deg_key} = {radians}`"
            ))
        });
    }
    radians
}

/// `local` turned by the matrix's rotation (and scale, then normalized):
/// the way a light with no `direction` points. A matrix that crushes it to
/// nothing leaves it as written.
fn turned(m: &[[f32; 4]; 4], local: [f32; 3]) -> [f32; 3] {
    let mut v = [0.0f32; 3];
    for (row, out) in v.iter_mut().enumerate() {
        *out = m[0][row] * local[0] + m[1][row] * local[1] + m[2][row] * local[2];
    }
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-6 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        local
    }
}

/// The lights the shader takes, up to `N`: all of them in name order when
/// they fit, else the `N` nearest `camera` (ties to the earlier name), still
/// in name order, so a scene under the cap draws as it always has. Also
/// how many, and the names of those left out.
pub(crate) fn nearest<T: Copy + Default, const N: usize>(
    found: &[Found<T>],
    camera: [f32; 3],
) -> ([T; N], u32, Vec<&str>) {
    let mut out = [T::default(); N];
    if found.len() <= N {
        for (slot, f) in out.iter_mut().zip(found) {
            *slot = f.light;
        }
        return (out, found.len() as u32, Vec::new());
    }
    let distance = |f: &Found<T>| {
        (0..3)
            .map(|i| (f.position[i] - camera[i]).powi(2))
            .sum::<f32>()
    };
    let mut order: Vec<usize> = (0..found.len()).collect();
    order.sort_by(|&a, &b| {
        distance(&found[a])
            .partial_cmp(&distance(&found[b]))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(&b))
    });
    let mut kept = vec![false; found.len()];
    for &i in &order[..N] {
        kept[i] = true;
    }
    let mut count = 0;
    let mut left_out = Vec::new();
    for (f, kept) in found.iter().zip(kept) {
        if kept {
            out[count] = f.light;
            count += 1;
        } else {
            left_out.push(f.name.as_str());
        }
    }
    (out, N as u32, left_out)
}

/// Say once a scene, by name, that a light is past its type's cap and so
/// is not drawn while it is.
fn say_past_cap(
    said: &mut BTreeSet<String>,
    name: &str,
    kind: &str,
    cap: usize,
    kept: &str,
) -> bool {
    if !said.insert(name.to_string()) {
        return false;
    }
    let diagnostic = Diagnostic::warning(
        codes::LIGHT_CAP,
        format!(
            "the shader takes {cap} {kind} lights, {kept}, and this one is not among them; \
             it is off while it isn't"
        ),
    )
    .entity(name)
    .component(comp::LIGHT)
    .hint(format!(
        "fewer {kind} lights, or merge near ones into one stronger light"
    ));
    tracing::warn!("{}", diagnostic.one_line());
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> toml::Value {
        toml::from_str(text).unwrap()
    }

    /// A world of `(name, light table, transform table)`.
    fn world_of(lights: &[(&str, &str, &str)]) -> FlintWorld {
        let mut world = FlintWorld::new();
        for (name, light, transform) in lights {
            let id = world.spawn(*name).unwrap();
            world.set_component(id, comp::LIGHT, table(light)).unwrap();
            if !transform.is_empty() {
                world
                    .set_component(id, comp::TRANSFORM, table(transform))
                    .unwrap();
            }
        }
        world
    }

    fn said(world: &FlintWorld) -> Vec<String> {
        world
            .read_reports()
            .snapshot()
            .iter()
            .map(|d| d.one_line())
            .collect()
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-5)
    }

    #[test]
    fn an_unknown_type_is_off_and_said_once_by_name() {
        let world = world_of(&[("lamp", "type = \"spott\"\nintensity = 30", "")]);
        let found = gather(&world, &mut MotionHold::default());
        assert!(found.spots.is_empty() && found.points.is_empty());
        assert!(found.directionals.is_empty());
        assert_eq!(found.unusable, 1);
        gather(&world, &mut MotionHold::default());
        let said = said(&world);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains("entity 'lamp'"), "{}", said[0]);
        assert!(
            said[0].contains("\"spott\" is not a light type"),
            "{}",
            said[0]
        );
        assert!(said[0].contains("did you mean `spot`?"), "{}", said[0]);
    }

    #[test]
    fn the_first_spot_by_name_that_asks_casts_and_the_next_is_said() {
        let world = world_of(&[
            ("b_key", "type = \"spot\"\ncast_shadows = true", ""),
            ("a_rim", "type = \"spot\"", ""),
            ("c_fill", "type = \"spot\"\ncast_shadows = true", ""),
        ]);
        let found = gather(&world, &mut MotionHold::default());
        let shadow: Vec<(&str, f32)> = found
            .spots
            .iter()
            .map(|f| (f.name.as_str(), f.light.shadow))
            .collect();
        assert_eq!(shadow, [("a_rim", 0.0), ("b_key", 1.0), ("c_fill", 0.0)]);
        gather(&world, &mut MotionHold::default());
        let said = said(&world);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0].contains("warning[shadow-cap] entity 'c_fill' light.cast_shadows"),
            "{}",
            said[0]
        );
        assert!(said[0].contains("'b_key' does"), "{}", said[0]);
    }

    #[test]
    fn no_spot_casts_unless_one_asks() {
        let world = world_of(&[
            ("lamp", "type = \"spot\"", ""),
            ("off", "type = \"spot\"\ncast_shadows = false", ""),
        ]);
        let found = gather(&world, &mut MotionHold::default());
        assert!(found.spots.iter().all(|f| f.light.shadow == 0.0));
        let uploaded: Vec<SpotLight> = found.spots.iter().map(|f| f.light).collect();
        assert!(casting_spot(&uploaded).is_none());
        assert!(said(&world).is_empty());
    }

    #[test]
    fn the_casting_spot_is_the_marked_one_that_gives_light() {
        let lit = SpotLight {
            intensity: 5.0,
            ..SpotLight::default()
        };
        let marked = SpotLight { shadow: 1.0, ..lit };
        let dark = SpotLight {
            intensity: 0.0,
            ..marked
        };
        assert_eq!(casting_spot(&[lit, marked]).map(|s| s.shadow), Some(1.0));
        assert!(
            casting_spot(&[lit, dark]).is_none(),
            "a dark spot casts nothing"
        );
        assert!(casting_spot(&[lit]).is_none());
    }

    #[test]
    fn a_cast_shadows_a_light_cant_honour_is_said() {
        let world = world_of(&[
            ("bulb", "type = \"point\"\ncast_shadows = true", ""),
            ("sun", "type = \"directional\"\ncast_shadows = false", ""),
            ("moon", "type = \"directional\"\ncast_shadows = true", ""),
            ("lamp", "type = \"point\"\ncast_shadows = false", ""),
        ]);
        gather(&world, &mut MotionHold::default());
        let said = said(&world);
        // A point light that asks casts since #1112: only the sun is said
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0].contains("entity 'sun' light.cast_shadows"),
            "{}",
            said[0]
        );
        assert!(
            said[0].contains("whatever `cast_shadows` says"),
            "{}",
            said[0]
        );
    }

    #[test]
    fn the_first_point_by_name_that_asks_casts_and_the_next_is_said() {
        let world = world_of(&[
            ("b_lantern", "type = \"point\"\ncast_shadows = true", ""),
            ("a_candle", "type = \"point\"", ""),
            ("c_lamp", "type = \"point\"\ncast_shadows = true", ""),
            // A spot that asks casts too: the two kinds don't share one
            ("d_key", "type = \"spot\"\ncast_shadows = true", ""),
        ]);
        let found = gather(&world, &mut MotionHold::default());
        let shadow: Vec<(&str, f32)> = found
            .points
            .iter()
            .map(|f| (f.name.as_str(), f.light.shadow))
            .collect();
        assert_eq!(
            shadow,
            [("a_candle", 0.0), ("b_lantern", 1.0), ("c_lamp", 0.0)]
        );
        assert_eq!(found.spots[0].light.shadow, 1.0);
        gather(&world, &mut MotionHold::default());
        let said = said(&world);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0].contains("warning[shadow-cap] entity 'c_lamp' light.cast_shadows"),
            "{}",
            said[0]
        );
        assert!(
            said[0].contains("one point light casts a shadow, the first by name: 'b_lantern' does"),
            "{}",
            said[0]
        );
    }

    #[test]
    fn the_casting_point_is_the_marked_one_that_gives_light() {
        let lit = PointLight {
            intensity: 5.0,
            ..PointLight::default()
        };
        let marked = PointLight { shadow: 1.0, ..lit };
        let dark = PointLight {
            intensity: 0.0,
            ..marked
        };
        assert_eq!(casting_point(&[lit, marked]).map(|p| p.shadow), Some(1.0));
        assert!(
            casting_point(&[lit, dark]).is_none(),
            "a dark lamp casts nothing"
        );
        assert!(casting_point(&[lit]).is_none());
    }

    #[test]
    fn a_missing_type_is_a_directional_light_and_said() {
        let world = world_of(&[("lamp", "intensity = 3", "")]);
        let found = gather(&world, &mut MotionHold::default());
        assert_eq!(found.directionals.len(), 1);
        let said = said(&world);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains("warning[missing-field] entity 'lamp' light.type"));
    }

    #[test]
    fn a_spot_with_no_direction_turns_with_its_parent() {
        let mut world = world_of(&[("torch", "type = \"spot\"", "position = [0.0, 0.0, 0.0]")]);
        let arm = world.spawn("arm").unwrap();
        world
            .set_component(
                arm,
                comp::TRANSFORM,
                table("position = [0.0, 4.0, 0.0]\nrotation = [0.0, 0.0, 90.0]"),
            )
            .unwrap();
        let torch = world.get_id("torch").unwrap();
        world.set_parent(torch, arm).unwrap();

        let found = gather(&world, &mut MotionHold::default());
        let spot = found.spots[0].light;
        assert!(close(spot.position, [0.0, 4.0, 0.0]), "{:?}", spot.position);
        // A quarter turn about z takes the spot's own -Y to +X
        assert!(
            close(spot.direction, [1.0, 0.0, 0.0]),
            "{:?}",
            spot.direction
        );
    }

    #[test]
    fn a_written_direction_is_world_space_and_kept() {
        let world = world_of(&[(
            "torch",
            "type = \"spot\"\ndirection = [0.0, 0.0, -1.0]",
            "rotation = [0.0, 0.0, 90.0]",
        )]);
        let found = gather(&world, &mut MotionHold::default());
        assert_eq!(found.spots[0].light.direction, [0.0, 0.0, -1.0]);
    }

    #[test]
    fn an_unturned_light_with_no_direction_shines_down() {
        let world = world_of(&[
            ("sun", "type = \"directional\"", ""),
            ("spot", "type = \"spot\"", ""),
        ]);
        let found = gather(&world, &mut MotionHold::default());
        // Toward the light: a sun overhead, not below the ground
        assert_eq!(found.directionals[0].light.direction, [0.0, 1.0, 0.0]);
        assert_eq!(found.spots[0].light.direction, [0.0, -1.0, 0.0]);
    }

    #[test]
    fn cone_angles_take_degrees_and_say_a_radian_key_holding_degrees() {
        let world = world_of(&[
            (
                "deg",
                "type = \"spot\"\ninner_angle_deg = 10\nouter_angle_deg = 30.0",
                "",
            ),
            (
                "rad",
                "type = \"spot\"\ninner_angle = 0.2\nouter_angle = 0.5",
                "",
            ),
            ("unset", "type = \"spot\"", ""),
            (
                "both",
                "type = \"spot\"\ninner_angle = 0.2\ninner_angle_deg = 20",
                "",
            ),
            ("wrong", "type = \"spot\"\nouter_angle = 30", ""),
        ]);
        let found = gather(&world, &mut MotionHold::default());
        let by = |name: &str| found.spots.iter().find(|f| f.name == name).unwrap().light;
        assert!((by("deg").inner_angle - 10f32.to_radians()).abs() < 1e-6);
        assert!((by("deg").outer_angle - 30f32.to_radians()).abs() < 1e-6);
        assert_eq!((by("rad").inner_angle, by("rad").outer_angle), (0.2, 0.5));
        assert_eq!(
            (by("unset").inner_angle, by("unset").outer_angle),
            (0.3, 0.5)
        );
        assert!((by("both").inner_angle - 20f32.to_radians()).abs() < 1e-6);

        let said = said(&world);
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(said
            .iter()
            .any(|s| s.contains("warning[two-forms] entity 'both'")
                && s.contains("`inner_angle_deg` is written too")));
        assert!(said
            .iter()
            .any(|s| s.contains("warning[out-of-range] entity 'wrong'")
                && s.contains("write `outer_angle_deg = 30`")));
    }

    fn lamps(n: usize) -> Vec<Found<PointLight>> {
        (0..n)
            .map(|i| Found {
                name: format!("lamp_{:02}", i + 1),
                position: [i as f32, 0.0, 0.0],
                light: PointLight {
                    intensity: (i + 1) as f32,
                    ..PointLight::default()
                },
            })
            .collect()
    }

    #[test]
    fn under_the_cap_every_light_is_kept_in_name_order() {
        let found = lamps(MAX_POINT_LIGHTS);
        let (lights, count, left_out) = nearest::<_, MAX_POINT_LIGHTS>(&found, [1000.0, 0.0, 0.0]);
        assert_eq!(count as usize, MAX_POINT_LIGHTS);
        assert!(left_out.is_empty());
        let order: Vec<f32> = lights.iter().map(|l| l.intensity).collect();
        let want: Vec<f32> = (1..=MAX_POINT_LIGHTS).map(|i| i as f32).collect();
        assert_eq!(order, want);
    }

    #[test]
    fn past_the_cap_the_nearest_the_camera_are_kept() {
        let found = lamps(MAX_POINT_LIGHTS + 3);
        // The camera stands past the last lamp: the first three are farthest
        let camera = [(MAX_POINT_LIGHTS + 5) as f32, 1.0, 0.0];
        let (lights, count, left_out) = nearest::<_, MAX_POINT_LIGHTS>(&found, camera);
        assert_eq!(count as usize, MAX_POINT_LIGHTS);
        assert_eq!(left_out, ["lamp_01", "lamp_02", "lamp_03"]);
        // Still in name order, so the uniforms match a scene of just these
        let order: Vec<f32> = lights.iter().map(|l| l.intensity).collect();
        let want: Vec<f32> = (4..=MAX_POINT_LIGHTS + 3).map(|i| i as f32).collect();
        assert_eq!(order, want);

        // From the other end, the last three go
        let (_, _, left_out) = nearest::<_, MAX_POINT_LIGHTS>(&found, [-5.0, 0.0, 0.0]);
        assert_eq!(left_out, ["lamp_17", "lamp_18", "lamp_19"]);
    }

    #[test]
    fn a_light_past_the_cap_is_said_once_a_scene() {
        let mut said = BTreeSet::new();
        let why = "the nearest the camera";
        assert!(say_past_cap(&mut said, "lamp_01", "point", 16, why));
        assert!(!say_past_cap(&mut said, "lamp_01", "point", 16, why));
        assert!(say_past_cap(&mut said, "lamp_02", "point", 16, why));
    }
}
