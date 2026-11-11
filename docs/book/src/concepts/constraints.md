# Constraints

Constraints are declarative validation rules that define what a correct scene looks like. They live in TOML files under `schemas/constraints/` and are checked by `flint validate` for one scene (which also checks the scene's [`[wooze]` table](procgen.md#the-wooze-field) against its schema), by [`flint check`](../cli-reference/overview.md#the-check-command) for every scene of a project, and by the viewer as you edit.

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
| `query` | Flint [query](queries.md) that selects which entities this constraint applies to: one resource and at most one condition. It must parse and select `entities` |
| `severity` | `"error"` (blocks) or `"warning"` (advisory) |
| `message` | Violation message. `{name}` is replaced with the entity name |

A constraint whose `query` does not parse is an **error**, wherever constraints are evaluated. It names the constraint, the file and the parse error:

```
[ERROR] [constraint]: constraint 'lit_props_have_a_place' (schemas/constraints/rules.toml): its query `entities where archetype == 'prop' and lamp.brightness > 0` can't be evaluated: Parse error: ...
```

Such a constraint used to be skipped without a word, so a typo in a query passed every scene. A constraint file that is not valid TOML is an error naming the file too.

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

A second query, `rule`, run once for each entity the constraint's `query` selects. The entity is in violation when `rule` selects nothing:

```toml
[constraint.kind]
type = "query_rule"
rule = "entities where archetype == 'player'"
```

Use case: a scene with a spawn point must have a player entity. `rule` is a query like any other: it must parse and select entities, or the constraint is an error.

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

`flint validate` checks two things: the scene against its schemas (an unknown archetype, component or field, a value of the wrong type, an entity reference to nothing; see [Scene Diagnostics](schemas.md#scene-diagnostics)), then the constraints. The scene is loaded and checked whether or not any constraint file exists, so a scene that does not load fails in a project with no constraints at all. The exit code is 0 if neither finds an error, 1 if either does. Warnings do not affect the exit code. `--format` is `text` or `json` (anything else is refused). With `--format json` the document has both lists (`diagnostics` and `violations`), and `valid`, `errors` and `warnings` count both.

`--fix` and `--dry-run` follow the same rule: the exit code says whether the scene file, as the command left it, still has an error. `--fix` exits 1 if an error-level violation remains that no fix covered; `--dry-run` writes nothing, so it exits as `flint validate` alone would. With `--format json` they print the same document with a `fix` object added (`dry_run`, `iterations`, `actions` with `constraint` / `entity` / `strategy` / `description`, `remaining_violations`, `cycle_detected`, `saved`, `not_written`, `diff`), and `violations` are those of the file as the run left it.

`flint validate` does not look for the files a scene names. [`flint check`](../cli-reference/overview.md#the-check-command) does, for one scene or for the whole project, and runs the constraints as well.

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
