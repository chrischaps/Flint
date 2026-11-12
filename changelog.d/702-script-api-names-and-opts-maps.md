### Script functions renamed to one convention, and every draw call takes an opts map (#702)
**What changed.** Names put the verb first and read a settable value with `get_`; aliases and `_ex` variants are gone. Every draw primitive has an opts form under its own name, `draw_rect(x, y, w, h, #{color, rounding, layer})`, a colour being `[r, g, b(, a)]` or `"#rrggbb"`; the positional colour forms stay. New: `touch_id(index)`, since `touch_x`/`touch_y` take a touch's id, not its index. The old names keep working for one release: `flint script api` lists them as deprecated and `flint script check` warns at each call.

| Old | New |
|---|---|
| `this_entity`, `log_info` | `self_entity`, `log` |
| `lod_scale`, `draw_distance_scale` | `get_lod_scale`, `get_draw_distance_scale` |
| `current_scene`, `current_state`, `state_stack` | `get_current_scene`, `get_current_state`, `get_state_stack` |
| `sprite_play`, `sprite_stop`, `sprite_set_speed`, `sprite_is_playing` | `play_sprite`, `stop_sprite`, `set_sprite_speed`, `is_sprite_playing` |
| `camera_follow`, `camera_follow_position`, `camera_follow_set` | `follow_with_camera`, `get_camera_follow_position`, `set_camera_follow_position` |
| `camera_shake`, `camera_shake_stop`, `camera_apply_shake` | `shake_camera`, `stop_camera_shake`, `apply_camera_shake` |
| `draw_text_ex`, `draw_text_stroked(_ex)`, `draw_text_opts` | `draw_text(x, y, text, size, #{color, stroke, layer, ..})` |
| `draw_rect_ex`, `draw_rect_ex4` | `draw_rect(x, y, w, h, #{color, rounding, layer})` (`rounding: [tl, tr, br, bl]`) |
| `draw_rect_gradient` (14 arguments) | `draw_rect_gradient(x, y, w, h, #{from, to, vertical, layer})` |
| `draw_rect_outline_ex`, `draw_circle_ex`, `draw_circle_outline_ex`, `draw_line_ex` | the base name with `#{color, thickness, rounding, layer}` |
| `draw_sprite_ex` | `draw_sprite(x, y, w, h, name, #{uv, color, layer})` |
| `draw_line_3d_ex`, `measure_text_ex`, `set_anim_layer_ex` | `draw_line_3d(.., #{..})`, `measure_text(text, size, #{font, spacing})`, `set_anim_layer(.., #{mode, mask})` |

**What to do.** Run `flint script check` on your scripts and move each warned call to its new name. A script looping over `touch_x(i)` for `i` in `0..touch_count()` should pass `touch_id(i)`.
