//! Animation sequences: timestamped animator events in a `*.sequence.toml`
//!
//! A sequence is a list of `[[events]]` sorted by `time`. Each event is one
//! of the things a script can already do to an `animator` component —
//! crossfade the base clip, set or fade a layer, change speed — plus named
//! cues that scripts observe. The runner applies steps as plain ECS field
//! writes, so the same file drives the `flint edit` previewer and the
//! player identically, and seeking is a deterministic replay from `t = 0`.
//!
//! ```toml
//! name = "showcase"
//! loop = false
//!
//! [[events]]
//! time = 1.5
//! kind = "blend"
//! clip = "WalkCycle"
//! duration = 0.4
//!
//! [[events]]
//! time = 2.5
//! kind = "layer"
//! index = 0
//! clip = "StarCower"
//! mask = "head"
//! weight = 1.0
//! fade = 0.3
//!
//! [[events]]
//! time = 6.0
//! kind = "cue"
//! name = "done"
//! ```

use crate::layer_edit;
use flint_core::components as comp;
use flint_core::{EntityId, FlintError, Result};
use flint_ecs::FlintWorld;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Smallest crossfade the skeletal tier will run: `blend_duration <= 0`
/// disables blending entirely, so a "hard cut" is a one-frame fade.
pub const MIN_BLEND: f64 = 0.001;

/// Ordered, timestamped animator events
#[derive(Debug, Clone, Deserialize)]
pub struct AnimSequence {
    pub name: String,
    /// Explicit length; defaults to the last event plus its transition
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default, rename = "loop")]
    pub looping: bool,
    #[serde(default)]
    pub events: Vec<SequenceEvent>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SequenceEvent {
    pub time: f64,
    #[serde(flatten)]
    pub step: SequenceStep,
}

/// One thing to do to the animator at an event's time
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SequenceStep {
    /// Crossfade the base clip (`duration = 0` → hard cut)
    Blend {
        clip: String,
        #[serde(default)]
        duration: f64,
    },
    /// Set a layer slot. Omitted fields keep their current value; `fade`
    /// ramps the weight over that many seconds.
    Layer {
        index: usize,
        #[serde(default)]
        clip: Option<String>,
        #[serde(default)]
        weight: Option<f64>,
        #[serde(default)]
        fade: f64,
        #[serde(default)]
        mode: Option<String>,
        #[serde(default)]
        mask: Option<String>,
    },
    /// Base playback speed
    Speed { value: f64 },
    /// Named marker delivered to the entity's script (`on_sequence_cue`)
    Cue { name: String },
}

impl SequenceStep {
    /// Seconds the step keeps changing the pose after its event time
    pub fn transition_len(&self) -> f64 {
        match self {
            SequenceStep::Blend { duration, .. } => duration.max(0.0),
            SequenceStep::Layer { fade, .. } => fade.max(0.0),
            _ => 0.0,
        }
    }

    /// Short label for timelines / logs
    pub fn label(&self) -> String {
        match self {
            SequenceStep::Blend { clip, duration } => {
                if *duration > 0.0 {
                    format!("blend → {clip} ({duration:.2}s)")
                } else {
                    format!("cut → {clip}")
                }
            }
            SequenceStep::Layer {
                index,
                clip,
                weight,
                fade,
                ..
            } => {
                let mut s = format!("L{index}");
                if let Some(c) = clip {
                    s.push_str(&format!(" {c}"));
                }
                if let Some(w) = weight {
                    s.push_str(&format!(" w={w:.2}"));
                }
                if *fade > 0.0 {
                    s.push_str(&format!(" fade {fade:.2}s"));
                }
                s
            }
            SequenceStep::Speed { value } => format!("speed ×{value:.2}"),
            SequenceStep::Cue { name } => format!("cue '{name}'"),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            SequenceStep::Blend { .. } => "blend",
            SequenceStep::Layer { .. } => "layer",
            SequenceStep::Speed { .. } => "speed",
            SequenceStep::Cue { .. } => "cue",
        }
    }
}

