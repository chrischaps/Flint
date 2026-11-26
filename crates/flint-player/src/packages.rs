//! Whether this build has the packages a project lists (#1335, ADR 0136).
//!
//! A game's `[packages]` says which engine packages it uses, and `flint
//! build` builds a player and a CLI with only those. A binary built some
//! other way may lack one the list asks for: a slim `flint-player` in a
//! project that lists `living` would play its props standing still. So the
//! player, `flint play`, `flint render` and `flint check` hold the list to
//! the binary first, and a listed package the binary lacks is an error
//! naming `flint build` ([`Unbuilt`]).
//!
//! With no `[packages]` table there is nothing to hold: every package is
//! in, and the default build has every one.
//!
//! **The launcher** (ADR 0137 decision 5, #1552). What a binary was built
//! with is the list its launcher hands the kernel: [`launch`] at the top of
//! `main`, once a run, before any scene loads. Each package registers its
//! scene subsystems, policy names, script modules and debug keys into the
//! run's [`Registry`] ([`registry`]), which is where the player finds them;
//! nothing in the player lists a package's files. The stock `flint-player`
//! (and `flint`, and the web player) list [`stock`]: every package this
//! build has, as "no `[packages]` key means every package" says (ADR 0136
//! decision 3). A host that never calls [`launch`] (a test, a game binary
//! that hasn't yet) gets the stock list.
//!
//! Also here: the binaries `flint build` set aside on Windows
//! ([`sweep_set_aside`]).

use flint_core::package::{Package as Manifest, Packages};
use flint_core::project::{shown, SchemaDirs};
pub use flint_package::{Package, Registry};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// A package whose part is a registration function: the shape of every
/// package still inside this crate, and of a small package a game or a
/// test writes.
pub struct Part {
    /// Its `[package] name`.
    pub name: &'static str,
    /// What it registers.
    pub register: fn(&mut Registry),
}

impl Package for Part {
    fn name(&self) -> &'static str {
        self.name
    }

    fn register(&self, registry: &mut Registry) {
        (self.register)(registry)
    }
}

/// The kernel's own registration, `engine`'s: the `field` subsystem and
/// module, and, for each package the build left out, the line a scene that
/// uses it hears (and `living`'s policy name, which a game state may name
/// in any build). It is always first. Music's line asks the run's registry
/// (`missing_features::unbuilt_music`), since the package is its own
/// crate (#1555) and this one names no feature of it.
pub static ENGINE: Part = Part {
    name: "engine",
    register: |registry| {
        use crate::player_app::subsystem;
        subsystem::field::register(registry);
        // A game state may name `living` in any build (#727), and a scene
        // that uses living props in a build without the package hears so
        // once (its own crate since #1558)
        registry
            .policy("living")
            .scene_subsystem(crate::player_app::missing_features::unbuilt_living);
        registry.scene_subsystem(crate::player_app::missing_features::unbuilt_music);
        // Debug-hud's point for a package's scene panels (the Living Props
        // panel, #1558), which the packages after the kernel fill
        #[cfg(feature = "debug-hud")]
        registry.insert_point(flint_debug_ui::ScenePanels::default());
    },
};

/// What `build.rs` writes from the package manifests (#1555): `ORDER`,
/// `PLAYER_PACKAGES`, `BUILT` and `moved()`, the player parts of the
/// packages in their own folders that this build has.
mod generated {
    include!(concat!(env!("OUT_DIR"), "/package_list.rs"));
}

// The packages whose code is still in this crate, each behind its feature
// until it moves into `packages/<name>/` (#1556-#1559). One with nothing
// to register yet (debug-hud) is listed all the same: the list is what the
// binary was built with. A package in its own folder (music, #1555;
// procgen, #1557) is its crate's `player::PACKAGE`, which `generated::moved`
// lists from the manifests.
#[cfg(feature = "debug-hud")]
static DEBUG_HUD: Part = Part {
    name: "debug-hud",
    register: |_| {},
};
#[cfg(feature = "rhythm")]
static RHYTHM: Part = Part {
    name: "rhythm",
    register: crate::player_app::subsystem::rhythm::register,
};

