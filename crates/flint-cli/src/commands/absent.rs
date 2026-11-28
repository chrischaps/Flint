//! Commands a build left out (#1333, ADR 0136).
//!
//! flint-cli's package features (`living`, `music`, `rhythm`, `procgen`,
//! `ai`) each take crates out of the binary. A package that registers its
//! commands with the CLI launcher (#1553: `ai`'s under `asset`; `music`
//! and `rhythm`'s, #1555) needs no stand-in: a build without it has none
//! of them in its parser, and a line asking for one is answered from the
//! manifests compiled in ([`missing_command`]): exit 2, naming the package
//! to build with. A kernel command a package fills in (`flint gen` for a
//! spec, `gen-preview`, `tex-edit`, #1557) names the package that provides
//! it the same way ([`missing_owned`]), and so does a flag that needs one
//! ([`missing`]). Until #1557 the left-out commands were `stub!` modules.

use std::ffi::OsString;

/// Every `packages/<name>/package.toml` of the tree this binary was built
/// from, as `(file, text)`: `build.rs` writes the list.
const MANIFESTS: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/package_manifests.rs"));

/// The manifests compiled in, read. One that doesn't read is left out
/// (`flint package list` names its problems).
fn manifests() -> Vec<flint_core::package::Package> {
    MANIFESTS
        .iter()
        .filter_map(|(file, text)| {
            flint_core::package::parse(text, std::path::Path::new(file)).ok()
        })
        .collect()
}

/// A command line `parser` refused with an unknown subcommand, when the
/// words ask for a command a package provides (its manifest's
/// `[provides] commands`) that this parser lacks: the package left out of
/// this build, which the line is told to add (#1553). `None` for any other
/// mistake, which clap explains itself.
pub fn missing_command(
    error: &clap::Error,
    argv: &[OsString],
    parser: &clap::Command,
) -> Option<MissingPackage> {
    if error.kind() != clap::error::ErrorKind::InvalidSubcommand {
        return None;
    }
    let words = command_words(argv);
    // How many of the words this parser knows as commands
    let mut known = 0;
    let mut at = parser;
    for word in &words {
        match at.find_subcommand(word) {
            Some(below) => {
                at = below;
                known += 1;
            }
            None => break,
        }
    }
    manifests().into_iter().find_map(|package| {
        let provided = package.provides.commands.iter().find(|command| {
            let path: Vec<&str> = command.split(' ').collect();
            path.len() > known
                && path.len() <= words.len()
                && path.iter().zip(&words).all(|(a, b)| a == b)
        })?;
        // Once a run, on its way out
        let package: &'static str = Box::leak(package.name.into_boxed_str());
        Some(MissingPackage {
            what: format!("flint {provided}"),
            package,
            unsynced: unsynced(package),
        })
    })
}

/// A command line `parser` refused with an unknown argument, when the
/// argument is a flag a package adds to that command (its manifest's
/// `[provides] flags`: `render --ai-refine`) that this parser lacks: the
/// package left out, named as for a command (#1572). `None` for any other
/// mistake.
pub fn missing_flag(
    error: &clap::Error,
    argv: &[OsString],
    parser: &clap::Command,
) -> Option<MissingPackage> {
    use clap::error::{ContextKind, ContextValue};
    if error.kind() != clap::error::ErrorKind::UnknownArgument {
        return None;
    }
    let Some(ContextValue::String(given)) = error.get(ContextKind::InvalidArg) else {
        return None;
    };
    let flag = given.split('=').next().unwrap_or(given);
    // The command the flag was given to: as many of the words as this
    // parser knows as commands
    let mut command = Vec::new();
    let mut at = parser;
    for word in command_words(argv) {
        match at.find_subcommand(&word) {
            Some(below) => {
                at = below;
                command.push(word);
            }
            None => break,
        }
    }
    manifests().into_iter().find_map(|package| {
        let line = package.provides.flags.iter().find(|line| {
            flint_core::package::FlagLine::parse(line).is_some_and(|l| {
                l.flag == flag && l.value.is_none() && l.command == command.as_slice()
            })
        })?;
        let package: &'static str = Box::leak(package.name.into_boxed_str());
        Some(MissingPackage {
            what: format!("flint {line}"),
            package,
            unsynced: unsynced(package),
        })
    })
}

