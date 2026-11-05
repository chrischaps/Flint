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
    let before = knobs.clone();
    knobs.echo_count = 5;
    knobs.glide = 0.8;
    let moved = knobs.moved(&before, &Default::default());
    panel(&mut app).send_for_test(LivingPanelAction::Set(moved));
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

/// A second species beside [`SPEC`]'s: other echoes, stagger, variance and clock.
const THORN: &str = r#"
generator = "tree_v1"
[meta]
name = "thorn"
version = "1.0.0"
[seed]
mode = "fixed"
value = 2
[params]
branch_method = "lsystem"
lsystem_iterations = 2
leaf_density = 0.0
[living]
lineage_seed = 53
pose_ring = 4
step = 0.8
glide = 0.1
stagger = "per_branch"
echoes = { count = 2, spread = 0.7, alpha = 0.22 }
variance = { amount = 0.5, steady = 0.6, keep_size = false }
"#;

/// Two oaks of [`SPEC`] (echoes 4, per level) and two thorns, as #244 found the
/// bug: in a grove of several species, one knob flattened them all.
fn two_species(folder: &Path, field: bool) -> PlayerApp {
    let mut app = app_with(0, folder);
    let mut oak = ProcGenSpec::parse(SPEC).unwrap();
    oak.living.as_mut().unwrap().echoes = EchoConfig {
        count: 4,
        spread: 0.9,
        alpha: 0.2,
    };
    app.procgen_resolver.index_spec(oak);
    app.procgen_resolver
        .index_spec(ProcGenSpec::parse(THORN).unwrap());
    for (i, spec) in ["sapling", "sapling", "thorn", "thorn"].iter().enumerate() {
        app.procgen_resolver
            .add_living_prop(LivingPropRequest {
                entity: EntityId::from_raw(i as u64 + 1),
                entity_name: format!("{spec}_{i}"),
                position: [i as f32, 0.0, 0.0],
                spec_name: spec.to_string(),
                lineage_seed: None,
                pose_ring: None,
                phase_offset: Some(0.25),
                stagger: None,
            })
            .unwrap();
    }
    app.procgen_resolver.make_living_now();
    if !field {
        app.wooze = None;
    }
    app.tick_procgen(0.0);
    app
}

/// What each species has of the knobs props can differ on.
#[derive(Debug, Clone, PartialEq)]
struct Species {
    echoes: Vec<EchoConfig>,
    staggers: Vec<Stagger>,
    variances: Vec<VarianceConfig>,
}

fn species_of(app: &PlayerApp) -> Species {
    let living = app.procgen_resolver.living_props();
    Species {
        echoes: living.props().iter().map(|p| p.echo_config()).collect(),
        staggers: living.props().iter().map(|p| p.stagger()).collect(),
        variances: living.variances(),
    }
}

#[test]
fn the_two_species_start_apart() {
    let folder = Folder::new("species");
    let app = two_species(&folder.0, true);
    let start = species_of(&app);
    assert_eq!(start.variances.len(), 2, "a lineage each");
    assert_ne!(start.echoes[0], start.echoes[2]);
    assert_eq!(start.staggers[0], Stagger::PerLevel);
    assert_eq!(start.staggers[2], Stagger::PerBranch);
    assert_ne!(start.variances[0], start.variances[1]);
}

#[test]
fn a_sparse_echo_count_from_a_script_leaves_each_species_its_spread() {
    let folder = Folder::new("sparse");
    let mut app = two_species(&folder.0, true);
    let start = species_of(&app);
    let count: WoozePreset = toml::from_str("[living]\nechoes = { count = 1 }\n").unwrap();
    app.tune_living(&count).unwrap();
    for (now, was) in species_of(&app).echoes.iter().zip(&start.echoes) {
        assert_eq!(now.count, 1);
        assert_eq!((now.spread, now.alpha), (was.spread, was.alpha));
    }
}

#[cfg(feature = "debug-hud")]
mod panel_edits {
    use super::*;
    use flint_debug_ui::{KnobFlags, LivingKnobs, LivingPanelAction};

