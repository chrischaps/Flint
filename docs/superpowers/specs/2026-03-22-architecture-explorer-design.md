# Architecture Explorer — Design Spec

## Overview

A web-based interactive visualization tool for exploring the Flint engine's crate structure, module hierarchy, and public API surface. Two components: a Rust analyzer that generates a JSON data snapshot at build time, and a static Cytoscape.js web app that renders the interactive graph.

**Audience:** Engine author (deep architectural analysis) and contributors/users (onboarding and orientation).

## System Architecture

### Component 1: Data Analyzer (`tools/arch-analyzer/`)

A Rust binary that statically analyzes the Flint workspace and outputs a single `arch-data.json` file.

**Responsibilities:**
- Parse each crate's `Cargo.toml` to extract internal (`flint-*`) dependencies
- Walk each crate's `src/` tree using `syn` to extract module structure and public API items
- Collect metrics: line count per module, external dependency count per crate
- Compute dependency tiers via topological sort (tier 0 = no internal deps, higher = deeper in the graph)
- Serialize everything to `arch-data.json`

**Invocation:** `cargo run -p flint-arch-analyzer` — writes to `tools/arch-viewer/arch-data.json` by default.

**Key dependencies:** `toml` (Cargo.toml parsing), `syn` (Rust AST parsing), `serde`/`serde_json` (output serialization), `walkdir` (source tree traversal).

### Component 2: Web Viewer (`tools/arch-viewer/`)

A static web app — single `index.html` + `app.js` + `style.css` — that loads `arch-data.json` and renders the interactive graph using Cytoscape.js.

**No build step required.** Open `index.html` directly in a browser or serve from any static host.

## Data Model

```json
{
  "generated_at": "2026-03-22T14:30:00Z",
  "crates": [
    {
      "name": "flint-core",
      "path": "crates/flint-core",
      "description": "EntityId, ContentHash, Transform, Vec3, Color, FlintError",
      "lines": 2450,
      "external_deps": ["toml", "serde", "glam"],
      "internal_deps": [],
      "tier": 0,
      "modules": [
        {
          "name": "transform",
          "path": "src/transform.rs",
          "lines": 180,
          "public_items": [
            {
              "kind": "struct",
              "name": "Transform",
              "members": [
                { "name": "position", "type": "Vec3" },
                { "name": "rotation", "type": "Quat" },
                { "name": "scale", "type": "Vec3" }
              ]
            },
            {
              "kind": "fn",
              "name": "from_toml",
              "members": [
                { "name": "value", "type": "&toml::Value" },
                { "name": "return", "type": "Transform" }
              ]
            }
          ],
          "children": []
        }
      ]
    }
  ],
  "edges": [
    { "from": "flint-ecs", "to": "flint-core" }
  ]
}
```

**Key design decisions:**
- **Tiers** computed automatically from dependency depth — not hardcoded.
- **Modules** are nested via `children` to reflect `mod` trees (directories with submodules).
- **Public items** include structs, enums, traits, and functions. Each has a `members` array for drill-down:
  - Structs → fields (name + type)
  - Enums → variants (name + optional payload type)
  - Traits → method signatures
  - Functions → parameters + return type
- **Edges** are crate-level only, kept as a flat list for easy graph construction.

## UI Design

### Three-Panel Layout

1. **Left Toolbar (220px)** — search, filters, layout switcher, tools
2. **Center Canvas (flex)** — Cytoscape.js graph with zoom/pan/drag
3. **Right Detail Panel (260px)** — context for selected element

### Left Toolbar

- **Search:** Fuzzy-find across crate names, module names, type names. Results highlight matching nodes in the graph.
- **Tier Filter:** Toggle buttons per tier (T0 Core, T1 ECS, T2 Scene, T3 Systems, T4+). Each tier has a distinct color. Toggling hides/shows crates at that tier.
- **Layout Switcher:** Three layout algorithms:
  - Hierarchical (default) — tiers flow top-to-bottom
  - Force-directed — organic clustering by connectivity
  - Concentric — tiers as concentric rings
