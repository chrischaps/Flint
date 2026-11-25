//! Commands a build left out (#1333, ADR 0136).
//!
//! flint-cli's package features (`living`, `music`, `rhythm`, `procgen`,
//! `ai`) each take crates out of the binary. A command that needs one of
//! them stays in the parser, so `flint --help` lists the same commands in
//! every build and a game's own command can't take its name. Its
//! arguments become [`Absent`], which takes whatever words follow, and
//! running it is [`missing`]: exit 2, naming the package to build with.
//!
//! [`stub!`] writes the module a left-out command's file would have been,
//! with the same `Args` type name and `run`, so `music.rs` and the others
//! read the same in both builds:
//!
//! ```ignore
//! #[cfg(feature = "rhythm")]
//! pub mod calibrate;
//! #[cfg(not(feature = "rhythm"))]
//! absent::stub!(calibrate, CalibrateArgs, "music calibrate", "rhythm");
//! ```

use std::ffi::OsString;

/// What a feature this binary was built with brings: the package names
/// (`[build] features` of `packages/<name>/package.toml`) that are in.
/// flint-player's `debug-hud` and `frame-history` are always on in the CLI.
pub const BUILT_FEATURES: &[(&str, bool)] = &[
    ("debug-hud", true),
    ("frame-history", true),
    ("living", cfg!(feature = "living")),
    ("music", cfg!(feature = "music")),
    ("rhythm", cfg!(feature = "rhythm")),
    ("procgen", cfg!(feature = "procgen")),
    ("ai", cfg!(feature = "ai")),
    ("audio", cfg!(feature = "audio")),
    ("gamepad", cfg!(feature = "gamepad")),
    ("dialogs", cfg!(feature = "dialogs")),
];

/// Whether this binary was built with `feature`.
pub fn built_with(feature: &str) -> bool {
    BUILT_FEATURES.iter().any(|(f, on)| *on && *f == feature)
}

/// The arguments of a command this build left out: every word after its
/// name, unread, so any command line that would have parsed still does.
#[derive(clap::Args, Debug, Default)]
pub struct Absent {
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        hide = true,
        num_args = 0..
    )]
    pub words: Vec<OsString>,
}

/// A command (or a flag of one) needs a package this binary was built
/// without. `output::finish` exits 2 on it, naming the package.
#[derive(Debug)]
pub struct MissingPackage {
    /// What was asked for: `flint music chart play`, `--ai-refine`.
    pub what: String,
    /// The package (and the flint-cli feature) it needs.
    pub package: &'static str,
}

/// The exit code a missing package leaves with: clap's for a command line
/// it can't take, since this one can't be run here either.
pub const EXIT: u8 = 2;

impl std::fmt::Display for MissingPackage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} needs the `{}` package, and this flint was built without it: \
             list `{} = \"*\"` under [packages] in flint.toml and run `flint build` \
             (`flint package list --built` names the packages this one has)",
            self.what, self.package, self.package
        )
    }
}

impl std::error::Error for MissingPackage {}

/// The error a left-out command returns. A build with every package that
/// refuses anything (ai, living, music, rhythm) has none to leave out. The
/// features are named one by one, not as `packages`: a build that lists
/// them (as a game's does) never turns the alias on (#1336's matrix).
#[cfg_attr(
    all(
        feature = "ai",
        feature = "living",
        feature = "music",
        feature = "rhythm"
    ),
    allow(dead_code)
)]
pub fn missing(what: impl Into<String>, package: &'static str) -> anyhow::Error {
    MissingPackage {
        what: what.into(),
        package,
    }
    .into()
}

/// A module standing in for a command file a build left out: its `Args`
/// type is [`Absent`] and its `run` is [`missing`]. Only the music and
/// rhythm commands are stubbed, so a build with both has none.
#[cfg_attr(all(feature = "music", feature = "rhythm"), allow(unused_macros))]
macro_rules! stub {
    ($module:ident, $args:ident, $words:literal, $package:literal) => {
        pub mod $module {
            pub type $args = $crate::commands::absent::Absent;
            pub fn run(_: $args) -> anyhow::Result<()> {
                Err($crate::commands::absent::missing(
                    concat!("flint ", $words),
                    $package,
                ))
            }
        }
    };
}
#[cfg_attr(all(feature = "music", feature = "rhythm"), allow(unused_imports))]
pub(crate) use stub;

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

    #[test]
    fn every_build_has_the_debug_panels() {
        assert!(built_with("debug-hud"));
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

    /// `words` after `flint`, parsed by the engine's parser and run.
    #[allow(dead_code)] // a build with every package runs none of the cases
    fn ran(words: &[&str]) -> anyhow::Result<()> {
        use clap::Parser as _;
        let argv = std::iter::once("flint").chain(words.iter().copied());
        let cli = crate::Cli::try_parse_from(argv)
            .unwrap_or_else(|e| panic!("{words:?} doesn't parse: {e}"));
        crate::run(cli)
    }

    /// The package a run that failed for want of one names.
    #[allow(dead_code)]
    fn wanted(words: &[&str]) -> &'static str {
        let e = ran(words).expect_err("a left-out command runs nothing");
        e.downcast_ref::<MissingPackage>()
            .unwrap_or_else(|| panic!("{words:?}: {e:#}"))
            .package
    }

    /// The done-when of #1333: in a build without rhythm the chart's
    /// player still parses, whatever follows it, and exits 2 naming rhythm.
    #[cfg(not(feature = "rhythm"))]
    #[test]
    fn without_rhythm_the_rhythm_commands_name_it() {
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
        let text = ran(&["music", "chart", "play", "x"])
            .unwrap_err()
            .to_string();
        assert!(text.contains("flint music chart play"), "{text}");
        assert!(text.contains("`rhythm`"), "{text}");
    }

    #[cfg(not(feature = "music"))]
    #[test]
    fn without_music_the_suite_commands_name_it() {
        assert_eq!(wanted(&["music", "validate", "s.suite.toml"]), "music");
        assert_eq!(wanted(&["music", "play", "s.suite.toml"]), "music");
        assert_eq!(
            wanted(&["music", "render", "s.suite.toml", "-o", "x.wav"]),
            "music"
        );
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

    /// A build with every package parses the same commands to their own
    /// arguments, not the stand-in's.
    #[cfg(all(feature = "rhythm", feature = "ai"))]
    #[test]
    fn with_every_package_no_command_is_a_stand_in() {
        use clap::CommandFactory as _;
        let mut cli = crate::Cli::command();
        let chart = cli
            .find_subcommand_mut("music")
            .and_then(|m| m.find_subcommand_mut("chart"))
            .and_then(|c| c.find_subcommand_mut("play"))
            .expect("music chart play");
        assert!(chart.get_arguments().any(|a| a.get_id() == "window"));
    }
}
