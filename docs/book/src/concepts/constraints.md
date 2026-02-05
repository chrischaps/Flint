# Constraints

> This page is a stub. Content coming soon.

Constraints are declarative validation rules that define what a correct scene looks like. This page will cover:

- Constraint file format (TOML in `schemas/constraints/`)
- Constraint kinds: `required_component`, `required_child`, `value_range`, `reference_valid`, `query_rule`
- Auto-fix strategies: `add_child`, `set_default`, `remove_invalid`, `assign_from_parent`
- The validation loop: evaluate, fix, re-evaluate, detect cycles
- CLI usage: `flint validate`, `--fix`, `--dry-run`, `--output-diff`
- JSON and text output formats

See the [Writing Constraints](../guides/writing-constraints.md) guide for practical examples.

Quick start:

```bash
flint validate levels/tavern.scene.toml --schemas schemas
flint validate levels/tavern.scene.toml --fix --dry-run
```
