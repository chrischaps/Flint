//! A [`Stage`] (#725, #1145): one scene in one renderer, with its own
//! clocks, its game stepped in the player's frame order.

use super::*;

/// One cell's scene, as [`Stage::load`] takes it.
pub(super) struct StageScene<'a> {
    pub(super) world: flint_ecs::FlintWorld,
    pub(super) scene_file: flint_scene::SceneFile,
    /// The scene as the stage's renderer binds it (#739).
    pub(super) binding: &'a flint_render::SceneBinding,
    /// Which cell this is, a row at a time; the first says what all share.
    pub(super) index: usize,
    /// Which of the sweep's values this stage takes: (column, row).
    pub(super) cell: (usize, usize),
    pub(super) grid: Option<&'a crate::commands::sweep::SweepGrid>,
    /// The cell's post-processing: the scene's, under any look and the
    /// cell's values, with the CLI's flags over it.
    pub(super) config: flint_render::PostProcessConfig,
    /// The look the cell draws with before a script asks for another, for
    /// `get_look()` (#497).
    pub(super) look: Option<String>,
    pub(super) timeline: Option<ExpressionTimeline>,
    /// Resolve the scene's procgen assets as the player does, whatever the
    /// scene: a replay of the player's capture (#1089). A render resolves
    /// them only in a scene with living props.
    pub(super) resolve_procgen: bool,
}

/// The look a stage's cell draws with before a script asks for another
/// (#497), as the player names it: a `look=` sweep's value, else `--look`
/// as given, else the scene's own (none when it couldn't be found).
pub(super) fn stage_look(
    args: &RenderArgs,
    grid: Option<&crate::commands::sweep::SweepGrid>,
    (column, row): (usize, usize),
    scene_pp: Option<&flint_scene::PostProcessDef>,
) -> Option<String> {
    grid.and_then(|g| g.cell_look(column, row))
        .or(args.look.as_deref())
        .or_else(|| scene_pp.and_then(|d| d.look.as_deref()))
        .map(str::to_string)
}

/// One scene in one renderer, with its own clocks.
pub(super) struct Stage {
    /// The scene's game (#725): its world, clock, living props and wooze
    /// field, scripts, particles and `--anim-time`'s animation, stepped in
    /// the player's frame order.
    pub(super) game: HeadlessGame,
    pub(super) camera: Camera,
    pub(super) renderer: SceneRenderer,
    /// The moment of the last capture, which the next one's style history
    /// follows on from.
    pub(super) captured: Option<f64>,
    /// Whether the game's particles are loaded (they are drawn).
    particles_loaded: bool,
    /// The project the scene is in (ADR 0114), found once when it loads:
    /// what every asset search of this stage asks.
    pub(super) project: flint_core::project::Project,
    /// How many fixed steps the particles have taken.
    particle_steps: u32,
    /// Whether the particles step with the other clocks, for the scripts' sake,
    /// rather than all at once at a capture.
    particles_on_clock: bool,
    /// Whether the first capture's particle count has been said.
    particles_said: bool,
    /// Whether the scene's scripts run (`--scripts`, #8, or a timeline).
    pub(super) scripted: bool,
    /// The renderer's post config generation at the last capture: a style the
    /// scripts switched on since gets its resources then.
    post_generation: u64,
    /// The whole-frame styles as the last capture left them, to tell the
    /// scripts' changes since from a look's.
    laid_styles: flint_render::stylize::StyleSettings,
    /// What the scripts' look was when it last went to the renderer.
    look_applied: ScriptLook,
    /// What the deprecated render mode overwrote (#746), to put back.
    mode_shim: flint_render::render_mode::RenderModeShim,
    /// A script's cross-fade to a look (#496) and the game time it was asked
    /// at: each capture lays it at its own moment, as the player's frame does.
    look_fade: Option<(flint_render::look_fade::LookFade, f64)>,
    /// The scene's own `[post_process]` block, which a script's `set_look`
    /// lays a look under (#697), as the player's does.
    scene_post: Option<flint_scene::PostProcessDef>,
    /// What each capture's frame inputs keep from the load (#739): the
    /// pixels per point, the 2D view, and the boil `--boil-time` holds.
    pixels_per_point: f32,
    view_2d: flint_render::View2d,
    pinned_boil: flint_core::field_frame::BoilFrame,
    /// Seconds between the last two captures, for styles whose history is a
    /// rate (#342); 1/60 until a second capture follows on.
    style_dt: f32,
    /// A captured frame's inputs as data (#1089), for its `replay/`.
    pub(super) replay_inputs: Option<flint_render::frame_replay::FrameInputsState>,
    /// The HUD's painter when scripts run (#731): their draw calls and UI
    /// documents over each capture, as the player draws them.
    hud: Option<flint_player::HeadlessHud>,
}

