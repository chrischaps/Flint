### A reveal mask for whole-frame styles (#406)

Every style's `[post_process.styles.<name>]` table takes a **reveal**: the style shows only inside a screen-space region, the frame it read shows outside, and a drawn line runs along the edge between them. `reveal = 0..1` grows the region from nothing to the whole frame; `reveal_shape` is `tear` (a ragged hole ripping open), `wipe`, `iris` or `panel` (a comic-panel rectangle); `reveal_x`, `reveal_y`, `reveal_angle`, `reveal_ragged`, `reveal_scale`, `reveal_edge` (points), `reveal_ink`, `reveal_boil` and `reveal_seed` tune it. `--style pencil:reveal=0.5` and `--sweep pencil.reveal=0,0.5,1` take it too, and `--style-time` steps the edge's boil deterministically.

New script call: `tween_post_style_param(style, param, target, seconds[, easing])` eases a whole-frame style's number (`reveal`, `blend`, a `reveal_*` key or a parameter) and fires `post_style_tween_done` with `#{ style, param }`. One call tears a live scene into a style: see `examples/reveal_tear.scene.toml`.

What to do: nothing, unless a user style names a parameter `reveal` or `reveal_*`. Those names are now reserved, and such a style fails to load; rename the parameter.
