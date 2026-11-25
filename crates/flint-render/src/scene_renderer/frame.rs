//! The frame (ADR 0118, #736): what a host calls each frame to draw the
//! scene, and the order the frame's work is recorded in.
//!
//! - `render` (a window) and `render_to` (any target, with its own submit)
//!   both go through `encode_to`, which records the whole frame into the
//!   caller's encoder: the grass compute, the shadow cascades, the
//!   per-frame uniforms, the scene pass (split around the ocean's
//!   refraction grab when water draws, with MSAA's depth resolved after
//!   each opaque phase), the style mask and albedo, the post chain, the
//!   depth upscale and the world lines (#139, one submission a frame).
//! - `encode_again_to` draws the same frame into another target.
//! - `frame_submitted` runs after the host's submit: the grass count and GPU
//!   timer read-backs.
//!
//! The subsystems own their state (`msaa`, `sky`, `lights`, `terrain`,
//! `style_mask`, `water`, `skinning`, `grass`) and this file orchestrates
//! them.
//!
//! The host protocol (ADR 0118 decision 5, #739), the same for the player,
//! the viewer and `flint render`:
//!
//! ```text
//! bind_scene(world, &SceneBinding)        once a scene load (binding.rs)
//! each frame:
//!   set_motion_time(t), then bone writes  (only a host that writes bones)
//!   prepare(world, &FrameInputs)          everything the frame reads
//!   encode_to / encode_frame_to           record it
//!   submit, then frame_submitted
//! ```
//!
//! [`FrameInputs`] is every per-frame input in one struct: a field a host
//! leaves at its default turns its feature off for the frame, so nothing a
//! host forgot to set lingers from an earlier frame.

use super::*;

/// What a frame draws that the world doesn't say (ADR 0118 decision 5,
/// #739), handed to [`SceneRenderer::prepare`] once a frame. Every field has
/// a default that turns its feature off, so a host names only what it has:
/// `FrameInputs { time, dt, pixels_per_point, ..Default::default() }`.
///
/// One clock: `time` is the frame's game time, and everything that moves
/// runs on it unless a host holds part of it still (`look_time`,
/// `grass_time`).
#[derive(Clone)]
pub struct FrameInputs<'a> {
    /// The frame's clock, in seconds of game time (the player's
    /// `total_time`, a capture's moment). Motion held on twos ticks on it
    /// (#405), and so, unless held below, do the ocean's waves and its
    /// contact foam, the sky, the post chain's grain and animated styles
    /// (`post_time`), and the grass's wind.
    pub time: f64,
    /// Seconds this frame stepped, for styles whose history is a rate
    /// (ASCII's smoothing, #342). Default 1/60, what a per-frame weight
    /// always meant.
    pub dt: f32,
    /// Holds the ocean, the sky and the post chain at this moment while
    /// `time` runs on (`--style-time`, #328). `None`: they run on `time`.
    pub look_time: Option<f64>,
    /// Holds the grass's wind at this moment (woozy grass's sway, #252, or a
    /// still's rest). `None`: the wind runs on `time`.
    pub grass_time: Option<f32>,
    /// The comb of woozy grass this frame (#252). `None` combs nothing.
    pub grass_wooze: Option<ShallowWoozeDraw>,
    /// Each woozy entity's drift this frame (#215). Empty: every mesh at rest.
    pub shallow_wooze: &'a [(flint_core::EntityId, ShallowWoozeDraw)],
    /// The shading boil's drawing this frame (#214), from whoever owns the
    /// boil clock. The default boils nothing.
    pub boil: flint_core::field_frame::BoilFrame,
    /// Physical pixels per logical point of the target: styles sized in
    /// points (ASCII cells, #265), comic panels' gutters and world lines'
    /// widths scale by it. Default 1, a headless target.
    pub pixels_per_point: f32,
    /// How 2D sprites and screen-anchored entities are placed (ADR 0063).
    pub view_2d: View2d,
    /// The particles to draw, packed (`ParticleSystem::pack`). `None` draws
    /// none.
    pub particles: Option<&'a flint_particles::ParticleSync>,
    /// Depth-tested script lines (ADR 0083), widths in logical points.
    pub world_lines: &'a [flint_core::world_line::WorldLine],
    /// What bends the grass this frame (bend-on-contact). Empty bends none.
    pub grass_benders: &'a [GrassEntityPosition],
    /// Each `ocean_contact` hull's velocity in m/s this frame, from whoever
    /// knows it (the player's physics, for a dynamic body; #749). A hull
    /// not listed has its drawn position differenced across frames instead.
    pub contact_velocities: &'a [(flint_core::EntityId, [f32; 3])],
    /// A tint for the sea this frame over its `ocean` component's (#746),
    /// from a host that drives one without the component: the
    /// `set_render_mode` shim's blood (2) and underside (5). `None` draws
    /// the component's.
    pub ocean_tint: Option<crate::ocean_pipeline::OceanTint>,
    /// A frame debugger's controls over what is drawn (#1089): entities
    /// left out of every pass, and one pass stopped after its Nth draw.
    /// Debug only: `None`, every frame a game draws, changes nothing.
    pub draw_controls: Option<&'a DrawControls>,
    /// The screen field the game filled this frame (#859): what a style
    /// with `blend_source = "field"` follows. Empty: no field, and such a
    /// style is off as at blend 0. Past `MAX_FIELD_SHAPES` the rest are
    /// dropped.
    pub style_field: &'a [flint_core::style_field::FieldShape],
}

