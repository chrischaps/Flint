//! `[post_process.styles.<name>]`: each style's settings in a config
//! (moved from `stylize/mod.rs`, #742).

use super::*;

// ---------------------------------------------------------------------------
// Settings: `[post_process.styles.<name>]`
// ---------------------------------------------------------------------------

/// One style's settings from the config.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StyleSetting {
    pub enabled: bool,
    pub params: BTreeMap<String, f32>,
    pub options: BTreeMap<String, String>,
    /// Where the style stands in its stage's chain (#299): lower runs
    /// first. `None` is its registration index ([`chain_order`]).
    pub order: Option<f32>,
    /// The scene's `feeds_bloom` (#300), when it sets one; `None` keeps
    /// the descriptor's default.
    pub feeds_bloom: Option<bool>,
    /// How much of the style's result shows over what it read, 0 to 1
    /// (#369); `None` is all of it. A script fades a whole-frame style in
    /// and out by ramping it (the stage draws what the style read back over
    /// its result, [`run_stage`]).
    pub blend: Option<f32>,
    /// Where on the screen the style shows (#406): `reveal` (0 to 1) and
    /// the `reveal_*` keys, a ragged region the frame it read shows around.
    /// Empty is everywhere.
    pub reveal: reveal::StyleReveal,
    /// `blend_source = "tear"` (#746): the style shows only where the frame
    /// tears (`[post_process] tear`), its blend times the tear's mask at
    /// each pixel, as a prop's does with the same key (#397). With no tear
    /// it is left out of the chain. `"own"` (the default) is its blend
    /// alone.
    pub follows_tear: bool,
    /// Off everywhere (#376): not only the table but every entity whose
    /// component or `style_mask` would switch the style on for its own
    /// pixels. `--style <name>:off` sets it, so a styled scene renders
    /// unstyled with the rest of its post-processing kept; switching the
    /// style on clears it. A scene's table never sets it.
    pub off_everywhere: bool,
}

/// Keys every style's table reads besides its own parameters and options;
/// no style may name a parameter or option after one. The `reveal` keys
/// (#406) are [`reveal::KEYS`].
pub const RESERVED_KEYS: [&str; 19] = [
    "enabled",
    "order",
    "feeds_bloom",
    "blend",
    "blend_source",
    "reveal",
    "reveal_shape",
    "reveal_ink",
    "reveal_rim_color",
    "reveal_scale",
    "reveal_ragged",
    "reveal_edge",
    "reveal_angle",
    "reveal_x",
    "reveal_y",
    "reveal_boil",
    "reveal_seed",
    "reveal_rim",
    "reveal_shadow",
];

/// A table's `blend_source` (#746): `Some(true)` for `"tear"`, `Some(false)`
/// for `"own"`, `None` for anything else.
pub fn blend_source(value: &str) -> Option<bool> {
    match value {
        "tear" => Some(true),
        "own" => Some(false),
        _ => None,
    }
}

/// The generic style settings on [`PostProcessConfig`], by style name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StyleSettings {
    pub(super) styles: BTreeMap<String, StyleSetting>,
}

impl StyleSettings {
    pub fn is_enabled(&self, name: &str) -> bool {
        self.styles.get(name).is_some_and(|s| s.enabled)
    }

    /// Switch style `name`'s table on or off. On also lifts an
    /// [`off_everywhere`](Self::set_off_everywhere); off leaves the style's
    /// entities to switch it on for their own pixels.
    pub fn set_enabled(&mut self, name: &str, enabled: bool) {
        let setting = self.styles.entry(name.to_string()).or_default();
        setting.enabled = enabled;
        if enabled {
            setting.off_everywhere = false;
        }
    }

    /// Whether style `name` is off everywhere (#376), its entities'
    /// own components and `style_mask`s included.
    pub fn is_off_everywhere(&self, name: &str) -> bool {
        self.styles.get(name).is_some_and(|s| s.off_everywhere)
    }

    /// Turn style `name` off everywhere (#376): its table, and every
    /// entity that would switch it on for its own pixels. What
    /// `--style <name>:off` does; [`set_enabled`](Self::set_enabled)`(name,
    /// true)` undoes it.
    pub fn set_off_everywhere(&mut self, name: &str) {
        let setting = self.styles.entry(name.to_string()).or_default();
        setting.enabled = false;
        setting.off_everywhere = true;
    }

    pub fn param(&self, name: &str, param: &str) -> Option<f32> {
        self.styles.get(name)?.params.get(param).copied()
    }

