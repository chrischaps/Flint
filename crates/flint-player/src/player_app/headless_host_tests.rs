//! A headless capture is a script host like the player (#697): every
//! command is either applied here or on [`PRESENTATION_ONLY`], and the style
//! commands the old dispatcher dropped now land.

use super::*;
use flint_script::context::LogLevel;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The variants of `ScriptCommand`, read from where it is declared, so a new
/// one is seen here the day it is added.
fn declared_variants() -> BTreeSet<String> {
    let source = include_str!("../../../flint-script/src/context.rs");
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

/// A command's variant name, as its `Debug` starts.
fn variant(cmd: &ScriptCommand) -> String {
    format!("{cmd:?}")
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect()
}

/// One of each command, aimed at `entity`, a live one.
fn one_of_each(entity: EntityId) -> Vec<ScriptCommand> {
    use ScriptCommand as C;
    let s = |v: &str| v.to_string();
    let table = |k: &str, v: f64| {
        let mut t = toml::Table::new();
        t.insert(k.into(), v.into());
        toml::Value::Table(t)
    };
    vec![
        C::PlaySound {
            name: s("ding"),
            volume: 1.0,
            pitch: 1.0,
        },
        C::PlaySoundAt {
            name: s("ding"),
            position: (0.0, 0.0, 0.0),
            volume: 1.0,
            pitch: 1.0,
            distances: None,
            rolloff: None,
        },
        C::StopSound { name: s("ding") },
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
        C::ResetPostStyle {
            style: s("kuwahara"),
        },
        C::ResetStyleHistory,
        C::SetCameraCutDetection { enabled: false },
        C::SetLook {
            name: Some(s("graphite")),
        },
        C::SetWooze {
            knobs: table("amplitude", 0.5),
            seconds: 0.0,
        },
        C::SetWoozeZone {
            name: s("pond"),
            fields: table("amplitude", 0.5),
            seconds: 0.0,
        },
        C::RemoveWoozeZone {
            name: s("pond"),
            seconds: 0.0,
        },
        C::TuneLiving {
            knobs: toml::Value::Table(toml::Table::new()),
        },
        C::ClearLiving {
            species: s("oak"),
            keys: Vec::new(),
        },
        C::FreezeLiving { frozen: true },
        C::StepLiving,
        C::SaveLivingPreset { name: s("mine") },
        C::LoadLivingPreset { name: s("calm") },
        C::CommitLiving,
        C::CompareLiving {
            a: s("calm"),
            b: String::new(),
        },
        C::FlipLiving,
        C::SplitLiving { split: true },
        C::StopComparingLiving,
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
            post: PostProcessOverrides {
                vignette: Some(0.4),
                ..Default::default()
            },
        },
        C::SetFogColor {
            color: [0.5, 0.5, 0.6],
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

fn load(scene: &str, world: &mut FlintWorld) -> HeadlessScripts {
    let project = flint_core::project::Project::for_scene(std::path::Path::new(scene));
    HeadlessScripts::load(
        scene,
        &project,
        world,
        None,
        (320.0, 180.0),
        &PostProcessConfig::default(),
    )
}

#[test]
fn every_command_is_applied_headless_or_presentation_only() {
    let (dir, scene) = scene_folder("every_command", &[]);
    let mut world = FlintWorld::new();
    let entity = world.spawn("barrel").unwrap();
    let mut run = load(&scene, &mut world);

    // Each sample alone, so what it skipped is its own
    let mut skipped_by: BTreeMap<String, Vec<&'static str>> = BTreeMap::new();
    let samples = one_of_each(entity);
    for cmd in samples.iter().cloned() {
        let before = run.skipped();
        let name = variant(&cmd);
        run.apply(vec![cmd], &mut world, None, &mut None, None, false);
        let new: Vec<&'static str> = run
            .skipped()
            .into_iter()
            .filter(|c| !before.contains(c))
            .collect();
        skipped_by.insert(name, new);
    }
    let _ = std::fs::remove_dir_all(dir);

    // The samples are every variant, and only those
    let sampled: BTreeSet<String> = samples.iter().map(variant).collect();
    assert_eq!(sampled.len(), samples.len(), "one sample per variant");
    let declared = declared_variants();
    assert!(declared.len() >= 52, "the enum wasn't read: {declared:?}");
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

    let mut run = load(&scene, &mut world);
    let view = CameraView {
        position: [0.0, 0.0, 5.0],
        forward: [0.0, 0.0, -1.0],
        view_proj: [[0.0; 4]; 4],
    };
    let cell = |world: &FlintWorld| {
        let c = world.get_component(barrel, "ascii_style")?;
        let v = c.get("cell")?;
        v.as_float().or_else(|| v.as_integer().map(|i| i as f64))
    };
    let dt = 0.25;
    let mut t = 0.0;
    // The first tick carries on_init's commands: styled, at 8, easing
    let commands = run.tick(&mut world, t, 0.0, view);
    run.apply(commands, &mut world, None, &mut None, None, false);
    assert_eq!(
        cell(&world),
        Some(8.0),
        "set_entity_style and set_style_param land"
    );
    for expect in [10.0, 12.0, 14.0, 16.0] {
        t += dt;
        let commands = run.tick(&mut world, t, dt, view);
        run.apply(commands, &mut world, None, &mut None, None, false);
        assert_eq!(cell(&world), Some(expect), "at {t}");
    }
    // get_entity_style reads through the capture's style reader
    let seen = world
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
