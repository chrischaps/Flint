# Architecture Decision Records

Flint's commits, the book and CLAUDE.md have cited ADRs by number since
about ADR 0049 ("ADR 0068", "ADR 0110"). Until now, though, the records
existed only in those commit messages and PR descriptions. This folder holds
the records from ADR 0111 on, so the decision and its reasons are kept
in the repository.

- One file per decision: `NNNN-short-slug.md`, numbered after the highest
  ADR cited anywhere in the repo or its git log. Check
  `git log --all --format=%B | grep -o "ADR 0[0-9]*" | sort -u | tail -1`
  before taking a number.
- Sections: Context, Decision, Alternatives considered, Consequences, and
  Measured when there are numbers.
- Cite a record as `ADR NNNN` in code comments, commits and the book, as
  before. Link the book chapter that teaches the feature, and have the
  chapter link back.

| ADR | Decision |
|---|---|
| [0111](0111-stylized-rendering.md) | Stylized rendering: per-entity selection on one screen grid, two stages, the descriptor-driven style framework and the id-only style mask (#260, #261) |
| [0112](0112-user-authored-styles.md) | User-authored styles: `styles/<name>.shader.toml` + one WGSL `fn style`, a generated prelude, naga check with file:line errors, passthrough fallback, append-only registry (#274, #275) |
| [0113](0113-one-glyph-atlas-for-text.md) | One glyph atlas for text: `ui_text` sprite fonts are layout over a `GlyphAtlas` (`builtin`, a `.glyphs.toml` set, or a `.font.toml` PNG grid kept for compatibility), uploaded once as white RGBA with coverage alpha (#283) |
| [0114](0114-project-manifest-and-one-resolver.md) | A project manifest (`flint.toml`) and one resolver: `Project` in flint-core over the vfs, one schema resolver, one asset search rule with a `tried` list (#706) |
| [0115](0115-scene-diagnostics-and-complete-schemas.md) | One list of scene diagnostics from one pass, a schema for every engine component, entity and asset field types, and typed component reads that report (#683) |
| [0116](0116-declared-script-api.md) | A declared script API: one registration builder that records name, argument kinds and doc, coerces numbers, resolves entities, and feeds `flint script api` and the checker (#695) |
| [0117](0117-player-subsystems-and-one-game-step.md) | Player subsystems on named stages, one `enter_scene`, and one windowless `GameCore::step` shared with headless runs (#724) |
| [0118](0118-renderer-owners-for-pipelines-draws-and-frame-inputs.md) | The renderer gets one owner each for sample-count pipelines, draw kinds (`DrawItem`) and per-frame inputs (`FrameInputs`, `bind_scene`) (#736) |
