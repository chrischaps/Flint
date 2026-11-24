//! The `animator` component: the one file that spells its keys (#733).
//!
//! - **Reading** (ADR 0115): [`Animator`] and [`layers`], the one read the
//!   property, node, skeletal and sequence syncs share, through the typed
//!   view, so a wrong type or an unknown layer mode is reported once by
//!   entity, component and field (`animator.layers[1].mode`) instead of
//!   reading as the default in silence.
//! - **Following it**: [`Animator::start`] and [`Animator::apply_to`], one
//!   rule set for what a clip, `playing`, `speed`, `loop` and `blend_target`
//!   do to a [`ClipPlaybackState`], whichever kind of clip plays; and
//!   [`publish`], which tells the animator what the runtime changed.
//! - **Writing**: [`write`], what scripts, sequences, the previewer and the
//!   syncs' write-backs set, and [`edit_layer`] for one `layers` entry.

use crate::playback_state::{has_finished, start_time, AnimLayer, ClipPlaybackState, LayerMode};
use flint_core::components as comp;
use flint_core::diagnostic::{self, codes, Diagnostic};
use flint_core::tween::Easing;
use flint_core::EntityId;
use flint_ecs::{ComponentView, ComponentsMut, FlintWorld};

/// The spellings a layer's `mode` takes, after trimming and lower-casing:
/// `replace` is `override`'s other name. Anything else is additive, and
/// reported.
pub const LAYER_MODES: &[&str] = &["additive", "override", "replace"];

/// A crossfade's length when `blend_duration` isn't written.
pub const DEFAULT_BLEND_SECONDS: f32 = 0.3;

// The keys. Nothing outside this file names one (a test in
// `tests/animator_keys.rs` holds it).
const CLIP: &str = "clip";
const PLAYING: &str = "playing";
const AUTOPLAY: &str = "autoplay";
const LOOP: &str = "loop";
const SPEED: &str = "speed";
const BLEND_TARGET: &str = "blend_target";
const BLEND_DURATION: &str = "blend_duration";
const BLEND_EASING: &str = "blend_easing";
const SEQUENCE: &str = "sequence";
const LAYERS: &str = "layers";
const LEGACY_LAYER_CLIP: &str = "layer_clip";
const LEGACY_LAYER_WEIGHT: &str = "layer_weight";
// ...and those of one `layers` entry.
const WEIGHT: &str = "weight";
const MODE: &str = "mode";
const MASK: &str = "mask";
const FADE_TARGET: &str = "fade_target";
const FADE_DURATION: &str = "fade_duration";
const FADE_EASING: &str = "fade_easing";

/// The scalar fields of an `animator`, with the defaults every sync used.
#[derive(Debug, Clone, PartialEq)]
pub struct Animator<'a> {
    pub clip: &'a str,
    pub playing: bool,
    pub autoplay: bool,
    pub looping: bool,
    pub speed: f64,
    pub blend_target: &'a str,
    pub blend_duration: f32,
    /// How a crossfade's weight moves (#1248): linear unless written.
    pub blend_easing: Easing,
    pub sequence: &'a str,
}

impl<'a> Animator<'a> {
    /// `entity`'s animator; all defaults when it has none.
    pub fn of(world: &'a FlintWorld, entity: EntityId) -> Self {
        Self::read(&world.view(entity, comp::ANIMATOR))
    }

    pub fn read(view: &ComponentView<'a>) -> Self {
        Self {
            clip: view.str(CLIP, ""),
            playing: view.bool(PLAYING, false),
            autoplay: view.bool(AUTOPLAY, false),
            looping: view.bool(LOOP, true),
            speed: view.f64(SPEED, 1.0),
            blend_target: view.str(BLEND_TARGET, ""),
            blend_duration: view.f32(BLEND_DURATION, DEFAULT_BLEND_SECONDS),
            // A crossfade written before easings (#1248) is linear, as it was
            blend_easing: Easing::from_name(view.choice(BLEND_EASING, Easing::NAMES, "linear"))
                .unwrap_or(Easing::Linear),
            sequence: view.str(SEQUENCE, ""),
        }
    }

