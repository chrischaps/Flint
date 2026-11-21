//! A headless capture is a script host like the player (#697): every
//! command is either applied here or on [`PRESENTATION_ONLY`], and the style
//! commands the old dispatcher dropped now land.

use super::*;
use flint_core::EntityId;
use flint_script::context::LogLevel;
use flint_script::CameraOverrides;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The variants of `ScriptCommand`, read from where it is declared, so a new
/// one is seen here the day it is added.
fn declared_variants() -> BTreeSet<String> {
    // A Windows checkout with `core.autocrlf` has CRLF line endings.
    let source = include_str!("../../../flint-script/src/context.rs").replace("\r\n", "\n");
    let start = source
        .find("pub enum ScriptCommand {")
        .expect("ScriptCommand is declared in flint-script's context.rs");
    let body = &source[start..];
    let end = body.find("\n}\n").expect("the enum closes");
    body[..end]
        .lines()
        .skip(1)
        .filter_map(|line| {
            let rest = line.strip_prefix("    ")?;
            if !rest.chars().next()?.is_ascii_uppercase() {
                return None;
            }
            Some(
                rest.chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect(),
            )
        })
        .collect()
}

/// A command's variant name, as its `Debug` starts; a module's command
/// (#698) is `Module:` and its function.
fn variant(cmd: &ScriptCommand) -> String {
    if let ScriptCommand::Module(m) = cmd {
        return format!("Module:{}", m.function());
    }
    format!("{cmd:?}")
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect()
}

/// A living-subsystem module command, as `function` queues it.
fn module(
    function: &'static str,
    command: crate::player_app::living_api::LivingCommand,
) -> ScriptCommand {
    ScriptCommand::Module(flint_script::ModuleCommand::new(function, command))
}

