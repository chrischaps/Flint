# Design Principles

Flint's architecture follows six principles that guide every design decision. They are listed in priority order --- when principles conflict, higher-ranked ones win.

## 1. CLI-First

Every operation is expressible as a composable command. There is no operation that *requires* a GUI. The CLI is the source of truth for what the engine can do.

This means:
- All commands accept flags for output format (`--format json`, `--format toml`)
- Commands compose via pipes and standard shell tooling
- Batch operations are first-class, not afterthoughts
- The viewer is a *consumer* of state, not a *producer* of it

## 2. Introspectable

You can query any aspect of engine state as structured data. Nothing is hidden behind opaque handles or binary blobs.

```bash
# What entities exist?
flint query "entities where archetype == 'door'"

# What does a door look like?
flint schema door

# What would this change break?
flint validate levels/tavern.scene.toml --fix --dry-run
```

The query language is the same whether you're exploring interactively or writing constraint rules. Learn it once, use it everywhere.

## 3. Deterministic

Same inputs always produce identical outputs. No hidden state, no ambient randomness, no order-dependent behavior.

- Entity IDs are stable across save/load cycles
- Procedural generation uses explicit seeds
- Build manifests record exact asset hashes
- Headless renders are reproducible for regression testing

## 4. Text-Based

Scene and asset formats are human-readable, machine-parseable, and diffable. TOML is the primary format throughout.

```toml
[entities.front_door]
archetype = "door"
parent = "main_hall"

[entities.front_door.transform]
position = [5, 0, 0]

[entities.front_door.door]
style = "hinged"
locked = false
```

This isn't just about readability --- it's about *collaboration*. Text files merge cleanly in version control. Diffs are meaningful. AI agents can read and write them directly.

## 5. Constraint-Driven

Declarative rules define what a valid scene looks like. The engine validates against these rules and can optionally auto-fix violations.

Constraints serve multiple roles:
- **Validation** --- catch errors before they become runtime bugs
- **Documentation** --- constraints describe what "correct" means
- **Automation** --- auto-fix rules handle routine corrections
- **Communication** --- constraints are a shared contract between human and AI

## 6. Hybrid Workflows

Humans and AI agents collaborate effectively on the same project. Neither workflow is an afterthought.

The typical loop:
1. An AI agent creates or modifies scene content via CLI
2. Constraints validate the changes automatically
3. A human reviews the result in the viewer
4. Feedback flows back to the agent as structured data

This principle ensures Flint doesn't optimize so hard for agents that humans can't use it, or so hard for humans that agents can't automate it.