    /// `playing`, or `autoplay`: what starts a clip on first sight.
    pub fn starts_playing(&self) -> bool {
        self.playing || self.autoplay
    }

    /// The playback this animator starts on first sight, or `None` while
    /// its `clip` is empty or not one `duration_of` knows (seconds long).
    ///
    /// It starts at the clip's start for its direction of play, playing if
    /// `playing` or `autoplay` says so, with `blend_target` armed (a skinned
    /// entity retires one it doesn't know). When
    /// `autoplay` started it the state is unpublished: the animator's
    /// `playing = false` must not read as a `stop_clip` next frame.
    pub fn start(&self, duration_of: impl Fn(&str) -> Option<f64>) -> Option<ClipPlaybackState> {
        if self.clip.is_empty() {
            return None;
        }
        let duration = duration_of(self.clip)?;
        let playing = self.starts_playing();
        let mut state =
            ClipPlaybackState::new(self.clip.to_string(), self.speed, self.looping, playing);
        state.time = start_time(duration, self.speed);
        state.blend_target = self.blend_target.to_string();
        state.blend_duration = self.blend_duration;
        state.blend_easing = self.blend_easing;
        state.unpublished = playing != self.playing;
        Some(state)
    }

    /// Follow this animator on a state already playing: the one rule set
    /// for property, node and skeletal clips (#732, #733).
    ///
    /// - `speed` and `loop` are dials, copied every frame.
    /// - A new `clip` (one `duration_of` knows) cuts to it from its start
    ///   and cancels a crossfade: `play_clip`.
    /// - `playing` is live: false pauses, true resumes where it stopped, and
    ///   turning a finished one-shot back on replays it from its start.
    /// - Neither is read while the state is unpublished (the runtime moved
    ///   them and the animator doesn't say so yet; see [`publish`]).
    /// - A new `blend_target` (a known clip) starts a crossfade to it over
    ///   `blend_duration`, moving as `blend_easing` says, even in the frame
    ///   a `clip` changed. Both are read when the crossfade starts.
    pub fn apply_to(
        &self,
        state: &mut ClipPlaybackState,
        duration_of: impl Fn(&str) -> Option<f64>,
    ) {
        state.speed = self.speed;
        state.looping = self.looping;

        if !state.unpublished {
            let new_clip = !self.clip.is_empty() && self.clip != state.clip_name;
            if new_clip {
                if let Some(duration) = duration_of(self.clip) {
                    state.clip_name = self.clip.to_string();
                    state.time = start_time(duration, state.speed);
                    state.blend_target.clear();
                    state.blend_elapsed = 0.0;
                    state.clear_fired_events();
                }
            } else if self.playing && !state.playing && !state.looping {
                // A one-shot that ended replays from its start; one stopped
                // part-way resumes where it was.
                if let Some(duration) = duration_of(&state.clip_name) {
                    if has_finished(state.time, duration, state.speed) {
                        state.time = start_time(duration, state.speed);
                        state.clear_fired_events();
                    }
                }
            }
            state.playing = self.playing;
        }

        if !self.blend_target.is_empty()
            && self.blend_target != state.blend_target
            && duration_of(self.blend_target).is_some()
        {
            state.blend_target = self.blend_target.to_string();
            state.blend_duration = self.blend_duration;
            state.blend_easing = self.blend_easing;
            state.blend_elapsed = 0.0;
        }
    }
}

/// Tell the animator what the runtime changed on `state`: the clip a
/// crossfade settled on, a one-shot's end, an autoplay's start. `clip` and
/// `playing` are live ([`Animator::apply_to`]), so they must say what plays.
/// Does nothing when the state is published already. Every sync calls it
/// once a frame after advancing, before the next `apply_to`.
pub fn publish(state: &mut ClipPlaybackState, comps: &mut ComponentsMut<'_>) {
    if !state.unpublished {
        return;
    }
    write(comps).clip(&state.clip_name).playing(state.playing);
    state.unpublished = false;
}

