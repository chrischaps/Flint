# Importing Assets

This guide walks through importing external files into Flint's content-addressed asset store.

## Basic Import

Import a glTF model with the `flint asset import` command:

```bash
flint asset import models/chair.glb --name tavern_chair --tags furniture,medieval
```

This does three things:
1. **Hashes** the file (SHA-256) and stores it under `.flint/assets/<hash>/`
2. **Extracts** mesh, material, and texture data (for glTF/GLB files)
3. **Writes** a `.asset.toml` sidecar with metadata

## Content-Addressed Storage

Every imported file is stored by its content hash:

```
.flint/
└── assets/
    ├── a1/
    │   └── a1b2c3d4e5f6...  (the actual file)
    └── f7/
        └── f7a8b9c0d1e2...  (another file)
```

The first two hex characters of the hash form a subdirectory, preventing any single directory from having too many entries. Identical files are automatically deduplicated --- importing the same model twice stores it only once.

## glTF/GLB Import

For 3D models, the importer extracts structured data:

```bash
$ flint asset import models/tavern_door.glb --name tavern_door
Imported: 3 mesh(es), 2 texture(s), 2 material(s)
Asset 'tavern_door' registered.
  Hash: sha256:a1b2c3...
  Type: Mesh
  Sidecar: assets/meshes/tavern_door.asset.toml
```

Extracted data includes:
- **Meshes** --- vertex positions, normals, texture coordinates, indices, and optionally joint indices/weights for skeletal meshes
- **Materials** --- PBR properties (base color, roughness, metallic, emissive)
- **Textures** --- embedded or referenced image files
- **Skeletons** --- joint hierarchy and inverse bind matrices (if the model has skins)
- **Animations** --- per-joint keyframe channels (translation, rotation, scale)

## Sidecar Metadata

Each imported asset gets an `.asset.toml` file in the `assets/` directory:

```toml
[asset]
name = "tavern_chair"
type = "mesh"
hash = "sha256:a1b2c3d4e5f6..."
source_path = "models/chair.glb"
format = "glb"
tags = ["furniture", "medieval"]
```

For AI-generated assets, the sidecar also records provenance:

```toml
[asset.properties]
prompt = "weathered wooden tavern chair"
provider = "meshy"
```

## Tagging and Organization

Tags help organize and filter assets:

```bash
# Import with tags
flint asset import models/barrel.glb --name barrel --tags furniture,storage,medieval

# Filter by tag
flint asset list --tag medieval

# Filter by type
flint asset list --type mesh

# Get details on a specific asset
flint asset info tavern_chair
```

## Asset Catalog

The catalog is built by scanning all `.asset.toml` files in the `assets/` directory. It provides indexed lookup by name, type, and tag:

```bash
# List all assets
flint asset list

# JSON output for scripting
flint asset list --format json
```

## Resolving References

Check that a scene's asset references are satisfied:

```bash
# Strict mode --- all references must exist
flint asset resolve levels/tavern.scene.toml --strategy strict

# Placeholder mode --- missing assets replaced with fallback geometry
flint asset resolve levels/tavern.scene.toml --strategy placeholder

# AI generation --- missing assets created by AI providers
flint asset resolve levels/tavern.scene.toml --strategy ai_generate --style medieval_tavern
```

## Supported Formats

| Format | Type | Import Support |
|--------|------|----------------|
| `.glb`, `.gltf` | 3D Model | Full (mesh, material, texture, skeleton, animation) |
| `.png`, `.jpg`, `.bmp`, `.tga`, `.hdr` | Texture | Hash and catalog |
| `.wav`, `.ogg`, `.mp3`, `.flac` | Audio | Hash and catalog |
| Other | Generic | Hash and catalog (type guessed from extension) |

## Further Reading

- [Assets](../concepts/assets.md) --- content-addressed storage concept
- [AI Asset Generation](../concepts/ai-generation.md) --- generating assets with AI providers
- [File Formats](../formats/overview.md) --- `.asset.toml` sidecar format
