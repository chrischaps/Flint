### A tweened per-entity ASCII `cell` no longer piles up GPU objects (#507)
**What changed.** Since #507 a per-entity ASCII variant with screen bounds
gets its own band of cells even at `footprint` 1, and that band's textures
were sized exactly to its grid. A script tweening the entity's `cell` (with
`set_style_param`) changed the grid every few frames, and each new size
remade both variant-cell textures and kept one more bind group over them for
the rest of the run (24 animated frames kept 4). The textures now only grow,
as the dissolve's do: a smaller grid, or a frame with none, keeps the size
they have. Nothing on screen changes.
**What to do.** Nothing.
