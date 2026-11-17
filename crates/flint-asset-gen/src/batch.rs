//! Batch scene resolution
//!
//! Scans a scene TOML for asset references and resolves missing ones
//! using various strategies: AI generation, human tasks, or a combination.

use crate::calls::Calls;
use crate::config::FlintConfig;
use crate::human_task::generate_task_file;
use crate::provider::{AssetKind, AudioParams, GenerateRequest, ModelParams, TextureParams};
use crate::registration::register_generated_asset_in;
use crate::style::StyleGuide;
use flint_asset::{Project, ProjectStore};
use flint_core::{FlintError, Result};
use std::path::Path;

/// Strategy for batch resolution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchStrategy {
    /// Generate all missing assets via AI
    AiGenerate,
    /// Create human task files for all missing assets
    HumanTask,
    /// Try AI first, fall back to human tasks on failure
    AiThenHuman,
}

/// A missing asset reference found during scene scanning
#[derive(Debug, Clone)]
pub struct MissingAsset {
    pub entity_name: String,
    pub field: String,
    pub asset_name: String,
    pub kind: AssetKind,
}

/// Result of a batch resolution operation
#[derive(Debug)]
pub struct BatchResult {
    pub total_refs: usize,
    pub already_found: usize,
    pub generated: usize,
    pub tasks_created: usize,
    pub failed: usize,
    /// Assets whose call the cost ceiling refused before it was sent
    pub over_budget: usize,
    /// What the run's paid calls cost
    pub spent_usd: f64,
}

/// Scan a scene TOML for asset references and find missing ones
pub fn scan_scene_assets(scene_path: &str) -> Result<(usize, Vec<MissingAsset>)> {
    let content = std::fs::read_to_string(scene_path)?;
    let scene: toml::Value = toml::from_str(&content)
        .map_err(|e| FlintError::GenerationError(format!("Failed to parse scene: {}", e)))?;

    // The scene's own project, not the working directory's (#179).
    let project = Project::for_path(Path::new(scene_path))?;
    let catalog =
        flint_asset::AssetCatalog::load_from_directory(project.catalog_dir()).unwrap_or_default();

    let mut total = 0;
    let mut missing = Vec::new();

    if let Some(entities) = scene.get("entities").and_then(|e| e.as_table()) {
        for (entity_name, entity_data) in entities {
            if let Some(table) = entity_data.as_table() {
                // Check top-level asset fields
                check_asset_field(
                    table,
                    entity_name,
                    "mesh",
                    AssetKind::Model,
                    &catalog,
                    &mut total,
                    &mut missing,
                );

                // Check nested component fields
                if let Some(model) = table.get("model").and_then(|v| v.as_table()) {
                    if let Some(asset_name) = model.get("asset").and_then(|v| v.as_str()) {
                        total += 1;
                        if catalog.get(asset_name).is_none() {
                            missing.push(MissingAsset {
                                entity_name: entity_name.clone(),
                                field: "model.asset".to_string(),
                                asset_name: asset_name.to_string(),
                                kind: AssetKind::Model,
                            });
                        }
                    }
                }

                if let Some(material) = table.get("material").and_then(|v| v.as_table()) {
                    if let Some(tex) = material.get("texture").and_then(|v| v.as_str()) {
                        total += 1;
                        if catalog.get(tex).is_none() {
                            missing.push(MissingAsset {
                                entity_name: entity_name.clone(),
                                field: "material.texture".to_string(),
                                asset_name: tex.to_string(),
                                kind: AssetKind::Texture,
                            });
                        }
                    }
                }

                if let Some(audio) = table.get("audio_source").and_then(|v| v.as_table()) {
                    if let Some(file) = audio.get("file").and_then(|v| v.as_str()) {
                        total += 1;
                        if catalog.get(file).is_none() {
                            missing.push(MissingAsset {
                                entity_name: entity_name.clone(),
                                field: "audio_source.file".to_string(),
                                asset_name: file.to_string(),
                                kind: AssetKind::Audio,
                            });
                        }
                    }
                }
            }
        }
    }

    Ok((total, missing))
}