/// The names of the packages this crate has a part of, in any build: what
/// a player can lack. Those whose `[build] features` are features of this
/// crate, from the manifests (`build.rs`). A package with no part here
/// (`ai`, the CLI's; a game's own) is not the player's to lack.
pub const PLAYER_PACKAGES: &[&str] = generated::PLAYER_PACKAGES;

/// Whether this build has the player feature `feature` on, for a feature a
/// package's `[build] features` names.
pub fn built_feature(feature: &str) -> bool {
    generated::BUILT.iter().any(|(f, on)| *on && *f == feature)
}

/// Every package this build has, each after the ones it `[requires]`: the
/// stock launcher's list, those still in this crate and those in their own
/// folder (`generated::moved`), in the manifests' order. A test build ends
/// it with the toy package.
#[allow(clippy::vec_init_then_push, unused_mut)] // a build with no package pushes none
pub fn stock() -> Vec<&'static dyn Package> {
    let mut list: Vec<&'static dyn Package> = generated::moved();
    #[cfg(feature = "debug-hud")]
    list.push(&DEBUG_HUD);
    #[cfg(feature = "rhythm")]
    list.push(&RHYTHM);
    let at = |p: &&dyn Package| {
        generated::ORDER
            .iter()
            .position(|name| *name == p.name())
            .unwrap_or(usize::MAX)
    };
    list.sort_by_key(at);
    #[cfg(test)]
    list.push(&crate::player_app::subsystem::toy::PACKAGE);
    list
}

/// The registry a launcher's list makes: the kernel's registration
/// ([`ENGINE`]), then each package in the order listed.
pub fn compose(packages: &[&dyn Package]) -> Registry {
    let mut registry = Registry::default();
    registry.add(&ENGINE);
    for package in packages {
        registry.add(*package);
    }
    registry
}

static RUN: OnceLock<Registry> = OnceLock::new();

/// Hand the kernel the run's packages: what a launcher's `main` does
/// first. `false`, with a warning, when the run's registry was already
/// made (a second call, or a scene, a script system or the debug-key table
/// asked first): the packages are registered once a run.
pub fn launch(packages: &[&dyn Package]) -> bool {
    let composed = compose(packages);
    let names = composed.built_with().join(", ");
    let set = RUN.set(composed).is_ok();
    if set {
        tracing::debug!("launched with packages {names}");
    } else {
        tracing::warn!(
            "packages {names} came too late: the run's packages were already registered ({})",
            registry().built_with().join(", ")
        );
    }
    set
}

/// The run's registry: the launcher's list, or [`stock`] when no launcher
/// named one.
pub fn registry() -> &'static Registry {
    RUN.get_or_init(|| compose(&stock()))
}

/// Whether this player lacks `package`: one this crate has a part of, which
/// the run's launcher didn't list (or the build compiled out).
pub fn lacks(package: &str) -> bool {
    PLAYER_PACKAGES.contains(&package) && !registry().has(package)
}

/// The packages a project lists that a binary was built without.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unbuilt {
    /// The binary that lacks them: `flint`, `flint-player`.
    pub binary: String,
    /// The packages, by name.
    pub packages: Vec<String>,
    /// The `flint.toml` whose `[packages]` lists them.
    pub manifest: PathBuf,
}

impl Unbuilt {
    /// What to do about it, the one wording every caller gives.
    pub fn hint(&self) -> String {
        "run `flint build` in the project: it builds a flint and a flint-player \
         with the packages flint.toml lists (`flint build --dry-run` shows the \
         cargo command)"
            .to_string()
    }
}

impl Unbuilt {
    /// What is missing, without what to do about it: a diagnostic's
    /// message, its hint beside it.
    pub fn what(&self) -> String {
        let names: Vec<String> = self.packages.iter().map(|p| format!("`{p}`")).collect();
        let (noun, list) = match names.as_slice() {
            [one] => ("package", one.clone()),
            [rest @ .., last] => ("packages", format!("{} and {last}", rest.join(", "))),
            [] => ("packages", String::new()),
        };
        format!(
            "this {} was built without the {list} {noun}, which {} uses",
            self.binary,
            shown_from_here(&self.manifest),
        )
    }
}

impl std::fmt::Display for Unbuilt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.what(), self.hint())
    }
}

impl std::error::Error for Unbuilt {}