/// A flag the owning command declares and a package serves (`gen
/// --ai-create`, `asset resolve --strategy ai_generate`: `line` as the
/// manifest's `[provides] flags` spells it), given in a run whose packages
/// don't serve it: the package that lists it, named from the manifests, as
/// [`missing_owned`] names a command's (#1572).
pub fn missing_served(line: &str) -> anyhow::Error {
    let owner = manifests()
        .into_iter()
        .find(|package| package.provides.flags.iter().any(|l| l == line))
        .map(|package| package.name)
        .unwrap_or_else(|| line.to_string());
    let package: &'static str = Box::leak(owner.into_boxed_str());
    let mut e = missing(format!("flint {line}"), package);
    judge(&mut e);
    e
}

/// A value of a flag the owning command declares (`render --sweep
/// wooze.glide`), in a run whose packages don't draw it: the package whose
/// manifest's `[provides] flags` has a line for the command and flag whose
/// value names `given` (`render --sweep wooze.*`, `FlagLine::names_value`),
/// named from the manifests, as [`missing_served`] names a whole line's
/// (#1595). `what` is what the error says needs it.
pub fn missing_value(
    command: &[&str],
    flag: &str,
    given: &str,
    what: impl Into<String>,
) -> anyhow::Error {
    let owner = owner_of_value(&manifests(), command, flag, given)
        .unwrap_or_else(|| format!("{} {flag} {given}", command.join(" ")));
    let package: &'static str = Box::leak(owner.into_boxed_str());
    let mut e = missing(what, package);
    judge(&mut e);
    e
}

/// The package among `manifests` whose flag line names `given` as a value
/// of `command`'s `flag`.
fn owner_of_value(
    manifests: &[flint_core::package::Package],
    command: &[&str],
    flag: &str,
    given: &str,
) -> Option<String> {
    manifests
        .iter()
        .find(|package| {
            package.provides.flags.iter().any(|line| {
                flint_core::package::FlagLine::parse(line)
                    .is_some_and(|l| l.command == command && l.flag == flag && l.names_value(given))
            })
        })
        .map(|package| package.name.clone())
}

/// `what` needs whichever package's manifest provides the command `words`
/// (`["gen"]`: procgen's), named from the manifests compiled in, so the
/// source names no package to say it (#1557).
pub fn missing_owned(what: impl Into<String>, words: &[&str]) -> anyhow::Error {
    let command = words.join(" ");
    let owner = manifests()
        .into_iter()
        .find(|package| package.provides.commands.contains(&command))
        .map(|package| package.name)
        .unwrap_or(command);
    // Once a run, on its way out
    let package: &'static str = Box::leak(owner.into_boxed_str());
    let mut e = missing(what, package);
    judge(&mut e);
    e
}

/// Whether `package`'s folder was in the tree this flint was built from
/// while the launchers had no feature for it (#1587): a `[build] features`
/// entry that is none of this crate's features, built or not, so
/// `scripts/package-sync.py` hadn't written it. Listing the package and
/// running `flint build` can't help then: the build refuses it too.
pub fn unsynced(package: &str) -> bool {
    unsynced_among(&manifests(), package)
}

/// [`unsynced`] over `manifests`: `package`'s has a `[build] features`
/// entry that isn't among [`built_features`]' names.
fn unsynced_among(manifests: &[flint_core::package::Package], package: &str) -> bool {
    manifests
        .iter()
        .find(|p| p.name == package)
        .is_some_and(|p| {
            p.features
                .iter()
                .any(|f| !built_features().any(|(known, _)| known == f))
        })
}

/// Sets a [`MissingPackage`] error's `unsynced` from the manifests compiled
/// in: a package's own command names one with `missing`, which can't know.
/// Any other error is left as it is.
pub fn judge(e: &mut anyhow::Error) {
    if let Some(m) = e.downcast_mut::<MissingPackage>() {
        m.unsynced = m.unsynced || unsynced(m.package);
    }
}

/// The leading subcommand words of a command line: `asset generate` of
/// `flint --json asset generate texture -d moss`, past the global
/// `--json` and `--project DIR` wherever they come first.
fn command_words(argv: &[OsString]) -> Vec<String> {
    let mut words = Vec::new();
    let mut rest = argv.iter().skip(1).map(|w| w.to_string_lossy());
    while let Some(word) = rest.next() {
        match &*word {
            "--json" => {}
            "--project" => {
                rest.next();
            }
            w if w.starts_with("--project=") => {}
            w if w.starts_with('-') => break,
            w => words.push(w.to_string()),
        }
    }
    words
}

