# File Formats

> This page is a stub. Full reference coming soon.

All Flint data formats use TOML. This page will provide a complete reference for:

## Scene Files (`.scene.toml`)

```toml
[scene]
name = "Scene Name"
version = "1.0"

[entities.<name>]
archetype = "<archetype>"
parent = "<parent_name>"

[entities.<name>.<component>]
field = value
```

## Component Schemas (`schemas/components/*.toml`)

```toml
[component.<name>]
description = "..."

[component.<name>.fields]
field_name = { type = "<type>", default = <value> }
```

## Archetype Schemas (`schemas/archetypes/*.toml`)

```toml
[archetype.<name>]
description = "..."
components = ["comp1", "comp2"]

[archetype.<name>.defaults.<component>]
field = value
```

## Constraint Files (`schemas/constraints/*.toml`)

```toml
[[constraint]]
name = "rule_name"
query = "entities where ..."
severity = "error"
message = "..."

[constraint.kind]
type = "<kind>"
```

## Asset Sidecars (`assets/**/*.asset.toml`)

```toml
[asset]
name = "asset_name"
type = "mesh"
hash = "sha256:..."
source_path = "..."
tags = ["tag1", "tag2"]
```