/// One of each command, aimed at `entity`, a live one.
fn one_of_each(entity: EntityId) -> Vec<ScriptCommand> {
    use crate::player_app::living_api::LivingCommand as L;
    use ScriptCommand as C;
    let s = |v: &str| v.to_string();
    let table = |k: &str, v: f64| {
        let mut t = toml::Table::new();
        t.insert(k.into(), v.into());
        toml::Value::Table(t)
    };
    vec![
        C::PlaySound {
            handle: 1,
            name: s("ding"),
            play: flint_script::SoundPlay::default(),
        },
        C::PlaySoundAt {
            handle: 2,
            name: s("ding"),
            position: (0.0, 0.0, 0.0),
            play: flint_script::SoundPlay::default(),
            distances: None,
            rolloff: None,
        },
        C::StopSound {
            sound: flint_script::SoundRef::Name(s("ding")),
            fade: 0.0,
        },
        C::SetSoundVolume {
            handle: 1,
            volume: 0.5,
            fade: 0.0,
        },
        C::SetBusVolume {
            bus: s("voice"),
            volume: 0.5,
        },
        C::FireEvent {
            name: s("bell"),
            data: toml::Value::Boolean(true),
        },
        C::Log {
            level: LogLevel::Info,
            message: s("a line"),
        },
        C::EmitBurst { entity, count: 4 },
        C::PlayEffect {
            handle: 1,
            name: s("sparks"),
            position: (0.0, 0.0, 0.0),
        },
        C::StopEffect { handle: 1 },
        C::SetEffectParam {
            handle: 1,
            param: s("scale"),
            value: 2.0,
        },
        C::SetEntityStyle {
            entity,
            style: s("ascii"),
        },
        C::SetStyleParam {
            entity,
            param: s("cell"),
            value: 10.0.into(),
        },
        C::TweenStyleParam {
            entity,
            param: s("cell"),
            target: 20.0.into(),
            seconds: 1.0,
            easing: s("linear"),
        },
        C::Tween {
            entity,
            path: s("light.intensity"),
            target: 2.0.into(),
            seconds: 1.0,
            easing: flint_core::tween::Easing::Linear,
        },
        C::ClearEntityStyle { entity },
        C::SetPostStyle {
            style: s("kuwahara"),
            enabled: true,
        },
        C::SetPostStyleParam {
            style: s("kuwahara"),
            param: s("blend"),
            value: 0.5.into(),
        },
        C::TweenPostStyleParam {
            style: s("kuwahara"),
            param: s("reveal"),
            target: 1.0,
            seconds: 1.0,
            easing: s("linear"),
        },
        C::ResetPostStyle {
            style: s("kuwahara"),
        },
        C::ResetStyleHistory,
        C::SetCameraCutDetection { enabled: false },
        C::SetPanels {
            layout: "[panel.a]\nrect = [0, 0, 1, 1]\n".parse().unwrap(),
        },
        C::SetPanel {
            name: s("b"),
            keys: "rect = [0.5, 0, 0.5, 1]\n".parse().unwrap(),
        },
        C::RemovePanel { name: s("b") },
        C::ClearPanels,
        C::Rumble {
            pad: -1,
            strength: 0.5,
            seconds: 0.2,
        },
        C::SetLook {
            name: Some(s("graphite")),
            seconds: 2.0,
        },
        C::PrepareLook {
            name: s("ascii_noir"),
        },
        // The living subsystem's module commands (#698), one a function
        module(
            "set_wooze",
            L::SetWooze {
                knobs: table("amplitude", 0.5),
                seconds: 0.0,
            },
        ),
        module(
            "set_wooze_zone",
            L::SetWoozeZone {
                name: s("pond"),
                fields: table("amplitude", 0.5),
                seconds: 0.0,
            },
        ),
        module(
            "remove_wooze_zone",
            L::RemoveWoozeZone {
                name: s("pond"),
                seconds: 0.0,
            },
        ),
        module(
            "tune_living",
            L::Tune {
                knobs: toml::Value::Table(toml::Table::new()),
                seconds: 0.0,
            },
        ),
        module(
            "clear_living",
            L::Clear {
                species: s("oak"),
                keys: Vec::new(),
            },
        ),
        module("freeze_living", L::Freeze { frozen: true }),
        module("step_living", L::Step),
        module("save_living_preset", L::SavePreset { name: s("mine") }),
        module("load_living_preset", L::LoadPreset { name: s("calm") }),
        module("commit_living", L::Commit),
        module(
            "compare_living",
            L::Compare {
                a: s("calm"),
                b: String::new(),
            },
        ),
        module("flip_living", L::Flip),
        module("split_living", L::Split { split: true }),
        module("stop_comparing_living", L::StopComparing),
        C::LoadScene {
            path: s("levels/next.scene.toml"),
        },
        C::ReloadScene,
        C::QuitGame,
        C::PushState { name: s("paused") },
        C::PopState,
        C::ReplaceState { name: s("paused") },
        C::SetVelocity2D {
            entity,
            vx: 1.0,
            vy: 0.0,
        },
        C::LoadChunk {
            path: s("chunks/a.scene.toml"),
            offset_x: 0.0,
            offset_y: 0.0,
            chunk_id: s("a"),
        },
        C::UnloadChunk { chunk_id: s("a") },
        C::SetCamera {
            camera: CameraOverrides {
                fov: Some(50.0),
                ..Default::default()
            },
        },
        C::SetPostProcess {
            post: {
                let mut post = PostProcessOverrides::default();
                post.set("vignette_intensity", &0.4.into()).unwrap();
                post.set(
                    "fog_color",
                    &toml::Value::try_from([0.5, 0.5, 0.6]).unwrap(),
                )
                .unwrap();
                post
            },
        },
        C::SetRenderMode {
            mode: Some((1, 1.0)),
            params: None,
        },
        C::SetTimeScale { scale: 0.5 },
        C::SetAudioLowpass { cutoff_hz: 800.0 },
        C::SetTouchControls { enabled: false },
        C::SetLodScales {
            lod_scale: Some(0.5),
            draw_distance_scale: None,
        },
        C::SetCursorCaptured { captured: true },
        C::SetQuality {
            settings: QualitySettings::default(),
        },
    ]
}