    pub fn set_param(&mut self, name: &str, param: &str, value: f32) {
        self.styles
            .entry(name.to_string())
            .or_default()
            .params
            .insert(param.to_string(), value);
    }

    /// Set colour `base` (`tint`, #315) of style `name`: its `_r`, `_g`
    /// and `_b` channels.
    pub fn set_color(&mut self, name: &str, base: &str, rgb: [f32; 3]) {
        for (c, v) in ["r", "g", "b"].iter().zip(rgb) {
            self.set_param(name, &format!("{base}_{c}"), v);
        }
    }

    pub fn option(&self, name: &str, option: &str) -> Option<&str> {
        self.styles
            .get(name)?
            .options
            .get(option)
            .map(String::as_str)
    }

    pub fn set_option(&mut self, name: &str, option: &str, value: &str) {
        self.styles
            .entry(name.to_string())
            .or_default()
            .options
            .insert(option.to_string(), value.to_string());
    }

    /// Whether the scene says bloom reads style `name` (#300); `None` when
    /// it doesn't say.
    pub fn feeds_bloom(&self, name: &str) -> Option<bool> {
        self.styles.get(name)?.feeds_bloom
    }

    /// Say whether bloom reads style `name`; `None` goes back to the
    /// style's default. Leaves the style on or off as it was.
    pub fn set_feeds_bloom(&mut self, name: &str, feeds: Option<bool>) {
        self.styles.entry(name.to_string()).or_default().feeds_bloom = feeds;
    }

    /// How much of style `name` shows over what it read (#369): its
    /// table's `blend`, 1 when it sets none.
    pub fn blend(&self, name: &str) -> f32 {
        self.styles
            .get(name)
            .and_then(|s| s.blend)
            .map_or(1.0, |b| b.clamp(0.0, 1.0))
    }

    /// Set how much of style `name` shows, clamped to 0..1; `None` (or 1)
    /// shows all of it. Leaves the style on or off as it was.
    pub fn set_blend(&mut self, name: &str, blend: Option<f32>) {
        let blend = blend
            .filter(|b| b.is_finite())
            .map(|b| b.clamp(0.0, 1.0))
            .filter(|b| *b < 1.0);
        match self.styles.get_mut(name) {
            Some(setting) => setting.blend = blend,
            None if blend.is_some() => {
                self.styles.entry(name.to_string()).or_default().blend = blend;
            }
            None => {}
        }
    }

    /// Style `name`'s reveal (#406), when its table has one: `None` with
    /// no `reveal` key, whatever other `reveal_*` keys it sets.
    pub fn reveal(&self, name: &str) -> Option<&reveal::StyleReveal> {
        self.styles
            .get(name)
            .map(|s| &s.reveal)
            .filter(|r| r.amount.is_some())
    }

    /// Set one reveal key of style `name` (#406) from a TOML value:
    /// `reveal` (0..1), `reveal_shape`, `reveal_ink` or a `reveal_*`
    /// number. Out of range a number is clamped, or refused with
    /// `strict`. `Ok(false)`: not a reveal key. Leaves the style on or off
    /// as it was.
    pub fn set_reveal(
        &mut self,
        name: &str,
        key: &str,
        value: &toml::Value,
        strict: bool,
    ) -> Result<bool, String> {
        if !reveal::is_key(key) {
            return Ok(false);
        }
        let mut next = self
            .styles
            .get(name)
            .map(|s| s.reveal.clone())
            .unwrap_or_default();
        next.set(key, value, strict)?;
        self.styles.entry(name.to_string()).or_default().reveal = next;
        Ok(true)
    }

    /// Make style `name`'s reveal exactly `reveal` (F4's Reveal group,
    /// #875), every key at once; an empty one is [`clear_reveal`](Self::clear_reveal).
    /// Leaves the style on or off as it was.
    pub fn replace_reveal(&mut self, name: &str, reveal: reveal::StyleReveal) {
        if reveal.is_empty() {
            self.clear_reveal(name);
        } else {
            self.styles.entry(name.to_string()).or_default().reveal = reveal;
        }
    }

    /// Take style `name`'s reveal off (#406): it shows everywhere again.
    pub fn clear_reveal(&mut self, name: &str) {
        if let Some(setting) = self.styles.get_mut(name) {
            setting.reveal = reveal::StyleReveal::default();
        }
    }

