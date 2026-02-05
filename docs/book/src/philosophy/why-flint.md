# Why Flint?

## The Problem

Game engines today --- Unity, Unreal, Godot --- are designed around visual editors. You drag objects into scenes, connect nodes in graphs, click through property inspectors. These workflows are excellent for humans using a mouse, but they create friction in two growing scenarios:

1. **AI agents building game content.** When an AI coding agent needs to place a door in a scene, it shouldn't need to simulate mouse clicks on a GUI. It should issue a command and get structured feedback.

2. **Automation and CI pipelines.** Validating a scene, running regression tests on visual output, or batch-processing hundreds of entities --- these tasks fight against editor-centric architectures.

The core tension: existing engines treat programmatic access as a *secondary* concern. The API exists, but it's bolted onto a system designed for spatial interaction. Scene formats are binary or semi-readable. Introspection is limited. Determinism is not guaranteed.

## The Thesis

Flint starts from the opposite assumption: **the primary interface is CLI and code**. Visual tools are for *validation*, not *creation*.

This doesn't mean Flint is hostile to humans. It means every operation flows through a composable, scriptable interface first. If you can do it in the CLI, you can automate it. If you can automate it, an AI agent can do it. The viewer is the place where a human confirms: "Yes, that's what I wanted."

## What This Enables

### For AI agents

An agent working with Flint has a clean contract:
- Issue CLI commands, get structured JSON/TOML responses
- Query any aspect of engine state with a SQL-inspired language
- Validate work against declarative constraint rules
- Produce visual artifacts (headless renders) for verification

No simulated GUI interaction. No screen scraping. No ambiguous visual state.

### For humans

A developer working with Flint gets:
- Scene files that are human-readable TOML, easily diffable in git
- A query language for exploring what's in a scene without opening an editor
- Constraint rules that serve as living documentation of what a "correct" scene looks like
- A hot-reload viewer that updates in real-time as files change

### For teams

A team using Flint gets:
- Deterministic builds --- same inputs always produce identical outputs
- Text-based formats that merge cleanly in version control
- Structured output for CI pipelines and automated testing
- A shared vocabulary between human developers and AI tools

## Comparison

| Aspect | Traditional Engines | Flint |
|--------|-------------------|-------|
| Primary interface | GUI editor | CLI |
| Scene format | Binary or semi-text | TOML (fully text) |
| Programmatic API | Secondary | Primary |
| Introspection | Limited | Full (query language) |
| Deterministic builds | Generally no | Yes |
| AI-agent optimized | No | Yes |
| Validation | Runtime errors | Declarative constraints |

## The Name

Flint is a tool for starting fires. Simple, reliable, fundamental. Strike it and something sparks into existence. That's the idea: minimal friction between intent and result.
