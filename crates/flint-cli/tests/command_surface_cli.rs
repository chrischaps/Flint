//! The command surface (#709): `flint --help` lists the general tools plus
//! `music` and `goldens`; the six old music commands and `spike-rumble`
//! are unknown (#1141, after a release as aliases); the rumble spike is an
//! example.

use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn flint(args: &[&str]) -> Output {
    Command::new(FLINT).args(args).output().expect("run flint")
}

/// The command names a `--help` lists under `Commands:`.
fn commands(help: &str) -> Vec<String> {
    help.lines()
        .skip_while(|l| !l.starts_with("Commands:"))
        .skip(1)
        .take_while(|l| l.starts_with("  "))
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .filter(|c| c != "help")
        .collect()
}

#[test]
fn top_level_help_lists_the_general_tools_plus_music_and_goldens() {
    let out = flint(&["--help"]);
    assert!(out.status.success());
    let listed = commands(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(
        listed,
        [
            "init", "entity", "scene", "query", "schema", "script", "edit", "validate", "check",
            "frame", "profile", "examples", "env", "bundle", "export", "music", "goldens",
            "prefab", "asset", "package", "build", "play", "playtest", "capture", "gen", "render",
            "dxc", "style",
        ],
        "a new top-level command belongs here only if it is a general tool"
    );
}

#[test]
fn music_holds_the_six_rhythm_tools() {
    let music = flint(&["music", "--help"]);
    assert!(music.status.success());
    assert_eq!(
        commands(&String::from_utf8_lossy(&music.stdout)),
        ["validate", "play", "render", "calibrate", "chart"]
    );
    let chart = flint(&["music", "chart", "--help"]);
    assert!(chart.status.success());
    assert_eq!(
        commands(&String::from_utf8_lossy(&chart.stdout)),
        ["play", "replay"]
    );
}

/// #1141: the old names ran for one release after #709 and are gone. Each
/// is an unknown command, `--help` after it included, and a goldens
/// manifest after `render-suite` (#327's spelling) is no exception.
#[test]
fn the_old_names_are_unknown_commands() {
    for old in [
        &["validate-suite", "--help"][..],
        &["play-suite", "--help"],
        &["render-suite", "--help"],
        &["render-suite", "s.goldens.toml"],
        &["calibrate", "--help"],
        &["play-chart", "--help"],
        &["replay-chart", "--help"],
        &["spike-rumble", "--help"],
    ] {
        let out = flint(old);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{old:?} still runs");
        assert!(
            err.contains(&format!("unrecognized subcommand '{}'", old[0])),
            "{old:?}: {err}"
        );
    }
    // The spike lives on as an example program
    let example = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../flint-input-capture/examples/spike_rumble.rs"
    );
    assert!(std::path::Path::new(example).is_file(), "{example}");
}