impl Stage {
    /// Load a column's scene into a new renderer, ready for its first capture.
    pub(super) fn load(
        args: &RenderArgs,
        ctx: &HeadlessContext,
        scene: StageScene,
    ) -> Result<Self> {
        let StageScene {
            world,
            scene_file,
            binding,
            index,
            cell,
            grid,
            config: mut pp_config,
            look: cell_look,
            timeline,
            resolve_procgen,
        } = scene;
        let moment = is_moment(args, grid);

        // Configure camera — scene-level first, then CLI overrides take precedence
        let mut camera = Camera::new();
        camera.aspect = ctx.aspect_ratio();

        // Apply scene-level camera configuration
        let mut scene_set_position = false;
        if let Some(cam_def) = &scene_file.camera {
            if cam_def.projection == "orthographic" {
                camera.orthographic = true;
                if cam_def.ortho_height > 0.0 {
                    camera.ortho_height = cam_def.ortho_height;
                }
            }
            if let Some(pos) = cam_def.position {
                camera.position = Vec3::new(pos[0], pos[1], pos[2]);
                scene_set_position = true;
            }
            if let Some(target) = cam_def.target {
                camera.target = Vec3::new(target[0], target[1], target[2]);
            }
            if let Some(fov) = cam_def.fov {
                camera.fov = fov;
            }
            if let Some(near) = cam_def.near {
                camera.near = near;
            }
            if let Some(far) = cam_def.far {
                camera.far = far;
            }
        }

        // Derive orbit parameters from position/target so update_orbit() is consistent
        if scene_set_position {
            let dir = camera.position - camera.target;
            camera.distance = dir.length();
            if camera.distance > 0.001 {
                let n = dir * (1.0 / camera.distance);
                camera.pitch = n.y.asin();
                camera.yaw = n.x.atan2(n.z);
            }
        }

        // CLI overrides take precedence
        if let Some(d) = args.camera.distance {
            camera.distance = d;
        }
        if let Some(y) = args.camera.yaw {
            camera.yaw = y.to_radians();
        }
        if let Some(p) = args.camera.pitch {
            camera.pitch = p.to_radians();
        }
        if let Some(t) = args.camera.target {
            camera.target = Vec3::new(t[0], t[1], t[2]);
        }
        if let Some(f) = args.camera.fov {
            camera.fov = f;
        }
        // The player draws from the authored position, and going round through the
        // orbit moves it by a rounding error, which is enough to change thousands of
        // pixels. So a capture that claims to be the player's stays where the scene
        // put it; an ordinary render keeps the pixels it has always had.
        let orbit_asked = args.camera.distance.is_some()
            || args.camera.yaw.is_some()
            || args.camera.pitch.is_some()
            || args.camera.target.is_some();
        let as_authored =
            scene_set_position && !orbit_asked && (on_clock(args) || living_wanted(&world));
        if !as_authored {
            camera.update_orbit();
        }

        // Create scene renderer
        let made = load_report::span(load_report::Stage::Renderer, String::new);
        let mut renderer = SceneRenderer::new_headless(
            &ctx.device,
            &ctx.queue,
            ctx.format,
            ctx.width,
            ctx.height,
            RendererConfig {
                // The grid is an editor's. A capture of a moment is the game's view.
                show_grid: !args.camera.no_grid && !moment,
                sample_count: args.shading.msaa,
            },
        );
        drop(made);
        // Parts skipped or repeated by label (#1317)
        renderer.set_pass_knobs(args.measure.pass_knobs.knobs()?);

        // The scene bound, its levers set (#739), as the one-renderer path does
        bind_capture_scene(args, ctx, &mut renderer, &world, &scene_file, binding);

        // Load models and textures from the scene
        let config = ModelLoadConfig::from_scene_path(&args.scene);

        // The scene's game (#725), which owns the world from here
        let wants_living = living_wanted(&world);
        let mut game = HeadlessGame::new(world, &args.scene, config.project.clone());

        // Living props (#216), and the procgen assets their scene asks for, before the
        // model loader as in the player: catalog > procgen > file. A build
        // without the procgen package (#1333) makes none: every name is a file.
        #[cfg(feature = "procgen")]
        if wants_living || resolve_procgen {
            game.resolve_procgen(
                flint_procgen::LivingTables::or_warn(&scene_file)
                    .living
                    .as_ref(),
                GpuSink {
                    renderer: &mut renderer,
                    device: &ctx.device,
                    queue: &ctx.queue,
                    config: &config,
                },
            );
        }
        // A preset column names a preset file beside the scene before a built-in,
        // as the player's Load does (#249), and the file's [living] tables, species
        // and all, go over the scene's as they do there.
        let (column_preset, column_knobs) = cell_knobs(args, grid, cell)?;
        #[cfg(not(feature = "procgen"))]
        let _ = resolve_procgen;
        #[cfg(not(feature = "living"))]
        let _ = (wants_living, &column_knobs);
        #[cfg(feature = "living")]
        if let (true, Some(preset)) = (wants_living, &column_preset) {
            let props = game.living_props_mut();
            for problem in flint_player::apply_living_preset(props, &preset.living) {
                eprintln!("Warning: --sweep preset: {problem}");
            }
        }
        // The stage's one wooze field (#581), with the column's knobs over the scene's:
        // the living props read it, the shading boil always, and shallow wooze in a
        // capture of a moment. A plain render shows those meshes at rest (#215).
        // A build without the living package runs no field (#1333).
        #[cfg(feature = "living")]
        let field = flint_player::wooze_of_scene(
            flint_procgen::LivingTables::or_warn(&scene_file)
                .wooze
                .as_ref(),
            column_knobs.as_ref(),
        )
        .map_err(|reason| anyhow!("--sweep: {reason}"))?;
        // The scene's subsystems, every one the player's files build (#1252):
        // the living subsystem runs the props and the field (as its first
        // frame, the props read it without moving), the music and the
        // declared fields come for the scripts
        #[cfg(feature = "living")]
        let field_for_boil = field.clone();
        let scripted = args.clock.scripts || timeline.is_some();
        game.start_subsystems(
            scene_file.scene_tables(),
            flint_player::CaptureStart {
                #[cfg(feature = "living")]
                wooze: field,
                moment,
                scripts: scripted,
            },
        );

        let models = model_loader::load_models_from_world(
            game.world_mut(),
            &mut renderer,
            &ctx.device,
            &ctx.queue,
            &config,
        );

        // Meshes swept along the scene's splines (#750)
        flint_render::load_scene_splines(
            &args.scene,
            game.world_mut(),
            &mut renderer,
            &ctx.device,
            &ctx.queue,
        );

        // Post-processing: the column's, with a preset column's shading boil
        // under the CLI's boil flags
        if let Some(boil) = column_preset.as_ref().and_then(|preset| preset.boil) {
            let on = boil.enabled.unwrap_or(true);
            let (paint, ink) = pp_config.boil_amounts();
            let amount = |v: Option<f32>, was: f32| v.map_or(was, |v| if on { v } else { 0.0 });
            pp_config.set_boil_amounts(amount(boil.kuwahara, paint), amount(boil.contour, ink));
            if let Some(cadence) = boil.on {
                pp_config.boil_on = cadence.clamp(1, 3);
            }
            render_post::apply_flags(args, &mut pp_config, binding.styles())?;
        }
        // The shading boil's drawing (#214) comes from the field where the camera
        // is. --boil-time picks one and holds it, in one tick at the tempo of that
        // moment. Otherwise each capture stamps the drawing its clock has reached.
        let (paint, ink) = pp_config.boil_amounts();
        let boil_wanted = paint > 0.0 || ink > 0.0;
        #[cfg(not(feature = "living"))]
        if boil_wanted && index == 0 {
            if scene_file.tables.contains_key("wooze") {
                say!("This flint was built without the living package, so nothing boils.");
            } else {
                say!("This scene has no [wooze] table, so nothing boils.");
            }
        }
        #[cfg(feature = "living")]
        match (&field_for_boil, args.clock.boil_time) {
            (Some(field), Some(time)) => {
                pp_config.boil = flint_player::boil_at(
                    field,
                    camera.position_array(),
                    time.max(0.0),
                    pp_config.boil_on,
                );
            }
            (None, _) if boil_wanted && index == 0 => {
                say!("This scene has no [wooze] table, so nothing boils.");
            }
            _ => {}
        }
        // The drawing --boil-time holds every capture at (none without it)
        let pinned_boil = pp_config.boil;
        // The ASCII grid read back, for text, flips and measures (#596)
        let report_file = args
            .measure
            .report
            .as_deref()
            .is_some_and(|r| r != REPORT_TO_STDOUT);
        // The debug views, shadows, the column's post-processing and the
        // panels its scripts may cut between, render scale and quality, as
        // the one-renderer path
        set_up_capture(
            args,
            ctx,
            &mut renderer,
            CaptureSetup {
                scene_file: &scene_file,
                config: pp_config,
                text_capture: args.out.ascii_text.is_some()
                    || args.out.ascii_text_ansi.is_some()
                    || args.seq.flip_stats
                    || args.seq.flip_json.is_some()
                    || report_file
                    || args.measure.reference_text.is_some(),
                first: index == 0,
            },
        )?;
        // What scripts are told the capture draws at
        let pixels_per_point = args.out.pixels_per_point.unwrap_or(1.0);
        game.set_pixels_per_point(pixels_per_point);

        // Scripts (#8), as the player starts them once its models are loaded.
        game.set_scene_look(cell_look.as_deref());
        // Their HUD's painter first (#731), so measure_text and the UI
        // documents lay text out as it draws
        let hud = scripted.then(|| {
            flint_player::HeadlessHud::new(
                &ctx.device,
                &ctx.queue,
                ctx.format,
                flint_core::project::AssetScope::new(
                    config.project.clone(),
                    Path::new(&args.scene)
                        .parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or(Path::new(".")),
                ),
            )
        });
        if let Some(hud) = &hud {
            game.set_text_measurer(hud.measurer());
        }
        // The screen in points, as the player tells its scripts
        game.start_scripts(
            scripted,
            timeline,
            (
                ctx.width as f32 / pixels_per_point,
                ctx.height as f32 / pixels_per_point,
            ),
            renderer.post_process_config(),
        );
        // A menu nothing opens without input (#731)
        if scripted {
            let (show, focus) = (&args.clock.ui_show, args.clock.ui_focus.as_deref());
            let missing = game.with_ui(|ui| {
                let mut missing = Vec::new();
                for id in show.iter().map(String::as_str).chain(focus) {
                    if !ui.exists(id) {
                        missing.push(id.to_string());
                    }
                }
                for id in show {
                    ui.show(id);
                }
                if focus.is_some() {
                    ui.focus(focus);
                }
                missing
            });
            if !missing.is_empty() {
                anyhow::bail!(
                    "--ui-show / --ui-focus: no UI document the scripts loaded has {}",
                    missing.join(", ")
                );
            }
        }
        // A capture that draws particles has them from here, so what the scripts
        // emit reaches them where it would in the player (#579), and they step
        // on the scripts' clock. One drawing them at --particle-time steps there.
        let particles_drawn = args.clock.particle_time.is_some() || args.clock.time.is_some();
        let mut particles_loaded = false;
        if scripted && particles_drawn {
            load_particles(&config.project, &args.scene, &mut game, &mut renderer, ctx)?;
            particles_loaded = true;
        }
        let particles_on_clock = particles_loaded && args.clock.particle_time.is_none();
        // The game's first frame, the player's: no time passes, the timeline's
        // states at 0 are heard, what the scripts ask for is done, and the
        // living clocks and the field are read where they stand.
        game.step(
            0.0,
            &mut camera,
            Some(&mut renderer),
            Some((&ctx.device, &ctx.queue)),
        );

        // Animation (#307) after the scripts' first frame, as in the player's, and
        // IK after it; without it, IK (ADR 0070) poses authored chains once. A
        // sweep's later cells say nothing the first didn't.
        let step = args.clock.frame_step.unwrap_or(ANIM_DT);
        let animation = pose_animation(
            args,
            &models,
            &config.project,
            game.world_mut(),
            step,
            index == 0,
        );
        if animation.is_none() {
            flint_animation::IkPass::new().run(game.world_mut());
        }
        game.set_animation(animation);

        if let Some(name) = &args.camera.select {
            let id = game
                .world()
                .get_id(name)
                .with_context(|| format!("--select: no entity named '{name}'"))?;
            renderer.set_selected_entity(Some(id));
        }

        let post_generation = renderer.post_config_generation();
        let laid_styles = renderer.post_process_config().styles.clone();
        let view_2d = capture_view_2d(&camera);
        Ok(Self {
            game,
            camera,
            renderer,
            captured: None,
            particles_loaded,
            project: config.project,
            particle_steps: 0,
            particles_on_clock,
            particles_said: false,
            scripted,
            post_generation,
            laid_styles,
            look_applied: ScriptLook::default(),
            mode_shim: Default::default(),
            look_fade: None,
            scene_post: scene_file.post_process.clone(),
            pixels_per_point,
            view_2d,
            pinned_boil,
            style_dt: FrameInputs::default().dt,
            replay_inputs: None,
            hud,
        })
    }