/// The layers an animator composes: `layers`, or when that is empty the
/// legacy `layer_clip` / `layer_weight` as one additive layer.
pub fn layers(view: &ComponentView<'_>) -> Vec<AnimLayer> {
    let written = view.tables(LAYERS);
    if !written.is_empty() {
        return written.iter().map(layer).collect();
    }
    let legacy = view.str(LEGACY_LAYER_CLIP, "");
    if legacy.is_empty() {
        return Vec::new();
    }
    vec![AnimLayer::new(legacy, view.f32(LEGACY_LAYER_WEIGHT, 1.0))]
}

/// One entry of `animator.layers`; an entry that is no table is an inactive
/// slot (the view reported it).
pub fn layer(entry: &ComponentView<'_>) -> AnimLayer {
    if !entry.exists() {
        return AnimLayer::default();
    }
    let weight = entry.f32(WEIGHT, 1.0);
    AnimLayer {
        clip: entry.str(CLIP, "").to_string(),
        weight,
        fade_target: entry.f32(FADE_TARGET, weight),
        fade_duration: entry.f32(FADE_DURATION, 0.0).max(0.0),
        // A fade written before easings (#734) is linear, as it was
        fade_easing: Easing::from_name(entry.choice(FADE_EASING, Easing::NAMES, "linear"))
            .unwrap_or(Easing::Linear),
        mode: layer_mode(entry),
        mask: entry.str(MASK, "").to_string(),
        time: 0.0,
        speed: entry.f64(SPEED, 1.0),
    }
}

/// A layer's `mode`, read as [`LayerMode::parse`] always read it (any case,
/// spaces trimmed), and reported when it is none of [`LAYER_MODES`].
fn layer_mode(entry: &ComponentView<'_>) -> LayerMode {
    let Some(written) = entry.opt_str(MODE) else {
        return LayerMode::default();
    };
    let spelled = written.trim().to_ascii_lowercase();
    if !LAYER_MODES.contains(&spelled.as_str()) {
        entry.report(MODE, || {
            let mut hint = String::new();
            if let Some(near) = diagnostic::did_you_mean(&spelled, LAYER_MODES.iter().copied()) {
                hint.push_str(&near);
                hint.push(' ');
            }
            hint.push_str("allowed values: ");
            hint.push_str(&LAYER_MODES.join(", "));
            hint.push_str("; the layer is additive");
            Diagnostic::error(
                codes::BAD_ENUM_VALUE,
                format!("\"{written}\" is not a value of `mode`"),
            )
            .hint(hint)
        });
    }
    LayerMode::parse(&spelled)
}

// ── Writing ─────────────────────────────────────────────────────────────

/// A whole `animator`, as a host makes one for an entity it animates (the
/// model previewer): `clip` (empty until one is chosen), `playing`, `loop`
/// and `speed`.
pub fn component(clip: &str, playing: bool, looping: bool, speed: f64) -> toml::Value {
    let mut a = toml::map::Map::new();
    a.insert(CLIP.into(), toml::Value::String(clip.into()));
    a.insert(PLAYING.into(), toml::Value::Boolean(playing));
    a.insert(LOOP.into(), toml::Value::Boolean(looping));
    a.insert(SPEED.into(), toml::Value::Float(speed));
    toml::Value::Table(a)
}

/// Write fields of the entity's `animator` (made if it has none), which
/// the animation system follows next frame:
/// `write(&mut comps).clip("wave").playing(true)`.
pub fn write<'c, 'w>(comps: &'c mut ComponentsMut<'w>) -> AnimatorWrite<'c, 'w> {
    AnimatorWrite { comps }
}

/// What [`write`] returns; each setter writes one field.
pub struct AnimatorWrite<'c, 'w> {
    comps: &'c mut ComponentsMut<'w>,
}

impl AnimatorWrite<'_, '_> {
    fn set(&mut self, field: &str, value: toml::Value) -> &mut Self {
        self.comps.set_field(comp::ANIMATOR, field, value);
        self
    }