    /// What the panel does with a hand on one knob: the knobs it last heard,
    /// that one moved, and only what moved sent.
    fn move_on_panel(app: &mut PlayerApp, touch: impl FnOnce(&mut LivingKnobs)) {
        let heard = LivingKnobs::from_preset(&app.living_knobs());
        let mut knobs = heard.clone();
        touch(&mut knobs);
        let moved = knobs.moved(&heard, &KnobFlags::default());
        app.apply_living_action(LivingPanelAction::Set(moved));
    }

    #[test]
    fn moving_glide_leaves_every_species_its_own_echoes_stagger_and_variance() {
        let folder = Folder::new("glide");
        let mut app = two_species(&folder.0, true);
        let start = species_of(&app);
        let reholding = app.procgen_resolver.living_props().is_reholding();

        move_on_panel(&mut app, |k| k.glide = 0.8);
        app.tick_procgen(0.0);

        assert_eq!(species_of(&app), start);
        assert_eq!(
            app.procgen_resolver.living_props().is_reholding(),
            reholding,
            "no variance changed, so nothing to re-hold"
        );
        let glides: Vec<f32> = (app.procgen_resolver.living_props().props().iter())
            .map(|p| p.clock().glide)
            .collect();
        assert_eq!(glides, [0.8; 4], "and glide did move: {glides:?}");
    }

    #[test]
    fn moving_echo_count_sets_count_alone() {
        let folder = Folder::new("count");
        let mut app = two_species(&folder.0, true);
        let start = species_of(&app);

        move_on_panel(&mut app, |k| k.echo_count = 6);

        let now = species_of(&app);
        for (now, was) in now.echoes.iter().zip(&start.echoes) {
            assert_eq!(now.count, 6);
            assert_eq!((now.spread, now.alpha), (was.spread, was.alpha));
        }
        assert_eq!(now.staggers, start.staggers);
        assert_eq!(now.variances, start.variances);

        // Variance amount, the same, lineage by lineage.
        move_on_panel(&mut app, |k| k.variance_amount = 0.9);
        let now = species_of(&app);
        for (now, was) in now.variances.iter().zip(&start.variances) {
            assert_eq!(now.amount, 0.9);
            assert_eq!((now.steady, now.keep_size), (was.steady, was.keep_size));
        }
    }

    #[test]
    fn a_mixed_knob_set_to_the_first_props_value_still_moves() {
        // The panel shows the first prop's count; picking it again is meant.
        let folder = Folder::new("same");
        let mut app = two_species(&folder.0, true);
        let heard = LivingKnobs::from_preset(&app.living_knobs());
        assert!(app.living_mixed().echo_count);
        let touched = KnobFlags {
            echo_count: true,
            ..Default::default()
        };
        let moved = heard.moved(&heard, &touched);
        app.apply_living_action(LivingPanelAction::Set(moved));
        let counts: Vec<u32> = species_of(&app).echoes.iter().map(|e| e.count).collect();
        assert_eq!(counts, [4; 4]);
        assert!(!app.living_mixed().echo_count);
        assert!(
            app.living_mixed().echo_spread,
            "the rest still each its own"
        );
    }

    #[test]
    fn as_loaded_gives_each_species_back_its_own() {
        let folder = Folder::new("as_loaded");
        let mut app = two_species(&folder.0, true);
        app.before_living_tick();
        let start = species_of(&app);

        // A whole set, as a preset file sends: every species the same.
        let mut flat = LivingKnobs::from_preset(&app.living_knobs()).to_preset();
        flat.boil = None;
        app.tune_living(&flat).unwrap();
        assert_ne!(species_of(&app), start, "flattened");
        app.apply_living_action(LivingPanelAction::Reset);
        assert_eq!(species_of(&app), start);
    }

    #[test]
    fn the_panel_says_mixed_where_species_differ() {
        let folder = Folder::new("mixed");
        let mut app = two_species(&folder.0, true);
        let mixed = app.living_mixed();
        assert!(mixed.stagger && mixed.echo_count && mixed.echo_spread && mixed.echo_alpha);
        assert!(mixed.variance_amount && mixed.variance_steady && mixed.keep_size);
        assert!(!mixed.any_clock(), "the field gives every prop one clock");

        move_on_panel(&mut app, |k| k.stagger = "none".to_string());
        assert!(!app.living_mixed().stagger, "moved, so the same everywhere");
        assert!(app.living_mixed().echo_count, "and nothing else");

        // With no field, each species runs its spec's clock.
        let folder = Folder::new("mixed_nofield");
        let app = two_species(&folder.0, false);
        assert!(app.living_mixed().step && app.living_mixed().glide);
    }

