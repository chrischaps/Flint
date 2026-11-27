//! The ai package's side of the `flint` command line (#1553, ADR 0137
//! section 4): what the CLI launcher lists when the package is in.
//!
//! [`extensions`] is the package's `cli()`: its subcommands of the kernel's
//! `asset` (`generate`, `regenerate`, `validate`, `manifest`, `costs`,
//! `job`) as a [`CliExtension`] under `asset`. Built against
//! `flint_package::cli`, so nothing here depends on flint-cli, and a CLI
//! built without the package has none of these commands in its parser.
//!
//! [`costs`] is the ledger, prices and meter every paid call goes through
//! (#721).
//!
//! Its steps of commands it doesn't own are a second extension, with no
//! command of its own (#1572, ADR 0137 "As built: the CLI launcher"):
//! `render --ai-refine` ([`look_refine`]: its flags in the slot `render`
//! declares, the loop behind the kernel's `LookRefines` point), `gen
//! --ai-*`, `--cost-report` and `--style-guide` ([`gen_ai`]: its groups in
//! the three slots `gen` declares, #1592, the steps behind procgen's
//! `AiPoint`) and `asset resolve --strategy ai_*` ([`resolve`]: the
//! kernel's `BatchResolve`). A flint without the package has none of its
//! flags in its parser; the resolve strategies, values of a flag the owner
//! declares, are served ([`Points::serve`]), so a run without the package
//! refuses them naming it.

pub mod asset;
pub mod costs;
pub mod gen_ai;
pub mod gen_refine;
pub mod look_refine;
pub mod resolve;

use anyhow::Result;
use clap::{ArgMatches, Command};
use flint_package::cli::{subcommands, CliExtension, Flags, Points};

/// Every extension the ai package adds to `flint`.
pub fn extensions() -> Vec<Box<dyn CliExtension>> {
    vec![subcommands(asset::run).under(&["asset"]), Box::new(Steps)]
}

/// The package's steps of other commands: no command, its points and its
/// flags.
struct Steps;

impl CliExtension for Steps {
    fn commands(&self) -> Vec<Command> {
        Vec::new()
    }

    fn run(&mut self, _matches: &ArgMatches) -> Result<()> {
        unreachable!("the ai package's steps add no command")
    }

    fn register(&self, points: &mut Points) {
        points
            .insert(flint_procgen_cli::gen::AiPoint(Box::new(gen_ai::Ai)))
            .insert(look_refine::point())
            .insert(resolve::point());
        for flag in resolve::SERVED {
            points.serve(flag);
        }
    }

    fn flags(&self) -> Vec<Flags> {
        let mut flags = vec![look_refine::flags()];
        flags.extend(gen_ai::flags());
        flags
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every command the manifest says the package provides is one this
    /// module adds, and nothing else: the launcher names the package for a
    /// command from the manifest, so the two must agree.
    #[test]
    fn the_extension_adds_what_the_manifest_lists() {
        let manifest = include_str!("../../../../package.toml");
        let manifest: toml::Value = toml::from_str(manifest).unwrap();
        let mut listed: Vec<String> = manifest["provides"]["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .collect();
        let mut added: Vec<String> = extensions()
            .iter()
            .flat_map(|e| {
                let under = e.under().join(" ");
                e.commands()
                    .into_iter()
                    .map(move |c| format!("{under} {}", c.get_name()).trim().to_string())
            })
            .collect();
        listed.sort();
        added.sort();
        assert_eq!(added, listed);
    }

    /// Every flag the manifest lists is one this package adds to a command
    /// (each long flag of its groups) or serves (a flag the owner declares),
    /// and nothing else: the launcher names the package for a flag from
    /// the manifest (#1572).
    #[test]
    fn the_flags_are_what_the_manifest_lists() {
        let manifest = include_str!("../../../../package.toml");
        let manifest: toml::Value = toml::from_str(manifest).unwrap();
        let mut listed: Vec<String> = manifest["provides"]["flags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .collect();
        let mut points = Points::new();
        let mut found: Vec<String> = Vec::new();
        for extension in extensions() {
            extension.register(&mut points);
            for group in extension.flags() {
                let cmd = (group.augment)(Command::new("x"));
                for arg in cmd.get_arguments() {
                    let long = arg.get_long().expect("a package's flag is a long one");
                    found.push(format!("{} --{long}", group.on.join(" ")));
                }
            }
        }
        found.extend(points.served().map(String::from));
        listed.sort();
        found.sort();
        assert_eq!(found, listed);
    }
}
