//! `flint check <dir|scene>`: the pre-flight (#687). Everything wrong with
//! a project, or with one scene, in one run: the list `flint bundle` checks
//! before it packs and `flint validate` checks of a scene, and more
//! (`crate::check` has the table).
//!
//! ```bash
//! flint check                         # the project in the working directory
//! flint check examples/               # every scene under a folder
//! flint check scenes/dock.scene.toml  # one scene and the files it names
//! flint check . --json                # one JSON document on stdout
//! ```
//!
//! It exits 1 when there is an error and 0 otherwise; a warning does not
//! fail it. `--strict` (as for `flint validate`) makes a field no schema
//! declares an error.

use crate::check::{self, Checks, Subject};
use anyhow::{bail, Result};
use std::path::PathBuf;

#[derive(clap::Args)]
pub struct CheckArgs {
    /// A project folder (every scene under it is checked) or one scene file
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Print one JSON document on stdout instead of text
    #[arg(long)]
    pub json: bool,

    /// Schema directories, later ones overriding earlier (repeatable).
    /// Without it: the project's flint.toml `schemas`, else schemas/ in the
    /// working directory, beside each scene and one level up
    #[arg(long, action = clap::ArgAction::Append)]
    pub schemas: Vec<String>,

    /// A field no schema declares is an error, not a warning (as
    /// `[project] strict = true` in flint.toml makes it for every run)
    #[arg(long)]
    pub strict: bool,

    /// A game's own script functions, for the script check
    #[command(flatten)]
    pub script: check::scripts::ScriptApiArgs,
}

/// The report for `args`, without printing it or exiting.
pub fn report(args: &CheckArgs) -> Result<check::Report> {
    let path = &args.path;
    let mut subject = if path.is_dir() {
        Subject::project(path, super::bundle::project_files(path)?, &args.schemas)
    } else if path.is_file() {
        Subject::scene(path, &args.schemas)
    } else {
        bail!(
            "{} is neither a folder nor a scene file",
            flint_core::project::shown(path)
        );
    };
    subject.strict = args.strict;
    subject.script_api = Some(args.script.api()?);
    Ok(check::run(&subject, Checks::ALL))
}

pub fn run(args: CheckArgs) -> Result<()> {
    let report = report(&args)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&check::json(&report))?);
    } else {
        print!("{}", check::text(&report));
    }
    if report.has_errors() {
        std::process::exit(1);
    }
    Ok(())
}