/// A folder of this test's own, with a scene in it, for a capture to load.
fn scene_folder(test: &str, scripts: &[(&str, &str)]) -> (PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("flint_697_{test}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    for (name, source) in scripts {
        std::fs::write(dir.join("scripts").join(name), source).unwrap();
    }
    let scene = dir.join("stage.scene.toml");
    std::fs::write(&scene, "[scene]\nname = \"stage\"\n").unwrap();
    (dir, scene.to_string_lossy().to_string())
}

fn load(scene: &str, world: FlintWorld) -> HeadlessGame {
    let project = flint_core::project::Project::for_scene(std::path::Path::new(scene));
    let mut game = HeadlessGame::new(world, scene, project);
    game.start_scripts(true, None, (320.0, 180.0), &PostProcessConfig::default());
    game
}

#[test]
fn every_command_is_applied_headless_or_presentation_only() {
    let (dir, scene) = scene_folder("every_command", &[]);
    let mut world = FlintWorld::new();
    let entity = world.spawn("barrel").unwrap();
    let mut run = load(&scene, world);

    // Each sample alone, so what it skipped is its own
    let mut skipped_by: BTreeMap<String, Vec<&'static str>> = BTreeMap::new();
    let samples = one_of_each(entity);
    for cmd in samples.iter().cloned() {
        let before = run.skipped();
        let name = variant(&cmd);
        run.dispatch_for_test(vec![cmd]);
        let new: Vec<&'static str> = run
            .skipped()
            .into_iter()
            .filter(|c| !before.contains(c))
            .collect();
        skipped_by.insert(name, new);
    }
    let _ = std::fs::remove_dir_all(dir);

    // The samples are every variant, and only those (a module's commands
    // are one variant, sampled once a function)
    let sampled: BTreeSet<String> = samples.iter().map(variant).collect();
    assert_eq!(sampled.len(), samples.len(), "one sample per variant");
    let sampled: BTreeSet<String> = sampled
        .iter()
        .map(|v| v.split(':').next().unwrap().to_string())
        .collect();
    let declared = declared_variants();
    assert!(declared.len() >= 39, "the enum wasn't read: {declared:?}");
    let unsampled: Vec<_> = declared.difference(&sampled).collect();
    assert!(
        unsampled.is_empty(),
        "ScriptCommand has variants with no sample in one_of_each: {unsampled:?}. \
         Add one, then make the headless host do it or put its command on \
         PRESENTATION_ONLY"
    );
    let unknown: Vec<_> = sampled.difference(&declared).collect();
    assert!(unknown.is_empty(), "samples of no variant: {unknown:?}");

    // What a capture skips is on the list, and nothing else is on it
    let listed: BTreeSet<&str> = PRESENTATION_ONLY.iter().map(|s| s.command).collect();
    let mut seen = BTreeSet::new();
    for (variant, skipped) in &skipped_by {
        for command in skipped {
            assert!(
                listed.contains(command),
                "{variant} ({command}) isn't done headless and isn't on PRESENTATION_ONLY"
            );
            seen.insert(*command);
        }
    }
    let stale: Vec<_> = listed.difference(&seen).collect();
    assert!(
        stale.is_empty(),
        "on PRESENTATION_ONLY but done headless (take them off): {stale:?}"
    );
    // The style commands the old catch-all dropped are done
    for name in [
        "SetEntityStyle",
        "SetStyleParam",
        "TweenStyleParam",
        "Tween",
        "ClearEntityStyle",
        "SetPostStyle",
        "SetLook",
    ] {
        assert!(
            skipped_by[name].is_empty(),
            "{name}: {:?}",
            skipped_by[name]
        );
    }
}

