//! Living props and the wooze field as one of the player's subsystems (ADR
//! 0117, #724 step 3c): the props' clock and the field it reads, the shading
//! boil's beats, shallow wooze and woozy grass, the hand tuning and the A/B
//! comparison, in one place a scene without them never runs.
//!
//! The props themselves stay in the procgen resolver, which makes their poses
//! on its frame budget from its generators and specs; every hook is lent it
//! (`StageCtx::resolver`). The tuning and A/B code is `impl LivingSubsystem`
//! in `living_tuning.rs` and `living_ab.rs`.

use super::living_props;
use super::subsystem::{
    EncodeCtx, ExitReport, FrameCtx, PlayerInput, ReportCtx, SceneCtx, Stage, StageCtx, Subsystem,
};
use flint_render::FrameInputs;
use flint_script::context::ScriptCommand;
use flint_wooze::shallow::{self as shallow_wooze, GrassWooze, ShallowWooze};

/// Living props' and the wooze field's state for one scene.
#[derive(Default)]
pub(crate) struct LivingSubsystem {
    /// Room to blend the props each frame
    pub(crate) blend: flint_living::BlendFrame,
    /// The field made from it, sampled on a clock of its own that stops
    /// when a game state pauses `living`. No table: no field, no samples.
    pub(crate) wooze: Option<flint_wooze::WoozeField>,
    pub(crate) wooze_time: f64,
    pub(crate) wooze_missing_warned: bool,
    /// The shading boil's beats (#214), on the same clock as the field.
    pub(crate) boil_clock: flint_wooze::BoilClock,
    /// Shallow wooze (#215): the clocks of the meshes on the field with no poses.
    pub(crate) shallow_wooze: ShallowWooze,
    /// Woozy grass (#252): the grass's sway and comb on the same clock.
    pub(crate) grass_wooze: GrassWooze,
    /// What's been done to a running grove by hand (#218).
    pub(crate) tune: super::living_tuning::LivingTune,
    /// The knobs a script's `tune_living(map, seconds)` is easing (#514).
    pub(crate) ramps: super::living_ramps::KnobRamps,
    /// `Some` in a capture (`flint render`, #216, #725): see [`Capture`].
    pub(crate) capture: Option<Capture>,
    /// The field as the `wooze` module's reads last had it (#505), shared
    /// until it changes, so a frame that changes nothing copies nothing.
    published: Option<std::sync::Arc<flint_wooze::WoozeField>>,
    /// This run of the subsystem, so scripts' beat clocks start again in
    /// each scene.
    scene_token: u64,
}

/// A new [`LivingSubsystem::scene_token`].
fn next_scene_token() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// How a capture's grove differs from the player's: every pose a step asks
/// for is made then, off the frame budget, since a pose arriving late would
/// change the picture; the props are blended once, at the capture, not each
/// step; and shallow wooze draws only in a capture of a moment (a plain
/// render shows those meshes at rest, #215).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Capture {
    pub shallow: bool,
}

impl LivingSubsystem {
    /// Its name, and the key a game state pauses its clock by (#727).
    pub(crate) const NAME: &'static str = "living";

    /// A left click on a living prop while the Living Props panel is open picks
    /// it (#248), and the click, press and release, stops there: the game never
    /// hears it. One that lands on no prop goes on as it would have.
    pub(crate) fn living_takes_click(
        &mut self,
        ctx: &mut StageCtx,
        state: winit::event::ElementState,
        button: winit::event::MouseButton,
    ) -> bool {
        #[cfg(feature = "debug-hud")]
        {
            if button != winit::event::MouseButton::Left {
                return false;
            }
            match state {
                winit::event::ElementState::Pressed => {
                    if !self.living_wants_clicks(ctx) {
                        return false;
                    }
                    let ppp = ctx.pixels_per_point;
                    let (x, y) = ctx.input.mouse_position;
                    let took = self.pick_living_at(ctx, egui::pos2(x as f32 / ppp, y as f32 / ppp));
                    self.tune.click_held = took;
                    took
                }
                winit::event::ElementState::Released => std::mem::take(&mut self.tune.click_held),
            }
        }
        #[cfg(not(feature = "debug-hud"))]
        {
            let _ = (ctx, state, button);
            false
        }
    }

