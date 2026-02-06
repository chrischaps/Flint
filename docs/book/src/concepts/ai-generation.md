# AI Asset Generation

Flint includes an integrated AI asset generation pipeline through the `flint-asset-gen` crate. The system connects to external AI services to produce textures, 3D models, and audio from text descriptions, while maintaining visual consistency through style guides and validating results against constraints.

## Overview

The pipeline follows a request-enrich-generate-validate-catalog flow:

```
Description + Style Guide
        │
        ▼
  Prompt Enrichment (palette, materials, constraints)
        │
        ▼
  GenerationProvider (Flux / Meshy / ElevenLabs / Mock)
        │
        ▼
  Post-generation Validation (geometry, materials)
        │
        ▼
  Content-Addressed Storage + Asset Catalog
```

## Providers

Flint uses a pluggable `GenerationProvider` trait. Each provider handles one or more asset types:

| Provider | Asset Types | Service | Description |
|----------|-------------|---------|-------------|
| **Flux** | Textures | Flux API | AI image generation for PBR textures |
| **Meshy** | 3D Models | Meshy API | Text-to-3D model generation (GLB output) |
| **ElevenLabs** | Audio | ElevenLabs API | AI sound effect and voice generation |
| **Mock** | All | Local | Generates minimal valid files for testing without network access |

The `GenerationProvider` trait defines the interface:

```rust
pub trait GenerationProvider: Send {
    fn name(&self) -> &str;
    fn supported_kinds(&self) -> Vec<AssetKind>;
    fn health_check(&self) -> Result<ProviderStatus>;
    fn generate(&self, request: &GenerateRequest, style: Option<&StyleGuide>, output_dir: &Path) -> Result<GenerateResult>;
    fn submit_job(&self, request: &GenerateRequest, style: Option<&StyleGuide>) -> Result<GenerationJob>;
    fn poll_job(&self, job: &GenerationJob) -> Result<JobPollResult>;
    fn download_result(&self, job: &GenerationJob, output_dir: &Path) -> Result<GenerateResult>;
    fn build_prompt(&self, request: &GenerateRequest, style: Option<&StyleGuide>) -> String;
}
```

The Mock provider generates solid-color PNGs, minimal valid GLB files, and silence WAV files --- useful for testing workflows and CI pipelines without API keys or network access.

## Style Guides

Style guides enforce visual consistency across generated assets. They are TOML files that define a palette, material constraints, geometry constraints, and prompt modifiers:

```toml
# styles/medieval_tavern.style.toml
[style]
name = "medieval_tavern"
description = "Weathered medieval fantasy tavern"
prompt_prefix = "Medieval fantasy tavern style, low-fantasy realism"
prompt_suffix = "Photorealistic textures, warm candlelight tones"
negative_prompt = "modern, sci-fi, neon, plastic, chrome"
palette = ["#8B4513", "#A0522D", "#D4A574", "#4A4A4A", "#2F1B0E"]

[style.materials]
roughness_range = [0.6, 0.95]
metallic_range = [0.0, 0.15]
preferred_materials = ["aged oak wood", "rough-hewn stone", "hammered wrought iron"]

[style.geometry]
max_triangles = 5000
require_uvs = true
require_normals = true
```

When a style guide is active, the provider enriches the generation prompt by prepending the `prompt_prefix`, appending palette colors and material descriptors, and adding the `prompt_suffix`. The `negative_prompt` tells AI services what to avoid.

Style guides are searched in `styles/` then `.flint/styles/` by name (e.g., `medieval_tavern` finds `styles/medieval_tavern.style.toml`).

## Semantic Asset Definitions

The `asset_def` component describes what an entity *needs* in terms of assets, expressed as intent rather than file paths:

