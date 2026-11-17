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

use flint_core::tween::Tween;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_render::entity_style;
use flint_render::post_style;
use flint_render::stylize::StyleRegistry;
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
    /// Applied as [`Done`](Self::Done), with a value outside its setting's
    /// range clamped into it (#492): what to tell the script's author, a
    /// warning the host logs.
    Clamped(Option<GameEvent>, String),
}

impl StyleApplied {
    /// Whether the command was applied (with or without a clamp).
    pub fn applied(&self) -> bool {
        !matches!(self, StyleApplied::Failed(_))
    }
}

/// `set_style_param`'s words for a clamp of `entity`'s `style` setting:
/// "set_style_param: ascii on 'barrel': cell=400 is outside its range
/// 4..32; clamped to 32".
fn entity_note(
    function: &str,
    world: &FlintWorld,
    entity: EntityId,
    style: &str,
    note: &str,
) -> String {
    let on = match world.get_name(entity) {
        Some(name) => format!("'{name}'"),
        None => format!("entity {}", entity.raw()),
    };
    format!("{function}: {style} on {on}: {note}")
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
    /// The registry the frame is drawn with (#742).
    styles: StyleRegistry,
}

impl SceneStyles {
    /// A reader over `config`, the settings the frame is drawn with, and
    /// `styles`, the registry it is drawn with.
    pub fn new(config: PostProcessConfig, styles: StyleRegistry) -> Self {
        Self { config, styles }
    }
}

impl StyleReader for SceneStyles {
    fn entity_style(&self, world: &FlintWorld, id: EntityId) -> Option<EntityStyle> {
        entity_style::drawn_style(world, &self.styles, &self.config, id).map(|d| EntityStyle {
            style: d.style,
            variant: d.variant,
        })
    }

    fn style_param(&self, world: &FlintWorld, id: EntityId, param: &str) -> Option<toml::Value> {
        entity_style::style_setting(world, &self.styles, &self.config, id, param)
    }
}

/// The event a finished style tween fires to scripts' `on_event` (#370),
/// with `#{ entity, param }`.
pub const STYLE_TWEEN_DONE: &str = "style_tween_done";

/// How a style tween moves from its start to its target (#370): the
/// engine's one easing (#734), which every timed change names.
pub use flint_core::tween::Easing;

#[derive(Debug)]
struct StyleTween {
    entity: EntityId,
    /// The style it was started on: a tween ends quietly once the entity
    /// is given another
    style: &'static str,
    param: String,
    /// A number is a list of one; a colour, of three or four.
    tween: Tween<Vec<f64>>,
}

