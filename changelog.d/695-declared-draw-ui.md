### Draw calls, UI, sprites, the 2D camera and touch take integers; `draw_rect_ex(.., 0.0)` works (#695)
**What changed.** The UI Draw API (`draw_text*`, `draw_rect*`, `draw_circle*`,
`draw_line*`, `draw_sprite*`, rings, arcs, polygons, clips, `measure_text*`,
`screen_width`/`height`), the world draws (`draw_line_3d*`, `draw_world_line`,
`draw_world_text`), the screen-to-world queries (`world_to_screen`,
`screen_to_ground`, `screen_to_mesh`, `screen_to_heightfield`,
`ground_height`, `raycast_meshes`), the Data-Driven UI functions, the
screen-UI setters (`set_text`, `set_text_color`, `ui_set_value`,
`set_anchor`, `set_anchor_offset`), the Sprite API and sprite animator,
`camera_follow` and the shake, the touch functions, `move_character`,
`get_collider_extents` and `find_nearest_interactable` are declared like the
Entity API. A number takes an int (`draw_rect(10, 10, 100, 50, 1, 1, 1, 1)`)
and a `layer` or other int takes a whole float (`draw_text_ex(.., 0.0)`),
where both used to be "Function not found". A draw call with all-int numbers
is as fast as one with floats; a call mixing the two works but is slower in a
tight loop (the book's scripting page, API Reference). The sprite, screen-UI,
`camera_follow`, `move_character` and `get_collider_extents` functions now
warn once for `-1` or a despawned id, where they did nothing silently.
`find_nearest_interactable()` from a script's top level is a script error
("there is no world here") instead of a panic. The book's `ui_get_rect` keys
are corrected to `#{x, y, w, h}`, which is what it always returned.
**What to do.** Nothing. `.to_float()` calls added only to satisfy a draw call
can go. A new warning names a call on a missing entity: check it with
`entity_exists(id)` first.
