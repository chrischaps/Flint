//! `tune_living(map, seconds)` (#514), headless: a grove of saplings with no
//! window, eased the way a script eases it, frame by frame.

use crate::player_app::PlayerApp;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_living::LivingPropRequest;
use flint_procgen::{LivingKnob, ProcGenSpec};
use flint_wooze::{WoozeField, WoozeSample};

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
echoes = { count = 2, spread = 0.4 }
"#;

/// A player with three built saplings on a neutral field, and no renderer.
fn grove() -> PlayerApp {
    let mut app = PlayerApp::new(
        FlintWorld::new(),
        std::env::temp_dir()
            .join("flint_living_ramps.scene.toml")
            .to_string_lossy()
            .into_owned(),
        false,
        None,
        None,
    );
    let core = &mut app.core;
    let resolver = &mut core.procgen_resolver;
    resolver.index_spec(ProcGenSpec::parse(SPEC).unwrap());
    for i in 0..3u64 {
        (core.living_props)
            .add_prop(
                resolver,
                LivingPropRequest {
                    entity: EntityId::from_raw(i + 1),
                    entity_name: format!("sapling_{i}"),
                    position: [i as f32, 0.0, 0.0],
                    spec_name: "sapling".to_string(),
                    lineage_seed: None,
                    pose_ring: None,
                    phase_offset: Some(0.25),
                    stagger: None,
                },
            )
            .unwrap();
    }
    core.living_props.make_now(resolver);
    app.living().wooze = Some(WoozeField::new(WoozeSample::NEUTRAL));
    app
}

fn map(toml: &str) -> toml::Value {
    toml::from_str(toml).unwrap()
}

fn spread(app: &mut PlayerApp) -> f32 {
    app.living_view(None)
        .get(LivingKnob::EchoSpread)
        .value
        .as_f32()
        .unwrap()
}

/// Each prop's echoes as its ring shows them, so a knob that has "arrived" in
/// the layers but not on the props would fail.
fn prop_spreads(app: &mut PlayerApp) -> Vec<f32> {
    let props = &app.core.living_props;
    props
        .props()
        .iter()
        .map(|p| p.echo_config().spread)
        .collect()
}

