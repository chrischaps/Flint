//! What the style commands scripts queue do (#269): `set_entity_style`,
//! `set_style_param`, `tween_style_param` and `clear_entity_style`, applied
//! to the world by `flint_render::entity_style` through [`StyleTweens`]. The
//! renderer reads the world after scripts each frame, so a change shows the
//! frame it was made.
//!
//! The whole-frame ones (#369), `set_post_style`, `set_post_style_param`
//! and `reset_post_style`, edit a post-process config through
//! `flint_render::post_style` instead ([`set_post_style`] and the two after
//! it). `tween_post_style_param` (#406) eases one of their numbers, a
//! `reveal` or a `blend`, through [`StyleTweens::tween_post`].
//!
//! The game core's dispatcher calls these (`game_core.rs`, #697, #725), for
//! the player and a headless capture alike, so a style command does the
//! same in each.

use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_render::entity_style;
use flint_render::post_style;
use flint_render::PostProcessConfig;
use flint_runtime::GameEvent;
use flint_script::{EntityStyle, StyleReader};

/// The event a style change fires to scripts' `on_event`, with
/// `#{ entity, style }` (`style` is `""` once cleared).
pub const STYLE_CHANGED: &str = "style_changed";

/// The event a whole-frame style change fires (#369), with
/// `#{ style, enabled }`: on `set_post_style` and `reset_post_style`.
pub const POST_STYLE_CHANGED: &str = "post_style_changed";

/// What applying one style command did.
#[derive(Debug)]
pub enum StyleApplied {
    /// Applied. Setting or clearing a style hands back the `style_changed`
    /// event to fire; a param change fires none (it may come every frame).
    Done(Option<GameEvent>),
    /// Refused, with what to tell the script's author.
    Failed(String),
}

/// An entity as an event's data gives it to scripts: its id, an int.
fn entity_value(entity: EntityId) -> toml::Value {
    toml::Value::Integer(entity.raw() as i64)
}

fn changed(entity: EntityId, style: &str) -> GameEvent {
    let mut data = toml::Table::new();
    data.insert("entity".into(), entity_value(entity));
    data.insert("style".into(), style.into());
    GameEvent::Custom {
        name: STYLE_CHANGED.into(),
        data: toml::Value::Table(data),
    }
}

/// What scripts' `get_entity_style`, `get_entity_style_variant` and
/// `get_style_param` read (#372): the renderer's own rules
/// ([`entity_style::drawn_style`], [`entity_style::style_setting`]) over a
/// copy of the post-process settings the frame is drawn with. The player
/// installs a fresh one whenever the renderer's settings change
/// ([`flint_render::SceneRenderer::post_config_generation`]).
pub struct SceneStyles {
    config: PostProcessConfig,
}

impl SceneStyles {
    /// A reader over `config`, the settings the frame is drawn with.
    pub fn new(config: PostProcessConfig) -> Self {
        Self { config }
    }
}

impl StyleReader for SceneStyles {
    fn entity_style(&self, world: &FlintWorld, id: EntityId) -> Option<EntityStyle> {
        entity_style::drawn_style(world, &self.config, id).map(|d| EntityStyle {
            style: d.style,
            variant: d.variant,
        })
    }

    fn style_param(&self, world: &FlintWorld, id: EntityId, param: &str) -> Option<toml::Value> {
        entity_style::style_setting(world, &self.config, id, param)
    }
}

/// The event a finished style tween fires to scripts' `on_event` (#370),
/// with `#{ entity, param }`.
pub const STYLE_TWEEN_DONE: &str = "style_tween_done";

/// How a style tween moves from its start to its target (#370).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Easing {
    Linear,
    /// Smoothstep: slow out, slow in. The default.
    Smooth,
    /// Cubic, slow out of the start.
    EaseIn,
    /// Cubic, slow into the target.
    EaseOut,
    /// Cubic both ends.
    EaseInOut,
}

impl Easing {
    pub const NAMES: &'static [&'static str] =
        &["linear", "smooth", "ease_in", "ease_out", "ease_in_out"];

    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "linear" => Self::Linear,
            "" | "smooth" => Self::Smooth,
            "ease_in" => Self::EaseIn,
            "ease_out" => Self::EaseOut,
            "ease_in_out" => Self::EaseInOut,
            _ => return None,
        })
    }

    /// The eased fraction of the way at `t` (0..1).
    pub fn apply(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::Smooth => t * t * (3.0 - 2.0 * t),
            Self::EaseIn => t * t * t,
            Self::EaseOut => 1.0 - (1.0 - t).powi(3),
            Self::EaseInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
        }
    }
}

