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

/// What the kernel knows of a package this crate has a part of, in any
/// build, from its manifest (`build.rs`'s `WITHOUT`, #1585): what a scene
/// uses of it (`[provides]` components and scene tables) and what a build
/// without it still says and takes (`[without]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Without {
    /// Its `[package] name`.
    pub package: &'static str,
    /// `[provides] components`.
    pub components: &'static [&'static str],
    /// `[provides] scene_tables`.
    pub scene_tables: &'static [&'static str],
    /// `[without] does`: what the scene's parts do in a build without it.
    pub does: Option<&'static str>,
    /// `[without] policies`: names a game state may give in every build.
    pub policies: &'static [&'static str],
}

/// [`Without`] of every package in [`PLAYER_PACKAGES`], in that order.
pub const WITHOUT: &[Without] = generated::WITHOUT;

/// The kernel's own registration, `engine`'s: the `field` subsystem and
/// module, the policy names the manifests' `[without]` give (a game state
/// may name them in any build, #727), and the line a scene that uses a
/// package the run lacks hears, written from that package's manifest
/// (`missing_features::unbuilt`, ADR 0137 decision 8, #1585), so the
/// kernel names no package. It is always first.
pub static ENGINE: Part = Part {
    name: "engine",
    register: |registry| {
        use crate::player_app::subsystem;
        subsystem::field::register(registry);
        for without in WITHOUT {
            for policy in without.policies {
                registry.policy(policy);
            }
        }
        registry.scene_subsystem(crate::player_app::missing_features::unbuilt);
        // The point for a package's scene panels (the Living Props panel,
        // #1558), which the packages after the kernel fill. A kernel type
        // since #1559: the player makes them only where a debug HUD is
        registry.insert_point(flint_package::panels::ScenePanels::default());
    },
};

/// What `build.rs` writes from the package manifests (#1555): `ORDER`,
/// `PLAYER_PACKAGES`, `BUILT` and `moved()`, the player parts of the
/// packages in their own folders that this build has.
mod generated {
    include!(concat!(env!("OUT_DIR"), "/package_list.rs"));
}

// Every package is in its own folder (music, #1555; procgen, #1557;
// rhythm, #1556; living, #1558; debug-hud, #1559): its crate's
// `player::PACKAGE`, which `generated::moved` lists from the manifests.

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

/// `command` with the run's packages' own flags (living's `--compare` and
/// `--split`, `Registry::player_args`, #1585): what `flint-player` and
/// `flint play` parse.
pub fn player_flags(command: clap::Command) -> clap::Command {
    registry().augment_player_args(command)
}

/// The packages' flags, read back from the matches of a command
/// [`player_flags`] made, for [`PlayerArgs::packages`](crate::PlayerArgs::packages).
pub fn read_player_flags(
    matches: &clap::ArgMatches,
) -> Result<flint_package::PackageFlags, clap::Error> {
    registry().read_player_args(matches)
}

/// Whether this player lacks `package`: one this crate has a part of, which
/// the run's launcher didn't list (or the build compiled out).
pub fn lacks(package: &str) -> bool {
    PLAYER_PACKAGES.contains(&package) && !registry().has(package)
}

