//! Every game kept in the checkout finds every file it names (#760).
//!
//! A game kept in the checkout is a top-level directory with a `flint.toml`
//! (the example game is one), found here by its manifest so the engine names
//! no game. Nothing ran one, so when `demo/` was renamed (#763) the
//! example game's `audio` symlink was left pointing at a folder that no
//! longer existed, its input config was written from the wrong root, and
//! `flint check` on it reported 16 missing files that nobody saw.
//!
//! This runs `flint check <game> --json` on each and fails on any
//! `missing-*` diagnostic. Other errors (a constraint the game's levels
//! don't meet) are the game's own and are left to `flint check`.

use std::path::{Path, PathBuf};
use std::process::Command;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The top-level directories with a `flint.toml`, by name.
fn checkout_games(root: &Path) -> Vec<String> {
    let mut games: Vec<String> = std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter(|e| e.path().join("flint.toml").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    games.sort();
    games
}

#[test]
fn every_game_kept_in_the_checkout_finds_every_file_it_names() {
    let root = repo_root();
    let games = checkout_games(&root);
    assert!(!games.is_empty(), "no game kept in {}", root.display());
    let mut missing = Vec::new();
    for game in &games {
        let out = Command::new(FLINT)
            .args(["check", game, "--json"])
            .current_dir(&root)
            .output()
            .expect("run flint check");
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "{game}: `flint check --json` printed no report ({e}):\n{}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        let diagnostics = report["diagnostics"].as_array().expect("diagnostics");
        for d in diagnostics {
            let code = d["code"].as_str().unwrap_or_default();
            if code.starts_with("missing-") {
                missing.push(format!(
                    "{}: {code}: {}",
                    d["file"].as_str().unwrap_or(game),
                    d["message"].as_str().unwrap_or_default()
                ));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "{} file(s) the games kept in the checkout name and don't have:\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
}
