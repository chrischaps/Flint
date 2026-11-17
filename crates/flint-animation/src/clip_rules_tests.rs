//! One set of behaviour tests for what an `animator` asks of a clip, run on
//! every kind that plays one: property clips (`AnimationSync`), node clips
//! (`NodeSync`) and skinned clips (`SkeletalSync`). They follow the animator
//! by one rule set (`Animator::apply_to`, #733); these hold all three to it,
//! so a kind can't drift from the others again the way skeletal clips
//! ignored `play_clip` (#732).
//!
//! Each test drives a rig through what scripts write (`play_clip`,
//! `stop_clip`, `set_anim_speed`, through `animator::write`, as the Rhai
//! calls do) and steps it as the player does: sync, advance, write back.

use crate::animator::{self, Animator};
use crate::clip::AnimationClip;
use crate::node_clip::NodeClip;
use crate::node_sync::NodeSync;
use crate::playback_state::ClipPlaybackState;
use crate::player::AnimationPlayer;
use crate::skeletal_clip::SkeletalClip;
use crate::skeletal_sync::SkeletalSync;
use crate::skeleton::{JointPose, Skeleton};
use crate::sync::AnimationSync;
use flint_core::components as comp;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Property,
    Node,
    Skeletal,
}

const KINDS: [Kind; 3] = [Kind::Property, Kind::Node, Kind::Skeletal];

/// The clips every rig knows: a 4 s idle and a 2 s wave.
const CLIPS: [(&str, f64); 2] = [("idle", 4.0), ("wave", 2.0)];

const DT: f64 = 1.0 / 60.0;

struct Rig {
    kind: Kind,
    world: FlintWorld,
    id: EntityId,
    player: AnimationPlayer,
    property: AnimationSync,
    node: NodeSync,
    skeletal: SkeletalSync,
}

impl Rig {
    /// An entity of `kind` whose animator is `animator` (TOML).
    fn new(kind: Kind, animator: &str) -> Self {
        let mut world = FlintWorld::new();
        let id = world.spawn("hero").unwrap();
        let data: toml::Value = toml::from_str(animator).unwrap();
        world.set_component(id, comp::ANIMATOR, data).unwrap();
        let mut rig = Rig {
            kind,
            world,
            id,
            player: AnimationPlayer::new(),
            property: AnimationSync::new(),
            node: NodeSync::new(),
            skeletal: SkeletalSync::new(),
        };
        for (name, duration) in CLIPS {
            let name = name.to_string();
            match kind {
                Kind::Property => rig.player.add_clip(AnimationClip {
                    name,
                    duration,
                    tracks: vec![],
                    events: vec![],
                }),
                Kind::Node => rig.node.add_clip(NodeClip {
                    name,
                    duration,
                    node_tracks: vec![],
                }),
                Kind::Skeletal => rig.skeletal.add_clip(SkeletalClip {
                    name,
                    duration,
                    joint_tracks: vec![],
                }),
            }
        }
        match kind {
            Kind::Property => {}
            Kind::Node => rig.node.register_entity(id, HashMap::new()),
            Kind::Skeletal => {
                rig.world
                    .set_component(id, comp::SKELETON, toml::Value::Table(Default::default()))
                    .unwrap();
                rig.skeletal.add_skeleton(id, one_joint_skeleton());
            }
        }
        rig
    }

    /// `n` frames as the player steps them.
    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            match self.kind {
                Kind::Property => {
                    self.property.sync_from_world(&self.world, &self.player);
                    self.property
                        .advance_and_write(&mut self.world, &self.player, DT);
                }
                Kind::Node => {
                    self.node.sync_from_world(&self.world);
                    self.node.advance_and_apply(&mut self.world, DT);
                }
                Kind::Skeletal => {
                    self.skeletal.sync_from_world(&self.world);
                    self.skeletal.advance_and_compute(DT);
                    self.skeletal.write_back(&mut self.world);
                }
            }
        }
    }

    fn state(&self) -> &ClipPlaybackState {
        let state = match self.kind {
            Kind::Property => self.property.playback_state(&self.id),
            Kind::Node => self.node.get_playback_state(&self.id),
            Kind::Skeletal => self.skeletal.get_playback_state(&self.id),
        };
        state.unwrap_or_else(|| panic!("{:?}: no playback", self.kind))
    }

    fn animator(&self) -> Animator<'_> {
        Animator::of(&self.world, self.id)
    }

    /// What a script's call writes.
    fn write(&mut self, f: impl FnOnce(&mut animator::AnimatorWrite)) {
        let mut comps = self.world.get_components_mut(self.id).unwrap();
        f(&mut animator::write(&mut comps));
    }

    fn play_clip(&mut self, clip: &str) {
        self.write(|w| {
            w.clip(clip).playing(true).no_blend();
        });
    }

    fn stop_clip(&mut self) {
        self.write(|w| {
            w.playing(false);
        });
    }
}

