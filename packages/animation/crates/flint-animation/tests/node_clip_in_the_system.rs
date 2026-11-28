//! A node-animated entity driven through `AnimationSystem` as the player
//! drives it (`initialize`, then one `update` a frame, every tier in its
//! order), watched where a player sees it: the child node's transform.
//!
//! #1225: an `autoplay` node clip held its first frame after one frame (the
//! next sync read the animator's `playing = false` as a script's), and a
//! finished node one-shot could not be replayed (`play_clip` of the same
//! clip changed nothing the sync could see). `clip_rules_tests.rs` holds
//! `NodeSync` to the shared rules on its own; this holds the whole system.

use flint_animation::animator::{self, Animator};
use flint_animation::clip::Interpolation;
use flint_animation::node_clip::{NodeClip, NodeTrack};
use flint_animation::skeletal_clip::{JointKeyframe, JointProperty};
use flint_animation::AnimationSystem;
use flint_core::components as comp;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_runtime::RuntimeSystem;
use std::collections::HashMap;

const DT: f64 = 1.0 / 60.0;

/// A clip sliding the `sail` node along `axis` from 0 to `reach` over
/// `duration` seconds.
fn slide(name: &str, duration: f64, axis: usize, reach: f32) -> NodeClip {
    let mut end = vec![0.0; 3];
    end[axis] = reach;
    let key = |time: f64, value: Vec<f32>| JointKeyframe {
        time,
        value,
        in_tangent: vec![],
        out_tangent: vec![],
    };
    NodeClip {
        name: name.into(),
        duration,
        node_tracks: vec![NodeTrack {
            node_name: "sail".into(),
            property: JointProperty::Translation,
            interpolation: Interpolation::Linear,
            keyframes: vec![key(0.0, vec![0.0; 3]), key(duration, end)],
        }],
    }
}

struct Windmill {
    world: FlintWorld,
    system: AnimationSystem,
    mill: EntityId,
    sail: EntityId,
}

impl Windmill {
    /// A windmill whose animator is `animator` (TOML) and whose `sail`
    /// child the clips move: `turn` (1 s, along x) and `nod` (0.5 s, along y).
    fn new(animator: &str) -> Self {
        let mut world = FlintWorld::new();
        let mill = world.spawn("windmill").unwrap();
        let sail = world.spawn("sail").unwrap();
        world.set_parent(sail, mill).unwrap();
        world
            .set_component(mill, comp::ANIMATOR, toml::from_str(animator).unwrap())
            .unwrap();
        let mut system = AnimationSystem::new();
        system.add_node_clip(slide("turn", 1.0, 0, 1.0));
        system.add_node_clip(slide("nod", 0.5, 1, 2.0));
        system.register_node_entity(mill, HashMap::from([("sail".to_string(), sail)]));
        system.initialize(&mut world).unwrap();
        Windmill {
            world,
            system,
            mill,
            sail,
        }
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.system.update(&mut self.world, DT).unwrap();
        }
    }

    /// Where the sail is: what the renderer draws.
    fn sail(&self) -> [f64; 3] {
        let p = self
            .world
            .view(self.sail, comp::TRANSFORM)
            .vec3("position", [0.0; 3]);
        p.map(f64::from)
    }

    fn playing(&self) -> bool {
        self.system
            .node_playback_state(&self.mill)
            .expect("the windmill has a playback")
            .playing
    }

    /// What the Rhai calls write.
    fn play_clip(&mut self, clip: &str) {
        let mut comps = self.world.get_components_mut(self.mill).unwrap();
        animator::write(&mut comps)
            .clip(clip)
            .playing(true)
            .no_blend();
    }

    fn stop_clip(&mut self) {
        let mut comps = self.world.get_components_mut(self.mill).unwrap();
        animator::write(&mut comps).playing(false);
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn an_autoplay_node_clip_keeps_moving_until_stop_clip() {
    // `playing` left at its schema default (false): autoplay alone starts it.
    let mut mill = Windmill::new("clip = \"turn\"\nautoplay = true\nloop = false");
    mill.frames(1);
    let first = mill.sail()[0];
    mill.frames(29);
    let after_half_a_second = mill.sail()[0];
    assert!(
        near(after_half_a_second, 30.0 * DT),
        "the sail keeps turning after frame 1: x = {after_half_a_second} (frame 1: {first})"
    );
    assert!(mill.playing());
    assert!(
        Animator::of(&mill.world, mill.mill).playing,
        "the animator says so, for scripts and the next sync"
    );

    mill.stop_clip();
    mill.frames(1);
    let held = mill.sail()[0];
    mill.frames(20);
    assert!(!mill.playing(), "stop_clip stops it");
    assert_eq!(mill.sail()[0], held, "and the sail holds where it stopped");
}

#[test]
fn a_finished_node_one_shot_replays_on_play_clip() {
    let mut mill = Windmill::new("clip = \"nod\"\nplaying = true\nloop = false");
    mill.frames(60); // 1 s into a 0.5 s clip
    assert!(!mill.playing(), "it ended");
    assert!(
        near(mill.sail()[1], 2.0),
        "on its last frame: {:?}",
        mill.sail()
    );
    assert!(
        !Animator::of(&mill.world, mill.mill).playing,
        "the end is published, so the animator can't restart it unasked"
    );
    mill.frames(10);
    assert!(near(mill.sail()[1], 2.0), "it stays ended");

    mill.play_clip("nod");
    mill.frames(6);
    assert!(mill.playing(), "play_clip replays it");
    assert!(
        near(mill.sail()[1], 2.0 * 6.0 * DT / 0.5),
        "from its start: {:?}",
        mill.sail()
    );
    mill.frames(60);
    assert!(!mill.playing(), "and it ends again");
    assert!(near(mill.sail()[1], 2.0));
}

#[test]
fn play_clip_switches_a_node_entity_to_another_clip_from_its_start() {
    let mut mill = Windmill::new("clip = \"turn\"\nautoplay = true");
    mill.frames(30);
    mill.play_clip("nod");
    mill.frames(3);
    let sail = mill.sail();
    assert!(near(sail[1], 2.0 * 3.0 * DT / 0.5), "nod from 0: {sail:?}");
    assert_eq!(
        mill.system
            .node_playback_state(&mill.mill)
            .unwrap()
            .clip_name,
        "nod"
    );
}
