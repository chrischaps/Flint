#![cfg(test)]
//! A toy subsystem, to hold #724's third box: a subsystem is what its
//! package registers (#1552: `PACKAGE`, which the test build's stock list
//! ends with), and nothing in `player_app/` names it. A scene with a
//! `[toy]` table gets one, which counts the
//! frames it hears. A capture (`HeadlessGame`) builds it from the same list
//! (#1252), unless its table says `window = true`: then it is the window's
//! alone, and the capture names it in its notes.
//!
//! It also holds #1551's hook, the one `vehicle` needs: it hears each fixed
//! physics step (`fixed_update`), and with `[toy] body = "<entity>"` and
//! `spring = k` it pulls that body towards `anchor` (default the origin)
//! with a spring of `k` newtons a metre, evaluated and applied once a step,
//! as a suspension's force is. Its script module's `toy_steps()` reads the
//! steps it has heard.

use super::{own_modules, FixedCtx, Lasts, SceneCtx, Stage, StageCtx, Subsystem};
use crate::player_app::PlayerApp;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use flint_physics::rapier::math::Vector;
use std::sync::Arc;

/// The toy package, which a test build's stock list ends with
/// (`packages::stock`): its builder, its script module `toy` (#698:
/// `toy_frames()` and `toy_steps()`, enabled in a scene with a toy) and its
/// key. It reads no game-state policy.
pub(crate) static PACKAGE: crate::packages::Part = crate::packages::Part {
    name: "toy",
    register: |registry| {
        registry
            .scene_subsystem(for_scene)
            .script_modules(&[&ToyModule])
            .debug_keys(DEBUG_KEYS);
    },
};

/// Its debug key (#717): F12 says how many frames it has heard, in a scene
/// with a toy; anywhere else F12 is the game's.
pub(crate) const DEBUG_KEYS: flint_runtime::DebugKeyGroup = flint_runtime::DebugKeyGroup {
    owner: "toy",
    when: "a scene with a [toy] table",
    keys: &[flint_runtime::DebugKey {
        action: "toy_frames",
        key: "F12",
        does: "Say how many frames the toy has heard",
    }],
};

/// What the toy hands its module's functions: the frames it has heard.
#[derive(Debug, Default)]
struct ToyFrames(u32);

/// And the fixed physics steps it has heard (#1551).
#[derive(Debug, Default)]
struct ToySteps(u32);

struct ToyModule;

impl flint_script::ScriptModule for ToyModule {
    fn name(&self) -> &'static str {
        "toy"
    }

    fn enabled_when(&self) -> &'static str {
        "the scene has a [toy] table"
    }

    fn declare(&self, m: &mut flint_script::api::Module<'_>) {
        m.func("toy_frames", "The frames the toy has heard.")
            .returns(flint_script::api::builder::kind::Int, "frames")
            .run(|call, ()| call.ctx().module_state.get_mut::<ToyFrames>().0 as i64);
        m.func("toy_steps", "The fixed physics steps the toy has heard.")
            .returns(flint_script::api::builder::kind::Int, "steps")
            .run(|call, ()| call.ctx().module_state.get_mut::<ToySteps>().0 as i64);
    }
}

#[derive(Default)]
pub(crate) struct Toy {
    pub(crate) frames: u32,
    /// The fixed steps it has heard, and each one's `FixedCtx::step`.
    pub(crate) steps: u32,
    pub(crate) step_marks: Vec<(u32, u32)>,
    /// `[toy] window = true`: a toy only a window shows.
    window: bool,
    spring: Option<Spring>,
}

/// `[toy] body`, `spring`, `anchor`: a spring on one body.
struct Spring {
    body: String,
    /// Newtons a metre.
    k: f32,
    anchor: [f32; 3],
}

/// A toy for a scene with a `[toy]` table.
pub(crate) fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    let table = ctx.tables.get("toy")?;
    let window = table.get("window").and_then(|w| w.as_bool()) == Some(true);
    use flint_core::toml_util::{toml_f32, toml_vec3};
    let spring = table
        .get("body")
        .and_then(|b| b.as_str())
        .map(|body| Spring {
            body: body.to_string(),
            k: table.get("spring").and_then(toml_f32).unwrap_or(0.0),
            anchor: table.get("anchor").and_then(toml_vec3).unwrap_or([0.0; 3]),
        });
    Some(Box::new(Toy {
        window,
        spring,
        ..Toy::default()
    }))
}

