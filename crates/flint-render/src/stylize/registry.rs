//! The style registry as a value (#742): the built-ins, the styles a game
//! registers in Rust (#419) and the user styles a scene can see (#274),
//! owned by whoever draws with it (a `SceneRenderer`, the player, a
//! command) rather than by the process.
//!
//! Until #742 the registry was one leaked snapshot for the process: two
//! scenes in one process (a transition, `flint goldens`, an A/B, tests)
//! shared one namespace, so the second scene's `glow` replaced the
//! first's; every user style install leaked its descriptor and slices; and
//! a hot reload leaked another. Now:
//!
//! - A [`StyleRegistry`] is a value. Two of them can hold two user styles
//!   of one name from two folders, and drawing with one never touches the
//!   other. Cloning one is cheap (the user styles are shared `Arc`s).
//! - A user style's descriptor lives in an `Arc` with the rest of what was
//!   loaded for it ([`Style`]); a reload replaces the `Arc` in the
//!   registry, and the old one is freed when the last pass built from it
//!   is rebuilt.
//! - A style's identity is its name and [`generation`](StyleDescriptor::generation)
//!   (0 for a built-in or game style, a new number for each user install),
//!   never a descriptor's address.
//! - Indices ([`StyleRegistry::index_of`], the owner table the GPU reads)
//!   are positions in the registry the frame is drawn with, so they depend
//!   on what that registry holds, not on what else the process loaded.
//!
//! What is still the process's: the built-in list (a `static` slice of the
//! style modules' `static STYLE`s, compiled in), the counter that numbers
//! installs, and [`intern`]'s pool of the user styles' names and labels,
//! keyed by content, so a reload that changes none of them adds nothing.

use super::{custom, ownership, StyleDescriptor, StyleStage};
use crate::postprocess::PostProcessConfig;
use crate::style_mask::StyleIds;
use std::sync::Arc;

/// A slice a descriptor holds (its parameters, options or an option's
/// choices): a built-in's `'static` one, or a user style's own, freed with
/// it. Reads as the slice it holds.
pub enum List<T: 'static> {
    Static(&'static [T]),
    Shared(Arc<[T]>),
}

impl<T> List<T> {
    /// The empty list, for `const` descriptors.
    pub const EMPTY: List<T> = List::Static(&[]);
}

impl<T> std::ops::Deref for List<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        match self {
            List::Static(s) => s,
            List::Shared(s) => s,
        }
    }
}

impl<T> Clone for List<T> {
    fn clone(&self) -> Self {
        match self {
            List::Static(s) => List::Static(s),
            List::Shared(s) => List::Shared(s.clone()),
        }
    }
}

impl<'a, T> IntoIterator for &'a List<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for List<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<T> From<Vec<T>> for List<T> {
    fn from(v: Vec<T>) -> Self {
        List::Shared(v.into())
    }
}

/// `text` as a `&'static str`: the same pointer for the same text, made
/// once (#742). A user style's name, label, parameter and option names,
/// help lines, choices and pass labels are read as `&'static str` wherever
/// a built-in's are (bind cache keys, GPU timer labels, F4), so they come
/// from here. The pool only grows with text it hasn't seen: loading or
/// reloading a style whose names didn't change adds nothing, and
/// descriptors themselves are never kept here.
pub fn intern(text: &str) -> &'static str {
    static POOL: std::sync::Mutex<std::collections::BTreeSet<&'static str>> =
        std::sync::Mutex::new(std::collections::BTreeSet::new());
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = pool.get(text) {
        return found;
    }
    let made: &'static str = Box::leak(text.to_owned().into_boxed_str());
    pool.insert(made);
    made
}

/// `items` as a `&'static` list: the same slice for the same strings, made
/// once (#742), as [`intern`] does for one. A user style's option choices.
pub fn intern_list(items: &[&str]) -> &'static [&'static str] {
    // A few lists a style: a walk is enough
    static POOL: std::sync::Mutex<Vec<&'static [&'static str]>> = std::sync::Mutex::new(Vec::new());
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = pool.iter().find(|p| **p == items) {
        return found;
    }
    let made: Vec<&'static str> = items.iter().map(|s| intern(s)).collect();
    let made: &'static [&'static str] = Box::leak(made.into_boxed_slice());
    pool.push(made);
    made
}