fn check_asset_field(
    table: &toml::map::Map<String, toml::Value>,
    entity_name: &str,
    field: &str,
    kind: AssetKind,
    catalog: &flint_asset::AssetCatalog,
    total: &mut usize,
    missing: &mut Vec<MissingAsset>,
) {
    if let Some(value) = table.get(field) {
        if let Some(name) = value.as_str() {
            *total += 1;
            if catalog.get(name).is_none() {
                missing.push(MissingAsset {
                    entity_name: entity_name.to_string(),
                    field: field.to_string(),
                    asset_name: name.to_string(),
                    kind,
                });
            }
        }
    }
}

/// Resolve missing assets in a scene using the specified strategy. The
/// progress goes to stderr; the [`BatchResult`] is the answer.
///
/// Every provider call goes through `calls`, so one meter sees the whole
/// scene: an asset whose call would take the run past the meter's ceiling
/// is refused before anything is sent (counted in `over_budget`, and handed
/// to a person under [`BatchStrategy::AiThenHuman`]), and the next, cheaper
/// one may still fit.
pub fn resolve_scene(
    scene_path: &str,
    strategy: BatchStrategy,
    style: Option<&StyleGuide>,
    config: &FlintConfig,
    output_dir: &str,
    calls: &Calls,
) -> Result<BatchResult> {
    let (total, missing_assets) = scan_scene_assets(scene_path)?;
    let found = total - missing_assets.len();
    let project = Project::for_path(Path::new(scene_path))?;

    eprintln!("Scanning scene for asset references...");
    eprintln!(
        "Found {} references, {} missing:",
        total,
        missing_assets.len()
    );

    let mut generated = 0;
    let mut tasks_created = 0;
    let mut failed = 0;
    let mut over_budget = 0;
    let spent_before = calls.meter.spent();

    for asset in &missing_assets {
        // Printed whole after the outcome, so a notice from the meter during
        // the call never lands inside the line.
        let head = format!("  {} ({})  -> ", asset.asset_name, asset.kind);

        match strategy {
            BatchStrategy::AiGenerate => {
                match generate_asset(&project, asset, style, config, output_dir, calls) {
                    Ok(secs) => {
                        eprintln!(
                            "{head}Generated via {} ({:.1}s)",
                            config.default_provider(asset.kind),
                            secs
                        );
                        generated += 1;
                    }
                    Err(Unresolved::OverBudget(e)) => {
                        eprintln!("{head}NOT SENT: {}", e);
                        over_budget += 1;
                    }
                    Err(Unresolved::Failed(e)) => {
                        eprintln!("{head}FAILED: {}", e);
                        failed += 1;
                    }
                }
            }
            BatchStrategy::HumanTask => match create_human_task(asset, style, output_dir) {
                Ok(path) => {
                    eprintln!("{head}Task created: {}", path.display());
                    tasks_created += 1;
                }
                Err(e) => {
                    eprintln!("{head}FAILED: {}", e);
                    failed += 1;
                }
            },
            BatchStrategy::AiThenHuman => {
                match generate_asset(&project, asset, style, config, output_dir, calls) {
                    Ok(secs) => {
                        eprintln!("{head}Generated ({:.1}s)", secs);
                        generated += 1;
                    }
                    Err(why) => match create_human_task(asset, style, output_dir) {
                        Ok(path) => {
                            let what = match why {
                                Unresolved::OverBudget(_) => {
                                    over_budget += 1;
                                    "over budget"
                                }
                                Unresolved::Failed(_) => "AI failed",
                            };
                            eprintln!("{head}{what}, task created: {}", path.display());
                            tasks_created += 1;
                        }
                        Err(e) => {
                            eprintln!("{head}FAILED: {}", e);
                            failed += 1;
                        }
                    },
                }
            }
        }
    }

    let spent_usd = calls.meter.spent() - spent_before;
    eprintln!(
        "\nResolution: {}/{} resolved, {} generated, {} tasks, {} failed, {} over budget ({} spent)",
        found + generated + tasks_created,
        total,
        generated,
        tasks_created,
        failed,
        over_budget,
        flint_ai_core::cost::usd(spent_usd)
    );

    Ok(BatchResult {
        total_refs: total,
        already_found: found,
        generated,
        tasks_created,
        failed,
        over_budget,
        spent_usd,
    })
}