#[derive(Debug)]
struct StyleTween {
    entity: EntityId,
    /// The style it was started on: a tween ends quietly once the entity
    /// is given another
    style: &'static str,
    param: String,
    from: Vec<f64>,
    to: Vec<f64>,
    seconds: f64,
    elapsed: f64,
    easing: Easing,
}

impl StyleTween {
    fn value(&self, k: f64) -> toml::Value {
        let at = |(a, b): (&f64, &f64)| a + (b - a) * k;
        if self.from.len() == 1 {
            toml::Value::Float(at((&self.from[0], &self.to[0])))
        } else {
            toml::Value::Array(
                self.from
                    .iter()
                    .zip(&self.to)
                    .map(|p| toml::Value::Float(at(p)))
                    .collect(),
            )
        }
    }
}

fn tween_done(entity: EntityId, param: &str) -> GameEvent {
    let mut data = toml::Table::new();
    data.insert("entity".into(), entity_value(entity));
    data.insert("param".into(), param.into());
    GameEvent::Custom {
        name: STYLE_TWEEN_DONE.into(),
        data: toml::Value::Table(data),
    }
}

/// The event a finished whole-frame style tween fires (#406), with
/// `#{ style, param }`.
pub const POST_STYLE_TWEEN_DONE: &str = "post_style_tween_done";

fn post_tween_done(style: &str, param: &str) -> GameEvent {
    let mut data = toml::Table::new();
    data.insert("style".into(), style.into());
    data.insert("param".into(), param.into());
    GameEvent::Custom {
        name: POST_STYLE_TWEEN_DONE.into(),
        data: toml::Value::Table(data),
    }
}

/// A whole-frame style's number easing (#406): `reveal`, `blend`, a
/// `reveal_*` key or a parameter.
#[derive(Debug)]
struct PostStyleTween {
    style: String,
    param: String,
    from: f64,
    to: f64,
    seconds: f64,
    elapsed: f64,
    easing: Easing,
}

impl PostStyleTween {
    fn value(&self, k: f64) -> toml::Value {
        toml::Value::Float(self.from + (self.to - self.from) * k)
    }
}

/// What one frame of style tweens did.
#[derive(Debug, Default)]
pub struct TweenStep {
    /// `style_tween_done` for each tween that reached its target
    pub events: Vec<GameEvent>,
    /// Tweens that couldn't write (the entity went away), for the log
    pub failures: Vec<String>,
}

/// The style tweens running (#370): the engine eases an entity's style
/// setting so a script doesn't have to call `set_style_param` from every
/// `on_update`. Stepped once a frame on game time, so a `--frame-step`
/// capture is the same every run.
///
/// A tween starts where the setting is (the entity's field, else the
/// scene's table, else the default) and begins to move the frame after
/// it was asked for. A new tween of the same setting replaces the old one
/// from wherever it had got to; a `set_style_param` of that setting stops
/// it (the script took over) and `clear_entity_style` stops all of the
/// entity's.
#[derive(Debug, Default)]
pub struct StyleTweens {
    tweens: Vec<StyleTween>,
    /// Whole-frame styles' numbers easing (#406)
    post: Vec<PostStyleTween>,
}

impl StyleTweens {
    /// How many tweens are running.
    pub fn len(&self) -> usize {
        self.tweens.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tweens.is_empty()
    }

    /// Whether `entity`'s `param` is easing.
    pub fn is_tweening(&self, entity: EntityId, param: &str) -> bool {
        self.tweens
            .iter()
            .any(|t| t.entity == entity && t.param == param)
    }

    /// Drop every tween (a new scene).
    pub fn clear(&mut self) {
        self.tweens.clear();
        self.post.clear();
    }

    /// Whether whole-frame style `style`'s `param` is easing (#406).
    pub fn is_tweening_post(&self, style: &str, param: &str) -> bool {
        self.post
            .iter()
            .any(|t| t.style == style && t.param == param)
    }

    /// How many whole-frame style tweens are running.
    pub fn post_len(&self) -> usize {
        self.post.len()
    }