    /// Lay on the renderer what the scripts have asked of it that waits for
    /// the capture: each look asked since the last capture, in the order
    /// asked (it replaces the config, as the player's does, or fades to it
    /// from the moment it was asked, #496), then the post-processing
    /// overrides, sticky as the player's. Their styles, boil tunes, distance
    /// and quality levers landed as they asked. `t` is the capture's moment
    /// on the game clock.
    ///
    /// A fade starts from the frame as it was when it was asked (#1157): a
    /// cut asked before it since the last capture, or a fade under way at
    /// that moment of it, never the config the last capture drew, which
    /// could be the scene's authored look several looks ago.
    fn lay_script_requests(&mut self, args: &RenderArgs, t: f64) -> Result<()> {
        let asked = self.game.take_look_requests();
        let mut config = self.renderer.post_process_config().clone();
        let mut changed = false;
        // Whole-frame styles the scripts changed since the last capture,
        // which go over a look's, as they did before the look
        let styled = config.styles != self.laid_styles;
        for flint_player::LookRequest { name, seconds, at } in &asked {
            // The frame at the moment this look was asked
            let mut before = config.clone();
            if let Some((fade, began)) = &self.look_fade {
                fade.at((at - began) as f32).apply(&mut before);
            }
            let base = match &self.scene_post {
                Some(def) => def.clone(),
                None => flint_scene::PostProcessDef::from_authored(toml::Table::new())
                    .map_err(|e| anyhow::anyhow!("set_look: {e}"))?,
            };
            let scope = flint_core::project::AssetScope::new(
                self.project.clone(),
                Path::new(&args.scene)
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            );
            match flint_render::look::resolve_in(&base, name.as_deref(), &scope) {
                Ok(def) => {
                    let boil = config.boil;
                    let styles = styled.then(|| config.styles.clone());
                    let mut target = post_process_config(args, Some(&def), self.renderer.styles())?;
                    // The shading boil's drawing is the clock's, not the look's
                    target.boil = boil;
                    if let Some(styles) = styles {
                        target.styles = styles;
                    }
                    // The scripts' overrides go over the new look
                    self.look_applied = ScriptLook::default();
                    self.mode_shim.forget();
                    changed = true;
                    self.look_fade = None;
                    if *seconds > 0.0 && seconds.is_finite() {
                        // A style the new look turns on is built at the
                        // capture it shows in
                        let fade = flint_render::look_fade::LookFade::new(
                            before.clone(),
                            target,
                            *seconds as f32,
                            self.renderer.styles(),
                        );
                        config = before;
                        self.look_fade = Some((fade, *at));
                    } else {
                        config = target;
                    }
                }
                Err(e) => tracing::warn!("set_look: {e}"),
            }
        }
        let fading = self.look_fade.is_some();
        if let Some((fade, at)) = &self.look_fade {
            let at_moment = fade.at((t - at) as f32);
            at_moment.apply(&mut config);
            changed = true;
            if at_moment.done() {
                self.look_fade = None;
            }
        }
        let look = self.game.look();
        // A fade rewrites what the looks differ in each capture, so the
        // scripts' overrides go over it again
        if look != self.look_applied || fading {
            flint_player::apply_look(&mut config, &look, &mut self.mode_shim);
            self.look_applied = look;
            changed = true;
        }
        if changed {
            self.renderer.set_post_process_config(config);
        }
        self.laid_styles = self.renderer.post_process_config().styles.clone();
        Ok(())
    }

