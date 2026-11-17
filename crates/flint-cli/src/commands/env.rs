//! `flint env` (#715): every `FLINT_*` environment switch the engine reads,
//! what it accepts, what unset means, and its value in this shell, from
//! the one table they're read through (`flint_core::env`).
//!
//! ```bash
//! flint env                     # every switch, grouped, with its value now
//! flint env --json              # the same as one JSON document
//! flint env --group render      # one group (window, render, player, assets, tests)
//! flint env --set               # only the switches set here
//! flint env --book              # write docs/book/src/cli-reference/environment.md
//! flint env --check-book        # exit 1 when that page is stale (CI)
//! ```
//!
//! It exits 1 when a set switch holds a value it doesn't accept, so a perf
//! script can run it first and stop on `FLINT_VSYNC=fasle`. A key
//! (`FLINT_*_API_KEY`) is listed as `(set)`, never shown.

use anyhow::{bail, Result};
use flint_core::env::{self, Group, Status};
use std::path::{Path, PathBuf};

/// The book's page, under its `src/`.
pub const BOOK_PAGE: &str = "cli-reference/environment.md";

#[derive(clap::Args)]
pub struct EnvArgs {
    /// Print the switches as JSON: each one's name, group, kind, what it
    /// accepts, its default, help, value now (null when unset) and the
    /// problem with that value (null when it's fine)
    #[arg(long)]
    pub json: bool,

    /// Only the switches of this group: window, render, player, assets or
    /// tests
    #[arg(long, value_parser = parse_group)]
    pub group: Option<Group>,

    /// Only the switches set in this environment
    #[arg(long)]
    pub set: bool,

    /// Write the book's page of switches (src/cli-reference/environment.md).
    /// DIR is the book (the folder holding `src/`); default `docs/book`
    #[arg(long, value_name = "DIR", num_args = 0..=1, default_missing_value = "docs/book",
          conflicts_with_all = ["json", "group", "set", "check_book"])]
    pub book: Option<PathBuf>,

    /// As --book, but write nothing: exit 1 when the page is stale (CI)
    #[arg(long, value_name = "DIR", num_args = 0..=1, default_missing_value = "docs/book",
          conflicts_with_all = ["json", "group", "set"])]
    pub check_book: Option<PathBuf>,
}

fn parse_group(s: &str) -> Result<Group, String> {
    Group::ALL
        .into_iter()
        .find(|g| g.name() == s.to_ascii_lowercase())
        .ok_or_else(|| {
            let names: Vec<&str> = Group::ALL.iter().map(|g| g.name()).collect();
            format!("no group {s:?}: one of {}", names.join(", "))
        })
}

pub fn run(args: EnvArgs) -> Result<()> {
    if let Some(dir) = args.check_book.as_deref() {
        return book(dir, true);
    }
    if let Some(dir) = args.book.as_deref() {
        return book(dir, false);
    }
    let all = env::statuses();
    let bad = all.iter().filter(|s| s.problem.is_some()).count();
    let shown: Vec<&Status> = all
        .iter()
        .filter(|s| args.group.is_none_or(|g| s.group == g))
        .filter(|s| !args.set || s.value.is_some())
        .collect();
    if args.json {
        let doc = serde_json::json!({
            "valid": bad == 0,
            "set": all.iter().filter(|s| s.value.is_some()).count(),
            "problems": bad,
            "switches": shown,
        });
        println!("{}", serde_json::to_string_pretty(&doc)?);
    } else {
        print!("{}", text(&shown));
    }
    if bad > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// The listing as text: a block per group, two or three lines a switch.
fn text(shown: &[&Status]) -> String {
    let mut out = String::new();
    for group in Group::ALL {
        let of: Vec<&&Status> = shown.iter().filter(|s| s.group == group).collect();
        if of.is_empty() {
            continue;
        }
        out.push_str(&format!("{}:\n", group.name()));
        for s in of {
            let value = match &s.value {
                Some(v) => format!("= {v:?}"),
                None => "unset".to_string(),
            };
            out.push_str(&format!("  {} {value}\n", s.name));
            out.push_str(&format!("      {}\n", s.help));
            out.push_str(&format!(
                "      accepts {}; unset: {}\n",
                s.accepts, s.default
            ));
            if let Some(p) = &s.problem {
                out.push_str(&format!("      ! {p}\n"));
            }
        }
    }
    let set = shown.iter().filter(|s| s.value.is_some()).count();
    let bad = shown.iter().filter(|s| s.problem.is_some()).count();
    out.push_str(&format!(
        "{} switches, {set} set{}\n",
        shown.len(),
        if bad > 0 {
            format!(", {bad} with a value they don't accept")
        } else {
            String::new()
        }
    ));
    out
}

/// `--book` / `--check-book`: the page is [`env::book_page`] whole, LF.
fn book(dir: &Path, check: bool) -> Result<()> {
    if !dir.join("src").is_dir() {
        bail!(
            "{} has no src/: give the book's folder (the one holding book.toml)",
            dir.display()
        );
    }
    let path = dir.join("src").join(BOOK_PAGE);
    let want = env::book_page();
    let have = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    let shown = path.display().to_string().replace('\\', "/");
    if have == want {
        println!("{shown}: up to date");
        return Ok(());
    }
    if check {
        eprintln!("{shown} is stale: run `flint env --book`");
        std::process::exit(1);
    }
    std::fs::write(&path, want)?;
    println!("wrote {shown}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_parse_by_name_and_a_bad_one_lists_them() {
        assert_eq!(parse_group("Render"), Ok(Group::Render));
        let e = parse_group("gpu").unwrap_err();
        assert!(e.contains("window, render, player, assets, tests"), "{e}");
    }

    #[test]
    fn text_says_a_problem_under_its_switch() {
        let mut s = env::VSYNC.status();
        s.value = Some("fasle".into());
        s.problem = env::VSYNC.parse("fasle").err();
        let out = text(&[&s]);
        assert!(
            out.starts_with("window:\n  FLINT_VSYNC = \"fasle\"\n"),
            "{out}"
        );
        assert!(
            out.contains("      ! FLINT_VSYNC=\"fasle\" is not 1|on|fifo"),
            "{out}"
        );
        assert!(out.ends_with("1 switches, 1 set, 1 with a value they don't accept\n"));
    }

    #[test]
    fn a_secret_is_listed_as_set_never_shown() {
        // The table's own status hides the key; the listing prints that
        let mut s = env::FLUX_API_KEY.status();
        s.value = Some("(set)".into());
        assert!(text(&[&s]).contains("FLINT_FLUX_API_KEY = \"(set)\""));
    }
}