- **Tools:**
  - Path Finder — select two nodes, highlight shortest dependency path between them
  - Metrics Overlay — scale node size by line count
  - Dependency Explorer — select a node, highlight all upstream ("what does this depend on?") or downstream ("what depends on this?") transitive dependencies

### Center Canvas — Interaction Model

- **Click crate node** → expand to show child modules as nodes inside a compound (dashed border) container
- **Click module node** → populate detail panel with public API items
- **Click public item in detail panel** → show members (struct fields, fn signature, etc.)
- **Hover edge** → highlight the edge and show dependency info in the detail panel
- **Drag nodes** → rearrange layout manually
- **Scroll** → zoom in/out
- **Drag canvas background** → pan

### Right Detail Panel

Context-sensitive based on selection:
- **Crate selected:** description, internal/external deps, line count, tier
- **Module selected:** file path, line count, list of public items (color-coded by kind)
- **Item selected:** full signature, members/fields, type information
- **Edge selected:** which crate depends on which, and what it uses

Clickable dependency links in the panel navigate the graph (centers + highlights the target node).

### Visual Encoding

- **Node color** = tier (green for T0, blue for T1, purple for T2, amber for T3, red for T4+)
- **Node size** = line count (when metrics overlay active)
- **Border thickness** = number of dependents (more depended upon = thicker)
- **Edge opacity** = dims non-selected paths when a node or path is focused
- **Expanded crates** = dashed compound border containing module child nodes

### Color Scheme

Dark theme matching Flint's development aesthetic:
- Background: `#0f0f1a`
- Panel background: `#1a1a2e`
- Tier 0 (Core): `#4ade80` (green)
- Tier 1 (ECS/Schema): `#60a5fa` (blue)
- Tier 2 (Scene/Import): `#a78bfa` (purple)
- Tier 3 (Systems): `#fbbf24` (amber)
- Tier 4+ (Aggregators): `#f87171` (red)

## Analyzer Implementation

### Modules

- **`main.rs`** — CLI entry point. Locates workspace root, orchestrates analysis, writes JSON output.
- **`cargo_parser.rs`** — Reads each crate's `Cargo.toml`, extracts `flint-*` internal dependencies and external dependency names. Reads crate description from `[package]` if present.
- **`source_parser.rs`** — Uses `syn` to parse `.rs` files. Walks the module tree by following `mod` declarations and directory structure. Extracts public items (`pub fn`, `pub struct`, `pub enum`, `pub trait`) with their members (struct fields, enum variants, trait methods, function parameters/return types).
- **`metrics.rs`** — Line counts per file. Tier computation via topological sort of the dependency graph.
- **`model.rs`** — Serde-serializable data structures matching the JSON schema above.

### Parsing Strategy

1. Start from each crate's `src/lib.rs` (or `src/main.rs`)
2. Parse with `syn::parse_file`
3. Walk items: for each `pub` item, extract name, kind, and members
4. For `mod foo;` declarations, resolve to `src/foo.rs` or `src/foo/mod.rs` and recurse
5. Build the nested module tree with `children` arrays

## File Structure

```
tools/
  arch-analyzer/
    Cargo.toml              # deps: toml, syn (full features), serde, serde_json, walkdir
    src/
      main.rs
      cargo_parser.rs
      source_parser.rs
      metrics.rs
      model.rs
  arch-viewer/
    index.html              # Single page app shell
    app.js                  # Cytoscape graph + UI logic
    style.css               # Dark theme, tier colors, panel layout
    arch-data.json          # Generated (gitignored)
```

## Deployment

- **Local:** `cargo run -p flint-arch-analyzer` then open `tools/arch-viewer/index.html` in a browser. No server needed.
- **GitHub Pages:** CI runs the analyzer and deploys `tools/arch-viewer/` as a static site (with generated `arch-data.json` included).
- **Freshness:** `generated_at` timestamp displayed in the UI footer. Stale data is immediately visible.
- **Cytoscape.js** loaded from CDN (`<script>` tag) with a vendored fallback for offline use.
