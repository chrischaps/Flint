//! The living props' runtime, end to end: rings, clocks, blending, budgets,
//! live knobs and the A/B, on real lineages. Settings resolution on its own,
//! with none built, is `layers.rs`'s.

use super::blend::*;
use super::ring::*;
use super::schedule::*;
use super::*;
use crate::test_grove::Grove;
use flint_procgen::living::{LivingPose, SegmentPose, BOIL_BONES};
use flint_procgen::LivingGenerator;
use flint_procgen::ProcGenCacheConfig;
use flint_procgen::{EchoConfig, EchoCopy};
use flint_wooze::WoozeField;
use std::path::Path;
use web_time::Instant;

/// Whole configs set in the global layer, as one tune of every field: what
/// `set_echoes`, `set_stagger` and `set_variance` did before #753.
trait SetAll {
    fn set_echoes(&mut self, echoes: EchoConfig);
    fn set_stagger(&mut self, stagger: Stagger);
    fn set_variance(&mut self, variance: VarianceConfig);
}

impl SetAll for LivingProps {
    fn set_echoes(&mut self, echoes: EchoConfig) {
        self.tune_echoes(LivingScope::All, EchoOverride::full(echoes));
    }

    fn set_stagger(&mut self, stagger: Stagger) {
        self.tune_stagger(LivingScope::All, stagger);
    }

    fn set_variance(&mut self, variance: VarianceConfig) {
        self.tune_variance(LivingScope::All, VarianceOverride::full(variance));
    }
}

const OAK: &str = "../../specs/examples/oak_living.procgen.toml";

/// A small tree, so tests can make many poses even in a debug build.
fn sapling(extra: &str) -> String {
    format!(
        r#"
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
variance = {{ amount = 0.5, steady = 0.5, keep_size = false }}
{extra}
{echoes}
"#,
        // These tests are about the prop itself, except those that ask.
        echoes = if extra.contains("echoes") {
            ""
        } else {
            "echoes = { count = 0 }"
        },
    )
}

fn resolver_with(spec: &str) -> Grove {
    let mut resolver = Grove::new();
    resolver.index_spec(ProcGenSpec::parse(spec).unwrap());
    resolver
}

fn request(n: u64, name: &str) -> LivingPropRequest {
    LivingPropRequest {
        entity: EntityId::from_raw(n),
        entity_name: name.to_string(),
        position: [0.0; 3],
        spec_name: "sapling".to_string(),
        lineage_seed: None,
        pose_ring: None,
        phase_offset: None,
        stagger: None,
    }
}

fn pose_seeds(props: &LivingProps, id: LivingPropId) -> Vec<u64> {
    let prop = props.prop(id);
    let first = prop.first_step().unwrap();
    (first..first + prop.ring_len() as u64)
        .map(|i| prop.pose(i).unwrap().shape_seed)
        .collect()
}

#[test]
fn a_capture_step_leaves_no_pose_missing_however_far_it_goes() {
    let mut resolver = resolver_with(&sapling(""));
    resolver.add_living_prop(request(1, "first")).unwrap();
    resolver.make_living_now();
    // Far past the whole ring in one step, as a capture may be.
    resolver.step_living_now(30.0, None);
    let props = resolver.living_props();
    let prop = &props.props()[0];
    assert!(prop.ring_len() > 0);
    assert_eq!(prop.ready(), prop.ring_len());
    assert!(prop.pose(prop.first_step().unwrap()).is_some());
    assert_eq!(props.lineage_seed(LivingPropId(0)), 7);
}

#[test]
fn what_was_drawn_is_what_it_reports() {
    let mut resolver = resolver_with(&sapling(""));
    resolver.add_living_prop(request(1, "first")).unwrap();
    resolver.make_living_now();
    resolver.step_living_now(1.0, None);
    assert!(resolver.living_props().props()[0].drawn_times().is_empty());

    let mut frame = BlendFrame::default();
    resolver
        .living_props_mut()
        .blend_prop(LivingPropId(0), &mut frame);
    let prop = &resolver.living_props().props()[0];
    let trunk = prop.drawn_times()[0];
    assert_eq!(trunk.index, prop.beats().floor() as u64);
    assert!(prop.pose(trunk.index).is_some());
}

/// 40 props ticked over `seconds` with a budget wide enough to never bind.
fn run(resolver: &mut Grove, seconds: f64, dt: f64) {
    for _ in 0..(seconds / dt).round() as usize {
        resolver.process_living(dt);
    }
}

#[test]
fn a_ring_slides_without_losing_what_is_ahead() {
    let mut ring = PoseRing::new(10, 3);
    let pose = |seed| {
        let pose = Arc::new(LivingPose {
            shape_seed: seed,
            segments: Vec::new(),
        });
        RingPose {
            full: pose.clone(),
            held: pose,
            generation: 0,
        }
    };
    for i in 10..14 {
        assert_eq!(ring.next_missing(), Some(i));
        ring.put(i, pose(i));
    }
    assert_eq!(ring.next_missing(), None);
    assert_eq!(ring.short(), 0);

    // A beat spends the spare, and the ring is still full.
    ring.slide_to(11);
    assert_eq!(ring.short(), 0);
    assert_eq!(ring.next_missing(), Some(14));

    ring.slide_to(12);
    assert_eq!(ring.short(), 1);
    assert_eq!(ring.ready, 2);
    assert!(ring.get(11).is_none(), "behind the window");
    assert_eq!(ring.get(13).unwrap().shape_seed, 13);
    assert!(ring.get(14).is_none());
    assert_eq!(ring.next_missing(), Some(14));

    // A slot is reused without its old pose answering for the new step.
    ring.put(14, pose(14));
    assert_eq!(ring.get(14).unwrap().shape_seed, 14);
    assert_eq!(ring.next_missing(), Some(15));

    // Sliding further than the window empties it.
    ring.slide_to(100);
    assert_eq!(ring.ready, 0);
    assert_eq!(ring.next_missing(), Some(100));
}

fn plain(seed: u64) -> RingPose {
    let pose = Arc::new(LivingPose {
        shape_seed: seed,
        segments: Vec::new(),
    });
    RingPose {
        full: pose.clone(),
        held: pose,
        generation: 0,
    }
}

#[test]
fn a_ring_can_reach_back_and_keeps_what_both_windows_share() {
    let mut ring = PoseRing::new(10, 3);
    for i in 10..14 {
        ring.put(i, plain(i));
    }
    ring.reshape(8, 6);
    assert_eq!((ring.first, ring.len()), (8, 6));
    assert_eq!(ring.ready, 4, "kept all four");
    assert_eq!(ring.next_missing(), Some(8), "what's behind comes first");
    assert_eq!(ring.get(12).unwrap().shape_seed, 12);
    assert!(!ring.holds(8, 2));
    ring.put(8, plain(8));
    ring.put(9, plain(9));
    assert!(ring.holds(8, 6));

    // And in again, keeping only what's still in the window.
    ring.reshape(11, 2);
    assert_eq!((ring.first, ring.len()), (11, 2));
    assert_eq!(ring.ready, 3);
    assert!(ring.get(10).is_none());
    assert_eq!(ring.get(13).unwrap().shape_seed, 13);
}

fn with_echoes(echoes: &str) -> Grove {
    resolver_with(&sapling(&format!("echoes = {{ {echoes} }}")))
}

/// One prop with its rings full, a little way into its stream so that what's
/// behind it matters. Its clock is on a fixed step of half a second.
fn running(echoes: &str, seconds: f64) -> (Grove, LivingPropId) {
    let mut resolver = with_echoes(echoes);
    let id = resolver.add_living_prop(request(1, "oak")).unwrap().0;
    resolver.make_living_now();
    run(&mut resolver, seconds, 1.0 / 60.0);
    (resolver, id)
}

fn no_pose_missed(resolver: &Grove) {
    let stats = resolver.living_props().stats();
    assert_eq!(stats.frames_starved, 0, "{stats:?}");
    assert_eq!(stats.blends_starved, 0, "{stats:?}");
}

fn blend_all(resolver: &mut Grove, frame: &mut BlendFrame, id: LivingPropId) -> usize {
    let props = resolver.living_props_mut();
    props.blend_prop(id, frame);
    let mut wrote = 0;
    for echo in 0..props.prop(id).echo_count() {
        wrote += usize::from(props.blend_echo(id, echo, frame).is_some());
    }
    wrote
}

/// A prop's bone buffers, its own first, and the tints it sent.
type Drawn = (Vec<Vec<[[f32; 4]; 4]>>, Vec<(usize, [f32; 3])>);

/// Everything a prop's buffers would hold now, written whole: its bones, then
/// each echo's, then its tints.
fn everything_drawn(props: &mut LivingProps, id: LivingPropId) -> Drawn {
    props.redraw_all();
    let mut frame = BlendFrame::default();
    let blend = props.blend_prop(id, &mut frame);
    assert_eq!(
        blend.first_bone,
        Some(0),
        "a redraw writes the whole palette"
    );
    assert_eq!(blend.echo_count, Some(props.prop(id).echo_count()));
    let tints = frame.tints.clone();
    let mut bones = vec![frame.bones.clone()];
    for echo in 0..props.prop(id).echo_count() {
        assert_eq!(props.blend_echo(id, echo, &mut frame), Some(0));
        bones.push(frame.bones.clone());
    }
    (bones, tints)
}

#[test]
fn a_redraw_writes_everything_and_moves_nothing() {
    let (mut resolver, id) = running("count = 3", 5.3);
    let props = resolver.living_props_mut();
    let mut frame = BlendFrame::default();
    props.blend_prop(id, &mut frame);
    // Nothing has moved, so nothing would be written.
    assert_eq!(props.blend_prop(id, &mut frame).first_bone, None);
    let once = everything_drawn(props, id);
    assert_eq!(everything_drawn(props, id), once);
}

#[test]
fn a_fork_on_the_same_knobs_draws_the_same_bones_and_finds_its_poses_made() {
    let (mut resolver, id) = running("count = 3", 4.0);
    let mut side = resolver.living_props().fork();
    for _ in 0..240 {
        resolver.process_living(1.0 / 60.0);
        resolver.process_living_beside(&mut side, 1.0 / 60.0, None);
    }
    assert_eq!(
        side.prop(id).beats(),
        resolver.living_props().prop(id).beats()
    );
    let ours = everything_drawn(resolver.living_props_mut(), id);
    assert_eq!(everything_drawn(&mut side, id), ours);
    let stats = side.stats();
    assert_eq!(stats.poses_made, 0, "the scene made them first: {stats:?}");
    assert!(stats.poses_from_cache > 0, "{stats:?}");
    assert_eq!(stats.frames_starved, 0, "{stats:?}");
}

#[test]
fn a_fork_keeps_the_species_values_and_turns_its_own() {
    let (resolver, id) = running("count = 2, spread = 0.2", 1.0);
    let mut props = resolver.living_props().fork();
    let species = props.species_of(id).to_string();
    let slow = SpeciesOverride {
        clock: ClockOverride {
            step: Some(1.5),
            ..ClockOverride::default()
        },
        ..SpeciesOverride::default()
    };
    assert!(props.set_species(&species, slow));
    let side = props.fork();
    assert_eq!(
        side.resolved(id, None),
        props.resolved(id, None),
        "carried over"
    );

    let mut side = side;
    let quick = SpeciesOverride {
        clock: ClockOverride {
            step: Some(0.25),
            ..ClockOverride::default()
        },
        ..SpeciesOverride::default()
    };
    side.set_species(&species, quick);
    assert_ne!(side.resolved(id, None), props.resolved(id, None));
    assert_eq!(
        props.resolved(id, None),
        props.fork().resolved(id, None),
        "left alone"
    );
}

#[test]
fn a_fork_turns_its_own_knobs_and_settles_them_at_once() {
    let (mut resolver, id) = running("count = 2, spread = 0.2", 6.0);
    let before = everything_drawn(resolver.living_props_mut(), id);
    let mut side = resolver.living_props().fork();
    side.set_echoes(EchoConfig {
        count: 6,
        spread: 1.2,
        alpha: 0.3,
    });
    side.set_stagger(Stagger::PerBranch);
    side.set_variance(VarianceConfig {
        amount: 1.0,
        steady: 0.0,
        keep_size: false,
    });
    assert!(side.prop(id).is_waiting() && side.is_reholding());
    resolver.settle_living_now(Some(&mut side));
    let prop = side.prop(id);
    assert!(!prop.is_waiting() && !side.is_reholding());
    assert_eq!(prop.echo_count(), 6);
    assert_eq!(prop.ready(), prop.ring_len(), "full");

    // Settled is still: ticks that move no clock change nothing it draws.
    let settled = everything_drawn(&mut side, id);
    for _ in 0..30 {
        resolver.process_living_beside(&mut side, 0.0, None);
    }
    assert_eq!(everything_drawn(&mut side, id), settled);
    // And the scene's own look never heard of it.
    assert_eq!(everything_drawn(resolver.living_props_mut(), id), before);
    assert_eq!(resolver.living_props().prop(id).echo_count(), 2);
}

#[test]
fn a_wider_spread_waits_until_the_ring_has_what_it_reads() {
    let (mut resolver, id) = running("count = 2, spread = 0.2", 8.0);
    let props = resolver.living_props_mut();
    let first_before = props.prop(id).first_step().unwrap();
    let in_use: Vec<EchoCopy> = (0..2).map(|i| props.prop(id).echo(i)).collect();

    props.set_echoes(EchoConfig {
        count: 4,
        spread: WIDE,
        alpha: 0.2,
    });
    let prop = props.prop(id);
    assert!(prop.is_waiting());
    assert_eq!(prop.echo_config().count, 4, "a knob reads as turned");
    assert_eq!(prop.echo_count(), 2, "and the old echoes keep drawing");
    assert_eq!(prop.echo(0), in_use[0]);
    assert!(
        prop.first_step().unwrap() < first_before,
        "its window has reached back for them"
    );

    // A frame with no budget can't make them, so nothing has changed.
    let mut frame = BlendFrame::default();
    resolver.living_props_mut().stats = LivingStats::default();
    resolver.frame_budget_for_test(0.0);
    resolver.process_living(1.0 / 60.0);
    blend_all(&mut resolver, &mut frame, id);
    assert!(resolver.living_props().prop(id).is_waiting());
    assert_eq!(resolver.living_props().prop(id).echo_count(), 2);
    no_pose_missed(&resolver);

    // Given the room, the ring fills and they start, all at once.
    resolver.frame_budget_for_test(100.0);
    resolver.process_living(1.0 / 60.0);
    blend_all(&mut resolver, &mut frame, id);
    let prop = resolver.living_props().prop(id);
    assert!(!prop.is_waiting());
    assert_eq!(prop.echo_count(), 4);
    assert_eq!(prop.echo(0).offset, -WIDE);
    no_pose_missed(&resolver);
}

/// Wide enough to reach well behind, and short of where an echo fades out.
const WIDE: f32 = 1.6;

