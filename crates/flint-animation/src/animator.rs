//! Reading an `animator` component (ADR 0115): the one read the property,
//! node, skeletal and sequence syncs share, through the typed view, so a
//! wrong type or an unknown layer mode is reported once by entity, component
//! and field (`animator.layers[1].mode`) instead of reading as the default
//! in silence.

use crate::playback_state::{AnimLayer, LayerMode};
use flint_core::components as comp;
use flint_core::diagnostic::{self, codes, Diagnostic};
use flint_core::EntityId;
use flint_ecs::{ComponentView, FlintWorld};

/// The spellings a layer's `mode` takes, after trimming and lower-casing:
/// `replace` is `override`'s other name. Anything else is additive, and
/// reported.
pub const LAYER_MODES: &[&str] = &["additive", "override", "replace"];

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
    pub sequence: &'a str,
}

impl<'a> Animator<'a> {
    /// `entity`'s animator; all defaults when it has none.
    pub fn of(world: &'a FlintWorld, entity: EntityId) -> Self {
        Self::read(&world.view(entity, comp::ANIMATOR))
    }

    pub fn read(view: &ComponentView<'a>) -> Self {
        Self {
            clip: view.str("clip", ""),
            playing: view.bool("playing", false),
            autoplay: view.bool("autoplay", false),
            looping: view.bool("loop", true),
            speed: view.f64("speed", 1.0),
            blend_target: view.str("blend_target", ""),
            blend_duration: view.f32("blend_duration", 0.3),
            sequence: view.str("sequence", ""),
        }
    }

    /// `playing`, or `autoplay`: what starts a clip on first sight.
    pub fn starts_playing(&self) -> bool {
        self.playing || self.autoplay
    }
}

/// The layers an animator composes: `layers`, or when that is empty the
/// legacy `layer_clip` / `layer_weight` as one additive layer.
pub fn layers(view: &ComponentView<'_>) -> Vec<AnimLayer> {
    let written = view.tables("layers");
    if !written.is_empty() {
        return written.iter().map(layer).collect();
    }
    let legacy = view.str("layer_clip", "");
    if legacy.is_empty() {
        return Vec::new();
    }
    vec![AnimLayer::new(legacy, view.f32("layer_weight", 1.0))]
}

/// One entry of `animator.layers`; an entry that is no table is an inactive
/// slot (the view reported it).
pub fn layer(entry: &ComponentView<'_>) -> AnimLayer {
    if !entry.exists() {
        return AnimLayer::default();
    }
    let weight = entry.f32("weight", 1.0);
    AnimLayer {
        clip: entry.str("clip", "").to_string(),
        weight,
        fade_target: entry.f32("fade_target", weight),
        fade_duration: entry.f32("fade_duration", 0.0).max(0.0),
        mode: layer_mode(entry),
        mask: entry.str("mask", "").to_string(),
        time: 0.0,
        speed: entry.f64("speed", 1.0),
    }
}

/// A layer's `mode`, read as [`LayerMode::parse`] always read it (any case,
/// spaces trimmed), and reported when it is none of [`LAYER_MODES`].
fn layer_mode(entry: &ComponentView<'_>) -> LayerMode {
    let Some(written) = entry.opt_str("mode") else {
        return LayerMode::default();
    };
    let spelled = written.trim().to_ascii_lowercase();
    if !LAYER_MODES.contains(&spelled.as_str()) {
        entry.report("mode", || {
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
