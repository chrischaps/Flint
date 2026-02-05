# AI Agent Workflow

> This page is a stub. Content coming soon.

A guide for AI agents (and their developers) working with Flint. This guide will cover:

- The agent interaction loop: create, validate, query, render
- Structured I/O: JSON output for machine parsing
- Using queries for state inspection
- Constraint validation as automated feedback
- Headless rendering for visual verification
- Example: an agent building a complete scene from a text description
- Error handling patterns for agent workflows
- Best practices for deterministic scene construction

Example agent workflow:

```bash
# 1. Create scene and entities
flint scene create levels/dungeon.scene.toml --name "Dungeon Level 1"
flint entity create --archetype room --name "entrance" --scene levels/dungeon.scene.toml ...

# 2. Validate
flint validate levels/dungeon.scene.toml --format json

# 3. Query to verify
flint query "entities where archetype == 'door'" --scene levels/dungeon.scene.toml --format json

# 4. Render a preview
flint render levels/dungeon.scene.toml --output preview.png
```