    pub(super) fn capture(
        &mut self,
        args: &RenderArgs,
        ctx: &HeadlessContext,
        t: f64,
        frame_step: f64,
    ) -> Result<image::RgbaImage> {
        self.advance(ctx, t, frame_step);
        // Poses held on twos tick on the capture's moment (#405), the post
        // time's own seconds, stamped before the bones go up
        self.renderer.set_motion_time(t);
        #[cfg(feature = "living")]
        self.game
            .upload_living(&mut self.renderer, &ctx.device, &ctx.queue);
        if let Some(posed) = self.game.animation() {
            posed.upload(&mut self.renderer, &ctx.device, &ctx.queue);
        }
        // What the scripts asked of the renderer that waits for a capture
        // (#697): a look, then the post overrides
        if self.scripted {
            self.lay_script_requests(args, t)?;
        }
        // A style switched on since the last capture (by a script, a look)
        // gets what it draws with
        if self.renderer.post_config_generation() != self.post_generation {
            self.renderer.ensure_style_resources(&ctx.device);
            self.renderer.ensure_fxaa_resources(&ctx.device);
            self.post_generation = self.renderer.post_config_generation();
        }
        // Styles with history (ASCII's, #277) follow on from the last capture
        if let Some(before) = self.captured {
            if t > before {
                self.style_dt = (t - before) as f32;
            }
        }
        self.captured = Some(t);

        // Particles (ADR 0068): fixed-step, fixed-seed simulation so a snapshot
        // at a given time is reproducible run to run.
        let particle_time = args
            .clock
            .particle_time
            .or_else(|| on_clock(args).then_some(t as f32));
        if let Some(seconds) = particle_time {
            self.step_particles(args, ctx, seconds)?;
        }

        // The frame's inputs (#739). On the clock, the post time and the
        // grass are the player's: the capture's moment (grain has its own
        // flag, woozy grass the field's clock). Off it, the post time stays
        // where the load put it, and the grass where the load's first step
        // left it: woozy grass's sway and comb, else at rest.
        let clocked = on_clock(args);
        let style_time = args
            .style
            .style_time
            .or(args.clock.grain_time)
            .map(f64::from);
        let mut inputs = FrameInputs {
            time: t,
            dt: self.style_dt,
            look_time: if clocked {
                style_time
            } else {
                Some(style_time.unwrap_or(0.0))
            },
            boil: self.pinned_boil,
            pixels_per_point: self.pixels_per_point,
            view_2d: self.view_2d,
            // The deprecated render mode's sea (#746), a script's or the flag's
            ocean_tint: self
                .mode_shim
                .ocean_tint()
                .or_else(|| legacy_render_mode(args).and_then(|m| m.ocean_tint())),
            ..Default::default()
        };
        // The game's boil, shallow wooze and woozy grass, as the player's
        let on = self.renderer.post_process_config().boil_on;
        self.game.frame_inputs(&self.camera, on, &mut inputs);
        if args.clock.boil_time.is_some() {
            inputs.boil = self.pinned_boil;
        }
        if !clocked {
            inputs.grass_time = inputs.grass_time.or(Some(0.0));
        }
        inputs.particles = particle_time.map(|_| &self.game.particles().sync);
        // A captured frame keeps its inputs for its replay (#1089)
        self.replay_inputs = self
            .renderer
            .capturing_frame()
            .then(|| flint_render::frame_replay::FrameInputsState::of(&inputs, self.game.world()));
        self.renderer
            .prepare(&ctx.device, &ctx.queue, self.game.world(), &inputs);

        // Render, as comic panels when the scene has them (#880)
        self.renderer.render_frame_to(
            &ctx.device,
            &ctx.queue,
            ctx.depth_target(),
            &self.camera,
            &ctx.color_view,
            Some(self.game.world()),
        );

        // The HUD over it, as the player draws it over its frame (#731)
        if let Some(hud) = &mut self.hud {
            hud.paint(
                &ctx.device,
                &ctx.queue,
                &ctx.hud_view(),
                [ctx.width, ctx.height],
                self.pixels_per_point,
                self.game.hud(),
            );
        }

        // Read back pixels
        let pixels =
            pollster::block_on(ctx.read_pixels()).context("Failed to read rendered pixels")?;
        image::RgbaImage::from_raw(ctx.width, ctx.height, pixels)
            .context("Failed to create image from pixel data")
    }