/// The whole range of both knobs, back and forth, at a budget tight enough to
/// matter and with blending the whole way, as a hand on a slider would. The
/// ring has to keep up, or hold the old values: no pose is ever missing.
#[test]
fn sweeping_count_and_spread_across_their_range_misses_no_pose() {
    let mut resolver = resolver_with(&sapling(
        "stagger = \"per_branch\"\nechoes = { count = 6, spread = 0.9 }",
    ));
    let mut ids = Vec::new();
    for n in 0..6u64 {
        let name = format!("oak_{n}");
        ids.push(resolver.add_living_prop(request(n + 1, &name)).unwrap().0);
    }
    resolver.make_living_now();
    // A pose a frame: well under what a real budget makes, so it has to wait.
    resolver.frame_budget_for_test(1e-6);
    let mut frame = BlendFrame::default();
    let dt = 1.0 / 60.0;
    let mut most_echoes = 0;
    let mut waits = 0;
    for f in 0..1800 {
        let t = f64::from(f) / 1800.0;
        // A triangle each, out of step, so every pairing of the two gets a turn.
        let tri = |x: f64| 1.0 - (2.0 * x.fract() - 1.0).abs();
        let echoes = EchoConfig {
            count: (tri(t * 3.0) * f64::from(flint_procgen::MAX_ECHOES)).round() as u32,
            spread: (tri(t * 7.0 + 0.3) * f64::from(flint_procgen::MAX_ECHO_SPREAD)) as f32,
            alpha: 0.2,
        };
        resolver.living_props_mut().set_echoes(echoes);
        resolver.process_living(dt);
        for id in &ids {
            blend_all(&mut resolver, &mut frame, *id);
            let prop = resolver.living_props().prop(*id);
            most_echoes = most_echoes.max(prop.echo_count());
            waits += usize::from(prop.is_waiting());
            assert!(prop.ring_len() >= 3);
        }
    }
    no_pose_missed(&resolver);
    assert!(most_echoes >= 14, "the sweep got there: {most_echoes}");
    assert!(waits > 0, "and some of it had to wait for poses");

    // At rest it settles at what it was last told, with a ring its own size.
    let echoes = EchoConfig {
        count: 0,
        spread: 0.0,
        alpha: 0.2,
    };
    resolver.living_props_mut().set_echoes(echoes);
    resolver.frame_budget_for_test(100.0);
    resolver.process_living(dt);
    for id in &ids {
        let prop = resolver.living_props().prop(*id);
        assert!(!prop.is_waiting());
        assert_eq!(prop.echo_count(), 0);
        assert_eq!(prop.ring_len(), 4, "back to pose_ring");
    }
}

#[test]
fn a_change_of_alpha_keeps_the_echoes_where_they_are() {
    let (mut resolver, id) = running("count = 2, spread = 0.5, alpha = 0.2", 2.0);
    let mut frame = BlendFrame::default();
    blend_all(&mut resolver, &mut frame, id);
    let offset = resolver.living_props().prop(id).echo(0).offset;
    let opacity = resolver.living_props().prop(id).echo(0).opacity;
    let (first, len) = {
        let prop = resolver.living_props().prop(id);
        (prop.first_step(), prop.ring_len())
    };

    resolver.living_props_mut().set_echoes(EchoConfig {
        count: 2,
        spread: 0.5,
        alpha: 0.4,
    });
    resolver.process_living(0.0);
    let prop = resolver.living_props().prop(id);
    assert!(!prop.is_waiting(), "it needs no new pose");
    assert_eq!(prop.echo(0).offset, offset);
    assert!((prop.echo(0).opacity - opacity * 2.0).abs() < 1e-5);
    assert_eq!((prop.first_step(), prop.ring_len()), (first, len));

    // A clock that's stopped still shows it: an opacity reaches the renderer
    // with a write, so both echoes have one to send.
    assert_eq!(blend_all(&mut resolver, &mut frame, id), 2);
}

#[test]
fn dropping_echoes_says_how_many_are_left_once() {
    let (mut resolver, id) = running("count = 4", 1.0);
    let mut frame = BlendFrame::default();
    resolver.living_props_mut().blend_prop(id, &mut frame);
    resolver.living_props_mut().set_echoes(EchoConfig {
        count: 1,
        ..EchoConfig::default()
    });
    resolver.process_living(0.0);
    let blend = resolver.living_props_mut().blend_prop(id, &mut frame);
    assert_eq!(blend.echo_count, Some(1));
    let blend = resolver.living_props_mut().blend_prop(id, &mut frame);
    assert_eq!(blend.echo_count, None);
}

#[test]
fn a_new_stagger_grows_the_ring_first_and_starts_in_one_piece() {
    let mut resolver = resolver_with(&sapling("stagger = \"none\""));
    let mut small = request(1, "oak");
    small.pose_ring = Some(2);
    let id = resolver.add_living_prop(small).unwrap().0;
    resolver.make_living_now();
    run(&mut resolver, 4.0, 1.0 / 60.0);
    assert_eq!(resolver.living_props().prop(id).ring_len(), 2);

    resolver.living_props_mut().set_stagger(Stagger::PerBranch);
    let prop = resolver.living_props().prop(id);
    assert_eq!(prop.stagger(), Stagger::PerBranch);
    assert_eq!(prop.clock().stagger, Stagger::None, "not until it's ready");

    let mut frame = BlendFrame::default();
    resolver.process_living(1.0 / 60.0);
    blend_all(&mut resolver, &mut frame, id);
    let prop = resolver.living_props().prop(id);
    assert_eq!(prop.clock().stagger, Stagger::PerBranch);
    assert_eq!(prop.ring_len(), 3, "per-branch reaches further");
    no_pose_missed(&resolver);
}

fn held_is_what_variance_says(resolver: &Grove, id: LivingPropId, want: VarianceConfig) {
    let props = resolver.living_props();
    let prop = props.prop(id);
    let (skeleton, _) = props.rig_of(id).unwrap();
    let first = prop.first_step().unwrap();
    for index in first..first + prop.ring_len() as u64 {
        let full = prop.full_pose(index).unwrap();
        let held = prop.pose(index).unwrap();
        assert_eq!(
            **held,
            skeleton.with_variance(full, &want).unwrap(),
            "{index}"
        );
    }
}

/// A pose is as its seed grew it, in the ring and the cache alike, so a new
/// variance reaches all of them and nothing has to grow again.
#[test]
fn a_new_variance_reaches_every_pose_in_the_ring_and_regrows_none() {
    let (mut resolver, id) = running("count = 2", 3.0);
    let start = resolver.living_props().stats();
    let want = VarianceConfig {
        amount: 0.9,
        steady: 0.1,
        keep_size: true,
    };
    resolver.living_props_mut().set_variance(want);
    assert!(resolver.living_props().is_reholding());
    resolver.process_living(0.0);
    assert!(!resolver.living_props().is_reholding());
    held_is_what_variance_says(&resolver, id, want);
    let stats = resolver.living_props().stats();
    assert_eq!(stats.poses_made, start.poses_made, "no regrowing");
    assert!(stats.poses_reheld >= 5, "{stats:?}");

    // What's made from here on starts at it, whichever way it comes: from the
    // generator, and from the cache after it was left behind.
    run(&mut resolver, 2.0, 1.0 / 60.0);
    held_is_what_variance_says(&resolver, id, want);
    let back = resolver.living_props().prop(id).first_step().unwrap();
    resolver.living_props_mut().set_echoes(EchoConfig {
        count: 2,
        spread: 1.8,
        alpha: 0.2,
    });
    resolver.process_living(0.0);
    let stats = resolver.living_props().stats();
    assert!(
        resolver.living_props().prop(id).first_step().unwrap() < back,
        "it reached back into the cache"
    );
    assert!(stats.poses_from_cache > 0, "{stats:?}");
    held_is_what_variance_says(&resolver, id, want);
}

#[test]
fn unheld_variance_is_each_pose_exactly_as_it_grew() {
    let (mut resolver, id) = running("count = 0", 2.0);
    resolver
        .living_props_mut()
        .set_variance(VarianceConfig::FULL);
    resolver.process_living(0.0);
    let props = resolver.living_props();
    let prop = props.prop(id);
    let (skeleton, _) = props.rig_of(id).unwrap();
    let spec = ProcGenSpec::parse(&sapling("")).unwrap();
    let tree = flint_procgen::TreeGenerator;
    let first = prop.first_step().unwrap();
    for index in first..first + prop.ring_len() as u64 {
        let grew = LivingGenerator::full_pose(&tree, &spec, skeleton, prop.shape_seed(index));
        assert_eq!(**prop.pose(index).unwrap(), grew.unwrap(), "{index}");
    }
}

/// Re-holding comes out of the frame's budget, a share at a time, and a prop
/// holding still draws what it gets, without its clock having moved.
#[test]
fn a_re_hold_spreads_over_frames_and_redraws_a_stopped_prop() {
    let mut resolver = resolver_with(&sapling("echoes = { count = 2 }"));
    let mut ids = Vec::new();
    for n in 0..8u64 {
        let name = format!("oak_{n}");
        ids.push(resolver.add_living_prop(request(n + 1, &name)).unwrap().0);
    }
    resolver.make_living_now();
    resolver.process_living(0.0);
    let mut frame = BlendFrame::default();
    for id in &ids {
        blend_all(&mut resolver, &mut frame, *id);
        assert_eq!(
            blend_all(&mut resolver, &mut frame, *id),
            0,
            "holding still is free"
        );
    }

    resolver.living_props_mut().set_variance(VarianceConfig {
        amount: 0.9,
        steady: 0.0,
        keep_size: true,
    });
    // No room at all: it waits its turn and can't lose anything.
    resolver.frame_budget_for_test(0.0);
    resolver.process_living(0.0);
    assert!(resolver.living_props().is_reholding());
    assert_eq!(resolver.living_props().stats().poses_reheld, 0);

    // A frame's worth of room for one is the least it gets: it still finishes.
    resolver.frame_budget_for_test(1e-6);
    let mut frames = 0;
    while resolver.living_props().is_reholding() {
        resolver.process_living(0.0);
        frames += 1;
        assert!(frames < 1000);
    }
    assert!(frames > 1, "took {frames} frames");
    for id in &ids {
        assert!(
            blend_all(&mut resolver, &mut frame, *id) > 0,
            "the new poses go up though no clock has moved"
        );
    }
    no_pose_missed(&resolver);
}

/// A re-hold costs what a pose does, so a whole grove's rings take seconds.
/// What the props are drawing has to go first, on every prop, before any ring
/// gets finished.
#[test]
fn a_re_hold_reaches_what_every_prop_is_drawing_first() {
    let mut resolver = resolver_with(&sapling("echoes = { count = 0 }"));
    let mut ids = Vec::new();
    for n in 0..6u64 {
        let name = format!("oak_{n}");
        ids.push(resolver.add_living_prop(request(n + 1, &name)).unwrap().0);
    }
    resolver.make_living_now();
    run(&mut resolver, 1.0, 1.0 / 60.0);

    let want = VarianceConfig {
        amount: 0.9,
        steady: 0.1,
        keep_size: true,
    };
    resolver.living_props_mut().set_variance(want);
    let is_current = |resolver: &Grove, id: LivingPropId, index: u64| {
        let props = resolver.living_props();
        let prop = props.prop(id);
        let (skeleton, _) = props.rig_of(id).unwrap();
        let full = prop.full_pose(index).unwrap();
        **prop.pose(index).unwrap() == skeleton.with_variance(full, &want).unwrap()
    };
    // A pose a frame, and enough frames for what's in use and little more.
    resolver.frame_budget_for_test(1e-6);
    let in_use = |resolver: &Grove, id: LivingPropId| {
        let prop = resolver.living_props().prop(id);
        let first = prop.beats().floor() as u64;
        first..first + prop.clock().reach(2) + 1
    };
    let mut frames = 0;
    while !ids
        .iter()
        .all(|id| in_use(&resolver, *id).all(|index| is_current(&resolver, *id, index)))
    {
        resolver.process_living(0.0);
        frames += 1;
        assert!(frames < 200);
    }
    assert!(
        resolver.living_props().is_reholding(),
        "it got here with some of the rings still to do"
    );
    let last = *ids.last().unwrap();
    let prop = resolver.living_props().prop(last);
    let ring_end = prop.first_step().unwrap() + prop.ring_len() as u64;
    assert!(
        !is_current(&resolver, last, ring_end - 1),
        "and hadn't gone on to what nothing is reading yet"
    );
}

/// A ring about to run out is more urgent than any re-hold.
#[test]
fn a_ring_about_to_run_out_comes_before_a_re_hold() {
    let (mut resolver, id) = running("count = 0", 1.0);
    let props = resolver.living_props_mut();
    props.props[id.0].ring.as_mut().unwrap().slide_to(2);
    let first = props.prop(id).beats().floor() as u64;
    let ring = props.props[id.0].ring.as_mut().unwrap();
    let (slot, was) = (ring.slot(first + 1), first + 1);
    if ring.slots[slot].take().is_some() {
        ring.ready -= 1;
    }
    assert_eq!(
        props.prop(id).next_needed(false).map(|(step, _)| step),
        Some(was)
    );
    props.set_variance(VarianceConfig {
        amount: 0.9,
        ..VarianceConfig::default()
    });
    resolver.frame_budget_for_test(1e-6);
    resolver.process_living(0.0);
    let prop = resolver.living_props().prop(id);
    assert!(prop.pose(was).is_some(), "the pose came first");
    assert_eq!(resolver.living_props().stats().pose_failures, 0);
}

/// All of it at once with a pose a frame to do it in: six props at 0.12 s a step
/// want 50 poses a second out of 60, under a stagger that reaches far, a new
/// variance every frame, and echoes that want poses behind the props. There's
/// only room if what's being read wins every time.
/// A stall on a busy machine leaves the estimate over the budget, and it comes
/// down a little with each pose until it's just under. Exactly at the budget,
/// no frame can ever fit a pose, since finding that out takes some of the frame.
/// The rings ran dry like this at 0.25 ms of a 4 ms budget, for as long as the
/// game went on, at a steady 60 fps (#219). It takes a grove's worth of props:
/// with a few, stepping their clocks fits inside the timer's resolution, the
/// frame reads as having used nothing, and an estimate at the budget still fits.
#[test]
fn an_estimate_just_under_the_budget_does_not_stop_the_poses() {
    // Four props at a step of 0.1 s want 40 poses a second, two thirds of
    // one a 60 Hz frame. So the rings keep up as long as most frames make
    // a pose at all, whatever a pose costs here (#597: 256 props needed
    // ~43 a frame, which a debug build can't make in 4 ms, and the test
    // measured the machine instead of the estimate).
    const PROPS: u64 = 4;
    const BUDGET_MS: f64 = 4.0;
    let mut resolver = resolver_with(&sapling("step = 0.1"));
    for n in 0..PROPS {
        let name = format!("oak_{n}");
        resolver.add_living_prop(request(n + 1, &name)).unwrap();
    }
    resolver.make_living_now();
    resolver.frame_budget_for_test(BUDGET_MS);
    let made_before = resolver.living_props().stats().poses_made;
    // Any time spent before the first pose puts it over. Whether a frame
    // spends any before it is the machine's clock's to say (a few props can
    // finish inside one tick of it), so the decay that keeps this estimate
    // from turning away every frame is pinned on a clock of the tests' own,
    // in `a_refused_frame_brings_a_stuck_estimate_down` below.
    resolver.living_props_mut().pose_estimate_ms = BUDGET_MS;

    let frames = 240u32;
    run(&mut resolver, f64::from(frames) / 60.0, 1.0 / 60.0);

    let props = resolver.living_props();
    let stats = props.stats();
    assert!(stats.poses_made > made_before, "no poses in 4 s: {stats:?}");
    // What 4 s of beats asked for, give or take the ring's spare.
    let wanted = PROPS * u64::from(frames) / 6;
    assert!(
        stats.poses_made - made_before >= wanted - PROPS,
        "{} poses for {wanted} beats: {stats:?}",
        stats.poses_made - made_before
    );
    assert!(
        props.pose_estimate_ms < BUDGET_MS,
        "the estimate never came down: {} ms",
        props.pose_estimate_ms
    );
    for prop in props.props() {
        assert_eq!(prop.ready(), prop.ring_len(), "{stats:?}");
    }
}

