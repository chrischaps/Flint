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
