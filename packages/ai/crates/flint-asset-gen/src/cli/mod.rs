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
//! (#721). The kernel's `gen --ai-*`, `--cost-report` and `render
//! --ai-refine` still live in flint-cli behind its `ai` feature and reach
//! it there (ADR 0137, "As built: the CLI launcher").

pub mod asset;
pub mod costs;

use flint_package::cli::{subcommands, CliExtension};

/// Every extension the ai package adds to `flint`.
pub fn extensions() -> Vec<Box<dyn CliExtension>> {
    vec![subcommands(asset::run).under(&["asset"])]
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
}