/// Frames of [`fill_while_it_fits`] on a clock of the test's own: each frame
/// spends `overhead_ms` before its first pose and `pose_ms` on each, and a
/// pose more goes missing every frame. Returns the poses each frame made and
/// the estimate after the last.
fn fake_frames(
    frames: usize,
    budget_ms: f64,
    overhead_ms: f64,
    pose_ms: f64,
    mut estimate_ms: f64,
) -> (Vec<u32>, f64) {
    let missing = std::cell::Cell::new(0u32);
    let mut made = Vec::with_capacity(frames);
    for _ in 0..frames {
        missing.set(missing.get() + 1);
        let clock = std::cell::Cell::new(overhead_ms);
        let mut this_frame = 0;
        fill_while_it_fits(
            &mut estimate_ms,
            budget_ms,
            || clock.get(),
            || {
                if missing.get() == 0 {
                    return None;
                }
                clock.set(clock.get() + pose_ms);
                missing.set(missing.get() - 1);
                this_frame += 1;
                Some((true, pose_ms))
            },
        );
        made.push(this_frame);
    }
    (made, estimate_ms)
}

#[test]
fn a_refused_frame_brings_a_stuck_estimate_down() {
    // An estimate at the budget, a frame that has spent 0.05 ms before its
    // first pose: refused. Without the decay every later frame would be too,
    // whatever a pose really costs (0.2 ms here).
    let (made, estimate) = fake_frames(240, 4.0, 0.05, 0.2, 4.0);
    assert_eq!(made[0], 0, "the first frame can't fit the estimate");
    let first = made.iter().position(|&n| n > 0).expect("never made a pose");
    // 4 x 0.99^n <= 3.95 at n = 2: the third frame fits.
    assert_eq!(first, 2, "{made:?}");
    // A pose a frame is wanted; the backlog is gone well before the end.
    assert_eq!(made.iter().sum::<u32>(), 240, "{made:?}");
    assert!(estimate < 1.0, "the estimate stayed at {estimate} ms");
}

#[test]
fn a_pose_dearer_than_the_budget_still_gets_one_a_frame() {
    // A debug build whose pose costs more than the whole budget.
    let (made, estimate) = fake_frames(60, 4.0, 0.05, 6.0, 0.0);
    assert!(made.iter().all(|&n| n == 1), "{made:?}");
    assert_eq!(estimate, 6.0);
}

#[test]
fn a_frame_stops_at_the_pose_that_would_pass_the_budget() {
    // 0.5 ms poses in a 4 ms budget after 0.05 ms: seven fit, an eighth
    // would end at 4.05 ms.
    let mut estimate = 0.5;
    let clock = std::cell::Cell::new(0.05);
    let mut made = 0;
    fill_while_it_fits(
        &mut estimate,
        4.0,
        || clock.get(),
        || {
            clock.set(clock.get() + 0.5);
            made += 1;
            Some((true, 0.5))
        },
    );
    assert_eq!(made, 7);
    assert!(clock.get() <= 4.0);
}

#[test]
fn a_pose_being_read_beats_every_other_use_of_a_tight_budget() {
    let mut resolver = resolver_with(&sapling(
        "step = 0.12\nstagger = \"per_branch\"\nechoes = { count = 6, spread = 0.9 }",
    ));
    let mut ids = Vec::new();
    for n in 0..6u64 {
        let name = format!("oak_{n}");
        ids.push(resolver.add_living_prop(request(n + 1, &name)).unwrap().0);
    }
    resolver.make_living_now();
    resolver.frame_budget_for_test(1e-6);
    let mut frame = BlendFrame::default();
    let tri = |x: f64| 1.0 - (2.0 * x.fract() - 1.0).abs();
    for f in 0..1800 {
        let t = f64::from(f) / 1800.0;
        let props = resolver.living_props_mut();
        props.set_echoes(EchoConfig {
            count: (tri(t * 5.0) * 16.0).round() as u32,
            spread: (tri(t * 9.0 + 0.3) * 1.8) as f32,
            alpha: 0.2,
        });
        props.set_variance(VarianceConfig {
            amount: tri(t * 7.0) as f32,
            steady: tri(t * 3.0) as f32,
            keep_size: f % 200 < 100,
        });
        resolver.process_living(1.0 / 60.0);
        for id in &ids {
            blend_all(&mut resolver, &mut frame, *id);
        }
    }
    no_pose_missed(&resolver);
}

#[test]
fn advancing_by_beats_moves_every_clock_one_whole_step() {
    let (mut resolver, id) = running("count = 2", 1.0);
    let before = resolver.living_props().prop(id).beats();
    let time = resolver.living_props().time();
    resolver.living_props_mut().advance_beats(1.0, 0.5);
    assert_eq!(resolver.living_props().prop(id).beats() - before, 1.0);
    assert_eq!(resolver.living_props().time() - time, 0.5);
    resolver.process_living(0.0);
    let prop = resolver.living_props().prop(id);
    assert!(prop.pose(prop.first_step().unwrap()).is_some(), "ready");
    no_pose_missed(&resolver);
}

#[test]
fn the_scene_can_say_stagger_and_variance_over_the_spec() {
    let mut resolver = resolver_with(&sapling("stagger = \"none\""));
    resolver.set_living_defaults(LivingDefaults {
        stagger: Some(Stagger::PerBranch),
        variance: VarianceOverride {
            amount: Some(1.0),
            steady: None,
            keep_size: Some(false),
        },
        ..Default::default()
    });
    let mut own = request(1, "oak");
    own.stagger = Some(Stagger::None);
    let id = resolver.add_living_prop(own).unwrap().0;
    resolver.make_living_now();
    let props = resolver.living_props();
    assert_eq!(props.prop(id).clock().stagger, Stagger::PerBranch);
    let variance = props.variance().unwrap();
    assert_eq!(variance.amount, 1.0);
    assert_eq!(variance.steady, 0.5, "from the spec");
    assert!(!variance.keep_size);
    held_is_what_variance_says(&resolver, id, variance);
}

#[test]
fn sliding_backwards_changes_nothing() {
    let mut ring = PoseRing::new(10, 2);
    ring.slide_to(5);
    assert_eq!(ring.first, 10);
}

#[test]
fn a_ring_of_one_spec_does_not_collapse_to_one_request() {
    let mut resolver = resolver_with(&sapling(""));
    let id = resolver.add_living_prop(request(1, "oak")).unwrap().0;
    resolver.make_living_now();
    let props = resolver.living_props();
    let seeds = pose_seeds(props, id);
    assert_eq!(seeds.len(), 4);
    let mut unique = seeds.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), 4, "{seeds:?}");
    assert_ne!(
        props.prop(id).pose(0).unwrap().segments,
        props.prop(id).pose(1).unwrap().segments
    );
    // None of it reached the mesh cache.
    assert_eq!(resolver.cache_stats().entry_count, 0);
}

#[test]
fn props_share_a_lineage_and_not_a_stream() {
    let mut resolver = resolver_with(&sapling(""));
    let (a, asset_a) = resolver.add_living_prop(request(1, "a")).unwrap();
    let (b, asset_b) = resolver.add_living_prop(request(2, "b")).unwrap();
    let mut other = request(3, "c");
    other.lineage_seed = Some(8);
    let (_, asset_c) = resolver.add_living_prop(other).unwrap();

    assert_eq!(asset_a, "sapling@7");
    assert_eq!(asset_b, "sapling@7");
    assert_eq!(asset_c, "sapling@8");

    let done = resolver.make_living_now();
    assert_eq!(done.len(), 2, "one build a lineage");
    assert_eq!(done[0].entities.len(), 2);
    assert_eq!(done[1].entities.len(), 1);
    let props = resolver.living_props();
    assert_eq!(props.lineage_count(), 2);
    assert_ne!(pose_seeds(props, a), pose_seeds(props, b));
}

#[test]
fn a_rest_palette_is_the_identity() {
    let mut resolver = resolver_with(&sapling(""));
    resolver.add_living_prop(request(1, "a")).unwrap();
    let done = resolver.make_living_now();
    let palette = &done[0].rest_palette;
    let bones = done[0].rig.as_ref().unwrap().bone_segments.len();
    assert_eq!(palette.len(), bones + BOIL_BONES);
    for bone in &palette[bones..] {
        assert_eq!(*bone, [[0.0; 4]; 4], "still jitter bones");
    }
    for bone in &palette[..bones] {
        for (c, column) in bone.iter().enumerate() {
            for (r, value) in column.iter().enumerate() {
                let want = if c == r { 1.0 } else { 0.0 };
                assert!((value - want).abs() < 1e-3, "{bone:?}");
            }
        }
    }
}

#[test]
fn the_component_overrides_the_spec() {
    let mut resolver = resolver_with(&sapling("stagger = \"per_level\""));
    let mut custom = request(1, "a");
    custom.pose_ring = Some(6);
    custom.phase_offset = Some(1.25);
    custom.stagger = Some(Stagger::None);
    let id = resolver.add_living_prop(custom).unwrap().0;
    resolver.make_living_now();
    let prop = resolver.living_props().prop(id);
    assert_eq!(prop.ring_len(), 6);
    assert_eq!(prop.phase_offset(), 0.25, "wraps into one step");
    assert_eq!(prop.clock().stagger, Stagger::None);
}

#[test]
fn a_name_sets_the_phase_the_same_way_every_time() {
    let mut resolver = resolver_with(&sapling(""));
    let a = resolver.add_living_prop(request(1, "tree_a")).unwrap().0;
    let b = resolver.add_living_prop(request(2, "tree_b")).unwrap().0;
    // Whatever the id: entity ids differ run to run.
    let again = resolver.add_living_prop(request(99, "tree_a")).unwrap().0;
    let props = resolver.living_props();
    assert_eq!(
        props.prop(a).phase_offset(),
        props.prop(again).phase_offset()
    );
    assert_ne!(props.prop(a).phase_offset(), props.prop(b).phase_offset());
    assert_eq!(props.prop(a).shape_seed(3), props.prop(again).shape_seed(3));
    let phase = props.prop(a).phase_offset();
    assert!((0.0..1.0).contains(&phase), "{phase}");
}

/// Names that differ only in their last characters, as a scene's do, have
/// to land all over the cycle: in step, a grove would be one big pop, and
/// ask for all its poses on one frame.
#[test]
fn a_grove_of_numbered_names_spreads_across_the_cycle() {
    let phases: Vec<f32> = (0..40)
        .map(|i| name_phase(name_hash(&format!("oak_{i:02}"))))
        .collect();
    let mut buckets = [0; 10];
    for phase in &phases {
        buckets[(*phase * 10.0) as usize] += 1;
    }
    assert!(
        buckets.iter().all(|b| (1..=10).contains(b)),
        "{buckets:?} from {phases:?}"
    );
}

#[test]
fn neighbouring_names_do_not_share_poses() {
    // A stream is its start plus the step, so two that started close would
    // be the same poses a few steps apart.
    let mut starts: Vec<u64> = (0..200)
        .map(|i| name_hash(&format!("oak_{i:02}")))
        .collect();
    starts.sort_unstable();
    for pair in starts.windows(2) {
        assert!(pair[1] - pair[0] > 1 << 32, "{pair:?}");
    }
}

#[test]
fn a_small_ring_still_reaches_every_staggered_part() {
    // Two poses is valid in a spec, and per-branch stagger needs three.
    let mut resolver = resolver_with(&sapling("stagger = \"per_branch\""));
    let mut small = request(1, "a");
    small.pose_ring = Some(2);
    let id = resolver.add_living_prop(small).unwrap().0;
    resolver.make_living_now();

    let props = resolver.living_props();
    let (skeleton, _) = props.rig_of(id).unwrap();
    let prop = props.prop(id);
    assert_eq!(prop.ring_len(), 3);
    for i in 0..200 {
        let t = f64::from(i) * 0.013;
        let first = prop.clock().at(t).index;
        for segment in &skeleton.segments {
            let now = prop.clock().for_segment(t, segment).index;
            assert!(now + 1 < first + prop.ring_len() as u64, "at {t}");
        }
    }
}

#[test]
fn rings_stay_full_as_the_clock_runs() {
    let mut resolver = resolver_with(&sapling(""));
    let ids: Vec<_> = (0..8)
        .map(|n| {
            let name = format!("tree_{n}");
            resolver.add_living_prop(request(n, &name)).unwrap().0
        })
        .collect();
    resolver.make_living_now();
    run(&mut resolver, 5.0, 1.0 / 60.0);

    let props = resolver.living_props();
    for id in &ids {
        let prop = props.prop(*id);
        assert_eq!(prop.ready(), prop.ring_len());
        assert_eq!(prop.first_step(), Some(prop.beats().floor() as u64));
    }
    let stats = props.stats();
    assert_eq!(stats.frames, 300);
    assert_eq!(stats.frames_starved, 0);
    // 5 s at 2 steps a second: about 10 new poses a prop, with no extras.
    let made = stats.poses_made;
    assert!((70..=90).contains(&made), "{made}");
    assert_eq!(stats.most_short, 0, "{stats:?}");
    assert_eq!(stats.frames_full, 300, "{stats:?}");
}

#[test]
fn a_paused_clock_makes_nothing() {
    let mut resolver = resolver_with(&sapling(""));
    resolver.add_living_prop(request(1, "a")).unwrap();
    resolver.make_living_now();
    for _ in 0..600 {
        resolver.process_living(0.0);
    }
    assert_eq!(resolver.living_props().stats().poses_made, 0);
    assert_eq!(resolver.living_props().time(), 0.0);
}

#[test]
fn poses_do_not_depend_on_the_frame_rate_or_the_budget() {
    let seeds_after = |dts: &[f64], budget: f64| {
        let mut resolver = Grove::with_config(ProcGenCacheConfig {
            runtime_max_generation_ms_per_frame: budget,
            ..Default::default()
        });
        resolver.index_spec(ProcGenSpec::parse(&sapling("")).unwrap());
        let id = resolver.add_living_prop(request(5, "the_tree")).unwrap().0;
        resolver.make_living_now();
        for dt in dts {
            resolver.process_living(*dt);
        }
        // Give it whatever it still owes, with the clock stopped.
        for _ in 0..200 {
            resolver.process_living(0.0);
        }
        let props = resolver.living_props();
        let prop = props.prop(id);
        let first = prop.first_step().unwrap();
        (
            first,
            pose_seeds(props, id),
            prop.pose(first).unwrap().clone(),
        )
    };
    let smooth = seeds_after(&[1.0 / 60.0; 240], 4.0);
    let lumpy = seeds_after(&[0.25, 1.0, 0.5, 0.25, 2.0 - 1e-9], 0.001);
    assert_eq!(smooth, lumpy);
}

#[test]
fn a_tiny_budget_still_makes_one_pose_a_frame() {
    let mut resolver = Grove::with_config(ProcGenCacheConfig {
        runtime_max_generation_ms_per_frame: 1e-6,
        ..Default::default()
    });
    resolver.index_spec(ProcGenSpec::parse(&sapling("")).unwrap());
    resolver.add_living_prop(request(1, "a")).unwrap();

    // No make_living_now: the first frame builds, then it's a pose a frame.
    assert_eq!(resolver.process_living(0.0).len(), 1);
    let filled = |r: &Grove| r.living_props().props()[0].ready();
    assert_eq!(filled(&resolver), 0);
    resolver.process_living(0.0);
    assert_eq!(filled(&resolver), 1);
    resolver.process_living(0.0);
    assert_eq!(filled(&resolver), 2);
}

