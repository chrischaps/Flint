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
| `flint serve <scene>` | Launch the hot-reload PBR viewer with egui inspector |
| `flint play <scene>` | Play a scene with first-person controls and physics |
| `flint render <scene>` | Render a scene to PNG (headless) |

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
| F4 | Toggle shadows |
| F5 | Toggle bloom |
| F6 | Toggle post-processing pipeline |
| F11 | Toggle fullscreen |

Gamepad controllers are also supported when connected. Bindings for gamepad buttons and axes can be configured in input config TOML files.

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
| `--schemas <path>` | `schemas` | Schemas directory (repeatable) |

## The `serve` Command

Launch the interactive scene viewer with hot-reload:

```bash
flint serve demo/phase3_showcase.scene.toml --watch --schemas schemas
flint serve scene.toml --no-inspector
```

| Flag | Default | Description |
|------|---------|-------------|
| `--watch` | `false` | Watch for file changes and auto-reload |
| `--no-inspector` | `false` | Hide egui inspector panels |
| `--schemas <path>` | `schemas` | Schemas directory (repeatable) |

### Viewer Controls

| Input | Action |
|-------|--------|
| Left-drag | Orbit camera |
| Right-drag | Pan camera |
| Scroll | Zoom |
| F1 | Cycle debug mode |
| F2 | Toggle wireframe overlay |
| F3 | Toggle normal arrows |
| F4 | Toggle shadows |

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

## Common Flags

| Flag | Description |
|------|-------------|
| `--scene <path>` | Path to scene file |
| `--schemas <path>` | Path to schemas directory (repeatable for multi-schema layering; default: `schemas`) |
| `--format <fmt>` | Output format: `json`, `toml`, or `text` |
| `--watch` | Watch for file changes (with `serve`) |
| `--fix` | Apply auto-fixes (with `validate`) |
| `--dry-run` | Preview changes without applying |

## Usage

```bash
# Get help
flint --help
flint <command> --help

# Examples
flint init my-game
flint serve levels/tavern.scene.toml --watch --schemas schemas
flint play levels/tavern.scene.toml
flint query "entities where archetype == 'door'" --scene levels/tavern.scene.toml
```