    pub fn clip(&mut self, clip: &str) -> &mut Self {
        self.set(CLIP, toml::Value::String(clip.into()))
    }

    pub fn playing(&mut self, playing: bool) -> &mut Self {
        self.set(PLAYING, toml::Value::Boolean(playing))
    }

    pub fn looping(&mut self, looping: bool) -> &mut Self {
        self.set(LOOP, toml::Value::Boolean(looping))
    }

    pub fn speed(&mut self, speed: f64) -> &mut Self {
        self.set(SPEED, toml::Value::Float(speed))
    }

    /// Crossfade to `target` over `seconds`, eased as `blend_easing`
    /// already says (linear when nothing wrote it).
    pub fn blend(&mut self, target: &str, seconds: f64) -> &mut Self {
        self.set(BLEND_TARGET, toml::Value::String(target.into()))
            .set(BLEND_DURATION, toml::Value::Float(seconds))
    }

    /// [`Self::blend`], moving as `easing` says (#1248). The easing stays
    /// written: a later [`Self::blend`] eases the same way.
    pub fn blend_eased(&mut self, target: &str, seconds: f64, easing: Easing) -> &mut Self {
        self.blend(target, seconds)
            .set(BLEND_EASING, easing.name().into())
    }

    /// No crossfade asked for: what `play_clip` writes so one in flight
    /// can't settle on its target, and what a settled one retires to.
    pub fn no_blend(&mut self) -> &mut Self {
        self.set(BLEND_TARGET, toml::Value::String(String::new()))
    }

    /// Play the `*.sequence.toml` named `name`; empty stops it.
    pub fn sequence(&mut self, name: &str) -> &mut Self {
        self.set(SEQUENCE, toml::Value::String(name.into()))
    }

    /// Replace the whole layer list.
    pub fn layers(&mut self, layers: &[AnimLayer]) -> &mut Self {
        let list = layers.iter().map(layer_table).collect();
        self.set(LAYERS, toml::Value::Array(list))
    }

    /// Publish runtime layer weights onto the entries already written, as
    /// `(index, weight, fade_done)`: a done fade has its `fade_duration`
    /// zeroed so the next sync doesn't re-arm it.
    pub fn layer_weights(
        &mut self,
        weights: impl IntoIterator<Item = (usize, f32, bool)>,
    ) -> &mut Self {
        let Some(list) = self
            .comps
            .get_mut(comp::ANIMATOR)
            .and_then(|a| a.get_mut(LAYERS))
            .and_then(|l| l.as_array_mut())
        else {
            return self;
        };
        for (index, weight, fade_done) in weights {
            let Some(table) = list.get_mut(index).and_then(|v| v.as_table_mut()) else {
                continue;
            };
            table.insert(WEIGHT.into(), toml::Value::Float(weight as f64));
            if fade_done {
                table.insert(FADE_DURATION.into(), toml::Value::Float(0.0));
            }
        }
        self
    }
}

/// One `animator.layers` entry, as the layer list writes it.
fn layer_table(layer: &AnimLayer) -> toml::Value {
    let mut t = toml::map::Map::new();
    t.insert(CLIP.into(), toml::Value::String(layer.clip.clone()));
    t.insert(WEIGHT.into(), toml::Value::Float(layer.weight as f64));
    t.insert(MODE.into(), toml::Value::String(layer.mode.as_str().into()));
    t.insert(MASK.into(), toml::Value::String(layer.mask.clone()));
    t.insert(SPEED.into(), toml::Value::Float(layer.speed));
    if layer.fade_duration > 0.0 {
        t.insert(
            FADE_TARGET.into(),
            toml::Value::Float(layer.fade_target as f64),
        );
        t.insert(
            FADE_DURATION.into(),
            toml::Value::Float(layer.fade_duration as f64),
        );
        t.insert(FADE_EASING.into(), layer.fade_easing.name().into());
    }
    toml::Value::Table(t)
}