#[test]
fn a_spec_that_cannot_live_says_why() {
    let mut resolver = resolver_with(
        r#"
generator = "tree_v1"
[meta]
name = "still"
version = "1.0.0"
[seed]
mode = "fixed"
value = 1
"#,
    );
    let mut no_table = request(1, "a");
    no_table.spec_name = "still".into();
    let reason = resolver.add_living_prop(no_table).unwrap_err();
    assert!(reason.contains("[living]"), "{reason}");

    let mut unknown = request(2, "b");
    unknown.spec_name = "nothing".into();
    assert!(resolver.add_living_prop(unknown).is_err());

    let mut resolver = resolver_with(&sapling("").replace("\"lsystem\"", "\"space_colonization\""));
    let reason = resolver.add_living_prop(request(3, "c")).unwrap_err();
    assert!(reason.contains("space"), "{reason}");
    assert!(resolver.living_props().is_empty());
}

#[test]
fn coming_back_to_a_scene_reads_its_poses_from_the_cache() {
    let mut resolver = resolver_with(&sapling(""));
    let id = resolver.add_living_prop(request(1, "a")).unwrap().0;
    resolver.make_living_now();
    let before = pose_seeds(resolver.living_props(), id);

    resolver.clear_living_props();
    assert!(resolver.living_props().is_empty());

    let id = resolver.add_living_prop(request(2, "a")).unwrap().0;
    for _ in 0..8 {
        resolver.process_living(0.0);
    }
    let props = resolver.living_props();
    assert_eq!(pose_seeds(props, id), before);
    let stats = props.stats();
    // Four in the ring and the spare.
    assert_eq!((stats.poses_made, stats.poses_from_cache), (0, 5));
}

#[test]
fn the_pose_cache_keeps_to_its_size_and_rings_keep_their_poses() {
    let pose = |seed| {
        Arc::new(LivingPose {
            shape_seed: seed,
            segments: vec![SegmentPose::IDENTITY; 10],
        })
    };
    let one = PoseCache::size_of(&pose(0));
    let mut cache = PoseCache::new(one * 3);
    let identity = ContentHash::from_str("x");
    let held = pose(0);
    cache.insert(identity, 0, held.clone());
    for seed in 1..5 {
        cache.insert(identity, seed, pose(seed));
    }
    assert!(cache.bytes <= one * 3);
    assert_eq!(cache.poses.len(), 3);
    assert!(cache.get(identity, 0).is_none(), "the oldest went first");
    assert!(cache.get(identity, 4).is_some());
    assert_eq!(held.shape_seed, 0, "what a ring holds outlives the cache");
}

/// The figures #211 was sized on, with the real oak: run it with
/// `cargo test --release -p flint-procgen a_grove -- --ignored --nocapture`.
/// A debug build is too slow to say anything about a budget.
#[test]
#[ignore = "wants a release build to mean anything"]
fn a_grove_of_forty_keeps_full_within_the_budget() {
    let mut resolver = Grove::new();
    resolver.index_spec(ProcGenSpec::from_file(Path::new(OAK)).unwrap());
    for n in 0..40u64 {
        let mut prop = request(n, &format!("oak_{n:02}"));
        prop.spec_name = "oak_living".into();
        prop.lineage_seed = Some(n % 5);
        resolver.add_living_prop(prop).unwrap();
    }
    let started = Instant::now();
    resolver.make_living_now();
    println!("load: {:.0} ms", started.elapsed().as_secs_f64() * 1e3);

    run(&mut resolver, 60.0, 1.0 / 60.0);
    let stats = resolver.living_props().stats();
    println!("{stats:#?}");
    assert_eq!(stats.frames, 3600);
    assert_eq!(stats.frames_starved, 0);
    assert_eq!(stats.most_short, 0, "{stats:?}");
    // A pose can't be cut short, so the odd slow one runs over.
    assert!(stats.frames_over_budget <= 36, "{stats:?}");
    assert!(stats.worst_ms < 8.0, "{stats:?}");
}

/// A full ring, one sapling prop on `extra`'s settings, and a place to blend.
fn one_prop(extra: &str, defaults: LivingDefaults) -> (Grove, LivingPropId) {
    let mut resolver = resolver_with(&sapling(extra));
    resolver.set_living_defaults(defaults);
    let id = resolver.add_living_prop(request(1, "oak")).unwrap().0;
    resolver.make_living_now();
    (resolver, id)
}

const NO_JITTER: &str = "jitter = { amplitude = 0.0 }";

/// Everything the bones say about a tree: each segment's far end.
fn tips(resolver: &Grove, id: LivingPropId, bones: &[[[f32; 4]; 4]]) -> Vec<[f32; 3]> {
    let (skeleton, rig) = resolver.living_props().rig_of(id).unwrap();
    skeleton
        .segments
        .iter()
        .map(|s| {
            let m = &bones[rig.segment_bones[s.id as usize] as usize];
            let e = s.end.to_array();
            std::array::from_fn(|r| m[0][r] * e[0] + m[1][r] * e[1] + m[2][r] * e[2] + m[3][r])
        })
        .collect()
}

fn farthest(a: &[[f32; 3]], b: &[[f32; 3]]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(a, b)| {
            let d: f32 = (0..3).map(|i| (a[i] - b[i]).powi(2)).sum();
            d.sqrt()
        })
        .fold(0.0, f32::max)
}

/// `seconds` of play at 60 fps: each frame's tips and whether its bones changed.
fn play(resolver: &mut Grove, id: LivingPropId, seconds: f64) -> Vec<(Vec<[f32; 3]>, bool)> {
    let mut frame = BlendFrame::default();
    let first = resolver.living_props_mut().blend_prop(id, &mut frame);
    assert_eq!(first.first_bone, Some(0), "the first blend writes it all");
    let mut bones = frame.bones.clone();
    let mut out = vec![(tips(resolver, id, &bones), true)];
    for _ in 0..(seconds * 60.0) as usize {
        resolver.process_living(1.0 / 60.0);
        let blend = resolver.living_props_mut().blend_prop(id, &mut frame);
        let moved = blend.first_bone == Some(0);
        if moved {
            bones.clone_from(&frame.bones);
        }
        out.push((tips(resolver, id, &bones), moved));
    }
    out
}

/// The most the tree moves between two poses, measured with the bones.
fn pose_gap(resolver: &Grove, id: LivingPropId) -> f32 {
    let props = resolver.living_props();
    let prop = props.prop(id);
    let (skeleton, rig) = props.rig_of(id).unwrap();
    let first = prop.first_step().unwrap();
    (first..first + 3)
        .map(|step| {
            let mut a = Vec::new();
            let mut b = Vec::new();
            rig.palette(skeleton, prop.pose(step).unwrap(), &mut a)
                .unwrap();
            rig.palette(skeleton, prop.pose(step + 1).unwrap(), &mut b)
                .unwrap();
            farthest(&tips(resolver, id, &a), &tips(resolver, id, &b))
        })
        .fold(0.0, f32::max)
}

#[test]
fn at_glide_point_three_nothing_pops_even_across_a_beat() {
    let extra = format!("glide = 0.3\nstep = 0.5\nstagger = \"per_level\"\n{NO_JITTER}");
    let (mut resolver, id) = one_prop(&extra, LivingDefaults::default());
    let gap = pose_gap(&resolver, id);
    assert!(
        gap > 0.01,
        "a sapling that doesn't move tests nothing: {gap}"
    );

    let frames = play(&mut resolver, id, 6.0);
    let worst = frames
        .windows(2)
        .map(|w| farthest(&w[0].0, &w[1].0))
        .fold(0.0, f32::max);
    // A glide takes 0.15 s, 9 frames, and a smoothstep peaks at 1.5 times
    // its mean speed: about a sixth of a pose's distance in a frame. Parents
    // and children glide at different times, so a tip can add a few levels' moves.
    assert!(worst < gap * 0.45, "moved {worst} in a frame of {gap}");
}

#[test]
fn at_glide_zero_it_holds_still_and_then_snaps_on_the_beat() {
    let extra = format!("glide = 0.0\nstep = 0.5\nstagger = \"none\"\n{NO_JITTER}");
    let (mut resolver, id) = one_prop(&extra, LivingDefaults::default());
    let phase = resolver.living_props().prop(id).phase_offset();
    let frames = play(&mut resolver, id, 6.0);

    let mut snaps = 0;
    for (i, w) in frames.windows(2).enumerate() {
        let moved = farthest(&w[0].0, &w[1].0);
        if w[1].1 {
            snaps += 1;
            assert!(moved > 0.0, "an upload that changed nothing, frame {i}");
        } else {
            assert_eq!(moved, 0.0, "frame {i} moved without a beat");
        }
    }
    // 6 s at two beats a second, starting at a phase of {phase}.
    assert!((11..=13).contains(&snaps), "{snaps} snaps at phase {phase}");
}

#[test]
fn per_level_stagger_puts_each_level_at_its_own_place_in_the_cycle() {
    let extra = format!("glide = 1.0\nstagger = \"per_level\"\n{NO_JITTER}");
    let (mut resolver, id) = one_prop(&extra, LivingDefaults::default());
    let mut frame = BlendFrame::default();
    resolver.process_living(0.1);
    resolver.living_props_mut().blend_prop(id, &mut frame);

    let props = resolver.living_props();
    let (skeleton, _) = props.rig_of(id).unwrap();
    let drawn = &props.prop(id).drawn.bones.times;
    let by_depth = |depth: u32| {
        let at = skeleton.segments.iter().position(|s| s.depth == depth);
        drawn[at.expect("the sapling has this level")]
    };
    let shallowest = skeleton.segments.iter().map(|s| s.depth).min().unwrap();
    assert!(skeleton.max_depth() > shallowest, "needs two levels");
    assert_ne!(by_depth(shallowest), by_depth(skeleton.max_depth()));
    // Same depth, same place: only the level matters.
    for (s, time) in skeleton.segments.iter().zip(drawn) {
        assert_eq!(*time, by_depth(s.depth));
    }
}

#[test]
fn the_runtime_and_the_strip_agree_on_every_bone() {
    // The strip blends a stream with `PoseStream`, the player a ring, and
    // both through `LivingPose::blend_from`, so the same poses and clock must
    // give the same bones.
    let extra = format!("stagger = \"per_level\"\n{NO_JITTER}");
    let (mut resolver, id) = one_prop(&extra, LivingDefaults::default());
    let mut frame = BlendFrame::default();
    let mut checked = 0;
    for _ in 0..25 {
        resolver.process_living(0.03);
        let blend = resolver.living_props_mut().blend_prop(id, &mut frame);
        if blend.first_bone != Some(0) {
            continue;
        }
        checked += 1;
        let props = resolver.living_props();
        let prop = props.prop(id);
        let (skeleton, rig) = props.rig_of(id).unwrap();

        let segments = skeleton
            .segments
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let (index, weight) =
                    flint_wooze::warp(prop.beats() + prop.clock.phase(s), prop.clock.glide);
                SegmentPose::blend(
                    &prop.pose(index).unwrap().segments[i],
                    &prop.pose(index + 1).unwrap().segments[i],
                    weight,
                )
            })
            .collect();
        let by_hand = LivingPose {
            shape_seed: 0,
            segments,
        };
        let mut expected = Vec::new();
        rig.palette(skeleton, &by_hand, &mut expected).unwrap();
        assert_eq!(&frame.bones[..expected.len()], &expected[..]);
    }
    assert!(checked > 5, "only {checked} frames moved");
}

#[test]
fn a_blend_uploads_the_whole_palette_with_still_jitter_bones_at_the_end() {
    let (mut resolver, id) = one_prop(NO_JITTER, LivingDefaults::default());
    let mut frame = BlendFrame::default();
    resolver.living_props_mut().blend_prop(id, &mut frame);
    let (_, rig) = resolver.living_props().rig_of(id).unwrap();
    assert_eq!(frame.bones.len(), rig.bone_segments.len() + BOIL_BONES);
    for bone in &frame.bones[rig.bone_segments.len()..] {
        assert_eq!(*bone, [[0.0; 4]; 4], "a still jitter bone adds nothing");
    }
}

#[test]
fn a_paused_clock_costs_no_uploads() {
    let (mut resolver, id) = one_prop("", LivingDefaults::default());
    let mut frame = BlendFrame::default();
    resolver.process_living(0.3);
    assert_eq!(
        resolver
            .living_props_mut()
            .blend_prop(id, &mut frame)
            .first_bone,
        Some(0)
    );
    for _ in 0..10 {
        resolver.process_living(0.0);
        let blend = resolver.living_props_mut().blend_prop(id, &mut frame);
        assert_eq!(blend.first_bone, None);
        assert!(frame.tints.is_empty());
        assert!(!blend.shadow_changed);
    }
}

#[test]
fn jitter_alone_writes_only_its_own_bones() {
    let extra = "glide = 0.0\nstep = 100.0\nstagger = \"none\"\njitter = { amplitude = 0.05 }";
    let (mut resolver, id) = one_prop(extra, LivingDefaults::default());
    let mut frame = BlendFrame::default();
    resolver.living_props_mut().blend_prop(id, &mut frame);
    let tail = resolver
        .living_props()
        .rig_of(id)
        .unwrap()
        .1
        .bone_segments
        .len();

    let mut writes = 0;
    for _ in 0..120 {
        resolver.process_living(1.0 / 60.0);
        if let Some(first) = resolver
            .living_props_mut()
            .blend_prop(id, &mut frame)
            .first_bone
        {
            assert_eq!(first, tail, "a holding pose isn't written again");
            assert_eq!(frame.bones.len(), BOIL_BONES);
            writes += 1;
        }
    }
    // 2 s of 4 channels at 8 beats a second, some of them on one frame.
    assert!((16..=64).contains(&writes), "{writes}");
}

#[test]
fn a_missing_pose_leaves_the_prop_as_it_was_and_is_counted() {
    let (mut resolver, id) = one_prop(NO_JITTER, LivingDefaults::default());
    let mut frame = BlendFrame::default();
    resolver.living_props_mut().blend_prop(id, &mut frame);

    // Past the ring, with no frame budget to have filled it.
    let props = resolver.living_props_mut();
    for prop in &mut props.props {
        prop.beats += 1000.0;
        prop.ring.as_mut().unwrap().slide_to(100_000);
    }
    let blend = props.blend_prop(id, &mut frame);
    assert_eq!(blend.first_bone, None);
    assert_eq!(props.stats().blends_starved, 1);
}

#[test]
fn drift_shadows_move_slower_than_the_prop_and_never_run_out_of_poses() {
    let defaults = LivingDefaults {
        shadows: LivingShadows::Drift,
        ..Default::default()
    };
    let extra = "glide = 0.3\nstep = 0.5\nstagger = \"none\"";
    let mut resolver = resolver_with(&sapling(extra));
    resolver.set_living_defaults(defaults);
    // The smallest ring it can be asked for, to prove the slow pair fits.
    let mut small = request(1, "oak");
    small.pose_ring = Some(2);
    let id = resolver.add_living_prop(small).unwrap().0;
    resolver.make_living_now();
    let (bone_count, tail) = {
        let rig = resolver.living_props().rig_of(id).unwrap().1;
        (
            rig.bone_segments.len() + BOIL_BONES,
            rig.bone_segments.len(),
        )
    };
    let mut frame = BlendFrame::default();
    let (mut shadow_writes, mut pose_writes) = (0, 0);
    for _ in 0..600 {
        resolver.process_living(1.0 / 60.0);
        let blend = resolver.living_props_mut().blend_prop(id, &mut frame);
        pose_writes += usize::from(blend.first_bone == Some(0));
        if blend.shadow_changed {
            shadow_writes += 1;
            assert_eq!(frame.shadow_bones.len(), bone_count);
            for bone in &frame.shadow_bones[tail..] {
                assert_eq!(*bone, [[0.0; 4]; 4], "shadows don't jitter");
            }
        }
    }
    // Half the beats, each with a glide twice as long: about as many writes,
    // but a tree's shadow moves half as often.
    assert!(shadow_writes > 0);
    assert!(
        shadow_writes <= pose_writes + 20,
        "{shadow_writes} {pose_writes}"
    );
    assert_eq!(resolver.living_props().stats().blends_starved, 0);
}

