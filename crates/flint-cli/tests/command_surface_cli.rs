//! The command surface (#709): `flint --help` lists the general tools plus
//! `music` and `goldens`; the six old music commands still run, as hidden
//! aliases that say their new spelling; the rumble spike is an example.

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
            "prefab", "asset", "play", "playtest", "capture", "gen", "render", "dxc", "style",
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

/// Each old name prints its new command's help, and a note on stderr
/// naming the new spelling.
#[test]
fn the_old_music_names_still_run_and_say_where_they_went() {
    for (old, new) in [
        ("validate-suite", "music validate"),
        ("play-suite", "music play"),
        ("render-suite", "music render"),
        ("calibrate", "music calibrate"),
        ("play-chart", "music chart play"),
        ("replay-chart", "music chart replay"),
    ] {
        let out = flint(&[old, "--help"]);
        let said = String::from_utf8_lossy(&out.stdout);
        let note = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{old}: {note}");
        assert!(
            said.contains(&format!("Usage: flint.exe {new} "))
                || said.contains(&format!("Usage: flint {new} ")),
            "{old} --help is {new}'s:\n{said}"
        );
        assert!(
            note.contains(&format!("`flint {old}` is `flint {new}` now")),
            "{old}: {note}"
        );
    }
}

/// The spike is an example now; its old command still answers, hidden.
#[test]
fn spike_rumble_is_a_hidden_alias_of_the_example() {
    let out = flint(&["spike-rumble", "--help"]);
    assert!(out.status.success());
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("--example spike_rumble"), "{said}");
    let example = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../flint-input-capture/examples/spike_rumble.rs"
    );
    assert!(std::path::Path::new(example).is_file(), "{example}");
}