fn one_joint_skeleton() -> Skeleton {
    const IDENT: [[f32; 4]; 4] = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    Skeleton {
        joint_names: vec!["root".into()],
        parents: vec![None],
        inverse_bind_matrices: vec![IDENT],
        parent_offsets: vec![None],
        local_poses: vec![JointPose::default()],
        rest_poses: vec![JointPose::default()],
        bone_matrices: vec![IDENT],
        global_matrices: vec![IDENT],
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn play_clip_cuts_a_tracked_entity_to_the_new_clip_from_its_start() {
    for kind in KINDS {
        let mut rig = Rig::new(kind, "clip = \"idle\"\nplaying = true");
        rig.frames(60);
        assert!(rig.state().time > 0.9, "{kind:?}");

        rig.play_clip("wave");
        rig.frames(6);
        let s = rig.state();
        assert_eq!(s.clip_name, "wave", "{kind:?}: play_clip must switch");
        assert!(near(s.time, 6.0 * DT), "{kind:?}: from 0, got {}", s.time);
        assert!(s.playing, "{kind:?}");
    }
}

#[test]
fn stop_clip_freezes_time_and_play_resumes_where_it_stopped() {
    for kind in KINDS {
        let mut rig = Rig::new(kind, "clip = \"idle\"\nplaying = true\nloop = false");
        rig.frames(30);
        rig.stop_clip();
        rig.frames(1);
        let held = rig.state().time;
        rig.frames(30);
        assert!(!rig.state().playing, "{kind:?}: stop_clip pauses");
        assert_eq!(rig.state().time, held, "{kind:?}: time holds");

        rig.play_clip("idle");
        rig.frames(1);
        assert!(rig.state().playing, "{kind:?}");
        assert!(
            near(rig.state().time, held + DT),
            "{kind:?}: a one-shot stopped part-way resumes, got {}",
            rig.state().time
        );
    }
}

#[test]
fn a_finished_one_shot_says_so_and_replays_when_played_again() {
    for kind in KINDS {
        let mut rig = Rig::new(kind, "clip = \"wave\"\nplaying = true\nloop = false");
        rig.frames(150); // 2.5 s into a 2 s clip
        assert!(!rig.state().playing, "{kind:?}: it ended");
        assert!(near(rig.state().time, 2.0), "{kind:?}");
        assert!(
            !rig.animator().playing,
            "{kind:?}: the animator is told, so it can't replay on its own"
        );
        rig.frames(10);
        assert!(!rig.state().playing, "{kind:?}: it stays ended");

        rig.play_clip("wave");
        rig.frames(3);
        assert!(rig.state().playing, "{kind:?}: play_clip replays it");
        assert!(near(rig.state().time, 3.0 * DT), "{kind:?}: from its start");
    }
}

#[test]
fn autoplay_keeps_playing_and_the_animator_says_so() {
    for kind in KINDS {
        let mut rig = Rig::new(kind, "clip = \"idle\"\nautoplay = true");
        rig.frames(30);
        assert!(
            rig.state().playing,
            "{kind:?}: autoplay must not stop on frame 2"
        );
        assert!(near(rig.state().time, 30.0 * DT), "{kind:?}");
        assert!(rig.animator().playing, "{kind:?}: published");
    }
}

#[test]
fn a_clip_not_playing_holds_its_first_frame_until_played() {
    for kind in KINDS {
        let mut rig = Rig::new(kind, "clip = \"idle\"");
        rig.frames(30);
        assert!(!rig.state().playing, "{kind:?}");
        assert_eq!(rig.state().time, 0.0, "{kind:?}");
        rig.play_clip("idle");
        rig.frames(30);
        assert!(
            near(rig.state().time, 30.0 * DT),
            "{kind:?}: play_clip starts it"
        );
    }
}

#[test]
fn played_backwards_a_clip_starts_at_its_end() {
    for kind in KINDS {
        let mut rig = Rig::new(kind, "clip = \"idle\"\nplaying = true\nloop = false");
        rig.frames(1);
        rig.write(|w| {
            w.speed(-1.0);
        });
        rig.play_clip("wave");
        rig.frames(6);
        assert!(
            near(rig.state().time, 2.0 - 6.0 * DT),
            "{kind:?}: got {}",
            rig.state().time
        );
        assert!(rig.state().playing, "{kind:?}: not ended at once");
    }
}

#[test]
fn speed_and_loop_are_dials() {
    for kind in KINDS {
        let mut rig = Rig::new(kind, "clip = \"wave\"\nplaying = true");
        rig.frames(60);
        assert!(rig.state().looping, "{kind:?}: loop defaults on");
        rig.write(|w| {
            w.speed(3.0).looping(false);
        });
        rig.frames(1);
        assert_eq!(rig.state().speed, 3.0, "{kind:?}");
        assert!(!rig.state().looping, "{kind:?}");
        rig.frames(60);
        assert!(!rig.state().playing, "{kind:?}: a one-shot now, so it ends");
    }
}

#[test]
fn an_unknown_clip_is_ignored_and_the_current_one_keeps_playing() {
    for kind in KINDS {
        let mut rig = Rig::new(kind, "clip = \"idle\"\nplaying = true");
        rig.frames(10);
        rig.play_clip("nowhere");
        rig.frames(10);
        assert_eq!(rig.state().clip_name, "idle", "{kind:?}");
        assert!(near(rig.state().time, 20.0 * DT), "{kind:?}");
    }
}