#[test]
fn blended_shadows_cost_nothing_extra() {
    let (mut resolver, id) = one_prop("", LivingDefaults::default());
    let mut frame = BlendFrame::default();
    for _ in 0..60 {
        resolver.process_living(1.0 / 60.0);
        let blend = resolver.living_props_mut().blend_prop(id, &mut frame);
        assert!(!blend.shadow_changed);
    }
}

#[test]
fn an_echo_is_the_props_own_blend_at_an_offset_time() {
    let extra = format!("stagger = \"per_level\"\nechoes = {{ count = 6 }}\n{NO_JITTER}");
    let (mut resolver, id) = one_prop(&extra, LivingDefaults::default());
    assert_eq!(resolver.living_props().prop(id).echo_count(), 6);
    let mut frame = BlendFrame::default();
    let mut checked = 0;
    for _ in 0..40 {
        resolver.process_living(0.03);
        for echo in 0..6 {
            let first = resolver.living_props_mut().blend_echo(id, echo, &mut frame);
            if first != Some(0) {
                continue;
            }
            checked += 1;
            let props = resolver.living_props();
            let prop = props.prop(id);
            let (skeleton, rig) = props.rig_of(id).unwrap();
            let segments = skeleton
                .segments
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let (index, weight) = flint_wooze::warp(
                        prop.echo_beats(echo) + prop.clock.phase(s),
                        prop.clock.glide,
                    );
                    SegmentPose::blend(
                        &prop.pose(index).unwrap().segments[i],
                        &prop.pose(index + 1).unwrap().segments[i],
                        weight,
                    )
                })
                .collect();
            let by_hand = LivingPose {
                shape_seed: 0,
                segments,
            };
            let mut expected = Vec::new();
            rig.palette(skeleton, &by_hand, &mut expected).unwrap();
            assert_eq!(&frame.bones[..expected.len()], &expected[..]);
        }
    }
    assert!(checked > 30, "only {checked} echo blends moved");
}

#[test]
fn an_echo_at_a_place_is_its_blend_moved_there() {
    // Two echoes, a step either side, each already at its place.
    let extra = format!(
            "stagger = \"per_level\"\nechoes = {{ count = 2, spread = 1.0 }}\n\
             echo_places = {{ at = [[0.1, 0.0, -1.0], [0.0, 0.2, 1.0]], from = 0.2, whole = 0.8 }}\n{NO_JITTER}"
        );
    let (mut resolver, id) = one_prop(&extra, LivingDefaults::default());
    let prop = resolver.living_props().prop(id);
    assert_eq!(prop.echo(0).place, [0.1, 0.0]);
    assert_eq!(prop.echo(1).place, [0.0, 0.2]);
    let mut frame = BlendFrame::default();
    let mut checked = 0;
    for _ in 0..40 {
        resolver.process_living(0.03);
        for echo in 0..2 {
            let first = resolver.living_props_mut().blend_echo(id, echo, &mut frame);
            if first != Some(0) {
                continue;
            }
            checked += 1;
            let props = resolver.living_props();
            let prop = props.prop(id);
            let (skeleton, rig) = props.rig_of(id).unwrap();
            let segments = skeleton
                .segments
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let (index, weight) = flint_wooze::warp(
                        prop.echo_beats(echo) + prop.clock.phase(s),
                        prop.clock.glide,
                    );
                    SegmentPose::blend(
                        &prop.pose(index).unwrap().segments[i],
                        &prop.pose(index + 1).unwrap().segments[i],
                        weight,
                    )
                })
                .collect();
            let mut by_hand = LivingPose {
                shape_seed: 0,
                segments,
            };
            // A tree has no place axes of its own: the prop's X and Y.
            skeleton.show_at(&mut by_hand, 1.0, prop.echo(echo).place);
            let mut expected = Vec::new();
            rig.palette(skeleton, &by_hand, &mut expected).unwrap();
            assert_eq!(&frame.bones[..expected.len()], &expected[..]);
        }
    }
    assert!(checked > 10, "only {checked} echo blends moved");
}

#[test]
fn a_ring_has_every_pose_every_echo_wants() {
    for stagger in ["none", "per_level", "per_branch"] {
        // A small ring and the widest spread there is.
        let extra = format!("stagger = \"{stagger}\"\nechoes = {{ count = 6, spread = 1.8 }}");
        let (mut resolver, id) = one_prop(&extra, LivingDefaults::default());
        let mut frame = BlendFrame::default();
        for _ in 0..600 {
            resolver.process_living(1.0 / 60.0);
            let props = resolver.living_props_mut();
            props.blend_prop(id, &mut frame);
            for echo in 0..6 {
                props.blend_echo(id, echo, &mut frame);
            }
        }
        assert_eq!(
            resolver.living_props().stats().blends_starved,
            0,
            "with {stagger} stagger"
        );
    }
}

#[test]
fn drift_shadows_still_find_their_poses_with_echoes_on() {
    let defaults = LivingDefaults {
        shadows: LivingShadows::Drift,
        ..Default::default()
    };
    let extra = "echoes = { count = 6, spread = 1.8 }";
    let (mut resolver, id) = one_prop(extra, defaults);
    let mut frame = BlendFrame::default();
    let mut wrote = 0;
    for _ in 0..600 {
        resolver.process_living(1.0 / 60.0);
        let props = resolver.living_props_mut();
        let prop = props.prop(id);
        let slow = flint_wooze::warp(prop.beats() / 2.0, prop.clock.glide).0;
        let (from, to) = LivingClock::slow_pair(slow);
        assert!(prop.pose(from).is_some() && prop.pose(to).is_some());
        wrote += u32::from(props.blend_prop(id, &mut frame).shadow_changed);
    }
    assert!(wrote > 30, "the shadow only moved {wrote} times");
}

#[test]
fn with_no_echoes_a_ring_is_what_it_was() {
    let off = "echoes = { count = 0 }\nstagger = \"per_level\"";
    let on = "echoes = { count = 6 }\nstagger = \"per_level\"";
    let (mut resolver_off, id_off) = one_prop(off, LivingDefaults::default());
    let (mut resolver_on, id_on) = one_prop(on, LivingDefaults::default());
    run(&mut resolver_off, 3.0, 1.0 / 60.0);
    run(&mut resolver_on, 3.0, 1.0 / 60.0);

    let props = resolver_off.living_props();
    let prop = props.prop(id_off);
    assert_eq!(prop.echo_count(), 0);
    // The window starts at now, as it did before echoes.
    assert_eq!(prop.first_step(), Some(prop.beats().floor() as u64));
    assert_eq!(prop.ring_len(), 4);

    // With them it starts behind, for the farthest one to read.
    let props = resolver_on.living_props();
    let prop = props.prop(id_on);
    assert!(prop.first_step().unwrap() < prop.beats().floor() as u64);
}

#[test]
fn an_echo_costs_nothing_while_the_clock_is_paused() {
    let (mut resolver, id) = one_prop("echoes = { count = 2 }", LivingDefaults::default());
    let mut frame = BlendFrame::default();
    let props = resolver.living_props_mut();
    assert_eq!(props.blend_echo(id, 0, &mut frame), Some(0));
    assert_eq!(props.blend_echo(id, 1, &mut frame), Some(0));
    resolver.process_living(0.0);
    let props = resolver.living_props_mut();
    assert_eq!(props.blend_echo(id, 0, &mut frame), None);
    assert_eq!(props.blend_echo(id, 1, &mut frame), None);
}

#[test]
fn an_echo_with_no_pose_writes_nothing_and_never_part_of_its_bones() {
    // It has no rest pose under it, so jitter alone can't be its first write.
    let extra = "echoes = { count = 2 }\njitter = { amplitude = 0.3, step = 0.01 }";
    let mut resolver = resolver_with(&sapling(extra));
    let id = resolver.add_living_prop(request(1, "oak")).unwrap().0;
    let props = resolver.living_props_mut();
    let mut frame = BlendFrame::default();
    assert_eq!(props.blend_echo(id, 0, &mut frame), None, "no lineage yet");
    resolver.make_living_now();
    let props = resolver.living_props_mut();
    let ring = props.props[id.0].ring.as_mut().unwrap();
    ring.slots.fill(None);
    ring.ready = 0;
    props.time = 0.05;
    assert_eq!(props.blend_echo(id, 0, &mut frame), None);
    assert_eq!(props.stats().blends_starved, 1);
}

#[test]
fn a_scene_overrides_echoes_a_field_at_a_time() {
    let defaults = LivingDefaults {
        echoes: EchoOverride {
            count: Some(2),
            alpha: Some(0.5),
            ..Default::default()
        },
        ..Default::default()
    };
    let (resolver, id) = one_prop("echoes = { count = 6, spread = 0.5 }", defaults);
    let prop = resolver.living_props().prop(id);
    assert_eq!(prop.echo_count(), 2);
    assert_eq!(prop.echo(0).offset, -0.5);
    assert!((prop.echo(0).opacity - 0.5 * (1.0 - 0.5 * 0.55)).abs() < 1e-5);

    let off = LivingDefaults {
        echoes: EchoOverride {
            count: Some(0),
            ..Default::default()
        },
        ..Default::default()
    };
    let (resolver, id) = one_prop("echoes = { count = 6 }", off);
    assert_eq!(resolver.living_props().prop(id).echo_count(), 0);
}

#[test]
fn leaf_tints_are_few_and_only_change_on_their_beat() {
    let spec = std::fs::read_to_string(OAK).unwrap();
    let mut resolver = resolver_with(&spec);
    let mut req = request(1, "oak");
    req.spec_name = "oak_living".into();
    let id = resolver.add_living_prop(req).unwrap().0;
    resolver.make_living_now();

    let mut frame = BlendFrame::default();
    let mut seen = std::collections::HashSet::new();
    let mut changes = 0;
    for _ in 0..240 {
        resolver.process_living(1.0 / 60.0);
        resolver.living_props_mut().blend_prop(id, &mut frame);
        for (part, tint) in &frame.tints {
            assert_ne!(*part, 0, "the bark isn't a leaf");
            seen.insert(tint.map(f32::to_bits));
            changes += 1;
        }
    }
    // 4 s at 8 beats a second on five shades, if none repeats a level.
    assert!(changes > 30 && changes <= 160, "{changes}");
    // Each distinct tint costs a material per shade, for the whole grove.
    assert!(seen.len() <= 9, "{} tints", seen.len());
}

#[test]
fn a_redraw_sends_every_leaf_tint_even_one_on_step_zero() {
    let spec = std::fs::read_to_string(OAK).unwrap();
    let mut resolver = resolver_with(&spec);
    let mut req = request(1, "oak");
    req.spec_name = "oak_living".into();
    let id = resolver.add_living_prop(req).unwrap().0;
    resolver.make_living_now();
    // With no jitter every part sits on step 0, the value a cleared memory of
    // what was sent would read as.
    let still = WoozeField::new(WoozeSample {
        jitter: 0.0,
        ..WoozeSample::NEUTRAL
    });
    let mut frame = BlendFrame::default();
    resolver.process_living_in(1.0 / 60.0, Some((&still, 0.0)));
    let sent = |resolver: &mut Grove, frame: &mut BlendFrame| {
        resolver.living_props_mut().blend_prop(id, frame);
        frame.tints.len()
    };
    let leaves = resolver
        .living_props()
        .rig_of(id)
        .unwrap()
        .1
        .leaf_parts
        .len();
    assert!(leaves > 0);
    sent(&mut resolver, &mut frame);
    assert_eq!(sent(&mut resolver, &mut frame), 0, "nothing new to send");
    resolver.living_props_mut().redraw_all();
    assert_eq!(sent(&mut resolver, &mut frame), leaves, "all of them again");
}

// ---- The wooze field (#213) ----

fn field(global: &str, zones: &[&str]) -> WoozeField {
    let mut field = WoozeField::new(WoozeSample::NEUTRAL);
    field
        .set_global(&global.parse().unwrap(), 0.0, 0.0)
        .unwrap();
    for zone in zones {
        field.add_zone(&zone.parse().unwrap()).unwrap();
    }
    field
}

/// One frame on `field`, with the bones of each prop that moved.
fn tick_with(resolver: &mut Grove, field: &WoozeField, time: f64, dt: f64) {
    resolver.process_living_in(dt, Some((field, time)));
}

const DREAMY: &str = r#"
        name = "pocket"
        shape = "sphere"
        center = [0, 0, 0]
        radius = 3
        falloff = 4
        glide = 0.8
    "#;

/// The Done When: props inside a `glide = 0.8` sphere glide dreamier than
/// those outside, with a smooth falloff between. A prop at glide `g` moves
/// for `g` of its time, so that's what to count.
#[test]
fn a_zone_makes_the_props_inside_it_glide_longer_with_a_falloff_between() {
    let extra = format!("stagger = \"none\"\n{NO_JITTER}");
    let mut resolver = resolver_with(&sapling(&extra));
    let places = [("in", 0.0), ("edge", 5.0), ("out", 12.0)];
    let ids: Vec<_> = places
        .iter()
        .enumerate()
        .map(|(n, (name, x))| {
            let mut prop = request(n as u64, name);
            prop.position = [*x, 0.0, 0.0];
            resolver.add_living_prop(prop).unwrap().0
        })
        .collect();
    resolver.make_living_now();
    let field = field("step = 0.5\nglide = 0.3", &[DREAMY]);

    let mut frame = BlendFrame::default();
    let mut moving = [0usize; 3];
    let frames = 600;
    for f in 0..frames {
        let time = f64::from(f) / 60.0;
        tick_with(&mut resolver, &field, time, 1.0 / 60.0);
        for (i, id) in ids.iter().enumerate() {
            let blend = resolver.living_props_mut().blend_prop(*id, &mut frame);
            moving[i] += usize::from(blend.first_bone == Some(0));
        }
    }
    // The frame of each beat moves too, one in the 30 of a step.
    let share = |i: usize| moving[i] as f32 / frames as f32 - 1.0 / 30.0;
    assert!((share(0) - 0.8).abs() < 0.04, "inside {}", share(0));
    // Halfway down the falloff, it's halfway between the two.
    assert!((share(1) - 0.55).abs() < 0.04, "falloff {}", share(1));
    assert!((share(2) - 0.3).abs() < 0.04, "outside {}", share(2));
}

/// The tips' largest move on any frame of `seconds` of play, where `change`
/// runs on the frame it's given.
fn largest_move(
    resolver: &mut Grove,
    id: LivingPropId,
    field: &mut WoozeField,
    seconds: f64,
    change_at: usize,
    change: impl Fn(&mut WoozeField, f64),
) -> (f32, usize) {
    let mut frame = BlendFrame::default();
    let mut time = 0.0;
    tick_with(resolver, field, time, 1.0 / 60.0);
    resolver.living_props_mut().blend_prop(id, &mut frame);
    let mut bones = frame.bones.clone();
    let mut worst = (0.0_f32, 0);
    for f in 0..(seconds * 60.0) as usize {
        if f == change_at {
            change(field, time);
        }
        time += 1.0 / 60.0;
        tick_with(resolver, field, time, 1.0 / 60.0);
        let blend = resolver.living_props_mut().blend_prop(id, &mut frame);
        if blend.first_bone == Some(0) {
            let moved = farthest(
                &tips(resolver, id, &bones),
                &tips(resolver, id, &frame.bones),
            );
            if moved > worst.0 {
                worst = (moved, f);
            }
            bones.clone_from(&frame.bones);
        }
    }
    worst
}