    /// `tween_post_style_param` (#406): ease whole-frame style `style`'s
    /// number `param` (`reveal`, `blend`, a `reveal_*` key or a parameter)
    /// to `target` over `seconds` of game time, from where `config` has
    /// it (a style with no reveal starts at 0, so a tween of `reveal` from
    /// there tears the frame open). It moves from the next frame; a new
    /// tween of the same setting replaces the old one, and no time to ease
    /// over sets it now.
    pub fn tween_post(
        &mut self,
        config: &mut PostProcessConfig,
        style: &str,
        param: &str,
        target: f64,
        seconds: f64,
        easing: &str,
    ) -> StyleApplied {
        let Some(easing) = Easing::from_name(easing) else {
            return StyleApplied::Failed(format!(
                "tween_post_style_param: no easing `{easing}`; one of {}",
                Easing::NAMES.join(", ")
            ));
        };
        if !target.is_finite() {
            return StyleApplied::Failed(format!(
                "tween_post_style_param: `{param}` can't ease to {target}"
            ));
        }
        let from = match post_style::post_style_number(config, style, param) {
            Ok(n) => f64::from(n),
            Err(e) => return StyleApplied::Failed(format!("tween_post_style_param: {e}")),
        };
        self.stop_post(style, Some(param));
        let tween = PostStyleTween {
            style: style.to_string(),
            param: param.to_string(),
            from,
            to: target,
            seconds,
            elapsed: 0.0,
            easing,
        };
        // Written once now, at its start: a reveal the style didn't have
        // is then 0, so the frame stays live until it moves
        let k = if seconds > 0.0 { 0.0 } else { 1.0 };
        if let Err(e) = post_style::set_post_style_param(config, style, param, tween.value(k)) {
            return StyleApplied::Failed(format!("tween_post_style_param: {e}"));
        }
        if seconds.is_nan() || seconds <= 0.0 {
            return StyleApplied::Done(Some(post_tween_done(style, param)));
        }
        self.post.push(tween);
        StyleApplied::Done(None)
    }

    /// Stop whole-frame style `style`'s tweens: of `param`, or all of them.
    pub fn stop_post(&mut self, style: &str, param: Option<&str>) {
        self.post
            .retain(|t| !(t.style == style && param.is_none_or(|p| t.param == p)));
    }

    /// Move every whole-frame style tween on by `dt` seconds of game time
    /// and write its value into `config` (#406). `false` in the second
    /// place: nothing was running, so `config` is as it was.
    pub fn step_post(&mut self, config: &mut PostProcessConfig, dt: f64) -> (TweenStep, bool) {
        let mut out = TweenStep::default();
        if self.post.is_empty() {
            return (out, false);
        }
        self.post.retain_mut(|t| {
            t.elapsed += dt.max(0.0);
            let done = t.elapsed >= t.seconds * (1.0 - 1e-9);
            let k = if done {
                1.0
            } else {
                t.easing.apply(t.elapsed / t.seconds)
            };
            if let Err(e) = post_style::set_post_style_param(config, &t.style, &t.param, t.value(k))
            {
                out.failures.push(format!("tween_post_style_param: {e}"));
                return false;
            }
            if done {
                out.events.push(post_tween_done(&t.style, &t.param));
            }
            !done
        });
        (out, true)
    }

    /// `set_entity_style`: draw `entity` (and its subtree) in `style`. A
    /// running tween ends by itself once the style it eased is gone.
    pub fn set_entity_style(
        &mut self,
        world: &mut FlintWorld,
        entity: EntityId,
        style: &str,
    ) -> StyleApplied {
        match entity_style::set_entity_style(world, entity, style) {
            Ok(()) => StyleApplied::Done(Some(changed(entity, style))),
            Err(e) => StyleApplied::Failed(format!("set_entity_style: {e}")),
        }
    }

    /// `set_style_param`: one setting of the entity's style, now. A tween of
    /// that setting stops (the script took over).
    pub fn set_style_param(
        &mut self,
        world: &mut FlintWorld,
        entity: EntityId,
        param: &str,
        value: toml::Value,
    ) -> StyleApplied {
        self.stop(entity, Some(param));
        match entity_style::set_style_param(world, entity, param, value) {
            Ok(_) => StyleApplied::Done(None),
            Err(e) => StyleApplied::Failed(format!("set_style_param: {e}")),
        }
    }

    /// `clear_entity_style`: take the entity's own style off, and stop its
    /// tweens.
    pub fn clear_entity_style(&mut self, world: &mut FlintWorld, entity: EntityId) -> StyleApplied {
        self.stop(entity, None);
        let had = entity_style::clear_entity_style(world, entity);
        StyleApplied::Done(had.then(|| changed(entity, "")))
    }