    /// Whether any of style `name` reaches the frame: a blend above 0
    /// (#399) and, when it has a reveal (#406), a reveal above 0. A style
    /// that doesn't is left out of its chain, so the frame is the one
    /// without it, byte for byte. One that follows the tear (#746) needs
    /// the frame torn too: [`shows_in`](Self::shows_in).
    pub fn shows(&self, name: &str) -> bool {
        self.blend(name) > 0.0
            && self
                .reveal(name)
                .and_then(|r| r.amount)
                .is_none_or(|a| a > 0.0)
    }

    /// [`shows`](Self::shows), and, for a style that follows the tear
    /// (#746), a frame that tears (`postprocess::tear_active`).
    pub fn shows_in(&self, name: &str, config: &PostProcessConfig) -> bool {
        self.shows(name) && (!self.follows_tear(name) || crate::postprocess::tear_active(config))
    }

    /// Whether style `name` shows only where the frame tears
    /// (`blend_source = "tear"`, #746).
    pub fn follows_tear(&self, name: &str) -> bool {
        self.styles.get(name).is_some_and(|s| s.follows_tear)
    }

    /// Say whether style `name` follows the tear (#746). Leaves the style
    /// on or off as it was.
    pub fn set_follows_tear(&mut self, name: &str, follows: bool) {
        match self.styles.get_mut(name) {
            Some(setting) => setting.follows_tear = follows,
            None if follows => {
                self.styles
                    .entry(name.to_string())
                    .or_default()
                    .follows_tear = true;
            }
            None => {}
        }
    }

    /// Style `name`'s settings, if it has any.
    pub fn get(&self, name: &str) -> Option<&StyleSetting> {
        self.styles.get(name)
    }

    /// Style `name`'s authored place in its stage, if it has one (#299).
    pub fn order(&self, name: &str) -> Option<f32> {
        self.styles.get(name)?.order
    }

    /// Give style `name` a place in its stage, or (`None`) put it back at
    /// its registration index.
    pub fn set_order(&mut self, name: &str, order: Option<f32>) {
        match self.styles.get_mut(name) {
            Some(setting) => setting.order = order,
            None if order.is_some() => {
                self.styles.entry(name.to_string()).or_default().order = order;
            }
            None => {}
        }
    }

    /// Put style `name`'s parameters, options and place in the chain back
    /// to their defaults, leaving it on or off as it was.
    pub fn reset(&mut self, name: &str) {
        if let Some(setting) = self.styles.get_mut(name) {
            setting.params.clear();
            setting.options.clear();
            setting.order = None;
            setting.feeds_bloom = None;
            setting.blend = None;
            setting.reveal = reveal::StyleReveal::default();
            setting.follows_tear = false;
        }
    }

    /// Make style `name` exactly what it is in `from` (the scene's
    /// authored settings): on, off and every value; no settings in `from`
    /// means none here.
    pub fn restore(&mut self, name: &str, from: &StyleSettings) {
        match from.styles.get(name) {
            Some(setting) => {
                self.styles.insert(name.to_string(), setting.clone());
            }
            None => {
                self.styles.remove(name);
            }
        }
    }

    /// Make style `name`'s settings exactly `setting`, or (`None`) give it
    /// none: a look's cross-fade (#496) lays each in-between setting so.
    pub fn put(&mut self, name: &str, setting: Option<StyleSetting>) {
        match setting {
            Some(setting) => {
                self.styles.insert(name.to_string(), setting);
            }
            None => {
                self.styles.remove(name);
            }
        }
    }

    /// Whether style `name` is set the same here and in `other`. A style
    /// with no settings is the same as one that is off with none.
    pub fn same_style(&self, name: &str, other: &StyleSettings) -> bool {
        let empty = StyleSetting::default();
        self.styles.get(name).unwrap_or(&empty) == other.styles.get(name).unwrap_or(&empty)
    }

