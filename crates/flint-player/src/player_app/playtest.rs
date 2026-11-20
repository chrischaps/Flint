//! `flint playtest` (#714): the player's own frames with no window, no GPU
//! and no sound, stepped on a fixed clock until the run says how it went.
//!
//! It is the player, not a capture: a [`PlayerApp`] brought up as
//! `flint-player` brings one up (the scene's schemas through the one
//! resolver, its settings, `enter_scene`), then ticked frame by frame, so
//! physics, the character controller, scripts, animation, particles,
//! transitions and the input script all run as they do in play. What it
//! leaves out is presentation: the window, the renderer and the sound
//! device. A model is not loaded (nothing draws it); a sound is (a missing
//! file still warns). Saves stay in memory: a playtest starts from no save
//! file and writes none. Hot reload is off, and `random()` is seeded.
//!
//! The run ends at the first of: a script's `test_pass()` or `test_fail()`,
//! `quit_game()`, a failed scene transition, `exit_after` (the end of a
//! run that passes if nothing failed) or `timeout` (the end of a run that
//! was meant to end itself first, a failure). It fails on a failed
//! assertion, `test_fail`, an input-script `expect` or `wait` that didn't
//! hold, any script error, a failed transition, a timeout, and under
//! `strict` any warning while the scene loaded. The [`PlaytestReport`] is
//! the same bytes for the same run: everything in it is on game time, and
//! the wall clock only with `timings`.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context as _;
use serde::Serialize;
use serde_json::Value;

use super::input_checks::CheckOutcome;
use super::playtest_warnings::{self as warnings, Phase, Warning};
use super::scene_entry::SceneEntry;
use super::PlayerApp;

/// The step when none is given: the player's fixed physics step.
pub const DEFAULT_STEP: f64 = 1.0 / 60.0;
/// How long a run lasts when it says neither `exit_after` nor `timeout`.
pub const DEFAULT_EXIT_AFTER: f64 = 10.0;

/// What to play and how.
#[derive(Debug, Clone)]
pub struct PlaytestOptions {
    /// The scene, as a path.
    pub scene: String,
    /// `--schemas` (the whole list); empty: the scene's project's.
    pub schemas: Vec<String>,
    /// An input script's path.
    pub input_script: Option<String>,
    /// Game seconds a frame.
    pub frame_step: f64,
    /// `random()`'s seed; `None` is the player's stepped-run seed.
    pub seed: Option<u64>,
    /// End here, passing if nothing failed.
    pub exit_after: Option<f64>,
    /// End here as a failure: the run should have ended itself by then
    /// (`test_pass()`, `quit_game()`).
    pub timeout: Option<f64>,
    /// A warning while the scene loads fails the run.
    pub strict: bool,
    /// Entities whose components the report holds at the end, by name.
    pub dump: Vec<String>,
    /// Put wall-clock times in the report (then no two runs are alike).
    pub timings: bool,
    /// The screen scripts see, in points.
    pub screen: (f32, f32),
}

impl PlaytestOptions {
    pub fn new(scene: impl Into<String>) -> Self {
        Self {
            scene: scene.into(),
            schemas: Vec::new(),
            input_script: None,
            frame_step: DEFAULT_STEP,
            seed: None,
            exit_after: None,
            timeout: None,
            strict: false,
            dump: Vec::new(),
            timings: false,
            screen: (1280.0, 720.0),
        }
    }
}

/// How the run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Ended {
    /// It reached `exit_after`.
    ExitAfter,
    /// A script called `test_pass()`.
    TestPass,
    /// A script called `test_fail()`.
    TestFail,
    /// A script called `quit_game()`.
    QuitGame,
    /// It reached `timeout` without ending itself.
    Timeout,
    /// A scene transition failed.
    SceneFailed,
    /// The scene didn't start.
    StartFailed,
}

/// One reason the run failed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Failure {
    /// `assertion`, `test_fail`, `check`, `script_error`, `scene`,
    /// `timeout`, `start` or `load_warning`; `preflight` for what `flint
    /// playtest --strict` adds from `flint check`.
    pub kind: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// The game time it happened at, when that's known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<f64>,
}

impl Failure {
    /// A failure a host adds of its own (`flint playtest --strict`'s
    /// pre-flight findings).
    pub fn new(
        kind: &'static str,
        message: impl Into<String>,
        file: Option<String>,
        line: Option<usize>,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            file,
            line,
            at: None,
        }
    }
}