impl AnimSequence {
    /// Explicit `duration`, else the last event's time plus its transition
    pub fn resolved_duration(&self) -> f64 {
        self.duration.unwrap_or_else(|| {
            self.events
                .iter()
                .map(|e| e.time + e.step.transition_len())
                .fold(0.0, f64::max)
        })
    }

    fn validate(mut self, origin: &str) -> Result<Self> {
        if self.name.is_empty() {
            return Err(FlintError::AnimationError(format!(
                "{origin}: sequence has no name"
            )));
        }
        if let Some(e) = self.events.iter().find(|e| e.time < 0.0) {
            return Err(FlintError::AnimationError(format!(
                "{origin}: event at negative time {}",
                e.time
            )));
        }
        if let Some(d) = self.duration {
            if d <= 0.0 {
                return Err(FlintError::AnimationError(format!(
                    "{origin}: non-positive duration {d}"
                )));
            }
        }
        if self.looping && self.resolved_duration() <= 0.0 {
            return Err(FlintError::AnimationError(format!(
                "{origin}: looping sequence has zero duration"
            )));
        }
        // Stable sort keeps same-time events in authored order
        self.events.sort_by(|a, b| a.time.total_cmp(&b.time));
        Ok(self)
    }
}

/// Load a `*.sequence.toml`
pub fn load_sequence_from_file(path: &Path) -> Result<AnimSequence> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        FlintError::AnimationError(format!("Failed to read {}: {}", path.display(), e))
    })?;
    load_sequence_from_str(&content, &path.display().to_string())
}

/// Parse a sequence from TOML text; `origin` names it in errors
pub fn load_sequence_from_str(content: &str, origin: &str) -> Result<AnimSequence> {
    let seq: AnimSequence = toml::from_str(content)
        .map_err(|e| FlintError::AnimationError(format!("Failed to parse {origin}: {e}")))?;
    seq.validate(origin)
}

/// A cue an active sequence passed this frame
#[derive(Debug, Clone, PartialEq)]
pub struct SequenceCueEvent {
    pub entity_id: EntityId,
    pub sequence: String,
    pub cue: String,
    /// Sequence time the cue is authored at
    pub time: f64,
}

/// Per-entity playback of one sequence
#[derive(Debug, Clone)]
pub struct SequenceRuntime {
    pub sequence: Arc<AnimSequence>,
    pub time: f64,
    pub playing: bool,
    /// Runtime loop override (previewer checkbox); `None` = file's value
    pub loop_override: Option<bool>,
    fired: Vec<bool>,
}

impl SequenceRuntime {
    fn new(sequence: Arc<AnimSequence>) -> Self {
        let n = sequence.events.len();
        Self {
            sequence,
            time: 0.0,
            playing: true,
            loop_override: None,
            fired: vec![false; n],
        }
    }

    pub fn name(&self) -> &str {
        &self.sequence.name
    }

    pub fn duration(&self) -> f64 {
        self.sequence.resolved_duration()
    }

    pub fn looping(&self) -> bool {
        self.loop_override.unwrap_or(self.sequence.looping)
    }

    /// Whether event `i` has fired since the last (re)start / loop wrap
    pub fn fired(&self, i: usize) -> bool {
        self.fired.get(i).copied().unwrap_or(false)
    }

    pub fn fired_count(&self) -> usize {
        self.fired.iter().filter(|f| **f).count()
    }

    /// Index of the most recently fired event, if any
    pub fn last_fired(&self) -> Option<usize> {
        self.fired.iter().rposition(|f| *f)
    }

    /// Index of the next unfired event, if any
    pub fn next_unfired(&self) -> Option<usize> {
        self.fired.iter().position(|f| !*f)
    }

    fn restart(&mut self) {
        self.time = 0.0;
        self.playing = true;
        self.fired.iter_mut().for_each(|f| *f = false);
    }
}