    /// The styles that have settings, by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &StyleSetting)> {
        self.styles.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Read a scene's `[post_process.styles]` tables. A table switches its
    /// style on unless it says `enabled = false`; an option is a string, a
    /// parameter a number, a colour (#315) an array of three numbers or a
    /// `#rrggbb` string (its channels may still be set one by one), and
    /// `feeds_bloom` (#300) true or false. Returns the settings and a
    /// warning for each name, key or value the registry doesn't know.
    pub fn from_tables(
        tables: &BTreeMap<String, toml::Table>,
        styles: &StyleRegistry,
    ) -> (Self, Vec<String>) {
        let mut settings = Self::default();
        let mut warnings = Vec::new();
        for (name, table) in tables {
            let Some(descriptor) = styles.find(name) else {
                warnings.push(format!(
                    "unknown style '{name}' in [post_process.styles]; registered: {}",
                    styles.names().join(", ")
                ));
                continue;
            };
            let mut setting = StyleSetting {
                enabled: true,
                ..Default::default()
            };
            for (key, value) in table {
                if key == "enabled" {
                    match value.as_bool() {
                        Some(on) => setting.enabled = on,
                        None => {
                            warnings.push(format!("styles.{name}.enabled must be true or false"))
                        }
                    }
                    continue;
                }
                if key == "order" {
                    let number = flint_core::toml_util::toml_f64(value).filter(|n| n.is_finite());
                    match number {
                        Some(n) => setting.order = Some(n as f32),
                        None => warnings.push(format!("styles.{name}.order must be a number")),
                    }
                    continue;
                }
                if key == "blend" {
                    let number = flint_core::toml_util::toml_f64(value).filter(|n| n.is_finite());
                    match number {
                        Some(n) => {
                            let b = (n as f32).clamp(0.0, 1.0);
                            setting.blend = (b < 1.0).then_some(b);
                        }
                        None => warnings
                            .push(format!("styles.{name}.blend must be a number from 0 to 1")),
                    }
                    continue;
                }
                if key == "blend_source" {
                    // Where its blend comes from (#746)
                    match value.as_str().and_then(blend_source) {
                        Some(tear) => setting.follows_tear = tear,
                        None => warnings.push(format!(
                            "styles.{name}.blend_source must be \"own\" or \"tear\""
                        )),
                    }
                    continue;
                }
                if reveal::is_key(key) {
                    // Where the style shows (#406)
                    if let Err(e) = setting.reveal.set(key, value, false) {
                        warnings.push(format!("styles.{name}.{e}"));
                    }
                    continue;
                }
                if key == "feeds_bloom" {
                    match value.as_bool() {
                        Some(_) if descriptor.stage != StyleStage::Hdr => warnings.push(format!(
                            "styles.{name}.feeds_bloom: {name} is a display style, after bloom; \
                             only an hdr style can feed bloom"
                        )),
                        Some(feeds) => setting.feeds_bloom = Some(feeds),
                        None => warnings
                            .push(format!("styles.{name}.feeds_bloom must be true or false")),
                    }
                    continue;
                }
                if let Some(option) = descriptor.options.iter().find(|o| o.name == key) {
                    match value.as_str() {
                        // Kept all the same: the style falls back to its
                        // default, and F4 shows the stray value as it is
                        Some(text) => {
                            if option.refuses(text) {
                                warnings.push(format!(
                                    "styles.{name}.{key}: unknown value '{text}'; \
                                     it takes: {} (drawing as '{}')",
                                    option.choices.join(", "),
                                    option.default
                                ));
                            }
                            setting.options.insert(key.clone(), text.to_string());
                        }
                        None => warnings.push(format!("styles.{name}.{key} must be a string")),
                    }
                    continue;
                }
                if let Some(color) = descriptor.color_setting(key) {
                    match color_from_toml(value) {
                        Some(rgb) => {
                            for (p, v) in color.channels.iter().zip(rgb) {
                                if let Some(e) = p.range_error(v) {
                                    warnings.push(format!(
                                        "styles.{name}.{key}: {e}; kept as written, \
                                         past what the style is tuned for"
                                    ));
                                }
                                setting.params.insert(p.name.to_string(), v);
                            }
                        }
                        None => warnings.push(format!(
                            "styles.{name}.{key} must be a colour: [r, g, b] (0-1) or \"#rrggbb\""
                        )),
                    }
                    continue;
                }
                let Some(param) = descriptor.params.iter().find(|p| p.name == key) else {
                    let known = descriptor.setting_keys();
                    warnings.push(format!(
                        "style '{name}' has no parameter '{key}'; it takes: {}",
                        if known.is_empty() {
                            "none".to_string()
                        } else {
                            known.join(", ")
                        }
                    ));
                    continue;
                };
                let number = flint_core::toml_util::toml_f64(value);
                match number {
                    Some(n) => {
                        // Kept as written, as a stray option is: F4 shows
                        // it, and the style decides what to draw
                        if let Some(e) = param.range_error(n as f32) {
                            warnings.push(format!(
                                "styles.{name}.{e}; kept as written, past what the style is tuned for"
                            ));
                        }
                        setting.params.insert(key.clone(), n as f32);
                    }
                    None => warnings.push(format!("styles.{name}.{key} must be a number")),
                }
            }
            settings.styles.insert(name.clone(), setting);
        }
        (settings, warnings)
    }
}