impl Subsystem for Toy {
    fn name(&self) -> &'static str {
        "toy"
    }

    fn lasts(&self) -> Lasts {
        Lasts::Scene
    }

    fn script_modules(&self) -> &[&'static str] {
        &["toy"]
    }

    fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {
        if stage == Stage::Sync {
            self.frames += 1;
            ctx.script.set_module_state(ToyFrames(self.frames));
            ctx.script.set_module_state(ToySteps(self.steps));
        }
    }

    /// The spring's pull, from where the body is at this step, as an
    /// impulse of the step's length: Rapier's step, right after, integrates
    /// it.
    fn fixed_update(&mut self, ctx: &mut FixedCtx) {
        self.steps += 1;
        self.step_marks.push(ctx.step);
        let Some(spring) = &self.spring else {
            return;
        };
        let Some(id) = ctx.world.get_id(&spring.body) else {
            return;
        };
        // Physics is the run's resource (#1616), as a package finds it
        let Some(physics) = flint_physics::of_mut(ctx.resources) else {
            return;
        };
        let Some(handle) = physics.body_of(id) else {
            return;
        };
        let Some(body) = physics.bodies_mut().get_mut(handle) else {
            return;
        };
        let at = *body.translation();
        let [ax, ay, az] = spring.anchor;
        let pull = Vector::new(ax - at.x, ay - at.y, az - at.z) * (spring.k * ctx.dt as f32);
        body.apply_impulse(pull, true);
    }

    fn debug_keys(&self) -> &'static [flint_runtime::DebugKey] {
        DEBUG_KEYS.keys
    }

    fn on_debug_key(&mut self, _action: &str, key: &str, ctx: &mut StageCtx) {
        ctx.note.say(key, &format!("{} frames", self.frames));
    }

    fn presentation_only(&self) -> Option<&'static str> {
        self.window.then_some("a toy only a window shows")
    }
}

/// The toy's F12 is a debug key only in a scene with a toy (#717): there it
/// is dispatched and never reaches the game's input, and `--list-keys`'
/// table says it is live; in a scene without one the game gets F12.
#[test]
fn the_toys_key_is_live_only_in_a_scene_with_a_toy() {
    use winit::keyboard::KeyCode;
    let mut files = MemorySource::new();
    files.insert("toys/flint.toml", "");
    files.insert(
        "toys/scenes/with.scene.toml",
        "[scene]\nname = \"With\"\n\n[toy]\n",
    );
    files.insert(
        "toys/scenes/without.scene.toml",
        "[scene]\nname = \"Without\"\n",
    );
    vfs::scoped(Arc::new(files), || {
        let mut app = app_in("toys/scenes/with.scene.toml");
        app.tick();
        let live = |app: &PlayerApp| {
            let table = app.debug_key_table();
            table
                .entries
                .iter()
                .find(|e| e.action == "toy_frames")
                .unwrap()
                .live
        };
        assert!(live(&app));
        assert!(app.handle_debug_key(KeyCode::F12), "the toy's");
        assert_eq!(app.hud.key_note.showing(), Some(("F12", "1 frames")));
        assert!(!app.input.is_key_down(KeyCode::F12));

        let mut app = app_in("toys/scenes/without.scene.toml");
        assert!(!live(&app), "listed, but not live");
        assert!(!app.handle_debug_key(KeyCode::F12), "the game's");
    });
}

fn app_in(scene: &str) -> PlayerApp {
    app_stepping(scene, 0.25)
}

/// `app_in` on a clock of `frame_step` seconds a frame.
fn app_stepping(scene: &str, frame_step: f64) -> PlayerApp {
    let registry = flint_schema::SchemaRegistry::new();
    let (world, file) = flint_scene::load_scene(scene, &registry).expect("the scene loads");
    let mut app = PlayerApp::new(world, scene.into(), false, None, None);
    app.apply_scene_settings(&file);
    app.set_schemas(
        registry,
        SchemaDirs {
            dirs: Vec::new(),
            source: SchemaSource::Flags,
            dropped: Vec::new(),
            packages: None,
        },
    );
    app.core.clock.frame_step = Some(frame_step);
    app.enter_scene(crate::player_app::scene_entry::SceneEntry::Start)
        .expect("the scene starts");
    app
}

