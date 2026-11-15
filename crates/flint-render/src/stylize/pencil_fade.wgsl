// Pencil's drawing pass as every frame draws it (#879): no drawing on.
// Prepended to pencil.wgsl's module, so the draw-on code is folded out
// at compile time and the pass compiles exactly as it did before it.
const DRAW_ON: bool = false;
