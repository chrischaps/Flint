//! The game with no window (#8, #725): what `flint render` steps, so a
//! capture at a game time shows what the player shows at it.
//!
//! A [`HeadlessGame`] is a [`GameCore`] and a capture's part in its frame.
//! A step is [`game_core::step`], the player's own frame order, run with a
//! capture as its host ([`CaptureFrame`]): the same stages and subsystems
//! (living props and the wooze field, an expression timeline), the same
//! scripts, and their commands through the same dispatcher. What differs is
//! the host's: the clock is the caller's fixed step, there is no window, no
//! sound and no physics, and animation (`--anim-time`) and particles step on
//! fixed steps of their own.
//!
//! It is deterministic: `random()` is a seeded sequence
//! ([`ScriptSystem::make_deterministic`](flint_script::ScriptSystem::make_deterministic)),
//! scripts run in the order of their entities' names, there is no input,
//! and living props make every pose a step asks for at once, off the frame
//! budget. Same scene, same timeline, same pixels.
//!
//! What a capture can't do is [`PRESENTATION_ONLY`], refused before the
//! dispatcher sees it; a test holds every command to being done headless
//! or on that list. Of those, sound and the window's pointer and touch have
//! nothing to show and are dropped quietly; the rest is named once in
//! [`HeadlessGame::notes`], so a review of the capture knows what it isn't
//! showing.

use std::collections::BTreeSet;

use flint_core::project::Project;
use flint_core::quality::QualitySettings;
use flint_core::wooze::{BoilFrame, WoozeField};
use flint_ecs::FlintWorld;
use flint_particles::ParticleSystem;
use flint_procgen::LivingProps;
use flint_render::{Camera, PostProcessConfig, SceneRenderer};
use flint_runtime::{InputState, RuntimeSystem};
use flint_script::context::ScriptCommand;
use flint_script::{PostProcessOverrides, ScriptHost};

use super::expression_feed::{ExpressionFeed, ExpressionSubsystem, ExpressionTimeline};
use super::game_core::{self, FrameHost, GameCore, Lent};
use super::headless_animation::HeadlessAnimation;
use super::living_subsystem::LivingSubsystem;
use super::style_commands::SceneStyles;

/// The seed of `random()` in a headless run. One fixed value, so that every
/// render of a scene draws the same numbers.
pub const SCRIPT_SEED: u64 = 0x5EED_0F08;

/// A command a headless capture doesn't do, and why.
#[derive(Debug, Clone, Copy)]
pub struct Skipped {
    /// The script function, as [`ScriptHost::unsupported`] names it.
    pub command: &'static str,
    /// Why a capture can't. `None`: nothing in a picture shows it, so it is
    /// dropped without a note.
    pub why: Option<&'static str>,
}

const fn quiet(command: &'static str) -> Skipped {
    Skipped { command, why: None }
}

const fn noted(command: &'static str, why: &'static str) -> Skipped {
    Skipped {
        command,
        why: Some(why),
    }
}

/// The commands a headless capture leaves to the player: sound, the window's
/// pointer and touch, the clock, files, the A/B view, other scenes, quitting
/// and physics. Every other command is applied headless; a test
/// (`headless_host_tests.rs`) fails on a command that is neither.
pub const PRESENTATION_ONLY: &[Skipped] = &[
    quiet("play_sound"),
    quiet("play_sound_at"),
    quiet("stop_sound"),
    quiet("set_audio_lowpass"),
    quiet("set_touch_controls"),
    quiet("set_cursor_captured"),
    quiet("rumble"),
    noted(
        "set_time_scale",
        "the capture's clock is its own fixed step",
    ),
    noted(
        "set_camera_cut_detection",
        "a capture has no camera-cut detection to switch",
    ),
    noted("save_living_preset", "a capture writes no files"),
    noted("commit_living", "a capture writes no files"),
    noted("compare_living", "the A/B view is the player's window"),
    noted("flip_living", "the A/B view is the player's window"),
    noted("split_living", "the A/B view is the player's window"),
    noted(
        "stop_comparing_living",
        "the A/B view is the player's window",
    ),
    noted("load_scene", "a capture draws the one scene it loaded"),
    noted("reload_scene", "a capture draws the one scene it loaded"),
    noted("load_chunk", "a capture draws the scene as it loaded"),
    noted("unload_chunk", "a capture draws the scene as it loaded"),
    noted("quit_game", "a capture ends at its last frame"),
    noted("set_velocity_2d", "physics doesn't step in a capture"),
];