/// A script's `test_pass()` or `test_fail()`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VerdictReport {
    pub passed: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
    pub file: String,
    pub entity: String,
    pub line: Option<usize>,
    pub at: f64,
}

/// One `test_assert` place.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AssertionReport {
    pub file: String,
    pub entity: String,
    pub line: Option<usize>,
    pub message: String,
    pub passed: u64,
    pub failed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_failed_at: Option<f64>,
}

/// One script error, as `flint render --report json` lists them.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScriptErrorReport {
    pub file: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub entity: String,
    pub callback: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    pub message: String,
    pub count: u64,
    pub first_frame: u64,
}

/// The engine's warnings, if they were counted.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WarningsReport {
    /// False when the host installed no
    /// [`warning_layer`](super::playtest_warnings::warning_layer): nothing
    /// was counted.
    pub counted: bool,
    pub at_load: u64,
    pub while_running: u64,
    pub messages: Vec<Warning>,
}

/// The run's figures, on game time; the wall clock only with `timings`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PerfReport {
    pub frames: u64,
    pub frame_step: f64,
    pub game_time: f64,
    /// Fixed physics steps that ran.
    pub fixed_steps: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wall_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wall_ms_per_frame: Option<f64>,
}

/// Everything a playtest found, as `flint playtest` writes it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlaytestReport {
    pub scene: String,
    pub passed: bool,
    pub ended: Ended,
    pub game_time: f64,
    pub frames: u64,
    /// Every reason it failed, in the order of the lists below.
    pub failures: Vec<Failure>,
    pub verdict: Option<VerdictReport>,
    pub assertions: Vec<AssertionReport>,
    /// The input script's `expect` and `wait` lines.
    pub checks: Vec<CheckOutcome>,
    pub script_errors: Vec<ScriptErrorReport>,
    pub warnings: WarningsReport,
    /// What scripts gave `test_log`, each key's last value.
    pub log: BTreeMap<String, Value>,
    /// The `persist_*` store at the end.
    pub persist: BTreeMap<String, Value>,
    /// The entities asked for by name, their components at the end; an
    /// entity that isn't there is `null`.
    pub entities: BTreeMap<String, Value>,
    pub perf: PerfReport,
}

impl PlaytestReport {
    /// One line a failure, for a terminal.
    pub fn failure_lines(&self) -> Vec<String> {
        self.failures
            .iter()
            .map(|f| {
                let place = match (&f.file, f.line) {
                    (Some(file), Some(line)) => format!("{file}:{line}: "),
                    (Some(file), None) => format!("{file}: "),
                    _ => String::new(),
                };
                let at = f.at.map(|t| format!(" (t={t:.3} s)")).unwrap_or_default();
                format!("{}: {place}{}{at}", f.kind, f.message)
            })
            .collect()
    }
}

/// The report of a run that didn't start.
fn not_started(options: &PlaytestOptions, why: String) -> PlaytestReport {
    let messages = warnings::end();
    PlaytestReport {
        scene: options.scene.clone(),
        passed: false,
        ended: Ended::StartFailed,
        game_time: 0.0,
        frames: 0,
        failures: vec![Failure {
            kind: "start",
            message: why,
            file: None,
            line: None,
            at: None,
        }],
        verdict: None,
        assertions: Vec::new(),
        checks: Vec::new(),
        script_errors: Vec::new(),
        warnings: warnings_report(messages),
        log: BTreeMap::new(),
        persist: BTreeMap::new(),
        entities: BTreeMap::new(),
        perf: PerfReport {
            frames: 0,
            frame_step: options.frame_step,
            game_time: 0.0,
            fixed_steps: 0,
            wall_ms: None,
            wall_ms_per_frame: None,
        },
    }
}

fn warnings_report(messages: Vec<Warning>) -> WarningsReport {
    WarningsReport {
        counted: warnings::counting(),
        at_load: messages.iter().map(|w| w.at_load).sum(),
        while_running: messages.iter().map(|w| w.while_running).sum(),
        messages,
    }
}