/// The packages `dirs` let in (its `[packages]` resolved, #1334) that a
/// binary lacks, by `lacks` of each one's manifest: the player asks
/// [`lacks`] of its name (the launcher's list), the CLI of its `[build]
/// features`. `None` with no `[packages]` table, with no engine package
/// folder to read the manifests from, or when the binary has them all.
pub fn unbuilt(
    dirs: &SchemaDirs,
    binary: &str,
    lacks: impl Fn(&Manifest) -> bool,
) -> Option<Unbuilt> {
    let in_use = dirs.packages.as_ref()?;
    let folder = flint_core::project::engine_packages_for(&dirs.dirs)?;
    let manifests = Packages::read(&folder);
    let packages: Vec<String> = in_use
        .on
        .iter()
        .filter(|name| manifests.get(name).is_some_and(&lacks))
        .cloned()
        .collect();
    (!packages.is_empty()).then(|| Unbuilt {
        binary: binary.to_string(),
        packages,
        manifest: in_use.manifest.clone(),
    })
}

/// The running binary's name as a user types it (`flint`, `flint-player`,
/// a game's own), for [`Unbuilt::binary`].
pub fn this_binary() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "flint-player".to_string())
}

/// A path as the user would type it from the working directory.
fn shown_from_here(path: &Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok().map(shown))
        .unwrap_or_else(|| shown(path))
}

/// The name `flint build` moves a binary it is about to replace to, beside
/// it (ADR 0136 decision 10): `flint.exe` -> `flint.old.exe`. Windows lets
/// a running image be renamed in its folder, never overwritten.
pub fn set_aside_name(binary: &Path) -> PathBuf {
    let stem = binary
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = binary
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    binary.with_file_name(format!("{stem}{SET_ASIDE}{ext}"))
}

/// What a set-aside binary's stem ends with.
pub const SET_ASIDE: &str = ".old";

