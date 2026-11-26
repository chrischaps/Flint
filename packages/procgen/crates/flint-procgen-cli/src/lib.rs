//! The procgen package's part of the `flint` command line (#1557, ADR 0137
//! decision 4). Until #1557 these were flint-cli's `commands/` files behind
//! its `procgen` feature; they build against the Package API now, and
//! flint-cli names none of them.
//!
//! The commands stay the kernel's (`flint gen`, `flint edit`, the hidden
//! `gen-preview` and `tex-edit`), so `flint --help` is the same in every
//! build and a glyph set or a terrain spec is the kernel's to run. What
//! this crate brings it registers as points ([`cli::extensions`], in
//! [`CliExtension::register`]):
//!
//! - [`SpecGen`]: `flint gen <spec>`, the whole run ([`gen::run`]);
//! - [`SpecEditors`]: the previewer and the texture graph editor a
//!   `.procgen.toml` opens in;
//! - [`SpecChecks`]: what `flint check` asks of a spec a scene names.
//!
//! Its own `gen` asks two points of other packages, which the binary
//! registers when it has them: the ai package's steps ([`AiSteps`]) and
//! living's contact sheet ([`StripSheet`]).

// Unit tests read their files off the disk (#761)
#![cfg_attr(test, allow(clippy::disallowed_methods))]

pub mod gen;
mod gen_catalog;
mod gen_preview;
mod gen_preview_common;
mod gen_store;
mod spec_save;
mod tex_edit;

pub use gen::{AiSteps, StripOptions, StripSheet};
pub use gen_preview::{build_mesh_world, PREVIEW_ASSET};
pub use gen_store::{slug, Provenance, WrittenFile};

use flint_package::cli::{CliExtension, Points, SpecChecks, SpecGen, SpecIssue};
use flint_viewer::host::{Editor, SpecEdit, SpecEditors, SpecKind};
use std::path::Path;

/// What the CLI launcher lists for the procgen package
/// (`packages/procgen/package.toml`'s `[rust] cli`, which flint-cli's
/// `build.rs` turns into `flint_procgen_cli::cli::extensions()`, #1554).
pub mod cli {
    use flint_package::cli::CliExtension;

    /// The package's one extension: no command, its points.
    pub fn extensions() -> Vec<Box<dyn CliExtension>> {
        vec![Box::new(super::Procgen)]
    }
}

/// The package's extension: no command of its own, its points.
struct Procgen;

impl CliExtension for Procgen {
    fn commands(&self) -> Vec<clap::Command> {
        Vec::new()
    }

    fn run(&mut self, _matches: &clap::ArgMatches) -> anyhow::Result<()> {
        unreachable!("the procgen package adds no command of its own")
    }

    fn register(&self, points: &mut Points) {
        points
            .insert(SpecGen(gen::run))
            .insert(SpecEditors {
                kind: spec_kind,
                open: open_editor,
            })
            .insert(SpecChecks {
                parse_error,
                param_issues,
                terrain_issues,
            });
    }
}

/// A spec's previewer or graph editor: a spec that draws images is a
/// texture, any other a mesh; a pipeline the router has already told apart.
fn spec_kind(path: &Path) -> anyhow::Result<SpecKind> {
    Ok(if gen_preview::draws_images(path)? {
        SpecKind::Texture
    } else {
        SpecKind::Mesh
    })
}

fn open_editor(kind: SpecKind, edit: SpecEdit) -> anyhow::Result<Box<dyn Editor>> {
    Ok(match kind {
        SpecKind::Mesh | SpecKind::Texture => Box::new(gen_preview::editor(edit)?),
        SpecKind::Graph => Box::new(tex_edit::editor(edit)?),
    })
}

/// A spec the player skips, and so names nobody's asset.
fn parse_error(path: &Path) -> Option<String> {
    flint_procgen::ProcGenSpec::from_file(path)
        .err()
        .map(|e| e.to_string())
}

/// The terrain_v1 schema's issues with a `.terrain.toml`'s text, as `flint
/// gen` prints them; a file that won't parse says so as its one issue.
fn terrain_issues(text: &str) -> Vec<String> {
    match flint_procgen::generators::terrain::check_terrain_toml(text) {
        Ok(issues) => issues
            .iter()
            .map(|i| {
                let level = if i.is_error() { "error" } else { "warning" };
                format!("{level}: {i}")
            })
            .collect(),
        Err(e) => vec![format!("error: {e}")],
    }
}

/// The one validator `flint gen` runs before generating (#718), against
/// the built-ins and `mock`, which every host resolves with.
fn param_issues(path: &Path) -> Option<(String, Vec<SpecIssue>)> {
    let spec = flint_procgen::ProcGenSpec::from_file(path).ok()?;
    let registry = flint_procgen::GeneratorRegistry::with_built_ins();
    let generator = registry.get(&spec.generator)?;
    let issues = flint_procgen::validate_params(&generator.param_schema(), &spec.params)
        .into_iter()
        .map(|issue| SpecIssue {
            error: issue.is_error(),
            key: issue.key().to_string(),
            message: issue.to_string(),
        })
        .collect();
    Some((spec.generator.clone(), issues))
}