    /// Draw the moment `t` again under `config` (an `--ai-refine` candidate,
    /// #596): the clocks stay where they are, and the style history starts
    /// over, as the one-renderer path draws each candidate as a first frame.
    #[cfg(feature = "ai")]
    pub(super) fn redraw(
        &mut self,
        args: &RenderArgs,
        ctx: &HeadlessContext,
        t: f64,
        frame_step: f64,
        mut config: flint_render::PostProcessConfig,
    ) -> Result<image::RgbaImage> {
        // The shading boil's drawing is the clock's, not the look's
        config.boil = self.renderer.post_process_config().boil;
        self.renderer.set_post_process_config(config);
        self.renderer.ensure_style_resources(&ctx.device);
        self.renderer.ensure_fxaa_resources(&ctx.device);
        self.renderer.reset_style_history();
        // The scripts' look goes over the candidate's, as over the scene's
        self.look_applied = ScriptLook::default();
        self.mode_shim.forget();
        self.captured = None;
        self.capture(args, ctx, t, frame_step)
    }

    /// Draw the next capture as a first frame (#574): no style history to
    /// follow on from and the load's frame dt, so a strip's tile at `t` is
    /// what `--time t` rendered alone draws. The clocks are left as they are.
    pub(super) fn forget_history(&mut self) {
        self.renderer.reset_style_history();
        self.captured = None;
        self.style_dt = FrameInputs::default().dt;
    }

