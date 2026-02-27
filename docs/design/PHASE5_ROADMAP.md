# Phase 5: AI Asset Pipeline

Phase 5 adds AI-powered asset generation to Flint, completing the "AI-agent-optimized" thesis. A new `flint-asset-gen` crate provides a pluggable provider framework with three concrete integrations.

## Stages

### Stage 1: Provider Framework & Configuration
- [x] `flint-asset-gen` crate — provider trait, request/result types, asset kinds (texture, model, audio)
- [x] `FlintConfig` — layered config loading: `~/.flint/config.toml` < `.flint/config.toml` < env vars (`FLINT_{PROVIDER}_API_KEY`)
- [x] `GenerationJob` / `JobStore` — file-based job tracking in `.flint/jobs/*.job.toml`
- [x] `StyleGuide` — TOML-defined style guides with palette, material, and geometry constraints; prompt enrichment
- [x] `MockProvider` — generates solid-color PNGs, unit-cube GLBs, silence WAV without network
- [x] Provider registry — `create_provider(name, config) -> Box<dyn GenerationProvider>`
- [x] `medieval_tavern.style.toml` — first style guide with color palette and material/geometry constraints
- [x] `GenerationError` variant added to `FlintError`
- [x] 21 unit tests — config loading, env var override, job serialization, style enrichment, mock generation

### Stage 2: Texture Generation (Flux)
- [x] `FluxProvider` — POST to fal.ai Flux API, download result PNG
- [x] CLI `Generate` subcommand — `flint asset generate texture --description "..." --name NAME --provider flux --style medieval_tavern`
- [x] CLI `Job` subcommands — `flint asset job status ID`, `flint asset job list`
- [x] Generate flow: config → style → provider → prompt enrichment → API → download → content store → sidecar
- [x] `flint-asset-gen` added to `flint-cli` dependencies
- [x] 24 unit tests — Flux response parsing, prompt building, CLI argument handling

### Stage 3: 3D Model Generation (Meshy)
- [x] `MeshyProvider` — POST to Meshy v2 text-to-3d API, poll status with progress %, download GLB
- [x] `validate.rs` — model validation against style constraints (triangle count, UVs, normals, material ranges)
- [x] CLI `Validate` subcommand — `flint asset validate model.glb --style medieval_tavern`
- [x] Post-generation validation in `run_generate` for models
- [x] `flint-import` dependency added for GLB import during validation
- [x] 31 unit tests — Meshy response parsing, model validation, GLB import integration

### Stage 4: Audio Generation & Human Fallback
- [x] `ElevenLabsProvider` — POST to ElevenLabs sound generation API, save WAV/OGG
- [x] `human_task.rs` — generate `.task.toml` files with structured specs for human artists
- [x] `batch.rs` — `scan_scene_assets()` + `resolve_scene()` with strategies: AiGenerate, HumanTask, AiThenHuman
- [x] Extended CLI `Resolve` — `--strategy ai_generate|human_task|ai_then_human`, `--style`, `--output-dir`
- [x] 39 unit tests — ElevenLabs parsing, human task generation, batch scanning, fallback strategies

### Stage 5: Runtime Catalog Integration, Manifests & Demo
- [x] Runtime catalog integration — `AssetCatalog` + `ContentStore` in `PlayerApp`; catalog lookup → content store → file fallback
- [x] `manifest.rs` — `BuildManifest` tracking generated assets with provenance (provider, prompt, seed, hash, time)
- [x] `semantic.rs` — `SemanticAssetDef` intent-based definitions → `GenerateRequest` conversion
- [x] CLI `Manifest` subcommand — `flint asset manifest --output build/manifest.toml`
- [x] CLI `Regenerate` subcommand — `flint asset regenerate NAME --seed 42 --provider flux`
- [x] `asset_def.toml` component schema — semantic asset definition fields
- [x] `phase5_ai_demo.scene.toml` — demo scene with semantic asset definitions
- [x] 45 unit tests — manifest round-trip, semantic def conversion, full pipeline integration

## Architecture

```
flint-cli
  └── flint-asset-gen (NEW — crate #18)
        ├── flint-core       (errors, ContentHash)
        ├── flint-asset      (AssetCatalog, ContentStore, AssetMeta, AssetType)
        ├── flint-import     (import_gltf for model validation)
        ├── ureq             (HTTP client)
        ├── dirs             (home directory for global config)
        ├── image            (MockProvider image generation)
        ├── serde + serde_json + toml  (serialization)
        ├── uuid             (job IDs)
        └── sha2             (content hashing)

flint-player
  └── flint-asset            (AssetCatalog + ContentStore for runtime resolution)
```

## New CLI Commands

```bash
# Generate assets
flint asset generate texture --description "weathered red brick" --name brick_wall --provider flux --style medieval_tavern
flint asset generate model --description "sturdy wooden chair" --name tavern_chair --provider meshy
flint asset generate audio --description "heavy door creaking" --name door_creak --provider elevenlabs

# Validate generated models
flint asset validate model.glb --style medieval_tavern

# Batch resolve a scene
flint asset resolve demo/phase5_ai_demo.scene.toml --strategy ai_generate --style medieval_tavern
flint asset resolve demo/scene.toml --strategy human_task --output-dir tasks/
flint asset resolve demo/scene.toml --strategy ai_then_human

# Build manifest
flint asset manifest --output build/manifest.toml

# Regenerate with new seed
flint asset regenerate brick_wall --seed 99

# Job management
flint asset job status abc-123
flint asset job list
```

## Test Summary

- 45 new unit tests in `flint-asset-gen`
- 217 total tests across all 18 crates (all passing)
- No new clippy warnings