/// A style setting's value, from the numbers a tween eases: one is a
/// number, more a colour.
fn style_value(numbers: Vec<f64>) -> toml::Value {
    match numbers[..] {
        [one] => toml::Value::Float(one),
        _ => toml::Value::Array(numbers.into_iter().map(toml::Value::Float).collect()),
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

/// The event a look's cross-fade fires when it reaches the new look (#496:
/// `set_look(name, seconds)`, `clear_look(seconds)`, F4's picker with a fade
/// time), with `#{ look }`: its name, `""` for none.
pub const LOOK_FADED: &str = "look_faded";

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
    tween: Tween<f64>,
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
    #[allow(clippy::too_many_arguments)]
    pub fn tween_post(
        &mut self,
        config: &mut PostProcessConfig,
        styles: &StyleRegistry,
        style: &str,
        param: &str,
        target: f64,
        seconds: f64,
        easing: &str,
    ) -> StyleApplied {
        let easing = match Easing::parse(easing, "tween_post_style_param") {
            Ok(easing) => easing,
            Err(e) => return StyleApplied::Failed(e),
        };
        if !target.is_finite() {
            return StyleApplied::Failed(format!(
                "tween_post_style_param: `{param}` can't ease to {target}"
            ));
        }
        let from = match post_style::post_style_number(config, styles, style, param) {
            Ok(n) => f64::from(n),
            Err(e) => return StyleApplied::Failed(format!("tween_post_style_param: {e}")),
        };
        // A target past the range is clamped at each step; say so once,
        // here, from what setting it would do (on a copy)
        let note = match post_style::set_post_style_param(
            &mut config.clone(),
            styles,
            style,
            param,
            target.into(),
        ) {
            Ok(note) => note.map(|n| format!("tween_post_style_param: {style}.{n}")),
            Err(e) => return StyleApplied::Failed(format!("tween_post_style_param: {e}")),
        };
        let applied = |event: Option<GameEvent>| match note {
            Some(note) => StyleApplied::Clamped(event, note),
            None => StyleApplied::Done(event),
        };
        self.stop_post(style, Some(param));
        let tween = PostStyleTween {
            style: style.to_string(),
            param: param.to_string(),
            tween: Tween::new(from, target, seconds, easing),
        };
        // Written once now, at its start (its target, with no time to ease
        // over): a reveal the style didn't have is then 0, so the frame
        // stays live until it moves
        let now = toml::Value::Float(tween.tween.value());
        if let Err(e) = post_style::set_post_style_param(config, styles, style, param, now) {
            return StyleApplied::Failed(format!("tween_post_style_param: {e}"));
        }
        if tween.tween.done() {
            return applied(Some(post_tween_done(style, param)));
        }
        self.post.push(tween);
        applied(None)
    }

    /// Stop whole-frame style `style`'s tweens: of `param`, or all of them.
    pub fn stop_post(&mut self, style: &str, param: Option<&str>) {
        self.post
            .retain(|t| !(t.style == style && param.is_none_or(|p| t.param == p)));
    }

    /// Move every whole-frame style tween on by `dt` seconds of game time
    /// and write its value into `config` (#406). `false` in the second
    /// place: nothing was running, so `config` is as it was.
    pub fn step_post(
        &mut self,
        config: &mut PostProcessConfig,
        styles: &StyleRegistry,
        dt: f64,
    ) -> (TweenStep, bool) {
        let mut out = TweenStep::default();
        if self.post.is_empty() {
            return (out, false);
        }
        self.post.retain_mut(|t| {
            let value = toml::Value::Float(t.tween.step(dt));
            if let Err(e) =
                post_style::set_post_style_param(config, styles, &t.style, &t.param, value)
            {
                out.failures.push(format!("tween_post_style_param: {e}"));
                return false;
            }
            let done = t.tween.done();
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
        styles: &StyleRegistry,
        entity: EntityId,
        style: &str,
    ) -> StyleApplied {
        match entity_style::set_entity_style(world, styles, entity, style) {
            Ok(()) => StyleApplied::Done(Some(changed(entity, style))),
            Err(e) => StyleApplied::Failed(format!("set_entity_style: {e}")),
        }
    }

    /// `set_style_param`: one setting of the entity's style, now. A tween of
    /// that setting stops (the script took over).
    pub fn set_style_param(
        &mut self,
        world: &mut FlintWorld,
        styles: &StyleRegistry,
        entity: EntityId,
        param: &str,
        value: toml::Value,
    ) -> StyleApplied {
        self.stop(entity, Some(param));
        match entity_style::set_style_param(world, styles, entity, param, value) {
            Ok(set) => match set.clamped {
                None => StyleApplied::Done(None),
                Some(note) => StyleApplied::Clamped(
                    None,
                    entity_note("set_style_param", world, entity, set.style, &note),
                ),
            },
            Err(e) => StyleApplied::Failed(format!("set_style_param: {e}")),
        }
    }

    /// `clear_entity_style`: take the entity's own style off, and stop its
    /// tweens.
    pub fn clear_entity_style(
        &mut self,
        world: &mut FlintWorld,
        styles: &StyleRegistry,
        entity: EntityId,
    ) -> StyleApplied {
        self.stop(entity, None);
        let had = entity_style::clear_entity_style(world, styles, entity);
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
        styles: &StyleRegistry,
        config: &PostProcessConfig,
        entity: EntityId,
        param: &str,
        target: &toml::Value,
        seconds: f64,
        easing: &str,
    ) -> StyleApplied {
        let easing = match Easing::parse(easing, "tween_style_param") {
            Ok(easing) => easing,
            Err(e) => return StyleApplied::Failed(e),
        };
        let id = entity;
        let ends = match entity_style::tween_endpoints(world, styles, id, param, target, config) {
            Ok(ends) => ends,
            Err(e) => return StyleApplied::Failed(format!("tween_style_param: {e}")),
        };
        // A target past the range eases to the range's end, and says so
        let note = ends
            .clamped
            .as_deref()
            .map(|n| entity_note("tween_style_param", world, id, ends.style, n));
        let applied = |event: Option<GameEvent>| match note {
            Some(note) => StyleApplied::Clamped(event, note),
            None => StyleApplied::Done(event),
        };
        self.stop(id, Some(param));
        let tween = StyleTween {
            entity: id,
            style: ends.style,
            param: param.to_string(),
            tween: Tween::new(ends.from, ends.to, seconds, easing),
        };
        if tween.tween.done() {
            // No time to ease over: there now, done now
            let there = style_value(tween.tween.value());
            return match entity_style::set_style_param(world, styles, id, param, there) {
                Ok(_) => applied(Some(tween_done(id, param))),
                Err(e) => StyleApplied::Failed(format!("tween_style_param: {e}")),
            };
        }
        self.tweens.push(tween);
        applied(None)
    }

    fn stop(&mut self, entity: EntityId, param: Option<&str>) {
        self.tweens
            .retain(|t| !(t.entity == entity && param.is_none_or(|p| t.param == p)));
    }

    /// Move every tween on by `dt` seconds of game time and write its
    /// value. A tween that reaches its target writes it exactly, ends and
    /// fires `style_tween_done`.
    pub fn step(&mut self, world: &mut FlintWorld, styles: &StyleRegistry, dt: f64) -> TweenStep {
        let mut out = TweenStep::default();
        self.tweens.retain_mut(|t| {
            let id = t.entity;
            if entity_style::entity_style(world, styles, id) != Some(t.style) {
                // Given another style (or none) since: nothing left to ease
                return false;
            }
            // 60 steps of 1/60 s finish a 1 s tween (`Tween::done`'s slack)
            let value = style_value(t.tween.step(dt));
            if let Err(e) = entity_style::set_style_param(world, styles, id, &t.param, value) {
                out.failures.push(format!("tween_style_param: {e}"));
                return false;
            }
            let done = t.tween.done();
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
pub fn set_post_style(
    config: &mut PostProcessConfig,
    styles: &StyleRegistry,
    style: &str,
    enabled: bool,
) -> StyleApplied {
    match post_style::set_post_style(config, styles, style, enabled) {
        Ok(()) => StyleApplied::Done(Some(post_changed(style, config))),
        Err(e) => StyleApplied::Failed(format!("set_post_style: {e}")),
    }
}

/// `set_post_style_param`: one setting of a whole-frame style. Fires
/// nothing (it may come every frame).
pub fn set_post_style_param(
    config: &mut PostProcessConfig,
    styles: &StyleRegistry,
    style: &str,
    param: &str,
    value: toml::Value,
) -> StyleApplied {
    match post_style::set_post_style_param(config, styles, style, param, value) {
        Ok(None) => StyleApplied::Done(None),
        Ok(Some(note)) => {
            StyleApplied::Clamped(None, format!("set_post_style_param: {style}.{note}"))
        }
        Err(e) => StyleApplied::Failed(format!("set_post_style_param: {e}")),
    }
}

/// `reset_post_style`: put a whole-frame style back to `authored`, the
/// scene's own config, firing `post_style_changed`.
pub fn reset_post_style(
    config: &mut PostProcessConfig,
    styles: &StyleRegistry,
    authored: &PostProcessConfig,
    style: &str,
) -> StyleApplied {
    match post_style::reset_post_style(config, styles, authored, style) {
        Ok(()) => StyleApplied::Done(Some(post_changed(style, config))),
        Err(e) => StyleApplied::Failed(format!("reset_post_style: {e}")),
    }
}