#[test]
fn a_toy_in_its_own_file_comes_with_a_scene_that_asks_and_hears_its_frames() {
    let mut files = MemorySource::new();
    files.insert("toys/flint.toml", "");
    files.insert(
        "toys/scenes/with.scene.toml",
        "[scene]\nname = \"With\"\n\n[toy]\nsize = 1\n",
    );
    files.insert(
        "toys/scenes/without.scene.toml",
        "[scene]\nname = \"Without\"\n",
    );
    vfs::scoped(Arc::new(files), || {
        let mut app = app_in("toys/scenes/with.scene.toml");
        app.tick();
        app.tick();
        let toy = app
            .core
            .subsystems
            .find::<Toy>()
            .expect("a [toy] scene has one");
        assert_eq!(toy.frames, 2);

        assert!(app.execute_scene_transition("without.scene.toml"));
        assert!(
            app.core.subsystems.find::<Toy>().is_none(),
            "it went with its scene"
        );

        let app = app_in("toys/scenes/without.scene.toml");
        assert!(app.core.subsystems.find::<Toy>().is_none());
    });
}

/// A script asks the toy's module from its `on_init` and every frame: the
/// subsystem is built before the scripts load, so the module is enabled
/// for their first call (#698). In a scene with no toy the same call is
/// the "not enabled" error, and nothing it would have written is.
#[test]
fn a_scene_with_a_toy_lets_its_scripts_call_the_toy_module_from_on_init() {
    const COUNTER: &str = r#"
fn on_init() { set_field(self_entity(), "probe", "init", toy_frames()); }
fn on_update() { set_field(self_entity(), "probe", "seen", toy_frames()); }
"#;
    let scripted = |name: &str, toy: bool| {
        format!(
            "[scene]\nname = \"{name}\"\n{}\n[entities.counter]\n[entities.counter.probe]\n\
             init = -1\nseen = -1\n[entities.counter.script]\nsource = \"counter.rhai\"\n",
            if toy { "\n[toy]\nsize = 1\n" } else { "" }
        )
    };
    let mut files = MemorySource::new();
    files.insert("toys/flint.toml", "");
    files.insert("toys/scenes/with.scene.toml", scripted("With", true));
    files.insert("toys/scenes/without.scene.toml", scripted("Without", false));
    files.insert("toys/scripts/counter.rhai", COUNTER);
    vfs::scoped(Arc::new(files), || {
        let probe = |app: &PlayerApp, field: &str| {
            let id = app.core.world.get_id("counter").expect("the counter");
            app.core
                .world
                .get_components(id)
                .and_then(|c| c.get_field("probe", field))
                .and_then(|v| v.as_integer())
        };
        let mut app = app_in("toys/scenes/with.scene.toml");
        assert_eq!(own_modules(app.core.script.enabled_modules()), ["toy"]);
        assert_eq!(probe(&app, "init"), Some(0), "on_init reached the module");
        app.tick();
        app.tick();
        assert_eq!(probe(&app, "seen"), Some(1), "last frame's count");

        let mut app = app_in("toys/scenes/without.scene.toml");
        assert!(own_modules(app.core.script.enabled_modules()).is_empty());
        app.tick();
        assert_eq!(probe(&app, "init"), Some(-1), "the call was an error");
        assert_eq!(probe(&app, "seen"), Some(-1));
    });
}

/// The capture of `scene` (`flint render --scripts`'s game), its scene's
/// subsystems built from the player's list and its scripts started.
fn capture_of(scene: &str) -> crate::HeadlessGame {
    let registry = flint_schema::SchemaRegistry::new();
    let (world, file) = flint_scene::load_scene(scene, &registry).expect("the scene loads");
    let project = flint_core::project::Project::for_scene(std::path::Path::new(scene));
    let mut game = crate::HeadlessGame::new(world, scene, project);
    game.start_subsystems(
        file.scene_tables(),
        crate::CaptureStart {
            scripts: true,
            ..Default::default()
        },
    );
    game.start_scripts(
        true,
        None,
        (320.0, 180.0),
        &flint_render::PostProcessConfig::default(),
    );
    game
}

