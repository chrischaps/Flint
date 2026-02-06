# Querying Entities

Flint includes a SQL-inspired query language for filtering and inspecting entities. Queries let you search scenes by archetype, component values, or nested field data.

## Basic Syntax

All queries follow the pattern:

```
entities where <condition>
```

The simplest query returns all entities:

```bash
flint query "entities" --scene levels/tavern.scene.toml
```

## Filtering by Archetype

The most common filter --- find entities of a specific type:

```bash
# Find all doors
flint query "entities where archetype == 'door'" --scene levels/tavern.scene.toml

# Find all rooms
flint query "entities where archetype == 'room'" --scene levels/tavern.scene.toml
```

## Comparison Operators

| Operator | Meaning | Example |
|----------|---------|---------|
| `==` | Equal | `archetype == 'door'` |
| `!=` | Not equal | `archetype != 'room'` |
| `>` | Greater than | `transform.position.y > 5.0` |
| `<` | Less than | `door.open_angle < 90` |
| `>=` | Greater or equal | `audio_source.volume >= 0.5` |
| `<=` | Less or equal | `collider.friction <= 0.3` |
| `contains` | String contains | `name contains 'wall'` |

## Querying Component Fields

Access component fields with dot notation:

```bash
# Find locked doors
flint query "entities where door.locked == true" --scene levels/tavern.scene.toml

# Find entities above a certain height
flint query "entities where transform.position.y > 2.0" --scene levels/tavern.scene.toml

# Find loud audio sources
flint query "entities where audio_source.volume > 0.8" --scene levels/tavern.scene.toml
```

## Output Formats

Query results can be formatted for different consumers:

```bash
# Human-readable (default)
flint query "entities where archetype == 'door'" --scene levels/tavern.scene.toml

# JSON for scripting and AI agents
flint query "entities where archetype == 'door'" --scene levels/tavern.scene.toml --format json

# TOML for configuration workflows
flint query "entities where archetype == 'door'" --scene levels/tavern.scene.toml --format toml
```

## Combining with Shell Tools

JSON output composes with standard tools:

```bash
# Count doors
flint query "entities where archetype == 'door'" --scene levels/tavern.scene.toml --format json | jq length

# Get just the names
flint query "entities where archetype == 'door'" --scene levels/tavern.scene.toml --format json | jq '.[].name'

# Find entities with a specific parent
flint query "entities" --scene levels/tavern.scene.toml --format json | jq '.[] | select(.parent == "main_hall")'
```

## Further Reading

- [Queries](../concepts/queries.md) --- full grammar reference and advanced usage
- [Constraints](../concepts/constraints.md) --- queries used in validation rules
- [CLI Reference](../cli-reference/overview.md) --- all command options
