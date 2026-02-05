# Phase 2: Constraints and Assets

Flint's core thesis is that the primary interface is CLI and code, with visual tools focused on *validating* results rather than *creating* them. Phase 2 extends that idea into two domains: **how do you know a scene is correct?** and **how do you manage the stuff scenes are made of?**

These are the two subsystems that sit between raw scene editing (Phase 1) and full rendering/runtime (Phases 3-4). They don't depend on each other, but they converge at the CLI, where a single `flint validate` or `flint asset import` command ties everything together.

---

## Philosophy

### Constraints as living documentation

Traditional engines bury validation logic in code. You discover rules by breaking them. Flint takes a different approach: constraints are **data**, authored in TOML alongside the schemas they protect. They are readable, diffable, and versionable. A constraint file is both a validation rule *and* a specification document — it tells you what the scene *should* look like, not just what it *can* look like.

The query language (`entities where archetype == 'door'`) is the same one used in the CLI for ad-hoc exploration. This means learning one syntax teaches you both how to explore scenes and how to write rules for them.

Auto-fix is deliberately conservative. The system will iterate up to 10 times, but it tracks every (constraint, entity) pair it has touched. If it sees the same pair twice, it stops and reports a cycle rather than looping forever. Dry-run mode lets you preview what *would* happen without touching the scene file.

### Content-addressed assets

Every asset is identified by the SHA-256 hash of its contents. Two imports of the same file produce the same hash and are stored once. This makes deduplication automatic and change detection trivial — if the hash changed, the content changed.

The storage layout (`.flint/assets/<first-2-hex>/<full-hash>.<ext>`) is borrowed from Git's object store. Metadata lives in `.asset.toml` sidecar files alongside the project's `assets/` directory, not inside the binary store. This keeps the human-readable catalog separate from the content-addressed blobs.

Asset references in scene files are just strings — `mesh = "tavern_chair"` in an entity's TOML. The `AssetRef` type resolves these by name, hash, or path, so scenes stay readable while the underlying storage remains content-addressed.

---

## Directory Structure

After `flint init`, a project looks like this:

```
my-project/
├── schemas/
│   ├── components/        # What fields exist
│   │   ├── transform.toml
│   │   ├── bounds.toml
│   │   └── door.toml
│   ├── archetypes/        # What bundles of components form an entity type
│   │   ├── room.toml
│   │   ├── door.toml
│   │   ├── furniture.toml
│   │   └── character.toml
│   └── constraints/       # What rules the scene must satisfy    [NEW]
│       └── basics.toml
├── assets/                                                       [NEW]
│   ├── meshes/            # .asset.toml sidecars for mesh assets
│   ├── textures/          # .asset.toml sidecars for textures
│   └── materials/         # .asset.toml sidecars for materials
├── levels/
│   └── demo.scene.toml
└── .flint/
    └── assets/            # Content-addressed blob storage        [NEW]
        └── a1/
            └── a1b2c3...full-hash.glb
```

---

## Constraints

### Writing a constraint

Constraint files live in `schemas/constraints/` and use TOML's array-of-tables syntax. Each `[[constraint]]` block defines one rule.

```toml
[[constraint]]
name = "doors_have_transform"
description = "Every door must have a transform component"
query = "entities where archetype == 'door'"
severity = "error"
message = "Door '{name}' is missing a transform component"

[constraint.kind]
type = "required_component"
archetype = "door"
component = "transform"
```

**Fields:**

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Unique identifier for the constraint |
| `description` | no | Human-readable explanation |
| `query` | yes | Flint query selecting which entities to check |
| `severity` | yes | `error`, `warning`, or `info` |
| `message` | yes | Template with `{name}` and `{archetype}` placeholders |
| `kind` | yes | What to check (see below) |
| `auto_fix` | no | How to fix violations automatically |

### Constraint kinds

**`required_component`** — An entity of a given archetype must have a specific component.

```toml
[constraint.kind]
type = "required_component"
archetype = "door"
component = "transform"
```

**`required_child`** — An entity must have at least one child with the given archetype.

```toml
[constraint.kind]
type = "required_child"
child_archetype = "handle"
```

**`value_range`** — A numeric field must fall within bounds. The field path uses dot notation: `component.field`.

```toml
[constraint.kind]
type = "value_range"
field = "door.open_angle"
min = 0.0
max = 180.0
```

**`reference_valid`** — A string field must name an entity that exists in the scene.

```toml
[constraint.kind]
type = "reference_valid"
field = "link.target"
```

