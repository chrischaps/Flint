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
| `--schemas <path>` | Path to schemas directory (default: `schemas`) |
| `--fullscreen` | Launch in fullscreen mode |

### Player Controls

| Input | Action |
|-------|--------|
| WASD | Move |
| Mouse | Look around |
| Space | Jump |
| Shift | Sprint |
| Escape | Release cursor / Exit |
| F1 | Cycle debug rendering mode |
| F4 | Toggle shadows |
| F11 | Toggle fullscreen |

The `play` command requires the scene to have a `player` archetype entity with a `character_controller` component. Physics colliders on other entities define the walkable geometry.

### Standalone Player Binary

The player is also available as a standalone binary for distribution:

```bash
cargo run --bin flint-player -- demo/phase4_runtime.scene.toml --schemas schemas
```

## Common Flags

| Flag | Description |
|------|-------------|
| `--scene <path>` | Path to scene file |
| `--schemas <path>` | Path to schemas directory (default: `schemas`) |
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