/// What a frame debugger's replay leaves out of a frame (#1089,
/// [`FrameInputs::draw_controls`]). Applied as cull flags on the frame's
/// draw items, the way the frustum and LOD culls are, so a frame with no
/// controls takes the path it always did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DrawControls {
    /// Entities drawn in no pass: not in the shadows, the scene, the style
    /// mask or the selection. Grass, water and the sky aren't entity draws
    /// and stay.
    pub hidden: Vec<flint_core::EntityId>,
    /// One pass drawn only up to its Nth draw (#1204); every other pass
    /// draws whole.
    pub limit: Option<DrawLimit>,
    /// The scene pass's draws to time on the GPU, by their place in its
    /// list as the description has them (#1203): a timestamp before and
    /// after each, inside the pass, on a device with
    /// `TIMESTAMP_QUERY_INSIDE_PASSES`. At most
    /// [`DRAW_TIME_BATCH`](super::SceneRenderer::DRAW_TIME_BATCH) a frame;
    /// read with [`read_draw_times`](super::SceneRenderer::read_draw_times).
    pub time_draws: Option<std::ops::Range<usize>>,
    /// Draw the scene pass's overdraw after it (#1203): how many of its
    /// draws shade each pixel, as the `Overdraw` pass's `overdraw_count`
    /// and its heat map `overdraw`.
    pub overdraw: bool,
}

impl DrawControls {
    /// The draw limit, when it is `pass`'s.
    pub fn limit_of(&self, pass: SteppedPass) -> Option<usize> {
        self.limit.filter(|l| l.pass == pass).map(|l| l.draws)
    }
}

/// A pass stopped after its first `draws` draws (#1204), counted as the
/// frame's description lists that pass's draws (`flint frame show --draws
/// --pass <name>`): step N ends on its row N.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrawLimit {
    pub pass: SteppedPass,
    pub draws: usize,
}

/// A pass a replay can step a draw at a time (#1204): one whose draws are
/// the frame's draw items again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SteppedPass {
    /// The scene pass: the entities' draws with grass and 2D sprite batches
    /// among them; the sky, water and grid aren't counted and always draw.
    /// What follows the scene (the style mask and albedo, the fog veil, the
    /// selection) draws only what it drew.
    Scene,
    /// A shadow map layer, by index: a cascade (0 to 2), the casting
    /// spot's ([`SPOT_SHADOW_LAYER`](crate::shadow::SPOT_SHADOW_LAYER)) or
    /// the casting point light's six faces
    /// ([`POINT_SHADOW_LAYER`](crate::shadow::POINT_SHADOW_LAYER)).
    /// Its casters in list order; the grass it casts isn't counted. The
    /// scene still reads the whole of every other layer.
    Shadow(usize),
    /// The entity style mask's styled draws; the veil over them stays whole.
    StyleMask,
    /// The style albedo's opaque meshes.
    StyleAlbedo,
}

impl SteppedPass {
    /// Every pass that can be stepped, in frame order.
    pub fn all() -> impl Iterator<Item = SteppedPass> {
        std::iter::once(SteppedPass::Scene)
            .chain((0..crate::shadow::SHADOW_LAYERS).map(SteppedPass::Shadow))
            .chain([SteppedPass::StyleMask, SteppedPass::StyleAlbedo])
    }

    /// Its pass's name in the frame's description (`Shadow cascade 0`).
    pub fn name(self) -> &'static str {
        match self {
            SteppedPass::Scene => "Scene",
            SteppedPass::Shadow(n) => super::render_passes::SHADOW_PASS_LABELS
                .get(n)
                .copied()
                .unwrap_or("Shadow"),
            SteppedPass::StyleMask => "Style mask",
            SteppedPass::StyleAlbedo => "Style albedo",
        }
    }

    /// The pass named `name` in a frame's description, if it can be stepped.
    pub fn from_name(name: &str) -> Option<SteppedPass> {
        Self::all().find(|p| p.name().eq_ignore_ascii_case(name.trim()))
    }
}

impl Default for FrameInputs<'_> {
    fn default() -> Self {
        Self {
            time: 0.0,
            dt: 1.0 / 60.0,
            look_time: None,
            grass_time: None,
            grass_wooze: None,
            shallow_wooze: &[],
            boil: Default::default(),
            pixels_per_point: 1.0,
            view_2d: View2d::default(),
            particles: None,
            world_lines: &[],
            grass_benders: &[],
            contact_velocities: &[],
            ocean_tint: None,
            draw_controls: None,
            style_field: &[],
        }
    }
}

/// The 2D view sprites and screen-anchored entities are laid out in (ADR
/// 0063): the camera's offset in world units and the orthographic
/// viewport's height and aspect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View2d {
    pub camera_offset: [f32; 2],
    pub ortho_height: f32,
    pub aspect_ratio: f32,
}

impl Default for View2d {
    fn default() -> Self {
        Self {
            camera_offset: [0.0, 0.0],
            ortho_height: 10.0,
            aspect_ratio: 16.0 / 9.0,
        }
    }
}

impl View2d {
    /// The view `camera` gives: its position's x and y, its orthographic
    /// height (10 when it has none), its aspect.
    pub fn of(camera: &Camera) -> Self {
        Self {
            camera_offset: [camera.position.x, camera.position.y],
            ortho_height: if camera.ortho_height > 0.0 {
                camera.ortho_height
            } else {
                10.0
            },
            aspect_ratio: camera.aspect,
        }
    }
}

impl SceneRenderer {
    // ── The frame's inputs ──

