# ADR 0113: One glyph atlas for sprite text and the ASCII styles

Status: accepted, 2026-11-09 (#283)

## Context

After #264, `flint-render` had two ways to turn a font into glyphs on the GPU:

- `bitmap_font::BitmapFont`, read from a `.font.toml` naming a PNG grid
  (`glyph_width`, `glyph_height`, `columns`, `first_char`, `char_count`,
  `spacing`). `ui_text` sprites used it. It mapped a character to a UV rect
  in the PNG, and the model loader put the PNG in the `TextureCache` like any
  image.
- `glyph_atlas::GlyphSet` / `GlyphAtlas`, read from a `*.glyphs.toml` naming
  a TTF/OTF, a PNG grid, hand-drawn pixel text or the built-in 8x8 set. It
  packs an `R8Unorm` coverage atlas with a gutter round each cell and measures
  each glyph's ink. The ASCII styles used it.

Both cut PNG grids and both map characters to UV rects, so the same job was
done twice. Only the second could read a TTF or the built-in font, and a
`.font.toml` couldn't name either.

## Decision

`BitmapFont` is now text layout over a `GlyphAtlas`, and nothing else. Every
glyph source the engine reads produces a `GlyphAtlas`, and both the ASCII
styles and sprite text draw from one.

- `ui_text.font` names any of:
  - `builtin`;
  - a glyph set, by name (`fonts/<name>.glyphs.toml` beside the scene, then
    in its parent) or by a `.glyphs.toml` path. These are the ASCII styles'
    rules: the bitmap font asks a `GlyphSetCache` for the set.
  - a `.font.toml`, kept as the compatibility path. Its PNG grid is cut by
    `GlyphAtlas::from_grid_image` into the same kind of atlas. It can also
    name a set (`glyphs = "<name>"`), which keeps `spacing` available for a
    TTF.
- The atlas uploads once, from `SceneRenderer::load_ui_font` while the scene's
  textures load, as white RGBA with the coverage as alpha. It goes into the
  `TextureCache` under `font:<what ui_text named>`. Sprite text still batches
  through the sprite2d pipeline, so no shader or pipeline was added.
- Extraction only lays text out (`BitmapFont::layout_text`). The font was
  loaded and uploaded with the scene, so a per-frame `layout_text` call
  reads no files and creates no GPU objects. A font that fails warns once,
  is cached as a failure, and draws nothing.

## Alternatives considered

- **Keep both and document why.** The only difference left would have been
  the texture format: RGBA from a file against R8 coverage. That doesn't
  justify keeping a second grid cutter and a second UV mapper, and a scene's
  HUD text still couldn't use the font its ASCII look draws in.
- **Sample the `R8Unorm` atlas from a new sprite text pipeline.** That would
  save three bytes a texel. But sprite text atlases are small (the demo's is
  under 70 KB as RGBA), and it would cost a pipeline, a bind group layout and
  a shader. Expanding to RGBA once at load keeps sprite text a sprite.
- **Drop `.font.toml`.** `demo/ui_test` and game repos use it, and as a
  wrapper it costs one small function.

## Consequences

- World and HUD sprite text can use a TTF, the built-in 8x8 set or a
  hand-drawn set, the same files an ASCII look uses.
- A `.font.toml`'s grid now gets the atlas's one-pixel gutter. A linearly
  filtered glyph no longer picks up its neighbour's edge in the PNG.
- `BitmapFont`'s public grid fields (`glyph_width`, `columns`, `first_char`,
  ...) and the unused `char_widths` are gone. `glyph_uv(ch)` and
  `layout_text(...)` no longer take texture dimensions, because the atlas
  knows its own size.
- `.font.toml` PNGs resolve the way image textures do: beside the scene, in
  its `textures/`, in the parent and its `textures/`, then beside the font
  file. Image `overrides` don't apply to them.

Book: [Glyph Atlas](../book/src/concepts/glyph-atlas.md), "Sprite text".