/// The risk in the ticket. Going from a long glide to a short one mid-cycle
/// computed from absolute time would jump the eased weight, and a new step
/// would jump t' itself. Neither shows here.
#[test]
fn a_change_of_step_or_glide_at_any_moment_never_pops() {
    let extra = format!("stagger = \"per_level\"\n{NO_JITTER}");
    let run = |change: fn(&mut WoozeField, f64), change_at: usize| {
        let (mut resolver, id) = one_prop(&extra, LivingDefaults::default());
        let mut field = field("step = 0.5\nglide = 0.5", &[]);
        let gap = pose_gap(&resolver, id);
        let worst = largest_move(&mut resolver, id, &mut field, 6.0, change_at, change);
        assert!(worst.0 < gap * 0.45, "moved {worst:?} of a gap of {gap}");
    };
    // A spread of moments, so one lands in a hold, a glide and on a beat.
    for change_at in [60, 71, 83, 95, 107, 119] {
        run(
            |f, t| {
                f.set_global(&"step = 5.0".parse().unwrap(), t, 0.0)
                    .unwrap()
            },
            change_at,
        );
        run(
            |f, t| {
                f.set_global(&"glide = 1.0".parse().unwrap(), t, 0.0)
                    .unwrap()
            },
            change_at,
        );
        run(
            |f, t| {
                f.set_global(&"glide = 0.2".parse().unwrap(), t, 0.0)
                    .unwrap()
            },
            change_at,
        );
        run(
            |f, t| {
                f.set_global(&"amplitude = 0.0".parse().unwrap(), t, 0.0)
                    .unwrap()
            },
            change_at,
        );
    }
}

/// The Done When: a script changes the global step at runtime and every prop
/// follows on its next cycle, and the clock doesn't jump to get there.
#[test]
fn a_new_global_step_reaches_every_prop_by_its_next_cycle() {
    let mut resolver = resolver_with(&sapling(NO_JITTER));
    let ids: Vec<_> = (0..4)
        .map(|n| {
            let mut prop = request(n, &format!("tree_{n}"));
            prop.position = [n as f32 * 20.0, 0.0, 0.0];
            resolver.add_living_prop(prop).unwrap().0
        })
        .collect();
    resolver.make_living_now();
    let mut field = field("step = 0.5", &[]);

    let dt = 1.0 / 60.0;
    let mut time = 0.0;
    for _ in 0..60 {
        time += dt;
        tick_with(&mut resolver, &field, time, dt);
    }
    let before: Vec<f64> = ids
        .iter()
        .map(|id| resolver.living_props().prop(*id).beats())
        .collect();
    field
        .set_global(&"step = 2.0".parse().unwrap(), time, 0.0)
        .unwrap();
    time += dt;
    tick_with(&mut resolver, &field, time, dt);
    for (id, before) in ids.iter().zip(&before) {
        let prop = resolver.living_props().prop(*id);
        assert_eq!(prop.clock().step, 2.0);
        // One frame of the new step, from where it stood: bent, not jumped.
        let moved = prop.beats() - before;
        assert!((moved - dt / 2.0).abs() < 1e-9, "{moved}");
    }

    // A full cycle now takes the new step's 2 s, for every prop.
    let start: Vec<f64> = ids
        .iter()
        .map(|id| resolver.living_props().prop(*id).beats())
        .collect();
    for _ in 0..120 {
        time += dt;
        tick_with(&mut resolver, &field, time, dt);
    }
    for (id, start) in ids.iter().zip(&start) {
        let beats = resolver.living_props().prop(*id).beats();
        assert!((beats - start - 1.0).abs() < 1e-9, "{}", beats - start);
    }
}

/// An echo is a number of steps from its prop, so a new step moves them both
/// the same way, and each of its parts has its own cycle to hold on to.
#[test]
fn an_echo_keeps_its_distance_and_never_pops_through_a_change() {
    let extra = format!("stagger = \"per_level\"\nechoes = {{ count = 4 }}\n{NO_JITTER}");
    let (mut resolver, id) = one_prop(&extra, LivingDefaults::default());
    let gap = pose_gap(&resolver, id);
    let mut field = field("step = 0.5\nglide = 0.5", &[]);
    let offset = |r: &Grove| {
        let prop = r.living_props().prop(id);
        prop.echo_beats(3) - prop.beats()
    };
    let before = offset(&resolver);

    let dt = 1.0 / 60.0;
    let mut frame = BlendFrame::default();
    let mut time = 0.0;
    let mut bones: Option<Vec<[[f32; 4]; 4]>> = None;
    let mut worst = 0.0_f32;
    for f in 0..360 {
        if f == 95 {
            field
                .set_global(
                    &"step = 2.0\nglide = 1.0\namplitude = 0.2".parse().unwrap(),
                    time,
                    0.0,
                )
                .unwrap();
        }
        time += dt;
        tick_with(&mut resolver, &field, time, dt);
        let props = resolver.living_props_mut();
        props.blend_prop(id, &mut frame);
        if props.blend_echo(id, 3, &mut frame) == Some(0) {
            if let Some(last) = &bones {
                let moved = farthest(
                    &tips(&resolver, id, last),
                    &tips(&resolver, id, &frame.bones),
                );
                worst = worst.max(moved);
            }
            bones = Some(frame.bones.clone());
        }
    }
    assert!((offset(&resolver) - before).abs() < 1e-9);
    assert!(worst > 0.0, "the echo never moved");
    assert!(worst < gap * 0.45, "moved {worst} of a gap of {gap}");
    assert_eq!(resolver.living_props().stats().blends_starved, 0);
}

#[test]
fn amplitude_is_how_far_from_rest_a_prop_strays() {
    let extra = format!("stagger = \"none\"\n{NO_JITTER}");
    let spread = |amplitude: &str| {
        let (mut resolver, id) = one_prop(&extra, LivingDefaults::default());
        let field = field(amplitude, &[]);
        tick_with(&mut resolver, &field, 0.0, 1.0 / 60.0);
        let mut frame = BlendFrame::default();
        resolver.living_props_mut().blend_prop(id, &mut frame);
        let (skeleton, rig) = resolver.living_props().rig_of(id).unwrap();
        let rest = LivingPose {
            shape_seed: 0,
            segments: vec![SegmentPose::IDENTITY; skeleton.segments.len()],
        };
        let mut rest_bones = Vec::new();
        rig.palette(skeleton, &rest, &mut rest_bones).unwrap();
        farthest(
            &tips(&resolver, id, &rest_bones),
            &tips(&resolver, id, &frame.bones),
        )
    };
    let none = spread("amplitude = 0.0");
    let half = spread("amplitude = 0.5");
    let all = spread("amplitude = 1.0");
    assert!(none < 1e-4, "{none}");
    assert!(half > none && all > half, "{none} {half} {all}");
}

#[test]
fn the_jitter_knob_scales_the_leaves_and_keeps_to_the_same_few_tints() {
    let spec = std::fs::read_to_string(OAK).unwrap();
    let turning = |jitter: &str| {
        let mut resolver = resolver_with(&spec);
        let mut req = request(1, "oak");
        req.spec_name = "oak_living".into();
        let id = resolver.add_living_prop(req).unwrap().0;
        resolver.make_living_now();
        let field = field(jitter, &[]);
        let mut frame = BlendFrame::default();
        let mut seen = std::collections::HashSet::new();
        let mut biggest = 0.0_f32;
        for f in 0..120 {
            tick_with(&mut resolver, &field, f64::from(f) / 60.0, 1.0 / 60.0);
            resolver.living_props_mut().blend_prop(id, &mut frame);
            // A first blend sends every part's tint, the identity included.
            for (_, tint) in frame.tints.iter().filter(|(_, tint)| *tint != [1.0; 3]) {
                seen.insert(tint.map(f32::to_bits));
            }
            let t = resolver.living_props().prop(id).jitter_ahead + resolver.living_props().time();
            let jitter_now = resolver
                .living_props()
                .prop(id)
                .jitter()
                .scaled(resolver.living_props().prop(id).wooze().jitter);
            biggest = biggest.max(jitter_now.card_turn(1, 0, t).abs());
        }
        (seen.len(), biggest)
    };
    let (none_tints, none_turn) = turning("jitter = 0.0");
    assert_eq!((none_tints, none_turn), (0, 0.0), "no jitter, no boil");
    let (one_tints, one_turn) = turning("jitter = 1.0");
    let (more_tints, more_turn) = turning("jitter = 4.0");
    assert!(one_tints > 0 && one_tints <= 9, "{one_tints}");
    assert!(more_tints <= 9, "{more_tints} would be a material each");
    assert!(more_turn > one_turn, "{more_turn} {one_turn}");
}

#[test]
fn a_prop_that_moves_reads_the_field_where_it_has_got_to() {
    let (mut resolver, id) = one_prop(NO_JITTER, LivingDefaults::default());
    let far = [50.0, 0.0, 0.0];
    resolver.living_props_mut().track_positions(|_| Some(far));
    let field = field("glide = 0.3", &[DREAMY]);
    tick_with(&mut resolver, &field, 0.0, 1.0 / 60.0);
    assert_eq!(resolver.living_props().prop(id).wooze().glide, 0.3);

    resolver
        .living_props_mut()
        .track_positions(|_| Some([0.0; 3]));
    tick_with(&mut resolver, &field, 0.1, 1.0 / 60.0);
    assert_eq!(resolver.living_props().prop(id).wooze().glide, 0.8);
    assert_eq!(resolver.living_props().prop(id).clock().glide, 0.8);
}

#[test]
fn with_no_field_a_prop_keeps_to_its_spec() {
    let (mut resolver, id) = one_prop("step = 0.25\nglide = 0.9", LivingDefaults::default());
    for _ in 0..30 {
        resolver.process_living(1.0 / 60.0);
    }
    let prop = resolver.living_props().prop(id);
    assert_eq!((prop.clock().step, prop.clock().glide), (0.25, 0.9));
    assert_eq!(prop.wooze().amplitude, 1.0);
}

#[test]
fn a_field_beats_the_spec_wherever_it_covers() {
    let (mut resolver, id) = one_prop("step = 0.25\nglide = 0.9", LivingDefaults::default());
    let field = field("step = 2.0\nglide = 0.0", &[]);
    tick_with(&mut resolver, &field, 0.0, 1.0 / 60.0);
    let clock = resolver.living_props().prop(id).clock();
    assert_eq!((clock.step, clock.glide), (2.0, 0.0));
}

// ---- Species (#246) ----

/// The sapling under another name, and nothing else changed.
fn renamed(spec: &str, name: &str) -> String {
    spec.replace("name = \"sapling\"", &format!("name = \"{name}\""))
}

/// Two saplings and two thorns, with the scene's species values, built.
fn grove(defaults: LivingDefaults) -> Grove {
    let spec = sapling("echoes = { count = 1, spread = 0.5 }");
    let mut resolver = resolver_with(&spec);
    resolver.index_spec(ProcGenSpec::parse(&renamed(&spec, "thorn")).unwrap());
    resolver.set_living_defaults(defaults);
    for (i, species) in ["sapling", "sapling", "thorn", "thorn"].iter().enumerate() {
        let mut asked = request(i as u64 + 1, &format!("{species}_{i}"));
        asked.spec_name = species.to_string();
        asked.position = [i as f32 * 10.0, 0.0, 0.0];
        resolver.add_living_prop(asked).unwrap();
    }
    resolver.make_living_now();
    resolver
}

fn thorny() -> LivingDefaults {
    let mut defaults = LivingDefaults::default();
    defaults.species.insert(
        "thorn".to_string(),
        SpeciesOverride {
            clock: ClockOverride {
                step: Some(0.25),
                ..Default::default()
            },
            stagger: Some(Stagger::None),
            echoes: EchoOverride {
                count: Some(3),
                ..Default::default()
            },
            variance: VarianceOverride {
                amount: Some(0.9),
                ..Default::default()
            },
            softness: None,
            jitter_rate: None,
            row_rate: None,
        },
    );
    defaults
}

#[test]
fn softness_layers_from_the_spec_up_and_changes_no_pose() {
    // The spec's 0 is a hard edge.
    let mut resolver = grove(LivingDefaults {
        softness: Some(0.25),
        ..LivingDefaults::default()
    });
    let field = field("", &[]);
    tick_with(&mut resolver, &field, 0.0, 1.0 / 60.0);
    let before = resolver.living_props().stats();
    let props = resolver.living_props_mut();
    let read = |props: &LivingProps, i: usize| {
        props
            .resolved(LivingPropId(i), None)
            .get(LivingKnob::Softness)
            .clone()
    };
    assert_eq!(read(props, 0).value, 0.25, "the scene's beats the spec's");
    assert_eq!(read(props, 0).layer, LivingLayer::Global);
    assert_eq!(props.global_softness(), Some(0.25));

    // A species of its own beats the grove's, and the grove's then moved live
    // leaves it standing.
    assert!(props.tune_softness(LivingScope::Species("thorn"), 0.8));
    assert!(props.tune_softness(LivingScope::All, 0.5));
    assert_eq!(props.softness(LivingPropId(0)), 0.5);
    assert_eq!(props.softness(LivingPropId(2)), 0.8);
    assert_eq!(read(props, 2).layer, LivingLayer::Species);
    // Out of range is held to it.
    assert!(props.tune_softness(LivingScope::All, 3.0));
    assert_eq!(props.softness(LivingPropId(1)), 1.0);
    assert!(!props.tune_softness(LivingScope::Species("fern"), 0.5));

    // As loaded puts every layer back.
    let layers = LivingLayers {
        softnesses: vec![None; 4],
        ..props.layers()
    };
    props.restore_layers(&layers);
    assert_eq!(props.softness(LivingPropId(0)), 0.0, "the spec's again");
    assert_eq!(read(props, 0).layer, LivingLayer::Spec);
    assert_eq!(props.softness(LivingPropId(2)), 0.8, "a species' own stays");

    // Nothing about the poses moved: softness is how the body draws.
    assert_eq!(resolver.living_props().stats(), before);
}

