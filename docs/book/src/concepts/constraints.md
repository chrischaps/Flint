# Constraints

Constraints are declarative validation rules that define what a correct scene looks like. They live in TOML files under `schemas/constraints/` and are checked by `flint validate`, which also checks a scene's [`[wooze]` table](procgen.md#the-wooze-field) against its schema.

## Constraint File Format

Each constraint file can contain multiple `[[constraint]]` entries:

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

| Field | Description |
|-------|-------------|
| `name` | Unique identifier for the constraint |
| `description` | Human-readable explanation of what the rule checks |
| `query` | Flint query that selects which entities this constraint applies to |
| `severity` | `"error"` (blocks) or `"warning"` (advisory) |
| `message` | Violation message. `{name}` is replaced with the entity name |

## Constraint Kinds

### `required_component`

Ensures that entities matching the query have a specific component:

```toml
[constraint.kind]
type = "required_component"
archetype = "door"
component = "transform"
```

Use case: every door must have a position in the world.

### `required_child`

Ensures that entities have a child entity of a specific archetype:

```toml
[constraint.kind]
type = "required_child"
archetype = "room"
child_archetype = "door"
```

Use case: every room must have at least one door.

### `value_range`

Checks that a numeric field falls within a valid range:

```toml
[constraint.kind]
type = "value_range"
field = "door.open_angle"
min = 0.0
max = 180.0
```

Use case: door angles must be physically possible.

### `reference_valid`

Checks that an entity reference field points to an existing entity:

```toml
[constraint.kind]
type = "reference_valid"
field = "door.target_room"
```

Use case: a door's target room must actually exist in the scene.

### `query_rule`

The most flexible kind --- validates that a query returns the expected number of results:

```toml
[constraint.kind]
type = "query_rule"
rule_query = "entities where archetype == 'player'"
expected = "exactly_one"
```

Use case: a playable scene must have exactly one player entity.

## Auto-Fix Strategies

Some constraint violations can be fixed automatically. The `fix` section defines how:

- **set_default** --- set a missing field to its schema default
- **add_child** --- create a child entity with the required archetype
- **remove_invalid** --- remove entities that violate the constraint
- **assign_from_parent** --- copy a field value from the parent entity

Auto-fix runs in a loop: fix violations, re-validate, fix new violations. Cycle detection prevents infinite loops.

## CLI Usage

```bash
# Check a scene for violations
flint validate levels/tavern.scene.toml

# JSON output for parsing
flint validate levels/tavern.scene.toml --format json

# Preview what auto-fix would change
flint validate levels/tavern.scene.toml --fix --dry-run

# Apply auto-fixes
flint validate levels/tavern.scene.toml --fix

# Specify a schemas directory
flint validate levels/tavern.scene.toml --schemas schemas

# A field no schema declares is an error, not a warning
flint validate levels/tavern.scene.toml --strict
```

`flint validate` checks two things: the scene against its schemas (an unknown archetype, component or field, a value of the wrong type; see [Scene Diagnostics](schemas.md#scene-diagnostics)), then the constraints. The schema check runs whether or not any constraint file exists. The exit code is 0 if neither finds an error, 1 if either does. Warnings do not affect the exit code. With `--format json` the document has both lists (`diagnostics` and `violations`), and `valid`, `errors` and `warnings` count both.

`--fix` writes only what its fixes changed: a field set in place (its comment kept), a field taken out, a new child entity appended at the end of the file. The rest of the file is left as written, its `[scene]` name, `[camera]`, `[environment]`, `[post_process]`, `[living]`, `[wooze]`, prefabs, comments and key order included. A fix to an entity a prefab instance expands into has nowhere in the scene file to go: it's printed as `not written` and the prefab is left alone. `--output-diff` prints the line diff of the file itself.

## Real Example

From `schemas/examples/constraints/basics.toml`, the three rules that come with the [examples' room-builder layer](schemas.md#the-examples-layer). The engine's own set, `schemas/`, ships no constraints: a game writes its own.

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

[[constraint]]
name = "rooms_have_bounds"
description = "Every room must have a bounds component"
query = "entities where archetype == 'room'"
severity = "error"
message = "Room '{name}' is missing a bounds component"

[constraint.kind]
type = "required_component"
archetype = "room"
component = "bounds"

[[constraint]]
name = "door_angle_range"
description = "Door open angle must be between 0 and 180 degrees"
query = "entities where archetype == 'door'"
severity = "warning"
message = "Door '{name}' has an open_angle outside the valid range"

[constraint.kind]
type = "value_range"
field = "door.open_angle"
min = 0.0
max = 180.0
```

## Further Reading

- [Writing Constraints](../guides/writing-constraints.md) --- practical guide to authoring rules
- [Queries](queries.md) --- the query language used in constraint selectors
- [File Formats](../formats/overview.md) --- constraint file format reference
