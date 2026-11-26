//! Registration points on the command line (ADR 0137 section 4, #1557): a
//! command of the kernel's (or of one package) that another package fills
//! in. The procgen package is what makes them real. The kernel's `flint
//! gen` runs a glyph set itself and hands a spec to [`SpecGen`], and
//! `flint check` judges a spec through [`SpecChecks`]. Procgen's own `gen`
//! asks for the ai package's steps and living's strip the same way, by
//! the type its crate declares.
//!
//! The points are one typed map for the run ([`Points`], one value per
//! type, as `Resources` is for the player), filled once before any command
//! runs: each [`CliExtension`](super::CliExtension)'s
//! [`register`](super::CliExtension::register), then the binary's own. A
//! missing point means the package isn't in this build, and the asker
//! says which package to list.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// One value per type: what each package registered on the command line.
#[derive(Default)]
pub struct Points {
    map: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl Points {
    pub fn new() -> Self {
        Self::default()
    }

    /// Put `value` in; a second of a type replaces the first.
    pub fn insert<T: Any + Send + Sync>(&mut self, value: T) -> &mut Self {
        self.map.insert(TypeId::of::<T>(), Box::new(value));
        self
    }

    pub fn get<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|value| value.downcast_ref::<T>())
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl std::fmt::Debug for Points {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Points")
            .field("len", &self.map.len())
            .finish()
    }
}

static RUN: OnceLock<Points> = OnceLock::new();

/// The run's points, made by `init` the first time they're asked for: the
/// launcher (flint-cli's `run_with`) asks before any command runs, so a
/// package's command finds them as the launcher made them.
pub fn points_or(init: impl FnOnce() -> Points) -> &'static Points {
    RUN.get_or_init(init)
}

/// The run's points, or none when no launcher made them.
pub fn points() -> &'static Points {
    RUN.get_or_init(Points::default)
}

/// `flint gen <spec>` (#1557): the kernel's command, which runs a glyph set
/// itself and a spec through the package that registered this.
pub struct SpecGen(pub fn(super::gen::GenArgs) -> anyhow::Result<()>);

/// What `flint check` asks of a procgen spec a scene names (#718, #1557).
pub struct SpecChecks {
    /// Why the spec at the path doesn't parse; `None` when it does.
    pub parse_error: fn(&Path) -> Option<String>,
    /// The spec's params against its generator's schema: its generator's
    /// name and each issue. `None` when the spec doesn't parse or its
    /// generator is no built-in (a game's own).
    pub param_issues: fn(&Path) -> Option<SpecParams>,
    /// A `.terrain.toml`'s issues as the terrain editor's status bar shows
    /// them (`error: ..` / `warning: ..`, `flint gen`'s own), from its text:
    /// the terrain_v1 schema's.
    pub terrain_issues: fn(&str) -> Vec<String>,
}

/// A spec's generator's name and its params' problems.
pub type SpecParams = (String, Vec<SpecIssue>);

/// What `flint check` asks of the scene tables a package reads itself
/// (#1558: living's `[living]` and `[wooze]`, which the scene file keeps raw
/// and the package's types read). Without it the scene pass's schema half
/// is all that is said of them, as a player without the package reads
/// neither.
pub struct TableChecks {
    /// Each of the package's tables that doesn't fit its type, with the
    /// reader's words: the scene doesn't load until it does.
    pub refused: fn(&flint_scene::SceneFile) -> Vec<(&'static str, String)>,
    /// The tables held to what reads them, in groups, in order: each with
    /// the table whose scene-pass errors it says again in its own words,
    /// which go when the group has anything to say.
    pub check: fn(&TableCheck) -> Vec<(Option<&'static str>, Vec<flint_core::Diagnostic>)>,
}

/// One scene, as [`TableChecks::check`] reads it.
pub struct TableCheck<'a> {
    pub scene: &'a Path,
    pub file: &'a flint_scene::SceneFile,
    pub registry: &'a flint_schema::SchemaRegistry,
    /// The loaded world, when it loaded: which specs the props grow from.
    pub world: Option<&'a flint_ecs::FlintWorld>,
}

/// `flint render`'s point for the package that clocks a scene
/// ([`super::render::StagePart`]).
pub struct Stages(pub &'static dyn super::render::StagePart);

/// One problem with a spec's params.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecIssue {
    /// A value the generator can't use, not a key it ignores.
    pub error: bool,
    /// The parameter's path: `ops[2].columns`.
    pub key: String,
    pub message: String,
}