**`query_rule`** — A freeform query. The constraint is violated if the rule query returns *no* results (i.e., the expected condition is not met).

```toml
[constraint.kind]
type = "query_rule"
rule = "entities where archetype == 'spawn_point'"
```

### Auto-fix strategies

Add an `auto_fix` block to any constraint to enable automatic repair.

**`add_child`** — Spawn a child entity with the given archetype.

```toml
[constraint.auto_fix]
enabled = true
strategy = "add_child"
archetype = "handle"
```

**`set_default`** — Set a field to a specific value.

```toml
[constraint.auto_fix]
enabled = true
strategy = "set_default"
field = "door.open_angle"
value = 90.0
```

**`remove_invalid`** — Remove the offending field entirely.

```toml
[constraint.auto_fix]
enabled = true
strategy = "remove_invalid"
```

**`assign_from_parent`** — Copy a value from the parent entity.

```toml
[constraint.auto_fix]
enabled = true
strategy = "assign_from_parent"
field = "child_comp.inherited_field"
source_field = "parent_comp.source_field"
```

### CLI usage

```bash
# Check a scene against all constraints
flint validate levels/demo.scene.toml --schemas schemas

# JSON output for CI pipelines (exits with code 1 on errors)
flint validate levels/demo.scene.toml --format json

# Preview what auto-fix would do, without changing anything
flint validate levels/demo.scene.toml --fix --dry-run

# Apply fixes and save the scene
flint validate levels/demo.scene.toml --fix

# Apply fixes and show a diff of what changed
flint validate levels/demo.scene.toml --fix --output-diff
```

**Text output example:**

```
3 violation(s): 1 error(s), 2 warning(s), 0 info

  [ERROR] bare_door: Door 'bare_door' is missing a transform component
  [WARN ] front_door: Door 'front_door' is missing a handle [fixable]
  [WARN ] back_door: Door 'back_door' is missing a handle [fixable]
```

**JSON output example:**

```json
{
  "valid": false,
  "summary": "3 violation(s): 1 error(s), 2 warning(s), 0 info",
  "errors": 1,
  "warnings": 2,
  "info": 0,
  "violations": [
    {
      "constraint": "doors_need_transform",
      "entity": "bare_door",
      "severity": "error",
      "message": "Door 'bare_door' is missing a transform component",
      "has_auto_fix": false
    }
  ]
}
```

### How evaluation works

1. The `ConstraintRegistry` loads every `.toml` file from `schemas/constraints/`.
2. For each constraint, the evaluator parses the `query` field using Flint's query language and executes it against the world to find matching entities.
3. Each matched entity is checked against the constraint's `kind` rules.
4. Violations are collected into a `ValidationReport` with severity counts.

When `--fix` is used, the fixer enters an iterative loop:

1. Validate the world.
2. For each fixable violation, apply the auto-fix strategy.
3. Re-validate. If new violations appeared (a cascade), loop again.
4. Stop when: no violations remain, no fixable violations remain, a cycle is detected, or 10 iterations are reached.

---

## Assets

### Importing an asset

```bash
# Import a glTF/GLB file
flint asset import models/chair.glb --name tavern_chair --tags furniture,medieval

# Import any file (type guessed from extension)
flint asset import textures/stone_wall.png --tags environment
```

This does three things:

1. **Hashes** the file (SHA-256) and copies it into `.flint/assets/<prefix>/<hash>.<ext>`.
2. **Extracts metadata** — for glTF files, this includes mesh count, vertex count, materials, and textures. For other files, basic file info is recorded.
3. **Writes a sidecar** `.asset.toml` file in the appropriate `assets/` subdirectory.

For glTF imports specifically, the importer extracts:
- Meshes with positions, normals, UVs, and indices
- PBR materials (base color, metallic, roughness, textures)
- Embedded textures with dimensions and format

### Asset sidecar format

Each imported asset gets a `.asset.toml` file:

```toml
[asset]
name = "tavern_chair"
type = "mesh"
hash = "sha256:a1b2c3d4e5f6..."
source_path = "models/chair.glb"
format = "glb"
tags = ["furniture", "medieval"]

[asset.properties]
vertex_count = 1234
mesh_count = 1
material_count = 2
```

These sidecars are the source of truth for the asset catalog. They're human-readable, diffable, and can be manually edited.

### Browsing assets

```bash
# List all assets
flint asset list

# Filter by type
flint asset list --type mesh

# Filter by tag
flint asset list --tag furniture

# JSON output
flint asset list --format json

# Show details for a specific asset
flint asset info tavern_chair
```

### Resolving asset references