/// The toy, in its own file and named nowhere else, runs in a capture as in
/// the player (#1252): `HeadlessGame` builds it from the generated list,
/// steps it with the frame and enables its module for `on_init` and every
/// step after. A scene without a `[toy]` table gets none there either.
#[test]
fn a_capture_builds_the_toy_from_the_players_list_and_its_scripts_read_its_frames() {
    const COUNTER: &str = r#"
fn on_init() { set_field(self_entity(), "probe", "init", toy_frames()); }
fn on_update() { set_field(self_entity(), "probe", "seen", toy_frames()); }
"#;
    let scripted = |name: &str, toy: bool| {
        format!(
            "[scene]\nname = \"{name}\"\n{}\n[entities.counter]\n[entities.counter.probe]\n\
             init = -1\nseen = -1\n[entities.counter.script]\nsource = \"counter.rhai\"\n",
            if toy { "\n[toy]\nsize = 1\n" } else { "" }
        )
    };
    let mut files = MemorySource::new();
    files.insert("toys/flint.toml", "");
    files.insert("toys/scenes/with.scene.toml", scripted("With", true));
    files.insert("toys/scenes/without.scene.toml", scripted("Without", false));
    files.insert("toys/scripts/counter.rhai", COUNTER);
    vfs::scoped(Arc::new(files), || {
        let probe = |game: &crate::HeadlessGame, field: &str| {
            let world = game.world();
            let id = world.get_id("counter").expect("the counter");
            world
                .get_components(id)
                .and_then(|c| c.get_field("probe", field))
                .and_then(|v| v.as_integer())
        };
        let mut camera = flint_render::Camera::new();
        let mut game = capture_of("toys/scenes/with.scene.toml");
        assert_eq!(probe(&game, "init"), Some(0), "on_init reached the module");
        for _ in 0..3 {
            game.step(0.25, &mut camera, None, None);
        }
        let toy = game.core.subsystems.find::<Toy>().expect("built");
        assert_eq!(toy.frames, 3, "it heard every step");
        assert_eq!(probe(&game, "seen"), Some(2), "last step's count");
        assert!(game.notes().is_empty(), "{:?}", game.notes());
        assert_eq!(game.script_errors().total(), 0);

        let mut game = capture_of("toys/scenes/without.scene.toml");
        game.step(0.25, &mut camera, None, None);
        assert!(game.core.subsystems.find::<Toy>().is_none());
        assert_eq!(probe(&game, "seen"), Some(-1), "the call was an error");
    });
}

/// A subsystem that is the window's alone is built by the player and left
/// out of a capture, which names it once in its notes, however many steps
/// it takes (#1252).
#[test]
fn a_capture_names_a_subsystem_it_leaves_out_once_in_its_notes() {
    const SCENE: &str = "[scene]\nname = \"Window\"\n\n[toy]\nwindow = true\n";
    let mut files = MemorySource::new();
    files.insert("toys/flint.toml", "");
    files.insert("toys/scenes/window.scene.toml", SCENE);
    vfs::scoped(Arc::new(files), || {
        let app = app_in("toys/scenes/window.scene.toml");
        assert!(app.core.subsystems.find::<Toy>().is_some(), "the player's");

        let mut camera = flint_render::Camera::new();
        let mut game = capture_of("toys/scenes/window.scene.toml");
        for _ in 0..3 {
            game.step(0.25, &mut camera, None, None);
        }
        assert!(game.core.subsystems.find::<Toy>().is_none(), "left out");
        assert_eq!(
            game.notes(),
            ["toy: a headless capture doesn't run this subsystem (a toy only a window shows)"]
        );
    });
}

/// A toy whose spring pulls `bob` (1 kg, no gravity, from x = 1 m) towards
/// the origin at 4 N/m: a period of pi seconds, x = cos 2t.
const SPRING_SCENE: &str = "[scene]\nname = \"Spring\"\n\n\
     [toy]\nbody = \"bob\"\nspring = 4.0\n\n\
     [entities.bob]\n[entities.bob.transform]\nposition = [1.0, 0.0, 0.0]\n\
     [entities.bob.rigidbody]\nbody_type = \"dynamic\"\ngravity_scale = 0.0\n";

/// Where `bob` is along x.
fn bob_x(app: &PlayerApp) -> f64 {
    let id = app.core.world.get_id("bob").expect("bob");
    app.core.world.get_transform(id).expect("placed").position.x as f64
}