/// The particle commands, which a capture drawing no particles drops.
const PARTICLE_COMMANDS: &[&str] = &[
    "emit_burst",
    "play_effect",
    "stop_effect",
    "set_effect_param",
];

/// What the scripts have asked of the picture so far.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ScriptLook {
    pub post: PostProcessOverrides,
    pub fog_color: Option<[f32; 3]>,
    /// The render mode of the last step: shown only while asked for.
    pub render_mode: Option<(u32, f32)>,
    pub mode_params: Option<[f32; 4]>,
}

/// One scene's game, stepped on a clock someone else keeps.
pub struct HeadlessGame {
    core: GameCore,
    /// Nobody presses anything in a capture.
    input: InputState,
    screen: (f32, f32),
    /// The scene's post-processing as it loaded, for `reset_post_style`.
    authored: PostProcessConfig,
    /// Whether the capture draws particles: if not, what the scripts emit
    /// is dropped with a note.
    particles_drawn: bool,
    /// `--anim-time` (#307), stepped where the player's animation is.
    animation: Option<HeadlessAnimation>,
    /// What the scripts asked for and didn't get, once each.
    notes: BTreeSet<String>,
    /// The commands of [`PRESENTATION_ONLY`] the scripts asked for.
    skipped: BTreeSet<&'static str>,
    /// Post-processing as the scripts last left it. The renderer's config is
    /// sticky in the player, so only the last write of each value matters.
    look: ScriptLook,
    /// `set_look` / `clear_look` since the stage last asked.
    look_request: Option<Option<String>>,
    /// What a debug key would say (#367): nobody presses one in a capture.
    key_note: super::key_note::KeyNote,
    #[cfg(feature = "debug-hud")]
    panels: Vec<Box<dyn flint_debug_ui::DebugPanel>>,
}

impl HeadlessGame {
    /// The game of `world`, the scene at `scene_path` in `project`, with
    /// nothing started.
    pub fn new(world: FlintWorld, scene_path: &str, project: Project) -> Self {
        let mut core = GameCore::new(world, scene_path.to_string(), project);
        core.script.make_deterministic(SCRIPT_SEED);
        Self {
            core,
            input: InputState::new(),
            screen: (1.0, 1.0),
            authored: PostProcessConfig::default(),
            particles_drawn: false,
            animation: None,
            notes: BTreeSet::new(),
            skipped: BTreeSet::new(),
            look: ScriptLook::default(),
            look_request: None,
            key_note: Default::default(),
            #[cfg(feature = "debug-hud")]
            panels: Vec::new(),
        }
    }

    pub fn world(&self) -> &FlintWorld {
        &self.core.world
    }

    pub fn world_mut(&mut self) -> &mut FlintWorld {
        &mut self.core.world
    }

    /// Game seconds so far.
    pub fn clock(&self) -> f64 {
        self.core.clock.total_time
    }

    /// Resolve the scene's procgen assets as the player does at load, before
    /// the model loader, every one at once, with every living prop's lineage
    /// built and ring full. `living` is the scene's `[living]`.
    pub fn resolve_procgen(
        &mut self,
        living: Option<&flint_procgen::LivingDef>,
        sink: super::procgen_assets::GpuSink<'_>,
    ) {
        let core = &mut self.core;
        core.procgen_resolver
            .set_living_defaults(super::living_props::defaults_of(living));
        super::procgen_assets::resolve_scene(
            &core.scene_path,
            &mut core.world,
            &mut core.procgen_resolver,
            super::procgen_assets::ProcGenMode::Immediate,
            sink,
        );
    }