```toml
[entities.tavern_wall]
archetype = "wall"

[entities.tavern_wall.asset_def]
name = "tavern_wall_texture"
description = "Rough stone wall with mortar lines, medieval tavern interior"
type = "texture"
material_intent = "rough stone"
wear_level = 0.7
tags = ["wall", "interior", "medieval"]
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Asset name identifier |
| `description` | string | What this asset is for (used as the generation prompt) |
| `type` | string | Asset type: `texture`, `model`, or `audio` |
| `material_intent` | string | Material intent (e.g., "aged wood", "rough stone") |
| `wear_level` | f32 | How worn/damaged (0.0 = pristine, 1.0 = heavily worn) |
| `size_class` | string | Size class: `small`, `medium`, `large`, `huge` |
| `tags` | array | Tags for categorization |

These definitions let the batch resolver automatically generate all assets a scene needs.

## Batch Resolution

The `flint asset resolve` command can resolve an entire scene's asset needs at once using different strategies:

| Strategy | Behavior |
|----------|----------|
| `strict` | All assets must already exist in the catalog. Missing assets are errors. |
| `placeholder` | Missing assets get placeholder geometry. |
| `ai_generate` | Missing assets are generated via AI providers and stored in the catalog. |
| `human_task` | Missing assets produce task files for manual creation. |
| `ai_then_human` | Generate with AI first, then produce review tasks for human approval. |

```bash
# Generate all missing assets for a scene using AI
flint asset resolve my_scene.scene.toml --strategy ai_generate --style medieval_tavern

# Create task files for a human artist
flint asset resolve my_scene.scene.toml --strategy human_task --output-dir tasks/
```

## Model Validation

After generating a 3D model, Flint can validate it against a style guide's constraints. The validator imports the GLB file through the same `import_gltf()` pipeline used by the player, then checks:

- **Triangle count** against `geometry.max_triangles`
- **UV coordinates** present if `geometry.require_uvs` is set
- **Normals** present if `geometry.require_normals` is set
- **Material properties** against `materials.roughness_range` and `materials.metallic_range`

```bash
flint asset validate model.glb --style medieval_tavern
```

Each check reports Pass, Warn, or Fail status.

## Build Manifests

Build manifests track the provenance of all generated assets in a project. They record which provider generated each asset, what prompt was used, and the content hash:

```bash
flint asset manifest --assets-dir assets --output build/manifest.toml
```

The manifest scans `.asset.toml` sidecar files for `provider` properties to identify which assets were AI-generated vs. manually created. This is useful for auditing, reproducing builds, and tracking which assets need regeneration when style guides change.

## Configuration

Flint uses a layered configuration system for API keys and provider settings:

**Global config** (`~/.flint/config.toml`):
```toml
[providers.flux]
api_key = "your-flux-key"
enabled = true

[providers.meshy]
api_key = "your-meshy-key"
enabled = true

[providers.elevenlabs]
api_key = "your-elevenlabs-key"
enabled = true

[generation]
default_style = "medieval_tavern"
```

**Project config** (`.flint/config.toml`): overrides global settings for this project.

**Environment variables**: override both config files:
- `FLINT_FLUX_API_KEY`
- `FLINT_MESHY_API_KEY`
- `FLINT_ELEVENLABS_API_KEY`

The layering order is: global config < project config < environment variables.

## CLI Commands

| Command | Description |
|---------|-------------|
| `flint asset generate <type> -d "<prompt>"` | Generate a single asset |
| `flint asset generate texture -d "stone wall" --style medieval_tavern` | Generate with style guide |
| `flint asset generate model -d "wooden chair" --provider meshy` | Generate with specific provider |
| `flint asset resolve <scene> --strategy ai_generate` | Batch-generate all missing scene assets |
| `flint asset validate <file> --style <name>` | Validate a model against style constraints |
| `flint asset manifest` | Generate a build manifest of all generated assets |
| `flint asset regenerate <name> --seed 42` | Regenerate an existing asset with a new seed |
| `flint asset job status <id>` | Check status of an async generation job |
| `flint asset job list` | List all generation jobs |

## Runtime Catalog Integration

The player can optionally load the asset catalog at startup for runtime asset resolution. When an entity references an asset by name, the resolution chain is:

1. Look up the name in the `AssetCatalog`
2. If found, resolve the content hash
3. Load from the `ContentStore` path (`.flint/assets/<hash>`)
4. Fall back to file-based loading if not in the catalog

This means scenes can seamlessly reference both pre-imported and AI-generated assets by name, without hardcoding file paths.

## Further Reading

- [Assets](assets.md) --- content-addressed storage and catalog system
- [File Formats](../formats/overview.md) --- `.style.toml` and `asset_def.toml` format reference
- [CLI Reference](../cli-reference/overview.md) --- full command documentation
- [AI Agent Workflow](../guides/ai-agent-workflow.md) --- using AI generation in automated workflows