    /// Let go of the tree picked in the world, if one is. Returns whether one was.
    pub(crate) fn living_let_go(&mut self, ctx: &mut StageCtx) -> bool {
        #[cfg(feature = "debug-hud")]
        if self.picked_living(ctx).is_some() {
            self.pick_living(ctx, None);
            return true;
        }
        #[cfg(not(feature = "debug-hud"))]
        let _ = ctx;
        false
    }

    /// The subsystem for the scene being entered, if it has living props, a
    /// `[living]` or `[wooze]` table, an entity with `shallow_wooze`, or grass
    /// that asks to wooze. A scene that asks for shallow wooze or woozy grass
    /// with no field to drive it is told so, once, here.
    pub(crate) fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
        if let Some(capture) = ctx.capture.as_deref_mut() {
            return Self::in_capture(ctx.living, ctx.resolver, ctx.script, capture);
        }
        let wooze = living_props::field_of(ctx.scene.wooze.as_ref());
        let grass = ctx.renderer.as_deref().and_then(|r| r.grass_config());
        shallow_wooze::warn_without_a_field(ctx.world, wooze.as_ref());
        shallow_wooze::warn_grass_without_a_field(
            grass.and_then(|c| c.wooze_comb()).is_some(),
            wooze.as_ref(),
        );
        let woozy_grass = grass.is_some_and(|c| c.enabled && c.wooze);
        let shallow = !ctx
            .world
            .entities_with_component(flint_wooze::components::SHALLOW_WOOZE)
            .is_empty();
        let used = !ctx.living.is_empty()
            || ctx.scene.living.is_some()
            || wooze.is_some()
            || shallow
            || woozy_grass;
        if !used {
            return None;
        }
        let mut living = Self {
            wooze,
            scene_token: next_scene_token(),
            ..Default::default()
        };
        // What --compare asked for, before any scene was: which presets, and
        // whether to start comparing; the picks outlive the scene
        living.tune.ab_picks = ctx.launch.compare.clone();
        living.tune.compare_at_start = ctx.launch.compare_at_start.take();
        // The scripts' on_init reads the field at the scene's first moment
        living.publish_reading(ctx.script, 0.0);
        Some(Box::new(living))
    }

    /// Hand the `wooze` module's reads (#505) the field and the time on its
    /// clock `time`, replacing what an earlier scene or frame handed them.
    pub(crate) fn publish_reading(&mut self, script: &mut flint_script::ScriptSystem, time: f64) {
        let field = match (&self.wooze, &self.published) {
            (Some(field), Some(published)) if **published == *field => Some(published.clone()),
            (Some(field), _) => Some(std::sync::Arc::new(field.clone())),
            (None, _) => None,
        };
        self.published = field.clone();
        script.set_module_state(super::living_api::WoozeReading {
            field,
            time,
            scene: self.scene_token,
        });
    }

    /// One frame of living props, on `dt` of the living clock: the clock and
    /// the field, the boil, the poses, shallow wooze and grass on the same
    /// field and `dt`, then new lineages and the blends.
    /// The subsystem a capture's grove runs on (`flint render`, #1252), if
    /// the scene has living props or the capture a wooze field (the scene's
    /// `[wooze]` with any sweep's knobs over it, which it takes). As the
    /// player's first frame, the props read the field without moving: every
    /// pose is made at once, off the frame budget. A capture of a moment
    /// draws shallow wooze.
    fn in_capture(
        props: &mut flint_living::LivingProps,
        resolver: &flint_procgen::ProcGenResolver,
        script: &mut flint_script::ScriptSystem,
        capture: &mut super::headless_game::CaptureStart,
    ) -> Option<Box<dyn Subsystem>> {
        let wooze = capture.wooze.take();
        if props.is_empty() && wooze.is_none() {
            return None;
        }
        props.step_now(resolver, 0.0, wooze.as_ref().map(|field| (field, 0.0)));
        let mut living = Self {
            wooze,
            capture: Some(Capture {
                shallow: capture.moment,
            }),
            scene_token: next_scene_token(),
            ..Default::default()
        };
        // For on_init's reads of the field (#505), as the player's scene start
        living.publish_reading(script, 0.0);
        Some(Box::new(living))
    }

    pub(super) fn tick(&mut self, ctx: &mut StageCtx, dt: f64) {
        let living_dt = dt + self.before_living_tick(ctx);
        // Knobs a script is easing (#514), on the same clock, before the
        // props read them
        self.step_knob_ramps(ctx, living_dt);
        // A scene with a wooze field reads it where each prop now stands.
        if self.wooze.is_some() {
            self.wooze_time += living_dt;
            let world = &*ctx.world;
            ctx.living.track_positions(|entity| {
                world.get_world_position(entity).map(|p| [p.x, p.y, p.z])
            });
        }
        // So does the shading boil, where the viewer stands (#214).
        if let Some(field) = &self.wooze {
            let sample = field.sample(ctx.camera.position_array(), self.wooze_time);
            self.boil_clock.tick(living_dt, &sample);
        }
        let wooze = self.wooze.as_ref().map(|field| (field, self.wooze_time));
        let lineages = match self.capture {
            Some(_) => ctx.living.step_now(ctx.resolver, living_dt, wooze),
            None => ctx.living.process(ctx.resolver, living_dt, wooze),
        };
        let shallow = self.capture.is_none_or(|c| c.shallow);
        // Hand-made meshes read the same field on the same dt (#215); the
        // renderer takes their draws with the frame's inputs
        if let (Some(field), Some(renderer)) = (&self.wooze, ctx.renderer.as_deref_mut()) {
            if shallow {
                self.shallow_wooze
                    .advance(ctx.world, field, self.wooze_time, living_dt);
            }
            // And so does grass that asks to (#252), where the viewer stands.
            self.grass_wooze.advance(
                renderer.grass_config().and_then(|c| c.wooze_comb()),
                field,
                ctx.camera.position_array(),
                self.wooze_time,
                living_dt,
            );
        }
        if !lineages.is_empty() {
            if let (Some(renderer), Some((device, queue))) = (ctx.renderer.as_deref_mut(), ctx.gpu)
            {
                let config = flint_render::model_loader::ModelLoadConfig::in_project(
                    ctx.project.clone(),
                    ctx.scene_path,
                );
                let mut sink = super::procgen_assets::GpuSink {
                    renderer,
                    device,
                    queue,
                    config: &config,
                };
                living_props::apply_lineages(&lineages, &mut sink);
            }
        }
        // Blend after the rings have slid and any new lineage is up (a
        // capture blends once, when it draws).
        if self.capture.is_some() {
            return;
        }
        if let (Some(renderer), Some((device, queue))) = (ctx.renderer.as_deref_mut(), ctx.gpu) {
            living_props::upload_blends(ctx.living, &mut self.blend, renderer, device, queue);
        }
        // The other side of an A/B (#238), on the same dt, after the scene's own.
        self.tick_hidden_side(ctx, living_dt);
    }

    /// This frame's step of the living clock: the frame's, unless the game
    /// state pauses `living` (`paused` and `loading` do) or the grove is
    /// frozen (F5).
    fn living_dt(&self, ctx: &StageCtx) -> f64 {
        if ctx.runs(self.name()) && !self.tune.frozen {
            ctx.clock.delta_time
        } else {
            0.0
        }
    }

    /// The `--exit-after` lines: whether the rings stayed full and within
    /// budget. Silent with no props.
    fn report(&self, ctx: &ReportCtx, out: &mut ExitReport) {
        let props = &*ctx.living;
        if props.is_empty() {
            return;
        }
        let ring = props
            .props()
            .iter()
            .map(|p| p.ring_len())
            .max()
            .unwrap_or(0);
        for line in living_props::report(
            &props.stats(),
            props.props().len(),
            props.lineage_count(),
            ring,
            ctx.resolver.frame_budget_ms(),
        ) {
            out.line(line);
        }
        if let Some(line) = living_props::knobs_report(&props.stats(), props) {
            out.line(line);
        }
        // The other side of an A/B (#238): its rings, and its own blending.
        if let (Some(ab), Some(stats)) = (self.living_ab_state(), self.hidden_side_stats()) {
            let hidden = if ab.showing_b { "A" } else { "B" };
            out.line(format!(
                "[living] A/B {}split: hidden side {hidden}: {} of {} frames full, {} missing a pose \
                 in use, {} poses made, {} from cache; blending {:.3} ms/frame over {} frames",
                if ab.split { "" } else { "not " },
                stats.frames_full,
                stats.frames,
                stats.frames_starved,
                stats.poses_made,
                stats.poses_from_cache,
                stats.blend_total_ms / stats.blend_frames.max(1) as f64,
                stats.blend_frames,
            ));
        }
        if let Some(field) = &self.wooze {
            out.line(living_props::wooze_report(field, props));
            out.line(format!(
                "[wooze] shallow: {} entities on {} clocks",
                self.shallow_wooze.draw_count(),
                self.shallow_wooze.clock_count()
            ));
        }
        if let Some(renderer) = ctx.renderer {
            out.line(format!(
                "[living] {} skinned draws this frame",
                renderer.collect_stats().skinned_draws
            ));
            let echoes: usize = props.props().iter().map(|p| p.echo_count()).sum();
            if echoes > 0 {
                out.line(format!(
                    "[living] {echoes} echoes, {} echo draws this frame",
                    renderer.collect_stats().echo_draws
                ));
            }
        }
    }

    /// A script's change to the wooze field. A change that's no good is named and
    /// changes nothing. A scene with no `[wooze]` table never asked for a field, so
    /// it doesn't get one; it says so once, since a script may ask every frame.
    fn change_wooze(
        &mut self,
        change: impl FnOnce(&mut flint_wooze::WoozeField, f64) -> Result<(), String>,
    ) {
        let time = self.wooze_time;
        match &mut self.wooze {
            Some(field) => {
                if let Err(reason) = change(field, time) {
                    tracing::warn!("A script's change to the wooze field: {reason}");
                }
            }
            None if !self.wooze_missing_warned => {
                self.wooze_missing_warned = true;
                tracing::warn!(
                    "A script tried to change the wooze field, and this scene has none: \
                     give it a [wooze] table, even an empty one, to opt in"
                );
            }
            None => {}
        }
    }

    /// A script's living-props or wooze command (`living_api.rs`), as the
    /// player's host applied them before (#8, #218, #238); `false` for any
    /// other.
    fn command(&mut self, command: &ScriptCommand, ctx: &mut StageCtx) -> bool {
        use super::living_api::LivingCommand as C;
        let ScriptCommand::Module(command) = command else {
            return false;
        };
        let Some(command) = command.payload::<C>() else {
            return false;
        };
        match command {
            C::SetWooze { knobs, seconds } => self.change_wooze(|field, time| {
                let table = table_of(knobs)?;
                field.set_global(table, time, *seconds)
            }),
            C::SetWoozeZone {
                name,
                fields,
                seconds,
            } => self.change_wooze(|field, time| {
                let table = table_of(fields)?;
                field
                    .set_zone(name, table, time, *seconds)
                    .map_err(|e| format!("zone '{name}': {e}"))
            }),
            C::RemoveWoozeZone { name, seconds } => self.change_wooze(|field, time| {
                if field.remove_zone(name, time, *seconds) {
                    Ok(())
                } else {
                    Err(format!("there's no zone named '{name}' to remove"))
                }
            }),
            C::Tune { knobs, seconds } => {
                if let Err(problem) = self.tune_living_over(ctx, knobs.clone(), *seconds) {
                    tracing::warn!(target: "script", "tune_living: {problem}");
                }
            }
            C::Clear { species, keys } => {
                if let Err(problem) = self.clear_living_species(ctx, species, keys) {
                    tracing::warn!(target: "script", "clear_living: {problem}");
                }
            }
            C::Freeze { frozen } => self.tune.frozen = *frozen,
            C::Step => self.step_living(),
            C::SavePreset { name } => match self.save_living_preset(ctx, name) {
                Ok(path) => tracing::info!(target: "player", "[living] saved {}", path.display()),
                Err(problem) => {
                    tracing::warn!(target: "script", "save_living_preset: {problem}");
                }
            },
            C::LoadPreset { name } => {
                if let Err(problem) = self.load_living_preset(ctx, name) {
                    tracing::warn!(target: "script", "load_living_preset: {problem}");
                }
            }
            C::Commit => match self.commit_living(ctx) {
                Ok(said) => tracing::info!(target: "player", "[living] {said}"),
                Err(problem) => tracing::warn!(target: "script", "commit_living: {problem}"),
            },
            C::Compare { a, b } => {
                if let Err(problem) = self.compare_living(ctx, a, b) {
                    tracing::warn!(target: "script", "compare_living: {problem}");
                }
            }
            C::Flip => {
                if !self.flip_living(ctx) {
                    tracing::warn!(target: "script", "flip_living: nothing is being compared");
                }
            }
            C::Split { split } => {
                if !self.split_living(ctx, *split) {
                    tracing::warn!(target: "script", "split_living: nothing is being compared");
                }
            }
            C::StopComparing => self.stop_comparing_living(ctx),
        }
        true
    }
}