    /// The living props, if the scene grew any.
    pub fn living_props(&self) -> Option<&LivingProps> {
        let props = self.core.procgen_resolver.living_props();
        (!props.is_empty()).then_some(props)
    }

    /// The living props, to lay a preset over.
    pub fn living_props_mut(&mut self) -> &mut LivingProps {
        self.core.procgen_resolver.living_props_mut()
    }

    /// Start the living subsystem, the player's, if the scene has living
    /// props or a wooze `field` (its `[wooze]` with any sweep's knobs over
    /// it): it runs the props' clock and the field's, the boil's beats,
    /// woozy grass, shallow wooze in a capture of a `moment`, and the
    /// scripts' tunes. As the player's first frame, the props read the field
    /// without moving.
    pub fn start_living(&mut self, field: Option<WoozeField>, moment: bool) {
        let core = &mut self.core;
        if core.procgen_resolver.living_props().is_empty() && field.is_none() {
            return;
        }
        core.procgen_resolver
            .step_living_now(0.0, field.as_ref().map(|field| (field, 0.0)));
        core.subsystems
            .push(Box::new(LivingSubsystem::for_capture(field, moment)));
        core.enable_script_modules();
    }

    /// Load the scene's scripts, found in its project as the player finds
    /// them, and run their `on_init`, as the player does once its models
    /// are loaded, with the scene's music on the game clock for their
    /// `music_beat()`; and feed `timeline` to their `expression()`. With neither
    /// (no `--scripts`), the scene's scripts stay unloaded. `screen` is the
    /// capture's size, for `screen_width()` and the like; `post` the
    /// post-processing it draws with, for `on_init`'s style reads (each
    /// frame then reads the renderer's), and the scene's own, for
    /// `reset_post_style`.
    pub fn start_scripts(
        &mut self,
        scripts: bool,
        timeline: Option<ExpressionTimeline>,
        screen: (f32, f32),
        post: &PostProcessConfig,
    ) {
        self.screen = screen;
        self.authored = post.clone();
        let core = &mut self.core;
        core.script
            .set_style_reader(std::sync::Arc::new(SceneStyles::new(post.clone())));
        if let Some(timeline) = timeline {
            // The run's, as the player's feed is
            core.subsystems
                .push(Box::new(ExpressionSubsystem::new(ExpressionFeed {
                    timeline: Some(timeline),
                    ..Default::default()
                })));
        }
        if !scripts {
            return;
        }
        // The scene's music, for the scripts' beat: on the game clock, with
        // no sound (#745)
        if let Some(music) = crate::music::for_capture(&core.world, &core.project, &core.scene_path)
        {
            core.subsystems.push(music);
        }
        // The modules the capture's subsystems own (#698), as the player
        // enables them before its scripts load
        core.enable_script_modules();
        core.script
            .load_scripts_from_scene(&core.scene_path, &core.project);
        core.script
            .set_current_scene(&core.scene_path, &core.project);
        core.script.set_screen_size(screen.0, screen.1);
        let _scope =
            core.script
                .state_scope(&mut core.state_machine, &mut core.store, &core.physics);
        if let Err(e) = core.script.initialize(&mut core.world) {
            tracing::warn!("Script init failed: {e:?}");
        }
    }

    /// The particles the scripts' commands reach, to load and step. Until
    /// [`Self::set_particles_drawn`], what scripts emit is dropped with a note.
    pub fn particles_mut(&mut self) -> &mut ParticleSystem {
        &mut self.core.particles
    }

    /// The world and the particles, to step them on.
    pub fn world_and_particles(&mut self) -> (&mut FlintWorld, &mut ParticleSystem) {
        (&mut self.core.world, &mut self.core.particles)
    }

    pub fn set_particles_drawn(&mut self, drawn: bool) {
        self.particles_drawn = drawn;
    }

    /// `--anim-time`'s animation, stepped with the game, after its scripts.
    pub fn set_animation(&mut self, animation: Option<HeadlessAnimation>) {
        self.animation = animation;
    }

