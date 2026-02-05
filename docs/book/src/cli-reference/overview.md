# CLI Reference

> This page is a stub. Full reference coming soon.

Flint's CLI is the primary interface for all engine operations. Below is a summary of available commands.

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
| `flint serve <scene>` | Launch the hot-reload viewer |
| `flint render <scene>` | Render a scene to PNG (headless) |

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
flint query "entities where archetype == 'door'" --scene levels/tavern.scene.toml
```
