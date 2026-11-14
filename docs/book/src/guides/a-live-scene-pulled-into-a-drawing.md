# A Live Scene Pulled into a Drawing

A technique is proved only in motion. *Understudy* is a 28-second film made in Flint to test whether the rotoscoped-sketch pieces work together (epic #402). A wooden drawing mannequin waits in a clearing at dusk beside an easel with a blank page. It walks to the page and touches it, the picture tears open there, and it walks into a world traced in pencil, where it moves on twos as the lines do. Then the live world is wiped back over it, and a last panel draws the mannequin in pencil, stroke by stroke: it came back, mostly.

The scene is `examples/understudy.scene.toml`, with two scripts: `scripts/understudy.rhai` (the beats, the walk and the camera) and `scripts/understudy_hud.rhai` (the panels' ink and the captions). The engine supplies these pieces, and the rest is scene and script:

| Piece | What it does here | Reference |
|---|---|---|
| The `rotoscope` look (#404, #887) | the drawn world: ink-forward pencil on a blank sheet, shaded by the light, lines boiling 12 times a second | [Looks](../concepts/stylize-passes.md#looks-in-scenes-f4-and-scripts) |
| `motion_hold` (#405) | the figure drawn on twos while it is inside the drawing | [Stylize Passes](../concepts/stylize-passes.md) |
| The reveal (#406, #876) | the tear at the page with a torn sheet's white rim and its shadow, and the wipe back | [Revealing a whole-frame style](../concepts/stylize-passes.md#revealing-a-whole-frame-style) |
| Comic panels (#880) | the opening page of three panels, and the inset at the end with its own camera and look | [Comic Panels](../concepts/comic-panels.md) |
| Pencil `transition = "draw"` (#879) | the inset's drawing grows on, outlines first, then hatching | [Stylize Passes](../concepts/stylize-passes.md) |
| Pencil `clutter` (#883) | the meadow draws as a hatched sheet with a horizon line, not a black band of blades | [Stylize Passes](../concepts/stylize-passes.md) |

<video controls loop muted playsinline poster="../images/understudy-poster.jpg" width="960">
  <source src="../images/understudy.mp4" type="video/mp4">
</video>

*The whole loop at 960x540. Captured with `flint-player examples/understudy.scene.toml --width 1280 --height 720 --frame-step 0.0416667 --screenshot-every 0.0416667 --screenshot frames/f.png --exit-after 28.05 --pixels-per-point 1` on an RTX 4070, and encoded with ffmpeg. Nothing in it is played by hand.*

## The beats

| | |
|---|---|
| ![A comic page of three panels: the live clearing at dusk with the mannequin and the easel, a close shot of the blank page, and the mannequin's head and shoulders, with two captions](../images/understudy-live.png) | ![The tear has spread out from the page and the mannequin stands inside it, drawn in pencil, while the edges of the frame are still live](../images/understudy-tear.png) |
| **0 s, the page.** Three panels: the clearing, the blank page, the mannequin as it looks round. At 3.4 s the page cuts to one panel. | **7 s, the crossing.** It touched the page at 6 s, the tear burst open there, and from 6.6 s it is drawn on twos. |
| ![The whole frame in pencil: the meadow as a hatched band under a horizon line, the bench, the easel and the mannequin mid-stride, with captions](../images/understudy-drawn.png) | ![A ragged wipe has brought the live frame back on the right, over the crate and lantern; the left is still pencil](../images/understudy-wipe.png) |
| **13 s, the drawn world.** It walks round the clearing on paper. | **20 s, pulled back.** A wipe brings the live frame in from the right. |
| ![Live again: the mannequin in front of the easel, and an inset panel in the corner where the same mannequin is drawn in pencil, captioned MOSTLY](../images/understudy-mostly.png) | |
| **25.5 s, mostly.** An inset panel follows the mannequin in the rotoscope look, its drawing grown on over 1.6 s. | |

Each beat is one call made on the frame it starts, kept in a beat number on the director's `story` component so it fires once:

```rhai
// The touch: the tear starts where the page is on screen
let s = world_to_screen(page.x, page.y, page.z);
set_post_style_param("pencil", "reveal_x", s.x / screen_width());
set_post_style_param("pencil", "reveal_y", s.y / screen_height());
tween_post_style_param("pencil", "reveal", 1.0, 5.0, "ease_out");

// Inside the drawing it is drawn as the lines are: on twos
set_motion_hold(get_entity("mq"), 12.0);
```

The scene's pencil table starts at `reveal = 0`, so until the touch the frame is the live one, byte for byte. The look is named in `[post_process]` and lies under the scene's own keys, so the live side keeps its bloom, SSAO and vignette while the pencil takes the rotoscope's settings. The look also puts the whole frame on twos; the scene's `motion_hold = 0` takes that back, so the live world moves smoothly and only the figure is held.

The pages are `set_panels` calls. The inset at the end names the look and a pencil table of its own, and the director moves its `blend` each frame, the front that the drawing grows up to:

```rhai
set_panels(#{ gutter: 18.0, border: 0.0, gutter_color: [0.97, 0.96, 0.93], panel: #{
    wide: #{ rect: [0.0, 0.0, 1.0, 1.0] },
    inset: #{ rect: [0.63, 0.5, 0.34, 0.46], order: 1, follow: "mq",
              position: [0.75, 1.62, 1.75], target: [0.0, 1.42, 0.0], fov: 32.0,
              look: "rotoscope",
              post_process: #{ styles: #{ pencil: #{ reveal: 1.0, transition: "draw", blend: 0.0 } } } },
} });
// each frame after: the front moves on
set_panel("inset", #{ post_process: #{ styles: #{ pencil: #{ reveal: 1.0, transition: "draw", blend: b } } } });
```

The inset's table sets `reveal = 1`. The scene's pencil is at `reveal = 0` by then, where it starts and where the wipe ended, and a style at reveal 0 draws nothing.

## What made it work

- **A rip bursts, then spreads.** With `ease_in_out` nothing showed for a second and a half after the touch. With `ease_out` over 5 s the tear is visible at once and still reaches the corners slowly.
- **A blank page hides its own tear.** The tear starts on the page, and the pencil draws a blank page as blank paper, so its first frames are white on white. The "RRRIP!" lettering marks the moment until the tear passes the page's edge.
- **The figure is spheres.** A drawing mannequin is the right actor for a drawn world, and the examples have no walk clip. It is 23 scaled `Sphere` models hung from 13 joint pivots under one root. The director turns the pivots each frame from the distance walked: hip swing, knee bend through the swing, arms against legs, a pelvis bob twice a stride. The hold sits on the root, so every joint below steps on the same ticks as the boil.
- **The meadow came back.** The first cut of the film had to keep its grass sparse and near the camera, because the pencil inked every blade as a silhouette and a dense meadow drew as one black band. Pencil's `clutter` (#883), which the rotoscope look turns on, now fades a contour where lines already crowd round it, so the film has the examples' full meadow (density 40, out to 70 m) and it draws as a hatched sheet.
- **Ink is the HUD's, gutters are the engine's.** The pages set `border = 0` and the HUD draws each panel's outline from the same rects, by the panels' rule (the page is the frame less half a gutter all round, each panel its share less half a gutter), with corners that move on each tick. A one-panel stretch has no `[panels]` at all and the HUD draws that border too, so every cut keeps one hand. The inset gets a margin of paper round its ink, as a printed inset does.

## The page is drawn by the HUD

The panels' ink, the outer gutter, the caption boxes and the sound lettering are draw calls in `understudy_hud.rhai`, on the same 28-second clock as the director. Each caption names the panel it sits in. There is no engine code. The lettering is Patrick Hand (`examples/fonts/PatrickHand-Regular.ttf`, SIL Open Font License), which the HUD finds by its file stem:

```rhai
draw_text(x, y, "EVERY LINE WAS STILL DECIDING WHAT IT WOULD BE.", 27.0,
    #{ font: "PatrickHand-Regular", color: [0.09, 0.08, 0.07, 1.0], layer: 22 });
```

Each box and each panel's border is a polygon whose corners move a point or so on each of the 12 ticks a second, so the page is redrawn by the same hand as the pencil and the figure. `flint render` runs no HUD, but it does draw panels, so stills of a beat without captions or ink come from `flint render examples/understudy.scene.toml --scripts --time 13 --no-grid -o drawn.png`. For the captions, capture with `flint-player` as above.

## Making your own

The film keeps to the technique and takes nothing from any existing work: its set is the examples' clearing, its figure and story are original, and it has no music. To make another, start from the scene:

1. Put a whole-frame style on at `reveal = 0`, under a look if you like.
2. Script the beats as one-shot calls: `reveal_shape`, `reveal_x`/`reveal_y` from `world_to_screen`, then `tween_post_style_param` on `reveal`.
3. Hold what crosses over with `set_motion_hold(id, 12.0)`, and clear it when it comes back.
4. Lay out pages with `set_panels`, and give a panel its own look to show both worlds at once.
5. Draw the ink and captions in an `on_draw_ui`, and capture with `--frame-step` so every run is the same film.
