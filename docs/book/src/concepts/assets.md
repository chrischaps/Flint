# Assets

Flint uses a content-addressed asset system with SHA-256 hashing. Every imported file is identified by its content hash, which means identical files are automatically deduplicated and any change to a file produces a new, distinct hash.

## Content Addressing

When you import a file, Flint computes its SHA-256 hash and stores it under a content-addressed path:

```
.flint/assets/<first-2-hex>/<full-hash>.<ext>
```

This means:
- **Deduplication** --- importing the same file twice stores it only once
- **Change detection** --- if a source file changes, its hash changes, and the new version is stored separately
- **Integrity** --- the hash verifies the file hasn't been corrupted

## Asset Catalog

The asset catalog is a searchable index of all imported assets. Each entry tracks:

- **Name** --- a human-friendly identifier (e.g., `tavern_chair`)
- **Hash** --- the SHA-256 content hash
- **Type** --- asset type (`mesh`, `texture`, `material`, etc.)
- **Tags** --- arbitrary labels for organization and filtering
- **Source path** --- where the file was originally imported from

## Importing Assets

Use the CLI to import files into the asset store:

```bash
# Import a glTF model with name and tags
flint asset import models/chair.glb --name tavern_chair --tags furniture,medieval

# Browse the catalog
flint asset list --type mesh

# Check asset references in a scene
flint asset resolve levels/tavern.scene.toml --strategy strict
```

## glTF/GLB Import

The `flint-import` crate provides full glTF/GLB support, extracting:

- **Meshes** --- vertex positions, normals, texture coordinates, and indices
- **Materials** --- PBR properties (base color, roughness, metallic, emissive)
- **Textures** --- embedded or referenced image files

Imported meshes are rendered by `flint-render` with full PBR shading.

## Resolution Strategies

When a scene references assets, Flint can resolve them using different strategies:

| Strategy | Behavior |
|----------|----------|
| `strict` | All referenced assets must exist in the catalog. Missing assets are errors. |
| `placeholder` | Missing assets are replaced with placeholder geometry. Useful during development. |
| `ai_generate` | Missing assets are generated via AI providers (Flux, Meshy, ElevenLabs) and stored. |
| `human_task` | Missing assets produce task files for manual creation by an artist. |
| `ai_then_human` | Generate with AI first, then produce review tasks for human approval. |

The `ai_generate`, `human_task`, and `ai_then_human` strategies are part of the [AI Asset Generation](ai-generation.md) pipeline.

## Asset Sidecar Files

Each asset in the catalog has a `.asset.toml` sidecar file storing metadata:

```toml
[asset]
name = "tavern_chair"
type = "mesh"
hash = "sha256:a1b2c3..."
source_path = "models/chair.glb"
tags = ["furniture", "medieval"]
```

## Runtime Catalog Resolution

The player can load the asset catalog at startup for name-based asset resolution. When an entity references an asset by name, the resolution chain is:

1. Look up the name in the `AssetCatalog`
2. If found, resolve the content hash
3. Load from the `ContentStore` path (`.flint/assets/<hash>`)
4. Fall back to file-based loading if not in the catalog

This allows scenes to reference both pre-imported and AI-generated assets by name without hardcoding file paths.

## Further Reading

- [Importing Assets](../guides/importing-assets.md) --- step-by-step import guide
- [AI Asset Generation](ai-generation.md) --- AI-powered asset creation pipeline
- [Schemas](schemas.md) --- the `material` component schema for PBR properties
- [File Formats](../formats/overview.md) --- asset sidecar TOML format reference
