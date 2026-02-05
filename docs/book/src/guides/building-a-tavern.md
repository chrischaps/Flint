# Building a Tavern

> This page is a stub. Full tutorial coming soon.

A step-by-step tutorial that builds a complete tavern scene from scratch using only CLI commands. This guide will cover:

- Project initialization with `flint init`
- Building rooms with parent-child hierarchies
- Placing doors with component properties
- Furnishing spaces with tables, chairs, and decorations
- Adding characters (bartender, patrons, a mysterious stranger)
- Adding physics colliders for walkable geometry
- Creating a player entity with first-person controls
- Running queries to inspect the scene
- Validating against constraints
- Viewing the result with `flint serve --watch`
- Walking through the tavern with `flint play`

This tutorial follows the same steps as the `demo/build-tavern.ps1` showcase script, explained in detail.

In the meantime, you can explore the finished result:

```bash
# View the tavern in the scene viewer
cargo run --bin flint -- serve demo/phase4_runtime.scene.toml --watch

# Walk through the tavern in first person
cargo run --bin flint -- play demo/phase4_runtime.scene.toml
```

The `demo/phase4_runtime.scene.toml` scene contains a three-room tavern with a main hall, kitchen, and storage room, furnished with a bar counter, tables, fireplace, and barrels. Four NPCs populate the space: a bartender, two patrons, and a mysterious stranger. All surfaces have physics colliders for first-person exploration.
