# AI Agent Interface

Flint is designed from the ground up to be an excellent interface for AI coding agents. Where traditional engines optimize for human spatial reasoning and visual feedback, Flint optimizes for text-based reasoning, structured data, and automated validation.

## The Problem with GUI Engines

AI agents working with traditional game engines face fundamental friction:

- **Screenshot parsing** --- agents must interpret rendered pixels to understand scene state, an unreliable and lossy process
- **GUI automation** --- clicking buttons and dragging sliders through accessibility APIs or screenshot analysis is brittle
- **Binary formats** --- proprietary project files can't be read, diffed, or generated as text
- **Implicit state** --- engine state lives in inspector panels, viewport selections, and undo histories that agents can't access

Flint eliminates all of these friction points.

## Structured Input and Output

Every Flint command accepts text input and produces structured text output:

```bash
# JSON output for machine parsing
flint query "entities where archetype == 'door'" --scene levels/tavern.scene.toml --format json

# Exit codes signal success (0) or failure (1)
flint validate levels/tavern.scene.toml --format json
echo $?  # 0 = valid, 1 = violations found
```

An agent can create entities, modify scenes, and inspect state entirely through text --- no screenshots, no pixel coordinates, no GUI automation.

## Query-Based Introspection

The query language gives agents programmatic access to scene state. Instead of reading a screenshot to count doors, an agent can:

```bash
# How many doors are in this scene?
flint query "entities where archetype == 'door'" --scene levels/tavern.scene.toml --format json | jq length

# Is this door locked?
flint query "entities where door.locked == true" --scene levels/tavern.scene.toml --format json

# What components does the player entity have?
flint query "entities where archetype == 'player'" --scene levels/tavern.scene.toml --format json
```

Queries return structured data that agents can parse, reason about, and use to plan their next action.

## Constraint Validation as Feedback

Constraints provide an automated feedback loop. An agent doesn't need a human to check its work --- it can validate programmatically:

```bash
# Agent creates some entities...
flint entity create --archetype door --name "secret_door" --scene levels/tavern.scene.toml

# Then checks if the scene is still valid
flint validate levels/tavern.scene.toml --format json
```

If validation fails, the JSON output tells the agent exactly what's wrong and how to fix it. The `--fix --dry-run` mode even previews what auto-fixes would apply. This creates a tight create-validate-fix loop that agents can execute without human intervention.

## Schema Introspection

Agents can discover what components and archetypes are available without reading documentation:

```bash
# What fields does the 'door' component have?
flint schema door

# What components does the 'player' archetype include?
flint schema player
```

This means an agent can learn the engine's data model at runtime, then use that knowledge to create valid entities.

## Headless Rendering

Visual verification without a window:

```bash
# Render the scene to an image file
flint render levels/tavern.scene.toml --output preview.png --width 1920 --height 1080
```

An agent (or its supervisor) can render a preview image to check that the scene looks correct, without opening a GUI. This enables visual regression testing in CI and supports workflows where an agent builds a scene, renders a preview, and a human reviews the image.

## TOML as Scene Format

Scenes are plain TOML text files. An agent can:

- **Read** a scene file directly as text
- **Write** entity data by editing TOML
- **Diff** changes with standard tools (`git diff`)
- **Generate** entire scenes programmatically
- **Merge** changes from multiple agents without conflicts (each entity is a distinct TOML section)

No proprietary binary formats, no deserialization libraries, no SDK required.

## AI Asset Generation

Phase 5 extends the agent interface to asset creation. Agents can generate textures, 3D models, and audio through CLI commands:

```bash
# Generate a texture using AI
flint asset generate texture -d "rough stone wall with mortar lines" --style medieval_tavern

# Batch-generate all missing assets for a scene
flint asset resolve my_scene.scene.toml --strategy ai_generate --style medieval_tavern
```

Style guides ensure generated assets maintain visual consistency, and model validation checks results against constraints --- the same automated feedback loop that works for scene structure now works for asset quality.

## Further Reading

- [CLI-First Workflow](cli-first.md) --- the composable command interface
- [AI Agent Workflow](../guides/ai-agent-workflow.md) --- step-by-step guide for agent developers
- [AI Asset Generation](../concepts/ai-generation.md) --- the AI asset generation pipeline