/// The field's own easing, which the knobs share.
fn smoothstep(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

/// Run `frames` frames of `dt`, saying the spread after each.
fn spreads_over(app: &mut PlayerApp, frames: usize, dt: f64) -> Vec<f32> {
    (0..frames)
        .map(|_| {
            app.tick_procgen_for_test(dt);
            spread(app)
        })
        .collect()
}

/// The issue's first box: `#{ living: #{ echoes: #{ spread: 1.8 } } }` over
/// 2 s takes 2 s to get there, on the field's curve, every prop with it.
#[test]
fn a_spread_eased_over_two_seconds_takes_two_seconds_on_the_fields_curve() {
    let mut app = grove();
    app.tick_procgen_for_test(1.0 / 60.0);
    assert_eq!(spread(&mut app), 0.4);
    app.tune_living_over(map("[living.echoes]\nspread = 1.8"), 2.0)
        .unwrap();
    assert_eq!(spread(&mut app), 0.4, "nothing moves until the clock does");

    let seen = spreads_over(&mut app, 120, 1.0 / 60.0);
    for (frame, &value) in seen.iter().enumerate() {
        let t = (frame + 1) as f32 / 120.0;
        let want = 0.4 + (1.8 - 0.4) * smoothstep(t.min(1.0));
        assert!(
            (value - want).abs() < 1e-4,
            "frame {frame}: {value} where the curve says {want}"
        );
    }
    assert!(seen.windows(2).all(|w| w[1] >= w[0]), "never back");
    assert!(seen[58] < 1.8, "not there before two seconds");
    assert_eq!(seen[119], 1.8, "there at two seconds, exactly");
    assert!(app.living().ramps.is_empty(), "and done with");
    app.tick_procgen_for_test(1.0 / 60.0);
    assert_eq!(prop_spreads(&mut app), [1.8; 3]);
}

/// The second box: without seconds, it's the step it always was.
#[test]
fn with_no_seconds_it_is_a_step() {
    let mut stepped = grove();
    let mut timed = grove();
    let knobs = map("[living.echoes]\nspread = 1.8\ncount = 3");
    stepped.tune_living_from_script(knobs.clone()).unwrap();
    timed.tune_living_over(knobs, 0.0).unwrap();
    assert!(timed.living().ramps.is_empty());
    for app in [&mut stepped, &mut timed] {
        assert_eq!(spread(app), 1.8);
        assert_eq!(app.living_view(None).get(LivingKnob::EchoCount).value, 3);
    }
    assert_eq!(stepped.living_look(), timed.living_look());
}

/// Discrete knobs switch at the start; only the continuous ones ease.
#[test]
fn a_count_switches_at_once_while_its_spread_eases() {
    let mut app = grove();
    app.tune_living_over(map("[living.echoes]\ncount = 4\nspread = 1.0"), 1.0)
        .unwrap();
    let view = app.living_view(None);
    assert_eq!(view.get(LivingKnob::EchoCount).value, 4);
    assert_eq!(view.get(LivingKnob::EchoSpread).value, 0.4);
    let seen = spreads_over(&mut app, 60, 1.0 / 60.0);
    assert!(seen[0] > 0.4 && seen[0] < 0.41);
    assert_eq!(seen[59], 1.0);
}

/// A ramp started while another is running starts from where that one is: no
/// frame jumps.
#[test]
fn a_new_ramp_starts_from_where_the_last_one_stands() {
    let mut app = grove();
    app.tune_living_over(map("[living.echoes]\nspread = 1.8"), 2.0)
        .unwrap();
    let first = spreads_over(&mut app, 60, 1.0 / 60.0);
    let halfway = first[59];
    assert!((halfway - 1.1).abs() < 1e-4, "{halfway}");
    app.tune_living_over(map("[living.echoes]\nspread = 0.2"), 1.0)
        .unwrap();
    assert_eq!(app.living().ramps.len(), 1, "one ramp a knob");
    let back = spreads_over(&mut app, 60, 1.0 / 60.0);
    let mut all = vec![halfway];
    all.extend(back);
    let worst = (all.windows(2))
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    // A smoothstep's top speed is 1.5x its average: 1.6 over 60 frames
    assert!(worst < 1.5 * 1.6 / 60.0 + 1e-4, "a frame jumped {worst}");
    assert_eq!(*all.last().unwrap(), 0.2);
}

/// A script may ask every frame: asking again for where a ramp is heading
/// leaves it to arrive on time, for the props' knobs and the field's alike.
#[test]
fn asking_every_frame_for_the_same_place_still_arrives_on_time() {
    let mut app = grove();
    let knobs = map("[wooze]\nstep = 1.0\n[living.echoes]\nspread = 1.8");
    for _ in 0..60 {
        app.tune_living_over(knobs.clone(), 1.0).unwrap();
        app.tick_procgen_for_test(1.0 / 60.0);
    }
    assert_eq!(spread(&mut app), 1.8);
    let living = app.living();
    let step = living
        .wooze
        .as_ref()
        .unwrap()
        .global(living.wooze_time)
        .step;
    assert_eq!(step, 1.0, "the field's step too");
}

/// The grove's clock is the field's, and eases on the field's own ramp, which
/// is where `set_wooze` puts it.
#[test]
fn the_groves_clock_knobs_ease_on_the_field() {
    let mut app = grove();
    let start = WoozeSample::NEUTRAL.step;
    app.tune_living_over(map("[wooze]\nstep = 1.0"), 2.0)
        .unwrap();
    assert!(app.living().ramps.is_empty(), "nothing of the field's here");
    for _ in 0..60 {
        app.tick_procgen_for_test(1.0 / 60.0);
    }
    let living = app.living();
    let field = living.wooze.as_ref().unwrap();
    let halfway = field.global(living.wooze_time).step;
    assert!((halfway - (start + (1.0 - start) * 0.5)).abs() < 1e-4);
    assert_eq!(field.global_target().step, 1.0);
}

/// A knob set at once, by a script, the panel or a preset, takes over from the
/// ramp easing it; the others go on.
#[test]
fn a_step_takes_over_from_a_ramp() {
    let mut app = grove();
    app.tune_living_over(
        map("[living]\nsoftness = 1.0\n[living.echoes]\nspread = 1.8"),
        2.0,
    )
    .unwrap();
    spreads_over(&mut app, 30, 1.0 / 60.0);
    app.tune_living_from_script(map("[living.echoes]\nspread = 0.5"))
        .unwrap();
    assert_eq!(spread(&mut app), 0.5);
    assert_eq!(spreads_over(&mut app, 30, 1.0 / 60.0), [0.5; 30]);
    assert_eq!(app.living().ramps.len(), 1, "softness is still on its way");
    spreads_over(&mut app, 60, 1.0 / 60.0);
    assert_eq!(app.living_view(None).get(LivingKnob::Softness).value, 1.0);
}

/// A species' own values ease too, its clock included, and a freeze holds
/// them where they stand.
#[test]
fn a_species_eases_its_own_clock_and_a_freeze_holds_it() {
    let mut app = grove();
    let step = |app: &mut PlayerApp| {
        app.living_view(Some("sapling"))
            .get(LivingKnob::Step)
            .value
            .as_f32()
            .unwrap()
    };
    let start = step(&mut app);
    app.tune_living_over(map("species = \"sapling\"\nstep = 1.5"), 1.0)
        .unwrap();
    for _ in 0..30 {
        app.tick_procgen_for_test(1.0 / 60.0);
    }
    let halfway = step(&mut app);
    assert!((halfway - (start + (1.5 - start) * 0.5)).abs() < 1e-4);
    // Frozen is a dt of 0: nothing eases
    for _ in 0..30 {
        app.tick_procgen_for_test(0.0);
    }
    assert_eq!(step(&mut app), halfway);
    for _ in 0..30 {
        app.tick_procgen_for_test(1.0 / 60.0);
    }
    assert_eq!(step(&mut app), 1.5);
    let props = &app.core.living_props;
    assert!(props.props().iter().all(|p| p.clock().step == 1.5));
}

/// Clearing a species' value stops easing it, and so does going back to the
/// knobs as loaded.
#[test]
fn clearing_or_resetting_stops_the_ramps() {
    let mut app = grove();
    app.tick_procgen_for_test(1.0 / 60.0);
    app.tune_living_over(
        map("species = \"sapling\"\nstep = 1.5\nechoes = { alpha = 0.6 }"),
        1.0,
    )
    .unwrap();
    app.tick_procgen_for_test(1.0 / 60.0);
    app.clear_living_species("sapling", &["echoes".to_string()])
        .unwrap();
    assert_eq!(app.living().ramps.len(), 1, "the step is still easing");
    app.tune_living_over(map("[living.echoes]\nspread = 1.8"), 1.0)
        .unwrap();
    app.reset_living_knobs();
    assert!(app.living().ramps.is_empty());
    let before = spread(&mut app);
    spreads_over(&mut app, 10, 1.0 / 60.0);
    assert_eq!(spread(&mut app), before);
}

/// A value the knob can't take is named, as a step names it, and nothing
/// eases toward it; the good knobs beside it still go.
#[test]
fn a_bad_value_is_named_and_left_alone() {
    let mut app = grove();
    let problem = app
        .tune_living_over(
            map("[living]\nsoftness = 2.0\n[living.echoes]\nspread = 1.0"),
            1.0,
        )
        .unwrap_err();
    assert!(problem.contains("softness = 2"), "{problem}");
    assert_eq!(app.living().ramps.len(), 1);
    let problem = app
        .tune_living_over(map("species = \"oak\"\nstep = 1.0"), 1.0)
        .unwrap_err();
    assert!(
        problem.contains("no living prop grows from 'oak'"),
        "{problem}"
    );
}

/// #529: a script doubles the boil's pace at once and eases it to half, a
/// species takes a rate of its own and gives it back, and every prop's jitter
/// clock moves on at the rate the knob reads.
#[test]
fn the_jitter_rate_steps_eases_and_clears_and_the_boil_follows_it() {
    let mut app = grove();
    app.tick_procgen_for_test(1.0 / 60.0);
    let jitter_times = |app: &mut PlayerApp| -> Vec<f64> {
        let props = &app.core.living_props;
        props.props().iter().map(|p| p.jitter_time()).collect()
    };
    let rate = |app: &mut PlayerApp| {
        app.living_view(None)
            .get(LivingKnob::JitterRate)
            .value
            .as_f32()
            .unwrap()
    };
    assert_eq!(rate(&mut app), 1.0, "the spec's pace, with nothing said");

    app.tune_living_from_script(map("[living]\njitter_rate = 2.0"))
        .unwrap();
    assert_eq!(rate(&mut app), 2.0);
    let before = jitter_times(&mut app);
    app.tick_procgen_for_test(0.1);
    for (after, before) in jitter_times(&mut app).iter().zip(&before) {
        assert!((after - before - 0.2).abs() < 1e-9, "{after} from {before}");
    }

    // Eased to half over a second: down a little each frame, there at the end.
    app.tune_living_over(map("[living]\njitter_rate = 0.5"), 1.0)
        .unwrap();
    let mut last = rate(&mut app);
    for frame in 0..60 {
        app.tick_procgen_for_test(1.0 / 60.0);
        let now = rate(&mut app);
        assert!(
            now <= last && last - now < 0.05,
            "frame {frame}: {last} to {now}"
        );
        last = now;
    }
    assert_eq!(last, 0.5);
    assert!(app.living().ramps.is_empty());

    // A species' own beats the grove's, and clearing it gives the grove's back.
    app.tune_living_from_script(map("species = \"sapling\"\njitter_rate = 4.0"))
        .unwrap();
    assert_eq!(rate(&mut app), 4.0);
    app.clear_living_species("sapling", &["jitter_rate".to_string()])
        .unwrap();
    assert_eq!(rate(&mut app), 0.5);

    // Out of range is named and changes nothing.
    let problem = app
        .tune_living_from_script(map("[living]\njitter_rate = 9.0"))
        .unwrap_err();
    assert!(problem.contains("jitter_rate = 9"), "{problem}");
    assert_eq!(rate(&mut app), 0.5);
}

#[test]
fn the_row_rate_steps_eases_and_clears_and_the_row_clock_follows_it() {
    // #576: a script sets how fast a form's words drift, and the props' row
    // clocks move on at that rate from the frame it's set.
    let mut app = grove();
    app.tick_procgen_for_test(1.0 / 60.0);
    let row_times = |app: &mut PlayerApp| -> Vec<f64> {
        let props = &app.core.living_props;
        props.props().iter().map(|p| p.row_time()).collect()
    };
    let rate = |app: &mut PlayerApp| {
        app.living_view(None)
            .get(LivingKnob::RowRate)
            .value
            .as_f32()
            .unwrap()
    };
    assert_eq!(rate(&mut app), 1.0, "the spec's pace, with nothing said");

    app.tune_living_from_script(map("[living]\nrow_rate = 2.0"))
        .unwrap();
    assert_eq!(rate(&mut app), 2.0);
    let before = row_times(&mut app);
    app.tick_procgen_for_test(0.1);
    for (after, before) in row_times(&mut app).iter().zip(&before) {
        assert!((after - before - 0.2).abs() < 1e-9, "{after} from {before}");
    }

    app.tune_living_over(map("[living]\nrow_rate = 0.5"), 1.0)
        .unwrap();
    let mut last = rate(&mut app);
    for frame in 0..60 {
        app.tick_procgen_for_test(1.0 / 60.0);
        let now = rate(&mut app);
        assert!(
            now <= last && last - now < 0.05,
            "frame {frame}: {last} to {now}"
        );
        last = now;
    }
    assert_eq!(last, 0.5);
    assert!(app.living().ramps.is_empty());

    app.tune_living_from_script(map("species = \"sapling\"\nrow_rate = 4.0"))
        .unwrap();
    assert_eq!(rate(&mut app), 4.0);
    app.clear_living_species("sapling", &["row_rate".to_string()])
        .unwrap();
    assert_eq!(rate(&mut app), 0.5);

    let problem = app
        .tune_living_from_script(map("[living]\nrow_rate = 9.0"))
        .unwrap_err();
    assert!(problem.contains("row_rate = 9"), "{problem}");
    assert_eq!(rate(&mut app), 0.5);
}