/// One registered style: a built-in's or a game's `static` descriptor, or
/// a user style's, held with what was loaded for it. Reads as its
/// [`StyleDescriptor`].
#[derive(Clone)]
pub struct Style(Kind);

#[derive(Clone)]
enum Kind {
    Native(&'static StyleDescriptor),
    User(Arc<custom::Loaded>),
}

impl Style {
    /// A built-in's or a game's descriptor.
    pub fn native(descriptor: &'static StyleDescriptor) -> Self {
        Style(Kind::Native(descriptor))
    }

    pub(crate) fn user(loaded: Arc<custom::Loaded>) -> Self {
        Style(Kind::User(loaded))
    }

    /// The descriptor.
    pub fn descriptor(&self) -> &StyleDescriptor {
        match &self.0 {
            Kind::Native(d) => d,
            Kind::User(l) => &l.descriptor,
        }
    }

    /// What was loaded for a user style (its description, errors and bind
    /// names); `None` for a native one.
    pub(crate) fn loaded(&self) -> Option<&Arc<custom::Loaded>> {
        match &self.0 {
            Kind::Native(_) => None,
            Kind::User(l) => Some(l),
        }
    }

    /// Whether it is a user `.shader.toml` style.
    pub fn is_user(&self) -> bool {
        matches!(self.0, Kind::User(_))
    }

    /// Whether `self` and `other` are the same install of the same style:
    /// one name and one [`generation`](StyleDescriptor::generation).
    pub fn same(&self, other: &Style) -> bool {
        self.name == other.name && self.generation == other.generation
    }
}

impl std::ops::Deref for Style {
    type Target = StyleDescriptor;
    fn deref(&self) -> &StyleDescriptor {
        self.descriptor()
    }
}

impl std::fmt::Debug for Style {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Style({}#{})", self.name, self.generation)
    }
}

/// The styles a frame can draw, in chain order: the built-ins, then each
/// game style in the order it was registered and each user style in the
/// order it was loaded (#742, module docs).
///
/// [`StyleRegistry::new`] holds the built-ins alone.
/// [`register`](Self::register) adds a game's Rust style;
/// [`load_for`](Self::load_for) a scene's user styles and
/// [`for_scene`](Self::for_scene) a copy with another scene's in their
/// place; [`reload_changed_for`](Self::reload_changed_for) their hot
/// reload. A host that draws hands it to its renderer
/// (`SceneBinding`, `SceneRenderer::set_styles`) and parses the scene's
/// `[post_process]` against the same one.
#[derive(Clone)]
pub struct StyleRegistry {
    pub(crate) styles: Vec<Style>,
    /// The names of the game's styles ([`register`](Self::register)), in
    /// the order registered.
    game: Vec<&'static str>,
    /// The `.shader.toml`s that didn't load, by path (#425), until they do
    /// or are deleted.
    pub(crate) failed: std::collections::BTreeMap<std::path::PathBuf, custom::Failure>,
    /// The style folders the reload poll last searched, and for which
    /// scope (#1121): worked out once a scope, since working them out asks
    /// the working directory and the disk (`Project::search_dirs`).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) poll_dirs: Option<(flint_core::project::AssetScope, Vec<std::path::PathBuf>)>,
}

impl Default for StyleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for StyleRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.styles.iter()).finish()
    }
}

impl StyleRegistry {
    /// The styles compiled into the engine, nothing else.
    pub fn new() -> Self {
        Self {
            styles: super::builtins().iter().map(|d| Style::native(d)).collect(),
            game: Vec::new(),
            failed: Default::default(),
            poll_dirs: None,
        }
    }

    /// A copy holding only the native styles (built-ins and the game's):
    /// what [`for_scene`](Self::for_scene) loads a scene's user styles
    /// into.
    pub fn natives(&self) -> Self {
        Self {
            styles: self
                .styles
                .iter()
                .filter(|s| !s.is_user())
                .cloned()
                .collect(),
            game: self.game.clone(),
            failed: Default::default(),
            poll_dirs: None,
        }
    }

