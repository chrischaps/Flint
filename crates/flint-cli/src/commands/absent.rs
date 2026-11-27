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
        })
    })
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
    missing(what, package)
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
    ("audio", cfg!(feature = "audio")),
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
    #[cfg(feature = "ai")]
    #[test]
    fn with_ai_the_asset_commands_are_its_extensions() {
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

    #[cfg(not(feature = "ai"))]
    #[test]
    fn without_ai_the_paid_calls_name_it() {
        assert_eq!(
            wanted(&["asset", "generate", "texture", "-d", "moss"]),
            "ai"
        );
        assert_eq!(wanted(&["asset", "costs"]), "ai");
        assert_eq!(wanted(&["asset", "job", "list"]), "ai");
        assert_eq!(
            wanted(&[
                "asset",
                "resolve",
                "x.scene.toml",
                "--strategy",
                "ai_generate"
            ]),
            "ai"
        );
        assert_eq!(wanted(&["gen", "--cost-report"]), "ai");
        assert_eq!(wanted(&["gen", "--ai-create", "a dead tree"]), "ai");
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
