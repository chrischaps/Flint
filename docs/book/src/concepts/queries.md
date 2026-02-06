# Queries

Flint's query system provides a SQL-inspired language for filtering and inspecting entities. Queries are parsed by a PEG grammar (pest) and executed against the ECS world.

## Grammar

The query language is defined in `crates/flint-query/src/grammar.pest`:

```
query     = { resource ~ (where_clause)? }
resource  = { "entities" | "components" }
where_clause = { "where" ~ condition }
condition = { field ~ operator ~ value }
field     = { identifier ~ ("." ~ identifier)* }
operator  = { "==" | "!=" | "contains" | ">=" | "<=" | ">" | "<" }
value     = { string | number | boolean }
```

Whitespace is ignored between tokens. The `where` keyword is case-insensitive.

## Resources

Two resource types can be queried:

| Resource | Description |
|----------|-------------|
| `entities` | Returns entity data (name, archetype, components) |
| `components` | Returns component definitions from the schema registry |

## Operators

| Operator | Description | Value Types |
|----------|-------------|-------------|
| `==` | Exact equality | string, number, boolean |
| `!=` | Not equal | string, number, boolean |
| `>` | Greater than | number |
| `<` | Less than | number |
| `>=` | Greater than or equal | number |
| `<=` | Less than or equal | number |
| `contains` | Substring match | string |

## Field Paths

Fields use dot notation to access nested values:

| Pattern | Meaning |
|---------|---------|
| `archetype` | The entity's archetype name |
| `name` | The entity's name |
| `door.locked` | The `locked` field of the `door` component |
| `transform.position` | The `position` field of the `transform` component |

Examples:

```bash
# Top-level entity properties
flint query "entities where archetype == 'door'"
flint query "entities where name contains 'wall'"

# Component fields
flint query "entities where door.locked == true"
flint query "entities where audio_source.volume > 0.5"
flint query "entities where material.roughness >= 0.8"
flint query "entities where collider.shape == 'box'"
```

## Value Types

| Type | Syntax | Examples |
|------|--------|---------|
| String | Single or double quotes | `'door'`, `"wall"` |
| Number | Integers or decimals, optional negative | `42`, `3.14`, `-1.5` |
| Boolean | Unquoted keywords | `true`, `false` |

## Use in Constraints

Queries power the constraint system. Each constraint rule includes a `query` field that selects which entities the constraint applies to:

```toml
[[constraint]]
name = "doors_have_transform"
query = "entities where archetype == 'door'"
severity = "error"
message = "Door '{name}' is missing a transform component"

[constraint.kind]
type = "required_component"
archetype = "door"
component = "transform"
```

The query selects all door entities, and the constraint checks that each one has a transform component. See [Constraints](constraints.md) for details.

## CLI Usage

```bash
# Basic query
flint query "entities where archetype == 'door'" --scene levels/tavern.scene.toml

# JSON output for machine consumption
flint query "entities" --scene levels/tavern.scene.toml --format json

# TOML output
flint query "entities where door.locked == true" --scene levels/tavern.scene.toml --format toml

# Specify schemas directory
flint query "entities" --scene levels/tavern.scene.toml --schemas schemas
```

## Limitations

- Conditions are currently single-clause (one field-operator-value comparison per query at the parser level)
- Boolean combinators (`and`, `or`, `not`) are part of the grammar design but not yet implemented in the parser
- Queries operate on in-memory ECS state, not directly on TOML files
- Performance is linear in entity count (queries scan all entities matching the resource type)

## Further Reading

- [Querying Entities](../getting-started/first-query.md) --- getting started tutorial
- [Constraints](constraints.md) --- using queries in validation rules
- [CLI Reference](../cli-reference/overview.md) --- command-line options
