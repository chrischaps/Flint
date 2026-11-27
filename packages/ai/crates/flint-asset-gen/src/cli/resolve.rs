//! `flint asset resolve --strategy ai_generate | human_task |
//! ai_then_human` (#1572): the kernel's `resolve` finds a scene's asset
//! references and hands the missing ones here, through the
//! [`BatchResolve`] point this package registers. Once flint-cli's
//! `asset.rs` under `#[cfg(feature = "ai")]`.

use crate::batch::{BatchStrategy, MissingAsset, SceneNeeds};
use crate::provider::AssetKind as Made;
use crate::{FlintConfig, StyleGuide};
use anyhow::Result;
use flint_asset::{Project, ProjectStore};
use flint_core::project::AssetKind as Kind;
use flint_package::cli::asset::{Batch, BatchResolve};
use flint_package::cli::{self as output, progress, say};
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;

/// The strategies this package serves, as the manifest spells their lines.
pub const SERVED: &[&str] = &[
    "asset resolve --strategy ai_generate",
    "asset resolve --strategy human_task",
    "asset resolve --strategy ai_then_human",
];

/// The point this package registers.
pub fn point() -> BatchResolve {
    BatchResolve(resolve)
}

/// The missing references handed to a provider or a person.
fn resolve(batch: Batch) -> Result<()> {
    let strategy = match batch.strategy {
        "ai_generate" => BatchStrategy::AiGenerate,
        "human_task" => BatchStrategy::HumanTask,
        "ai_then_human" => BatchStrategy::AiThenHuman,
        other => anyhow::bail!("`{other}` is no batch strategy of the ai package's"),
    };
    let config = FlintConfig::load().unwrap_or_else(|_| FlintConfig {
        providers: HashMap::new(),
        generation: Default::default(),
    });

    let style = match batch.style.or_else(|| config.default_style()) {
        Some(s) => StyleGuide::find(s).ok(),
        None => None,
    };

    // The scene's project, as the kernel's command found the references
    let project = Project::for_path(Path::new(batch.scene))?;
    let generated = project.generated_dir().display().to_string();
    let out = batch.output_dir.unwrap_or(&generated);
    // One meter for the whole scene: the scene's project's ledger and
    // prices, and the run's ceiling.
    let calls = super::asset::project_calls(&project, batch.max_cost)?;

    // What a provider makes is a model, a texture or a sound; a missing
    // script or heightmap is reported, and left unresolved
    let mut needs = SceneNeeds::default();
    let mut no_provider = Vec::new();
    for missing in &batch.missing {
        match generated_kind(&missing.kinds) {
            Some(kind) => needs.missing.push(MissingAsset {
                entity_name: missing.place.clone(),
                field: missing.field.clone(),
                asset_name: missing.name.clone(),
                kind,
            }),
            None => no_provider.push(missing),
        }
    }
    needs.total = batch.found + needs.missing.len();
    for missing in &no_provider {
        say(format!(
            "  No provider makes {} {}: left missing",
            kinds_label(&missing.kinds),
            missing.field_of
        ));
    }

    let result = crate::batch::resolve_missing(
        &project,
        &needs,
        strategy,
        style.as_ref(),
        &config,
        out,
        &calls,
    )
    .map_err(|e| anyhow::anyhow!("{}", e))?;
    progress(super::costs::spent_line(&calls.meter));

    output::when_json(|| {
        json!({
            "scene": batch.scene,
            "strategy": batch.strategy,
            "references": batch.references,
            "found": result.already_found,
            "generated": result.generated,
            "tasks_created": result.tasks_created,
            "failed": result.failed,
            "over_budget": result.over_budget,
            "no_provider": no_provider.iter().map(|m| m.json.clone()).collect::<Vec<_>>(),
            "cost_usd": result.spent_usd,
            "max_cost": batch.max_cost,
        })
    });
    // A reference neither generated nor handed to a person is a failure
    let unresolved =
        result.total_refs - result.already_found - result.generated - result.tasks_created;
    if result.failed > 0 || unresolved > 0 || !no_provider.is_empty() {
        return Err(output::failed());
    }
    Ok(())
}

/// What a provider can make for a field of `kinds`: a model, a texture or
/// a sound, the first the field takes.
fn generated_kind(kinds: &[Kind]) -> Option<Made> {
    kinds.iter().find_map(|kind| match kind {
        Kind::Models => Some(Made::Model),
        Kind::Textures | Kind::ParticleTextures | Kind::Sprites => Some(Made::Texture),
        Kind::Audio => Some(Made::Audio),
        _ => None,
    })
}

/// `files`, or `models or specs`.
fn kinds_label(kinds: &[Kind]) -> String {
    let labels: Vec<&str> = kinds.iter().map(|k| k.label()).collect();
    labels.join(" or ")
}