    #[test]
    fn as_loaded_in_a_scene_with_no_field_gives_back_each_specs_clock() {
        let folder = Folder::new("as_loaded_nofield");
        let mut app = two_species(&folder.0, false);
        app.before_living_tick();
        let clocks = |app: &PlayerApp| -> Vec<(f32, f32)> {
            let living = app.procgen_resolver.living_props();
            living
                .props()
                .iter()
                .map(|p| (p.clock().step, p.clock().glide))
                .collect()
        };
        let start = clocks(&app);
        assert_ne!(start[0], start[2]);

        // Turning one gives the scene a field, and evens the clocks out.
        move_on_panel(&mut app, |k| k.glide = 0.5);
        app.tick_procgen(0.0);
        assert!(app.wooze.is_some());
        let even = clocks(&app);
        assert_eq!(even[0], even[2]);

        app.apply_living_action(LivingPanelAction::Reset);
        app.tick_procgen(0.0);
        assert!(app.wooze.is_none());
        assert_eq!(clocks(&app), start);
    }

    #[test]
    fn a_saved_preset_leaves_out_what_the_species_differ_on() {
        let folder = Folder::new("save_mixed");
        let mut app = two_species(&folder.0, true);
        let start = species_of(&app);
        app.save_living_preset("grove").unwrap();
        let saved = read_preset(&folder.0.join("grove.scene.toml"), "grove")
            .unwrap()
            .unwrap();
        assert!(saved.wooze.glide.is_some(), "the field's knobs are one");
        assert_eq!(saved.living.stagger, None);
        assert_eq!(saved.living.echoes.unwrap().count, None);
        app.load_living_preset("grove").unwrap();
        assert_eq!(species_of(&app), start);
    }
}

// ─── A/B (#238) ───────────────────────────────────────────

/// Every bone buffer the props would fill now, written whole: each prop's, then
/// each of its echoes'.
fn everything_drawn(app: &mut PlayerApp) -> Vec<Vec<[[f32; 4]; 4]>> {
    let props = app.procgen_resolver.living_props_mut();
    props.redraw_all();
    let mut frame = flint_procgen::BlendFrame::default();
    let mut drawn = Vec::new();
    for i in 0..props.props().len() {
        let id = flint_procgen::LivingPropId(i);
        props.blend_prop(id, &mut frame);
        drawn.push(frame.bones.clone());
        for echo in 0..props.prop(id).echo_count() {
            props.blend_echo(id, echo, &mut frame);
            drawn.push(frame.bones.clone());
        }
    }
    assert!(
        drawn.iter().all(|bones| !bones.is_empty()),
        "every buffer posed"
    );
    drawn
}

#[test]
fn with_the_clock_frozen_a_flip_changes_only_what_the_presets_differ_in() {
    let folder = Folder::new("ab_frozen");
    let mut app = app_with(3, &folder.0);
    app.tick_procgen(0.37);
    // Frozen from here: the frame loop hands living props a dt of 0.
    app.tick_procgen(0.0);
    let before = everything_drawn(&mut app);
    let beats = beats_of(&app);

    // Two of the same: a flip is nothing at all.
    app.compare_living("", "").unwrap();
    app.tick_procgen(0.0);
    assert_eq!(everything_drawn(&mut app), before, "A is what was drawn");
    assert!(app.flip_living());
    app.tick_procgen(0.0);
    assert_eq!(everything_drawn(&mut app), before, "B is the same look");
    assert_eq!(beats_of(&app), beats, "on the same clock");

    // A preset that differs: B is something else, and A comes back exactly.
    app.compare_living("", "linklater").unwrap();
    assert_eq!(everything_drawn(&mut app), before);
    app.flip_living();
    app.tick_procgen(0.0);
    let b = everything_drawn(&mut app);
    assert_ne!(b, before, "linklater's drift is half as far");
    assert_eq!(beats_of(&app), beats, "a flip moves no clock");
    let knobs = app.living_knobs();
    assert_eq!(
        (knobs.wooze.amplitude, knobs.wooze.glide),
        (Some(0.5), Some(0.6))
    );
    for _ in 0..3 {
        app.flip_living();
        app.tick_procgen(0.0);
    }
    assert_eq!(everything_drawn(&mut app), before, "back on A, to the bit");
    app.flip_living();
    assert_eq!(everything_drawn(&mut app), b, "and B to the bit");
}