/// flint-player's features the CLI always has. `debug-hud` is a package
/// feature of this crate's own since #1583, so it is in `PACKAGE_FEATURES`.
const ALWAYS_ON: &[(&str, bool)] = &[("frame-history", true)];

/// This crate's features that a package's `[build] features` names, each
/// with whether this build has it: `build.rs` writes it from the manifests
/// (#1554), so the source names no package.
const PACKAGE_FEATURES: &[(&str, bool)] =
    include!(concat!(env!("OUT_DIR"), "/package_features.rs"));

/// The device features.
const DEVICE_FEATURES: &[(&str, bool)] = &[
    ("sound", cfg!(feature = "sound")),
    ("gamepad", cfg!(feature = "gamepad")),
    ("dialogs", cfg!(feature = "dialogs")),
];

/// What a feature this binary was built with brings, each with whether it
/// is in: flint-player's `frame-history` (always on in the CLI), the
/// package features (`[build] features` of
/// `packages/<name>/package.toml`), then the device features.
pub fn built_features() -> impl Iterator<Item = &'static (&'static str, bool)> {
    ALWAYS_ON
        .iter()
        .chain(PACKAGE_FEATURES)
        .chain(DEVICE_FEATURES)
}

/// Whether this binary was built with `feature`.
pub fn built_with(feature: &str) -> bool {
    built_features().any(|(f, on)| *on && *f == feature)
}

/// A command (or a flag of one) needs a package this binary was built
/// without. `output::finish` exits 2 on it, naming the package. A
/// package's own command returns the same error, so it is flint-package's
/// (#1555).
pub use flint_package::cli::{missing, MissingPackage};