/// The frames that play `seconds` of game time at `frame_step` a frame: the
/// clock's first tick is 0 s long, a start and not a frame.
fn frames_for(seconds: f64, frame_step: f64) -> usize {
    (seconds / frame_step).round() as usize + 1
}

/// One frame of a spring run: the steps physics has run so far, the game
/// time and bob's x.
#[derive(Debug, Clone, Copy)]
struct Sample {
    steps: u32,
    time: f64,
    x: f64,
}

/// Bob's x once `steps` steps have run.
fn x_after(path: &[Sample], steps: u32) -> f64 {
    path.iter().rfind(|s| s.steps == steps).expect("reached").x
}

/// `scene` played for `seconds` at `frame_step` a frame. With `once_a_frame`
/// the toy has no spring (`spring = k` is read as 0) and the test pulls bob
/// itself after each frame's steps, with the same `k`: all a package could
/// do before #1551, and only with the core in hand, as a test has it.
fn spring_run(
    scene: &str,
    frame_step: f64,
    seconds: f64,
    once_a_frame: Option<f32>,
) -> (Vec<Sample>, Vec<(u32, u32)>) {
    let mut files = MemorySource::new();
    files.insert("toys/flint.toml", "");
    files.insert("toys/scenes/spring.scene.toml", scene.to_string());
    vfs::scoped(Arc::new(files), || {
        let mut app = app_stepping("toys/scenes/spring.scene.toml", frame_step);
        if once_a_frame.is_some() {
            app.core.subsystems.find_mut::<Toy>().unwrap().spring = None;
        }
        let frames = frames_for(seconds, frame_step);
        let mut path = Vec::with_capacity(frames);
        for _ in 0..frames {
            app.tick();
            if let Some(k) = once_a_frame {
                let id = app.core.world.get_id("bob").unwrap();
                if let Some(handle) = flint_physics::of_mut(&mut app.core.resources)
                    .unwrap()
                    .body_of(id)
                {
                    let body = flint_physics::of_mut(&mut app.core.resources)
                        .unwrap()
                        .bodies_mut()
                        .get_mut(handle)
                        .unwrap();
                    let x = body.translation().x;
                    body.apply_impulse(Vector::new(-x * k * frame_step as f32, 0.0, 0.0), true);
                }
            }
            let toy = app.core.subsystems.find::<Toy>().expect("a toy");
            path.push(Sample {
                steps: toy.steps,
                time: app.core.clock.total_time,
                x: bob_x(&app),
            });
        }
        let toy = app.core.subsystems.find::<Toy>().expect("a toy");
        (path, toy.step_marks.clone())
    })
}

/// #1551's first box: the toy's spring is a force a step, so a frame time
/// of 1/30 s (two steps a frame) and of 1/144 s (none on most frames, one
/// on the rest) play 2 s as the same steps and land bob in one place.
///
/// The stated tolerance is one step of bob's motion, 2 m/s x 1/60 s =
/// 0.034 m: 288 frames of 1/144 s add up to a hair under 2 s in floating
/// point, so the clock holds the 120th step for the next frame. After the
/// same number of steps the two are the same to a micrometre, and that is
/// where x = cos 2t puts bob, to within the step's own error.
#[test]
fn the_toys_spring_lands_its_body_in_one_place_at_30_and_at_144_hz() {
    let (at_30, marks_30) = spring_run(SPRING_SCENE, 1.0 / 30.0, 2.0, None);
    let (at_144, marks_144) = spring_run(SPRING_SCENE, 1.0 / 144.0, 2.0, None);
    let (at_60, _) = spring_run(SPRING_SCENE, 1.0 / 60.0, 2.0, None);
    // Each frame at 30 Hz ran two steps, and the toy heard which was which
    assert!(
        marks_30.chunks(2).all(|f| f == [(0, 2), (1, 2)]),
        "{marks_30:?}"
    );
    assert!(
        marks_144.iter().all(|&m| m == (0, 1)),
        "one step a frame at most: {marks_144:?}"
    );
    let end = |path: &[Sample]| *path.last().unwrap();
    let (e30, e60, e144) = (end(&at_30), end(&at_60), end(&at_144));
    eprintln!(
        "at 2 s: 30 Hz {} steps x {:.6}, 60 Hz {} steps x {:.6}, 144 Hz {} steps x {:.6}",
        e30.steps, e30.x, e60.steps, e60.x, e144.steps, e144.x
    );
    assert!((e30.time - e144.time).abs() < 1e-9, "the same game time");
    assert!(e30.steps.abs_diff(e144.steps) <= 1);
    assert!((e30.x - e144.x).abs() < 0.034, "{} vs {}", e30.x, e144.x);
    // Step for step, one motion (at 30 Hz a frame ends on an even step)
    let common = e30.steps.min(e60.steps).min(e144.steps) & !1;
    for n in [16, 30, 60, 90, common] {
        let (a, b, c) = (x_after(&at_30, n), x_after(&at_60, n), x_after(&at_144, n));
        assert!(
            (a - c).abs() < 1e-6 && (a - b).abs() < 1e-6,
            "step {n}: {a} {b} {c}"
        );
        let exact = (2.0 * n as f64 / 60.0).cos();
        assert!((a - exact).abs() < 0.05, "step {n}: {a} against {exact}");
    }
}