/// A grove with a zone of its own, on a field like Woozy Grove's.
fn zoned_app(folder: &Path) -> PlayerApp {
    let mut app = app_with(4, folder);
    let def: flint_scene::WoozeDef = toml::from_str(
        r#"
preset = "linklater"
[[zone]]
name = "pocket"
shape = "sphere"
center = [2.0, 0.0, 0.0]
radius = 1.0
falloff = 1.0
amplitude = 1.0
step = 1.1
glide = 0.9
jitter = 0.4
"#,
    )
    .unwrap();
    app.wooze = Some(def.field().0);
    app
}

#[test]
fn a_side_shown_by_a_flip_is_the_side_that_was_shown_all_along() {
    let run = |compare: (&str, &str), flip_at: Option<usize>| {
        let folder = Folder::new(&format!("ab_same_{}_{flip_at:?}", compare.0));
        let mut app = zoned_app(&folder.0);
        app.tick_procgen(0.0);
        app.compare_living(compare.0, compare.1).unwrap();
        for frame in 0..150 {
            if Some(frame) == flip_at {
                app.flip_living();
            }
            app.tick_procgen(1.0 / 60.0);
            // As the player's frame does: the shown side blends every frame.
            let props = app.procgen_resolver.living_props_mut();
            let mut blend = flint_procgen::BlendFrame::default();
            for i in 0..props.props().len() {
                let id = flint_procgen::LivingPropId(i);
                props.blend_prop(id, &mut blend);
                for echo in 0..props.prop(id).echo_count() {
                    props.blend_echo(id, echo, &mut blend);
                }
            }
        }
        everything_drawn(&mut app)
    };
    let all_along = run(("morph", "morph"), None);
    assert_eq!(
        run(("", "morph"), Some(30)),
        all_along,
        "flipped in at half a second"
    );
    assert_eq!(run(("", "morph"), Some(0)), all_along, "flipped in at once");
}

/// A player with `n` of the real living oak, leaves and all, and no field.
fn oak_app(n: u64, folder: &Path) -> PlayerApp {
    let scene_path = folder.join("grove.scene.toml");
    let mut app = PlayerApp::new(
        FlintWorld::new(),
        scene_path.to_string_lossy().into_owned(),
        false,
        None,
        None,
    );
    let spec = std::fs::read_to_string("../../specs/examples/oak_living.procgen.toml").unwrap();
    app.procgen_resolver
        .index_spec(ProcGenSpec::parse(&spec).unwrap());
    for i in 0..n {
        app.procgen_resolver
            .add_living_prop(LivingPropRequest {
                entity: EntityId::from_raw(i + 1),
                entity_name: format!("oak_{i}"),
                position: [i as f32 * 6.0, 0.0, 0.0],
                spec_name: "oak_living".to_string(),
                lineage_seed: None,
                pose_ring: None,
                phase_offset: None,
                stagger: None,
            })
            .unwrap();
    }
    app.procgen_resolver.make_living_now();
    app
}

/// What the renderer would hold for each prop if every frame's blend went up:
/// its bones, its echoes' bones and its parts' tints, as they stand.
#[derive(Debug, PartialEq)]
struct Held {
    bones: Vec<Vec<[[f32; 4]; 4]>>,
    tints: Vec<std::collections::BTreeMap<usize, [u32; 3]>>,
    echoes: Vec<usize>,
}