#[test]
fn a_species_value_beats_the_field_and_its_zones_and_no_other_species_sees_it() {
    let mut resolver = grove(thorny());
    // A zone over all four that slows the step, and says glide too.
    let field = field(
        "glide = 0.5",
        &[r#"name = "slow"
                shape = "sphere"
                center = [15, 0, 0]
                radius = 40
                step = 1.0"#],
    );
    tick_with(&mut resolver, &field, 0.0, 1.0 / 60.0);
    let props = resolver.living_props();
    let at = |i: usize| props.resolved(LivingPropId(i), Some((&field, 0.0)));

    let sapling = at(0);
    assert_eq!(sapling.species, "sapling");
    assert_eq!(sapling.get(LivingKnob::Step).value, 1.0);
    assert_eq!(
        sapling.get(LivingKnob::Step).layer,
        LivingLayer::Zone("slow".into())
    );
    assert_eq!(sapling.get(LivingKnob::Glide).layer, LivingLayer::Global);
    assert_eq!(sapling.get(LivingKnob::EchoCount).value, 1);
    assert_eq!(sapling.get(LivingKnob::EchoCount).layer, LivingLayer::Spec);
    assert_eq!(sapling.get(LivingKnob::Stagger).value, Stagger::PerLevel);
    assert_eq!(sapling.get(LivingKnob::VarianceAmount).value, 0.5);

    let thorn = at(2);
    assert_eq!(
        thorn.get(LivingKnob::Step).value,
        0.25,
        "the species beats the zone"
    );
    assert_eq!(thorn.get(LivingKnob::Step).layer, LivingLayer::Species);
    assert_eq!(
        thorn.get(LivingKnob::Glide).value,
        0.5,
        "and leaves the rest to the field"
    );
    assert_eq!(props.prop(LivingPropId(2)).clock().step, 0.25);
    assert_eq!(thorn.get(LivingKnob::EchoCount).value, 3);
    assert_eq!(thorn.get(LivingKnob::EchoSpread).value, 0.5);
    assert_eq!(thorn.get(LivingKnob::EchoCount).layer, LivingLayer::Species);
    assert_eq!(thorn.get(LivingKnob::EchoSpread).layer, LivingLayer::Spec);
    assert_eq!(thorn.get(LivingKnob::Stagger).value, Stagger::None);
    assert_eq!(thorn.get(LivingKnob::VarianceAmount).value, 0.9);
    assert_eq!(
        thorn.get(LivingKnob::VarianceSteady).layer,
        LivingLayer::Spec
    );
    assert_eq!(props.prop(LivingPropId(3)).echo_count(), 3);
    assert_eq!(
        props.variances(),
        vec![
            VarianceConfig {
                amount: 0.5,
                steady: 0.5,
                keep_size: false
            },
            VarianceConfig {
                amount: 0.9,
                steady: 0.5,
                keep_size: false
            },
        ]
    );
}

#[test]
fn a_global_knob_moved_live_leaves_explicit_species_values_standing() {
    let mut resolver = grove(thorny());
    let mut field = field("", &[]);
    tick_with(&mut resolver, &field, 0.0, 1.0 / 60.0);

    // Every global knob a person can move, moved.
    field
        .set_global(&"step = 2.0\nglide = 0.9".parse().unwrap(), 0.0, 0.0)
        .unwrap();
    let props = resolver.living_props_mut();
    props.set_echoes(EchoConfig {
        count: 6,
        spread: 1.0,
        alpha: 0.4,
    });
    props.set_stagger(Stagger::PerBranch);
    props.set_variance(VarianceConfig {
        amount: 0.2,
        steady: 0.1,
        keep_size: true,
    });
    tick_with(&mut resolver, &field, 0.0, 1.0 / 60.0);

    let props = resolver.living_props();
    let sapling = props.resolved(LivingPropId(0), Some((&field, 0.0)));
    assert_eq!(sapling.get(LivingKnob::Step).value, 2.0);
    assert_eq!(sapling.get(LivingKnob::Glide).value, 0.9);
    assert_eq!(sapling.get(LivingKnob::EchoCount).value, 6);
    assert_eq!(
        sapling.get(LivingKnob::EchoCount).layer,
        LivingLayer::Global
    );
    assert_eq!(sapling.get(LivingKnob::Stagger).value, Stagger::PerBranch);
    assert_eq!(sapling.get(LivingKnob::VarianceAmount).value, 0.2);

    let thorn = props.resolved(LivingPropId(2), Some((&field, 0.0)));
    assert_eq!(
        thorn.get(LivingKnob::Step).value,
        0.25,
        "the species' step stood"
    );
    assert_eq!(
        thorn.get(LivingKnob::Glide).value,
        0.9,
        "what it doesn't set follows"
    );
    assert_eq!(thorn.get(LivingKnob::EchoCount).value, 3);
    assert_eq!(thorn.get(LivingKnob::EchoSpread).value, 1.0);
    assert_eq!(thorn.get(LivingKnob::EchoSpread).layer, LivingLayer::Global);
    assert_eq!(thorn.get(LivingKnob::Stagger).value, Stagger::None);
    assert_eq!(thorn.get(LivingKnob::VarianceAmount).value, 0.9);
    assert_eq!(thorn.get(LivingKnob::VarianceSteady).value, 0.1);

    // Cleared, the species takes what the grove has.
    let props = resolver.living_props_mut();
    assert!(props.set_species("thorn", SpeciesOverride::default()));
    tick_with(&mut resolver, &field, 0.0, 1.0 / 60.0);
    let thorn = resolver
        .living_props()
        .resolved(LivingPropId(2), Some((&field, 0.0)));
    assert_eq!(thorn.get(LivingKnob::Step).value, 2.0);
    assert_eq!(thorn.get(LivingKnob::EchoCount).value, 6);
    assert_eq!(thorn.get(LivingKnob::Stagger).value, Stagger::PerBranch);
    assert_eq!(thorn.get(LivingKnob::VarianceAmount).value, 0.2);
}

#[test]
fn a_species_scope_reaches_its_own_props_and_lineages_only() {
    let mut resolver = grove(LivingDefaults::default());
    let props = resolver.living_props_mut();
    let echoes = EchoOverride {
        alpha: Some(0.05),
        ..Default::default()
    };
    assert!(props.tune_echoes(LivingScope::Species("thorn"), echoes));
    assert!(props.tune_stagger(LivingScope::Species("thorn"), Stagger::PerBranch));
    let variance = VarianceOverride {
        keep_size: Some(true),
        ..Default::default()
    };
    assert!(props.tune_variance(LivingScope::Species("thorn"), variance));
    assert!(props.tune_clock(
        "thorn",
        ClockOverride {
            glide: Some(0.0),
            ..Default::default()
        }
    ));
    let alphas: Vec<f32> = props
        .props()
        .iter()
        .map(|p| p.echo_config().alpha)
        .collect();
    let spec_alpha = EchoConfig::default().alpha;
    assert_eq!(alphas, [spec_alpha, spec_alpha, 0.05, 0.05]);
    let staggers: Vec<Stagger> = props.props().iter().map(|p| p.stagger()).collect();
    assert_eq!(staggers[..2], [Stagger::PerLevel; 2]);
    assert_eq!(staggers[2..], [Stagger::PerBranch; 2]);
    let keeps: Vec<bool> = props.variances().iter().map(|v| v.keep_size).collect();
    assert_eq!(keeps, [false, true]);
    // With no field, a species' clock goes over its spec's at once.
    let glides: Vec<f32> = props.props().iter().map(|p| p.clock().glide).collect();
    assert_eq!(glides, [0.3, 0.3, 0.0, 0.0]);

    // A species with no props keeps what it's given, for none of them.
    assert!(!props.tune_echoes(LivingScope::Species("birch"), echoes));
    assert_eq!(props.species_override("birch").unwrap().echoes, echoes);
    assert!(!props.has_species("birch"));
    assert!(props.has_species("thorn"));
}

#[test]
fn with_no_species_values_every_layer_is_what_it_was() {
    let resolver = grove(LivingDefaults::default());
    let props = resolver.living_props();
    for i in 0..4 {
        let at = props.resolved(LivingPropId(i), None);
        for layer in [
            &at.get(LivingKnob::Amplitude).layer,
            &at.get(LivingKnob::Step).layer,
            &at.get(LivingKnob::Glide).layer,
            &at.get(LivingKnob::Jitter).layer,
            &at.get(LivingKnob::Stagger).layer,
            &at.get(LivingKnob::EchoCount).layer,
            &at.get(LivingKnob::VarianceAmount).layer,
            &at.get(LivingKnob::KeepSize).layer,
        ] {
            assert_eq!(*layer, LivingLayer::Spec);
        }
        assert_eq!(
            props.prop(LivingPropId(i)).wooze(),
            props.prop(LivingPropId(i)).wooze_below()
        );
    }
}

#[test]
fn an_entity_stagger_is_its_own_layer_under_the_global_one() {
    let spec = sapling("");
    let mut resolver = resolver_with(&spec);
    let mut asked = request(1, "own");
    asked.stagger = Some(Stagger::None);
    let id = resolver.add_living_prop(asked).unwrap().0;
    resolver.make_living_now();
    let props = resolver.living_props_mut();
    assert_eq!(
        props.resolved(id, None).get(LivingKnob::Stagger).layer,
        LivingLayer::Entity
    );
    props.set_stagger(Stagger::PerBranch);
    assert_eq!(
        props.resolved(id, None).get(LivingKnob::Stagger).layer,
        LivingLayer::Global
    );
    props.tune_stagger(LivingScope::Species("sapling"), Stagger::PerLevel);
    let stagger = props.resolved(id, None).get(LivingKnob::Stagger).clone();
    assert_eq!(stagger.value, Stagger::PerLevel);
    assert_eq!(stagger.layer, LivingLayer::Species);
}

#[test]
fn a_clock_given_back_live_keeps_the_species_step_over_it() {
    let mut resolver = grove(thorny());
    let props = resolver.living_props_mut();
    let id = LivingPropId(2);
    let back = WoozeSample {
        step: 0.7,
        ..WoozeSample::NEUTRAL
    };
    props.set_wooze_of(id, back);
    assert_eq!(props.prop(id).clock().step, 0.25);
    assert_eq!(props.prop(id).wooze_below(), back);
    assert_eq!(
        props.resolved(id, None).get(LivingKnob::Glide).layer,
        LivingLayer::Global
    );
    props.set_wooze_of(LivingPropId(0), back);
    assert_eq!(props.prop(LivingPropId(0)).clock().step, 0.7);
}

#[test]
fn layers_put_back_are_the_grove_as_it_was() {
    let mut resolver = grove(thorny());
    let start = resolver.living_props().layers();
    let configs = |r: &Grove| -> Vec<(EchoConfig, Stagger, f32)> {
        let props = r.living_props();
        props
            .props()
            .iter()
            .map(|p| (p.echo_config(), p.stagger(), p.clock().step))
            .collect()
    };
    let before = (configs(&resolver), resolver.living_props().variances());
    let props = resolver.living_props_mut();
    props.set_echoes(EchoConfig::default());
    props.set_stagger(Stagger::PerBranch);
    props.set_species("thorn", SpeciesOverride::default());
    props.tune_clock(
        "sapling",
        ClockOverride {
            step: Some(3.0),
            ..Default::default()
        },
    );
    assert_ne!(configs(&resolver), before.0);
    resolver.living_props_mut().restore_layers(&start);
    assert_eq!(
        (configs(&resolver), resolver.living_props().variances()),
        before
    );
}

/// A fair side-by-side pair is a copy of a spec under a new name with the same
/// lineage seed: its own species, and the same tree. Its poses come from the
/// prop's name and the step, so the same name grows the very same pose.
#[test]
fn a_copy_of_a_spec_under_a_new_name_grows_the_same_skeleton() {
    let spec = sapling("");
    let mut resolver = resolver_with(&spec);
    resolver.index_spec(ProcGenSpec::parse(&renamed(&spec, "sapling_twin")).unwrap());
    let mut twin = request(2, "same_name");
    twin.spec_name = "sapling_twin".to_string();
    let a = resolver.add_living_prop(request(1, "same_name")).unwrap();
    let b = resolver.add_living_prop(twin).unwrap();
    assert_eq!(a.1, "sapling@7");
    assert_eq!(b.1, "sapling_twin@7", "a mesh of its own");
    resolver.make_living_now();
    let props = resolver.living_props();
    assert_eq!(props.lineage_count(), 2);
    let (skeleton_a, rig_a) = props.rig_of(a.0).unwrap();
    let (skeleton_b, rig_b) = props.rig_of(b.0).unwrap();
    assert_eq!(**skeleton_a, **skeleton_b);
    assert_eq!(**rig_a, **rig_b);
    assert_eq!(props.rest_height(a.0), props.rest_height(b.0));
    let first = props.prop(a.0).first_step().unwrap();
    for step in first..first + props.prop(a.0).ring_len() as u64 {
        assert_eq!(props.prop(a.0).pose(step), props.prop(b.0).pose(step));
    }
    assert_eq!(props.species_of(b.0), "sapling_twin");
}

/// Every channel of a prop's boil as it shows now: each card turn.
fn boil_now(props: &LivingProps, id: LivingPropId) -> Vec<f32> {
    let prop = props.prop(id);
    let jitter = prop.jitter().scaled(prop.wooze().jitter);
    let t = prop.jitter_now();
    (0..flint_procgen::living::JITTER_CHANNELS)
        .map(|channel| jitter.card_turn(prop.stream, channel, t))
        .collect()
}

/// A snapping boil on the spec's 0.125 s, big enough to read.
const BOILING: &str = "jitter = { amplitude = 0.5, step = 0.125, glide = 0.0 }";

/// For each channel, the seconds from now to its next change, frame by frame at
/// 240 fps, ticking `resolver` through them.
fn next_ticks(resolver: &mut Grove, id: LivingPropId, seconds: f64) -> Vec<f64> {
    let dt = 1.0 / 240.0;
    let start = boil_now(resolver.living_props(), id);
    let mut found: Vec<Option<f64>> = vec![None; start.len()];
    for frame in 1..=(seconds / dt) as usize {
        resolver.process_living(dt);
        let now = boil_now(resolver.living_props(), id);
        for (channel, at) in found.iter_mut().enumerate() {
            if at.is_none() && now[channel] != start[channel] {
                *at = Some(frame as f64 * dt);
            }
        }
    }
    found
        .into_iter()
        .map(|at| at.expect("every channel ticks"))
        .collect()
}

#[test]
fn an_untuned_boil_reads_the_props_clock_to_the_bit() {
    // A scene that never sets a rate is byte-identical to one from before
    // #529: its jitter reads the very seconds the props' clock holds, whatever
    // the frame times, and so does one tuned to exactly 1.
    let (mut resolver, id) = one_prop(BOILING, LivingDefaults::default());
    let (mut tuned, tuned_id) = one_prop(BOILING, LivingDefaults::default());
    tuned
        .living_props_mut()
        .tune_jitter_rate(LivingScope::All, 1.0);
    let dts = [1.0 / 60.0, 1.0 / 144.0, 0.0331, 0.0, 1.0 / 7.0];
    for frame in 0..400 {
        let dt = dts[frame % dts.len()];
        for r in [&mut resolver, &mut tuned] {
            r.process_living(dt);
            if frame % 50 == 0 {
                r.living_props_mut().advance_beats(1.0, 0.5);
            }
        }
    }
    for (r, id) in [(&resolver, id), (&tuned, tuned_id)] {
        let props = r.living_props();
        assert_eq!(
            props.prop(id).jitter_time().to_bits(),
            props.time().to_bits()
        );
        assert_eq!(props.jitter_rate(id), 1.0);
    }
    assert_eq!(
        boil_now(resolver.living_props(), id),
        boil_now(tuned.living_props(), tuned_id)
    );
}

#[test]
fn a_jitter_rate_halves_and_doubles_how_often_the_cards_turn() {
    // How many times each channel changes over four seconds at 240 fps, which
    // is finer than any rate here ticks.
    let changes = |rate: f32| {
        let (mut resolver, id) = one_prop(BOILING, LivingDefaults::default());
        resolver
            .living_props_mut()
            .tune_jitter_rate(LivingScope::All, rate);
        let mut last = boil_now(resolver.living_props(), id);
        let mut count = 0usize;
        for _ in 0..960 {
            resolver.process_living(1.0 / 240.0);
            let now = boil_now(resolver.living_props(), id);
            count += last.iter().zip(&now).filter(|(a, b)| a != b).count();
            last = now;
        }
        count
    };
    let channels = flint_procgen::living::JITTER_CHANNELS;
    // 4 s on a 0.125 s step is 32 ticks a channel; one more or less at the ends.
    let near = |count: usize, ticks: usize| count.abs_diff(ticks * channels) <= channels;
    let (normal, double, half) = (changes(1.0), changes(2.0), changes(0.5));
    assert!(near(normal, 32), "{normal} at the spec's pace");
    assert!(near(double, 64), "{double} at twice it");
    assert!(near(half, 16), "{half} at half");
    assert_eq!(changes(0.0), 0, "a rate of 0 holds the boil");
}

#[test]
fn a_new_jitter_rate_takes_effect_at_each_cards_next_tick() {
    // Two props on the same clocks, a second into play. One goes on at the
    // spec's pace, the other at a new rate. At the change nothing moves, and
    // each channel holds what it shows until its next tick, which comes after
    // the time it had left over the rate.
    for rate in [2.0f32, 0.5] {
        let (mut steady, a) = one_prop(BOILING, LivingDefaults::default());
        let (mut changed, b) = one_prop(BOILING, LivingDefaults::default());
        for r in [&mut steady, &mut changed] {
            for _ in 0..60 {
                r.process_living(1.0 / 60.0);
            }
        }
        let before = boil_now(changed.living_props(), b);
        assert_eq!(before, boil_now(steady.living_props(), a));
        changed
            .living_props_mut()
            .tune_jitter_rate(LivingScope::All, rate);
        assert_eq!(
            boil_now(changed.living_props(), b),
            before,
            "no card turns at the moment the rate changes"
        );
        let left = next_ticks(&mut steady, a, 1.0);
        let now = next_ticks(&mut changed, b, 1.0);
        for (left, now) in left.iter().zip(&now) {
            let want = left / f64::from(rate);
            // Within one 240 fps frame either way.
            assert!(
                (now - want).abs() <= 1.0 / 240.0 + 1e-9,
                "x{rate}: ticked after {now} s, not {want} ({left} at the old pace)"
            );
        }
    }
}

#[test]
fn a_jitter_rate_layers_like_softness_and_makes_no_pose() {
    let mut resolver = grove(LivingDefaults {
        jitter_rate: Some(1.5),
        ..LivingDefaults::default()
    });
    let field = field("", &[]);
    tick_with(&mut resolver, &field, 0.0, 1.0 / 60.0);
    let props = resolver.living_props_mut();
    let read = |props: &LivingProps, i: usize| {
        props
            .resolved(LivingPropId(i), None)
            .get(LivingKnob::JitterRate)
            .clone()
    };
    assert_eq!(read(props, 0).value, 1.5, "the scene's");
    assert_eq!(read(props, 0).layer, LivingLayer::Global);
    assert_eq!(props.global_jitter_rate(), Some(1.5));

    assert!(props.tune_jitter_rate(LivingScope::Species("thorn"), 0.5));
    assert!(props.tune_jitter_rate(LivingScope::All, 2.0));
    assert_eq!(props.jitter_rate(LivingPropId(0)), 2.0);
    assert_eq!(props.jitter_rate(LivingPropId(2)), 0.5);
    assert_eq!(read(props, 2).layer, LivingLayer::Species);
    // Out of range is held to it, and not a number is the spec's pace.
    assert!(props.tune_jitter_rate(LivingScope::All, 100.0));
    assert_eq!(props.jitter_rate(LivingPropId(1)), JITTER_RATE_MAX);
    assert!(props.tune_jitter_rate(LivingScope::All, f32::NAN));
    assert_eq!(props.jitter_rate(LivingPropId(1)), 1.0);
    assert!(!props.tune_jitter_rate(LivingScope::Species("fern"), 2.0));

    // Each species' boil runs at its own rate from here.
    props.tune_jitter_rate(LivingScope::All, 2.0);
    let (t0, t2) = (
        props.prop(LivingPropId(0)).jitter_time(),
        props.prop(LivingPropId(2)).jitter_time(),
    );
    let made = props.stats().poses_made;
    resolver.process_living(0.25);
    let props = resolver.living_props_mut();
    let moved =
        |props: &LivingProps, i: usize, from: f64| props.prop(LivingPropId(i)).jitter_time() - from;
    assert!((moved(props, 0, t0) - 0.5).abs() < 1e-9, "the grove's 2");
    assert!(
        (moved(props, 2, t2) - 0.125).abs() < 1e-9,
        "the thorn's own 0.5"
    );

    // As loaded puts every layer back.
    let layers = LivingLayers {
        jitter_rates: vec![None; 4],
        ..props.layers()
    };
    props.restore_layers(&layers);
    assert_eq!(
        props.jitter_rate(LivingPropId(0)),
        1.0,
        "the spec's pace again"
    );
    assert_eq!(read(props, 0).layer, LivingLayer::Spec);
    assert_eq!(
        props.jitter_rate(LivingPropId(2)),
        0.5,
        "a species' own stays"
    );

    // The boil is a pure function of its clock: a rate asks for no pose the
    // props' own clock didn't, which a grove never tuned shows.
    let mut untuned = grove(LivingDefaults::default());
    tick_with(&mut untuned, &field, 0.0, 1.0 / 60.0);
    let untuned_made = untuned.living_props().stats().poses_made;
    untuned.process_living(0.25);
    assert_eq!(
        resolver.living_props().stats().poses_made - made,
        untuned.living_props().stats().poses_made - untuned_made
    );
}

#[test]
fn an_echo_boils_at_its_props_rate_and_never_pops_through_a_change() {
    // An echo reads the prop's jitter clock a fixed few seconds of it away, so
    // a new rate is continuous for it too.
    let extra = format!("{BOILING}\nechoes = {{ count = 2, spread = 1.0 }}");
    let (mut resolver, id) = one_prop(&extra, LivingDefaults::default());
    run(&mut resolver, 1.0, 1.0 / 60.0);
    let echo_times = |props: &LivingProps| -> Vec<f64> {
        let prop = props.prop(id);
        (0..prop.echo_count())
            .map(|e| prop.echo_jitter_time(e, prop.jitter_now()))
            .collect()
    };
    let before = echo_times(resolver.living_props());
    assert_eq!(before.len(), 2);
    resolver
        .living_props_mut()
        .tune_jitter_rate(LivingScope::All, 2.0);
    assert_eq!(echo_times(resolver.living_props()), before);
    resolver.process_living(0.1);
    for (after, before) in echo_times(resolver.living_props()).iter().zip(&before) {
        assert!((after - before - 0.2).abs() < 1e-9, "{after} from {before}");
    }
}

// ── Drifting rows (#576) ─────────────────────────────────────────────────────

/// A page whose words drift along their lines at 3 cm a second, with no boil,
/// one prop of it, built.
fn drifting_page(echoes: &str) -> (Grove, LivingPropId) {
    let spec = format!(
        r#"
generator = "form_v1"
[meta]
name = "page"
version = "1.0.0"
[seed]
mode = "fixed"
value = 1
[params]
height = 0.9
radius = 0.3
profile = [1.0, 1.0, 1.0]
aspect = 0.1
section_deg = 90.0
curl_deg = 0.0
lean = 0.0
lobes = 0.0
rings = 24
sides = 16
cards = 600
card_layout = "rows"
row_faces = "front"
card_size = 0.012
card_aspect = 4.0
row_pace = 0.03
[living]
lineage_seed = 3
pose_ring = 4
jitter = {{ amplitude = 0.0 }}
echoes = {echoes}
"#
    );
    let mut resolver = resolver_with(&spec);
    let mut req = request(1, "page");
    req.spec_name = "page".to_string();
    let id = resolver.add_living_prop(req).unwrap().0;
    resolver.make_living_now();
    (resolver, id)
}

/// The drift bones a prop's blend wrote this frame, if it wrote them.
fn drift_bones(resolver: &mut Grove, id: LivingPropId) -> Option<Vec<[[f32; 4]; 4]>> {
    let props = resolver.living_props_mut();
    let first = props
        .rig_of(id)
        .unwrap()
        .1
        .rows
        .as_ref()
        .unwrap()
        .first_bone as usize;
    let mut frame = BlendFrame::default();
    let blend = props.blend_prop(id, &mut frame);
    let at = blend.first_bone?;
    Some(frame.bones[first - at..].to_vec())
}

/// How far apart two palettes' translations are, at most.
fn farthest_move(a: &[[[f32; 4]; 4]], b: &[[[f32; 4]; 4]]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(a, b)| {
            let d: [f32; 3] = std::array::from_fn(|i| a[3][i] - b[3][i]);
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        })
        .fold(0.0, f32::max)
}