/// Delete the copies of this binary that `flint build` set aside
/// ([`set_aside_name`], and the numbered names it uses when one is still
/// running). A copy still running refuses, and is left for the next start.
/// Windows only: elsewhere nothing is set aside.
pub fn sweep_set_aside() {
    #[cfg(windows)]
    {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let (Some(dir), Some(stem)) = (exe.parent(), exe.file_stem()) else {
            return;
        };
        let prefix = format!("{}{SET_ASIDE}", stem.to_string_lossy());
        // The binary's own folder on the disk, not a game's files: past the
        // vfs on purpose
        #[allow(clippy::disallowed_methods)] // the exe's folder, not a game's file
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_set_aside(&name, &prefix) {
                // Still running: Windows refuses, and the next start tries
                #[allow(clippy::disallowed_methods)] // the exe's folder, not a game's file
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Whether `name` is a copy of the binary whose set-aside stem is `prefix`:
/// `flint.old.exe`, or `flint.old.2.exe` when the first was still running.
fn is_set_aside(name: &str, prefix: &str) -> bool {
    let Some(rest) = name.strip_prefix(prefix) else {
        return false;
    };
    let rest = rest.strip_suffix(".exe").unwrap_or(rest);
    rest.is_empty()
        || rest
            .strip_prefix('.')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_aside_copy_is_named_beside_the_binary() {
        assert_eq!(
            set_aside_name(Path::new("target/debug/flint.exe")),
            Path::new("target/debug/flint.old.exe")
        );
        assert_eq!(
            set_aside_name(Path::new("target/debug/flint-player")),
            Path::new("target/debug/flint-player.old")
        );
        assert!(is_set_aside("flint.old.exe", "flint.old"));
        assert!(is_set_aside("flint.old.3.exe", "flint.old"));
        assert!(!is_set_aside("flint.exe", "flint.old"));
        assert!(!is_set_aside("flint.old.pdb", "flint.old"));
        assert!(!is_set_aside("flint.older.exe", "flint.old"));
        // flint's sweep never takes the player's copy
        assert!(!is_set_aside("flint-player.old.exe", "flint.old"));
    }

    #[test]
    fn one_package_or_several_are_named_with_the_command_to_run() {
        let mut u = Unbuilt {
            binary: "flint".into(),
            packages: vec!["living".into()],
            manifest: PathBuf::from("game/flint.toml"),
        };
        let one = u.to_string();
        assert!(
            one.starts_with("this flint was built without the `living` package, which "),
            "{one}"
        );
        assert!(one.contains("run `flint build`"), "{one}");
        u.packages = vec!["living".into(), "music".into(), "rhythm".into()];
        assert!(
            u.to_string()
                .contains("without the `living`, `music` and `rhythm` packages"),
            "{u}"
        );
    }

    #[test]
    fn a_package_the_player_has_no_part_of_is_not_lacking() {
        assert!(!lacks("ai"));
        assert!(!lacks("engine"));
        assert!(!lacks("frame-history"), "a build option, not a package");
        assert_eq!(lacks("living"), !built_feature("living"));
        let built = built_feature;
        assert_eq!(lacks("procgen"), !built("procgen"));
    }

    /// The stock list is the build's packages, each a manifest's name,
    /// each after what it `[requires]`, and the kernel first.
    #[test]
    fn the_stock_list_is_every_package_this_build_has_in_requires_order() {
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages");
        let manifests = Packages::read(&folder);
        assert!(!manifests.failed(), "{:?}", manifests.problems);
        let registry = compose(&stock());
        let built = registry.built_with();
        assert_eq!(built[0], "engine");
        for (i, name) in built.iter().enumerate() {
            if *name == "toy" {
                continue; // the test build's own
            }
            let manifest = manifests
                .get(name)
                .unwrap_or_else(|| panic!("`{name}` is no package's name"));
            for required in manifest.requires.keys() {
                let at = built.iter().position(|b| b == required);
                assert!(
                    at.is_some_and(|at| at < i),
                    "{name} is listed before {required}, which it requires"
                );
            }
        }
        for name in PLAYER_PACKAGES {
            assert!(manifests.get(name).is_some(), "{name}");
            let on = manifests
                .get(name)
                .unwrap()
                .features
                .iter()
                .all(|f| match f.as_str() {
                    // The packages still in this crate, by their gates
                    "debug-hud" => cfg!(feature = "debug-hud"),
                    "rhythm" => cfg!(feature = "rhythm"),
                    // A package in its own folder (music, #1555; procgen,
                    // #1557; living, #1558), as the build script read the
                    // feature
                    other => {
                        assert!(
                            generated::BUILT.iter().any(|(f, _)| *f == other),
                            "{name}'s feature {other} is no player feature"
                        );
                        built_feature(other)
                    }
                });
            assert_eq!(registry.has(name), on, "{name}");
        }
        // The order the stock list had by hand before the build script
        // wrote it (#1555): a moved package registers where it always did,
        // so its subsystem runs and tears down in the same place
        let before = [
            "engine",
            "debug-hud",
            "procgen",
            "living",
            "music",
            "rhythm",
            "toy",
        ];
        let kept: Vec<&str> = before.into_iter().filter(|p| registry.has(p)).collect();
        assert_eq!(built, kept);
    }

    /// The default build has the music package through the generated list,
    /// after procgen (#1557) when the build has it: registration order.
    #[cfg(feature = "rhythm")]
    #[test]
    fn the_music_package_comes_from_its_own_crate() {
        assert!(PLAYER_PACKAGES.contains(&"music"));
        assert!(built_feature("music"));
        let moved: Vec<&str> = generated::moved().iter().map(|p| p.name()).collect();
        // In registration order: procgen (standard), then the feature
        // packages by name; each in its own folder when the build has it
        let want: Vec<&str> = ["procgen", "living", "music"]
            .into_iter()
            .filter(|name| built_feature(name))
            .collect();
        assert_eq!(moved, want);
        assert!(registry().has("music"));
    }

    /// A launcher that lists fewer packages than the build has makes a
    /// player that lacks the rest: the list, not the features, is what it
    /// was built with.
    #[test]
    fn a_launcher_that_leaves_a_package_out_lacks_it() {
        let registry = compose(&[]);
        assert_eq!(registry.built_with(), ["engine"]);
        let names: Vec<&str> = registry.builders().map(|(p, _)| p).collect();
        assert!(names.iter().all(|p| *p == "engine"), "{names:?}");
        assert!(registry.modules().all(|m| m.name() == "field"));
    }
}