    pub fn animation(&self) -> Option<&HeadlessAnimation> {
        self.animation.as_ref()
    }

    /// One frame of the game, `dt` game seconds after the last, in the
    /// player's frame order, against `camera` (where the scripts move it)
    /// and `renderer` with its device (`None`: a test with no GPU).
    pub fn step(
        &mut self,
        dt: f64,
        camera: &mut Camera,
        renderer: Option<&mut SceneRenderer>,
        gpu: Option<(&wgpu::Device, &wgpu::Queue)>,
    ) {
        self.core.clock.advance_by(dt);
        let mut frame = CaptureFrame {
            game: self,
            camera,
            renderer,
            gpu,
        };
        game_core::step(&mut frame);
    }

    /// `commands` as if the scripts had just asked for them (tests).
    #[cfg(test)]
    pub(crate) fn dispatch_for_test(&mut self, commands: Vec<ScriptCommand>) {
        let mut camera = Camera::new();
        let mut frame = CaptureFrame {
            game: self,
            camera: &mut camera,
            renderer: None,
            gpu: None,
        };
        game_core::dispatch_commands(&mut frame, commands);
    }

    fn living(&self) -> Option<&LivingSubsystem> {
        self.core.subsystems.find::<LivingSubsystem>()
    }

    /// The wooze field and the time its clock has reached, with one.
    pub fn wooze(&self) -> Option<(&WoozeField, f64)> {
        let living = self.living()?;
        Some((living.wooze.as_ref()?, living.wooze_time))
    }

    /// The shading boil's drawing where the field's clock stands, for a
    /// viewer at `viewer`, with a field (#214).
    pub fn boil(&self, viewer: [f32; 3], on: u32) -> Option<BoilFrame> {
        let living = self.living()?;
        let field = living.wooze.as_ref()?;
        let sample = field.sample(viewer, living.wooze_time);
        Some(living.boil_clock.frame(&sample, on))
    }

    /// The grass's clock at `t`: the field's, for grass that woozes (#252),
    /// else the wall clock's.
    pub fn apply_grass(&self, renderer: &mut SceneRenderer, t: f64) {
        match self.living().filter(|living| living.wooze.is_some()) {
            Some(living) => living.grass_wooze.apply(renderer, t),
            None => renderer.grass_time = t as f32,
        }
    }

    /// Blend every prop where its clock stands and write what moved, as a
    /// frame of the player does.
    pub fn upload_living(
        &mut self,
        renderer: &mut SceneRenderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        let core = &mut self.core;
        if let Some(living) = core.subsystems.find_mut::<LivingSubsystem>() {
            super::living_props::upload_blends(
                &mut core.procgen_resolver,
                &mut living.blend,
                renderer,
                device,
                queue,
            );
        }
    }

    /// A look the scripts asked for since the last call (`Some(None)`:
    /// none), to lay under the scene's keys before the next draw.
    pub fn take_look_request(&mut self) -> Option<Option<String>> {
        self.look_request.take()
    }

    /// Post-processing and fog as the scripts have left them, and the last
    /// step's render mode.
    pub fn look(&self) -> ScriptLook {
        self.look
    }

    /// What the scripts asked for that a headless capture doesn't do, once each.
    pub fn notes(&self) -> Vec<String> {
        self.notes.iter().cloned().collect()
    }

    /// The commands of [`PRESENTATION_ONLY`] the scripts asked for, by name.
    pub fn skipped(&self) -> Vec<&'static str> {
        self.skipped.iter().copied().collect()
    }

    /// The expression state scripts see now.
    pub fn expression(&self) -> rhai::Map {
        self.core.script.expression()
    }

    fn note(&mut self, what: &str) {
        if self.notes.insert(what.to_string()) {
            tracing::warn!("{what}");
        }
    }
}

/// A capture's part in one frame of its game: the frame's camera and
/// renderer, lent for the step.
struct CaptureFrame<'a> {
    game: &'a mut HeadlessGame,
    camera: &'a mut Camera,
    renderer: Option<&'a mut SceneRenderer>,
    gpu: Option<(&'a wgpu::Device, &'a wgpu::Queue)>,
}