#[test]
fn a_drifting_page_writes_its_words_every_frame_it_moves() {
    let (mut resolver, id) = drifting_page("{ count = 0 }");
    resolver.process_living(1.0 / 60.0);
    let first = drift_bones(&mut resolver, id).expect("the first blend writes everything");
    let rig_len = resolver.living_props().rig_of(id).unwrap().1.palette_len();
    let rows = resolver
        .living_props()
        .rig_of(id)
        .unwrap()
        .1
        .rows
        .clone()
        .unwrap();
    assert_eq!(first.len(), rows.words.len());
    assert!(rig_len <= flint_procgen::MAX_SKINNED_BONES);
    // A frame on, the words have moved, though the pose may be holding.
    resolver.process_living(1.0 / 60.0);
    let next = drift_bones(&mut resolver, id).expect("the words moved");
    assert!(farthest_move(&first, &next) > 0.0);
    // Held at a rate of 0, nothing moves and nothing is written for them.
    resolver
        .living_props_mut()
        .tune_row_rate(LivingScope::All, 0.0);
    let held = resolver.living_props().prop(id).row_time();
    for _ in 0..10 {
        resolver.process_living(1.0 / 60.0);
    }
    assert_eq!(resolver.living_props().prop(id).row_time(), held);
}

#[test]
fn a_new_row_rate_moves_the_words_on_faster_from_where_they_are() {
    // Two pages a second in. One goes on at its spec's pace; the other's rate
    // goes to 3 on this frame. Nothing jumps at the change: the next frame's
    // words are one frame's travel at the new pace from where they stood.
    let dt = 1.0 / 60.0;
    let (mut steady, a) = drifting_page("{ count = 0 }");
    let (mut changed, b) = drifting_page("{ count = 0 }");
    for r in [&mut steady, &mut changed] {
        for _ in 0..60 {
            r.process_living(dt);
        }
        drift_bones(r, a).unwrap();
    }
    assert_eq!(
        steady.living_props().prop(a).row_time().to_bits(),
        changed.living_props().prop(b).row_time().to_bits()
    );
    changed
        .living_props_mut()
        .tune_row_rate(LivingScope::All, 3.0);
    assert_eq!(changed.living_props().row_rate(b), 3.0);
    let row_time = |r: &Grove, id| r.living_props().prop(id).row_time();
    let (t_steady, t_changed) = (row_time(&steady, a), row_time(&changed, b));
    assert_eq!(t_steady, t_changed, "setting a rate moves nothing");
    steady.process_living(dt);
    changed.process_living(dt);
    assert!((row_time(&steady, a) - t_steady - dt).abs() < 1e-12);
    assert!((row_time(&changed, b) - t_changed - 3.0 * dt).abs() < 1e-12);
    // The words drawn are the rig's at that clock, over the frame's pose.
    let after = drift_bones(&mut changed, b).unwrap();
    let mut expected = Vec::new();
    rows_at(&changed, b, row_time(&changed, b), &mut expected);
    assert!(farthest_move(&expected, &after) < 1e-6);
    // And a visible word moves along its line by at most a frame's travel at
    // the new pace: it goes on from where it was.
    let rig = changed.living_props().rig_of(b).unwrap().1.clone();
    let rows = rig.rows.as_ref().unwrap();
    let (from, to) = (t_changed, row_time(&changed, b));
    let mut seen = 0;
    for (w, word) in rows.words.iter().enumerate() {
        let pace = rows.lines[word.line as usize].pace;
        let ((x0, k0), (x1, k1)) = (rows.shown(w, from), rows.shown(w, to));
        if k0 > 0.0 && k1 > 0.0 {
            seen += 1;
            let travel = pace * (to - from) as f32;
            assert!(
                (x1 - x0) >= -1e-6 && (x1 - x0) <= travel + 1e-6,
                "word {w} went {} m along its line in a frame of {travel} m",
                x1 - x0
            );
        }
    }
    assert!(seen > rows.words.len() / 2);
}

/// The drift bones a prop's rig gives at `row_time`, over its current pose: the
/// segment bones it last wrote.
fn rows_at(resolver: &Grove, id: LivingPropId, row_time: f64, out: &mut Vec<[[f32; 4]; 4]>) {
    let props = resolver.living_props();
    let rig = props.rig_of(id).unwrap().1;
    let rows = rig.rows.as_ref().unwrap();
    let mut palette = props.prop(id).drawn.bones.segments.clone();
    palette.extend_from_slice(&[[[0.0; 4]; 4]; BOIL_BONES]);
    rows.bones_into(
        &[0.0; flint_procgen::living::JITTER_CHANNELS],
        row_time,
        &mut palette,
    );
    out.extend_from_slice(&palette[rows.first_bone as usize..]);
}

#[test]
fn an_echo_shows_the_words_a_little_before_or_after_now() {
    let (mut resolver, id) = drifting_page("{ count = 2, spread = 1.0, alpha = 0.3 }");
    for _ in 0..90 {
        resolver.process_living(1.0 / 60.0);
    }
    let props = resolver.living_props();
    let prop = props.prop(id);
    let now = prop.row_time();
    let echoes: Vec<f64> = (0..prop.echo_count())
        .map(|e| prop.echo_row_time(e, now))
        .collect();
    assert_eq!(echoes.len(), 2);
    assert!(echoes.iter().all(|t| *t != now), "{echoes:?} at {now}");
    assert!(echoes.iter().any(|t| *t < now) && echoes.iter().any(|t| *t > now));
    // A rate moves the echoes' clocks with the prop's: the same steps away.
    resolver
        .living_props_mut()
        .tune_row_rate(LivingScope::All, 2.0);
    resolver.process_living(0.1);
    let prop = resolver.living_props().prop(id);
    let later: Vec<f64> = (0..prop.echo_count())
        .map(|e| prop.echo_row_time(e, prop.row_time()))
        .collect();
    for (later, before) in later.iter().zip(&echoes) {
        assert!((later - before - 0.2).abs() < 1e-9, "{later} from {before}");
    }
}

#[test]
fn a_row_rate_layers_like_the_jitter_rate() {
    let mut resolver = grove(LivingDefaults {
        row_rate: Some(1.5),
        ..LivingDefaults::default()
    });
    let field = field("", &[]);
    tick_with(&mut resolver, &field, 0.0, 1.0 / 60.0);
    let props = resolver.living_props_mut();
    let read = |props: &LivingProps, i: usize| {
        props
            .resolved(LivingPropId(i), None)
            .get(LivingKnob::RowRate)
            .clone()
    };
    assert_eq!(read(props, 0).value, 1.5, "the scene's");
    assert_eq!(read(props, 0).layer, LivingLayer::Global);
    assert_eq!(props.global_row_rate(), Some(1.5));
    assert!(props.tune_row_rate(LivingScope::Species("thorn"), 0.5));
    assert!(props.tune_row_rate(LivingScope::All, 2.0));
    assert_eq!(props.row_rate(LivingPropId(0)), 2.0);
    assert_eq!(props.row_rate(LivingPropId(2)), 0.5);
    assert_eq!(read(props, 2).layer, LivingLayer::Species);
    assert!(props.tune_row_rate(LivingScope::All, 100.0));
    assert_eq!(props.row_rate(LivingPropId(1)), ROW_RATE_MAX);
    assert!(props.tune_row_rate(LivingScope::All, f32::NAN));
    assert_eq!(props.row_rate(LivingPropId(1)), 1.0);
    // "As loaded" puts it back.
    let layers = props.layers();
    props.tune_row_rate(LivingScope::All, 4.0);
    props.restore_layers(&layers);
    assert_eq!(props.row_rate(LivingPropId(1)), 1.0);
}
