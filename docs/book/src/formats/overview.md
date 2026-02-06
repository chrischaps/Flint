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

## Animation Clips (`demo/animations/*.anim.toml`)

```toml
name = "clip_name"
duration = 0.8

[[tracks]]
interpolation = "Linear"       # "Step", "Linear", or "CubicSpline"

[tracks.target]
type = "Rotation"              # "Position", "Rotation", "Scale", or "CustomFloat"

[[tracks.keyframes]]
time = 0.0
value = [0.0, 0.0, 0.0]       # [x, y, z] (euler degrees for rotation)

[[tracks.keyframes]]
time = 0.8
value = [0.0, 90.0, 0.0]
# in_tangent = [...]           # Optional, for CubicSpline
# out_tangent = [...]

[[events]]                     # Optional timed events
time = 0.0
event_name = "door_start"
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