/// What the hook is for. A suspension is stiff: at 400 N/m on 1 kg (a
/// period of 0.31 s) the same spring pulled once a frame, after the frame's
/// steps, is a different motion at each frame rate, where the toy's pull
/// once a step is one motion at every rate. Compared step for step. It
/// prints both for the presentation.
#[test]
fn a_stiff_spring_once_a_frame_drifts_with_the_frame_rate_and_once_a_step_does_not() {
    let stiff = SPRING_SCENE.replace("spring = 4.0", "spring = 400.0");
    let rates = [30.0, 60.0, 144.0];
    let runs = |once_a_frame: Option<f32>| -> Vec<Vec<Sample>> {
        rates
            .iter()
            .map(|hz| spring_run(&stiff, 1.0 / hz, 1.0, once_a_frame).0)
            .collect()
    };
    let (step, frame) = (runs(None), runs(Some(400.0)));
    let marks = [6, 12, 18, 24, 30, 36, 42, 48, 54, 58];
    for (label, paths) in [("once a step", &step), ("once a frame", &frame)] {
        for (hz, path) in rates.iter().zip(paths.iter()) {
            let row: Vec<String> = marks
                .iter()
                .map(|&n| format!("{:+.4}", x_after(path, n)))
                .collect();
            eprintln!("{label:>12} {hz:>3} Hz: {}", row.join(" "));
        }
    }
    let spread = |paths: &[Vec<Sample>]| {
        marks
            .iter()
            .map(|&n| {
                let xs: Vec<f64> = paths.iter().map(|p| x_after(p, n)).collect();
                let hi = xs.iter().cloned().fold(f64::MIN, f64::max);
                let lo = xs.iter().cloned().fold(f64::MAX, f64::min);
                hi - lo
            })
            .fold(0.0, f64::max)
    };
    let (hooked, framed) = (spread(&step), spread(&frame));
    eprintln!(
        "widest spread across 30/60/144 Hz: once a step {hooked:.6} m, once a frame {framed:.4} m"
    );
    assert!(hooked < 1e-5, "{hooked}");
    assert!(framed > 0.1, "{framed}");
}

/// #1551's second box: a script calls the toy's `toy_steps()` where the
/// scene has a toy, and in a scene without one the call is the "not
/// enabled" error (#698's rule).
#[test]
fn a_script_reads_the_toys_steps_and_without_a_toy_the_call_is_not_enabled() {
    const READER: &str =
        "fn on_update() { set_field(self_entity(), \"probe\", \"steps\", toy_steps()); }\n";
    let scripted = |name: &str, toy: bool| {
        format!(
            "[scene]\nname = \"{name}\"\n{}\n[entities.reader]\n[entities.reader.probe]\n\
             steps = -1\n[entities.reader.script]\nsource = \"reader.rhai\"\n",
            if toy { "\n[toy]\n" } else { "" }
        )
    };
    let mut files = MemorySource::new();
    files.insert("toys/flint.toml", "");
    files.insert("toys/scenes/with.scene.toml", scripted("With", true));
    files.insert("toys/scenes/without.scene.toml", scripted("Without", false));
    files.insert("toys/scripts/reader.rhai", READER);
    vfs::scoped(Arc::new(files), || {
        let steps = |app: &PlayerApp| {
            let id = app.core.world.get_id("reader").expect("the reader");
            app.core
                .world
                .get_components(id)
                .and_then(|c| c.get_field("probe", "steps"))
                .and_then(|v| v.as_integer())
        };
        // The first tick is the clock's start (0 s), the second a quarter
        // second: 15 steps, published at its end for the third's on_update
        let mut app = app_in("toys/scenes/with.scene.toml");
        app.tick();
        app.tick();
        app.tick();
        assert_eq!(steps(&app), Some(15), "the steps up to the frame before");
        assert_eq!(app.core.script.errors().total(), 0);

        let mut app = app_in("toys/scenes/without.scene.toml");
        app.tick();
        assert_eq!(steps(&app), Some(-1), "the call was an error");
        let errors = format!("{:?}", app.core.script.errors().all());
        assert!(
            errors.contains("toy_steps: the toy module is not enabled here"),
            "{errors}"
        );
    });
}

