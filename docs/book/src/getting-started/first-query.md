# Querying Entities

> This page is a stub. Content coming soon.

Flint includes a SQL-inspired query language for filtering and inspecting entities. This page will cover:

- Basic query syntax: `entities where <condition>`
- Comparison operators: `==`, `!=`, `>`, `<`, `>=`, `<=`, `contains`
- Querying by archetype, component values, and nested fields
- Output formats: JSON and TOML
- Combining queries with shell tools (`jq`, pipes)

For a full reference, see [Queries](../concepts/queries.md).

Quick example:

```bash
flint query "entities where archetype == 'door'" --scene levels/tavern.scene.toml
flint query "entities where door.locked == true" --scene levels/tavern.scene.toml
```
