# Queries

Flint's query language is a simple filter: one resource, and at most one condition on it. `entities where archetype == 'door'` is a whole query. It is parsed by a PEG grammar (pest) and run against the ECS world. Three things use it: `flint query`, the `query` of a [constraint](constraints.md), and the viewer, which evaluates the same constraints.

## Grammar

The query language is defined in `crates/flint-query/src/grammar.pest`:

```
query     = { SOI ~ resource ~ (where_clause)? ~ EOI }
resource  = { "entities" | "components" }
where_clause = { "where" ~ condition }
condition = { field ~ operator ~ value }
field     = { identifier ~ ("." ~ identifier)* }
operator  = { "==" | "!=" | "contains" | ">=" | "<=" | ">" | "<" }
value     = { string | number | boolean }
```

Whitespace is ignored between tokens. The `where` keyword is case-insensitive.

A query is the whole string: anything after the one condition is a parse error. `entities where archetype == 'door' and door.locked == true` does not parse, where it used to be read as its first condition alone with the rest dropped.

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

# Name the schema directories yourself (repeatable). Without the flag, the
# project's flint.toml `schemas`, else schemas/ in the working directory,
# beside the scene and one level up
flint query "entities" --scene levels/tavern.scene.toml --schemas engine/schemas --schemas schemas
```

## What it does not do

The language is one resource and one `field operator value` comparison. It has:

- no `and`, `or` or `not`: a query with one is a parse error, not a query that checks half of what it says;
- no `has <component>` test, though comparing any field of a component selects only entities that carry it;
- no way to reach a parent or a child (`parent.archetype`, "any child where ...");
- no comparison between two fields, and no arithmetic.

A constraint that needs two conditions is written as two constraints, or as a `query` that selects by one and a `kind` that checks the other. A constraint whose query does not parse is an error from `flint validate`, `flint check` and the viewer, naming the constraint, its file and the parse error; it is never skipped.

Queries run on the loaded world, not on TOML text, and scan every entity.

## Further Reading

- [Querying Entities](../getting-started/first-query.md) --- getting started tutorial
- [Constraints](constraints.md) --- using queries in validation rules
- [CLI Reference](../cli-reference/overview.md) --- command-line options
