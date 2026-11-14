# Archive

Plans and designs whose work has shipped or been dropped, kept for their history: why a system is shaped the way it is, what was tried, what was measured. None of them describes the engine as it is now. Each opens with a banner naming the page that does, usually in [the book](https://docs.chaps.dev/flint/).

What lives elsewhere:

- **The engine as it is**: the book, `docs/book/src/`.
- **Decisions**: `docs/adr/`, one file per numbered record.
- **Living design notes**: `docs/design/`, documents still being written against or measured by (the web port, the showcase film, Living Props targets, spike results).
- **Work to do**: GitHub issues. A plan here never holds an open item: when a plan moves to the archive, each of its unchecked items that is still true becomes an issue first (#756 did this for `TODO_REMAINING_PLAN.md`).

| Document | Was | Current |
|---|---|---|
| `flint-design-doc.md` | The design document, v0.7 (March 2026, Phase 7 in progress) | Architecture Overview, Roadmap |
| `PHASE4_ROADMAP.md`, `PHASE5_ROADMAP.md` | The runtime and AI-asset phase plans, both complete | Physics and Runtime, AI Asset Generation |
| `phase2-constraints-and-assets.md` | Phase 2's design (February 2026) | Constraints, Assets |
| `TODO_REMAINING_PLAN.md` | An audit's pending list | #721, #968, #969, #970 (its banner says which item went where) |
| `INPUT_EXTENSIBILITY_DESIGN.md` | Input bindings, rebinding and pads; implemented | Physics and Runtime: input |
| `flint-procgen-design.md`, `flint-procgen-implementation-plan.md` | `flint-procgen`'s draft design and task plan | Procedural Generation |
| `plans/` | The Kuwahara filter's design and plan (March), the Living Props spike (November) | Post-Processing, Procedural Generation |
| `superpowers/plans/`, `superpowers/specs/` | March plans and specs: terrain culling, terrain grass, the architecture explorer, the grass debug overlay, the stats overlay, the water system | Rendering, Terrain, Crate Dependency Graph, Debug Panels, Profiling, Ocean |

The `superpowers/` documents are written as instructions to an agent ("REQUIRED SUB-SKILL: ..."). They are not instructions to anyone now.