/// Why an asset was not generated.
enum Unresolved {
    /// The meter refused its call before it was sent.
    OverBudget(String),
    /// The provider could not be made, or the call failed.
    Failed(FlintError),
}

fn generate_asset(
    project: &Project,
    asset: &MissingAsset,
    style: Option<&StyleGuide>,
    config: &FlintConfig,
    output_dir: &str,
    calls: &Calls,
) -> std::result::Result<f64, Unresolved> {
    let provider_name = config.default_provider(asset.kind);
    let provider = crate::providers::create_provider(provider_name, config, calls)
        .map_err(Unresolved::Failed)?;

    let description = format!(
        "Create {} asset for entity '{}'",
        asset.kind, asset.entity_name
    );

    let request = GenerateRequest {
        name: asset.asset_name.clone(),
        description,
        kind: asset.kind,
        texture_params: if asset.kind == AssetKind::Texture {
            Some(TextureParams::default())
        } else {
            None
        },
        model_params: if asset.kind == AssetKind::Model {
            Some(ModelParams::default())
        } else {
            None
        },
        audio_params: if asset.kind == AssetKind::Audio {
            Some(AudioParams::default())
        } else {
            None
        },
        tags: vec![],
    };

    // Asked here as well as in the provider, so a refusal is told apart
    // from a failure; nothing has been sent yet.
    calls
        .meter
        .admit_call(provider.name(), provider.billed_units(&request))
        .map_err(|e| Unresolved::OverBudget(e.to_string()))?;
    calls.meter.set_context(&asset.asset_name, "resolve");
    let result = provider
        .generate(&request, style, Path::new(output_dir))
        .map_err(Unresolved::Failed)?;
    register_generated_asset_in(project, &request, &result).map_err(Unresolved::Failed)?;
    Ok(result.duration_secs)
}