/// Bring the scene up as `flint-player` would, with no window or sound.
fn start(options: &PlaytestOptions) -> anyhow::Result<PlayerApp> {
    if options.frame_step.is_nan() || options.frame_step <= 0.0 {
        anyhow::bail!("--frame-step must be over 0 (got {})", options.frame_step);
    }
    let scene = options.scene.clone();
    let scene_path = Path::new(&scene);
    let (registry, schema_dirs) =
        match flint_schema::SchemaRegistry::resolve(Some(scene_path), &options.schemas) {
            Ok(loaded) => loaded,
            Err(flint_core::FlintError::SchemaDirs(e)) => return Err(e.into()),
            Err(e) => return Err(e).context("Failed to load schemas"),
        };
    let input_script = options
        .input_script
        .as_deref()
        .map(crate::InputScript::load)
        .transpose()?;
    #[cfg_attr(not(feature = "living"), allow(unused_mut))]
    let (world, mut scene_file) = flint_scene::load_scene(&scene, &registry)
        .with_context(|| flint_scene::load_failed(&scene))?;
    #[cfg(feature = "living")]
    flint_procgen::settle_scene(&mut scene_file, scene_path)
        .with_context(|| flint_scene::load_failed(scene_path))?;

    let mut app = PlayerApp::with_audio(
        world,
        scene,
        false,
        None,
        scene_file.scene.input_config.clone(),
        flint_audio::AudioSystem::silent(),
    );
    let launch = &mut app.launch;
    launch.saves_in_memory = true;
    launch.headless_screen = Some(options.screen);
    launch.window_size = (options.screen.0 as u32, options.screen.1 as u32);
    // An unattended run writes nothing beside the game
    launch.procgen_disk_cache = false;
    launch.scene_file_writable = false;
    app.apply_scene_settings(&scene_file);
    app.capture.input_script = input_script;
    app.step_frames(Some(options.frame_step), options.seed);
    app.set_schemas(registry, schema_dirs);
    // Every named event, for the input script's `event` lines
    app.core.event_log = Some(Vec::new());
    app.enter_scene(SceneEntry::Start)
        .context("the scene did not start")?;
    Ok(app)
}

/// Play `options.scene` headless until it ends, and say how it went.
/// A run that can't start (a frame step of 0, a scene that doesn't load)
/// is a report too, `ended: start_failed`.
pub fn run(options: &PlaytestOptions) -> PlaytestReport {
    // One playtest at a time counts warnings (tests run several)
    let _one = warnings::exclusive();
    // A script's file changing mid-run is not part of the run
    flint_core::file_watch::disable();
    warnings::begin(Phase::Load);
    let wall = web_time::Instant::now();
    let mut app = match start(options) {
        Ok(app) => app,
        Err(e) => return not_started(options, format!("{e:#}")),
    };
    warnings::set_phase(Phase::Run);

    let exit_after = match (options.exit_after, options.timeout) {
        (None, None) => Some(DEFAULT_EXIT_AFTER),
        (e, _) => e,
    };
    let mut frames = 0u64;
    let mut fixed_steps = 0u64;
    let ended = loop {
        let now = app.core.clock.total_time;
        if let Some(t) = exit_after {
            if now + 1e-9 >= t {
                break Ended::ExitAfter;
            }
        }
        if let Some(t) = options.timeout {
            if now + 1e-9 >= t {
                break Ended::Timeout;
            }
        }
        app.tick();
        frames += 1;
        fixed_steps += app.core.fixed_steps as u64;
        if app.core.script.test_decided() {
            let passed = app
                .core
                .script
                .test_record()
                .verdict
                .is_some_and(|v| v.passed);
            break if passed {
                Ended::TestPass
            } else {
                Ended::TestFail
            };
        }
        if app.outcome.scene_failure.is_some() {
            break Ended::SceneFailed;
        }
        if app.outcome.exit_requested {
            break Ended::QuitGame;
        }
    };
    let now = app.core.clock.total_time;
    if let Some(script) = &mut app.capture.input_script {
        script.finish(now);
    }
    let wall_ms = wall.elapsed().as_secs_f64() * 1000.0;
    let messages = warnings::end();
    report(&app, options, ended, frames, fixed_steps, messages, wall_ms)
}

