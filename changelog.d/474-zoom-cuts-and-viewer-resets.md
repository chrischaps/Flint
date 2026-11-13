### A zoom-only camera cut resets style history, and so do the viewer's jumps (#474)
**What changed.** The player's camera-cut detector, which resets the history
of styles that remember frames (ASCII's steady glyphs), now also watches
zoom: an FOV or orthographic-height step of more than 1.25x in one frame,
four times faster than the frame before, is a cut, and so is any switch
between perspective and orthographic. A punch-in no longer carries the wide
shot's glyphs into the close one (2,974 of 14,400 cells at 2x on
`examples/ascii_cuts.scene.toml`, now 0). `flint edit` now resets style
history at Space, a projection switch and a hot reload: before, a fifth of
the viewport kept the old view's glyphs for as long as you looked.
**What to do.** Nothing. A script that tweens FOV smoothly keeps its
history. One that steps the FOV by more than 1.25x at once as an effect, not
a cut, and wants the glyphs held calls `set_camera_cut_detection(false)`.