impl Subsystem for LivingSubsystem {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    /// Its script modules (`living_api.rs`, #698): a script can turn the
    /// knobs and the field wherever it runs, and the A/B and the commit
    /// there in a build with the debug HUD (#854).
    fn script_modules(&self) -> &[&'static str] {
        crate::player_app::living_api::ENABLED
    }

    fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {
        // Before on_update, the field and the moment its clock reaches this
        // frame (#505): what this frame's poses and shallow wooze are drawn
        // at, so a script's motion holds on the same beat as theirs.
        if stage == Stage::PreScripts {
            let time = self.wooze_time + self.living_dt(ctx);
            self.publish_reading(ctx.script, time);
        }
        // What the panel asked this frame, then the clock it may have
        // frozen. What it draws goes to the renderer in `frame_inputs`.
        if stage == Stage::Procgen {
            #[cfg(feature = "debug-hud")]
            self.drain_living_panel(ctx);
            let dt = self.living_dt(ctx);
            self.tick(ctx, dt);
        }
    }

    fn script_command(&mut self, command: &ScriptCommand, ctx: &mut StageCtx) -> bool {
        self.command(command, ctx)
    }

    fn debug_keys(&self) -> &'static [flint_runtime::DebugKey] {
        super::subsystem::living::DEBUG_KEYS.keys
    }

    fn on_debug_key(&mut self, action: &str, key: &str, ctx: &mut StageCtx) {
        // Living Props A/B (#238): freeze the living clock, flip between the
        // two presets, split the view between them. Flip and split start a
        // comparison of the panel's picks if none is running.
        match action {
            "living_freeze" => self.toggle_living_freeze(ctx),
            "living_flip" => self.living_ab_key(ctx, false),
            "living_split" => self.living_ab_key(ctx, true),
            // Pick a living prop (#248): the one at the view centre, as with
            // the cursor captured, or the next by name, for a click that
            // missed.
            #[cfg(feature = "debug-hud")]
            "living_pick" => {
                if !self.pick_living_at_centre(ctx) {
                    ctx.note.say(key, "no living prop at the view centre");
                }
            }
            #[cfg(feature = "debug-hud")]
            "living_pick_next" => {
                self.pick_next_living(ctx, true);
            }
            _ => {}
        }
        let _ = key;
    }

    fn on_input(&mut self, input: PlayerInput, ctx: &mut StageCtx) -> bool {
        match input {
            PlayerInput::Click { state, button } => self.living_takes_click(ctx, state, button),
            PlayerInput::Escape => self.living_let_go(ctx),
        }
    }

    fn frame_inputs<'s>(&'s self, ctx: &FrameCtx, inputs: &mut FrameInputs<'s>) {
        if let Some(field) = &self.wooze {
            // The shading boil's drawing (#214): the field where the viewer
            // stands. With no field, none: neither pass boils.
            let sample = field.sample(ctx.camera.position_array(), self.wooze_time);
            inputs.boil = self.boil_clock.frame(&sample, ctx.boil_on);
            // Hand-made meshes' drift (#215), outside a capture of no moment
            if self.capture.is_none_or(|c| c.shallow) {
                inputs.shallow_wooze = self.shallow_wooze.draws();
            }
        }
        // The grass's clock: the field's, or the frame's when it isn't woozy
        inputs.grass_time = self.grass_wooze.sway_time();
        inputs.grass_wooze = self.grass_wooze.comb();
    }

    fn post_encode(&mut self, ctx: &mut EncodeCtx) {
        // A split A/B (#238): the drawing so far is the side being shown, so
        // it goes to the GPU now, and the other side draws over its half.
        let Some(ab) = self.tune.ab.as_deref_mut().filter(|ab| ab.is_split()) else {
            return;
        };
        let first = std::mem::replace(
            ctx.encoder,
            ctx.device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Split Frame Encoder"),
                }),
        );
        ctx.queue.submit(std::iter::once(first.finish()));
        let frame_copy = ctx.output.create_view(&wgpu::TextureViewDescriptor {
            format: Some(ctx.formats.1),
            ..Default::default()
        });
        super::living_ab::draw_other_side(
            super::living_ab::OtherSide {
                ab,
                living: ctx.living,
                blend: &mut self.blend,
                world: ctx.world,
                camera: ctx.camera,
            },
            ctx.renderer,
            ctx.device,
            ctx.queue,
            ctx.encoder,
            ctx.depth_view,
            &frame_copy,
            (ctx.output.width(), ctx.output.height()),
            ctx.formats,
        );
    }

    fn ui(&mut self, egui: &egui::Context) {
        // Which preset is where, while two are compared (#238).
        if let Some(ab) = self.living_ab_state() {
            super::living_ab::paint_labels(egui, &ab);
        }
    }

    fn scene_unloading(&mut self, ctx: &mut SceneCtx) {
        // The panel's picks outlive the scene, as --compare's do
        if let Some(picks) = &self.tune.ab_picks {
            ctx.launch.compare = Some(picks.clone());
        }
        // ...and a comparison asked for at start that no scene had props for yet
        if let Some(split) = self.tune.compare_at_start.take() {
            ctx.launch.compare_at_start = Some(split);
        }
        ctx.living.clear_props();
    }

    fn exit_report(&self, ctx: &ReportCtx, report: &mut ExitReport) {
        self.report(ctx, report);
    }
}