/// The exit code a missing package leaves with: clap's for a command line
/// it can't take, since this one can't be run here either.
pub const EXIT: u8 = 2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_package_names_the_command_and_the_package() {
        let e = missing("flint music chart play", "rhythm");
        let text = e.to_string();
        assert!(text.starts_with("flint music chart play needs the `rhythm` package"));
        assert!(
            text.contains("list `rhythm = \"*\"` under [packages]"),
            "{text}"
        );
        assert!(text.contains("run `flint build`"), "{text}");
        assert!(e.downcast_ref::<MissingPackage>().is_some());
    }

    /// The toy package's manifest (#1560's fixture): a folder whose
    /// feature no launcher has until `scripts/package-sync.py` runs.
    fn toy() -> flint_core::package::Package {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/packages/toy/package.toml");
        flint_core::package::read(&file).unwrap()
    }

    /// #1587: a package whose folder is in the tree and whose feature
    /// neither launcher has is told to run the sync script, not to list it
    /// and `flint build` (which refuses it too).
    #[test]
    fn a_package_the_launchers_dont_know_yet_names_the_sync_script() {
        let mut manifests = manifests();
        manifests.push(toy());
        assert!(unsynced_among(&manifests, "toy"));
        let e = MissingPackage {
            what: "flint toy springs".into(),
            package: "toy",
            unsynced: unsynced_among(&manifests, "toy"),
        };
        let text = e.to_string();
        assert!(
            text.starts_with("flint toy springs needs the `toy` package"),
            "{text}"
        );
        assert!(
            text.contains("run `python3 scripts/package-sync.py`"),
            "{text}"
        );
        assert!(!text.contains("[packages]"), "{text}");
    }

    /// Every package of the tree this flint was built from has its feature
    /// in the launchers, so each keeps the `[packages]` / `flint build`
    /// line, whether this build has it or not: the sync script is only for
    /// a folder the launchers don't know.
    #[test]
    fn a_package_the_launchers_have_keeps_the_packages_line() {
        let all = manifests();
        assert!(all.len() >= 7, "{} manifests", all.len());
        for p in &all {
            assert!(!unsynced(&p.name), "{} reads as unsynced", p.name);
            let mut e = missing("flint x", Box::leak(p.name.clone().into_boxed_str()));
            judge(&mut e);
            let text = e.to_string();
            assert!(text.contains("under [packages]"), "{text}");
            assert!(!text.contains("package-sync"), "{text}");
        }
        // A name no manifest has (a package's own `missing` of a typo'd
        // name) isn't judged unsynced either
        assert!(!unsynced("no-such-package"));
    }

    /// #1583: the debug panels are a package feature like the others, known
    /// in every build and in only those built with it; the frame history
    /// is in every build.
    #[test]
    fn the_debug_panels_are_a_package_feature_and_the_history_always_on() {
        assert!(PACKAGE_FEATURES.iter().any(|(f, _)| *f == "debug-hud"));
        assert!(!ALWAYS_ON.iter().any(|(f, _)| *f == "debug-hud"));
        assert!(built_with("frame-history"));
        assert!(!built_with("no-such-feature"));
    }

    #[test]
    fn a_missing_package_leaves_with_exit_2() {
        let code = crate::output::finish(Err(missing("flint x", "rhythm")));
        assert_eq!(
            format!("{code:?}"),
            format!("{:?}", std::process::ExitCode::from(EXIT))
        );
    }

    /// `words` after `flint`, parsed and run as this build's stock `flint`.
    #[allow(dead_code)] // a build with every package runs none of the cases
    fn ran(words: &[&str]) -> anyhow::Result<()> {
        crate::run_line(crate::stock(), words)
    }

    /// The done-when of #1553, in every build: a CLI launched with no
    /// package extensions has no `asset generate` (nor any of the ai
    /// package's commands) in its parser, and a line asking for one exits 2
    /// naming the package, from the manifest compiled in.
    #[test]
    fn a_cli_without_ai_has_no_asset_generate_and_names_ai() {
        use clap::CommandFactory as _;
        let bare = crate::extension::Added::new(Vec::new(), &crate::Cli::command())
            .unwrap()
            .augment(crate::Cli::command());
        let asset = bare.find_subcommand("asset").unwrap();
        for gone in [
            "generate",
            "regenerate",
            "validate",
            "manifest",
            "costs",
            "job",
        ] {
            assert!(asset.find_subcommand(gone).is_none(), "asset {gone}");
        }
        for (words, what) in [
            (
                &["asset", "generate", "texture", "-d", "moss"][..],
                "flint asset generate",
            ),
            (
                &["--json", "asset", "generate", "--help"],
                "flint asset generate",
            ),
            (&["--project", ".", "asset", "costs"], "flint asset costs"),
            (&["asset", "job", "list"], "flint asset job"),
        ] {
            let e = crate::run_line(Vec::new(), words).expect_err("no ai, no command");
            let missing = e
                .downcast_ref::<MissingPackage>()
                .unwrap_or_else(|| panic!("{words:?}: {e:#}"));
            assert_eq!((missing.what.as_str(), missing.package), (what, "ai"));
        }
        let text = crate::run_line(Vec::new(), &["asset", "generate"])
            .unwrap_err()
            .to_string();
        assert!(
            text.starts_with("flint asset generate needs the `ai` package"),
            "{text}"
        );
        assert!(
            text.contains("list `ai = \"*\"` under [packages]"),
            "{text}"
        );
    }

    /// A word no package provides is clap's own mistake, not a package's.
    #[test]
    fn a_typo_is_not_a_missing_package() {
        use clap::CommandFactory as _;
        let parser = crate::extension::Added::new(Vec::new(), &crate::Cli::command())
            .unwrap()
            .augment(crate::Cli::command());
        for words in [
            &["asset", "generat"][..],
            &["nope"],
            &["asset", "list", "x"],
        ] {
            let argv: Vec<OsString> = std::iter::once("flint")
                .chain(words.iter().copied())
                .map(OsString::from)
                .collect();
            let e = parser.clone().try_get_matches_from(&argv).unwrap_err();
            assert!(missing_command(&e, &argv, &parser).is_none(), "{words:?}");
        }
    }

    /// The stock `flint` of a build with ai has its commands where they
    /// always were.
    #[test]
    fn with_ai_the_asset_commands_are_its_extensions() {
        if !built_with("ai") {
            return; // a build without it has none (above)
        }
        let tree = crate::stock_command();
        let asset = tree.find_subcommand("asset").unwrap();
        for name in [
            "generate",
            "regenerate",
            "validate",
            "manifest",
            "costs",
            "job",
        ] {
            assert!(asset.find_subcommand(name).is_some(), "asset {name}");
        }
        let job = asset.find_subcommand("job").unwrap();
        assert!(job.find_subcommand("resume").is_some());
    }

    /// The package a run that failed for want of one names.
    #[allow(dead_code)]
    fn wanted(words: &[&str]) -> &'static str {
        let e = ran(words).expect_err("a left-out command runs nothing");
        e.downcast_ref::<MissingPackage>()
            .unwrap_or_else(|| panic!("{words:?}: {e:#}"))
            .package
    }

    /// The stock list less the rhythm package's extension, the one that
    /// adds `chart` (#1556): a launcher that lists every package this build
    /// has but rhythm.
    fn without_rhythm() -> Vec<Box<dyn crate::extension::CliExtension>> {
        crate::stock()
            .into_iter()
            .filter(|e| !e.commands().iter().any(|c| c.get_name() == "chart"))
            .collect()
    }

    /// The done-when of #1333, in every build since #1556: without rhythm
    /// the chart's player still parses, whatever follows it, and exits 2
    /// naming rhythm.
    #[test]
    fn without_rhythm_the_rhythm_commands_name_it() {
        let wanted = |words: &[&str]| {
            let e = crate::run_line(without_rhythm(), words).expect_err("no rhythm");
            e.downcast_ref::<MissingPackage>()
                .unwrap_or_else(|| panic!("{words:?}: {e:#}"))
                .package
        };
        assert_eq!(wanted(&["music", "chart", "play", "x"]), "rhythm");
        assert_eq!(
            wanted(&["music", "chart", "play", "x", "--window"]),
            "rhythm"
        );
        assert_eq!(
            wanted(&["music", "chart", "replay", "x", "--render", "y"]),
            "rhythm"
        );
        assert_eq!(wanted(&["music", "calibrate"]), "rhythm");
        let text = crate::run_line(without_rhythm(), &["music", "chart", "play", "x"])
            .unwrap_err()
            .to_string();
        assert!(text.contains("flint music chart"), "{text}");
        assert!(text.contains("`rhythm`"), "{text}");
    }

    /// The done-when of #1555, in every build: a CLI launched with no
    /// package extensions has no `music` command, and each of the music
    /// package's commands, and the rhythm package's under it, exits 2
    /// naming its package, from the manifests compiled in.
    #[test]
    fn a_cli_without_music_has_no_music_command_and_names_it() {
        use clap::CommandFactory as _;
        let bare = crate::extension::Added::new(Vec::new(), &crate::Cli::command())
            .unwrap()
            .augment(crate::Cli::command());
        assert!(bare.find_subcommand("music").is_none());
        for (words, what, package) in [
            (
                &["music", "validate", "s.suite.toml"][..],
                "flint music validate",
                "music",
            ),
            (
                &["music", "play", "s.suite.toml"],
                "flint music play",
                "music",
            ),
            (
                &["music", "render", "s.suite.toml", "-o", "x.wav"],
                "flint music render",
                "music",
            ),
            (
                &["--json", "music", "validate", "x"],
                "flint music validate",
                "music",
            ),
            (
                &["music", "chart", "play", "x"],
                "flint music chart",
                "rhythm",
            ),
            (&["music", "calibrate"], "flint music calibrate", "rhythm"),
        ] {
            let e = crate::run_line(Vec::new(), words).expect_err("no music, no command");
            let missing = e
                .downcast_ref::<MissingPackage>()
                .unwrap_or_else(|| panic!("{words:?}: {e:#}"));
            assert_eq!((missing.what.as_str(), missing.package), (what, package));
        }
    }

    /// With the music package and not rhythm's, `flint music` has the
    /// music's three commands, and a rhythm one names rhythm.
    #[test]
    fn music_without_rhythm_names_rhythm_for_its_commands() {
        let e = crate::run_line(without_rhythm(), &["music", "chart", "play", "x"])
            .expect_err("no rhythm");
        let missing = e.downcast_ref::<MissingPackage>().unwrap();
        assert_eq!(
            (missing.what.as_str(), missing.package),
            ("flint music chart", "rhythm")
        );
    }

    /// The stock `flint` of a build with music has its commands where they
    /// always were, rhythm's beside them.
    #[test]
    fn with_music_the_music_commands_are_its_extensions() {
        if !built_with("rhythm") {
            return; // a build without them has none (above)
        }
        let tree = crate::stock_command();
        let music = tree.find_subcommand("music").unwrap();
        for name in ["validate", "play", "render", "calibrate", "chart"] {
            assert!(music.find_subcommand(name).is_some(), "music {name}");
        }
    }

    /// The stock list less the ai package's extensions: its `asset`
    /// commands and its steps of other commands (the one extension with
    /// flags on a command it doesn't own).
    fn without_ai() -> Vec<Box<dyn crate::extension::CliExtension>> {
        crate::stock()
            .into_iter()
            .filter(|e| {
                e.flags().is_empty() && !e.commands().iter().any(|c| c.get_name() == "generate")
            })
            .collect()
    }

    /// The done-when of #1572 (and #1333's paid calls), in every build: a
    /// launcher without the ai package has no `--ai-refine` on `render`,
    /// and each of the package's flags, on a command it adds them to or one
    /// whose owner declares them, exits 2 naming the package, from the
    /// manifest compiled in.
    #[test]
    fn without_ai_the_paid_calls_and_their_flags_name_it() {
        use clap::{CommandFactory as _, Parser as _};
        let launcher = crate::extension::Added::new(without_ai(), &crate::Cli::command()).unwrap();
        let tree = launcher.parser(crate::Cli::command);
        let render = tree.find_subcommand("render").unwrap();
        for gone in ["ai_refine", "refine_rounds", "refine_out", "ai_provider"] {
            assert!(
                render.get_arguments().all(|a| a.get_id() != gone),
                "render {gone}"
            );
        }
        let named = |words: &[&str]| {
            let e = crate::run_line(without_ai(), words).expect_err("no ai, no paid call");
            let missing = e
                .downcast_ref::<MissingPackage>()
                .unwrap_or_else(|| panic!("{words:?}: {e:#}"));
            (missing.what.clone(), missing.package)
        };
        let refine = [
            "render",
            "x.scene.toml",
            "--look",
            "noir",
            "--reference",
            "r.png",
        ];
        assert_eq!(
            named(&[&refine[..], &["--ai-refine"]].concat()),
            ("flint render --ai-refine".to_string(), "ai")
        );
        assert_eq!(
            named(&["--json", "render", "x.scene.toml", "--refine-rounds=3"]),
            ("flint render --refine-rounds".to_string(), "ai")
        );
        assert_eq!(
            named(&["asset", "generate", "texture", "-d", "moss"]).1,
            "ai"
        );
        assert_eq!(named(&["asset", "costs"]).1, "ai");
        assert_eq!(named(&["asset", "job", "list"]).1, "ai");
        let text = crate::run_line(without_ai(), &[&refine[..], &["--ai-refine"]].concat())
            .unwrap_err()
            .to_string();
        assert!(
            text.starts_with("flint render --ai-refine needs the `ai` package"),
            "{text}"
        );
        // gen's AI, cost and style-guide flags are the package's groups
        // too (#1592): none in the parser, each named as render's are
        let gen = tree.find_subcommand("gen").unwrap();
        for gone in [
            "ai_assist",
            "ai_create",
            "ai_refine",
            "reference",
            "ai_provider",
            "max_cost",
            "cost_report",
            "since",
            "style_guide",
        ] {
            assert!(
                gen.get_arguments().all(|a| a.get_id() != gone),
                "gen {gone}"
            );
        }
        assert_eq!(
            named(&["gen", "--ai-create", "a dead tree"]),
            ("flint gen --ai-create".to_string(), "ai")
        );
        assert_eq!(
            named(&["gen", "--cost-report"]),
            ("flint gen --cost-report".to_string(), "ai")
        );
        assert_eq!(
            named(&["gen", "specs/oak.procgen.toml", "--ai-assist"]),
            ("flint gen --ai-assist".to_string(), "ai")
        );
        assert_eq!(
            named(&[
                "gen",
                "x.procgen.toml",
                "--validate",
                "--style-guide=g.toml"
            ]),
            ("flint gen --style-guide".to_string(), "ai")
        );
        assert_eq!(
            named(&["gen", "x.procgen.toml", "--ai-provider", "mock"]).1,
            "ai"
        );
        // procgen's own flags stay, and need no package of ai's
        for kept in ["print_spec", "save_spec", "strip", "list_generators"] {
            assert!(
                gen.get_arguments().any(|a| a.get_id() == kept),
                "gen {kept}"
            );
        }
        let help = tree
            .clone()
            .find_subcommand_mut("gen")
            .unwrap()
            .render_long_help()
            .to_string();
        for absent in [
            "AI assistance:",
            "Cost tracking:",
            "--ai-create",
            "--cost-report",
        ] {
            assert!(!help.contains(absent), "gen --help has {absent}:\n{help}");
        }
        let points = crate::points_of(&without_ai());
        let e = super::super::asset::batch_of(&points, "ai_generate").expect_err("no batch");
        let missing = e.downcast_ref::<MissingPackage>().unwrap();
        assert_eq!(
            (missing.what.as_str(), missing.package),
            ("flint asset resolve --strategy ai_generate", "ai")
        );
    }

    /// A flag no package adds is clap's own mistake, not a package's.
    #[test]
    fn an_unknown_flag_is_not_a_missing_package() {
        use clap::CommandFactory as _;
        let parser = crate::extension::Added::new(Vec::new(), &crate::Cli::command())
            .unwrap()
            .parser(crate::Cli::command);
        for words in [
            &["render", "x", "--ai-refin"][..],
            &["check", "--ai-refine"],
            &["render", "x", "--nope"],
        ] {
            let argv: Vec<OsString> = std::iter::once("flint")
                .chain(words.iter().copied())
                .map(OsString::from)
                .collect();
            let e = parser.clone().try_get_matches_from(&argv).unwrap_err();
            assert!(missing_flag(&e, &argv, &parser).is_none(), "{words:?}");
        }
    }

    /// The stock `flint` of a build with ai has `render`'s refine group
    /// where its heading always was: after the measures, before the
    /// debug views (#1572).
    #[test]
    fn with_ai_the_refine_flags_keep_their_place_in_render_help() {
        if !built_with("ai") {
            return;
        }
        let mut tree = crate::stock_command();
        let help = tree
            .find_subcommand_mut("render")
            .unwrap()
            .render_long_help()
            .to_string();
        // The line a heading or an option starts (other flags' help names
        // `--ai-refine` in prose)
        let lines: Vec<&str> = help.lines().map(str::trim).collect();
        let at = |line: &str| {
            lines
                .iter()
                .position(|l| *l == line)
                .unwrap_or_else(|| panic!("no line {line} in render --help"))
        };
        assert!(at("Measure and report:") < at("Refine a look:"));
        assert!(at("Refine a look:") < at("--ai-refine"));
        assert!(at("--ai-refine") < at("--max-cost <MAX_COST>"));
        assert!(at("--max-cost <MAX_COST>") < at("Debug, lists, GPU and schemas:"));
    }

    /// The stock `flint` has `gen`'s three slots filled where their flags
    /// always were (#1592): the AI group after the generators, procgen's
    /// `--print-spec` still under its heading, the cost group after it,
    /// and `--style-guide` between `--report` and the living props.
    #[test]
    fn with_ai_the_gen_flags_keep_their_places_in_gen_help() {
        if !built_with("ai") {
            return;
        }
        let mut tree = crate::stock_command();
        let help = tree
            .find_subcommand_mut("gen")
            .unwrap()
            .render_long_help()
            .to_string();
        let lines: Vec<&str> = help.lines().map(str::trim).collect();
        let at = |line: &str| {
            lines
                .iter()
                .position(|l| *l == line)
                .unwrap_or_else(|| panic!("no line {line} in gen --help"))
        };
        let order = [
            "--report <REPORT>",
            "--style-guide <STYLE_GUIDE>",
            "Generators:",
            "AI assistance:",
            "--ai-assist",
            "--refine-rounds <N>",
            "--print-spec",
            "--save-spec <PATH>",
            "Cost tracking:",
            "--max-cost <USD>",
            "--since <DATE>",
            "Living props:",
        ];
        for pair in order.windows(2) {
            assert!(at(pair[0]) < at(pair[1]), "{} before {}", pair[0], pair[1]);
        }
        assert!(help.contains("(not needed with --ai-create, --cost-report,"));
    }

    /// The done-when of #1543, in every build since #1557: a launcher
    /// without the procgen package (no points) parses `flint gen` and the
    /// spec editors and exits 2 naming it, a spec handed to `flint edit`
    /// before the file is read, while a glyph set is still `flint gen`'s
    /// (flint-render's, the kernel's): one that isn't there fails to load,
    /// naming no package.
    #[test]
    fn without_procgen_the_spec_commands_name_it() {
        use clap::Parser as _;
        let none = flint_package::cli::Points::new();
        let gen = |words: &[&str]| {
            let line = std::iter::once("flint")
                .chain(std::iter::once("gen"))
                .chain(words.iter().copied());
            let crate::Cli { command, .. } = crate::Cli::parse_from(line);
            let crate::Commands::Gen(args) = command else {
                unreachable!()
            };
            super::super::gen::run_in(*args, &none)
        };
        let package = |run: anyhow::Result<()>| {
            let e = run.expect_err("no package runs it");
            e.downcast_ref::<MissingPackage>()
                .unwrap_or_else(|| panic!("{e:#}"))
                .package
        };
        assert_eq!(
            package(gen(&["specs/oak.procgen.toml", "-o", "x.glb"])),
            "procgen"
        );
        assert_eq!(package(gen(&["--list-generators"])), "procgen");
        assert_eq!(package(gen(&["--describe", "texture_v1"])), "procgen");
        let editors = |words: &[&str]| {
            let line = std::iter::once("flint").chain(words.iter().copied());
            match crate::Cli::parse_from(line).command {
                crate::Commands::GenPreview(args) => {
                    super::super::spec_edit::gen_preview_in(*args, &none)
                }
                crate::Commands::TexEdit(args) => {
                    super::super::spec_edit::tex_edit_in(*args, &none)
                }
                crate::Commands::Edit(args) => super::super::edit_router::run_in(*args, &none),
                _ => unreachable!(),
            }
        };
        assert_eq!(
            package(editors(&["gen-preview", "specs/oak.procgen.toml"])),
            "procgen"
        );
        assert_eq!(
            package(editors(&["tex-edit", "specs/wall.procgen.toml"])),
            "procgen"
        );
        assert_eq!(
            package(editors(&[
                "edit",
                "no/such/oak.procgen.toml",
                "--render",
                "x.png"
            ])),
            "procgen"
        );
        let text = gen(&["specs/oak.procgen.toml"]).unwrap_err().to_string();
        assert!(
            text.starts_with("flint gen needs the `procgen` package"),
            "{text}"
        );
        let glyphs =
            gen(&["no/such/set.glyphs.toml", "--dry-run"]).expect_err("there is no such set");
        assert!(
            glyphs.downcast_ref::<MissingPackage>().is_none(),
            "{glyphs:#}"
        );
    }

    /// #1595: a `render --sweep` key a package draws names that package
    /// from its manifest's `[provides] flags` (`render --sweep wooze.*`), in
    /// every build, so the render's source names none; a key no line names
    /// is no package's.
    #[test]
    fn a_sweep_key_a_package_draws_names_it_from_its_manifest() {
        let all = manifests();
        let owner = |key: &str| owner_of_value(&all, &["render"], "--sweep", key);
        assert_eq!(owner("wooze.glide").as_deref(), Some("living"));
        assert_eq!(owner("wooze.preset").as_deref(), Some("living"));
        assert_eq!(owner("living.echoes.spread").as_deref(), Some("living"));
        assert_eq!(owner("ascii.cell"), None);
        assert_eq!(owner("post.exposure"), None);
        assert_eq!(
            owner_of_value(&all, &["gen"], "--sweep", "wooze.glide"),
            None
        );

        let e = missing_value(
            &["render"],
            "--sweep",
            "wooze.glide",
            "flint render --sweep wooze.glide",
        );
        let missing = e
            .downcast_ref::<MissingPackage>()
            .expect("a missing package");
        assert_eq!(
            (missing.what.as_str(), missing.package),
            ("flint render --sweep wooze.glide", "living")
        );
        assert!(
            e.to_string()
                .starts_with("flint render --sweep wooze.glide needs the `living` package"),
            "{e}"
        );
        let code = crate::output::finish(Err(e));
        assert_eq!(
            format!("{code:?}"),
            format!("{:?}", std::process::ExitCode::from(EXIT))
        );
    }

    /// A build with every package parses the same commands to their own
    /// arguments, not the stand-in's.
    #[test]
    fn with_every_package_no_command_is_a_stand_in() {
        if !(built_with("rhythm") && built_with("ai")) {
            return;
        }
        // The packages' commands are the launcher's extensions (#1555)
        let mut cli = crate::stock_command();
        let chart = cli
            .find_subcommand_mut("music")
            .and_then(|m| m.find_subcommand_mut("chart"))
            .and_then(|c| c.find_subcommand_mut("play"))
            .expect("music chart play");
        assert!(chart.get_arguments().any(|a| a.get_id() == "window"));
    }
}