fn report(
    app: &PlayerApp,
    options: &PlaytestOptions,
    ended: Ended,
    frames: u64,
    fixed_steps: u64,
    messages: Vec<Warning>,
    wall_ms: f64,
) -> PlaytestReport {
    let now = app.core.clock.total_time;
    let record = app.core.script.test_record();
    let mut failures = Vec::new();

    for a in record.failed_assertions() {
        failures.push(Failure {
            kind: "assertion",
            message: format!(
                "{} (failed {} of {} times)",
                a.message,
                a.failed,
                a.failed + a.passed
            ),
            file: Some(a.file.clone()),
            line: a.line,
            at: a.first_failed_at,
        });
    }
    if let Some(v) = record.verdict.as_ref().filter(|v| !v.passed) {
        failures.push(Failure {
            kind: "test_fail",
            message: v.message.clone(),
            file: Some(v.file.clone()),
            line: v.line,
            at: Some(v.at),
        });
    }
    let checks: Vec<CheckOutcome> = app
        .capture
        .input_script
        .as_ref()
        .map(|s| s.outcomes().to_vec())
        .unwrap_or_default();
    for c in checks.iter().filter(|c| !c.passed) {
        failures.push(Failure {
            kind: "check",
            message: super::input_checks::describe(c),
            file: options.input_script.clone(),
            line: Some(c.line),
            at: Some(c.at),
        });
    }
    let errors = app.core.script.errors();
    let script_errors: Vec<ScriptErrorReport> = errors
        .all()
        .iter()
        .map(|e| ScriptErrorReport {
            file: e.file.clone(),
            line: e.line,
            column: e.column,
            entity: e.entity_name.clone(),
            callback: e.callback.clone(),
            function: e.function.clone(),
            message: e.message.clone(),
            count: e.count,
            first_frame: e.first_frame,
        })
        .collect();
    for e in &script_errors {
        failures.push(Failure {
            kind: "script_error",
            message: format!("{} in {}: {}", e.entity, e.callback, e.message),
            file: Some(e.file.clone()),
            line: e.line,
            at: None,
        });
    }
    if let Some(why) = &app.outcome.scene_failure {
        failures.push(Failure {
            kind: "scene",
            message: why.clone(),
            file: None,
            line: None,
            at: Some(now),
        });
    }
    if ended == Ended::Timeout {
        failures.push(Failure {
            kind: "timeout",
            message: format!(
                "the run did not end itself (test_pass(), test_fail() or quit_game()) by {} s",
                options.timeout.unwrap_or(now)
            ),
            file: None,
            line: None,
            at: Some(now),
        });
    }
    if options.strict {
        for w in messages.iter().filter(|w| w.at_load > 0) {
            failures.push(Failure {
                kind: "load_warning",
                message: w.message.clone(),
                file: None,
                line: None,
                at: None,
            });
        }
    }

    let entities = options
        .dump
        .iter()
        .map(|name| {
            let value = app
                .core
                .world
                .get_id(name)
                .and_then(|id| app.core.world.get_components(id))
                .map(|c| {
                    let table: BTreeMap<&str, &toml::Value> = c
                        .component_names()
                        .into_iter()
                        .filter_map(|n| c.get(n).map(|v| (n, v)))
                        .collect();
                    serde_json::to_value(table).unwrap_or(Value::Null)
                })
                .unwrap_or(Value::Null);
            (name.clone(), value)
        })
        .collect();
    let persist = app
        .core
        .store
        .keys()
        .into_iter()
        .filter_map(|k| {
            let v = app.core.store.get(k)?;
            Some((
                k.to_string(),
                serde_json::to_value(v).unwrap_or(Value::Null),
            ))
        })
        .collect();

    PlaytestReport {
        scene: options.scene.clone(),
        passed: failures.is_empty(),
        ended,
        game_time: now,
        frames,
        failures,
        verdict: record.verdict.as_ref().map(|v| VerdictReport {
            passed: v.passed,
            message: v.message.clone(),
            file: v.file.clone(),
            entity: v.entity.clone(),
            line: v.line,
            at: v.at,
        }),
        assertions: record
            .assertions
            .iter()
            .map(|a| AssertionReport {
                file: a.file.clone(),
                entity: a.entity.clone(),
                line: a.line,
                message: a.message.clone(),
                passed: a.passed,
                failed: a.failed,
                first_failed_at: a.first_failed_at,
            })
            .collect(),
        checks,
        script_errors,
        warnings: warnings_report(messages),
        log: record.logged.clone(),
        persist,
        entities,
        perf: PerfReport {
            frames,
            frame_step: options.frame_step,
            game_time: now,
            fixed_steps,
            wall_ms: options.timings.then_some(wall_ms),
            wall_ms_per_frame: options.timings.then(|| wall_ms / (frames.max(1) as f64)),
        },
    }
}
