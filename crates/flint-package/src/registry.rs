//! Registration (ADR 0137 decision 4, #1552): a launcher lists its
//! packages, and each one's [`Package::register`] tells the kernel what it
//! brings, once a run, before any scene loads. The kernel then finds a
//! scene's subsystems, the game-state policy names, the script modules and
//! the debug keys here, never by a package's name.

use crate::assets::AssetMaker;
use crate::{Resources, SceneCtx, Subsystem};
use flint_core::package::InUse;
use flint_runtime::DebugKeyGroup;
use flint_script::ScriptModule;
use std::any::{Any, TypeId};
use std::collections::HashMap;

/// What a launcher lists: one value per package, named as its
/// `package.toml` is. A package's crate exports one as a `static`.
pub trait Package: Sync {
    /// The package's name, as its manifest's `[package] name`.
    fn name(&self) -> &'static str;

    /// Tell the kernel what this package brings. Called once a run, in the
    /// launcher's order (a package after the ones it `[requires]`).
    fn register(&self, registry: &mut Registry);
}

/// A scene subsystem's builder: its subsystem, if the scene uses it.
pub type BuildForScene = fn(&mut SceneCtx) -> Option<Box<dyn Subsystem>>;

/// What a scene that uses a package hears when the project's `[packages]`
/// leaves the package out: one line, or `None` when the scene doesn't use
/// it. The kernel asks it in place of the package's builders.
pub type LeftOutLine = fn(&SceneCtx) -> Option<String>;

/// What a package puts in every run's [`Resources`] (its resolver, its
/// props), called for each run's core as it is made.
pub type RunResources = fn(&mut Resources);

/// A package's settle of a scene file its tables are in (ADR 0137 section 4,
/// #1558): check them and fill in what they name (living's `[wooze] preset`,
/// which can set `[post_process]`'s boil too), right after the file loads
/// and before anything reads it. An error refuses the load; the scene's
/// path finds a file a table names.
pub type SettleScene = fn(&mut flint_scene::SceneFile, &std::path::Path) -> flint_core::Result<()>;

