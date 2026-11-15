// Pencil's drawing pass while a blend draws on (#879), prepended to
// pencil.wgsl's module: its own pipeline, drawn only in frames where some
// row's front is below 1 (pencil.rs picks it).
const DRAW_ON: bool = true;
