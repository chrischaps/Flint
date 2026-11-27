# AI Asset Generation

> **Package:** `ai`, a feature package: a game has it only when its `[packages]` lists `ai = "*"`, which brings `procgen` with it ([Packages](packages.md)).

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
    /// The price units a request is billed: megapixels, seconds, tasks
    fn billed_units(&self, request: &GenerateRequest) -> f64 { 0.0 }
    /// Finish a job a generate submitted and didn't see through
    fn resume(&self, job: &GenerationJob, output_dir: &Path) -> Result<GenerateResult>;
    fn build_prompt(&self, request: &GenerateRequest, style: Option<&StyleGuide>) -> String;
}
```

A paid provider is made with a `Calls` (`create_provider(name, &config, &calls)`) and sends every request through it: one transport and retry policy, and the run's cost meter, so no provider builds an HTTP client of its own. Tests drive a provider end to end over a `ScriptedTransport`.

The Mock provider generates solid-color PNGs, minimal valid GLB files, and silence WAV files --- useful for testing workflows and CI pipelines without API keys or network access.

## Style Guides

Style guides enforce visual consistency across generated assets. They are TOML files that define a palette, material constraints, geometry constraints, and prompt modifiers:

```toml
# styles/medieval_tavern.styleguide.toml
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

Style guides are searched in `styles/` then `.flint/styles/` by name (e.g., `medieval_tavern` finds `styles/medieval_tavern.styleguide.toml`).

The suffix is `.styleguide.toml`. Before #420 it was `.style.toml`, which is also the suffix of a [UI class file](scripting.md#data-driven-ui-system), so a folder holding both couldn't be told apart by name. A guide still named `<name>.style.toml` loads, but `flint` warns and names the file to rename it to; a later release stops reading the old name. When both names exist, the `.styleguide.toml` is the one loaded.

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
| `strict` | Every reference must be found. A missing one is an error (exit 1). |
| `placeholder` | Missing references are listed. The run still passes. |
| `ai_generate` | Missing models, textures and sounds are generated via AI providers and stored in the catalog. |
| `human_task` | Missing models, textures and sounds get task files for manual creation. |
| `ai_then_human` | Generate with AI first, then produce review tasks for human approval. |

```bash
# Generate all missing assets for a scene using AI
flint asset resolve my_scene.scene.toml --strategy ai_generate --style medieval_tavern

# Create task files for a human artist
flint asset resolve my_scene.scene.toml --strategy human_task --output-dir tasks/
```

### Which references it reads

`resolve` takes its list from the scene's schemas (#968), not from a list of its own. A reference is the value of any field the loaded schemas declare as `type = "asset"`. Use `flint schema <component>` to see a component's fields. Arrays and tables with fields are searched too. The search covers every component of every entity, including the entities a prefab instance expands to (named `<prefix>_<name>`), and every scene table, such as `[environment] skybox`. When a game adds an `asset` field to its own schema layer, `resolve` reads that field with no change to the engine. The schemas are found the way every command finds them: `--schemas` (repeatable), else the project's `flint.toml` `schemas`, else `schemas/` beside the scene. Three things are left out: an empty value, a component or table that no schema declares, and a field whose `unless` sibling is `true` (`model.runtime`). Only what the scene writes is read. An archetype's defaults are not.

The engine's schemas declare these fields as `asset`: `model.asset`, `material.texture`, `sprite.texture`, `audio_source.file`, `script.source`, `collider.model`, `animator.sequence`, `particle_effect.effect`, `particle_emitter.texture`, `terrain.heightmap`, `terrain.splat_map`, `terrain.layer0_texture` to `layer3_texture`, `grass_ground.mask`, `spline.file`, `spline_mesh.texture`, `spline_mesh.normal_texture` and `[environment] skybox`.