fn create_human_task(
    asset: &MissingAsset,
    style: Option<&StyleGuide>,
    output_dir: &str,
) -> std::result::Result<std::path::PathBuf, FlintError> {
    let description = format!(
        "Create {} asset for entity '{}'",
        asset.kind, asset.entity_name
    );
    generate_task_file(
        &asset.asset_name,
        asset.kind,
        &description,
        style,
        Path::new(output_dir),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("flint_batch_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_test_scene(dir: &std::path::Path) -> std::path::PathBuf {
        let scene = r#"
[scene]
name = "test"

[entities.chair]
archetype = "prop"

[entities.chair.model]
asset = "tavern_chair"

[entities.wall]
archetype = "wall"

[entities.wall.material]
texture = "brick_wall"

[entities.ambient]
archetype = "audio"

[entities.ambient.audio_source]
file = "tavern_ambient"
"#;
        let path = dir.join("test.scene.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(scene.as_bytes()).unwrap();
        path
    }

    #[test]
    fn test_scan_scene_assets() {
        let dir = temp_dir();
        let scene_path = write_test_scene(&dir);

        let (total, missing) = scan_scene_assets(scene_path.to_str().unwrap()).unwrap();
        assert_eq!(total, 3);
        assert_eq!(missing.len(), 3); // None in catalog

        let kinds: Vec<AssetKind> = missing.iter().map(|m| m.kind).collect();
        assert!(kinds.contains(&AssetKind::Model));
        assert!(kinds.contains(&AssetKind::Texture));
        assert!(kinds.contains(&AssetKind::Audio));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_resolve_scene_human_task() {
        let dir = temp_dir();
        let scene_path = write_test_scene(&dir);
        let tasks_dir = dir.join("tasks");

        let config = FlintConfig {
            providers: std::collections::HashMap::new(),
            generation: Default::default(),
        };

        let result = resolve_scene(
            scene_path.to_str().unwrap(),
            BatchStrategy::HumanTask,
            None,
            &config,
            tasks_dir.to_str().unwrap(),
            &Calls::offline(),
        )
        .unwrap();

        assert_eq!(result.total_refs, 3);
        assert_eq!(result.tasks_created, 3);
        assert_eq!(result.failed, 0);

        // Verify task files were created
        assert!(tasks_dir.join("tavern_chair.task.toml").exists());
        assert!(tasks_dir.join("brick_wall.task.toml").exists());
        assert!(tasks_dir.join("tavern_ambient.task.toml").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_resolve_scene_ai_then_human_fallback() {
        let dir = temp_dir();
        let scene_path = write_test_scene(&dir);
        let out_dir = dir.join("output");

        // Config with no API keys — AI will fail, should fall back to tasks
        let config = FlintConfig {
            providers: std::collections::HashMap::new(),
            generation: Default::default(),
        };

        let result = resolve_scene(
            scene_path.to_str().unwrap(),
            BatchStrategy::AiThenHuman,
            None,
            &config,
            out_dir.to_str().unwrap(),
            &Calls::offline(),
        )
        .unwrap();

        // AI fails (no API keys for real providers), falls back to human tasks
        assert_eq!(result.total_refs, 3);
        assert_eq!(result.tasks_created, 3);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// #721's done-when: a ceiling refuses the call that would pass it,
    /// before it is sent, and every call that was sent is recorded.
    #[test]
    fn a_ceiling_refuses_the_call_that_would_pass_it() {
        use crate::providers::test_support::{calls_over, test_config};
        use flint_ai_core::{HttpResponse, ScriptedTransport};

        let dir = temp_dir();
        std::fs::write(dir.join("flint.toml"), "[project]\nid = \"budget\"\n").unwrap();
        let mut scene = String::from("[scene]\nname = \"walls\"\n");
        for wall in ["wall_a", "wall_b", "wall_c"] {
            scene.push_str(&format!(
                "\n[entities.{wall}]\n\n[entities.{wall}.material]\ntexture = \"{wall}_tex\"\n"
            ));
        }
        let scene_path = dir.join("walls.scene.toml");
        std::fs::write(&scene_path, scene).unwrap();

        // Each 1024 x 1024 texture is 2 megapixels, $0.05; $0.12 pays for two.
        let mut answers = Vec::new();
        for _ in 0..2 {
            answers.push(ScriptedTransport::json(
                serde_json::json!({"images": [{"url": "http://fal.test/i.png"}]}),
            ));
            answers.push(Ok(HttpResponse::bytes(b"\x89PNG".to_vec())));
        }
        let script = ScriptedTransport::new(answers);
        let (calls, meter) = calls_over(&script, Some(0.12));
        let result = resolve_scene(
            scene_path.to_str().unwrap(),
            BatchStrategy::AiGenerate,
            None,
            &test_config("flux", "http://fal.test/fal-ai/flux/dev"),
            dir.join("out").to_str().unwrap(),
            &calls,
        )
        .unwrap();

        assert_eq!(result.generated, 2);
        assert_eq!(result.over_budget, 1);
        assert_eq!(result.failed, 0);
        assert!((result.spent_usd - 0.10).abs() < 1e-12);
        // Two submits and two downloads; the third submit never went out.
        assert_eq!(script.requests().len(), 4);
        assert_eq!(meter.refused(), 1);
        let names: Vec<_> = meter.entries().into_iter().map(|e| e.spec_name).collect();
        assert_eq!(names, ["wall_a_tex", "wall_b_tex"]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
