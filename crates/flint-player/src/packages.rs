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
//! Also here: the binaries `flint build` set aside on Windows
//! ([`sweep_set_aside`]).

use flint_core::package::Packages;
use flint_core::project::{shown, SchemaDirs};
use std::path::{Path, PathBuf};

/// This player library's package features: each `[build] features` name a
/// package can give that flint-player has, and whether it was compiled in.
pub const BUILT_FEATURES: &[(&str, bool)] = &[
    ("debug-hud", cfg!(feature = "debug-hud")),
    ("living", cfg!(feature = "living")),
    ("music", cfg!(feature = "music")),
    ("rhythm", cfg!(feature = "rhythm")),
    ("procgen", cfg!(feature = "procgen")),
];

/// Whether this player lacks `feature`: one of its own package features,
/// compiled out. A feature it doesn't have at all (`ai`, the CLI's) is not
/// the player's to lack.
pub fn lacks(feature: &str) -> bool {
    BUILT_FEATURES.iter().any(|(f, on)| *f == feature && !on)
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
/// binary lacks, by `lacks` of each package's `[build] features`. `None`
/// with no `[packages]` table, with no engine package folder to read the
/// manifests from, or when the binary has them all.
pub fn unbuilt(dirs: &SchemaDirs, binary: &str, lacks: impl Fn(&str) -> bool) -> Option<Unbuilt> {
    let in_use = dirs.packages.as_ref()?;
    let folder = flint_core::project::engine_packages_for(&dirs.dirs)?;
    let manifests = Packages::read(&folder);
    let packages: Vec<String> = in_use
        .on
        .iter()
        .filter(|name| {
            manifests
                .get(name)
                .is_some_and(|p| p.features.iter().any(|f| lacks(f)))
        })
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
    fn a_feature_the_player_has_no_switch_for_is_not_lacking() {
        assert!(!lacks("ai"));
        assert!(!lacks("frame-history"));
        assert_eq!(lacks("living"), !cfg!(feature = "living"));
    }
}