/// #1551's third box: `flint playtest`'s run (`flint_player::playtest`,
/// which the command calls) on the spring scene passes. A test script
/// watches bob swing through the origin and passes once the toy has heard
/// two seconds of steps. The toy is test-only, so the `flint` binary can't
/// link it: this is the command's code with the toy in, until a toy of its
/// own is a package in `packages/` (#1560).
#[test]
fn a_playtest_of_the_spring_scene_passes() {
    const CHECKS: &str = "let lowest = 2.0;\n\
         fn on_update() {\n\
         \x20   let x = get_position(get_entity(\"bob\")).x;\n\
         \x20   test_assert(x <= 1.001 && x >= -1.001, \"the spring holds bob within a metre\");\n\
         \x20   if x < lowest { lowest = x; }\n\
         \x20   test_log(\"steps\", toy_steps());\n\
         \x20   if toy_steps() >= 120 {\n\
         \x20       test_assert(lowest < -0.6, \"bob swung through the origin\");\n\
         \x20       test_pass();\n\
         \x20   }\n\
         }\n";
    let mut files = MemorySource::new();
    files.insert("toys/flint.toml", "");
    files.insert("toys/scenes/spring.scene.toml", SPRING_SCENE);
    files.insert("toys/playtests/swing.rhai", CHECKS);
    let report = vfs::scoped(Arc::new(files), || {
        let mut options = crate::PlaytestOptions::new("toys/scenes/spring.scene.toml");
        options.test_scripts = vec!["toys/playtests/swing.rhai".into()];
        options.timeout = Some(5.0);
        crate::playtest(&options)
    });
    let json = serde_json::to_string_pretty(&report).unwrap();
    eprintln!("{json}");
    assert!(report.passed, "{json}");
    assert!(
        matches!(report.ended, crate::PlaytestEnded::TestPass),
        "{json}"
    );
    assert!(report.script_errors.is_empty(), "{json}");
    assert_eq!(report.log["steps"], 120, "{json}");
}

/// A capture runs no physics (ADR 0137 section 3a), so its toy hears no
/// step, and its module still reads: `toy_steps()` is 0, not an error.
#[test]
fn a_capture_runs_no_fixed_step_and_its_module_still_reads() {
    const READER: &str =
        "fn on_update() { set_field(self_entity(), \"probe\", \"steps\", toy_steps()); }\n";
    let scene = format!(
        "{SPRING_SCENE}\n[entities.reader]\n[entities.reader.probe]\nsteps = -1\n\
         [entities.reader.script]\nsource = \"reader.rhai\"\n"
    );
    let mut files = MemorySource::new();
    files.insert("toys/flint.toml", "");
    files.insert("toys/scenes/spring.scene.toml", scene);
    files.insert("toys/scripts/reader.rhai", READER);
    vfs::scoped(Arc::new(files), || {
        let mut camera = flint_render::Camera::new();
        let mut game = capture_of("toys/scenes/spring.scene.toml");
        for _ in 0..4 {
            game.step(0.25, &mut camera, None, None);
        }
        let toy = game.core.subsystems.find::<Toy>().expect("built");
        assert_eq!((toy.frames, toy.steps), (4, 0));
        let world = game.world();
        let id = world.get_id("reader").unwrap();
        let read = world
            .get_components(id)
            .and_then(|c| c.get_field("probe", "steps"))
            .and_then(|v| v.as_integer());
        assert_eq!(read, Some(0));
        assert_eq!(game.script_errors().total(), 0);
    });
}