/// The script function a command is, as [`ScriptHost`] names it to
/// `unsupported`.
fn command_name(command: &ScriptCommand) -> Option<&'static str> {
    #[derive(Default)]
    struct Name(Option<&'static str>);
    impl ScriptHost for Name {
        fn unsupported(&mut self, command: &'static str) {
            self.0 = Some(command);
        }
    }
    let mut name = Name::default();
    flint_script::host::apply(&mut name, command.clone());
    name.0
}

impl FrameHost for CaptureFrame<'_> {
    fn lend(&mut self) -> (&mut GameCore, Lent<'_>) {
        let game = &mut *self.game;
        let pixels_per_point = self.renderer.as_ref().map_or(1.0, |r| r.pixels_per_point);
        (
            &mut game.core,
            Lent {
                input: &mut game.input,
                camera: &mut *self.camera,
                renderer: self.renderer.as_deref_mut(),
                gpu: self.gpu,
                pixels_per_point,
                cursor_captured: false,
                scene_file_writable: false,
                note: &mut game.key_note,
                #[cfg(feature = "debug-hud")]
                panels: &mut game.panels,
                audio: None,
                pads: None,
                quit: None,
            },
        )
    }

    fn core(&mut self) -> &mut GameCore {
        &mut self.game.core
    }

    fn capture(&self) -> bool {
        true
    }

    fn window_for_scripts(&mut self) {
        let game = &mut *self.game;
        game.core.script.set_transition_state(-1.0, "idle");
        game.core
            .script
            .set_screen_size(game.screen.0, game.screen.1);
    }

    fn refuses(&mut self, command: &ScriptCommand) -> bool {
        // A scale of 1 asks for nothing a capture lacks
        if let ScriptCommand::SetTimeScale { scale } = command {
            if *scale == 1.0 {
                return true;
            }
        }
        let Some(name) = command_name(command) else {
            return false;
        };
        let game = &mut *self.game;
        if !game.particles_drawn && PARTICLE_COMMANDS.contains(&name) {
            game.note(
                "particles: a still with no --time draws none; give it --time or \
                 --particle-time to see what the scripts emit",
            );
            return true;
        }
        let Some(skipped) = PRESENTATION_ONLY.iter().find(|s| s.command == name) else {
            return false;
        };
        game.skipped.insert(name);
        if let Some(why) = skipped.why {
            game.note(&format!("{name}: a headless capture doesn't do it ({why})"));
        }
        true
    }

    fn host_command(&mut self, command: ScriptCommand) {
        flint_script::host::apply(self, command);
    }

    fn script_ui_drawn(&mut self) {
        let game = &mut *self.game;
        if !game.core.script.drain_draw_commands().is_empty() {
            game.note("on_draw_ui: the HUD isn't drawn in a headless capture");
        }
        if !game.core.script.drain_world_lines().is_empty() {
            game.note("draw_world_line: world lines aren't drawn in a headless capture");
        }
        if !game.core.script.drain_world_texts().is_empty() {
            game.note("world text isn't drawn in a headless capture");
        }
    }

    fn bones_posed(&mut self) {
        // --anim-time's animation and IK (#307), on the frame's clock
        let game = &mut *self.game;
        if let Some(posed) = &mut game.animation {
            posed.step(&mut game.core.world, game.core.clock.delta_time);
        }
    }

    fn sync(&mut self) {
        let game = &mut *self.game;
        // This step's post-processing requests over what earlier steps left
        // (a render mode lasts the step that asks)
        let requests = &mut game.core.script_requests;
        game.look.post.merge(std::mem::take(&mut requests.post));
        if let Some(color) = requests.fog_color.take() {
            game.look.fog_color = Some(color);
        }
        game.look.render_mode = requests.render_mode.take();
        if let Some(params) = requests.mode_params.take() {
            game.look.mode_params = Some(params);
        }
        // The distance levers, as the player's frame ends
        let lod = requests.lod_scale.take();
        let draw = requests.draw_distance_scale.take();
        if let Some(renderer) = self.renderer.as_deref_mut() {
            let mut s = renderer.lod_settings();
            if lod.is_some() || draw.is_some() {
                s.lod_scale = lod.unwrap_or(s.lod_scale);
                s.draw_distance_scale = draw.unwrap_or(s.draw_distance_scale);
                renderer.set_lod_settings(s);
                s = renderer.lod_settings();
            }
            game.core
                .script
                .set_lod_state(s.lod_scale, s.draw_distance_scale);
        }
    }
}