    /// Add a style written in Rust outside the engine: a game binary's own
    /// (#419). From then on it is a style like a built-in for whoever
    /// draws with this registry: F4 lists it, `--style <name>` and
    /// `[post_process.styles.<name>]` switch it on, `--list-styles` names
    /// it with `"source": "game"`.
    ///
    /// Build the descriptor as the engine's styles are, a `static` made with
    /// [`StyleDescriptor::new`] and its setters (they are `const`, so a field
    /// the engine adds later gets its default there and the game's code still
    /// builds, #366):
    ///
    /// ```ignore
    /// static CYANOTYPE: StyleDescriptor =
    ///     StyleDescriptor::new("cyanotype", "Cyanotype", StyleStage::Display, create)
    ///         .params(&[StyleParam::new("exposure", 1.0, 0.25..=3.0)]);
    ///
    /// styles.register(&CYANOTYPE)?;
    /// ```
    ///
    /// Register before the scene's tables are read against the registry: a
    /// table naming a style it doesn't know is dropped with a warning. A
    /// game that embeds `PlayerApp` returns the descriptor from
    /// `PlayerExtension::styles` instead, and `add_extension` registers it
    /// in the player's registry.
    ///
    /// The name must be lowercase letters, digits and `_`, and new: a
    /// built-in, a user style already loaded or another game style of that
    /// name is refused, as is a per-entity `component` another style
    /// already takes. Registering the same descriptor again is accepted and
    /// changes nothing. A user `.shader.toml` style can't take a game
    /// style's name afterwards, and a hot reload never replaces one.
    pub fn register(&mut self, descriptor: &'static StyleDescriptor) -> Result<(), String> {
        let name = descriptor.name;
        if !custom::is_identifier(name) {
            return Err(format!(
                "style name \"{name}\" must start with a lowercase letter and hold only \
                 lowercase letters, digits and _"
            ));
        }
        if let Some(existing) = self.find(name) {
            if std::ptr::eq(existing, descriptor) {
                return Ok(());
            }
            let what = if super::is_builtin(name) {
                "a built-in style"
            } else if self.is_game_style(name) {
                "a style the game has already registered"
            } else {
                "a user style already loaded (styles/*.shader.toml)"
            };
            return Err(format!(
                "style \"{name}\" is {what}; pick another name. Registered: {}",
                self.names().join(", ")
            ));
        }
        if let Some(component) = descriptor.component {
            if let Some(other) = self.iter().find(|d| d.component == Some(component)) {
                return Err(format!(
                    "style \"{name}\": component \"{component}\" is already style \"{}\"'s",
                    other.name
                ));
            }
        }
        // Before the user styles, so a game style keeps its place in the
        // chain whatever a scene loads
        let at = self
            .styles
            .iter()
            .position(Style::is_user)
            .unwrap_or(self.styles.len());
        self.styles.insert(at, Style::native(descriptor));
        self.game.push(name);
        Ok(())
    }

    /// Whether `name` is a style the game registered in Rust
    /// ([`register`](Self::register)).
    pub fn is_game_style(&self, name: &str) -> bool {
        self.game.contains(&name)
    }

    /// Whether `name` is written in Rust: a built-in or a game's style, not
    /// a user `.shader.toml` one.
    pub fn is_native(&self, name: &str) -> bool {
        super::is_builtin(name) || self.is_game_style(name)
    }

    /// Whether `name` is a loaded user style (a `.shader.toml`'s), not a
    /// built-in or a game's.
    pub fn is_user_style(&self, name: &str) -> bool {
        self.style(name).is_some_and(Style::is_user)
    }

    /// Every registered style, in chain order.
    pub fn styles(&self) -> &[Style] {
        &self.styles
    }

