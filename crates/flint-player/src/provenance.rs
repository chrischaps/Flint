//! Which code a binary was built from, and which checkout it ran in (#1290).
//!
//! A trace's `engine_commit` and a capture's `engine.commit` used to be the
//! HEAD of the project the player ran in: in a game's repository, the
//! game's commit, whatever engine the binary was built from. The engine's
//! commit is now stamped at compile time (`build.rs`: `ENGINE_BUILD_COMMIT`,
//! `ENGINE_BUILD_SUBDIR`), and the project's HEAD is kept beside it under
//! its own name (`project_commit`, a capture's `project`).

#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

/// A commit and whether the tree had changes to tracked files, each where
/// git could say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checkout {
    pub commit: Option<String>,
    pub dirty: Option<bool>,
}

/// The engine a run was built from and the project it ran in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Provenance {
    /// The commit the engine's sources were at when this binary was built
    /// ([`BUILD_COMMIT`]), and whether that tree has uncommitted changes
    /// ([`engine`]). `None` when the build couldn't tell: no git, or sources
    /// unpacked from an archive.
    pub engine: Checkout,
    /// Where the engine's sources sit in the checkout `engine.commit` is
    /// of, when not at its root: `engine/` in a game carrying the engine as
    /// a subtree, where the commit is the game's (and still pins the
    /// engine's sources, since the subtree is in it).
    pub engine_subdir: Option<String>,
    /// The checkout the run's project is in, at run time: a game's own
    /// repository, or the engine's when the project is one of its examples.
    pub project: Checkout,
}

/// The commit the engine's sources were at when this binary was built.
pub const BUILD_COMMIT: Option<&str> = non_empty(env!("ENGINE_BUILD_COMMIT"));

/// Where in [`BUILD_COMMIT`]'s checkout the engine's sources were, when not
/// its root (`engine/`).
pub const BUILD_SUBDIR: Option<&str> = non_empty(env!("ENGINE_BUILD_SUBDIR"));

/// The Cargo profile flint-player was built under (`debug`, `release`,
/// `profiling`...), from `build.rs` (#1093). A perf report names it, since
/// a debug build's frame times compare with nothing but a debug build's.
pub const BUILD_PROFILE: &str = env!("ENGINE_BUILD_PROFILE");

/// The `opt-level` flint-player itself was built at (`0`..`3`, `s`, `z`).
pub const BUILD_OPT_LEVEL: &str = env!("ENGINE_BUILD_OPT_LEVEL");

const fn non_empty(s: &'static str) -> Option<&'static str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// The engine this binary was built from: [`BUILD_COMMIT`], and whether the
/// tree it was built in has changes to tracked files. That is asked of the
/// build's own source folder at the first call and kept, and only while
/// that folder is still at the commit, so a binary run on another machine,
/// or after its checkout moved on, says it doesn't know (a change made
/// after the build, before the ask, counts as built in).
pub fn engine() -> Checkout {
    Checkout {
        commit: BUILD_COMMIT.map(str::to_string),
        dirty: source_dirty(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn source_dirty() -> Option<bool> {
    static DIRTY: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    *DIRTY.get_or_init(|| {
        let commit = BUILD_COMMIT?;
        // crates/flint-player/../.. is the engine's root, a game's engine/
        // included: status of "." there is the engine's files alone
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        if git(&root, &["rev-parse", "HEAD"])? != commit {
            return None;
        }
        git(
            &root,
            &["status", "--porcelain", "--untracked-files=no", "--", "."],
        )
        .map(|s| !s.is_empty())
    })
}

#[cfg(target_arch = "wasm32")]
fn source_dirty() -> Option<bool> {
    None
}

impl Provenance {
    /// The engine alone: what a run with no project checkout (the web) can
    /// say.
    pub fn built() -> Self {
        Self {
            engine: engine(),
            engine_subdir: BUILD_SUBDIR.map(str::to_string),
            project: Checkout::default(),
        }
    }

    /// The engine, and the checkout at `project_root`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn of(project_root: &Path) -> Self {
        let (commit, dirty) = git_state(project_root);
        Self {
            project: Checkout { commit, dirty },
            ..Self::built()
        }
    }
}

/// The commit of the checkout at `root` and whether its tracked files
/// changed, where git can say: a trace's `project_commit` and a capture's
/// `project` (#1290; before it, wrongly, their engine's). The player's
/// writers and `flint render --frame-capture` all ask it.
#[cfg(not(target_arch = "wasm32"))]
pub fn git_state(root: &Path) -> (Option<String>, Option<bool>) {
    let commit = git(root, &["rev-parse", "HEAD"]);
    let dirty = commit
        .as_ref()
        .and_then(|_| git(root, &["status", "--porcelain", "--untracked-files=no"]))
        .map(|s| !s.is_empty());
    (commit, dirty)
}

/// `git -C <root> <args>`'s output, trimmed, when it ran and succeeded.
#[cfg(not(target_arch = "wasm32"))]
fn git(root: &Path, args: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    #![allow(clippy::disallowed_methods)] // a test makes its own repository
    use super::*;

    fn git_in(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c"])
            .arg("commit.gpgsign=false")
            .args(args)
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "git {args:?}");
    }

    /// The case #1290 found: the player run in another repository (a game)
    /// names the engine it was built from, and the game's HEAD only as the
    /// project's.
    #[test]
    fn a_run_in_another_repository_names_the_engine_it_was_built_from() {
        let dir = std::env::temp_dir().join(format!("flint_1290_game_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let game = dir.as_path();
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("no git: skipped");
            return;
        }
        git_in(game, &["init", "-q"]);
        std::fs::write(game.join("flint.toml"), "[project]\n").unwrap();
        git_in(game, &["add", "flint.toml"]);
        git_in(game, &["commit", "-q", "-m", "a game"]);
        let (game_head, _) = git_state(game);
        let game_head = game_head.expect("the game has a HEAD");

        let p = Provenance::of(game);
        assert_eq!(p.project.commit.as_deref(), Some(game_head.as_str()));
        assert_eq!(p.project.dirty, Some(false));
        assert_eq!(p.engine.commit.as_deref(), BUILD_COMMIT);
        assert_ne!(p.engine.commit.as_deref(), Some(game_head.as_str()));

        std::fs::write(game.join("flint.toml"), "[project]\nid = \"x\"\n").unwrap();
        assert_eq!(Provenance::of(game).project.dirty, Some(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// This test runs in the engine's checkout, so the build knew its
    /// commit, the engine's own (at the root, no subdir), and still matches
    /// the source folder's HEAD unless it moved since the build.
    #[test]
    fn a_build_in_the_engines_checkout_stamps_its_commit() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let Some(head) = git(&root, &["rev-parse", "HEAD"]) else {
            eprintln!("not in a git checkout: skipped");
            return;
        };
        let built = BUILD_COMMIT.expect("a build in a checkout knows its commit");
        assert_eq!(built.len(), 40, "{built}");
        assert_eq!(BUILD_SUBDIR, None);
        let e = engine();
        if head == built {
            assert!(e.dirty.is_some(), "{e:?}");
        } else {
            assert_eq!(e.dirty, None, "the checkout moved on: dirty is unknown");
        }
    }
}