fn hold_frame(app: &mut PlayerApp, held: &mut Held) {
    let props = app.procgen_resolver.living_props_mut();
    let mut frame = flint_procgen::BlendFrame::default();
    let n = props.props().len();
    held.bones.resize(n * 17, Vec::new());
    held.tints.resize(n, Default::default());
    held.echoes.resize(n, 0);
    for i in 0..n {
        let id = flint_procgen::LivingPropId(i);
        let blend = props.blend_prop(id, &mut frame);
        if let Some(first) = blend.first_bone {
            let buffer = &mut held.bones[i * 17];
            buffer.resize(buffer.len().max(first + frame.bones.len()), [[0.0; 4]; 4]);
            buffer[first..first + frame.bones.len()].copy_from_slice(&frame.bones);
        }
        if let Some(count) = blend.echo_count {
            held.echoes[i] = count;
        }
        for (part, tint) in &frame.tints {
            held.tints[i].insert(*part, tint.map(f32::to_bits));
        }
        for echo in 0..props.prop(id).echo_count() {
            if let Some(first) = props.blend_echo(id, echo, &mut frame) {
                let buffer = &mut held.bones[i * 17 + 1 + echo];
                buffer.resize(buffer.len().max(first + frame.bones.len()), [[0.0; 4]; 4]);
                buffer[first..first + frame.bones.len()].copy_from_slice(&frame.bones);
            }
        }
    }
}

#[test]
fn an_oak_shown_by_a_flip_holds_what_it_would_have_all_along() {
    let run = |compare: (&str, &str), flip_at: Option<usize>| {
        let folder = Folder::new(&format!("ab_oak_{}_{flip_at:?}", compare.0));
        let mut app = oak_app(3, &folder.0);
        let mut held = Held {
            bones: Vec::new(),
            tints: Vec::new(),
            echoes: Vec::new(),
        };
        app.tick_procgen(0.0);
        app.compare_living(compare.0, compare.1).unwrap();
        for frame in 0..90 {
            if Some(frame) == flip_at {
                app.flip_living();
            }
            app.tick_procgen(1.0 / 60.0);
            hold_frame(&mut app, &mut held);
        }
        // Tints a run never sent are the renderer's default, which is no tint.
        for tints in &mut held.tints {
            tints.retain(|_, tint| *tint != [1.0f32.to_bits(); 3]);
        }
        held
    };
    let all_along = run(("morph", "morph"), None);
    let flipped = run(("", "morph"), Some(30));
    assert_eq!(flipped.echoes, all_along.echoes);
    assert_eq!(flipped.tints, all_along.tints);
    for (i, (a, b)) in flipped.bones.iter().zip(&all_along.bones).enumerate() {
        assert_eq!(a, b, "buffer {i}");
    }
}

#[test]
fn both_sides_run_on_the_same_clock() {
    let folder = Folder::new("ab_clock");
    let mut app = app_with(2, &folder.0);
    app.tick_procgen(0.2);
    app.compare_living("", "linklater").unwrap();
    for _ in 0..90 {
        app.tick_procgen(1.0 / 60.0);
    }
    let (time, field) = (app.procgen_resolver.living_props().time(), app.wooze_time);
    app.flip_living();
    assert_eq!(app.procgen_resolver.living_props().time(), time);
    assert_eq!(app.wooze_time, field);

    // A step is the same seconds for both: a beat of the side being shown.
    app.step_living();
    app.tick_procgen(0.0);
    let moved = app.procgen_resolver.living_props().time() - time;
    assert!((moved - 0.5).abs() < 1e-9, "{moved}");
    app.flip_living();
    let other = app.procgen_resolver.living_props().time() - time;
    assert!((other - moved).abs() < 1e-12, "{other} against {moved}");
    let stats = app.hidden_side_stats().unwrap();
    assert_eq!(stats.frames_starved, 0, "{stats:?}");
}

