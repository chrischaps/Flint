### Whole-frame ASCII over a few styled props costs less (#507)
**What changed.** With `[post_process.styles.ascii]` covering the frame and some
entities carrying an `ascii_style`, the style reads the style mask only near
those entities' screen bounds, and its tone statistics and per-cell pre-pass
work only there. On an RTX 4070 at 1080p (`examples/ascii_entities`, edges
off) a frame whose styled props are all `enabled = false` went from 0.066 to
0.057 ms (the plain frame is 0.056), and the demo with its normalised barrel
from 0.112 to 0.088 ms. Every pixel is the same as before but one fix: with
more than 255 per-entity ASCII variants and `footprint` 2 or more, a variant
past the 255th sharing a grid with an earlier one drew some cells smooth; it
is drawn in glyphs now. A Rust style can
ask the same question: `StyleFrame::mask_rect(pad)` is where the mask can hold
any of the style's ids, opt-outs included, and `variant_rect(id, pad)` one
id's; both are `None` when that could be anywhere.
**What to do.** Nothing.