    /// Move the clocks on to `t` a `frame_step` at a time, landing on `t` exactly,
    /// without a picture. `t` only goes forward.
    pub(super) fn advance(&mut self, ctx: &HeadlessContext, t: f64, frame_step: f64) {
        let (steps, rest) = render_plan::steps_within(t - self.game.clock(), frame_step);
        for _ in 0..steps {
            self.step_clocks(ctx, frame_step);
        }
        if rest > 0.0 {
            self.step_clocks(ctx, rest);
        }
    }

    /// One frame of the game (#725): the player's frame order, scripts and what
    /// they ask for, animation, then the wooze field and living props. The
    /// particles step after it, on their fixed step.
    fn step_clocks(&mut self, ctx: &HeadlessContext, dt: f64) {
        self.game.step(
            dt,
            &mut self.camera,
            Some(&mut self.renderer),
            Some((&ctx.device, &ctx.queue)),
        );
        // The player's particles run after its scripts, on the frame's clock.
        if self.particles_on_clock {
            self.step_particles_to(self.game.clock() as f32);
        }
    }

    /// Step the particles on to `seconds` in fixed steps, counting them, so that
    /// a capture at `seconds` finds nothing left to do.
    fn step_particles_to(&mut self, seconds: f32) {
        if !self.particles_loaded {
            return;
        }
        let wanted = (seconds.max(0.0) / PARTICLE_DT).round() as u32;
        let steps = wanted.saturating_sub(self.particle_steps);
        let whole = self.renderer.effective_post_process_config().motion_hold;
        let (world, particles) = self.game.world_and_particles();
        // Emitters on held entities (#873) hold on each step's own time, as
        // the player stamps its frame's before the particles step
        particles.sync.set_motion_hold(whole);
        for k in 1..=steps {
            let at = (self.particle_steps + k) as f64 * PARTICLE_DT as f64;
            particles.sync.set_motion_time(at);
            particles.simulate_to(world, PARTICLE_DT, PARTICLE_DT);
        }
        self.particle_steps += steps;
    }