/// The presentation's commands in a capture (`game_core::for_the_host`):
/// those [`PRESENTATION_ONLY`] doesn't refuse.
impl ScriptHost for CaptureFrame<'_> {
    fn host_name(&self) -> &'static str {
        "a headless capture"
    }

    fn unsupported(&mut self, command: &'static str) {
        self.game.skipped.insert(command);
        self.game.note(&format!(
            "{command}: a script asked for it, and a headless capture doesn't do it"
        ));
    }

    fn fire_event(&mut self, _name: String, _data: toml::Value) {
        // transition_complete: a capture has no transition to end
    }

    fn set_look(&mut self, name: Option<String>) {
        self.game.look_request = Some(name);
    }

    fn set_quality(&mut self, settings: QualitySettings) {
        // The capture's size and scale are its flags', as the player keeps
        // the render scale its own to set
        let (Some(renderer), Some((device, _))) = (self.renderer.as_deref_mut(), self.gpu) else {
            return;
        };
        let q = QualitySettings {
            render_scale: renderer.render_scale(),
            ..settings.clamped()
        };
        let applied = renderer.set_quality(device, &q);
        self.game.core.script.set_quality_state(applied);
    }

    fn reset_post_style(&mut self, style: String) {
        let game = &mut *self.game;
        game_core::reset_post_style(
            &mut game.core,
            self.renderer.as_deref_mut(),
            &game.authored,
            &style,
        );
    }
}

/// Lay `look` over a post-process config, as the player's frame does with its
/// scripts' overrides. `before` is the look laid last time, so a render mode
/// that has stopped being asked for is taken off, as the player's is.
pub fn apply_look(config: &mut PostProcessConfig, look: &ScriptLook, before: &ScriptLook) {
    let post = &look.post;
    if let Some(v) = post.vignette {
        config.vignette_enabled = v > 0.001;
        config.vignette_intensity = v;
    }
    if let Some(b) = post.bloom {
        config.bloom_intensity = b;
    }
    if let Some(e) = post.exposure {
        config.exposure = e;
    }
    if let Some(ca) = post.chromatic_aberration {
        config.chromatic_aberration = ca;
    }
    if let Some(rb) = post.radial_blur {
        config.radial_blur = rb;
    }
    if let Some(si) = post.ssao_intensity {
        config.ssao_intensity = si;
    }
    if let Some(fd) = post.fog_density {
        config.fog_density = fd;
    }
    if let Some(d) = post.desaturation {
        config.desaturate = d;
    }
    if let Some(s) = post.dof_strength {
        config.dof_strength = s;
    }
    if let Some(fd) = post.dof_focus_distance {
        config.dof_focus_distance = fd;
    }
    if let Some(fr) = post.dof_focus_range {
        config.dof_focus_range = fr;
    }
    if let Some(fc) = look.fog_color {
        config.fog_color = fc;
    }
    // A reality tear: transient, unlike the sticky overrides above
    match look.render_mode {
        Some((mode, mix)) => {
            config.render_mode = mode;
            config.mode_mix = mix;
        }
        None if before
            .render_mode
            .is_some_and(|(mode, mix)| mode != 0 && mix > 0.0) =>
        {
            config.render_mode = 0;
            config.mode_mix = 0.0;
        }
        None => {}
    }
    if let Some(mp) = look.mode_params {
        config.mode_params = mp;
    }
}

#[cfg(test)]
#[path = "headless_host_tests.rs"]
mod tests;