/// `dirs` less the schema directories of every package this player
/// [`lacks`] (#1529): a player built or launched without `living` doesn't
/// know `living_prop`, whether the engine's tree on the disk or a bundle
/// holds its schemas. Every host settles its list through it: a launch
/// (`prepare`, a playtest, the web), a transition, an embedding game's
/// `set_schema_dirs`.
pub fn built_schema_dirs(dirs: SchemaDirs) -> SchemaDirs {
    dirs.without_packages(lacks)
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
        "run `flint build` in the project: it builds a flint, a flint-player and \
         the game's own binary with the packages flint.toml lists (`flint build \
         --dry-run` shows the cargo commands)"
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
    let game = dirs
        .dirs
        .iter()
        .find_map(|d| flint_core::project::game_packages_beside(d));
    let manifests = Packages::read_with_game(&folder, game.as_deref());
    // A game's own package with a player part is in a binary whose
    // launcher listed it: the one `flint build` writes (#1562)
    let own = |m: &Manifest| {
        m.tier == flint_core::package::Tier::Game && m.player.is_some() && !registry().has(&m.name)
    };
    let packages: Vec<String> = in_use
        .on
        .iter()
        .filter(|name| manifests.get(name).is_some_and(|m| own(m) || lacks(m)))
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
            let on = manifests.get(name).unwrap().features.iter().all(|f| {
                // Every package is in its own folder (#1559), its
                // feature as the build script read it
                assert!(
                    generated::BUILT.iter().any(|(b, _)| b == f),
                    "{name}'s feature {f} is no player feature"
                );
                built_feature(f)
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

    /// The packages in their own folders come from the generated list,
    /// each exactly when this build has its feature, in registration order:
    /// procgen (standard tier) first, then living and music, then rhythm,
    /// which requires music (#1555, #1556, #1557, #1558).
    #[test]
    fn the_moved_packages_come_from_their_own_crates() {
        let moved: Vec<&str> = generated::moved().iter().map(|p| p.name()).collect();
        // In registration order: procgen (standard), then the feature
        // packages by name, rhythm after the music it requires; each in its
        // own folder when the build has it
        let expected: Vec<&str> = ["debug-hud", "procgen", "living", "music", "rhythm"]
            .into_iter()
            .filter(|name| built_feature(name))
            .collect();
        assert_eq!(moved, expected);
        for name in ["debug-hud", "procgen", "living", "music", "rhythm"] {
            assert!(PLAYER_PACKAGES.contains(&name), "{name}");
            assert_eq!(registry().has(name), built_feature(name), "{name}");
        }
    }

    /// The packages' own player flags join `flint-player`'s command line
    /// with no clash against the kernel's (clap's debug checks run on the
    /// whole command), and in a build with living, `--compare ,calm
    /// --split` is living's start; a launcher without living has no
    /// `--compare` (#1585).
    #[test]
    fn the_packages_flags_join_the_player_command_line() {
        use clap::{CommandFactory, FromArgMatches};
        let parse = |registry: &Registry, line: &[&str]| {
            let matches = registry
                .augment_player_args(crate::PlayerCli::command())
                .try_get_matches_from(line)?;
            let mut cli = crate::PlayerCli::from_arg_matches(&matches)?;
            cli.args.packages = registry.read_player_args(&matches)?;
            Ok::<_, clap::Error>(cli)
        };
        player_flags(crate::PlayerCli::command()).debug_assert();
        let line = [
            "flint-player",
            "grove.scene.toml",
            "--compare",
            ",calm",
            "--split",
        ];
        let bare = compose(&[]);
        let refused = parse(&bare, &line).unwrap_err();
        assert_eq!(refused.kind(), clap::error::ErrorKind::UnknownArgument);
        if !registry().has("living") {
            return;
        }
        let cli = parse(registry(), &line).unwrap();
        assert_eq!(cli.args.scene, "grove.scene.toml");
        assert!(cli.args.packages.packages().any(|p| p == "living"));
        let mut resources = flint_package::Resources::default();
        cli.args.packages.put_in(&mut resources);
        let start = flint_living::player::subsystem::LivingStart::of(&mut resources);
        assert_eq!(start.compare, Some((String::new(), "calm".to_string())));
        assert_eq!(start.compare_at_start, Some(true));
        // Living's own check of the two names, as clap's refusal
        let one = parse(registry(), &["flint-player", "--compare", "calm"]).unwrap_err();
        assert!(one.to_string().contains("two presets, as A,B"), "{one}");
        let split_alone = parse(registry(), &["flint-player", "--split"]).unwrap_err();
        assert_eq!(
            split_alone.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
    }

    /// Every policy name a manifest's `[without]` gives registers in any
    /// run, even one whose launcher lists no package (#727, #1585).
    #[test]
    fn the_manifests_policies_register_in_every_run() {
        let bare = compose(&[]);
        for without in WITHOUT {
            for policy in without.policies {
                assert!(bare.policies().contains(policy), "{policy}");
            }
        }
        assert!(WITHOUT.iter().any(|w| !w.policies.is_empty()));
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