    /// Take this frame's inputs and read the world into the draw lists
    /// (#739): the one call between a scene's bone writes and `encode_to`.
    /// Everything `inputs` carries replaces the last frame's, so a field at
    /// its default is off this frame, whatever an earlier one set.
    ///
    /// Bones are written before it, after [`set_motion_time`]
    /// (Self::set_motion_time) stamps the same `time`: a held entity's bones
    /// are gated by its hold's tick (#405), and the uploads precede the
    /// extraction that reads them.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &FlintWorld,
        inputs: &FrameInputs<'_>,
    ) {
        flint_core::scope!("prepare");
        // A described frame starts its log here (#1084)
        if let Some(log) = &mut self.frame_log {
            **log = Default::default();
        }
        // and a captured one its copies (#1085)
        self.reset_frame_taps();
        // A replay's draw times and overdraw (#1203); a game's frame names
        // neither
        let controls = inputs.draw_controls;
        self.draw_times
            .arm(device, queue, controls.and_then(|c| c.time_draws.clone()));
        self.overdraw.wanted = controls.is_some_and(|c| c.overdraw);
        self.take_inputs(queue, inputs);
        self.update_from_world(world, device);
        match inputs.particles {
            Some(sync) => self.update_particles_from(device, queue, sync),
            None => self.update_particles(device, queue, &[]),
        }
    }

    /// [`prepare`](Self::prepare) again within the frame, for the other half
    /// of a split view (#238): the same inputs, with that side's boil and
    /// shallow wooze, and the world read again. Draw it with
    /// [`encode_again_to`](Self::encode_again_to), which draws that boil
    /// once; a later `encode_to` draws the frame's own again. The draws and
    /// the shallow wooze stay this side's until the next `prepare`.
    pub fn prepare_again(
        &mut self,
        device: &wgpu::Device,
        world: &FlintWorld,
        boil: flint_core::field_frame::BoilFrame,
        shallow_wooze: &[(flint_core::EntityId, ShallowWoozeDraw)],
    ) {
        self.again_boil = Some(boil);
        self.set_shallow_wooze(shallow_wooze.iter().copied());
        self.update_from_world(world, device);
    }

    /// The pixels per point the last [`prepare`](Self::prepare) took (1
    /// before any).
    pub fn pixels_per_point(&self) -> f32 {
        self.pixels_per_point
    }

    /// Everything of `inputs` but the world and the particles.
    fn take_inputs(&mut self, queue: &wgpu::Queue, inputs: &FrameInputs<'_>) {
        // The one clock and what holds part of it
        self.set_motion_time(inputs.time);
        self.ocean_time = inputs.look_time.unwrap_or(inputs.time);
        self.grass_time = inputs.grass_time.unwrap_or(inputs.time as f32);
        self.grass_wooze = inputs.grass_wooze;
        self.style_dt = inputs.dt;
        // The boil is the frame's, whatever config a host sets before
        // `encode_to`: it goes into the drawn config there too
        self.frame_boil = Some(inputs.boil);
        self.again_boil = None;
        self.set_boil(inputs.boil);
        self.pixels_per_point = inputs.pixels_per_point;
        let View2d {
            camera_offset,
            ortho_height,
            aspect_ratio,
        } = inputs.view_2d;
        self.camera_offset = camera_offset;
        self.ortho_height = ortho_height;
        self.aspect_ratio = aspect_ratio;
        self.set_shallow_wooze(inputs.shallow_wooze.iter().copied());
        self.update_grass_entities(queue, inputs.grass_benders);
        self.water.set_contact_velocities(inputs.contact_velocities);
        self.water.set_frame_tint(inputs.ocean_tint);
        self.world_lines.clear();
        self.world_lines.extend_from_slice(inputs.world_lines);
        self.world_line_px_per_point = inputs.pixels_per_point;
        self.draws.set_controls(inputs.draw_controls);
        // The field (#859) before the world is read, so the ids that follow
        // it fade or show by this frame's
        self.style_mask.field.set(inputs.style_field);
        self.style_mask
            .ids
            .set_field(!self.style_mask.field.is_empty());
    }

    /// The boil the last [`prepare`](Self::prepare) took (or a split view's
    /// other side's, [`prepare_again`](Self::prepare_again)), into the drawn
    /// and authored configs, in case a host set a config since (a look, a
    /// panel's edit) that carries another. A renderer never prepared keeps
    /// the config's own.
    fn stamp_frame_boil(&mut self) {
        if let Some(boil) = self.again_boil.or(self.frame_boil) {
            self.postprocess_config.boil = boil;
            self.authored_postprocess.boil = boil;
        }
    }

    // ── Render entry points ──

    /// Render the scene using a RenderContext (windowed mode)
    pub fn render(
        &mut self,
        context: &RenderContext,
        camera: &Camera,
        view: &wgpu::TextureView,
    ) -> Result<(), crate::SurfaceError> {
        if self.device_lost {
            return Ok(());
        }
        self.render_to(
            &context.device,
            &context.queue,
            context.depth_target(),
            camera,
            view,
        );
        Ok(())
    }

    // ── Slim render_to: orchestrates shadow → uniforms → main pass → postprocess ──

    /// Render the scene to an arbitrary texture view with explicit device/queue/depth
    pub fn render_to<'d>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        depth: impl Into<crate::bind_cache::DepthTarget<'d>>,
        camera: &Camera,
        target_view: &wgpu::TextureView,
    ) {
        if self.device_lost {
            return;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Frame Encoder"),
        });
        self.encode_to(device, queue, &mut encoder, depth, camera, target_view);
        queue.submit(std::iter::once(encoder.finish()));
        self.frame_submitted();
    }

    /// [`render_to`](Self::render_to) into `encoder`, for a host that
    /// draws more into the frame (its HUD) and submits once (#139). On the
    /// web each submission crosses into JS; the frame used to make twenty.
    ///
    /// The frame's uniforms are written to `queue` now and reach the GPU
    /// with the next submission, so encode one camera per submission: a
    /// second camera's uniforms would overwrite this one's. After
    /// submitting, call [`frame_submitted`](Self::frame_submitted).
    ///
    /// `depth` is the caller's depth buffer: a `&TextureView` or a
    /// [`DepthTarget`](crate::bind_cache::DepthTarget)
    /// ([`RenderContext::depth_target`](crate::RenderContext::depth_target)).
    /// The bind groups over it are kept from frame to frame while it is the
    /// same view (#379, #960).
    pub fn encode_to<'d>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        depth: impl Into<crate::bind_cache::DepthTarget<'d>>,
        camera: &Camera,
        target_view: &wgpu::TextureView,
    ) {
        flint_core::scope!("encode_to");
        if self.device_lost {
            return;
        }
        let depth = depth.into();
        let depth_view = depth.view;
        self.stamp_frame_boil();
        // A style switched on by any route (scene load, F4, a script's
        // config) is built before the frame that first needs it (#262).
        // When nothing new is on this is a walk over the registry.
        self.sync_debug_styles();
        // A tear is a script's, set every frame, maybe after the world was
        // read: a prop whose blend follows it (#397) is off while the
        // config this frame draws has none
        self.style_mask
            .ids
            .set_tear(crate::postprocess::tear_active(&self.postprocess_config));
        self.style_mask
            .ids
            .set_field(!self.style_mask.field.is_empty());
        self.ensure_style_resources(device);
        // FXAA likewise (#297): a host that only sets the config (the
        // viewer's hot reload and scene/default toggle, a script's post
        // override) drew none until a resize. Two `Option` checks when built.
        self.ensure_fxaa_resources(device);
        if self.device_lost {
            return;
        }
        // Taken for the frame, so the phases can be scoped while self is
        // borrowed; put back at the end (ADR 0108). With them the frame's
        // description, while one is asked for (#1084): each part logs its
        // pass where it opens its scope.
        // With them a captured frame's copies (#1085): each pass's targets
        // are read as it ends (`tap_pass`), each timed in this queue's
        // ticks (#1160: a still arms the capture after its `prepare`)
        let mut scopes = crate::frame_capture::FrameScopes {
            timer: self.gpu_timer.take(),
            log: self.frame_log.take(),
            taps: self.frame_taps.take().map(|mut taps| {
                taps.times_in(queue);
                taps
            }),
            // A host's skip and repeat knobs (#1317), else None
            knobs: self.pass_knobs.take(),
        };
        self.begin_view_taps(&scopes);
        if let Some(log) = scopes.log() {
            log.begin_view(
                self.surface_size.into(),
                crate::frame_replay::CameraState::of(camera),
            );
        }
        // The depth the scene draws against, in the description's names
        let sd = self.scene_depth_id();
        // Below render scale 1 the scene draws against a depth buffer of
        // its own size, copied into the caller's once the frame is drawn
        // (#122). Every scene pass and effect below sees that one.
        let surface_depth_view = depth_view;
        let scaled_depth = self.scaled_depth.clone();
        let depth_view = scaled_depth.as_ref().map_or(depth_view, |d| &d.view);
        // Whose view it is, so bind groups over it can be kept: the chain's
        // own lives with the chain, the caller's while it's the same view
        // (#379, #960).
        let depth_key = if scaled_depth.is_some() {
            crate::bind_cache::DepthKey::Owned
        } else {
            crate::bind_cache::DepthKey::Caller(depth_view)
        };
        let clear_color = wgpu::Color {
            r: self.clear_color[0] as f64,
            g: self.clear_color[1] as f64,
            b: self.clear_color[2] as f64,
            a: self.clear_color[3] as f64,
        };
        let view_proj = camera.view_projection_matrix();
        self.camera_frustum = Some(crate::frustum::Frustum::from_view_projection(&view_proj));

        // Draws an earlier render_to culled (another camera, same extract)
        // come back for the shadow pass and this camera's test, and the
        // terrain's chunks are items as the bound terrain has them now.
        self.draws.uncull();
        self.draws
            .sync_terrain(self.terrain.draws.len(), self.terrain.entity);

        // Pick each model's level of detail for this camera and drop what is
        // past its draw distance (#56), before the shadow pass sees them.
        self.cull_draws_to_lod(camera.position_array());
        // and what a replay hides (#1089), out of every pass
        self.draws.cull_hidden();

        // Count visible terrain chunks for stats
        self.terrain.visible_chunks = if let Some(ref frustum) = self.camera_frustum {
            self.terrain
                .draws
                .iter()
                .filter(|d| frustum.aabb_visible(d.aabb_min, d.aabb_max))
                .count() as u32
        } else {
            self.terrain.draws.len() as u32
        };

        let camera_pos = camera.position_array();
        let debug_mode_u32 = self.debug_state.mode.as_u32();
        let wireframe_only = self.debug_state.mode == DebugMode::WireframeOnly;

        // Update light uniforms
        {
            flint_core::scope!("upload_lights");
            self.lights.upload(queue, camera_pos);
        }

        // Grass compute (ADR 0092): scatters this camera's blades, culled to
        // its view, and counts them into the indirect draw arguments. It goes
        // before the shadow pass so the cascades cast this frame's blades.
        // Nothing waits on it: the draws read the count on the GPU, and the
        // stats get it a frame or so late.
        self.grass.drawing = false;
        if self.grass.needs_realloc {
            if let Some(config) = self.grass.config.clone() {
                self.reload_grass_config(device, config);
            }
        }
        if self.grass.config.as_ref().is_some_and(|c| c.enabled) {
            let grass_time = self.grass_time;
            // It counts blades into its draw arguments, which reset once a
            // frame: a second run would double them (#1317)
            let n = scopes.times_refusing("Grass", Some(crate::pass_knobs::ADDS_INTO_ITS_TARGET));
            if let Some(scope) = scopes.begin_times(encoder, "Grass", n) {
                self.dispatch_grass_compute(device, queue, encoder, camera, grass_time);
                scopes.end(encoder, scope);
                scopes.note("Grass", |p| {
                    p.writes(&["grass_instances"]);
                });
                self.tap_pass(device, &mut scopes, encoder, "Grass");
                // Its count is read back once the frame is submitted
                self.grass.readback_due = true;
            } else {
                // Skipped: the scene draws the blades the last compute left
                self.grass.drawing = self.grass.draw_args_buffer.is_some();
            }
        } else {
            self.grass.instance_count = 0;
            let config = self.grass.config.is_some();
            scopes.skip("Grass", || {
                if config {
                    "grass is off (enabled = false)"
                } else {
                    "no grass in the scene"
                }
                .to_string()
            });
        }

        // Shadow pass: render depth from light perspective. It sees every
        // draw (culled per cascade), since a caster outside the view can
        // still shadow what's in it.
        let n = scopes.times("Shadows");
        if let Some(scope) = scopes.begin_times(encoder, "Shadows", n) {
            scopes.repeat(n, |scopes| {
                self.render_shadow_pass(device, queue, encoder, camera, camera_pos, scopes.log());
            });
            scopes.end(encoder, scope);
            self.tap_pass(device, &mut scopes, encoder, "Shadows");
        }

        // Set aside draws outside the camera frustum for the rest of the frame.
        self.cull_draws_to_camera();
        // and, replaying, the scene pass's draws past its limit (#1089)
        self.cut_draws_to_limit();

        // All scene pipelines target Rgba16Float, so we always render to the HDR
        // buffer and composite to sRGB.
        let has_postprocess =
            self.postprocess_pipeline.is_some() && self.postprocess_resources.is_some();

        // Shader-side tonemapping is always OFF when compositing through the HDR
        // buffer (the composite pass handles ACES + gamma).
        let tonemapping_u32: u32 = if !has_postprocess && self.tonemapping_enabled {
            1
        } else {
            0
        };

        // Grab-pass refraction runs only with an active ocean + postprocess
        // (the HDR buffer is what gets snapshotted) in normal render mode.
        // Water bodies refract through the same grab (ADR 0094).
        self.water.grab_this_frame = self.water_active()
            && has_postprocess
            && !wireframe_only
            && self.pipelines.ocean.is_some();
        self.water.camera_near_far = (camera.near, camera.far);
        if self.water.grab_this_frame {
            let (w, h) = {
                let res = self.postprocess_resources.as_ref().unwrap();
                (res.width, res.height)
            };
            self.water
                .ensure_grab(device, self.pipelines.ocean.as_deref(), w, h);
            if self.water.grab.bind_group.is_none() {
                self.water.grab_this_frame = false;
            }
        }

        // The fog veil (#513): while the fog reaches a translucent draw, the
        // main pass ends before the first one, the picture behind them is
        // copied, and the veil is drawn after, so the composite can fog them
        // at their own depth.
        let fog_veiled = has_postprocess && !wireframe_only && self.fog_meets_translucents();
        self.fog_veil.drawn = false;
        if fog_veiled {
            let res = self.postprocess_resources.as_mut().unwrap();
            if res.fog_veil.is_none() {
                res.fog_veil = Some(crate::postprocess::FogVeilTargets::new(
                    device, res.width, res.height,
                ));
            }
        }

        // Update all per-frame uniforms and sort transparent draws
        self.update_per_frame_uniforms(
            queue,
            view_proj,
            camera_pos,
            debug_mode_u32,
            tonemapping_u32,
            camera,
        );

        // MSAA (ADR 0058): scene passes draw into MSAA color/depth and
        // resolve color into the HDR buffer; depth resolves (sample 0) into
        // the caller's depth view after each opaque phase so every existing
        // depth consumer stays single-sample. Requires the HDR chain (`build`
        // always makes it).
        let msaa_active = self.pipelines.samples() > 1 && has_postprocess;
        if msaa_active {
            let (w, h) = {
                let res = self.postprocess_resources.as_ref().unwrap();
                (res.width, res.height)
            };
            self.msaa.prepare(device, self.pipelines.samples(), w, h);
        }

        // Choose render target: HDR buffer or direct to surface
        let scene_target_view = if has_postprocess {
            &self.postprocess_resources.as_ref().unwrap().hdr_view
        } else {
            target_view
        };

        // Scene-pass attachments: MSAA views with a color resolve when
        // active, the plain single-sample pair otherwise.
        let (pass_color_view, pass_resolve_target, pass_depth_view) = if msaa_active {
            (
                self.msaa.color_view().unwrap(),
                Some(scene_target_view),
                self.msaa.depth_view().unwrap(),
            )
        } else {
            (scene_target_view, None, depth_view)
        };

        // Every scene pass clears what it draws into, so a repeat is the
        // same work again (#1317)
        let scene_times = scopes.times("Scene");
        let scene_scope = scopes.begin_times(encoder, "Scene", scene_times);
        scopes.note("Scene", |p| {
            p.reads(&["shadow_map"]);
            if msaa_active {
                p.writes(&["msaa_color", "msaa_depth"]);
            }
            p.writes(&[
                if has_postprocess {
                    "hdr_scene"
                } else {
                    "surface"
                },
                sd,
            ]);
        });

        scopes.repeat(scene_times, |scopes| {
            // A replay's draw times count the list from its start (#1203)
            self.draw_times.begin_pass();
            if !self.water.grab_this_frame {
                // Single-pass path (unchanged for scenes without an ocean), split
                // before the translucent draws while the fog veil is drawn.
                scopes.note("Scene", |p| p.render_pass("Render Pass"));
                let mut render_pass = scene_pass(
                    encoder,
                    "Render Pass",
                    pass_color_view,
                    pass_resolve_target,
                    pass_depth_view,
                    Some(clear_color),
                );
                self.render_main_pass(
                    &mut render_pass,
                    wireframe_only,
                    queue,
                    camera,
                    if fog_veiled {
                        RenderPhase::OPAQUE_AND_OCEAN
                    } else {
                        RenderPhase::ALL
                    },
                );
                drop(render_pass);
                if fog_veiled {
                    self.copy_fog_veil_behind(encoder);
                    scopes.note("Scene", |p| {
                        p.writes(&["fog_veil_behind"]);
                        p.render_pass("Render Pass (translucent)");
                    });
                    let mut render_pass = scene_pass(
                        encoder,
                        "Render Pass (translucent)",
                        pass_color_view,
                        pass_resolve_target,
                        pass_depth_view,
                        None,
                    );
                    self.render_main_pass(
                        &mut render_pass,
                        wireframe_only,
                        queue,
                        camera,
                        RenderPhase::REST,
                    );
                }
                if msaa_active {
                    self.msaa.resolve_depth_into(device, encoder, depth_view);
                }
            } else {
                // ── Grab-pass split: opaque scene → snapshot → ocean + rest ──
                // Pass A: sky + all opaque geometry (including the legs).
                scopes.note("Scene", |p| {
                    p.writes(&["ocean_grab_color", "ocean_grab_depth"]);
                    p.render_pass("Render Pass A (pre-ocean)");
                    p.render_pass("Ocean Grab Blit");
                    p.render_pass("Render Pass B (ocean + transparents)");
                });
                {
                    let mut pass_a = scene_pass(
                        encoder,
                        "Render Pass A (pre-ocean)",
                        pass_color_view,
                        pass_resolve_target,
                        pass_depth_view,
                        Some(clear_color),
                    );
                    self.render_main_pass(
                        &mut pass_a,
                        wireframe_only,
                        queue,
                        camera,
                        RenderPhase::PRE_OCEAN,
                    );
                }

                // With MSAA, the grab blit below reads the caller's single-sample
                // depth view — resolve the opaque depth into it first (ADR 0058).
                if msaa_active {
                    self.msaa.resolve_depth_into(device, encoder, depth_view);
                }

                // Blit: snapshot opaque color + depth into sampleable copies.
                {
                    let op = self.pipelines.ocean.as_deref().unwrap();
                    // The grab runs only into the HDR chain, so the bind group
                    // is the chain's to keep while the depth is its too (#139).
                    let resources = self.postprocess_resources.as_ref().unwrap();
                    let blit_bind_group = resources.binds.get_over(
                        depth_key,
                        "ocean grab blit",
                        0,
                        &[crate::bind_cache::Bound::View(scene_target_view)],
                        || {
                            device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                                layout: &op.blit_bind_group_layout,
                                entries: &[
                                    wgpu::BindGroupEntry {
                                        binding: 0,
                                        resource: wgpu::BindingResource::TextureView(
                                            scene_target_view,
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 1,
                                        resource: wgpu::BindingResource::TextureView(depth_view),
                                    },
                                ],
                                label: Some("Grab Blit Bind Group"),
                            })
                        },
                    );
                    let grab_color_view = &self.water.grab.color.as_ref().unwrap().1;
                    let grab_depth_view = &self.water.grab.depth.as_ref().unwrap().1;
                    let mut blit_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("Ocean Grab Blit"),
                        color_attachments: &[
                            Some(wgpu::RenderPassColorAttachment {
                                view: grab_color_view,
                                resolve_target: None,
                                depth_slice: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                    store: wgpu::StoreOp::Store,
                                },
                            }),
                            Some(wgpu::RenderPassColorAttachment {
                                view: grab_depth_view,
                                resolve_target: None,
                                depth_slice: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                                    store: wgpu::StoreOp::Store,
                                },
                            }),
                        ],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    blit_pass.set_pipeline(&op.blit_pipeline);
                    blit_pass.set_bind_group(0, &*blit_bind_group, &[]);
                    blit_pass.draw(0..3, 0..1);
                }

                // Pass B: ocean (sampling the snapshots) + transparents/particles,
                // split after the ocean while the fog veil is drawn.
                {
                    let mut pass_b = scene_pass(
                        encoder,
                        "Render Pass B (ocean + transparents)",
                        pass_color_view,
                        pass_resolve_target,
                        pass_depth_view,
                        None,
                    );
                    self.render_main_pass(
                        &mut pass_b,
                        wireframe_only,
                        queue,
                        camera,
                        if fog_veiled {
                            RenderPhase::OCEAN
                        } else {
                            RenderPhase::POST_OCEAN
                        },
                    );
                }
                if fog_veiled {
                    self.copy_fog_veil_behind(encoder);
                    scopes.note("Scene", |p| {
                        p.writes(&["fog_veil_behind"]);
                        p.render_pass("Render Pass C (transparents)");
                    });
                    let mut pass_c = scene_pass(
                        encoder,
                        "Render Pass C (transparents)",
                        pass_color_view,
                        pass_resolve_target,
                        pass_depth_view,
                        None,
                    );
                    self.render_main_pass(
                        &mut pass_c,
                        wireframe_only,
                        queue,
                        camera,
                        RenderPhase::REST,
                    );
                }

                // Final depth resolve so postprocess (SSAO/DoF/fog/volumetric)
                // sees the complete frame's depth (ADR 0058).
                if msaa_active {
                    self.msaa.resolve_depth_into(device, encoder, depth_view);
                }
            }
        });

        if let Some(scene_scope) = scene_scope {
            scopes.end(encoder, scene_scope);
            if let Some(pass) = scopes.log().and_then(|l| l.named("Scene")) {
                self.log_scene_draws(pass, camera_pos);
            }
            self.tap_pass(device, &mut scopes, encoder, "Scene");
        }
        // A replay's draw times (#1203) toward their read-back, and its
        // overdraw view of the pass just drawn
        self.draw_times.resolve(encoder);
        if self.overdraw.wanted {
            let scope = scopes.begin(encoder, "Overdraw");
            scopes.note("Overdraw", |p| {
                p.writes(&["overdraw_count", "overdraw"]);
            });
            self.encode_overdraw(device, encoder);
            scopes.end(encoder, scope);
            self.tap_pass(device, &mut scopes, encoder, "Overdraw");
        }

        // The fog veil (#513): the translucent draws' weight, depth and
        // coverage, against the finished frame's single-sample depth
        if fog_veiled {
            let n = scopes.times("Fog veil");
            if let Some(scope) = scopes.begin_times(encoder, "Fog veil", n) {
                for _ in 0..n {
                    self.fog_veil.drawn = self.encode_fog_veil(device, encoder, depth_view);
                }
                scopes.end(encoder, scope);
                if let Some(pass) = scopes.log().and_then(|l| l.named("Fog veil")) {
                    self.log_fog_veil_draws(pass);
                }
                scopes.note("Fog veil", |p| {
                    p.reads(&[sd]).writes(&["fog_veil"]);
                });
                self.tap_pass(device, &mut scopes, encoder, "Fog veil");
            } else {
                // Skipped (#1317): the composite still reads the veil, as
                // the veil last left it, so its own cost stays the same
                self.fog_veil.drawn = true;
            }
        } else {
            scopes.skip("Fog veil", || {
                if !has_postprocess {
                    "no HDR chain"
                } else if wireframe_only {
                    "the wireframe view"
                } else {
                    "no fog reaches a translucent draw"
                }
                .to_string()
            });
        }

        // The entity style mask (#263): the styled entities' ids, drawn
        // against the frame's single-sample depth, while a style reads it.
        if has_postprocess && self.style_mask_wanted() {
            let n = scopes.times("Style mask");
            if let Some(scope) = scopes.begin_times(encoder, "Style mask", n) {
                for _ in 0..n {
                    self.encode_style_mask(device, queue, encoder, depth_view, depth_key);
                }
                scopes.end(encoder, scope);
                if let Some(pass) = scopes.log().and_then(|l| l.named("Style mask")) {
                    self.log_style_mask_draws(pass);
                }
                scopes.note("Style mask", |p| {
                    p.reads(&[sd])
                        .writes(&["style veil", crate::style_mask::MASK_TARGET]);
                });
                self.tap_pass(device, &mut scopes, encoder, "Style mask");
            }
        } else {
            scopes.skip("Style mask", || {
                "no style reads the entity mask".to_string()
            });
        }

        // The style albedo (#869): the opaque meshes' own colour, while a
        // style divides the picture by it
        if has_postprocess
            && self
                .styles
                .any_uses_albedo(&self.postprocess_config, &self.style_mask.ids)
        {
            let n = scopes.times("Style albedo");
            if let Some(scope) = scopes.begin_times(encoder, "Style albedo", n) {
                for _ in 0..n {
                    self.encode_style_albedo(device, encoder, depth_view);
                }
                scopes.end(encoder, scope);
                if let Some(pass) = scopes.log().and_then(|l| l.named("Style albedo")) {
                    self.log_style_albedo_draws(pass);
                }
                scopes.note("Style albedo", |p| {
                    p.reads(&[sd]).writes(&[crate::style_albedo::ALBEDO_TARGET]);
                });
                self.tap_pass(device, &mut scopes, encoder, "Style albedo");
            }
        } else {
            scopes.skip("Style albedo", || "no style reads the albedo".to_string());
        }

        // The editor's selection (#602): its draws into a mask, against the
        // same depth. Nothing selected, nothing drawn.
        let selection = self.selection_set();
        let outlining = !selection.is_empty();
        if outlining {
            let n = scopes.times("Selection mask");
            if let Some(scope) = scopes.begin_times(encoder, "Selection mask", n) {
                for _ in 0..n {
                    self.encode_selection_mask(device, encoder, depth_view, &selection);
                }
                scopes.end(encoder, scope);
                if let Some(pass) = scopes.log().and_then(|l| l.named("Selection mask")) {
                    self.log_selection_draws(pass, &selection);
                }
                scopes.note("Selection mask", |p| {
                    p.reads(&[sd]).writes(&["selection_mask"]);
                });
                self.tap_pass(device, &mut scopes, encoder, "Selection mask");
            }
        } else {
            scopes.skip("Selection mask", || "nothing selected".to_string());
        }

        // Composite: always needed to convert HDR → sRGB surface
        if has_postprocess {
            self.render_postprocess(
                device,
                queue,
                encoder,
                depth_view,
                depth_key,
                target_view,
                camera,
                &mut scopes,
            );
        }

        // The scaled scene's depth, into the surface-sized view the overlays
        // below and the caller's own (world text, gizmos) test against.
        if let (Some(_), Some(upscale)) = (&scaled_depth, &self.depth_upscale) {
            upscale.record(encoder, surface_depth_view);
            if let Some(log) = scopes.log() {
                log.ran("Depth upscale", false)
                    .reads(&["scene_depth"])
                    .writes(&["depth"]);
            }
            self.tap_pass(device, &mut scopes, encoder, "Depth upscale");
        }

        // The selection outline, over the finished image: after every
        // style and the tonemap, so its colour is the one asked for (#602)
        if outlining {
            let n = scopes.times_refusing(
                "Selection outline",
                Some(crate::pass_knobs::DRAWS_OVER_THE_FRAME),
            );
            if let Some(scope) = scopes.begin_times(encoder, "Selection outline", n) {
                self.encode_selection_outline(queue, encoder, target_view);
                scopes.end(encoder, scope);
                scopes.note("Selection outline", |p| {
                    p.reads(&["selection_mask"]).writes(&["surface"]);
                });
                self.tap_pass(device, &mut scopes, encoder, "Selection outline");
            }
        } else {
            scopes.skip("Selection outline", || "nothing selected".to_string());
        }

        // World lines (ADR 0083) go over the finished image, tested against
        // the frame's single-sample depth. Without the HDR chain an MSAA depth
        // is never resolved, so they're skipped there.
        let lines_drawable = has_postprocess || self.pipelines.samples() == 1;
        if !self.world_lines.is_empty() && lines_drawable {
            let n =
                scopes.times_refusing("World lines", Some(crate::pass_knobs::DRAWS_OVER_THE_FRAME));
            if let Some(scope) = scopes.begin_times(encoder, "World lines", n) {
                self.render_world_lines(
                    device,
                    queue,
                    encoder,
                    surface_depth_view,
                    target_view,
                    camera,
                );
                scopes.end(encoder, scope);
                scopes.note("World lines", |p| {
                    p.reads(&["depth"]).writes(&["surface"]);
                });
                self.tap_pass(device, &mut scopes, encoder, "World lines");
            }
        } else {
            let none = self.world_lines.is_empty();
            scopes.skip("World lines", || {
                if none {
                    "no world lines this frame"
                } else {
                    "MSAA without the HDR chain"
                }
                .to_string()
            });
        }
        if let Some(knobs) = scopes.knobs.as_deref_mut() {
            knobs.end_frame();
        }
        self.gpu_timer = scopes.timer;
        self.frame_log = scopes.log;
        self.frame_taps = scopes.taps;
        self.pass_knobs = scopes.knobs;
    }

    /// [`encode_to`](Self::encode_to) a second time in a frame, for the other
    /// half of a split view (#238), with the GPU timer left out: its scopes are
    /// the first drawing's. Submit the first drawing before calling this, since
    /// what changed between the two (bones, the boil) reaches the GPU through
    /// the queue's writes, and those land with the next submission.
    pub fn encode_again_to(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        depth_view: &wgpu::TextureView,
        camera: &Camera,
        target_view: &wgpu::TextureView,
    ) {
        let timer = self.gpu_timer.take();
        // A described or captured frame is the shown side's (#1086): the
        // other side is neither logged nor read, only noted
        let held = self.hold_frame_capture();
        self.encode_to(device, queue, encoder, depth_view, camera, target_view);
        self.resume_frame_capture(held);
        if let Some(log) = self.frame_log.as_deref_mut() {
            let note = "a split view's other side (encode_again_to, the Living Props A/B \
                        split #238) was drawn over part of the frame: the passes and images \
                        are the shown side's"
                .to_string();
            if !log.notes.contains(&note) {
                log.notes.push(note);
            }
        }
        self.gpu_timer = timer;
        // The other side's boil is drawn once (#739)
        self.again_boil = None;
    }

    /// Start what waits on the frame [`encode_to`](Self::encode_to)
    /// recorded: the grass count's read-back and, once
    /// [`encode_gpu_timings`](Self::encode_gpu_timings) resolved them, the
    /// GPU timer's. Call once that frame is submitted.
    ///
    /// It also closes the frame's GPU count (#1091): what the process's
    /// renderers created and wrote since the last call
    /// ([`frame_gpu_calls`](Self::frame_gpu_calls)), and a
    /// `gpu objects` frame note when it created any.
    pub fn frame_submitted(&mut self) {
        flint_core::scope!("frame_submitted");
        let now = crate::gpu_counts::totals();
        self.frame_gpu_calls = now.since(&self.gpu_calls_at);
        self.gpu_calls_at = now;
        let calls = self.frame_gpu_calls;
        self.gpu_frames.0 += 1;
        if calls.created() > 0 {
            self.gpu_frames.1 += 1;
            use flint_core::frame_notes::{note, NoteKind};
            note(NoteKind::GpuObjects, || calls.created_line(), None);
        }
        if std::mem::take(&mut self.grass.readback_due) {
            if let Some(readback) = &mut self.grass.count_readback {
                readback.start_map();
            }
        }
        if let Some(timer) = &mut self.gpu_timer {
            timer.start_readback();
        }
    }

    fn render_world_lines(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        depth_view: &wgpu::TextureView,
        target_view: &wgpu::TextureView,
        camera: &Camera,
    ) {
        if self.postprocess_resources.is_none() {
            return;
        }
        flint_core::scope!("render_world_lines", self.world_lines.len());
        // Drawn into the surface, whatever the scene's render scale
        let viewport = [self.surface_size.0 as f32, self.surface_size.1 as f32];
        let format = self.surface_format;
        let pipeline = self.world_line_pipeline.get_or_insert_with(|| {
            crate::world_line_pipeline::WorldLinePipeline::new(device, format)
        });
        let view = crate::world_line_pipeline::LineView {
            view_proj: camera.view_projection_matrix(),
            eye: camera.position_array(),
            viewport,
            px_per_point: self.world_line_px_per_point,
            linear_output: format.is_srgb(),
        };
        pipeline.draw(
            device,
            queue,
            encoder,
            target_view,
            depth_view,
            &self.world_lines,
            &view,
        );
    }

    /// Copy the HDR picture as the scene passes left it into the fog veil's
    /// picture behind (#513): everything drawn so far, which is everything
    /// behind the translucent draws still to come.
    fn copy_fog_veil_behind(&self, encoder: &mut wgpu::CommandEncoder) {
        let Some(res) = &self.postprocess_resources else {
            return;
        };
        if let Some(targets) = &res.fog_veil {
            encoder.copy_texture_to_texture(
                res.hdr_texture.as_image_copy(),
                targets.behind.as_image_copy(),
                targets.behind.size(),
            );
        }
    }
}

/// Begin one of the frame's scene passes: cleared to `clear` (and depth to
/// 1) for the first, loading what's there for the ones after a split.
fn scene_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &str,
    color: &wgpu::TextureView,
    resolve_target: Option<&wgpu::TextureView>,
    depth: &wgpu::TextureView,
    clear: Option<wgpu::Color>,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: color,
            resolve_target,
            depth_slice: None,
            ops: wgpu::Operations {
                load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: depth,
            depth_ops: Some(wgpu::Operations {
                load: if clear.is_some() {
                    wgpu::LoadOp::Clear(1.0)
                } else {
                    wgpu::LoadOp::Load
                },
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}
