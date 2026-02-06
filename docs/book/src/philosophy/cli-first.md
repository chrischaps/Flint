# CLI-First Workflow

Flint's primary interface is the command line. Every engine operation --- creating entities, querying scenes, validating constraints, importing assets, generating content --- is a composable CLI command. Visual tools exist to *validate* results, not to *create* them.

## Why CLI-First?

Traditional game engines center on visual editors: drag a mesh into a viewport, tweak a slider, click Save. This works well for a single human at a desk, but it creates friction for:

- **Automation** --- you can't script a drag-and-drop operation
- **Reproducibility** --- a sequence of mouse clicks isn't version-controllable
- **AI agents** --- they see text, not pixels
- **CI/CD** --- headless servers have no windows to click in
- **Collaboration** --- binary project files don't merge cleanly in git

Flint inverts the priority: text-first, visual-second. The CLI is the engine's native language.

## Composable Commands

Every command reads structured input and produces structured output. This means standard shell patterns work naturally:

```bash
# Create a scene with several entities
flint scene create levels/dungeon.scene.toml --name "Dungeon Level 1"
flint entity create --archetype room --name "entrance" --scene levels/dungeon.scene.toml
flint entity create --archetype door --name "iron_gate" --parent "entrance" --scene levels/dungeon.scene.toml

# Query and filter with standard tools
flint query "entities where archetype == 'door'" --scene levels/dungeon.scene.toml --format json

# Validate and capture results
flint validate levels/dungeon.scene.toml --format json

# Render a preview image for review
flint render levels/dungeon.scene.toml --output preview.png --width 1920 --height 1080
```

## Structured Output

Commands support `--format json` and `--format toml` output modes, making their results machine-readable. This enables pipelines like:

```bash
# Count entities of each archetype
flint query "entities" --scene levels/tavern.scene.toml --format json | jq 'group_by(.archetype) | map({archetype: .[0].archetype, count: length})'

# Check if validation passes (exit code 0 = clean, 1 = violations)
flint validate levels/tavern.scene.toml --format json && echo "Scene is valid"
```

JSON output follows consistent schemas, so tools can parse results reliably across engine versions.

## Batch Operations

Because every operation is a command, building complex scenes is just a script:

```bash
#!/bin/bash
SCENE="levels/tavern.scene.toml"

flint scene create "$SCENE" --name "The Rusty Flagon"

# Build the structure
for room in main_hall kitchen storage; do
    flint entity create --archetype room --name "$room" --scene "$SCENE"
done

# Add doors between rooms
flint entity create --archetype door --name "kitchen_door" --parent "main_hall" --scene "$SCENE"
flint entity create --archetype door --name "storage_door" --parent "kitchen" --scene "$SCENE"

# Validate the whole thing
flint validate "$SCENE" --fix
```

This script is version-controllable, reproducible, and can run in CI.

## The Viewer as Validator

The `flint serve --watch` viewer and `flint play` command are verification tools, not authoring tools. They answer the question: *"Does the scene I built look correct?"*

```bash
# Edit the TOML in your text editor, viewer updates automatically
flint serve levels/tavern.scene.toml --watch

# Walk through the scene to verify physics, audio, and interactions
flint play levels/tavern.scene.toml
```

The viewer hot-reloads when the scene file changes. Edit TOML, save, see the result --- no GUI interaction required.

## Headless Rendering for CI

Scenes can be rendered to PNG without a window, enabling automated visual validation:

```bash
flint render levels/tavern.scene.toml --output screenshots/tavern.png --width 1920 --height 1080
```

This is the foundation for visual regression testing in CI pipelines --- render a baseline, then compare future renders against it.

## Contrast with GUI Engines

| Aspect | GUI Engine | Flint |
|--------|-----------|-------|
| Primary input | Mouse clicks, drag-and-drop | CLI commands, TOML files |
| Automation | Limited (editor scripting plugins) | Native (every operation is a command) |
| Version control | Binary project files | Text TOML files, clean git diffs |
| AI agent support | Screenshot parsing, GUI automation | Structured text I/O, query introspection |
| Headless operation | Usually not supported | First-class (render, validate, query) |
| Reproducibility | Manual steps, screenshots | Scripts, exit codes, structured output |

This doesn't mean Flint is text-only. It means the text interface is *complete* --- anything you can do in the viewer, you can do (and automate) from the command line.

## Further Reading

- [AI Agent Interface](ai-agent.md) --- how this philosophy benefits AI coding agents
- [Design Principles](design-principles.md) --- the broader design philosophy
- [CLI Reference](../cli-reference/overview.md) --- full command documentation
