//! `play_clip` / `stop_clip` from a real script, on a skinned entity the
//! animation system already tracks (#732): before, both only wrote fields the
//! skeletal sync read once, at the state's creation, so a tracked character
//! never changed clip, never paused, and a finished one-shot never replayed.

use flint_animation::clip::Interpolation;
use flint_animation::skeletal_clip::{JointKeyframe, JointProperty, JointTrack, SkeletalClip};
use flint_animation::skeleton::{JointPose, Skeleton};
use flint_animation::AnimationSystem;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_scene::load_scene_string;
use flint_schema::SchemaRegistry;
use flint_script::engine::ScriptEngine;

const IDENT: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

const DT: f64 = 1.0 / 60.0;

fn one_joint() -> Skeleton {
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

/// Slides the joint along +X, one unit a second, for `duration` seconds.
fn slide(name: &str, duration: f64) -> SkeletalClip {
    let key = |t: f64| JointKeyframe {
        time: t,
        value: vec![t as f32, 0.0, 0.0],
        in_tangent: vec![],
        out_tangent: vec![],
    };
    SkeletalClip {
        name: name.into(),
        duration,
        joint_tracks: vec![JointTrack {
            joint_index: 0,
            property: JointProperty::Translation,
            interpolation: Interpolation::Linear,
            keyframes: vec![key(0.0), key(duration)],
        }],
    }
}

const SCENE: &str = r#"
[scene]
name = "Clip calls"

[entities.hero.animator]
clip = "idle"
playing = true
loop = true

[entities.hero.skeleton]
skin = "rig"

[entities.hero.cue]
op = ""
clip = ""
"#;

/// The script does what the test cues, once: `play` or `stop`.
const SCRIPT: &str = r#"
fn on_update() {
    let me = self_entity();
    let op = get_field(me, "cue", "op");
    if op == "play" {
        play_clip(me, get_field(me, "cue", "clip"));
    } else if op == "stop" {
        stop_clip(me);
    }
    set_field(me, "cue", "op", "");
}
"#;

struct Rig {
    world: FlintWorld,
    engine: ScriptEngine,
    anim: AnimationSystem,
    hero: EntityId,
}

impl Rig {
    fn new() -> Self {
        let (mut world, _) = load_scene_string(SCENE, &SchemaRegistry::new()).unwrap();
        let hero = world.get_id("hero").unwrap();
        let mut engine = ScriptEngine::new();
        let ast = engine.compile(SCRIPT).unwrap();
        engine.add_script(hero, ast, "hero.rhai".into());
        engine.call_inits(&mut world);

        let mut anim = AnimationSystem::new();
        anim.add_skeletal_clip(slide("idle", 4.0));
        anim.add_skeletal_clip(slide("wave", 2.0));
        anim.add_skeletal_clip(slide("attack", 0.5));
        anim.add_skeleton(hero, one_joint());
        Rig {
            world,
            engine,
            anim,
            hero,
        }
    }

    /// Scripts, then the skeletal tier, as the player orders a frame.
    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.engine.call_updates(&mut self.world);
            self.anim.sync_skeletal_from_world(&self.world);
            self.anim.advance_skeletal(DT);
            self.anim.write_back_skeletal(&mut self.world);
        }
    }

    fn cue(&mut self, op: &str, clip: &str) {
        for (field, v) in [("op", op), ("clip", clip)] {
            self.world
                .set_field(self.hero, "cue", field, toml::Value::String(v.into()))
                .unwrap();
        }
    }

    fn clip(&self) -> String {
        self.anim
            .skeletal_playback_state(&self.hero)
            .unwrap()
            .clip_name
            .clone()
    }

    fn time(&self) -> f64 {
        self.anim.skeletal_playback_state(&self.hero).unwrap().time
    }

    fn playing(&self) -> bool {
        self.anim
            .skeletal_playback_state(&self.hero)
            .unwrap()
            .playing
    }

    fn joint_x(&self) -> f32 {
        self.anim.skeleton(&self.hero).unwrap().local_poses[0].translation[0]
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn play_clip_switches_a_tracked_skinned_entity() {
    let mut rig = Rig::new();
    rig.frames(60);
    assert_eq!(rig.clip(), "idle");

    rig.cue("play", "wave");
    rig.frames(30);
    assert_eq!(rig.clip(), "wave", "play_clip must switch the clip");
    // The cue's frame starts it at 0; 30 frames advance it half a second.
    assert!(
        close(rig.time(), 0.5),
        "wave from its start: {}",
        rig.time()
    );
}

#[test]
fn stop_clip_freezes_a_tracked_skinned_entity() {
    let mut rig = Rig::new();
    rig.frames(30);
    rig.cue("stop", "");
    rig.frames(1);
    let (t, x) = (rig.time(), rig.joint_x());
    rig.frames(90);
    assert!(!rig.playing());
    assert_eq!(rig.time(), t, "stop_clip must freeze its time");
    assert_eq!(rig.joint_x(), x, "and its pose");

    rig.cue("play", "idle");
    rig.frames(30);
    assert!(close(rig.time(), t + 0.5), "play resumes: {}", rig.time());
}

#[test]
fn a_finished_one_shot_replays() {
    let mut rig = Rig::new();
    rig.world
        .set_field(rig.hero, "animator", "loop", toml::Value::Boolean(false))
        .unwrap();
    rig.cue("play", "attack");
    rig.frames(60);
    assert!(!rig.playing(), "the half-second one-shot ended");
    assert!(close(rig.time(), 0.5));

    rig.cue("play", "attack");
    rig.frames(15);
    assert!(rig.playing(), "play_clip must replay it");
    assert!(close(rig.time(), 0.25), "from its start: {}", rig.time());
}