/// Edit entry `index` of `animator.layers` in place, the one way the Rhai
/// layer calls and the sequence runner change a layer, so the two never
/// disagree on slot padding or the legacy fields.
///
/// A legacy `layer_clip` / `layer_weight` pair moves into slot 0 first
/// (and `layer_clip` is emptied after), slots below `index` are padded with
/// inactive (empty-clip) entries so indices stay stable, and an index at or
/// past [`u8::MAX`] does nothing: layer ids are a `u8` in the composition
/// bookkeeping.
pub fn edit_layer(comps: &mut ComponentsMut<'_>, index: usize, edit: impl FnOnce(&mut LayerEdit)) {
    if index >= u8::MAX as usize {
        return;
    }
    let mut list = {
        let animator = comps.get(comp::ANIMATOR);
        let field = |key: &str| animator.and_then(|a| a.get(key));
        let mut list = field(LAYERS)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let legacy = field(LEGACY_LAYER_CLIP)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if list.is_empty() && !legacy.is_empty() {
            let weight = field(LEGACY_LAYER_WEIGHT)
                .and_then(flint_core::toml_util::toml_f64)
                .unwrap_or(1.0);
            let mut t = toml::map::Map::new();
            t.insert(CLIP.into(), toml::Value::String(legacy.to_string()));
            t.insert(WEIGHT.into(), toml::Value::Float(weight));
            list.push(toml::Value::Table(t));
        }
        list
    };
    while list.len() <= index {
        let mut t = toml::map::Map::new();
        t.insert(CLIP.into(), toml::Value::String(String::new()));
        list.push(toml::Value::Table(t));
    }
    if !list[index].is_table() {
        list[index] = toml::Value::Table(Default::default());
    }
    if let Some(table) = list[index].as_table_mut() {
        edit(&mut LayerEdit { table });
    }
    comps.set_field(comp::ANIMATOR, LAYERS, toml::Value::Array(list));
    comps.set_field(
        comp::ANIMATOR,
        LEGACY_LAYER_CLIP,
        toml::Value::String(String::new()),
    );
}

/// One `animator.layers` entry being edited by [`edit_layer`].
pub struct LayerEdit<'t> {
    table: &'t mut toml::map::Map<String, toml::Value>,
}