    /// Every registered style's descriptor, in chain order.
    pub fn iter(&self) -> impl Iterator<Item = &StyleDescriptor> + '_ {
        self.styles.iter().map(Style::descriptor)
    }

    pub fn len(&self) -> usize {
        self.styles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.styles.is_empty()
    }

    /// The style called `name`.
    pub fn style(&self, name: &str) -> Option<&Style> {
        self.styles.iter().find(|s| s.name == name)
    }

    /// The descriptor of the style called `name`.
    pub fn find(&self, name: &str) -> Option<&StyleDescriptor> {
        self.style(name).map(Style::descriptor)
    }

    /// Where style `name` stands in the registry.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.styles.iter().position(|s| s.name == name)
    }

    /// The registered names, for messages.
    pub fn names(&self) -> Vec<&'static str> {
        self.styles.iter().map(|s| s.name).collect()
    }

    /// Whether any style on this frame reads the style albedo (#869), so
    /// the renderer draws it after the scene pass.
    pub fn any_uses_albedo(&self, config: &PostProcessConfig, ids: &StyleIds) -> bool {
        config.enabled
            && self
                .iter()
                .any(|d| d.is_on(config, ids) && d.uses_albedo.now(config, ids))
    }

    /// Whether any style on this frame reads the entity style mask: one
    /// asks for it, or one that [`yields`](StyleDescriptor::yields) is on
    /// while another running style owns an entity's pixels (#384).
    pub fn any_uses_mask(&self, config: &PostProcessConfig, ids: &StyleIds) -> bool {
        config.enabled
            && self.iter().any(|d| {
                d.reads_mask(config, ids)
                    || (d.yields
                        && d.is_on(config, ids)
                        && !self.foreign_ids(d, config, ids).is_empty())
            })
    }

    /// Which style owns each mask id this frame (#384), by index in this
    /// registry, [`ownership::NONE`] for none: the style its variant names,
    /// when that style is registered and on and the variant isn't an
    /// opt-out. A faded id (#400: its entities all at blend 0) keeps its
    /// style as owner though the style is off for it, so a whole-frame
    /// style leaves the prop as it came, as it did while the faded style
    /// still ran; a style off everywhere (#376) owns nothing, so its props
    /// are plain scene to every other style. Id 0 (unstyled) is always
    /// none; the table has a slot for every id (#740: it held 256, and ids
    /// past them were silently the whole frame's).
    pub fn owner_table(&self, config: &PostProcessConfig, ids: &StyleIds) -> Vec<u32> {
        let mut table = vec![ownership::NONE; ids.len() + 1];
        for (id, _) in ids.iter() {
            let slot = &mut table[id as usize];
            let Some(v) = ids.variant(id) else {
                continue;
            };
            if v.off {
                continue;
            }
            if let Some(i) = self.iter().position(|d| {
                d.name == v.style
                    && !config.styles.is_off_everywhere(d.name)
                    && (ids.is_faded(id) || d.is_on(config, ids))
            }) {
                *slot = i as u32;
            }
        }
        table
    }

    /// The mask ids another running style owns this frame (#384), in id
    /// order: ids whose variant names a registered style other than `d`
    /// that is on ([`StyleDescriptor::is_on`]), opt-outs aside. A style
    /// that [`yields`](StyleDescriptor::yields) leaves their pixels as its
    /// input had them.
    pub fn foreign_ids(
        &self,
        d: &StyleDescriptor,
        config: &PostProcessConfig,
        ids: &StyleIds,
    ) -> Vec<u16> {
        let table = self.owner_table(config, ids);
        let me = self.index_of(d.name).map(|i| i as u32);
        (1..table.len())
            .filter(|&id| table[id] != ownership::NONE && Some(table[id]) != me)
            .map(|id| id as u16)
            .collect()
    }

    /// Every registered style of `stage`, in the order its chain would run
    /// them under `config` (on or not), with each one's
    /// [`chain_order`](super::chain_order). What the F4 menu lists;
    /// [`StyleSet::active`](super::StyleSet::active) keeps the ones that
    /// are on.
    pub fn stage_chain(
        &self,
        stage: StyleStage,
        config: &PostProcessConfig,
    ) -> Vec<(&StyleDescriptor, f32)> {
        let mut chain: Vec<(&StyleDescriptor, f32)> = self
            .iter()
            .enumerate()
            .filter(|(_, d)| d.stage == stage)
            .map(|(i, d)| (d, super::chain_order(i, d, config)))
            .collect();
        chain.sort_by(|a, b| a.1.total_cmp(&b.1));
        chain
    }
}
