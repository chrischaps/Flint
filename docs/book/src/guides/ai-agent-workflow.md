# AI Agent Workflow

This guide covers how AI coding agents interact with Flint to build game scenes programmatically. It describes the agent interaction loop, error handling patterns, and best practices.

## The Agent Interaction Loop

An agent building a scene follows a create-validate-query-render cycle:

```
┌──────────────────────────────────────────────────┐
│                                                  │
│   1. Discover ──► 2. Create ──► 3. Validate ─┐   │
│        ▲                                     │   │
│        │          4. Query ◄─── 5. Fix ◄─────┘   │
│        │              │                          │
│        └──────────────┤                          │
│                       ▼                          │
│                  6. Render ──► Human Review       │
│                                                  │
└──────────────────────────────────────────────────┘
```

### Step 1: Discover Available Schemas

Before creating entities, the agent learns what's available:

```bash
# List available archetypes
flint schema --list-archetypes --schemas schemas

# Inspect a specific archetype
flint schema player --schemas schemas

# Inspect a component
flint schema collider --schemas schemas
```

This tells the agent what fields exist, their types, and their defaults.

### Step 2: Create Scene and Entities

```bash
# Create the scene file
flint scene create levels/dungeon.scene.toml --name "Dungeon Level 1"

# Create entities
flint entity create --archetype room --name "entrance" \
    --scene levels/dungeon.scene.toml

flint entity create --archetype door --name "iron_gate" \
    --parent "entrance" \
    --scene levels/dungeon.scene.toml
```

Or the agent can write TOML directly --- often faster for complex scenes:

```toml
[scene]
name = "Dungeon Level 1"

[entities.entrance]
archetype = "room"

[entities.entrance.transform]
position = [0.0, 0.0, 0.0]

[entities.entrance.bounds]
size = [10.0, 4.0, 10.0]

[entities.iron_gate]
archetype = "door"
parent = "entrance"

[entities.iron_gate.transform]
position = [0.0, 1.5, -5.0]
```

### Step 3: Validate

Check the scene against constraints:

```bash
flint validate levels/dungeon.scene.toml --format json --schemas schemas
```

JSON output example:

```json
{
  "valid": false,
  "violations": [
    {
      "constraint": "doors_have_transform",
      "entity": "iron_gate",
      "severity": "error",
      "message": "Door 'iron_gate' is missing a transform component"
    }
  ]
}
```

The agent parses this JSON, understands what's wrong, and proceeds to fix it.

### Step 4: Query to Verify

After fixing violations, the agent can query to confirm the scene state:

```bash
# Verify the door now has a transform
flint query "entities where archetype == 'door'" \
    --scene levels/dungeon.scene.toml --format json

# Count entities
flint query "entities" \
    --scene levels/dungeon.scene.toml --format json | jq length
```

### Step 5: Fix and Iterate

If validation fails, the agent can either:

- **Auto-fix** --- let Flint handle it:
  ```bash
  flint validate levels/dungeon.scene.toml --fix --dry-run --format json
  flint validate levels/dungeon.scene.toml --fix
  ```

- **Manual fix** --- edit the TOML to add missing data

### Step 6: Render for Review

Generate a preview image for human (or vision-model) review:

```bash
flint render levels/dungeon.scene.toml --output preview.png \
    --width 1920 --height 1080 --distance 25 --yaw 45
```

## AI Asset Generation

Agents can generate assets alongside scene construction:

```bash
# Generate textures for the scene
flint asset generate texture \
    -d "rough dungeon stone wall, torch-lit" \
    --style medieval_tavern \
    --name dungeon_wall_texture

# Generate a 3D model
flint asset generate model \
    -d "iron-bound wooden door, medieval dungeon" \
    --provider meshy \
    --name iron_door_model

# Batch-generate all missing assets for the entire scene
flint asset resolve levels/dungeon.scene.toml \
    --strategy ai_generate \
    --style medieval_tavern
```

Semantic asset definitions in the scene file guide batch generation:

```toml
[entities.wall_section.asset_def]
name = "dungeon_wall_texture"
description = "Rough stone dungeon wall with moss and cracks"
type = "texture"
material_intent = "rough stone"
wear_level = 0.8
```

## Error Handling Patterns

### Exit Codes

All Flint commands use standard exit codes:
- **0** --- success
- **1** --- error (validation failure, missing file, etc.)

```bash
flint validate levels/dungeon.scene.toml --format json
if [ $? -ne 0 ]; then
    echo "Validation failed, fixing..."
    flint validate levels/dungeon.scene.toml --fix
fi
```

### JSON Error Output

Error details are always available in JSON:

```bash
flint validate levels/dungeon.scene.toml --format json 2>/dev/null
```

### Idempotent Operations

Most Flint operations are idempotent --- running them twice produces the same result. This is important for agents that may retry failed operations.

## Example: Agent Building a Complete Scene

Here's a complete agent workflow script:

```bash
#!/bin/bash
set -e
SCENE="levels/generated.scene.toml"
SCHEMAS="schemas"

# 1. Create scene
flint scene create "$SCENE" --name "Agent-Generated Level"

# 2. Build structure
flint entity create --archetype room --name "main_room" --scene "$SCENE"
flint entity create --archetype room --name "side_room" --scene "$SCENE"
flint entity create --archetype door --name "connecting_door" \
    --parent "main_room" --scene "$SCENE"

# 3. Add player
flint entity create --archetype player --name "player" --scene "$SCENE"

# 4. Validate (will likely fail --- no transforms yet)
flint validate "$SCENE" --schemas "$SCHEMAS" --format json || true

# 5. Auto-fix what we can
flint validate "$SCENE" --schemas "$SCHEMAS" --fix

# 6. Verify
ENTITY_COUNT=$(flint query "entities" --scene "$SCENE" --format json | jq length)
echo "Scene has $ENTITY_COUNT entities"

# 7. Render preview
flint render "$SCENE" --output preview.png --schemas "$SCHEMAS" \
    --width 1920 --height 1080

echo "Scene built successfully. Preview: preview.png"
```

## Best Practices

- **Always validate after creating entities** --- catch problems early
- **Use JSON output** --- easier to parse than text
- **Use `--dry-run` before `--fix`** --- preview changes before applying
- **Write TOML directly for complex scenes** --- faster than many CLI commands
- **Use semantic asset definitions** --- let batch resolution handle asset generation
- **Render previews** --- visual verification catches issues that validation can't

## Further Reading

- [AI Agent Interface](../philosophy/ai-agent.md) --- the design philosophy
- [CLI-First Workflow](../philosophy/cli-first.md) --- composable commands
- [AI Asset Generation](../concepts/ai-generation.md) --- the AI asset pipeline
- [CLI Reference](../cli-reference/overview.md) --- full command documentation
