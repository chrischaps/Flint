//! Old command names, rewritten onto today's before clap parses (#327,
//! #709).
//!
//! The music tools were six top-level commands until #709 and are now
//! `flint music ...`; a golden suite ran as `render-suite <x>.goldens.toml`
//! until #327 gave it `flint goldens`. Each old spelling still runs, for one
//! release: [`route`] renames the subcommand word (`play-chart` becomes
//! `music chart play`) and says so. Done on the words, not as hidden clap
//! variants, so `flint --help` lists only today's commands, `--help` after
//! an old name prints the new command's, and the `Commands` enum doesn't
//! grow a variant for each (each costs the debug parser stack, #708).

use std::ffi::OsString;

/// Each old music command and the words that replace it.
pub const MUSIC: &[(&str, &[&str])] = &[
    ("validate-suite", &["music", "validate"]),
    ("play-suite", &["music", "play"]),
    ("render-suite", &["music", "render"]),
    ("calibrate", &["music", "calibrate"]),
    ("play-chart", &["music", "chart", "play"]),
    ("replay-chart", &["music", "chart", "replay"]),
];

/// The command line with an old command name replaced by today's, and,
/// when one was, the note to print (on stderr) saying what it is now.
/// Anything else passes through unchanged.
///
/// `render-suite` is two old names in one: with any later word ending in
/// `.goldens.toml` (no music flag takes one, and the manifest may come
/// after a golden flag's value: `--out-dir out x.goldens.toml`) it is
/// `goldens`, otherwise `music render`.
pub fn route(mut args: Vec<OsString>) -> (Vec<OsString>, Option<String>) {
    // The subcommand is the first word after the binary that is neither a
    // flag nor `--project`'s value (the one global option taking a value)
    let mut i = 1;
    while i < args.len() {
        let word = args[i].to_string_lossy();
        if word == "--project" {
            i += 2;
        } else if word.starts_with('-') {
            i += 1;
        } else {
            break;
        }
    }
    let Some(word) = args.get(i).map(|w| w.to_string_lossy().into_owned()) else {
        return (args, None);
    };
    let goldens = word == "render-suite"
        && args[i + 1..]
            .iter()
            .any(|w| w.to_string_lossy().ends_with(".goldens.toml"));
    let new: &[&str] = if goldens {
        &["goldens"]
    } else if let Some((_, new)) = MUSIC.iter().find(|(old, _)| *old == word) {
        new
    } else {
        return (args, None);
    };
    args.splice(i..=i, new.iter().map(OsString::from));
    let old = if goldens {
        format!("{word} <x>.goldens.toml")
    } else {
        word
    };
    let note = format!(
        "note: `flint {old}` is `flint {}` now; the old name goes in a later release",
        new.join(" ")
    );
    (args, Some(note))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routed(words: &[&str]) -> Vec<String> {
        let args = words.iter().map(OsString::from).collect();
        route(args)
            .0
            .into_iter()
            .map(|w| w.to_string_lossy().into_owned())
            .collect()
    }

    /// #327: `render-suite <x>.goldens.toml` still runs the goldens, the
    /// manifest anywhere after it, `--project` before it or not.
    #[test]
    fn render_suite_with_a_goldens_manifest_is_routed_to_goldens() {
        assert_eq!(
            routed(&["flint", "render-suite", "s.goldens.toml", "--bless"]),
            ["flint", "goldens", "s.goldens.toml", "--bless"]
        );
        assert_eq!(
            routed(&["flint", "render-suite", "--out-dir", "o", "s.goldens.toml"]),
            ["flint", "goldens", "--out-dir", "o", "s.goldens.toml"]
        );
        assert_eq!(
            routed(&["flint", "--project", "g", "render-suite", "s.goldens.toml"]),
            ["flint", "--project", "g", "goldens", "s.goldens.toml"]
        );
    }

    /// #709: each old music command is its `flint music` command, its
    /// arguments kept, `--project` before it or not.
    #[test]
    fn the_old_music_commands_are_routed_under_music() {
        assert_eq!(
            routed(&["flint", "render-suite", "m.suite.toml", "-o", "x.wav"]),
            ["flint", "music", "render", "m.suite.toml", "-o", "x.wav"]
        );
        assert_eq!(
            routed(&["flint", "render-suite", "--help"]),
            ["flint", "music", "render", "--help"]
        );
        assert_eq!(
            routed(&["flint", "validate-suite", "m.suite.toml", "--rhythm"]),
            ["flint", "music", "validate", "m.suite.toml", "--rhythm"]
        );
        assert_eq!(
            routed(&["flint", "--project", "g", "play-chart", "m.suite.toml"]),
            [
                "flint",
                "--project",
                "g",
                "music",
                "chart",
                "play",
                "m.suite.toml"
            ]
        );
        assert_eq!(
            routed(&["flint", "replay-chart", "m.suite.toml"]),
            ["flint", "music", "chart", "replay", "m.suite.toml"]
        );
        assert_eq!(
            routed(&["flint", "play-suite", "m"]),
            ["flint", "music", "play", "m"]
        );
        assert_eq!(
            routed(&["flint", "calibrate", "m"]),
            ["flint", "music", "calibrate", "m"]
        );
        let (_, note) = route(
            ["flint", "play-chart", "m"]
                .iter()
                .map(OsString::from)
                .collect(),
        );
        assert_eq!(
            note.as_deref(),
            Some(
                "note: `flint play-chart` is `flint music chart play` now; \
                 the old name goes in a later release"
            )
        );
    }

    /// Today's names, a word an old name only appears after, and no
    /// command at all pass through, with no note.
    #[test]
    fn everything_else_is_left_alone() {
        for kept in [
            &["flint", "render", "s.goldens.toml"][..],
            &["flint", "music", "render", "m.suite.toml", "-o", "x.wav"],
            &["flint", "goldens", "s.goldens.toml"],
            &["flint", "entity", "create", "calibrate"],
            &[
                "flint",
                "--project",
                "render-suite",
                "render",
                "x.goldens.toml",
            ],
            &["flint", "--help"],
            &["flint"],
        ] {
            let args = kept.iter().map(OsString::from).collect();
            let (out, note) = route(args);
            let out: Vec<String> = out
                .into_iter()
                .map(|w| w.to_string_lossy().into_owned())
                .collect();
            assert_eq!(out, kept, "{kept:?}");
            assert_eq!(note, None, "{kept:?}");
        }
    }
}