/// A script that styles the barrel in ASCII and eases its cells from 8 to
/// 16 over a second: what `flint render --scripts --time` used to drop.
const STYLER: &str = r#"
fn on_init() {
    let barrel = get_entity("barrel");
    set_entity_style(barrel, "ascii");
    set_style_param(barrel, "cell", 8);
    tween_style_param(barrel, "cell", 16, 1.0, "linear");
}

fn on_update() {
    let me = self_entity();
    set_field(me, "seen", "style", get_entity_style(get_entity("barrel")));
}
"#;

#[test]
#[allow(clippy::disallowed_methods)] // a test asserts on the TOML exactly as written
fn a_capture_styles_tweens_and_reads_as_the_player_does() {
    let (dir, scene) = scene_folder("styles", &[("styler.rhai", STYLER)]);
    let mut world = FlintWorld::new();
    let barrel = world.spawn("barrel").unwrap();
    let director = world.spawn("director").unwrap();
    let mut script = toml::Table::new();
    script.insert("source".into(), "styler.rhai".into());
    world
        .set_component(director, "script", script.into())
        .unwrap();
    let mut seen = toml::Table::new();
    seen.insert("style".into(), "".into());
    world.set_component(director, "seen", seen.into()).unwrap();

    let mut run = load(&scene, world);
    let mut camera = flint_render::Camera::new();
    let cell = |world: &FlintWorld| {
        let c = world.get_component(barrel, "ascii_style")?;
        let v = c.get("cell")?;
        v.as_float().or_else(|| v.as_integer().map(|i| i as f64))
    };
    let dt = 0.25;
    let mut t = 0.0;
    // The first step carries on_init's commands: styled, at 8, easing
    run.step(0.0, &mut camera, None, None);
    assert_eq!(
        cell(run.world()),
        Some(8.0),
        "set_entity_style and set_style_param land"
    );
    for expect in [10.0, 12.0, 14.0, 16.0] {
        t += dt;
        run.step(dt, &mut camera, None, None);
        assert_eq!(cell(run.world()), Some(expect), "at {t}");
    }
    // get_entity_style reads through the capture's style reader
    let seen = run
        .world()
        .get_component(director, "seen")
        .and_then(|c| c.get("style").cloned());
    let _ = std::fs::remove_dir_all(dir);
    let seen = seen.expect("the script wrote what it read");
    let style = seen
        .as_table()
        .and_then(|t| t.get("style"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| seen.as_str().map(str::to_string));
    assert_eq!(style.as_deref(), Some("ascii"), "{seen:?}");
    assert!(run.skipped().is_empty(), "{:?}", run.skipped());
    assert!(run.notes().is_empty(), "{:?}", run.notes());
}

/// A script that eases a lamp's light from 2 to 10 over a second and writes
/// down the `tween_done` it hears (#734).
const LAMPLIGHTER: &str = r#"
fn on_init() {
    tween(get_entity("lamp"), "light.intensity", 10, 1.0, "linear");
}

fn on_event(name, data) {
    if name == "tween_done" {
        set_field(self_entity(), "seen", "done", data.path);
    }
}
"#;

#[test]
#[allow(clippy::disallowed_methods)] // a test asserts on the TOML exactly as written
fn a_capture_tweens_a_component_field_and_says_when_it_arrives() {
    let (dir, scene) = scene_folder("field_tween", &[("lamplighter.rhai", LAMPLIGHTER)]);
    let mut world = FlintWorld::new();
    let lamp = world.spawn("lamp").unwrap();
    let mut light = toml::Table::new();
    light.insert("type".into(), "point".into());
    light.insert("intensity".into(), 2.into());
    world.set_component(lamp, "light", light.into()).unwrap();
    let director = world.spawn("director").unwrap();
    let mut script = toml::Table::new();
    script.insert("source".into(), "lamplighter.rhai".into());
    world
        .set_component(director, "script", script.into())
        .unwrap();
    let mut seen = toml::Table::new();
    seen.insert("done".into(), "".into());
    world.set_component(director, "seen", seen.into()).unwrap();

    let mut run = load(&scene, world);
    let mut camera = flint_render::Camera::new();
    let intensity =
        |world: &FlintWorld| world.get_component(lamp, "light").unwrap()["intensity"].clone();
    let done = |world: &FlintWorld| world.get_component(director, "seen").unwrap()["done"].clone();
    // The first step carries on_init's command: the tween starts, unmoved
    run.step(0.0, &mut camera, None, None);
    assert_eq!(intensity(run.world()), toml::Value::Integer(2));
    for expect in [4.0, 6.0, 8.0] {
        run.step(0.25, &mut camera, None, None);
        assert_eq!(intensity(run.world()), toml::Value::Float(expect));
    }
    run.step(0.25, &mut camera, None, None);
    assert_eq!(
        intensity(run.world()),
        toml::Value::Integer(10),
        "lands whole"
    );
    assert_eq!(done(run.world()).as_str(), Some(""), "heard next frame");
    run.step(0.25, &mut camera, None, None);
    let _ = std::fs::remove_dir_all(dir);
    assert_eq!(done(run.world()).as_str(), Some("light.intensity"));
    assert!(run.script_errors().is_empty(), "{:?}", run.script_errors());
}

/// What `run` wrote to the log, every level (#1013).
fn logged(run: impl FnOnce()) -> String {
    use std::sync::{Arc, Mutex};
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let log = Captured::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, run);
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    text
}