impl super::PlayerApp {
    /// Start comparing living props preset `a` with `b` once a scene's props
    /// are built, split if `split`: `--compare a,b` and `--split` (#238).
    pub fn compare_living_at_start(&mut self, a: &str, b: &str, split: bool) {
        self.launch.compare = Some((a.to_string(), b.to_string()));
        self.launch.compare_at_start = Some(split);
    }
}

fn table_of(value: &toml::Value) -> Result<&toml::Table, String> {
    value
        .as_table()
        .ok_or_else(|| format!("{value} isn't a map of knobs"))
}

/// What `flint render` reads of a capture's grove: the field's clock and
/// the blends the stage writes before each capture. Here, beside the
/// subsystem, so `headless_game.rs` names none (#1252).
impl super::headless_game::HeadlessGame {
    fn living(&self) -> Option<&LivingSubsystem> {
        self.core.subsystems.find::<LivingSubsystem>()
    }

    /// The wooze field and the time its clock has reached, with one.
    pub fn wooze(&self) -> Option<(&flint_wooze::WoozeField, f64)> {
        let living = self.living()?;
        Some((living.wooze.as_ref()?, living.wooze_time))
    }

    /// Blend every prop where its clock stands and write what moved, as a
    /// frame of the player does.
    pub fn upload_living(
        &mut self,
        renderer: &mut flint_render::SceneRenderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        let core = &mut self.core;
        if let Some(living) = core.subsystems.find_mut::<LivingSubsystem>() {
            living_props::upload_blends(
                &mut core.living_props,
                &mut living.blend,
                renderer,
                device,
                queue,
            );
        }
    }
}