A reference counts as found when the project's [asset catalog](assets.md#asset-catalog) holds that name, or when the engine would load it: a file the project's [asset search](assets.md#where-the-engine-looks-for-a-file) finds for one of the field's kinds, or a model or texture in the catalog and store. This is the lookup `flint check` uses. A name that an `asset_def` declares and no file holds yet is missing, marked as not made yet: making it is what `resolve` is for. Each reference prints on its own line, `Found: boat.model.asset -> "boat" (models/boat.glb)` or `Missing: ...` with the paths tried. Under `--json`, the `references` list gives each one's `entity` (`null` for a scene table), `table`, `field`, `name`, `kinds`, `found` and `missing`.

A provider makes only models, textures and sounds. A batch strategy reports any other missing reference (a script, a heightmap, a skybox) as `No provider makes ...` and lists it in the JSON's `no_provider`, and the run exits 1.

## Costs and Budgets

Generation spends money, so every paid call goes through a cost meter (#721), the same one the AI steps of `flint gen` and `flint render --ai-refine` use:

- **`--max-cost <USD>`** on `flint asset generate`, `resolve` and `regenerate` is checked before each call is sent: a call that would take the run's spend past it is refused (`cost ceiling reached ... nothing more was sent`) and never reaches the provider. `resolve` keeps one meter for the whole scene, so an asset over budget is skipped (`over_budget` in its `--json` report; under `ai_then_human` it becomes a task) and a cheaper one after it may still fit. A scene left with an unresolved asset exits 1.
- **Every paid call is in the ledger**, `.flint/ai_costs.jsonl` at the project root: one JSON object per line with the provider, the endpoint, the asset, the operation (`generate`, `resolve`, `regenerate`, or `resume` for a stage a `flint asset job resume` paid for), the units billed (`megapixel`, `second`, `credit`) and `estimated_usd`. A call is recorded the moment the provider accepts it, so a generation that fails afterwards (a lost download) is still there. `flint asset costs [--since YYYY-MM-DD]` and `flint gen --cost-report` print the same report over it.
- **Prices are data**, in `flint-ai-core`'s `prices.toml`, dated and with each rate's source: Flux (fal.ai) per megapixel, each image rounded up; ElevenLabs per second of sound asked for; Meshy per credit, a textured model being two billed requests recorded apart: a preview (20 credits) and a refine of it (10). Meshy publishes no dollar price for an API credit, so its $0.02 a credit ($0.60 a model) is its Pro plan's; set your plan's in **`.flint/prices.toml`**, which overrides any entry:

```toml
# .flint/prices.toml
[calls.meshy]
unit = "credit"
usd = 0.015
```

A Meshy price in any other unit (`unit = "task"`, as Flint once advised) is read as the price of a whole textured model and split 2:1 between the preview and the refine.

With a ceiling set, a provider with no price at all is refused, since its cost can't be kept under one.

### Jobs

A Meshy model takes minutes, and is two tasks: a **preview**, which turns the prompt into a mesh, then a **refine** of that preview, which textures it (Meshy's v2 API takes the prompt only on the preview, and the refine names it by `preview_task_id`). The model is a job in `.flint/jobs/` from the moment Meshy accepts (and bills) the preview; the job names both tasks (`remote_id` the preview, `refine_id` the refine once it is submitted) and is updated as each is polled, its progress the preview's first half and the refine's second. `flint asset job list` shows each one with its status, progress and cost. A run that stops, or a wait that times out (half an hour a task), leaves the job; `flint asset job resume <id>` goes on from where it was: a job stopped in its preview waits for that preview and pays for the refine alone, one stopped in its refine waits for it and pays for nothing new. Either way it downloads the textured model and registers the asset.

A Flux texture is a job too. Its default endpoint, `https://queue.fal.run/fal-ai/flux/dev`, is fal's queue: the POST answers at once with a receipt (`request_id`, `status_url`, `response_url`), and the image comes later. Flint checks `status_url` every second until fal says `COMPLETED`, fetches the output from `response_url`, then downloads the image. The job records `response_url`, so `flint asset job resume <id>` can still fetch an image after the run that queued it has stopped. A request fal finishes with an `error` fails its job. A synchronous endpoint (`api_url = "https://fal.run/fal-ai/flux/dev"` under `[providers.flux]`) answers with the image right away and makes no job. With `RUST_LOG=flint_asset_gen=debug`, fal's answers are logged as they arrive.

## Model Validation

After generating a 3D model, Flint can validate it against a style guide's constraints. The validator imports the GLB file through the same `import_gltf()` pipeline used by the player, then runs the checks `flint gen --validate` runs on procedural output, under the same names (see [Validation](procgen.md#validation) for the full table):

- **mesh integrity** and **degenerate triangles**
- **style triangle budget** against `geometry.max_triangles`, per mesh (per LOD for a `LOD_0`, `LOD_1`, ... file)
- **UV range**: missing UVs warn, or fail if `geometry.require_uvs` is set
- **normals**: zero normals fail; missing ones warn, or fail if `geometry.require_normals` is set
- **PBR roughness / metallic** against `materials.roughness_range` and `materials.metallic_range`
- **palette**: each material's base colour against the style's `palette`

```bash
flint asset validate model.glb --style medieval_tavern
```

Each check reports PASS, WARN, or FAIL; any FAIL exits 1.

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
| `flint asset resolve <scene> --strategy ai_generate --max-cost 1.00` | Batch-generate, refusing any call that would pass $1.00 |
| `flint asset costs [--since DATE]` | What AI calls have cost, from `.flint/ai_costs.jsonl` |
| `flint asset job status <id>` | Check status of a generation job (a Meshy model) |
| `flint asset job list` | List all generation jobs |
| `flint asset job resume <id>` | Finish a job a run didn't see through, paying nothing new |

## Runtime Catalog Integration

The player reads the project root's asset catalog for runtime asset resolution, the same one `flint validate` and `flint check` ask. When an entity references an asset by name, the resolution chain is:

1. Look up the name in the `AssetCatalog` (`<root>/assets`)
2. If found, resolve the content hash
3. Load from the `ContentStore` path (`<root>/.flint/assets/<hash>`)
4. Fall back to file-based loading if not in the catalog

This means scenes can seamlessly reference both pre-imported and AI-generated assets by name, without hardcoding file paths.

## Further Reading

- [Assets](assets.md) --- content-addressed storage and catalog system
- [File Formats](../formats/overview.md) --- `.styleguide.toml` and `asset_def.toml` format reference
- [CLI Reference](../cli-reference/overview.md) --- full command documentation
- [AI Agent Workflow](../guides/ai-agent-workflow.md) --- using AI generation in automated workflows
