//! The knobs through the player, headless: a scene's props with no window, moved
//! the way the panel and scripts move them.

use super::*;
use flint_core::wooze::warp;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_procgen::{LivingPropRequest, ProcGenSpec};

const SPEC: &str = r#"
generator = "tree_v1"
[meta]
name = "sapling"
version = "1.0.0"
[seed]
mode = "fixed"
value = 1
[params]
branch_method = "lsystem"
lsystem_iterations = 2
leaf_density = 0.0
[living]
lineage_seed = 7
pose_ring = 4
echoes = { count = 2 }
"#;

/// A player with `n` built props and no renderer, in its own folder.
fn app_with(n: u64, folder: &Path) -> PlayerApp {
    let scene_path = folder.join("grove.scene.toml");
    let mut app = PlayerApp::new(
        FlintWorld::new(),
        scene_path.to_string_lossy().into_owned(),
        false,
        None,
        None,
    );
    app.procgen_resolver
        .index_spec(ProcGenSpec::parse(SPEC).unwrap());
    for i in 0..n {
        app.procgen_resolver
            .add_living_prop(LivingPropRequest {
                entity: EntityId::from_raw(i + 1),
                entity_name: format!("oak_{i}"),
                position: [i as f32, 0.0, 0.0],
                spec_name: "sapling".to_string(),
                lineage_seed: None,
                pose_ring: None,
                phase_offset: Some(0.25),
                stagger: None,
            })
            .unwrap();
    }
    app.procgen_resolver.make_living_now();
    app.wooze = Some(WoozeField::new(WoozeSample::NEUTRAL));
    app
}

/// A folder of its own, gone when dropped.
struct Folder(PathBuf);

