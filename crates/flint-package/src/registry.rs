//! Registration (ADR 0137 decision 4, #1552): a launcher lists its
//! packages, and each one's [`Package::register`] tells the kernel what it
//! brings, once a run, before any scene loads. The kernel then finds a
//! scene's subsystems, the game-state policy names, the script modules and
//! the debug keys here, never by a package's name.

use crate::{SceneCtx, Subsystem};
use flint_runtime::DebugKeyGroup;
use flint_script::ScriptModule;

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
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("built_with", &self.built)
            .field("builders", &self.builders.len())
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
}