#[test]
fn a_preset_that_isnt_there_is_named_and_stopping_keeps_what_is_shown() {
    let folder = Folder::new("ab_stop");
    let mut app = app_with(1, &folder.0);
    app.tick_procgen(0.1);
    let problem = app.compare_living("", "no_such").unwrap_err();
    assert!(problem.contains("B (no_such)"), "{problem}");
    assert!(
        app.living_ab_state().is_some(),
        "B is the knobs as they were"
    );

    app.compare_living("", "linklater").unwrap();
    app.flip_living();
    let b = app.living_knobs();
    app.stop_comparing_living();
    assert!(app.living_ab_state().is_none());
    assert_eq!(app.living_knobs(), b);
    assert!(!app.flip_living(), "nothing left to flip");
}

#[test]
fn a_preset_is_laid_over_the_scene_as_it_loaded() {
    let folder = Folder::new("ab_over_load");
    let mut app = app_with(1, &folder.0);
    app.tick_procgen(0.0);
    let at_load = app.living_knobs();
    // Turned by hand after load: as it was keeps that, a preset doesn't.
    let tuned: WoozePreset = toml::from_str("[living]\nechoes = { count = 5 }\n").unwrap();
    app.tune_living(&tuned).unwrap();
    app.compare_living("", "linklater").unwrap();
    assert_eq!(app.living_knobs().living.echoes.unwrap().count, Some(5));
    app.flip_living();
    let b = app.living_knobs();
    assert_eq!(b.living.echoes, at_load.living.echoes);
    assert_eq!(b.wooze.step, Some(0.5));
}

#[cfg(feature = "debug-hud")]
#[test]
fn the_panel_compares_flips_and_splits() {
    use flint_debug_ui::{LivingPanelAction, LivingPropsDebugPanel};

    let folder = Folder::new("ab_panel");
    let mut app = app_with(2, &folder.0);
    let id = app.world.spawn("oak_0").unwrap();
    app.world
        .set_component(
            id,
            flint_core::components::LIVING_PROP,
            toml::Value::Table(Default::default()),
        )
        .unwrap();
    app.create_living_debug_panel();
    app.debug_panels[0].toggle();
    fn panel(app: &mut PlayerApp) -> &mut LivingPropsDebugPanel {
        app.debug_panels[0]
            .as_any_mut()
            .downcast_mut::<LivingPropsDebugPanel>()
            .unwrap()
    }
    app.tick_procgen(0.0);
    panel(&mut app).send_for_test(LivingPanelAction::Compare(
        String::new(),
        "linklater".to_string(),
    ));
    app.drain_living_panel();
    let ab = panel(&mut app).state().ab.clone().unwrap();
    assert_eq!((ab.a.as_str(), ab.b.as_str()), ("", "linklater"));
    assert!(!ab.showing_b && !ab.split);

    panel(&mut app).send_for_test(LivingPanelAction::Flip);
    panel(&mut app).send_for_test(LivingPanelAction::Split(true));
    app.drain_living_panel();
    let ab = panel(&mut app).state().ab.clone().unwrap();
    assert!(ab.showing_b && ab.split);
    assert!(app.living_split());

    panel(&mut app).send_for_test(LivingPanelAction::StopComparing);
    app.drain_living_panel();
    assert!(panel(&mut app).state().ab.is_none());
}

#[test]
fn the_keys_start_a_comparison_of_the_picks_and_then_flip_and_split() {
    let folder = Folder::new("ab_keys");
    let mut app = app_with(1, &folder.0);
    app.tick_procgen(0.0);
    app.living_ab_key(false);
    let ab = app.living_ab_state().unwrap();
    assert_eq!((ab.a.as_str(), ab.b.as_str()), ("", "linklater"));
    assert!(!ab.showing_b);
    app.living_ab_key(false);
    assert!(app.living_ab_state().unwrap().showing_b);
    app.living_ab_key(true);
    assert!(app.living_split());
    app.living_ab_key(true);
    assert!(!app.living_split());
}

#[test]
fn compare_at_start_waits_for_the_props_and_outlives_the_scene_reset() {
    let folder = Folder::new("ab_start");
    let mut app = app_with(1, &folder.0);
    app.compare_living_at_start("", "linklater", true);
    app.living_tune.for_new_scene();
    assert!(app.living_ab_state().is_none());
    app.tick_procgen(0.0);
    let ab = app.living_ab_state().unwrap();
    assert!(ab.split && ab.b == "linklater");
}