/// A script's `log`, `log_warn` and `log_error` each print once in a
/// capture, as in the player (#1013): asking a command's name used to apply
/// it to a probe host, whose default `log` wrote the line a second time.
#[test]
fn a_script_log_line_is_written_once_in_a_capture() {
    let (dir, scene) = scene_folder("log_once", &[]);
    let mut run = load(&scene, FlintWorld::new());
    let said = [
        (LogLevel::Info, "said once at info"),
        (LogLevel::Warn, "said once at warn"),
        (LogLevel::Error, "said once at error"),
    ];
    let text = logged(|| {
        run.dispatch_for_test(
            said.iter()
                .map(|(level, message)| ScriptCommand::Log {
                    level: level.clone(),
                    message: (*message).into(),
                })
                .collect(),
        );
    });
    let _ = std::fs::remove_dir_all(dir);
    for (_, message) in said {
        assert_eq!(
            text.matches(message).count(),
            1,
            "{message:?} should be one line of the log:\n{text}"
        );
    }
}

/// The name a capture's refusal looks a command up by is the name its
/// `ScriptHost` default reports (`host::command_name`, #1013), for every
/// variant: a table, so that asking carries nothing out.
#[test]
fn the_name_table_is_what_each_default_reports() {
    /// Records what a default calls unsupported, and says nothing for
    /// `log`, the one default that does something.
    #[derive(Default)]
    struct Probe(Option<&'static str>);
    impl ScriptHost for Probe {
        fn unsupported(&mut self, command: &'static str) {
            self.0 = Some(command);
        }
        fn log(&mut self, _level: LogLevel, _message: String) {}
    }
    let mut world = FlintWorld::new();
    let entity = world.spawn("barrel").unwrap();
    let mut mismatched = Vec::new();
    for cmd in one_of_each(entity) {
        let mut probe = Probe::default();
        flint_script::host::apply(&mut probe, cmd.clone());
        let table = flint_script::host::command_name(&cmd);
        if table != probe.0 {
            mismatched.push(format!(
                "{}: the table says {table:?}, the default {:?}",
                variant(&cmd),
                probe.0
            ));
        }
    }
    assert!(mismatched.is_empty(), "{mismatched:#?}");
}
