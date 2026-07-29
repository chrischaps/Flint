# CLI Reference

Flint's CLI is the primary interface for all engine operations. Below is a reference of available commands.

## Commands

| Command | Description |
|---------|-------------|
| `flint init <name>` | Initialize a new project |
| `flint entity create` | Create an entity in a scene |
| `flint entity delete` | Delete an entity from a scene |
| `flint scene create` | Create a new scene file |
| `flint scene list` | List scene files |
| `flint scene info` | Show scene metadata and entity count |
| `flint query "<query>"` | Query entities with the Flint query language |
| `flint schema <name>` | Inspect a component or archetype schema |
| `flint validate <scene>` | Validate a scene against constraints |
| `flint asset import` | Import a file into the asset store |
| `flint asset list` | List assets in the catalog |
| `flint asset info` | Show details for a specific asset |
| `flint asset resolve` | Check asset references in a scene |
| `flint asset generate` | Generate an asset using AI providers |
| `flint asset validate` | Validate a generated model against style constraints |
| `flint asset manifest` | Generate a build manifest of all generated assets |
| `flint asset regenerate` | Regenerate an existing asset with new parameters |
| `flint asset job status` | Check status of an async generation job |
| `flint asset job list` | List all generation jobs |
| `flint edit <file>` | Unified interactive editor (auto-detects file type) |
| `flint play <scene>` | Play a scene with first-person controls and physics |
| `flint render <scene>` | Render a scene to PNG (headless) |
| `flint gen <spec>` | Run a procedural generation spec to produce meshes or textures |
| `flint prefab view <template>` | Preview a prefab template in the viewer |

## The `play` Command

Launch a scene as an interactive first-person experience with physics:

```bash
flint play demo/phase4_runtime.scene.toml
flint play levels/tavern.scene.toml --schemas schemas --fullscreen
```

| Flag | Description |
|------|-------------|
| `--schemas <path>` | Path to schemas directory (repeatable; later paths override earlier). Default: `schemas` |
| `--fullscreen` | Launch in fullscreen mode |
| `--input-config <path>` | Input config overlay path (highest priority, overrides all other layers) |

### Player Controls (Defaults)