/// Everything the listed packages registered, in the order they did.
///
/// A builder belongs to the package whose `register` added it, so the
/// kernel can skip a package the project leaves out without the package
/// asking (ADR 0137 decision 8).
#[derive(Default)]
pub struct Registry {
    built: Vec<&'static str>,
    current: &'static str,
    builders: Vec<(&'static str, BuildForScene)>,
    left_out: Vec<(&'static str, LeftOutLine)>,
    policies: Vec<&'static str>,
    modules: Vec<(&'static str, &'static dyn ScriptModule)>,
    debug_keys: Vec<(&'static str, DebugKeyGroup)>,
    run_resources: Vec<(&'static str, RunResources)>,
    settles: Vec<(&'static str, SettleScene)>,
    makers: Vec<(&'static str, &'static dyn AssetMaker)>,
    /// Registration points, one value per Rust type (ADR 0137 section 4).
    points: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

/// The line a scene that uses `package` hears when the project's
/// `[packages]` leaves it out (#1528): what it uses, what that means, and
/// the line to add. `None` when the scene uses none of it. One wording for
/// the kernel and every package, as `flint check` calls each part
/// `unlisted-package`.
pub fn left_out_line(in_use: &InUse, package: &str, uses: &[&str], and_so: &str) -> Option<String> {
    (!uses.is_empty()).then(|| {
        format!(
            "[{package}] [packages] in flint.toml leaves out `{package}`: the scene's {} {and_so} \
             ({})",
            uses.join(", "),
            in_use.hint_for(package)
        )
    })
}

impl Registry {
    /// Each package registered in turn. A name listed twice registers once,
    /// the first time.
    pub fn of(packages: &[&dyn Package]) -> Self {
        let mut registry = Self::default();
        for package in packages {
            registry.add(*package);
        }
        registry
    }

    /// `package` registered after those already here; `false` when a
    /// package of its name already is.
    pub fn add(&mut self, package: &dyn Package) -> bool {
        let name = package.name();
        if self.has(name) {
            tracing::debug!("package {name} is listed twice; registered once");
            return false;
        }
        self.built.push(name);
        let before = std::mem::replace(&mut self.current, name);
        package.register(self);
        self.current = before;
        true
    }

    // ── What a package registers ──────────────────────────────────────

    /// A scene subsystem's builder: asked at each scene's entry, after
    /// those registered before it.
    pub fn scene_subsystem(&mut self, build: BuildForScene) -> &mut Self {
        self.builders.push((self.current, build));
        self
    }

    /// The line a scene that uses this package hears when the project
    /// leaves the package out (#1528).
    pub fn when_left_out(&mut self, line: LeftOutLine) -> &mut Self {
        self.left_out.push((self.current, line));
        self
    }

    /// A name a game state's policy may pause (#727): `living: "pause"`.
    pub fn policy(&mut self, name: &'static str) -> &mut Self {
        if !self.policies.contains(&name) {
            self.policies.push(name);
        }
        self
    }

    /// Script modules, declared for every scene and enabled where a
    /// subsystem that owns them runs (#698).
    pub fn script_modules(&mut self, modules: &[&'static dyn ScriptModule]) -> &mut Self {
        for module in modules {
            self.modules.push((self.current, *module));
        }
        self
    }

    /// A group of debug keys (#717), live while a subsystem that declares
    /// them is in the scene.
    pub fn debug_keys(&mut self, group: DebugKeyGroup) -> &mut Self {
        self.debug_keys.push((self.current, group));
        self
    }

    /// What this package puts in each run's resources ([`Resources`]):
    /// called for every core the kernel makes, before any scene.
    pub fn run_resources(&mut self, put: RunResources) -> &mut Self {
        self.run_resources.push((self.current, put));
        self
    }

    /// A settle of the scene files this package has tables in: every host
    /// that loads a scene runs it ([`Registry::settle`]), the player, a
    /// transition, `flint render` and the viewer alike.
    pub fn settle_scene(&mut self, settle: SettleScene) -> &mut Self {
        self.settles.push((self.current, settle));
        self
    }

    /// A maker of assets by name (`crate::assets`), asked at each scene's
    /// load, for spawned entities and each frame, after those before it.
    pub fn asset_maker(&mut self, maker: &'static dyn AssetMaker) -> &mut Self {
        self.makers.push((self.current, maker));
        self
    }

    /// A registration point of this package: a value of its own type that
    /// a later package fills at its `register` ([`Registry::point_mut`]),
    /// and this package's builders read ([`Registry::point`]). Music's
    /// chart judge is one, which rhythm fills, and procgen's load steps,
    /// which living fills (ADR 0137 section 4). A second insert of a type
    /// replaces the first.
    pub fn insert_point<T: Any + Send + Sync>(&mut self, value: T) -> &mut Self {
        self.points.insert(TypeId::of::<T>(), Box::new(value));
        self
    }

    /// A registration point an earlier package inserted, to fill; `None`
    /// when that package isn't in the run.
    pub fn point_mut<T: Any + Send + Sync>(&mut self) -> Option<&mut T> {
        self.points
            .get_mut(&TypeId::of::<T>())
            .and_then(|value| value.downcast_mut::<T>())
    }

    /// A registration point, as the packages after its owner left it.
    pub fn point<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.points
            .get(&TypeId::of::<T>())
            .and_then(|value| value.downcast_ref::<T>())
    }

    // ── What the kernel reads ─────────────────────────────────────────

    /// The packages whose `register` ran, in order: what this binary was
    /// built with.
    pub fn built_with(&self) -> &[&'static str] {
        &self.built
    }

    /// Whether `package` registered.
    pub fn has(&self, package: &str) -> bool {
        self.built.contains(&package)
    }

    /// Every scene subsystem's builder, with the package it belongs to.
    pub fn builders(&self) -> impl Iterator<Item = (&'static str, BuildForScene)> + '_ {
        self.builders.iter().copied()
    }

    /// The left-out lines of `package`.
    pub fn left_out_lines(&self, package: &str) -> impl Iterator<Item = LeftOutLine> + '_ {
        let package = package.to_string();
        self.left_out
            .iter()
            .filter(move |(p, _)| *p == package)
            .map(|(_, line)| *line)
    }

    /// The names a game state's policy may give.
    pub fn policies(&self) -> &[&'static str] {
        &self.policies
    }

    /// Every registered script module, in order.
    pub fn modules(&self) -> impl Iterator<Item = &'static dyn ScriptModule> + '_ {
        self.modules.iter().map(|(_, m)| *m)
    }

    /// Every registered debug-key group, in order.
    pub fn debug_key_groups(&self) -> impl Iterator<Item = DebugKeyGroup> + '_ {
        self.debug_keys.iter().map(|(_, g)| *g)
    }

    /// Put every package's run resources in `resources`, in order.
    pub fn make_resources(&self, resources: &mut Resources) {
        for (_, put) in &self.run_resources {
            put(resources);
        }
    }

    /// Settle `scene` (loaded from `path`) through every package's
    /// [`settle_scene`](Registry::settle_scene), in order; the first error
    /// refuses it. With none registered the file is as it loaded.
    pub fn settle(
        &self,
        scene: &mut flint_scene::SceneFile,
        path: &std::path::Path,
    ) -> flint_core::Result<()> {
        for (_, settle) in &self.settles {
            settle(scene, path)?;
        }
        Ok(())
    }

    /// Every asset maker, in the order registered. None: every name a
    /// scene gives is a file.
    pub fn asset_makers(&self) -> impl Iterator<Item = &'static dyn AssetMaker> + '_ {
        self.makers.iter().map(|(_, m)| *m)
    }
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("built_with", &self.built)
            .field("builders", &self.builders.len())
            .field("asset_makers", &self.makers.len())
            .field("policies", &self.policies)
            .field(
                "modules",
                &self.modules().map(|m| m.name()).collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Named(&'static str, fn(&mut Registry));

    impl Package for Named {
        fn name(&self) -> &'static str {
            self.0
        }
        fn register(&self, registry: &mut Registry) {
            (self.1)(registry)
        }
    }

    fn none(_: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
        None
    }

    #[test]
    fn each_builder_belongs_to_the_package_that_registered_it() {
        let a = Named("a", |r| {
            r.scene_subsystem(none).policy("a");
        });
        let b = Named("b", |r| {
            r.scene_subsystem(none).scene_subsystem(none).policy("a");
        });
        let registry = Registry::of(&[&a, &b, &a]);
        assert_eq!(registry.built_with(), ["a", "b"]);
        let owners: Vec<&str> = registry.builders().map(|(p, _)| p).collect();
        assert_eq!(owners, ["a", "b", "b"]);
        assert_eq!(registry.policies(), ["a"], "a policy is named once");
        assert!(registry.has("b") && !registry.has("c"));
    }

    /// A package's registration point, filled by a package after it and
    /// read back; absent when its owner isn't in the run.
    #[test]
    fn a_later_package_fills_an_earlier_ones_point() {
        #[derive(Default)]
        struct Judge(Option<&'static str>);
        let owner = Named("owner", |r| {
            r.insert_point(Judge::default());
        });
        let filler = Named("filler", |r| {
            if let Some(judge) = r.point_mut::<Judge>() {
                judge.0 = Some("filler's");
            }
        });
        let registry = Registry::of(&[&owner, &filler]);
        assert_eq!(registry.point::<Judge>().unwrap().0, Some("filler's"));
        let alone = Registry::of(&[&filler]);
        assert!(alone.point::<Judge>().is_none());
    }
}