impl Folder {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("flint_living_tuning_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn beats_of(app: &PlayerApp) -> Vec<f64> {
    let props = app.procgen_resolver.living_props();
    props.props().iter().map(|p| p.beats()).collect()
}

fn poses_at(app: &PlayerApp) -> Vec<(u64, f32)> {
    let props = app.procgen_resolver.living_props();
    props
        .props()
        .iter()
        .map(|p| warp(p.beats(), p.clock().glide))
        .collect()
}

#[test]
fn freeze_holds_and_a_step_is_exactly_one_beat_of_the_warped_clock() {
    let folder = Folder::new("step");
    let mut app = app_with(3, &folder.0);
    app.tick_procgen(0.1);
    let before = beats_of(&app);
    let poses = poses_at(&app);
    let (time, field_time) = (app.procgen_resolver.living_props().time(), app.wooze_time);

    // Frozen is what the frame loop does with its dt: nothing moves.
    for _ in 0..5 {
        app.tick_procgen(0.0);
    }
    assert_eq!(beats_of(&app), before);
    assert_eq!(poses_at(&app), poses);

    app.step_living();
    app.tick_procgen(0.0);
    let after = beats_of(&app);
    for (a, b) in after.iter().zip(&before) {
        assert_eq!(a - b, 1.0, "one whole beat, to the bit");
    }
    for ((pose, weight), (was, was_weight)) in poses_at(&app).iter().zip(&poses) {
        assert_eq!(*pose, was + 1, "the next pose");
        assert_eq!(weight, was_weight, "at the same place in its step");
    }
    // A step of the field's 0.5 s for the clocks that run in seconds.
    let moved = app.procgen_resolver.living_props().time() - time;
    assert!((moved - 0.5).abs() < 1e-9, "{moved}");
    assert!((app.wooze_time - field_time - 0.5).abs() < 1e-9);

    // And it doesn't keep going.
    app.tick_procgen(0.0);
    assert_eq!(beats_of(&app), after);
}

#[test]
fn a_step_is_one_beat_wherever_a_zone_has_changed_how_long_that_is() {
    let folder = Folder::new("zones");
    let mut app = app_with(2, &folder.0);
    let zone: toml::Table = toml::from_str(
        r#"
name = "slow"
shape = "sphere"
center = [0, 0, 0]
radius = 0.5
falloff = 0
step = 3.0
"#,
    )
    .unwrap();
    app.wooze.as_mut().unwrap().add_zone(&zone).unwrap();
    app.tick_procgen(0.0);
    let steps: Vec<f32> = app
        .procgen_resolver
        .living_props()
        .props()
        .iter()
        .map(|p| p.clock().step)
        .collect();
    assert_ne!(steps[0], steps[1], "the zone covers only the first");

    let before = beats_of(&app);
    app.step_living();
    app.tick_procgen(0.0);
    for (a, b) in beats_of(&app).iter().zip(&before) {
        assert_eq!(a - b, 1.0);
    }
}

#[test]
fn a_set_of_knobs_reads_back_as_itself() {
    let folder = Folder::new("knobs");
    let mut app = app_with(2, &folder.0);
    let tune: WoozePreset = toml::from_str(
        r#"
[wooze]
amplitude = 0.6
step = 0.2
glide = 0.7
jitter = 3.0
[living]
stagger = "per_branch"
echoes = { count = 4, spread = 1.5, alpha = 0.3 }
variance = { amount = 0.9, steady = 0.1, keep_size = false }
"#,
    )
    .unwrap();
    app.tune_living(&tune).unwrap();
    let mut read = app.living_knobs();
    read.boil = None;
    assert_eq!(read, tune);

    // A knob a preset doesn't set stays where it is.
    let only_glide: WoozePreset = toml::from_str("[wooze]\nglide = 0.1\n").unwrap();
    app.tune_living(&only_glide).unwrap();
    let read = app.living_knobs();
    assert_eq!(read.wooze.glide, Some(0.1));
    assert_eq!(read.wooze.step, Some(0.2));
    assert_eq!(read.living.echoes.unwrap().count, Some(4));
}

#[test]
fn what_cant_be_used_is_named_and_the_rest_still_happens() {
    let folder = Folder::new("bad");
    let mut app = app_with(1, &folder.0);
    let tune: WoozePreset = toml::from_str(
        r#"
[wooze]
step = -1.0
glide = 0.7
[living]
stagger = "sideways"
echoes = { count = 99 }
variance = { amount = 0.2 }
"#,
    )
    .unwrap();
    let said = app.tune_living(&tune).unwrap_err();
    for word in ["sideways", "echoes.count", "step"] {
        assert!(said.contains(word), "{word} in {said}");
    }
    assert_eq!(said.matches("echoes.count").count(), 1, "once: {said}");
    let read = app.living_knobs();
    assert_eq!(read.wooze.glide, Some(0.7), "beside a bad step");
    assert_eq!(read.wooze.step, Some(0.5), "left alone");
    assert_eq!(read.living.variance.unwrap().amount, Some(0.2));
    assert_eq!(read.living.echoes.unwrap().count, Some(2), "left alone");
}

#[test]
fn turning_a_clock_knob_gives_a_scene_with_no_field_one() {
    let folder = Folder::new("nofield");
    let mut app = app_with(1, &folder.0);
    app.wooze = None;
    app.tick_procgen(0.0);
    let spec_step = app.procgen_resolver.living_props().props()[0].wooze().step;

    // Knobs that aren't the field's don't make one.
    let echoes: WoozePreset = toml::from_str("[living]\nechoes = { count = 1 }\n").unwrap();
    app.tune_living(&echoes).unwrap();
    assert!(app.wooze.is_none());

    let glide: WoozePreset = toml::from_str("[wooze]\nglide = 0.9\n").unwrap();
    app.tune_living(&glide).unwrap();
    let field = app.wooze.as_ref().unwrap().global(0.0);
    assert_eq!(field.glide, 0.9);
    assert_eq!(field.step, spec_step, "the rest as it stood");
    app.tick_procgen(0.0);
    assert_eq!(
        app.procgen_resolver.living_props().props()[0].clock().glide,
        0.9
    );
}

#[test]
fn save_then_load_brings_every_knob_back() {
    let folder = Folder::new("save");
    let mut app = app_with(2, &folder.0);
    let tuned: WoozePreset = toml::from_str(
        r#"
[wooze]
amplitude = 0.6
step = 0.2
glide = 0.7
jitter = 3.0
[living]
stagger = "none"
echoes = { count = 4, spread = 1.5, alpha = 0.3 }
variance = { amount = 0.9, steady = 0.1, keep_size = false }
"#,
    )
    .unwrap();
    app.tune_living(&tuned).unwrap();
    let path = app.save_living_preset("misty").unwrap();
    assert!(path.ends_with("wooze/misty.wooze.toml"), "{path:?}");
    let saved = app.living_knobs();

    // A fresh run, at another scene's defaults.
    let mut fresh = app_with(2, &folder.0);
    assert_ne!(fresh.living_knobs(), saved);
    assert!(fresh.living_preset_names().contains(&"misty".to_string()));
    fresh.load_living_preset("misty").unwrap();
    assert_eq!(fresh.living_knobs(), saved);
}

#[test]
fn a_built_in_loads_its_knobs_and_leaves_the_rest() {
    let folder = Folder::new("builtin");
    let mut app = app_with(1, &folder.0);
    let echoes = app.living_knobs().living.echoes;
    app.load_living_preset("boil").unwrap();
    let read = app.living_knobs();
    assert_eq!(read.wooze.step, Some(0.125));
    assert_eq!(read.living.echoes, echoes);
    let said = app.load_living_preset("nothing_of_the_kind").unwrap_err();
    assert!(said.contains("nothing_of_the_kind"), "{said}");
}

#[test]
fn as_loaded_puts_it_all_back() {
    let folder = Folder::new("reset");
    let mut app = app_with(2, &folder.0);
    app.tick_procgen(0.0);
    let at_load = app.living_knobs();
    let tuned: WoozePreset = toml::from_str(
        "[wooze]\nstep = 0.2\n[living]\nechoes = { count = 0 }\nvariance = { amount = 1.0 }\n",
    )
    .unwrap();
    app.tune_living(&tuned).unwrap();
    assert_ne!(app.living_knobs(), at_load);
    app.reset_living_knobs();
    assert_eq!(app.living_knobs(), at_load);
}

#[cfg(feature = "debug-hud")]
#[test]
fn a_panel_only_appears_in_a_scene_with_living_props_and_drives_them() {
    use flint_debug_ui::{LivingPanelAction, LivingPropsDebugPanel};

    fn panel(app: &mut PlayerApp) -> &mut LivingPropsDebugPanel {
        app.debug_panels[0]
            .as_any_mut()
            .downcast_mut::<LivingPropsDebugPanel>()
            .unwrap()
    }

    let folder = Folder::new("panel");
    let mut app = app_with(2, &folder.0);
    app.create_living_debug_panel();
    assert!(app.debug_panels.is_empty(), "no living_prop entities yet");

    let id = app.world.spawn("oak_0").unwrap();
    app.world
        .set_component(
            id,
            flint_core::components::LIVING_PROP,
            toml::Value::Table(Default::default()),
        )
        .unwrap();
    app.create_living_debug_panel();
    assert_eq!(app.debug_panels.len(), 1);
    app.debug_panels[0].toggle();

    app.drain_living_panel();
    let mut knobs = panel(&mut app).state().knobs.clone();
    assert_eq!(knobs.echo_count, 2);
    assert_eq!(panel(&mut app).state().rows.len(), 2);

    // What the panel sends, the grove does, and it mirrors back.
    knobs.echo_count = 5;
    knobs.glide = 0.8;
    panel(&mut app).send_for_test(LivingPanelAction::Set(knobs));
    panel(&mut app).send_for_test(LivingPanelAction::Freeze(true));
    panel(&mut app).send_for_test(LivingPanelAction::Overlay(true));
    app.drain_living_panel();
    assert!(app.living_tune.frozen);
    assert!(app.living_tune.overlay);
    let state = panel(&mut app).state().clone();
    assert_eq!(state.knobs.echo_count, 5);
    assert_eq!(state.knobs.glide, 0.8);
    assert!(state.frozen && state.overlay);

    let before = beats_of(&app);
    panel(&mut app).send_for_test(LivingPanelAction::Step);
    app.drain_living_panel();
    app.before_living_tick();
    let after = beats_of(&app);
    assert_eq!(after[0] - before[0], 1.0);

    panel(&mut app).send_for_test(LivingPanelAction::Save("from_panel".to_string()));
    app.drain_living_panel();
    let status = app.living_tune.status.clone().unwrap();
    assert!(status.contains("preset = \"from_panel\""), "{status}");
    assert!(folder.0.join("wooze/from_panel.wooze.toml").is_file());
}
