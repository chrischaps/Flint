//! The flipbook kind against `sprite_sync`'s `advance_frame`, whose tests
//! are the spec (ADR 0139 section 6): a loop wraps, a one-shot stops on its
//! last frame and says so, a ping-pong turns at its ends; and every frame
//! lands on the step it landed on before #1620, at the frame rates games
//! run at.

use super::*;
use crate::clip::SpriteAnimFrame;
use flint_animation::AnimationSystem;
use flint_runtime::RuntimeSystem;

fn clip(name: &str, loop_mode: LoopMode, durations: &[u32]) -> SpriteAnimClip {
    SpriteAnimClip {
        name: name.to_string(),
        texture: "sheet.png".to_string(),
        frame_width: 32,
        frame_height: 32,
        frames: durations
            .iter()
            .enumerate()
            .map(|(col, &duration_ms)| SpriteAnimFrame {
                col: col as u32,
                row: 0,
                duration_ms,
            })
            .collect(),
        loop_mode,
    }
}

/// A sprite whose animator plays `clip`.
fn sprite(world: &mut FlintWorld, clip: &str) -> EntityId {
    let id = world.spawn("hero").unwrap();
    let sprite: toml::Value = toml::from_str("mode = \"sprite2d\"").unwrap();
    world.set_component(id, comp::SPRITE, sprite).unwrap();
    let mut comps = world.get_components_mut(id).unwrap();
    animator::write(&mut comps).clip(clip).playing(true);
    id
}

/// The column the sprite shows (its frame's index: `col` is the frame's).
fn shown(world: &FlintWorld, id: EntityId) -> Option<usize> {
    let rect = world.view(id, comp::SPRITE).vec4("source_rect", [-1.0; 4]);
    (rect[0] >= 0.0).then_some(rect[0] as usize / 32)
}

fn system_with(clips: Vec<SpriteAnimClip>) -> AnimationSystem {
    let mut kind = FlipbookKind::new();
    for c in clips {
        kind.add_clip(c);
    }
    let mut system = AnimationSystem::new();
    system.add_kind(Box::new(kind));
    system
}

/// `sprite_sync`'s stepping before #1620, as it was: `f32` milliseconds a
/// frame, a frame's duration taken off as it advances. The spec the kind's
/// timing is held to.
fn old_frames(durations: &[u32], mode: LoopMode, dt: f64, steps: usize) -> Vec<usize> {
    let n = durations.len();
    let dt_ms = (dt * 1000.0) as f32;
    let (mut elapsed, mut index, mut direction, mut playing) = (0.0f32, 0usize, 1i64, true);
    let mut out = Vec::new();
    for _ in 0..steps {
        if playing {
            elapsed += dt_ms;
            while playing && elapsed >= durations[index] as f32 {
                elapsed -= durations[index] as f32;
                let next = index as i64 + direction;
                match mode {
                    LoopMode::Loop => index = if next >= n as i64 { 0 } else { next as usize },
                    LoopMode::PingPong => {
                        if next >= n as i64 {
                            direction = -1;
                            index = n - 2;
                        } else if next < 0 {
                            direction = 1;
                            index = 1;
                        } else {
                            index = next as usize;
                        }
                    }
                    LoopMode::Once => {
                        if next >= n as i64 {
                            playing = false;
                        } else {
                            index = next as usize;
                        }
                    }
                }
            }
        }
        out.push(index);
    }
    out
}

#[test]
fn a_loop_wraps_to_its_first_frame() {
    let mut world = FlintWorld::new();
    let id = sprite(&mut world, "walk");
    let mut system = system_with(vec![clip("walk", LoopMode::Loop, &[100; 4])]);
    system.initialize(&mut world).unwrap();
    let mut seen = Vec::new();
    for _ in 0..5 {
        system.update(&mut world, 0.1).unwrap();
        seen.push(shown(&world, id).unwrap());
    }
    assert_eq!(seen, [1, 2, 3, 0, 1]);
    assert!(Animator::of(&world, id).playing);
    assert!(system.drain_clip_ends().is_empty());
}

#[test]
fn a_one_shot_stops_on_its_last_frame_and_says_so() {
    let mut world = FlintWorld::new();
    let id = sprite(&mut world, "jump");
    let mut system = system_with(vec![clip("jump", LoopMode::Once, &[100; 4])]);
    system.initialize(&mut world).unwrap();
    for _ in 0..3 {
        system.update(&mut world, 0.1).unwrap();
    }
    assert_eq!(shown(&world, id), Some(3));
    assert!(system.drain_clip_ends().is_empty());
    system.update(&mut world, 0.1).unwrap();
    assert_eq!(shown(&world, id), Some(3), "it holds the last frame");
    let ends = system.drain_clip_ends();
    assert_eq!(
        ends,
        [ClipEnd {
            entity_id: id,
            clip_name: "jump".into()
        }]
    );
    assert!(!Animator::of(&world, id).playing, "the animator is told");
    system.update(&mut world, 0.1).unwrap();
    assert!(system.drain_clip_ends().is_empty(), "it ends once");

    // Played again, a finished one-shot replays from its start
    let mut comps = world.get_components_mut(id).unwrap();
    animator::write(&mut comps).playing(true);
    drop(comps);
    system.update(&mut world, 0.05).unwrap();
    assert_eq!(shown(&world, id), Some(0));
}

