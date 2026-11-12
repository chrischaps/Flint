//! The knobs through the player, headless: a scene's props with no window, moved
//! the way the panel and scripts move them.

use super::*;
use flint_core::wooze::warp;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_procgen::{LivingPropRequest, ProcGenSpec, Stagger, VarianceConfig};

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
    app.core
        .procgen_resolver
        .index_spec(ProcGenSpec::parse(SPEC).unwrap());
    for i in 0..n {
        app.core
            .procgen_resolver
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
    app.core.procgen_resolver.make_living_now();
    app.living().wooze = Some(WoozeField::new(WoozeSample::NEUTRAL));
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

fn beats_of(app: &mut PlayerApp) -> Vec<f64> {
    let props = app.core.procgen_resolver.living_props();
    props.props().iter().map(|p| p.beats()).collect()
}

fn poses_at(app: &mut PlayerApp) -> Vec<(u64, f32)> {
    let props = app.core.procgen_resolver.living_props();
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
    app.tick_procgen_for_test(0.1);
    let before = beats_of(&mut app);
    let poses = poses_at(&mut app);
    let (time, field_time) = (
        app.core.procgen_resolver.living_props().time(),
        app.living().wooze_time,
    );

    // Frozen is what the frame loop does with its dt: nothing moves.
    for _ in 0..5 {
        app.tick_procgen_for_test(0.0);
    }
    assert_eq!(beats_of(&mut app), before);
    assert_eq!(poses_at(&mut app), poses);

    app.step_living();
    app.tick_procgen_for_test(0.0);
    let after = beats_of(&mut app);
    for (a, b) in after.iter().zip(&before) {
        assert_eq!(a - b, 1.0, "one whole beat, to the bit");
    }
    for ((pose, weight), (was, was_weight)) in poses_at(&mut app).iter().zip(&poses) {
        assert_eq!(*pose, was + 1, "the next pose");
        assert_eq!(weight, was_weight, "at the same place in its step");
    }
    // A step of the field's 0.5 s for the clocks that run in seconds.
    let moved = app.core.procgen_resolver.living_props().time() - time;
    assert!((moved - 0.5).abs() < 1e-9, "{moved}");
    assert!((app.living().wooze_time - field_time - 0.5).abs() < 1e-9);

    // And it doesn't keep going.
    app.tick_procgen_for_test(0.0);
    assert_eq!(beats_of(&mut app), after);
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
    app.living()
        .wooze
        .as_mut()
        .unwrap()
        .add_zone(&zone)
        .unwrap();
    app.tick_procgen_for_test(0.0);
    let steps: Vec<f32> = app
        .core
        .procgen_resolver
        .living_props()
        .props()
        .iter()
        .map(|p| p.clock().step)
        .collect();
    assert_ne!(steps[0], steps[1], "the zone covers only the first");

    let before = beats_of(&mut app);
    app.step_living();
    app.tick_procgen_for_test(0.0);
    for (a, b) in beats_of(&mut app).iter().zip(&before) {
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
softness = 0.3
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
    app.living().wooze = None;
    app.tick_procgen_for_test(0.0);
    let spec_step = app.core.procgen_resolver.living_props().props()[0]
        .wooze()
        .step;

    // Knobs that aren't the field's don't make one.
    let echoes: WoozePreset = toml::from_str("[living]\nechoes = { count = 1 }\n").unwrap();
    app.tune_living(&echoes).unwrap();
    assert!(app.living().wooze.is_none());

    let glide: WoozePreset = toml::from_str("[wooze]\nglide = 0.9\n").unwrap();
    app.tune_living(&glide).unwrap();
    let field = app.living().wooze.as_ref().unwrap().global(0.0);
    assert_eq!(field.glide, 0.9);
    assert_eq!(field.step, spec_step, "the rest as it stood");
    app.tick_procgen_for_test(0.0);
    assert_eq!(
        app.core.procgen_resolver.living_props().props()[0]
            .clock()
            .glide,
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
softness = 0.45
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
    app.tick_procgen_for_test(0.0);
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
    use flint_debug_ui::{LivingPanelAction, LivingPropsDebugPanel, PanelScope};

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

    let id = app.core.world.spawn("oak_0").unwrap();
    app.core
        .world
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
    panel(&mut app).send_for_test(LivingPanelAction::Set(PanelScope::Global, moved));
    panel(&mut app).send_for_test(LivingPanelAction::Freeze(true));
    panel(&mut app).send_for_test(LivingPanelAction::Overlay(true));
    app.drain_living_panel();
    assert!(app.living().tune.frozen);
    assert!(app.living().tune.overlay);
    let state = panel(&mut app).state().clone();
    assert_eq!(state.knobs.echo_count, 5);
    assert_eq!(state.knobs.glide, 0.8);
    assert!(state.frozen && state.overlay);

    let before = beats_of(&mut app);
    panel(&mut app).send_for_test(LivingPanelAction::Step);
    app.drain_living_panel();
    app.before_living_tick();
    let after = beats_of(&mut app);
    assert_eq!(after[0] - before[0], 1.0);

    panel(&mut app).send_for_test(LivingPanelAction::Save("from_panel".to_string()));
    app.drain_living_panel();
    let status = app.living().tune.status.clone().unwrap();
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
    two_species_under(folder, field, None)
}

/// [`two_species`] in a scene whose `[living]` table is `living`.
fn two_species_under(folder: &Path, field: bool, living: Option<&str>) -> PlayerApp {
    let table = living.map(|living| toml::from_str::<LivingDef>(living).unwrap());
    two_species_from(folder, field, table.as_ref())
}

/// [`two_species`] in a scene whose `[living]` table is `living`, parsed.
fn two_species_from(folder: &Path, field: bool, living: Option<&LivingDef>) -> PlayerApp {
    let mut app = app_with(0, folder);
    if let Some(table) = living {
        app.core
            .procgen_resolver
            .set_living_defaults(super::super::living_props::defaults_of(Some(table)));
    }
    let mut oak = ProcGenSpec::parse(SPEC).unwrap();
    oak.living.as_mut().unwrap().echoes = EchoConfig {
        count: 4,
        spread: 0.9,
        alpha: 0.2,
    };
    app.core.procgen_resolver.index_spec(oak);
    app.core
        .procgen_resolver
        .index_spec(ProcGenSpec::parse(THORN).unwrap());
    for (i, spec) in ["sapling", "sapling", "thorn", "thorn"].iter().enumerate() {
        app.core
            .procgen_resolver
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
    app.core.procgen_resolver.make_living_now();
    if !field {
        app.living().wooze = None;
    }
    app.tick_procgen_for_test(0.0);
    app
}

/// What each species has of the knobs props can differ on.
#[derive(Debug, Clone, PartialEq)]
struct Species {
    echoes: Vec<EchoConfig>,
    staggers: Vec<Stagger>,
    variances: Vec<VarianceConfig>,
}

fn species_of(app: &mut PlayerApp) -> Species {
    let living = app.core.procgen_resolver.living_props();
    Species {
        echoes: living.props().iter().map(|p| p.echo_config()).collect(),
        staggers: living.props().iter().map(|p| p.stagger()).collect(),
        variances: living.variances(),
    }
}

#[test]
fn the_two_species_start_apart() {
    let folder = Folder::new("species");
    let mut app = two_species(&folder.0, true);
    let start = species_of(&mut app);
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
    let start = species_of(&mut app);
    let count: WoozePreset = toml::from_str("[living]\nechoes = { count = 1 }\n").unwrap();
    app.tune_living(&count).unwrap();
    for (now, was) in species_of(&mut app).echoes.iter().zip(&start.echoes) {
        assert_eq!(now.count, 1);
        assert_eq!((now.spread, now.alpha), (was.spread, was.alpha));
    }
}

#[cfg(feature = "debug-hud")]
mod panel_edits {
    use super::*;
    use flint_debug_ui::{KnobFlags, LivingKnobs, LivingPanelAction, PanelScope};

    /// What the panel does with a hand on one knob: the knobs it last heard,
    /// that one moved, and only what moved sent.
    fn move_on_panel(app: &mut PlayerApp, touch: impl FnOnce(&mut LivingKnobs)) {
        let heard = LivingKnobs::from_preset(&app.living_knobs());
        let mut knobs = heard.clone();
        touch(&mut knobs);
        let moved = knobs.moved(&heard, &KnobFlags::default());
        app.apply_living_action(LivingPanelAction::Set(PanelScope::Global, moved));
    }

    #[test]
    fn moving_glide_leaves_every_species_its_own_echoes_stagger_and_variance() {
        let folder = Folder::new("glide");
        let mut app = two_species(&folder.0, true);
        let start = species_of(&mut app);
        let reholding = app.core.procgen_resolver.living_props().is_reholding();

        move_on_panel(&mut app, |k| k.glide = 0.8);
        app.tick_procgen_for_test(0.0);

        assert_eq!(species_of(&mut app), start);
        assert_eq!(
            app.core.procgen_resolver.living_props().is_reholding(),
            reholding,
            "no variance changed, so nothing to re-hold"
        );
        let glides: Vec<f32> = (app.core.procgen_resolver.living_props().props().iter())
            .map(|p| p.clock().glide)
            .collect();
        assert_eq!(glides, [0.8; 4], "and glide did move: {glides:?}");
    }

    #[test]
    fn moving_echo_count_sets_count_alone() {
        let folder = Folder::new("count");
        let mut app = two_species(&folder.0, true);
        let start = species_of(&mut app);

        move_on_panel(&mut app, |k| k.echo_count = 6);

        let now = species_of(&mut app);
        for (now, was) in now.echoes.iter().zip(&start.echoes) {
            assert_eq!(now.count, 6);
            assert_eq!((now.spread, now.alpha), (was.spread, was.alpha));
        }
        assert_eq!(now.staggers, start.staggers);
        assert_eq!(now.variances, start.variances);

        // Variance amount, the same, lineage by lineage.
        move_on_panel(&mut app, |k| k.variance_amount = 0.9);
        let now = species_of(&mut app);
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
        app.apply_living_action(LivingPanelAction::Set(PanelScope::Global, moved));
        let counts: Vec<u32> = species_of(&mut app)
            .echoes
            .iter()
            .map(|e| e.count)
            .collect();
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
        let start = species_of(&mut app);

        // A whole set, as a preset file sends: every species the same.
        let mut flat = LivingKnobs::from_preset(&app.living_knobs()).to_preset();
        flat.boil = None;
        app.tune_living(&flat).unwrap();
        assert_ne!(species_of(&mut app), start, "flattened");
        app.apply_living_action(LivingPanelAction::Reset);
        assert_eq!(species_of(&mut app), start);
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
        let mut app = two_species(&folder.0, false);
        assert!(app.living_mixed().step && app.living_mixed().glide);
    }

    #[test]
    fn as_loaded_in_a_scene_with_no_field_gives_back_each_specs_clock() {
        let folder = Folder::new("as_loaded_nofield");
        let mut app = two_species(&folder.0, false);
        app.before_living_tick();
        let clocks = |app: &PlayerApp| -> Vec<(f32, f32)> {
            let living = app.core.procgen_resolver.living_props();
            living
                .props()
                .iter()
                .map(|p| (p.clock().step, p.clock().glide))
                .collect()
        };
        let start = clocks(&mut app);
        assert_ne!(start[0], start[2]);

        // Turning one gives the scene a field, and evens the clocks out.
        move_on_panel(&mut app, |k| k.glide = 0.5);
        app.tick_procgen_for_test(0.0);
        assert!(app.living().wooze.is_some());
        let even = clocks(&mut app);
        assert_eq!(even[0], even[2]);

        app.apply_living_action(LivingPanelAction::Reset);
        app.tick_procgen_for_test(0.0);
        assert!(app.living().wooze.is_none());
        assert_eq!(clocks(&mut app), start);
    }

    #[test]
    fn a_saved_preset_leaves_out_what_the_species_differ_on() {
        let folder = Folder::new("save_mixed");
        let mut app = two_species(&folder.0, true);
        let start = species_of(&mut app);
        app.save_living_preset("grove").unwrap();
        let saved = read_preset(&folder.0.join("grove.scene.toml"), "grove")
            .unwrap()
            .unwrap();
        assert!(saved.wooze.glide.is_some(), "the field's knobs are one");
        assert_eq!(saved.living.stagger, None);
        assert_eq!(saved.living.echoes.and_then(|echoes| echoes.count), None);
        app.load_living_preset("grove").unwrap();
        assert_eq!(species_of(&mut app), start);
    }
}

// ─── A/B (#238) ───────────────────────────────────────────

/// Every bone buffer the props would fill now, written whole: each prop's, then
/// each of its echoes'.
fn everything_drawn(app: &mut PlayerApp) -> Vec<Vec<[[f32; 4]; 4]>> {
    let props = app.core.procgen_resolver.living_props_mut();
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
    app.tick_procgen_for_test(0.37);
    // Frozen from here: the frame loop hands living props a dt of 0.
    app.tick_procgen_for_test(0.0);
    let before = everything_drawn(&mut app);
    let beats = beats_of(&mut app);

    // Two of the same: a flip is nothing at all.
    app.compare_living("", "").unwrap();
    app.tick_procgen_for_test(0.0);
    assert_eq!(everything_drawn(&mut app), before, "A is what was drawn");
    assert!(app.flip_living());
    app.tick_procgen_for_test(0.0);
    assert_eq!(everything_drawn(&mut app), before, "B is the same look");
    assert_eq!(beats_of(&mut app), beats, "on the same clock");

    // A preset that differs: B is something else, and A comes back exactly.
    app.compare_living("", "linklater").unwrap();
    assert_eq!(everything_drawn(&mut app), before);
    app.flip_living();
    app.tick_procgen_for_test(0.0);
    let b = everything_drawn(&mut app);
    assert_ne!(b, before, "linklater's drift is half as far");
    assert_eq!(beats_of(&mut app), beats, "a flip moves no clock");
    let knobs = app.living_knobs();
    assert_eq!(
        (knobs.wooze.amplitude, knobs.wooze.glide),
        (Some(0.5), Some(0.6))
    );
    for _ in 0..3 {
        app.flip_living();
        app.tick_procgen_for_test(0.0);
    }
    assert_eq!(everything_drawn(&mut app), before, "back on A, to the bit");
    app.flip_living();
    assert_eq!(everything_drawn(&mut app), b, "and B to the bit");
}

/// A grove with a zone of its own, on a field like Woozy Grove's.
fn zoned_app(folder: &Path) -> PlayerApp {
    let mut app = app_with(4, folder);
    let def: flint_procgen::WoozeDef = toml::from_str(
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
    app.living().wooze = Some(def.field().0);
    app
}

#[test]
fn a_side_shown_by_a_flip_is_the_side_that_was_shown_all_along() {
    let run = |compare: (&str, &str), flip_at: Option<usize>| {
        let folder = Folder::new(&format!("ab_same_{}_{flip_at:?}", compare.0));
        let mut app = zoned_app(&folder.0);
        app.tick_procgen_for_test(0.0);
        app.compare_living(compare.0, compare.1).unwrap();
        for frame in 0..150 {
            if Some(frame) == flip_at {
                app.flip_living();
            }
            app.tick_procgen_for_test(1.0 / 60.0);
            // As the player's frame does: the shown side blends every frame.
            let props = app.core.procgen_resolver.living_props_mut();
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
    app.core
        .procgen_resolver
        .index_spec(ProcGenSpec::parse(&spec).unwrap());
    for i in 0..n {
        app.core
            .procgen_resolver
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
    app.core.procgen_resolver.make_living_now();
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
    let props = app.core.procgen_resolver.living_props_mut();
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
        app.tick_procgen_for_test(0.0);
        app.compare_living(compare.0, compare.1).unwrap();
        for frame in 0..90 {
            if Some(frame) == flip_at {
                app.flip_living();
            }
            app.tick_procgen_for_test(1.0 / 60.0);
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
    app.tick_procgen_for_test(0.2);
    app.compare_living("", "linklater").unwrap();
    for _ in 0..90 {
        app.tick_procgen_for_test(1.0 / 60.0);
    }
    let (time, field) = (
        app.core.procgen_resolver.living_props().time(),
        app.living().wooze_time,
    );
    app.flip_living();
    assert_eq!(app.core.procgen_resolver.living_props().time(), time);
    assert_eq!(app.living().wooze_time, field);

    // A step is the same seconds for both: a beat of the side being shown.
    app.step_living();
    app.tick_procgen_for_test(0.0);
    let moved = app.core.procgen_resolver.living_props().time() - time;
    assert!((moved - 0.5).abs() < 1e-9, "{moved}");
    app.flip_living();
    let other = app.core.procgen_resolver.living_props().time() - time;
    assert!((other - moved).abs() < 1e-12, "{other} against {moved}");
    let stats = app.hidden_side_stats().unwrap();
    assert_eq!(stats.frames_starved, 0, "{stats:?}");
}

#[test]
fn a_preset_that_isnt_there_is_named_and_stopping_keeps_what_is_shown() {
    let folder = Folder::new("ab_stop");
    let mut app = app_with(1, &folder.0);
    app.tick_procgen_for_test(0.1);
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
    app.tick_procgen_for_test(0.0);
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
    let id = app.core.world.spawn("oak_0").unwrap();
    app.core
        .world
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
    app.tick_procgen_for_test(0.0);
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
    app.tick_procgen_for_test(0.0);
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
    app.core.subsystems.drop_scene_ones();
    assert!(app.living_ab_state().is_none());
    app.tick_procgen_for_test(0.0);
    let ab = app.living_ab_state().unwrap();
    assert!(ab.split && ab.b == "linklater");
}

// ---- Species (#246) ----

/// What one prop runs on, read the way a person would check it.
fn running(app: &mut PlayerApp, i: usize) -> (f32, f32, EchoConfig, Stagger) {
    let prop = &app.core.procgen_resolver.living_props().props()[i];
    (
        prop.clock().step,
        prop.clock().glide,
        prop.echo_config(),
        prop.stagger(),
    )
}

#[test]
fn a_scripts_species_table_tunes_that_species_alone() {
    let folder = Folder::new("species_script");
    let mut app = two_species(&folder.0, true);
    let oak_before = running(&mut app, 0);
    let knobs: toml::Value = toml::from_str(
        r#"
        species = "thorn"
        step = 0.25
        stagger = "none"
        echoes = { count = 1 }
        "#,
    )
    .unwrap();
    app.tune_living_from_script(knobs).unwrap();
    app.tick_procgen_for_test(0.0);

    assert_eq!(running(&mut app, 0), oak_before, "the oaks as they were");
    let (step, _, echoes, stagger) = running(&mut app, 2);
    assert_eq!(step, 0.25);
    assert_eq!(echoes.count, 1);
    assert_eq!(echoes.spread, 0.7, "the thorn's own spread, from its spec");
    assert_eq!(stagger, Stagger::None);
    assert_eq!(running(&mut app, 3).0, 0.25, "both thorns");
}

#[test]
fn a_species_nothing_grows_from_is_named_and_a_bad_value_left_out() {
    let folder = Folder::new("species_bad");
    let mut app = two_species(&folder.0, true);
    let knobs: toml::Value = toml::from_str("species = \"thron\"\nglide = 0.5\n").unwrap();
    let said = app.tune_living_from_script(knobs).unwrap_err();
    assert!(
        said.contains("'thron'") && said.contains("sapling, thorn"),
        "{said}"
    );

    let knobs: toml::Value =
        toml::from_str("species = \"thorn\"\nstep = -2.0\nglide = 0.5\n").unwrap();
    let said = app.tune_living_from_script(knobs).unwrap_err();
    assert!(said.contains("step = -2"), "{said}");
    app.tick_procgen_for_test(0.0);
    assert_eq!(running(&mut app, 2).1, 0.5, "the good value still went in");

    let misspelt: toml::Value = toml::from_str("species = \"thorn\"\nstepp = 1.0\n").unwrap();
    assert!(app.tune_living_from_script(misspelt).is_err());
}

#[test]
fn a_global_change_from_a_script_leaves_a_species_value_standing() {
    let folder = Folder::new("species_global");
    let mut app = two_species(&folder.0, true);
    let thorn: toml::Value = toml::from_str("species = \"thorn\"\nglide = 0.0\n").unwrap();
    app.tune_living_from_script(thorn).unwrap();
    let everyone: WoozePreset =
        toml::from_str("[wooze]\nglide = 0.9\n[living]\nechoes = { count = 5 }\n").unwrap();
    app.tune_living(&everyone).unwrap();
    app.tick_procgen_for_test(0.0);
    assert_eq!(running(&mut app, 0).1, 0.9);
    assert_eq!(running(&mut app, 2).1, 0.0, "the thorns' glide stood");
    assert_eq!(
        running(&mut app, 2).2.count,
        5,
        "and what they don't set followed"
    );

    // Cleared, the thorns take the grove's glide.
    app.clear_living_species("thorn", &["glide".to_string()])
        .unwrap();
    app.tick_procgen_for_test(0.0);
    assert_eq!(running(&mut app, 2).1, 0.9);
    assert!(app
        .clear_living_species("thorn", &["glyde".to_string()])
        .unwrap_err()
        .contains("glyde"));
}

/// Softness (#582) through every way in: a scene's `[living]`, a script's grove
/// and species tables, the panel's view of what's mixed, a clear, and a bad value.
#[test]
fn softness_comes_in_every_way_the_other_knobs_do() {
    let folder = Folder::new("softness");
    let mut app = two_species_under(&folder.0, true, Some("softness = 0.2"));
    let soft = |app: &PlayerApp, i: usize| {
        app.core
            .procgen_resolver
            .living_props()
            .softness(LivingPropId(i))
    };
    assert_eq!(
        (soft(&mut app, 0), soft(&mut app, 2)),
        (0.2, 0.2),
        "the scene's"
    );
    assert_eq!(app.living_look().living.softness, Some(0.2));

    let thorn: toml::Value = toml::from_str("species = \"thorn\"\nsoftness = 0.9\n").unwrap();
    app.tune_living_from_script(thorn).unwrap();
    let grove: toml::Value = toml::from_str("[living]\nsoftness = 0.5\n").unwrap();
    app.tune_living_from_script(grove).unwrap();
    assert_eq!((soft(&mut app, 0), soft(&mut app, 2)), (0.5, 0.9));
    let view = app.living_view(None);
    assert_eq!(view.softness.value, 0.5);
    assert!(
        !view.softness.mixed,
        "a species' own isn't the grove disagreeing"
    );
    assert_eq!(app.living_view(Some("thorn")).softness.layers, ["species"]);
    assert_eq!(
        app.living_look().living.species["thorn"].softness,
        Some(0.9),
        "what a commit or a saved preset writes"
    );

    app.clear_living_species("thorn", &["softness".to_string()])
        .unwrap();
    assert_eq!(soft(&mut app, 2), 0.5, "cleared, the grove's");

    let bad: toml::Value = toml::from_str("[living]\nsoftness = 2.0\n").unwrap();
    let said = app.tune_living_from_script(bad).unwrap_err();
    assert!(said.contains("softness = 2"), "{said}");
    assert_eq!(soft(&mut app, 0), 0.5, "left as it was");
}

#[test]
fn a_scenes_species_table_reaches_the_player_with_bad_values_left_out() {
    let living: flint_procgen::LivingDef = toml::from_str(
        r#"
        [species.thorn]
        step = 0.25
        glide = 4.0
        echoes = { count = 2, alpha = 9.0 }
        "#,
    )
    .unwrap();
    let defaults = super::super::living_props::defaults_of(Some(&living));
    let thorn = defaults.species["thorn"];
    assert_eq!(thorn.clock.step, Some(0.25));
    assert_eq!(thorn.clock.glide, None, "4 isn't a glide");
    assert_eq!(thorn.echoes.count, Some(2));
    assert_eq!(thorn.echoes.alpha, None);
    let problems = super::super::living_props::species_problems(&living.species["thorn"]);
    assert_eq!(problems.len(), 2, "{problems:?}");
}

#[cfg(feature = "debug-hud")]
mod species_panel {
    use super::*;
    use flint_debug_ui::{KnobFlags, LivingKnobs, LivingPanelAction, PanelScope};

    #[test]
    fn as_loaded_gives_a_species_back_its_own_values() {
        let folder = Folder::new("species_as_loaded");
        // The scene's own [living.species.thorn].
        let mut app = two_species_under(&folder.0, true, Some("[species.thorn]\nstep = 0.25\n"));
        app.before_living_tick();
        assert_eq!(
            running(&mut app, 2).0,
            0.25,
            "loaded with the scene's value"
        );
        let start: Vec<_> = (0..4).map(|i| running(&mut app, i)).collect();

        // A panel edit to the whole grove, then a species value cleared.
        let heard = LivingKnobs::from_preset(&app.living_knobs());
        let mut knobs = heard.clone();
        knobs.echo_count = 6;
        knobs.step = 1.5;
        app.apply_living_action(LivingPanelAction::Set(
            PanelScope::Global,
            knobs.moved(&heard, &KnobFlags::default()),
        ));
        app.clear_living_species("thorn", &[]).unwrap();
        app.tick_procgen_for_test(0.0);
        assert_eq!(running(&mut app, 2).0, 1.5);

        app.apply_living_action(LivingPanelAction::Reset);
        app.tick_procgen_for_test(0.0);
        let now: Vec<_> = (0..4).map(|i| running(&mut app, i)).collect();
        assert_eq!(now, start);
        assert_eq!(running(&mut app, 2).0, 0.25, "the species' step is back");
    }
}

// ---- Scopes on the panel (#247) ----

#[cfg(feature = "debug-hud")]
mod scopes {
    use super::*;
    use flint_debug_ui::{
        Knob, KnobFlags, LivingKnobs, LivingPanelAction, LivingSpeciesState, PanelScope,
    };
    use flint_procgen::{BlendFrame, LivingPropId, WarpedTime};

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// Woozy Grove's living props as the scene commits them, with no window:
    /// its four species, every tree, and its field with its zones.
    pub(super) fn woozy_grove(folder: &Path) -> PlayerApp {
        let text = std::fs::read_to_string(repo().join("examples/woozy_grove.scene.toml")).unwrap();
        let scene: flint_scene::SceneFile = toml::from_str(&text).unwrap();
        let mut app = app_with(0, folder);
        app.core.procgen_resolver.set_living_defaults(
            super::super::super::living_props::defaults_of(
                flint_procgen::LivingTables::of(&scene)
                    .unwrap()
                    .living
                    .as_ref(),
            ),
        );
        for spec in ["grove_birch", "grove_cypress", "grove_oak", "grove_thorn"] {
            let path = repo().join(format!("examples/specs/{spec}.procgen.toml"));
            let text = std::fs::read_to_string(path).unwrap();
            app.core
                .procgen_resolver
                .index_spec(ProcGenSpec::parse(&text).unwrap());
        }
        // In name order, so two runs build the same grove.
        let mut names: Vec<&String> = scene.entities.keys().collect();
        names.sort();
        for (i, name) in names.into_iter().enumerate() {
            let components = &scene.entities[name].components;
            let Some(component) = components.get("living_prop") else {
                continue;
            };
            let spec = components["model"]["asset"].as_str().unwrap();
            let at = components["transform"]["position"].as_array().unwrap();
            let position = [0, 1, 2].map(|j| flint_core::toml_util::toml_f32(&at[j]).unwrap());
            let request = super::super::super::living_props::read_request(
                EntityId::from_raw(i as u64 + 1),
                name,
                position,
                spec,
                component,
            )
            .unwrap();
            app.core.procgen_resolver.add_living_prop(request).unwrap();
        }
        app.core.procgen_resolver.make_living_now();
        app.living().wooze = super::super::super::living_props::field_of(
            flint_procgen::LivingTables::of(&scene)
                .unwrap()
                .wooze
                .as_ref(),
        );
        app.tick_procgen_for_test(0.0);
        app
    }

    fn species_at(app: &mut PlayerApp, i: usize) -> String {
        let living = app.core.procgen_resolver.living_props();
        living.species_of(LivingPropId(i)).to_string()
    }

    fn state_of(app: &mut PlayerApp, name: &str) -> LivingSpeciesState {
        let unsaved = app.living_unsaved();
        let states = app.living_species_states(&unsaved);
        states.into_iter().find(|s| s.name == name).unwrap()
    }

    /// Move one knob at a species, as the panel does with a hand on it there.
    fn move_at(app: &mut PlayerApp, species: &str, touch: impl FnOnce(&mut LivingKnobs)) {
        let heard = state_of(app, species).knobs;
        let mut knobs = heard.clone();
        touch(&mut knobs);
        let moved = knobs.moved(&heard, &KnobFlags::default());
        let scope = PanelScope::Species(species.to_string());
        app.apply_living_action(LivingPanelAction::Set(scope, moved));
    }

    /// The done-when of #247, headless: in Woozy Grove, echo `count` moved at
    /// one species' scope changes that species' trees and no others, and its
    /// clear gives them back.
    #[test]
    fn at_species_scope_in_woozy_grove_echo_count_moves_that_species_alone() {
        let folder = Folder::new("scope_grove");
        let mut app = woozy_grove(&folder.0);
        let n = app.core.procgen_resolver.living_props().props().len();
        assert_eq!(app.living_species().len(), 4, "{:?}", app.living_species());
        assert!(n >= 20, "{n} trees");
        let before: Vec<_> = (0..n).map(|i| running(&mut app, i)).collect();
        let variances = app.core.procgen_resolver.living_props().variances();
        let unsaved = app.living_unsaved();
        let states = app.living_species_states(&unsaved);
        let thorn = state_of(&mut app, "grove_thorn");
        assert_eq!(thorn.sources.of(Knob::EchoCount), ["spec"]);

        let count = thorn.knobs.echo_count + 3;
        move_at(&mut app, "grove_thorn", |k| k.echo_count = count);
        app.tick_procgen_for_test(0.0);

        let mut thorns = 0;
        for (i, was) in before.iter().enumerate() {
            let now = running(&mut app, i);
            if species_at(&mut app, i) == "grove_thorn" {
                thorns += 1;
                assert_eq!(now.2.count, count);
                let mut expected = *was;
                expected.2.count = count;
                assert_eq!(now, expected, "only the count moved");
            } else {
                assert_eq!(&now, was, "{} held", species_at(&mut app, i));
            }
        }
        assert_eq!(thorns, thorn.props);
        assert_eq!(
            app.core.procgen_resolver.living_props().variances(),
            variances
        );

        // The panel sees the same: the thorns' count is their own, the other
        // three species are exactly as they were, and Global says one owns it.
        let thorn_now = state_of(&mut app, "grove_thorn");
        assert_eq!(thorn_now.sources.of(Knob::EchoCount), ["species"]);
        assert_eq!(thorn_now.knobs.echo_count, count);
        for was in states.iter().filter(|s| s.name != "grove_thorn") {
            assert_eq!(&state_of(&mut app, &was.name), was);
        }

        // Cleared, the thorns' trees take what's below again: here, their spec.
        app.apply_living_action(LivingPanelAction::Clear(
            "grove_thorn".to_string(),
            Some(Knob::EchoCount),
        ));
        app.tick_procgen_for_test(0.0);
        let now: Vec<_> = (0..n).map(|i| running(&mut app, i)).collect();
        assert_eq!(now, before);
        assert_eq!(state_of(&mut app, "grove_thorn"), thorn);
    }

    /// Who set each value, for every species, is what `flint render --report
    /// json` would name: both read `LivingProps::resolved`, prop by prop. Woozy
    /// Grove has zones, so some species read more than one layer.
    #[test]
    fn every_source_mark_is_the_layer_the_report_names() {
        let folder = Folder::new("scope_sources");
        let mut app = woozy_grove(&folder.0);
        move_at(&mut app, "grove_birch", |k| {
            k.glide = 0.05;
            k.stagger = "none".to_string();
        });
        app.tick_procgen_for_test(0.0);
        let unsaved = app.living_unsaved();
        let states = app.living_species_states(&unsaved);
        let wooze = app.living().wooze.clone();
        let time = app.living().wooze_time;
        let living = app.core.procgen_resolver.living_props();
        let field = wooze.as_ref().map(|field| (field, time));
        let mut zoned = 0;
        for state in states {
            let props: Vec<_> = (0..living.props().len())
                .map(|i| living.resolved(LivingPropId(i), field))
                .filter(|r| r.species == state.name)
                .collect();
            assert_eq!(props.len(), state.props);
            for knob in Knob::ALL {
                let mut report: Vec<String> = Vec::new();
                for prop in &props {
                    let label = match knob {
                        Knob::Amplitude => prop.amplitude.layer.label(),
                        Knob::Step => prop.step.layer.label(),
                        Knob::Glide => prop.glide.layer.label(),
                        Knob::Jitter => prop.jitter.layer.label(),
                        Knob::Stagger => prop.stagger.layer.label(),
                        Knob::EchoCount => prop.echo_count.layer.label(),
                        Knob::EchoSpread => prop.echo_spread.layer.label(),
                        Knob::EchoAlpha => prop.echo_alpha.layer.label(),
                        Knob::VarianceAmount => prop.variance_amount.layer.label(),
                        Knob::VarianceSteady => prop.variance_steady.layer.label(),
                        Knob::KeepSize => prop.keep_size.layer.label(),
                        Knob::Softness => prop.softness.layer.label(),
                    };
                    if !report.contains(&label) {
                        report.push(label);
                    }
                }
                assert_eq!(state.sources.of(knob), report, "{} {knob:?}", state.name);
                zoned += usize::from(report.iter().any(|l| l.starts_with("zone:")));
            }
            if state.name == "grove_birch" {
                assert_eq!(state.sources.of(Knob::Glide), ["species"]);
                assert_eq!(state.sources.of(Knob::Stagger), ["species"]);
            }
        }
        assert!(zoned > 0, "some knob reads a zone");
        // Every knob's key is one the clear takes.
        for knob in Knob::ALL {
            app.clear_living_species("grove_birch", &[knob.key().to_string()])
                .unwrap();
            assert!(SPECIES_KEYS.contains(&knob.key()));
        }
    }

    /// What each of a prop's parts is drawn between, right now.
    fn drawn(app: &mut PlayerApp, i: usize) -> Vec<WarpedTime> {
        let living = app.core.procgen_resolver.living_props_mut();
        living.blend_prop(LivingPropId(i), &mut BlendFrame::default());
        living.prop(LivingPropId(i)).drawn_times().to_vec()
    }

    /// Clearing a species' value gives its trees the grove's again, the one
    /// set for every prop, and nothing drawn jumps: only stagger could.
    #[test]
    fn a_cleared_species_value_is_the_groves_again_with_no_pop() {
        let folder = Folder::new("scope_clear");
        let mut app = two_species(&folder.0, true);
        let grove: WoozePreset =
            toml::from_str("[wooze]\nglide = 0.4\n[living]\nechoes = { count = 5 }\n").unwrap();
        app.apply_living_action(LivingPanelAction::Set(PanelScope::Global, grove.clone()));
        move_at(&mut app, "thorn", |k| {
            k.step = 0.25;
            k.glide = 0.9;
            k.amplitude = 0.3;
            k.jitter = 2.0;
            k.echo_count = 1;
            k.echo_alpha = 0.5;
            k.variance_amount = 0.9;
        });
        for _ in 0..20 {
            app.tick_procgen_for_test(1.0 / 60.0);
        }
        // A grove with the same global values and no species ones: what the
        // thorns should be once every value is cleared.
        let reference = Folder::new("scope_clear_reference");
        let mut plain = two_species(&reference.0, true);
        plain.apply_living_action(LivingPanelAction::Set(PanelScope::Global, grove));
        plain.tick_procgen_for_test(0.0);
        let want = plain.living_view(Some("thorn"));

        for knob in [
            Knob::Step,
            Knob::Glide,
            Knob::Amplitude,
            Knob::Jitter,
            Knob::EchoCount,
            Knob::EchoAlpha,
            Knob::VarianceAmount,
        ] {
            let held: Vec<_> = (2..4).map(|i| drawn(&mut app, i)).collect();
            assert!(
                held.iter().all(|parts| parts.len() > 1),
                "every part drawn: {held:?}"
            );
            let beats: Vec<f64> = (2..4)
                .map(|i| app.core.procgen_resolver.living_props().props()[i].beats())
                .collect();
            app.apply_living_action(LivingPanelAction::Clear("thorn".to_string(), Some(knob)));
            app.tick_procgen_for_test(0.0);
            let now: Vec<_> = (2..4).map(|i| drawn(&mut app, i)).collect();
            assert_eq!(now, held, "clearing {knob:?} moved what's drawn");
            let beats_now: Vec<f64> = (2..4)
                .map(|i| app.core.procgen_resolver.living_props().props()[i].beats())
                .collect();
            assert_eq!(beats_now, beats, "clearing {knob:?} moved the clock");
            assert!(
                !state_of(&mut app, "thorn").sources.is_own(knob),
                "{knob:?} is still the species' own"
            );
        }
        let got = app.living_view(Some("thorn"));
        assert_eq!(got.step.value, want.step.value);
        assert_eq!(got.glide.value, want.glide.value);
        assert_eq!(got.glide.value, 0.4, "the grove's glide");
        assert_eq!(got.amplitude.value, want.amplitude.value);
        assert_eq!(got.jitter.value, want.jitter.value);
        assert_eq!(got.echo_count.value, 5, "the grove's count");
        assert_eq!(got.echo_alpha.value, want.echo_alpha.value);
        assert_eq!(got.variance_amount.value, want.variance_amount.value);
        assert!(!state_of(&mut app, "thorn").has_own());
    }

    /// At Global, a species that sets its own value is left out of the knob,
    /// which reads the grove's: so a thorn table doesn't make it `mixed`.
    #[test]
    fn a_species_own_value_doesnt_make_the_grove_knob_mixed() {
        let folder = Folder::new("scope_global");
        let mut app = two_species(&folder.0, true);
        let grove: WoozePreset = toml::from_str("[living]\nechoes = { count = 5 }\n").unwrap();
        app.apply_living_action(LivingPanelAction::Set(PanelScope::Global, grove));
        assert!(!app.living_mixed().echo_count);
        move_at(&mut app, "thorn", |k| k.echo_count = 1);
        assert!(!app.living_mixed().echo_count, "the thorns own theirs");
        assert_eq!(app.living_view(None).echo_count.value, 5);
        // Every species owning it, the knob reads what they run on.
        move_at(&mut app, "sapling", |k| k.echo_count = 3);
        let view = app.living_view(None);
        assert!(view.echo_count.mixed);
        assert_eq!(view.echo_count.layers, ["species"]);
    }
}

// ---- Picking a tree in the world (#248) ----

#[cfg(feature = "debug-hud")]
mod picks {
    use super::*;
    use flint_core::Vec3;
    use flint_debug_ui::{
        DebugPanel, Knob, KnobFlags, LivingKnobs, LivingPanelAction, LivingPropsDebugPanel,
        PanelScope,
    };
    use flint_procgen::{LivingPropId, ResolvedProp};
    use winit::event::{ElementState, MouseButton};

    /// Woozy Grove with its Living Props panel, on a 1280 x 720 window.
    fn grove(folder: &Path) -> PlayerApp {
        let mut app = super::scopes::woozy_grove(folder);
        app.debug_panels
            .push(Box::new(LivingPropsDebugPanel::new()));
        app.input.set_screen_size(1280.0, 720.0);
        app.camera.aspect = 1280.0 / 720.0;
        app
    }

    fn panel(app: &mut PlayerApp) -> &mut LivingPropsDebugPanel {
        (app.debug_panels.iter_mut())
            .find_map(|p| p.as_any_mut().downcast_mut::<LivingPropsDebugPanel>())
            .unwrap()
    }

    /// The prop standing at `x, z`, by name.
    fn named_at(app: &mut PlayerApp, x: f32, z: f32) -> String {
        let living = app.core.procgen_resolver.living_props();
        let prop = (living.props().iter())
            .find(|p| (p.position()[0] - x).abs() < 0.01 && (p.position()[2] - z).abs() < 0.01)
            .unwrap_or_else(|| panic!("no prop at {x}, {z}"));
        prop.entity_name().to_string()
    }

    fn index_of(app: &mut PlayerApp, name: &str) -> usize {
        let living = app.core.procgen_resolver.living_props();
        (living.props().iter())
            .position(|p| p.entity_name() == name)
            .unwrap()
    }

    /// Stand `back` metres behind `near` on the line from `far`, at eye height,
    /// looking through `near`'s crown at `far`'s.
    fn line_up(app: &mut PlayerApp, near: [f32; 2], far: [f32; 2], back: f32) {
        let (dx, dz) = (near[0] - far[0], near[1] - far[1]);
        let len = (dx * dx + dz * dz).sqrt();
        let (ux, uz) = (dx / len, dz / len);
        app.camera.position = Vec3::new(near[0] + ux * back, 1.65, near[1] + uz * back);
        app.camera.target = Vec3::new(far[0], 2.5, far[1]);
    }

    /// The done-when of #248, headless: against known cameras in Woozy Grove, a
    /// click picks the tree in front and not the one behind it, from either
    /// side, and the panel turns to its species.
    #[test]
    fn a_click_picks_the_tree_in_front_and_not_the_one_behind() {
        let folder = Folder::new("pick_front");
        let mut app = grove(&folder.0);
        panel(&mut app).toggle();
        // Two pairs the grove has: two hush trees, and two in the shiver.
        for (a, b) in [([20.0, 4.0], [23.6, 7.6]), ([-14.8, -12.0], [-20.0, -13.6])] {
            let (name_a, name_b) = (
                named_at(&mut app, a[0], a[1]),
                named_at(&mut app, b[0], b[1]),
            );
            for (near, far, want) in [(a, b, &name_a), (b, a, &name_b)] {
                line_up(&mut app, near, far, 6.0);
                assert!(app.pick_living_at_centre(), "{want}: nothing at the centre");
                assert_eq!(app.picked_living().as_deref(), Some(want.as_str()));
                let index = index_of(&mut app, want);
                let species = app
                    .core
                    .procgen_resolver
                    .living_props()
                    .species_of(LivingPropId(index))
                    .to_string();
                assert_eq!(panel(&mut app).scope(), &PanelScope::Species(species));
            }
        }

        // A click through the window does the same, in points, and its press
        // and release go no further; the sky picks nothing and goes on.
        line_up(&mut app, a_hush(), [23.6, 7.6], 6.0);
        app.pick_living(None);
        app.input.mouse_position = (640.0, 360.0);
        assert!(app.living_takes_click(ElementState::Pressed, MouseButton::Left));
        assert!(app.living_takes_click(ElementState::Released, MouseButton::Left));
        assert_eq!(app.picked_living(), Some(named_at(&mut app, 20.0, 4.0)));
        app.camera.target = Vec3::new(20.0, 200.0, 4.0);
        assert!(!app.living_takes_click(ElementState::Pressed, MouseButton::Left));
        assert!(!app.living_takes_click(ElementState::Released, MouseButton::Left));
        assert!(!app.pick_living_at_centre(), "the sky has no trees");

        // With the panel shut, a click is the game's, whatever is under it.
        app.camera.target = Vec3::new(23.6, 2.5, 7.6);
        panel(&mut app).toggle();
        assert!(!app.living_takes_click(ElementState::Pressed, MouseButton::Left));
        // Escape lets go first, and only then would it reach the window.
        assert!(app.living_let_go());
        assert_eq!(app.picked_living(), None);
        assert!(!app.living_let_go());
    }

    fn a_hush() -> [f32; 2] {
        [20.0, 4.0]
    }

    #[test]
    fn the_cycle_key_goes_through_the_grove_by_name() {
        let folder = Folder::new("pick_cycle");
        let mut app = grove(&folder.0);
        let mut names: Vec<String> = (app.core.procgen_resolver.living_props().props().iter())
            .map(|p| p.entity_name().to_string())
            .collect();
        names.sort();
        assert!(app.pick_next_living(true));
        assert_eq!(app.picked_living().as_ref(), names.first());
        assert!(app.pick_next_living(true));
        assert_eq!(app.picked_living().as_ref(), names.get(1));
        assert!(app.pick_next_living(false));
        assert!(app.pick_next_living(false));
        assert_eq!(app.picked_living().as_ref(), names.last(), "round the back");
    }

    /// Each value and its layer, as `flint render --report json` gives them.
    fn report_of(prop: &ResolvedProp) -> Vec<(Knob, String, String)> {
        let n = |v: f32| format!("{v}");
        vec![
            (
                Knob::Amplitude,
                n(prop.amplitude.value),
                prop.amplitude.layer.label(),
            ),
            (Knob::Step, n(prop.step.value), prop.step.layer.label()),
            (Knob::Glide, n(prop.glide.value), prop.glide.layer.label()),
            (
                Knob::Jitter,
                n(prop.jitter.value),
                prop.jitter.layer.label(),
            ),
            (
                Knob::Stagger,
                super::super::super::living_props::stagger_name(prop.stagger.value).to_string(),
                prop.stagger.layer.label(),
            ),
            (
                Knob::EchoCount,
                prop.echo_count.value.to_string(),
                prop.echo_count.layer.label(),
            ),
            (
                Knob::EchoSpread,
                n(prop.echo_spread.value),
                prop.echo_spread.layer.label(),
            ),
            (
                Knob::EchoAlpha,
                n(prop.echo_alpha.value),
                prop.echo_alpha.layer.label(),
            ),
            (
                Knob::VarianceAmount,
                n(prop.variance_amount.value),
                prop.variance_amount.layer.label(),
            ),
            (
                Knob::VarianceSteady,
                n(prop.variance_steady.value),
                prop.variance_steady.layer.label(),
            ),
            (
                Knob::KeepSize,
                prop.keep_size.value.to_string(),
                prop.keep_size.layer.label(),
            ),
            (
                Knob::Softness,
                n(prop.softness.value),
                prop.softness.layer.label(),
            ),
        ]
    }

    /// The same, as the picked tree's window shows them.
    fn shown_of(
        knobs: &LivingKnobs,
        sources: &flint_debug_ui::KnobSources,
    ) -> Vec<(Knob, String, String)> {
        let n = |v: f32| format!("{v}");
        let value = |knob: Knob| match knob {
            Knob::Amplitude => n(knobs.amplitude),
            Knob::Step => n(knobs.step),
            Knob::Glide => n(knobs.glide),
            Knob::Jitter => n(knobs.jitter),
            Knob::Stagger => knobs.stagger.clone(),
            Knob::EchoCount => knobs.echo_count.to_string(),
            Knob::EchoSpread => n(knobs.echo_spread),
            Knob::EchoAlpha => n(knobs.echo_alpha),
            Knob::VarianceAmount => n(knobs.variance_amount),
            Knob::VarianceSteady => n(knobs.variance_steady),
            Knob::KeepSize => knobs.keep_size.to_string(),
            Knob::Softness => n(knobs.softness),
        };
        Knob::ALL
            .iter()
            .map(|knob| {
                let layers = sources.of(*knob);
                assert_eq!(layers.len(), 1, "one tree, one layer: {knob:?} {layers:?}");
                (*knob, value(*knob), layers[0].clone())
            })
            .collect()
    }

    /// What a picked tree's window says is what the report says for it, value
    /// for value and layer for layer, for every tree in the grove: inside a
    /// zone, where the zone has the clock's last word, and out of one. And a
    /// species' own value shows as `species` once one is set.
    #[test]
    fn a_picked_trees_values_are_the_reports() {
        let folder = Folder::new("pick_report");
        let mut app = grove(&folder.0);
        let glide = 0.15;
        let named = named_at(&mut app, -9.6, 10.4);
        let mut k = app.living_picked_state(&named).unwrap().knobs;
        let before = k.clone();
        k.glide = glide;
        let index = index_of(&mut app, &named);
        let species = app
            .core
            .procgen_resolver
            .living_props()
            .species_of(LivingPropId(index))
            .to_string();
        let moved = k.moved(&before, &KnobFlags::default());
        app.apply_living_action(LivingPanelAction::Set(
            PanelScope::Species(species.clone()),
            moved,
        ));
        app.tick_procgen_for_test(0.0);

        let (mut zoned, mut open, mut own) = (0, 0, 0);
        let names: Vec<String> = (app.core.procgen_resolver.living_props().props().iter())
            .map(|p| p.entity_name().to_string())
            .collect();
        for (i, name) in names.iter().enumerate() {
            let picked = app.living_picked_state(name).unwrap();
            let wooze = app.living().wooze.clone();
            let time = app.living().wooze_time;
            let living = app.core.procgen_resolver.living_props();
            let field = wooze.as_ref().map(|field| (field, time));
            let resolved = living.resolved(LivingPropId(i), field);
            assert_eq!(
                shown_of(&picked.knobs, &picked.sources),
                report_of(&resolved),
                "{name}"
            );
            // A zone with the last word on a knob is one the tree stands in.
            for (_, _, layer) in report_of(&resolved) {
                if let Some(zone) = layer.strip_prefix("zone:") {
                    assert!(
                        picked.zones.iter().any(|(z, _)| z == zone),
                        "{name} in {zone}"
                    );
                }
            }
            if picked.zones.is_empty() {
                open += 1;
            } else {
                zoned += 1;
            }
            own += usize::from(picked.sources.is_own(Knob::Glide));
        }
        assert!(
            zoned > 0 && open > 0,
            "{zoned} trees in zones, {open} out of them"
        );
        let of_species = (0..names.len())
            .filter(|i| {
                app.core
                    .procgen_resolver
                    .living_props()
                    .species_of(LivingPropId(*i))
                    == species
            })
            .count();
        assert_eq!(
            own, of_species,
            "the species' own glide is on its trees alone"
        );
    }

    /// A slider in the picked tree's window turns its species and no other,
    /// frozen and at speed.
    #[test]
    fn a_slider_beside_a_tree_turns_its_species_alone() {
        for frozen in [true, false] {
            let folder = Folder::new(if frozen {
                "pick_slider_frozen"
            } else {
                "pick_slider"
            });
            let mut app = grove(&folder.0);
            app.apply_living_action(LivingPanelAction::Freeze(frozen));
            let name = named_at(&mut app, 20.0, 4.0);
            assert!(app.pick_living(Some(&name)));
            let i = index_of(&mut app, &name);
            let species = app
                .core
                .procgen_resolver
                .living_props()
                .species_of(LivingPropId(i))
                .to_string();
            let n = app.core.procgen_resolver.living_props().props().len();
            let before: Vec<_> = (0..n).map(|i| running(&mut app, i)).collect();

            // What the window sends for a hand on its step: the panel's state,
            // as the host mirrored it, through the window's own move.
            let shown = panel(&mut app).state().picked.clone().unwrap();
            assert_eq!(shown.name, name);
            let mut k = shown.knobs.clone();
            k.step = 0.8;
            k.echo_count += 2;
            let moved = k.moved(&shown.knobs, &KnobFlags::default());
            app.apply_living_action(LivingPanelAction::Set(
                PanelScope::Species(species.clone()),
                moved,
            ));
            for _ in 0..4 {
                app.tick_procgen_for_test(if frozen { 0.0 } else { 1.0 / 60.0 });
            }
            for (j, was) in before.iter().enumerate() {
                let now = running(&mut app, j);
                if app
                    .core
                    .procgen_resolver
                    .living_props()
                    .species_of(LivingPropId(j))
                    == species
                {
                    assert_eq!(now.0, 0.8, "frozen {frozen}");
                    assert_eq!(now.2.count, shown.knobs.echo_count + 2);
                } else {
                    assert_eq!(now.1, was.1, "frozen {frozen}");
                    assert_eq!(now.2, was.2);
                    assert_eq!(now.3, was.3);
                    if frozen {
                        assert_eq!(&now, was);
                    }
                }
            }
            let shown = app.living_picked_state(&name).unwrap();
            assert_eq!(shown.knobs.step, 0.8);
            assert_eq!(shown.sources.of(Knob::Step), ["species"]);
        }
    }

    /// With nothing picked, the panel shut and no overlay, the drain does what
    /// it did before #248: nothing.
    #[test]
    fn nothing_picked_costs_nothing() {
        let folder = Folder::new("pick_idle");
        let mut app = grove(&folder.0);
        app.drain_living_panel();
        assert!(
            panel(&mut app).state().rows.is_empty(),
            "the panel wasn't mirrored"
        );
        let named = named_at(&mut app, 20.0, 4.0);
        assert!(app.pick_living(Some(&named)));
        assert!(panel(&mut app).state().picked.is_some());
        app.pick_living(None);
        assert!(panel(&mut app).state().picked.is_none());
    }
}

// ---- Saving species values (#249) ----

mod saving {
    use super::*;

    /// A scene file for [`two_species`], with comments and a thorn table.
    const GROVE: &str = r#"# Two species, by hand.
[scene]
name = "grove"

[wooze]
step = 0.5   # the grove's beat

[living]
# The grove's own.
stagger = "per_level"

[living.species.thorn]
step = 0.25   # thorns twitch

[entities.marker]
archetype = "x"
"#;

    fn scene_path(folder: &Path) -> PathBuf {
        folder.join("grove.scene.toml")
    }

    /// A run of the scene file in `folder` as it stands now, as a fresh start
    /// reads it: its `[living]`, species and all, and its field.
    fn fresh(folder: &Path) -> PlayerApp {
        let text = std::fs::read_to_string(scene_path(folder)).unwrap();
        let scene: flint_scene::SceneFile = toml::from_str(&text).unwrap();
        let mut app = two_species_from(
            folder,
            true,
            flint_procgen::LivingTables::of(&scene)
                .unwrap()
                .living
                .as_ref(),
        );
        app.living().wooze = super::super::super::living_props::field_of(
            flint_procgen::LivingTables::of(&scene)
                .unwrap()
                .wooze
                .as_ref(),
        );
        app.tick_procgen_for_test(0.0);
        app.before_living_tick();
        app
    }

    fn tuned_grove(name: &str) -> (Folder, PlayerApp) {
        let folder = Folder::new(name);
        std::fs::write(scene_path(&folder.0), GROVE).unwrap();
        let app = fresh(&folder.0);
        (folder, app)
    }

    /// Every value each prop runs on, with the layer that set it.
    fn resolved(app: &mut PlayerApp) -> Vec<ResolvedProp> {
        app.living_resolved()
    }

    /// What each prop runs on: its clock, echoes and stagger, and its lineage's
    /// variance. What it looks like rests on these alone.
    fn runs_on(app: &mut PlayerApp) -> Vec<(f32, f32, EchoConfig, Stagger)> {
        (0..4).map(|i| running(app, i)).collect()
    }

    fn echo_count(count: u32) -> LivingSpeciesDef {
        LivingSpeciesDef {
            echoes: Some(LivingEchoesDef {
                count: Some(count),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// The lines of `after` that aren't in `before`, and the other way round.
    fn changed(before: &str, after: &str) -> (Vec<String>, Vec<String>) {
        let was: Vec<&str> = before.lines().collect();
        let is: Vec<&str> = after.lines().collect();
        let gone = (was.iter())
            .filter(|l| !is.contains(l))
            .map(|l| l.to_string());
        let new = (is.iter())
            .filter(|l| !was.contains(l))
            .map(|l| l.to_string());
        (gone.collect(), new.collect())
    }

    #[test]
    fn a_scene_as_loaded_has_nothing_to_commit() {
        let (_folder, mut app) = tuned_grove("commit_none");
        assert_eq!(app.living_unsaved(), Unsaved::default());
        assert_eq!(
            app.living_look().living.species.len(),
            1,
            "the thorns' table"
        );
    }

    #[test]
    fn committing_one_species_value_changes_exactly_its_line() {
        let (folder, mut app) = tuned_grove("commit_one");
        assert!(app.tune_species("thorn", &echo_count(1)).is_empty());
        let unsaved = app.living_unsaved();
        assert_eq!(unsaved.species, ["thorn"]);
        assert!(!unsaved.global);
        assert_eq!(unsaved.edits.len(), 1, "{:?}", unsaved.edits);

        let said = app.commit_living().unwrap();
        assert!(
            said.contains("[living.species.thorn] echoes.count = 1"),
            "{said}"
        );
        let after = std::fs::read_to_string(scene_path(&folder.0)).unwrap();
        let (gone, new) = changed(GROVE, &after);
        assert!(gone.is_empty(), "{gone:?}");
        assert_eq!(new, ["echoes = { count = 1 }"]);
        assert_eq!(
            app.living_unsaved(),
            Unsaved::default(),
            "nothing left to write"
        );
    }

    #[test]
    fn a_fresh_run_of_the_committed_scene_runs_on_the_tuned_values() {
        let (folder, mut app) = tuned_grove("commit_fresh");
        // One of each kind: a species value set and one cleared, a grove knob,
        // and a field knob.
        app.tune_species("thorn", &echo_count(1));
        app.clear_living_species("thorn", &["step".to_string()])
            .unwrap();
        app.tune_species(
            "sapling",
            &LivingSpeciesDef {
                variance: Some(LivingVarianceDef {
                    amount: Some(0.35),
                    ..Default::default()
                }),
                stagger: Some("none".into()),
                ..Default::default()
            },
        );
        let grove: WoozePreset = toml::from_str(
            "[wooze]
glide = 0.8
[living]
echoes = { alpha = 0.4 }
",
        )
        .unwrap();
        app.tune_living(&grove).unwrap();
        app.tick_procgen_for_test(0.0);
        let unsaved = app.living_unsaved();
        assert!(unsaved.global);
        assert_eq!(unsaved.species, ["sapling", "thorn"]);
        app.commit_living().unwrap();

        let mut run = fresh(&folder.0);
        assert_eq!(resolved(&mut run), resolved(&mut app));
        assert_eq!(species_of(&mut run), species_of(&mut app));
        assert_eq!(run.living_look(), app.living_look());
        let after = std::fs::read_to_string(scene_path(&folder.0)).unwrap();
        assert!(!after.contains("# thorns twitch"), "the cleared line went");
        assert!(after.contains("step = 0.5   # the grove's beat"), "{after}");
        assert!(after.contains("# The grove's own."), "{after}");
    }

    #[test]
    fn as_loaded_restores_both_levels_and_the_marks_go() {
        let (_folder, mut app) = tuned_grove("commit_reset");
        let start = resolved(&mut app);
        app.tune_species("thorn", &echo_count(5));
        let grove: WoozePreset = toml::from_str(
            "[wooze]
step = 0.75
",
        )
        .unwrap();
        app.tune_living(&grove).unwrap();
        app.tick_procgen_for_test(0.0);
        let unsaved = app.living_unsaved();
        assert!(
            unsaved.global && unsaved.species == ["thorn"],
            "{unsaved:?}"
        );

        app.reset_living_knobs();
        app.tick_procgen_for_test(0.0);
        assert_eq!(app.living_unsaved(), Unsaved::default());
        assert_eq!(resolved(&mut app), start);
    }

    #[test]
    fn after_a_commit_as_loaded_still_means_as_the_scene_loaded() {
        let (_folder, mut app) = tuned_grove("commit_then_reset");
        app.tune_species("thorn", &echo_count(1));
        app.commit_living().unwrap();
        app.reset_living_knobs();
        // The file says count 1 now, so the load's own look is unsaved.
        assert_eq!(app.living_unsaved().species, ["thorn"]);
    }

    #[test]
    fn a_preset_with_species_values_loads_them_whole_in_another_run() {
        let (folder, mut app) = tuned_grove("preset_species");
        // The thorns' step cleared, the oaks given a table of their own.
        app.clear_living_species("thorn", &[]).unwrap();
        app.tune_species("sapling", &echo_count(1));
        app.tick_procgen_for_test(0.0);
        app.save_living_preset("both").unwrap();
        let saved = read_preset(&scene_path(&folder.0), "both")
            .unwrap()
            .unwrap();
        assert_eq!(saved.living.species["sapling"], echo_count(1));
        assert!(
            saved.living.species["thorn"].is_empty(),
            "the thorns had values in the file and now have none: {:?}",
            saved.living.species
        );
        let text = std::fs::read_to_string(folder.0.join("wooze/both.wooze.toml")).unwrap();
        assert!(
            text.contains("[living.species.thorn]\n# No values"),
            "{text}"
        );

        // Another run of the same scene: its thorns start at the file's 0.25,
        // and loading the preset gives every species what the first run had.
        let mut other = fresh(&folder.0);
        assert_ne!(runs_on(&mut other), runs_on(&mut app));
        other.load_living_preset("both").unwrap();
        other.tick_procgen_for_test(0.0);
        // The same values everywhere. Who set them can differ: a knob the grove
        // agrees on is saved as the grove's, as it was before species tables.
        assert_eq!(runs_on(&mut other), runs_on(&mut app));
        assert_eq!(species_of(&mut other), species_of(&mut app));
    }

    #[test]
    fn a_script_tune_still_lays_species_values_over_what_they_have() {
        let (_folder, mut app) = tuned_grove("tune_merges");
        app.tune_living_from_script(
            toml::toml! {
                species = "thorn"
                echoes = { count = 1 }
            }
            .into(),
        )
        .unwrap();
        let thorn = app.living_look().living.species["thorn"].clone();
        assert_eq!(thorn.step, Some(0.25), "the scene's step stands");
        assert_eq!(thorn.echoes.unwrap().count, Some(1));
    }

    #[test]
    fn a_save_names_the_species_values_the_scene_would_keep() {
        let scene: flint_scene::SceneFile = toml::from_str(GROVE).unwrap();
        let says = super::super::scene_says(&scene);
        assert!(
            says.contains(&"[living.species.thorn] step".to_string()),
            "{says:?}"
        );
        assert!(says.contains(&"[wooze] step".to_string()), "{says:?}");
        assert!(says.contains(&"[living] stagger".to_string()), "{says:?}");
    }
}