Scenes reference assets by name in entity component data:

```toml
[entities.chair]
archetype = "furniture"
mesh = "tavern_chair"
```

The resolver checks that every asset reference in a scene can be found in the catalog:

```bash
# Strict mode: fail on any missing asset
flint asset resolve levels/tavern.scene.toml --strategy strict

# Placeholder mode: report missing but don't fail
flint asset resolve levels/tavern.scene.toml --strategy placeholder
```

### Content-addressed storage

The store uses a two-level directory structure based on the first two hex characters of the SHA-256 hash:

```
.flint/assets/
├── a1/
│   └── a1b2c3d4...full64charhash.glb
├── f7/
│   └── f7e8d9c0...full64charhash.png
```

This prevents any single directory from accumulating too many files. Duplicate imports are detected by hash and silently deduplicated — the file is only stored once regardless of how many times it's imported.

### Resolution strategies

| Strategy | Behavior |
|----------|----------|
| `strict` | Missing asset = error. Exit code 1. |
| `placeholder` | Missing asset = warning. A placeholder is used at runtime. |
| `human_task` | (Phase 5) Creates a task for a human to provide the asset. |
| `ai_generate` | (Phase 5) Queues the asset for AI generation. |

---

## Crate Architecture

Phase 2 adds three crates to the workspace. None of the existing crates depend on them — all new crates feed upward into `flint-cli`.

```
flint-cli
  ├── flint-constraint ──┬── flint-schema ── flint-core
  │                      ├── flint-ecs
  │                      └── flint-query
  ├── flint-asset ────── flint-core
  ├── flint-import ───── flint-asset
  │   (existing crates unchanged)
  ├── flint-query
  ├── flint-scene
  └── flint-render
```

**`flint-constraint`** (`crates/flint-constraint/`)
| Module | Purpose |
|--------|---------|
| `types.rs` | Severity, ConstraintKind, AutoFixStrategy, ConstraintDef |
| `registry.rs` | Load and query constraint definitions |
| `evaluator.rs` | Run constraints against a world |
| `report.rs` | Violation and ValidationReport types |
| `fixer.rs` | Auto-fix engine with cascade/cycle detection |
| `diff.rs` | Line-by-line TOML diff for `--output-diff` |

**`flint-asset`** (`crates/flint-asset/`)
| Module | Purpose |
|--------|---------|
| `types.rs` | AssetType, AssetMeta, AssetRef |
| `store.rs` | Content-addressed file storage |
| `catalog.rs` | Metadata catalog with name/hash/type/tag indexing |
| `resolver.rs` | Asset reference resolution with strategy pattern |

**`flint-import`** (`crates/flint-import/`)
| Module | Purpose |
|--------|---------|
| `types.rs` | ImportResult, ImportedMesh, ImportedTexture, ImportedMaterial |
| `gltf_import.rs` | glTF/GLB file importer |

---

## Extending the System

### Adding a new constraint kind

1. Add a variant to `ConstraintKind` in `crates/flint-constraint/src/types.rs`.
2. Add a `check_*` method in `evaluator.rs` and wire it into `evaluate_constraint`.
3. If it needs auto-fix, add a corresponding case in `fixer.rs`.

### Adding a new asset importer

1. Add a new module in `crates/flint-import/src/` (e.g., `fbx_import.rs`).
2. Return an `ImportResult` with the extracted data.
3. Wire the new format into the match on file extension in `crates/flint-cli/src/commands/asset.rs`.

### Adding a new auto-fix strategy

1. Add a variant to `AutoFixStrategy` in `types.rs`.
2. Implement the fix logic in `fixer.rs` inside `apply_fix`.
3. The TOML format picks it up automatically via serde's tagged enum.

---

## Relationship to Future Phases

Phase 2 is infrastructure. It doesn't render imported meshes or run constraints at runtime — those are Phase 3 and Phase 4 concerns. What it does is establish the two pipelines that everything else flows through:

- **Constraints** become the backbone of the scene editing loop: modify scene, validate, fix, verify. Phase 4's scripting system will be able to register constraints programmatically. The viewer (Phase 3) will display constraint violations as overlays.

- **Assets** become the input to rendering. Phase 3's enhanced renderer will consume `ImportedMesh` data to display real geometry instead of colored boxes. The AI asset generation pipeline (Phase 5) will produce assets that flow through the same content-addressed store.

The two systems are independent today but will intersect: a constraint can verify that an entity's `mesh` field references a real asset (`reference_valid` on asset names), and the asset resolver can check whether a scene's dependencies are all satisfied before attempting to render it.