impl LayerEdit<'_> {
    fn set(&mut self, field: &str, value: toml::Value) -> &mut Self {
        self.table.insert(field.into(), value);
        self
    }

    /// The slot plays nothing (no clip, or an empty one).
    pub fn is_inactive(&self) -> bool {
        self.table
            .get(CLIP)
            .and_then(|v| v.as_str())
            .is_none_or(str::is_empty)
    }

    /// A weight is written on the entry.
    pub fn has_weight(&self) -> bool {
        self.table.contains_key(WEIGHT)
    }

    /// The clip it plays; empty deactivates the slot, keeping its index.
    pub fn clip(&mut self, clip: &str) -> &mut Self {
        self.set(CLIP, toml::Value::String(clip.into()))
    }

    /// `additive` or `override` (see [`LAYER_MODES`]).
    pub fn mode(&mut self, mode: &str) -> &mut Self {
        self.set(MODE, toml::Value::String(mode.into()))
    }

    /// Root joint of the subtree it touches; empty for every joint.
    pub fn mask(&mut self, mask: &str) -> &mut Self {
        self.set(MASK, toml::Value::String(mask.into()))
    }

    /// Start from silence: weight 0, leaving any fade asked for in place.
    pub fn silent(&mut self) -> &mut Self {
        self.set(WEIGHT, toml::Value::Float(0.0))
    }

    /// Set the weight at once, cancelling any fade in flight.
    pub fn weight(&mut self, weight: f64) -> &mut Self {
        self.set(WEIGHT, toml::Value::Float(weight)).no_fade()
    }

    /// Stop a fade where it is.
    pub fn no_fade(&mut self) -> &mut Self {
        self.set(FADE_DURATION, toml::Value::Float(0.0))
    }

    /// Ramp the weight toward `weight` over `seconds`, linearly (at once
    /// when `seconds <= 0`).
    pub fn fade(&mut self, weight: f64, seconds: f64) -> &mut Self {
        self.fade_eased(weight, seconds, Easing::Linear)
    }

    /// [`Self::fade`], moving as `easing` says (#734).
    pub fn fade_eased(&mut self, weight: f64, seconds: f64, easing: Easing) -> &mut Self {
        if seconds <= 0.0 {
            return self.weight(weight);
        }
        self.set(FADE_TARGET, toml::Value::Float(weight))
            .set(FADE_DURATION, toml::Value::Float(seconds))
            .set(FADE_EASING, easing.name().into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world_with(text: &str) -> (FlintWorld, EntityId) {
        let mut world = FlintWorld::new();
        let id = world.spawn("hero").unwrap();
        let data: toml::Value = toml::from_str(text).unwrap();
        world.set_component(id, comp::ANIMATOR, data).unwrap();
        (world, id)
    }

    fn said(world: &FlintWorld) -> Vec<String> {
        world
            .read_reports()
            .snapshot()
            .iter()
            .map(|d| d.to_string())
            .collect()
    }

    #[test]
    fn a_wrong_type_reads_as_the_default_and_is_reported_once() {
        let (world, id) = world_with("clip = \"walk\"\nspeed = \"fast\"\nloop = 1\nplaying = true");
        for _ in 0..3 {
            let animator = Animator::of(&world, id);
            assert_eq!(animator.speed, 1.0);
            assert!(animator.looping);
            assert!(animator.starts_playing());
        }
        assert_eq!(
            said(&world),
            [
                "error[bad-type] entity 'hero' animator.loop: expected true or false, got integer 1",
                "error[bad-type] entity 'hero' animator.speed: expected a number, got string \"fast\"",
            ]
        );
    }

    #[test]
    fn an_unknown_layer_mode_is_additive_and_named_by_its_entry() {
        let (world, id) = world_with(
            "layers = [{ clip = \"wave\", mode = \"Override\" }, { clip = \"nod\", mode = \"overide\" }]",
        );
        for _ in 0..3 {
            let layers = layers(&world.view(id, comp::ANIMATOR));
            assert_eq!(layers[0].mode, LayerMode::Override, "any case, as before");
            assert_eq!(layers[1].mode, LayerMode::Additive);
        }
        let reports = world.read_reports().snapshot();
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert_eq!(
            reports[0].one_line(),
            "error[bad-enum-value] entity 'hero' animator.layers[1].mode: \"overide\" is not a \
             value of `mode` (did you mean `override`? allowed values: additive, override, \
             replace; the layer is additive)"
        );
        assert_eq!(reports[0].component.as_deref(), Some(comp::ANIMATOR));
        assert_eq!(reports[0].field.as_deref(), Some("layers[1].mode"));
    }

    #[test]
    fn a_legacy_layer_reads_its_weight_through_the_view() {
        let (world, id) = world_with("layer_clip = \"wave\"\nlayer_weight = \"full\"");
        for _ in 0..10 {
            assert_eq!(layers(&world.view(id, comp::ANIMATOR))[0].weight, 1.0);
        }
        assert_eq!(world.read_reports().len(), 1);
    }

    #[test]
    fn a_good_animator_reports_nothing() {
        let (world, id) = world_with(
            "clip = \"walk\"\nspeed = 2\nloop = false\nblend_target = \"run\"\n\
             blend_duration = 1\nlayers = [{ clip = \"wave\", weight = 0.5, mode = \"replace\" }]",
        );
        let animator = Animator::of(&world, id);
        assert_eq!(animator.speed, 2.0);
        assert_eq!(animator.blend_duration, 1.0);
        assert!(!animator.looping);
        let layers = layers(&world.view(id, comp::ANIMATOR));
        assert_eq!(layers[0].weight, 0.5);
        assert_eq!(layers[0].mode, LayerMode::Override);
        assert!(world.read_reports().is_empty());
    }
}