These are the built-in defaults. Games can override any binding via input config files (see [Physics and Runtime: Input System](../concepts/physics-and-runtime.md#input-system)).

| Input | Action |
|-------|--------|
| WASD | Move |
| Mouse | Look around |
| Left Click | Fire (weapon) |
| Space | Jump |
| Shift | Sprint |
| E | Interact with nearby object |
| R | Reload |
| 1 / 2 | Select weapon slot |
| Escape | Release cursor / Exit |
| F1 | Cycle debug rendering mode (PBR → Wireframe → Normals → Depth → UV → Unlit → Metal/Rough) |
| F3 | Toggle debug panels (see below) |
| F4 | Toggle shadows |
| F5 | Toggle bloom |
| F6 | Toggle post-processing pipeline |
| F11 | Toggle fullscreen |

Gamepad controllers are also supported when connected. Bindings for gamepad buttons and axes can be configured in input config TOML files.

### Debug Panels (F3)

`F3` opens live tuning panels for whichever systems the current scene actually
uses. Each panel is created **only when its driving component is present**, so a
scene with no ocean never sees an ocean panel and a scene with no panels at all
logs a note instead. Panels are fold-open headers distributed across up to three
size-balanced columns.

Built-in panels:

| Panel | Component | Controls |
|-------|-----------|----------|
| Ocean Debug | `ocean` | Wave spectrum, colors, foam, contact foam, cel band edges, clarity/turbidity, grid, CPU/GPU parity probe |
| Day / Time | `time_of_day` | Clock readout, 0–24 h scrub slider, preset hours, natural-advance toggle, day counter, day length (with the effective ramped length), sun path tilt |
| Camera | `camera_tuning` | Vertical FOV |
| Grass Debug | `grass` | Density, height, wind, LOD distances |

Edits apply live through the world's components. **Commit to File** writes the
current values back into the scene TOML, so a tuning session ends as a diff
rather than as notes. Values a game script owns each frame (a day counter, a
published factor) are deliberately never committed.

Games can add their own panels for their own components; unknown panels simply
take a default size weight in the column layout.

`Escape` releases the mouse so panels can be clicked; clicking the world
recaptures it.

The `play` command requires the scene to have a `player` archetype entity with a `character_controller` component. Physics colliders on other entities define the walkable geometry.

### Game Project Pattern

Games that define their own schemas, scripts, and assets use multiple `--schemas` paths. Game projects typically live in their own repositories with the engine included as a git subtree at `engine/`. The engine schemas come first, then the game-specific schemas overlay on top:

```bash
# From a game project root (engine at engine/)
cargo run --manifest-path engine/Cargo.toml --bin flint-player -- \
  scenes/level_1.scene.toml \
  --schemas engine/schemas \
  --schemas schemas
```

This loads the engine's built-in components (transform, material, rigidbody, etc.) from `engine/schemas/`, then adds game-specific components (health, weapon, enemy AI) from the game's own `schemas/`. See [Schemas: Game Project Schemas](../concepts/schemas.md#game-project-schemas) for directory structure details and [Building a Game Project](../guides/building-a-game-project.md) for the full workflow.

### Standalone Player Binary

The player is also available as a standalone binary for distribution:

```bash
cargo run --bin flint-player -- demo/phase4_runtime.scene.toml --schemas schemas

# With game project schemas (from a game repo with engine subtree)
cargo run --manifest-path engine/Cargo.toml --bin flint-player -- \
  scenes/level_1.scene.toml --schemas engine/schemas --schemas schemas
```

## The `render` Command

Render a scene to a PNG image without opening a window:

```bash
flint render demo/phase3_showcase.scene.toml --output hero.png --schemas schemas
flint render scene.toml -o shot.png --distance 20 --pitch 30 --yaw 45 --target 0,1,0 --no-grid
```

| Flag | Default | Description |
|------|---------|-------------|
| `--output <path>` / `-o` | `render.png` | Output file path |
| `--width <px>` | `1920` | Image width |
| `--height <px>` | `1080` | Image height |
| `--distance <f32>` | (auto) | Camera distance from target |
| `--yaw <deg>` | (auto) | Horizontal camera angle |
| `--pitch <deg>` | (auto) | Vertical camera angle |
| `--target <x,y,z>` | (auto) | Camera look-at point |
| `--fov <deg>` | (auto) | Field of view |
| `--no-grid` | `false` | Disable ground grid |
| `--debug-mode <mode>` | (none) | `wireframe`, `normals`, `depth`, `uv`, `unlit`, `metalrough` |
| `--wireframe-overlay` | `false` | Wireframe edges on solid geometry |
| `--show-normals` | `false` | Normal direction arrows |
| `--no-tonemapping` | `false` | Raw linear output |
| `--no-shadows` | `false` | Disable shadow mapping |
| `--shadow-resolution <px>` | `1024` | Shadow map resolution per cascade |
| `--no-postprocess` | `false` | Disable post-processing |
| `--bloom-intensity <f32>` | `0.04` | Bloom strength |
| `--bloom-threshold <f32>` | `1.0` | Bloom brightness threshold |
| `--exposure <f32>` | `1.0` | Exposure multiplier |
| `--ssao-radius <f32>` | `0.5` | SSAO sample radius |
| `--ssao-intensity <f32>` | `1.0` | SSAO intensity (0 = disabled) |
| `--fog-density <f32>` | `0.02` | Fog density (0 = disabled) |
| `--fog-color <r,g,b>` | `0.7,0.75,0.82` | Fog color |
| `--fog-height-falloff <f32>` | `0.1` | Fog height falloff |
| `--schemas <path>` | `schemas` | Schemas directory (repeatable) |

## The `edit` Command

Unified interactive editor that auto-detects file type and opens the appropriate tool:

```bash
flint edit levels/demo.scene.toml              # Scene viewer (hot-reload)
flint edit levels/demo.scene.toml --spline     # Spline/track editor
flint edit models/character.glb                # Model previewer (orbit camera)
flint edit models/character.glb --watch        # Model previewer with file watching
flint edit specs/oak_tree.procgen.toml         # Procgen previewer (mesh/texture)
flint edit specs/stone_wall.procgen.toml       # Texture pipeline editor (if pipeline pattern)
flint edit terrain.terrain.toml                # Terrain editor
```

### File Type Detection

| Extension | Tool | Description |
|-----------|------|-------------|
| `.scene.toml`, `.chunk.toml` | Scene viewer | Hot-reload, egui inspector, gizmos |
| `.procgen.toml` (pipeline pattern) | Texture pipeline editor | Node graph for texture specs |
| `.procgen.toml` (other) | Procgen previewer | Live preview of generated mesh/texture |
| `.terrain.toml` | Terrain editor | Heightmap terrain editing |
| `.glb`, `.gltf` | Model previewer | Orbit camera, animation playback |

### Common Flags

| Flag | Default | Description |
|------|---------|-------------|
| `--schemas <path>` | `schemas` | Schemas directory (repeatable) |
| `--width <px>` | (auto) | Window width |
| `--height <px>` | (auto) | Window height |
| `--no-grid` | `false` | Disable ground grid |
| `--watch` | `false` | Watch for file changes |
| `--seed <u64>` | (auto) | Override seed (procgen) |
| `--no-inspector` | `false` | Hide egui inspector (scene) |
| `--auto-orbit` | `false` | Auto-orbit camera (model/procgen) |

### Model Previewer Flags

| Flag | Default | Description |
|------|---------|-------------|
| `--distance <f32>` | (auto) | Camera orbit distance |
| `--yaw <deg>` | (auto) | Horizontal camera angle |
| `--pitch <deg>` | (auto) | Vertical camera angle |
| `--target <x,y,z>` | (auto) | Camera look-at point |
| `--fov <deg>` | (auto) | Field of view |
| `--no-animate` | `false` | Disable animation playback |
| `--clip <name>` | (none) | Start with a specific animation clip |
| `--anim-speed <f32>` | `1.0` | Animation playback speed multiplier |
| `--render <path>` | (none) | Render to PNG instead of opening a window |

### Scene Viewer Controls

| Input | Action |
|-------|--------|
| Left-click | Select entity / pick gizmo axis |
| Left-drag | Orbit camera (or drag gizmo if axis selected) |
| Right-drag | Pan camera |
| Scroll | Zoom |
| Ctrl+S | Save scene to disk |
| Ctrl+Z | Undo position change |
| Ctrl+Shift+Z | Redo position change |
| Escape | Cancel gizmo drag |
| F1 | Cycle debug mode |
| F2 | Toggle wireframe overlay |
| F3 | Toggle normal arrows |
| F4 | Toggle shadows |

When an entity is selected, a **translate gizmo** appears with colored axis arrows (red = X, green = Y, blue = Z) and plane handles. Click and drag an axis or plane to move the entity. Position changes can be undone/redone and saved back to the scene file.

### Spline Editor Controls (`--spline`)

| Input | Action |
|-------|--------|
| Left-click | Select control point |
| Left-drag | Move control point on constraint plane |
| Alt + drag | Move control point vertically (Y axis) |
| Middle-drag | Orbit camera |
| Right-drag | Pan camera |
| Scroll | Zoom |
| Tab / Shift+Tab | Cycle through control points |
| I | Insert a new control point after selected |
| Delete | Remove selected control point |
| Ctrl+S | Save spline to disk |
| Ctrl+Z | Undo |

> **Legacy aliases:** `flint serve`, `flint preview`, `flint gen-preview`, `flint tex-edit`, `flint terrain-edit`, and `flint spline-edit` still work but route through `flint edit`.

## The `asset generate` Command

Generate assets using AI providers:

```bash
flint asset generate texture -d "rough stone wall" --style medieval_tavern
flint asset generate model -d "wooden chair" --provider meshy --seed 42
flint asset generate audio -d "tavern ambient noise" --duration 10.0
```

| Flag | Description |
|------|-------------|
| `-d`, `--description` | Generation prompt (required) |
| `--name` | Asset name (derived from description if omitted) |
| `--provider` | Provider to use: `flux`, `meshy`, `elevenlabs`, `mock` |
| `--style` | Style guide name (e.g., `medieval_tavern`) |
| `--width`, `--height` | Image dimensions for textures (default: 1024x1024) |
| `--seed` | Random seed for reproducibility |
| `--tags` | Comma-separated tags |
| `--output` | Output directory (default: `.flint/generated`) |
| `--duration` | Audio duration in seconds (default: 3.0) |

Generated assets are automatically stored in content-addressed storage and registered in the asset catalog with a `.asset.toml` sidecar. Models are validated against style constraints after generation.

## The `gen` Command

Run a procedural generation spec to produce meshes (GLB) or textures (PNG):

```bash
flint gen specs/oak_tree.procgen.toml -o tree.glb
flint gen specs/stone_wall.procgen.toml -o wall.png
flint gen specs/oak_tree.procgen.toml --dry-run
flint gen specs/oak_tree.procgen.toml --seed 42 -o tree.glb
flint gen specs/oak_tree.procgen.toml --batch 10 --seed-start 0
```

| Flag | Default | Description |
|------|---------|-------------|
| `-o, --output <path>` | (derived from spec) | Output file or directory |
| `--seed <u64>` | (from spec) | Override the spec's seed |
| `--dry-run` | `false` | Print estimated cost without generating |
| `--format <fmt>` | (auto) | Force output format: `glb` or `png` |
| `--batch <N>` | (none) | Generate N variants with sequential seeds |
| `--seed-start <u64>` | `0` | Starting seed for batch generation |
| `--register` | `false` | Store output in content store with provenance |
| `--force` | `false` | Regenerate even if cached |
| `--validate` | `false` | Validate output after generation |
| `--strict` | `false` | Treat warnings as failures |
| `--style-guide <path>` | (none) | Style guide TOML for validation constraints |

## The `prefab view` Command

Preview a prefab template in the interactive viewer:

```bash
flint prefab view prefabs/kart.prefab.toml --schemas engine/schemas --schemas schemas
```

| Flag | Default | Description |
|------|---------|-------------|
| `--prefix <string>` | `"preview"` | Prefix for `${PREFIX}` substitution |
| `--schemas <path>` | `schemas` | Schemas directory (repeatable) |

This command loads the `.prefab.toml` template, performs variable substitution, builds a synthetic scene from the expanded entities, and launches the viewer for visual inspection. Useful for verifying prefab structure and appearance without creating a full scene.

## Common Flags

| Flag | Description |
|------|-------------|
| `--scene <path>` | Path to scene file |
| `--schemas <path>` | Path to schemas directory (repeatable for multi-schema layering; default: `schemas`) |
| `--format <fmt>` | Output format: `json`, `toml`, or `text` |
| `--fix` | Apply auto-fixes (with `validate`) |
| `--dry-run` | Preview changes without applying |

## Usage

```bash
# Get help
flint --help
flint <command> --help

# Examples
flint init my-game
flint edit levels/tavern.scene.toml              # Interactive scene viewer
flint edit models/character.glb --watch           # Model previewer
flint play levels/tavern.scene.toml
flint render levels/tavern.scene.toml -o shot.png
flint gen specs/oak_tree.procgen.toml -o tree.glb
flint query "entities where archetype == 'door'" --scene levels/tavern.scene.toml
```
