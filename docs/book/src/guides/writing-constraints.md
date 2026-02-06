# Writing Constraints

This guide walks through authoring constraint rules for your Flint project. Constraints are declarative TOML rules that define what a valid scene looks like, checked by `flint validate`.

## Anatomy of a Constraint File

Constraint files live in `schemas/constraints/` and contain one or more `[[constraint]]` entries:

```toml
[[constraint]]
name = "unique_identifier"
description = "Human-readable explanation"
query = "entities where <condition>"
severity = "error"
message = "Violation message for '{name}'"

[constraint.kind]
type = "<kind>"
# kind-specific fields...
```

- **name** --- unique identifier, used in logs and JSON output
- **description** --- what the rule checks (for documentation)
- **query** --- which entities this constraint applies to
- **severity** --- `"error"` fails validation, `"warning"` is advisory
- **message** --- shown when violated. `{name}` is replaced with the entity name

## Choosing the Right Kind

### `required_component` --- "Entity X must have component Y"

The most common kind. Use when an archetype needs a specific component:

```toml
[[constraint]]
name = "doors_have_transform"
description = "Every door must have a transform"
query = "entities where archetype == 'door'"
severity = "error"
message = "Door '{name}' is missing a transform component"

[constraint.kind]
type = "required_component"
archetype = "door"
component = "transform"
```

### `value_range` --- "Field X must be between A and B"

Validates that a numeric field is within bounds:

```toml
[[constraint]]
name = "door_angle_range"
description = "Door open angle must be between 0 and 180"
query = "entities where archetype == 'door'"
severity = "warning"
message = "Door '{name}' has an invalid open_angle"

[constraint.kind]
type = "value_range"
field = "door.open_angle"
min = 0.0
max = 180.0
```

### `required_child` --- "Entity X must have a child of archetype Y"

Enforces parent-child relationships:

```toml
[[constraint]]
name = "rooms_have_door"
description = "Every room needs at least one exit"
query = "entities where archetype == 'room'"
severity = "error"
message = "Room '{name}' has no door"

[constraint.kind]
type = "required_child"
archetype = "room"
child_archetype = "door"
```

### `reference_valid` --- "This reference field must point to an existing entity"

Checks referential integrity:

```toml
[[constraint]]
name = "door_target_exists"
description = "Door target room must exist"
query = "entities where archetype == 'door'"
severity = "error"
message = "Door '{name}' references a non-existent target"

[constraint.kind]
type = "reference_valid"
field = "door.target_room"
```

### `query_rule` --- "This query must return the expected count"

The most flexible kind, for arbitrary rules:

```toml
[[constraint]]
name = "one_player"
description = "Playable scenes need exactly one player"
query = "entities where archetype == 'player'"
severity = "error"
message = "Scene must have exactly one player entity"

[constraint.kind]
type = "query_rule"
rule_query = "entities where archetype == 'player'"
expected = "exactly_one"
```

## Auto-Fix Strategies

Add a `[constraint.fix]` section to enable automatic repair:

```toml
[[constraint]]
name = "rooms_have_bounds"
query = "entities where archetype == 'room'"
severity = "error"
message = "Room '{name}' needs bounds"

[constraint.kind]
type = "required_component"
archetype = "room"
component = "bounds"

[constraint.fix]
strategy = "set_default"
```

Available strategies:
- **set_default** --- add the missing component with schema defaults
- **add_child** --- create a child entity of the required archetype
- **remove_invalid** --- remove entities that violate the rule
- **assign_from_parent** --- copy a value from the parent entity

## Testing Constraints

Always test with `--dry-run` first to preview changes:

```bash
# See what violations exist
flint validate levels/tavern.scene.toml --schemas schemas

# Preview auto-fix changes without applying
flint validate levels/tavern.scene.toml --fix --dry-run

# Apply fixes
flint validate levels/tavern.scene.toml --fix
```

JSON output gives machine-readable results for CI:

```bash
flint validate levels/tavern.scene.toml --format json
```

## Organizing Constraint Files

Group related constraints into files by topic:

```
schemas/constraints/
├── basics.toml          # Fundamental rules (transform required, etc.)
├── physics.toml         # Physics constraints (collider sizes, mass ranges)
├── audio.toml           # Audio constraints (volume ranges, spatial settings)
└── gameplay.toml        # Game-specific rules (one player, door connectivity)
```

All `.toml` files in `schemas/constraints/` are loaded automatically.

## Cascade Detection

When auto-fix modifies one entity, it might cause a different constraint to fail. Flint handles this by running fix-validate cycles. If a cycle is detected (the same violation keeps appearing), the fixer stops and reports the issue.

## Further Reading

- [Constraints](../concepts/constraints.md) --- constraint system reference
- [Queries](../concepts/queries.md) --- query syntax used in constraint selectors
- [File Formats](../formats/overview.md) --- constraint TOML format