    fn step_particles(
        &mut self,
        args: &RenderArgs,
        ctx: &HeadlessContext,
        seconds: f32,
    ) -> Result<()> {
        if !self.particles_loaded {
            load_particles(
                &self.project,
                &args.scene,
                &mut self.game,
                &mut self.renderer,
                ctx,
            )?;
            self.particles_loaded = true;
        }
        self.step_particles_to(seconds);
        let (_, particles) = self.game.world_and_particles();
        // Packed for the capture's frame inputs
        particles.pack(Some(self.camera.position_array()));
        let first = !std::mem::replace(&mut self.particles_said, true);
        if first && (args.clock.particle_time.is_some() || particles.sync.emitter_count() > 0) {
            say!(
                "Particles: {} alive across {} emitter(s) at t = {:.2}s",
                particles.sync.total_alive(),
                particles.sync.emitter_count(),
                seconds
            );
        }
        Ok(())
    }
}

/// The particles' fixed step (ADR 0068), so a capture at a time is the same run
/// to run.
const PARTICLE_DT: f32 = 1.0 / 60.0;

/// The scene's particle system, in its game: every effect beside it
/// registered, the world's emitters found, and their textures uploaded.
pub(super) fn load_particles(
    project: &flint_core::project::Project,
    scene: &str,
    game: &mut HeadlessGame,
    renderer: &mut SceneRenderer,
    ctx: &HeadlessContext,
) -> Result<()> {
    use flint_runtime::RuntimeSystem;
    // What the scripts emit reaches them from here
    game.set_particles_drawn(true);
    let (world, particles) = game.world_and_particles();
    flint_particles::load_particle_effects_from_world(project, scene, particles);
    particles
        .initialize(world)
        .context("Failed to initialize particles")?;
    flint_render::load_scene_particle_textures(
        renderer,
        &ctx.device,
        &ctx.queue,
        &particles.sync,
        project,
        Path::new(scene).parent().unwrap_or(Path::new(".")),
    );
    Ok(())
}