    /// `tween_style_param`: ease `param` to `target` over `seconds` of game
    /// time, from where it is. `config` is the scene's post-process config,
    /// where a tween of a setting the entity doesn't carry starts. A new
    /// tween of the same setting replaces the old one from wherever it had
    /// got to; no time to ease over sets it now and is done now.
    #[allow(clippy::too_many_arguments)]
    pub fn tween(
        &mut self,
        world: &mut FlintWorld,
        config: &PostProcessConfig,
        entity: EntityId,
        param: &str,
        target: &toml::Value,
        seconds: f64,
        easing: &str,
    ) -> StyleApplied {
        let Some(easing) = Easing::from_name(easing) else {
            return StyleApplied::Failed(format!(
                "tween_style_param: no easing `{easing}`; one of {}",
                Easing::NAMES.join(", ")
            ));
        };
        let id = entity;
        let (style, from, to) =
            match entity_style::tween_endpoints(world, id, param, target, config) {
                Ok(ends) => ends,
                Err(e) => return StyleApplied::Failed(format!("tween_style_param: {e}")),
            };
        self.stop(id, Some(param));
        let tween = StyleTween {
            entity: id,
            style,
            param: param.to_string(),
            from,
            to,
            seconds,
            elapsed: 0.0,
            easing,
        };
        if seconds.is_nan() || seconds <= 0.0 {
            // No time to ease over: there now, done now
            return match entity_style::set_style_param(world, id, param, tween.value(1.0)) {
                Ok(_) => StyleApplied::Done(Some(tween_done(id, param))),
                Err(e) => StyleApplied::Failed(format!("tween_style_param: {e}")),
            };
        }
        self.tweens.push(tween);
        StyleApplied::Done(None)
    }

    fn stop(&mut self, entity: EntityId, param: Option<&str>) {
        self.tweens
            .retain(|t| !(t.entity == entity && param.is_none_or(|p| t.param == p)));
    }

    /// Move every tween on by `dt` seconds of game time and write its
    /// value. A tween that reaches its target writes it exactly, ends and
    /// fires `style_tween_done`.
    pub fn step(&mut self, world: &mut FlintWorld, dt: f64) -> TweenStep {
        let mut out = TweenStep::default();
        self.tweens.retain_mut(|t| {
            let id = t.entity;
            if entity_style::entity_style(world, id) != Some(t.style) {
                // Given another style (or none) since: nothing left to ease
                return false;
            }
            t.elapsed += dt.max(0.0);
            // A hair of slack so 60 steps of 1/60 s finish a 1 s tween
            let done = t.elapsed >= t.seconds * (1.0 - 1e-9);
            let k = if done {
                1.0
            } else {
                t.easing.apply(t.elapsed / t.seconds)
            };
            if let Err(e) = entity_style::set_style_param(world, id, &t.param, t.value(k)) {
                out.failures.push(format!("tween_style_param: {e}"));
                return false;
            }
            if done {
                out.events.push(tween_done(t.entity, &t.param));
            }
            !done
        });
        out
    }
}

fn post_changed(style: &str, config: &PostProcessConfig) -> GameEvent {
    let mut data = toml::Table::new();
    data.insert("style".into(), style.into());
    data.insert("enabled".into(), config.styles.is_enabled(style).into());
    GameEvent::Custom {
        name: POST_STYLE_CHANGED.into(),
        data: toml::Value::Table(data),
    }
}

/// `set_post_style` (#369): switch a whole-frame style on or off in
/// `config` (the renderer's, to be handed back with
/// `set_post_process_config`), firing `post_style_changed`.
pub fn set_post_style(config: &mut PostProcessConfig, style: &str, enabled: bool) -> StyleApplied {
    match post_style::set_post_style(config, style, enabled) {
        Ok(()) => StyleApplied::Done(Some(post_changed(style, config))),
        Err(e) => StyleApplied::Failed(format!("set_post_style: {e}")),
    }
}

/// `set_post_style_param`: one setting of a whole-frame style. Fires
/// nothing (it may come every frame).
pub fn set_post_style_param(
    config: &mut PostProcessConfig,
    style: &str,
    param: &str,
    value: toml::Value,
) -> StyleApplied {
    match post_style::set_post_style_param(config, style, param, value) {
        Ok(()) => StyleApplied::Done(None),
        Err(e) => StyleApplied::Failed(format!("set_post_style_param: {e}")),
    }
}

/// `reset_post_style`: put a whole-frame style back to `authored`, the
/// scene's own config, firing `post_style_changed`.
pub fn reset_post_style(
    config: &mut PostProcessConfig,
    authored: &PostProcessConfig,
    style: &str,
) -> StyleApplied {
    match post_style::reset_post_style(config, authored, style) {
        Ok(()) => StyleApplied::Done(Some(post_changed(style, config))),
        Err(e) => StyleApplied::Failed(format!("reset_post_style: {e}")),
    }
}