#[test]
fn a_ping_pong_turns_at_its_ends_without_showing_them_twice() {
    let mut world = FlintWorld::new();
    let id = sprite(&mut world, "sway");
    let mut system = system_with(vec![clip("sway", LoopMode::PingPong, &[100; 4])]);
    system.initialize(&mut world).unwrap();
    let mut seen = Vec::new();
    for _ in 0..8 {
        system.update(&mut world, 0.1).unwrap();
        seen.push(shown(&world, id).unwrap());
    }
    assert_eq!(seen, [1, 2, 3, 2, 1, 0, 1, 2]);
}

/// A pause holds its frame and resumes where it stood (it restarted before
/// #1620); the animator's `loop = false` plays any flipbook once.
#[test]
fn a_pause_resumes_and_loop_false_plays_once() {
    let mut world = FlintWorld::new();
    let id = sprite(&mut world, "walk");
    let mut system = system_with(vec![clip("walk", LoopMode::Loop, &[100; 4])]);
    system.initialize(&mut world).unwrap();
    system.update(&mut world, 0.25).unwrap();
    assert_eq!(shown(&world, id), Some(2));
    let set = |world: &mut FlintWorld, playing: bool| {
        let mut comps = world.get_components_mut(id).unwrap();
        animator::write(&mut comps).playing(playing);
    };
    set(&mut world, false);
    system.update(&mut world, 1.0).unwrap();
    assert_eq!(shown(&world, id), Some(2), "paused");
    set(&mut world, true);
    system.update(&mut world, 0.1).unwrap();
    assert_eq!(shown(&world, id), Some(3), "resumed, not restarted");

    let mut comps = world.get_components_mut(id).unwrap();
    animator::write(&mut comps).looping(false);
    drop(comps);
    system.update(&mut world, 0.1).unwrap();
    assert_eq!(shown(&world, id), Some(3));
    assert_eq!(system.drain_clip_ends().len(), 1);
}

/// Every frame lands on the step `advance_frame` put it on, at the steps
/// games run at, over a minute: the cursor's `f64` seconds would land some
/// a step apart (ADR 0139's risk), so the kind steps frames in `f32`
/// milliseconds as before.
#[test]
fn every_frame_lands_on_the_step_it_did_before() {
    let cases: &[(&[u32], LoopMode)] = &[
        (&[500, 500], LoopMode::Loop),
        (&[100, 100, 100, 100], LoopMode::Loop),
        (&[150, 200, 300], LoopMode::Once),
        (&[150, 200, 300], LoopMode::Loop),
        (&[120, 120, 120, 120], LoopMode::PingPong),
        (&[100, 100, 100], LoopMode::PingPong),
        (&[16, 33, 50, 67], LoopMode::Loop),
    ];
    for &fps in &[24.0, 30.0, 50.0, 60.0, 144.0] {
        let dt = 1.0 / fps;
        let steps = (60.0 * fps) as usize;
        for (durations, mode) in cases {
            let mut world = FlintWorld::new();
            let id = sprite(&mut world, "c");
            let mut system = system_with(vec![clip("c", mode.clone(), durations)]);
            system.initialize(&mut world).unwrap();
            let mut new = Vec::new();
            for _ in 0..steps {
                system.update(&mut world, dt).unwrap();
                new.push(shown(&world, id).unwrap());
            }
            let old = old_frames(durations, mode.clone(), dt, steps);
            let first = old.iter().zip(&new).position(|(a, b)| a != b);
            assert_eq!(first, None, "{fps} fps, {durations:?} {mode:?}");
        }
    }
}

/// The kind's clips by name, and a clip no kind knows plays nowhere.
#[test]
fn the_kind_knows_its_clips() {
    let mut kind = FlipbookKind::new();
    assert_eq!(kind.clip_count(), 0);
    kind.add_clip(clip("walk", LoopMode::Loop, &[100, 100]));
    assert_eq!(kind.clip_count(), 1);
    assert!(kind.has_clip("walk"));
    assert!(!kind.has_clip("run"));

    let mut world = FlintWorld::new();
    let id = sprite(&mut world, "run");
    kind.sync_from_world(&world);
    assert!(kind.state(id).is_none());
}

/// An old scene's `sprite_animator`, which the animator reads for one
/// release, still plays, and is the component told when a one-shot ends.
#[test]
fn a_retired_sprite_animator_still_plays() {
    let mut world = FlintWorld::new();
    let id = world.spawn("hero").unwrap();
    let sprite: toml::Value = toml::from_str("mode = \"sprite2d\"").unwrap();
    world.set_component(id, comp::SPRITE, sprite).unwrap();
    let old: toml::Value = toml::from_str("clip = \"jump\"\nplaying = true\nspeed = 2.0").unwrap();
    world.set_component(id, comp::SPRITE_ANIMATOR, old).unwrap();
    let mut system = system_with(vec![clip("jump", LoopMode::Once, &[100; 3])]);
    system.initialize(&mut world).unwrap();
    system.update(&mut world, 0.1).unwrap();
    assert_eq!(shown(&world, id), Some(2), "at speed 2");
    system.update(&mut world, 0.1).unwrap();
    assert_eq!(system.drain_clip_ends().len(), 1);
    assert!(world
        .get_components(id)
        .unwrap()
        .get(comp::ANIMATOR)
        .is_none());
    assert_eq!(
        world.view(id, comp::SPRITE_ANIMATOR).bool("playing", true),
        false
    );
}