/// Registry of sequences plus the per-entity runtimes driving them.
///
/// Runs *before* the skeletal tier each frame so its ECS writes are picked
/// up by `SkeletalSync::sync_from_world` in the same update. Scripts start
/// and stop sequences through the `animator.sequence` field (see
/// [`Self::sync_from_world`]); the previewer calls [`Self::play`] directly.
#[derive(Default)]
pub struct SequenceSync {
    sequences: HashMap<String, Arc<AnimSequence>>,
    active: HashMap<EntityId, SequenceRuntime>,
    /// Last `animator.sequence` value seen per entity, to detect edges
    last_seen: HashMap<EntityId, String>,
    fired_cues: Vec<SequenceCueEvent>,
}

impl SequenceSync {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop all runtimes (scene transition); keeps the registry
    pub fn clear(&mut self) {
        self.active.clear();
        self.last_seen.clear();
        self.fired_cues.clear();
    }

    // ── Registry ────────────────────────────────────────────────

    pub fn add_sequence(&mut self, seq: AnimSequence) {
        self.sequences.insert(seq.name.clone(), Arc::new(seq));
    }

    pub fn get(&self, name: &str) -> Option<&Arc<AnimSequence>> {
        self.sequences.get(name)
    }

    pub fn sequence_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.sequences.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn sequence_count(&self) -> usize {
        self.sequences.len()
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    // ── Control ─────────────────────────────────────────────────

    /// Start `name` on an entity from `t = 0`. Returns false if unknown.
    pub fn play(&mut self, entity_id: EntityId, name: &str) -> bool {
        let Some(seq) = self.sequences.get(name) else {
            return false;
        };
        self.active
            .insert(entity_id, SequenceRuntime::new(seq.clone()));
        true
    }

    pub fn stop(&mut self, entity_id: &EntityId) {
        self.active.remove(entity_id);
    }

    pub fn restart(&mut self, entity_id: &EntityId) {
        if let Some(rt) = self.active.get_mut(entity_id) {
            rt.restart();
        }
    }

    pub fn set_playing(&mut self, entity_id: &EntityId, playing: bool) {
        if let Some(rt) = self.active.get_mut(entity_id) {
            rt.playing = playing;
        }
    }

    pub fn set_loop_override(&mut self, entity_id: &EntityId, looping: Option<bool>) {
        if let Some(rt) = self.active.get_mut(entity_id) {
            rt.loop_override = looping;
        }
    }

    pub fn state(&self, entity_id: &EntityId) -> Option<&SequenceRuntime> {
        self.active.get(entity_id)
    }

    pub fn drain_cues(&mut self) -> Vec<SequenceCueEvent> {
        std::mem::take(&mut self.fired_cues)
    }

    // ── Frame ───────────────────────────────────────────────────

    /// Follow the `animator.sequence` field: a new non-empty value starts
    /// that sequence, an edge to `""` stops the active one. The field is
    /// the scripting surface (`play_sequence` / `stop_sequence` just write
    /// it) and lets a scene autoplay a sequence by authoring it.
    ///
    /// Only *edges* of the ECS value count — `last_seen` tracks what the
    /// field said last frame, never what [`Self::play`] started — so a
    /// runtime started directly (the previewer) survives an empty field.
    pub fn sync_from_world(&mut self, world: &FlintWorld) {
        let ids: Vec<EntityId> = world
            .entities_with_component(comp::ANIMATOR)
            .iter()
            .copied()
            .collect();
        for entity_id in ids {
            let wanted = world
                .get_components(entity_id)
                .and_then(|c| c.get(comp::ANIMATOR))
                .and_then(|a| a.get("sequence"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let seen = self.last_seen.get(&entity_id).cloned().unwrap_or_default();
            if wanted == seen {
                continue;
            }
            if wanted.is_empty() {
                self.stop(&entity_id);
            } else if !self.play(entity_id, &wanted) {
                println!(
                    "WARNING: entity {:?} asked for unknown sequence '{}'",
                    entity_id, wanted
                );
            }
            self.last_seen.insert(entity_id, wanted);
        }
        // Forget entities that vanished
        self.active
            .retain(|id, _| world.get_components(*id).is_some());
        self.last_seen
            .retain(|id, _| world.get_components(*id).is_some());
    }

    /// Advance every playing runtime by `dt`, applying passed events to the
    /// ECS. Events fire exactly once per pass (inclusive of `time == t`).
    pub fn advance(&mut self, world: &mut FlintWorld, dt: f64) {
        let ids: Vec<EntityId> = self.active.keys().copied().collect();
        for entity_id in ids {
            let Some(rt) = self.active.get_mut(&entity_id) else {
                continue;
            };
            if !rt.playing {
                continue;
            }
            rt.time += dt;
            let seq = rt.sequence.clone();
            let duration = seq.resolved_duration();

            if rt.looping() && duration > 0.0 {
                while rt.time >= duration {
                    rt.time -= duration;
                    rt.fired.iter_mut().for_each(|f| *f = false);
                    for (i, ev) in seq.events.iter().enumerate() {
                        if rt.fired[i] {
                            continue;
                        }
                        if ev.time > rt.time {
                            break;
                        }
                        rt.fired[i] = true;
                        Self::apply_step(world, entity_id, &seq.name, ev, &mut self.fired_cues);
                    }
                }
                for (i, ev) in seq.events.iter().enumerate() {
                    if rt.fired[i] {
                        continue;
                    }
                    if ev.time > rt.time {
                        break;
                    }
                    rt.fired[i] = true;
                    Self::apply_step(world, entity_id, &seq.name, ev, &mut self.fired_cues);
                }
            } else {
                for (i, ev) in seq.events.iter().enumerate() {
                    if rt.fired[i] {
                        continue;
                    }
                    if ev.time > rt.time {
                        break;
                    }
                    rt.fired[i] = true;
                    Self::apply_step(world, entity_id, &seq.name, ev, &mut self.fired_cues);
                }

                if duration > 0.0 && rt.time >= duration {
                    rt.time = duration;
                    rt.playing = false;
                    // Retire the request so re-playing the same name is an
                    // edge again (mirrors `blend_target` being cleared).
                    if self
                        .last_seen
                        .get(&entity_id)
                        .is_some_and(|s| !s.is_empty())
                    {
                        if let Some(comps) = world.get_components_mut(entity_id) {
                            comps.set_field(
                                comp::ANIMATOR,
                                "sequence",
                                toml::Value::String(String::new()),
                            );
                        }
                        self.last_seen.insert(entity_id, String::new());
                    }
                } else if duration <= 0.0 && !rt.looping() {
                    rt.time = 0.0;
                    rt.playing = false;
                    if self
                        .last_seen
                        .get(&entity_id)
                        .is_some_and(|s| !s.is_empty())
                    {
                        if let Some(comps) = world.get_components_mut(entity_id) {
                            comps.set_field(
                                comp::ANIMATOR,
                                "sequence",
                                toml::Value::String(String::new()),
                            );
                        }
                        self.last_seen.insert(entity_id, String::new());
                    }
                }
            }
        }
    }

    /// Apply one event as animator field writes — exactly what the Rhai
    /// `blend_to` / `set_anim_layer*` / `set_anim_speed` calls do.
    fn apply_step(
        world: &mut FlintWorld,
        entity_id: EntityId,
        sequence: &str,
        ev: &SequenceEvent,
        cues: &mut Vec<SequenceCueEvent>,
    ) {
        let Some(comps) = world.get_components_mut(entity_id) else {
            return;
        };
        match &ev.step {
            SequenceStep::Blend { clip, duration } => {
                // Only `blend_target` changes a tracked skeletal base clip;
                // `clip` is latched at state creation. Set both so an
                // untracked entity also starts on the right clip.
                let has_clip = comps
                    .get(comp::ANIMATOR)
                    .and_then(|a| a.get("clip"))
                    .and_then(|v| v.as_str())
                    .is_some_and(|c| !c.is_empty());
                if !has_clip {
                    comps.set_field(comp::ANIMATOR, "clip", toml::Value::String(clip.clone()));
                }
                comps.set_field(comp::ANIMATOR, "playing", toml::Value::Boolean(true));
                comps.set_field(
                    comp::ANIMATOR,
                    "blend_target",
                    toml::Value::String(clip.clone()),
                );
                comps.set_field(
                    comp::ANIMATOR,
                    "blend_duration",
                    toml::Value::Float(duration.max(MIN_BLEND)),
                );
            }
            SequenceStep::Layer {
                index,
                clip,
                weight,
                fade,
                mode,
                mask,
            } => {
                layer_edit::edit_layer_table(comps, *index, |t| {
                    // A slot that was inactive has no meaningful weight;
                    // a fade onto it should start from silence, not 1.0.
                    let was_inactive = t
                        .get("clip")
                        .and_then(|v| v.as_str())
                        .map_or(true, |c| c.is_empty());
                    if was_inactive && *fade > 0.0 && !t.contains_key("weight") {
                        t.insert("weight".into(), toml::Value::Float(0.0));
                    }
                    if let Some(c) = clip {
                        t.insert("clip".into(), toml::Value::String(c.clone()));
                    }
                    if let Some(m) = mode {
                        t.insert("mode".into(), toml::Value::String(m.clone()));
                    }
                    if let Some(m) = mask {
                        t.insert("mask".into(), toml::Value::String(m.clone()));
                    }
                    if let Some(w) = weight {
                        layer_edit::fade_weight(t, *w, *fade);
                    }
                });
            }
            SequenceStep::Speed { value } => {
                comps.set_field(comp::ANIMATOR, "speed", toml::Value::Float(*value));
            }
            SequenceStep::Cue { name } => cues.push(SequenceCueEvent {
                entity_id,
                sequence: sequence.to_string(),
                cue: name.clone(),
                time: ev.time,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::toml_util::toml_f64;

    const EXAMPLE: &str = r#"
name = "showcase"
loop = false

[[events]]
time = 2.5
kind = "layer"
index = 0
clip = "StarCower"
mask = "head"
weight = 1.0
fade = 0.3

[[events]]
time = 0.0
kind = "blend"
clip = "BreathingIdle"

[[events]]
time = 1.5
kind = "blend"
clip = "WalkCycle"
duration = 0.4

[[events]]
time = 4.0
kind = "speed"
value = 1.3

[[events]]
time = 5.5
kind = "layer"
index = 0
weight = 0.0
fade = 0.6

[[events]]
time = 5.8
kind = "cue"
name = "done"
"#;

    fn world_with_animator() -> (FlintWorld, EntityId) {
        let mut world = FlintWorld::new();
        let id = world.spawn("actor").unwrap();
        world
            .set_component(id, comp::ANIMATOR, toml::Value::Table(Default::default()))
            .unwrap();
        (world, id)
    }

    fn animator(world: &FlintWorld, id: EntityId) -> toml::Value {
        world
            .get_components(id)
            .unwrap()
            .get(comp::ANIMATOR)
            .cloned()
            .unwrap()
    }

    #[test]
    fn parses_sorts_and_resolves_duration() {
        let seq = load_sequence_from_str(EXAMPLE, "test").unwrap();
        let times: Vec<f64> = seq.events.iter().map(|e| e.time).collect();
        assert_eq!(times, vec![0.0, 1.5, 2.5, 4.0, 5.5, 5.8]);
        // 5.5 + 0.6 fade beats the 5.8 cue
        assert!((seq.resolved_duration() - 6.1).abs() < 1e-9);
        assert_eq!(seq.events[2].step.kind(), "layer");
    }

    #[test]
    fn rejects_bad_files() {
        assert!(load_sequence_from_str("name = \"\"", "t").is_err());
        assert!(load_sequence_from_str(
            "name = \"x\"\n[[events]]\ntime = -1\nkind = \"cue\"\nname = \"a\"",
            "t"
        )
        .is_err());
        assert!(
            load_sequence_from_str("name = \"x\"\n[[events]]\ntime = 1\nkind = \"nope\"", "t")
                .is_err()
        );
        assert!(load_sequence_from_str(
            "name = \"loop\"\nloop = true\n[[events]]\ntime = 0\nkind = \"cue\"\nname = \"a\"",
            "t"
        )
        .is_err());
    }

    #[test]
    fn zero_duration_non_looping_sequence_finishes_after_initial_events() {
        let seq = load_sequence_from_str(
            "name = \"instant\"\n[[events]]\ntime = 0.0\nkind = \"cue\"\nname = \"ready\"",
            "t",
        )
        .unwrap();
        let (mut world, id) = world_with_animator();
        world
            .get_components_mut(id)
            .unwrap()
            .set_field(comp::ANIMATOR, "sequence", "instant".into());
        let mut sync = SequenceSync::new();
        sync.add_sequence(seq);
        sync.sync_from_world(&world);
        sync.advance(&mut world, 1.0);
        let rt = sync.state(&id).unwrap();
        assert!(!rt.playing);
        assert_eq!(sync.drain_cues().len(), 1);
        assert_eq!(animator(&world, id)["sequence"].as_str(), Some(""));
    }

    #[test]
    fn rejects_out_of_range_layer_index() {
        let (mut world, id) = world_with_animator();
        {
            let mut comps = world.get_components_mut(id).unwrap();
            comps.set_field(
                comp::ANIMATOR,
                "layers",
                toml::Value::Array(vec![toml::Value::Table(Default::default())]),
            );
            layer_edit::edit_layer_table(&mut comps, u8::MAX as usize, |_| {
                panic!("out-of-range layer index should be rejected");
            });
        }
        let a = animator(&world, id);
        let layers = a.get("layers").and_then(|v| v.as_array()).unwrap();
        assert_eq!(layers.len(), 1);
    }

    #[test]
    fn events_fire_once_and_write_animator_fields() {
        let seq = load_sequence_from_str(EXAMPLE, "test").unwrap();
        let (mut world, id) = world_with_animator();
        let mut sync = SequenceSync::new();
        sync.add_sequence(seq);
        assert!(sync.play(id, "showcase"));

        // First tick fires the t=0 cut: blend_target set, duration clamped
        sync.advance(&mut world, 0.0);
        let a = animator(&world, id);
        assert_eq!(a["blend_target"].as_str(), Some("BreathingIdle"));
        assert_eq!(a["clip"].as_str(), Some("BreathingIdle"));
        assert!(toml_f64(&a["blend_duration"]).unwrap() >= MIN_BLEND);
        assert_eq!(sync.state(&id).unwrap().fired_count(), 1);

        // Walk to exactly 1.5 in 0.1 steps: event at time == t fires
        for _ in 0..15 {
            sync.advance(&mut world, 0.1);
        }
        let a = animator(&world, id);
        assert_eq!(a["blend_target"].as_str(), Some("WalkCycle"));
        assert_eq!(sync.state(&id).unwrap().fired_count(), 2);

        // Past the layer event: fade fields written, no double fire
        for _ in 0..12 {
            sync.advance(&mut world, 0.1);
        }
        let a = animator(&world, id);
        let layer = &a["layers"].as_array().unwrap()[0];
        assert_eq!(layer["clip"].as_str(), Some("StarCower"));
        assert_eq!(layer["mask"].as_str(), Some("head"));
        assert_eq!(toml_f64(&layer["fade_target"]), Some(1.0));
        assert!((toml_f64(&layer["fade_duration"]).unwrap() - 0.3).abs() < 1e-9);
        assert_eq!(sync.state(&id).unwrap().fired_count(), 3);

        // Run out: cue drained once, sequence stops and clears its request
        for _ in 0..40 {
            sync.advance(&mut world, 0.1);
        }
        let cues = sync.drain_cues();
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].cue, "done");
        assert!(sync.drain_cues().is_empty());
        let rt = sync.state(&id).unwrap();
        assert!(!rt.playing);
        assert_eq!(rt.fired_count(), 6);
        assert_eq!(toml_f64(&animator(&world, id)["speed"]), Some(1.3));
    }

    #[test]
    fn loop_wrap_rearms_events() {
        let mut seq = load_sequence_from_str(EXAMPLE, "test").unwrap();
        seq.looping = true;
        seq.duration = Some(6.0);
        let (mut world, id) = world_with_animator();
        let mut sync = SequenceSync::new();
        sync.add_sequence(seq);
        sync.play(id, "showcase");
        for _ in 0..125 {
            sync.advance(&mut world, 0.1);
        }
        let rt = sync.state(&id).unwrap();
        assert!(rt.playing);
        assert!(rt.time < 6.0);
        // After the wrap the early events fired again; cue twice overall
        assert_eq!(sync.drain_cues().len(), 2);
    }

    #[test]
    fn loop_wrap_handles_large_dt() {
        let seq = load_sequence_from_str(
            r#"
            name = "pulse"
            loop = true

            [[events]]
            time = 0.0
            kind = "cue"
            name = "start"

            [[events]]
            time = 1.0
            kind = "cue"
            name = "step"
            "#,
            "test",
        )
        .unwrap();
        let (mut world, id) = world_with_animator();
        let mut sync = SequenceSync::new();
        sync.add_sequence(seq);
        sync.play(id, "pulse");

        sync.advance(&mut world, 2.5);

        let cues: Vec<_> = sync.drain_cues().iter().map(|c| c.cue.clone()).collect();
        assert_eq!(cues, vec!["start", "step", "start"]);
        let rt = sync.state(&id).unwrap();
        assert!(rt.playing);
        assert!((rt.time - 0.5).abs() < 1e-9);
    }

    #[test]
    fn sequence_field_drives_play_and_stop() {
        let seq = load_sequence_from_str(EXAMPLE, "test").unwrap();
        let (mut world, id) = world_with_animator();
        let mut sync = SequenceSync::new();
        sync.add_sequence(seq);

        world.get_components_mut(id).unwrap().set_field(
            comp::ANIMATOR,
            "sequence",
            "showcase".into(),
        );
        sync.sync_from_world(&world);
        assert!(sync.state(&id).is_some());

        // Same value next frame: no restart
        sync.advance(&mut world, 1.0);
        sync.sync_from_world(&world);
        assert!((sync.state(&id).unwrap().time - 1.0).abs() < 1e-9);

        world
            .get_components_mut(id)
            .unwrap()
            .set_field(comp::ANIMATOR, "sequence", "".into());
        sync.sync_from_world(&world);
        assert!(sync.state(&id).is_none());

        // Finishing a non-looping sequence clears the field itself
        world.get_components_mut(id).unwrap().set_field(
            comp::ANIMATOR,
            "sequence",
            "showcase".into(),
        );
        sync.sync_from_world(&world);
        sync.advance(&mut world, 10.0);
        assert_eq!(animator(&world, id)["sequence"].as_str(), Some(""));
    }

    /// The previewer starts sequences with `play()` and never writes the
    /// ECS field; an empty field must not read as a stop request.
    #[test]
    fn direct_play_survives_empty_sequence_field() {
        let seq = load_sequence_from_str(EXAMPLE, "test").unwrap();
        let (mut world, id) = world_with_animator();
        let mut sync = SequenceSync::new();
        sync.add_sequence(seq);
        sync.play(id, "showcase");
        sync.sync_from_world(&world);
        sync.advance(&mut world, 3.0);
        sync.sync_from_world(&world);
        let rt = sync.state(&id).expect("still active");
        assert_eq!(rt.fired_count(), 3);
        // Fresh slot faded in from 0, not from the 1.0 default
        let a = animator(&world, id);
        let layer = &a["layers"].as_array().unwrap()[0];
        assert_eq!(toml_f64(&layer["weight"]), Some(0.0));
    }

    #[test]
    fn replay_is_deterministic() {
        let seq = load_sequence_from_str(EXAMPLE, "test").unwrap();

        let run = |steps: &[f64]| {
            let (mut world, id) = world_with_animator();
            let mut sync = SequenceSync::new();
            sync.add_sequence(seq.clone());
            sync.play(id, "showcase");
            for dt in steps {
                sync.advance(&mut world, *dt);
            }
            animator(&world, id)
        };

        let fine: Vec<f64> = std::iter::repeat(0.1).take(30).collect();
        assert_eq!(run(&fine), run(&[3.0]));
        assert_eq!(run(&[1.0, 2.0]), run(&[3.0]));
    }
}
