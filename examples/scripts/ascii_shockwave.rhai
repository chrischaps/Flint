// Shockwaves of text (#859). Every few seconds something lands in the
// clearing and a ring spreads from where it struck. The ring is the style
// field (`field_ring`), and every prop here follows it (`blend_source =
// "field"`), so a prop turns to glyphs as the front passes over it and is
// itself again once the front has gone by. Click anywhere to send one from
// the cursor; hold the right button for a flashlight (`field_cone`) from
// the bottom of the screen that shows what each prop really is.
//
// The field is immediate mode: what a frame draws is that frame's field,
// so the script draws every ring it has every frame, and a frame that draws
// none has no field (the props then draw as at blend 0, their style off).
//
//   0 s     an impact by the barrel; the ring crosses the bench and the bot
//   4 s     one behind the crate
//   8 s     one at the front, under the camera
//   12 s    the same again

let period = 12.0;
// Where the impacts land, in world space, and when within the period
let impacts = [
    #{ at: [2.2, 0.0, 5.0], t: 0.6 },
    #{ at: [1.8, 0.0, 1.8], t: 4.6 },
    #{ at: [-0.4, 0.0, 6.6], t: 8.6 },
];
// The rings out now: where on the screen each started, and when
let waves = [];
let last_t = 0.0;

fn on_update() {
    let drift = total_time() * 0.04;
    set_camera_position(-1.8 + 0.6 * drift.sin(), 1.6, 8.6);
    set_camera_target(0.6, 0.6, 3.6);
}

fn on_draw_ui() {
    let now = total_time();
    let w = screen_width();
    let h = screen_height();

    // A new impact as the period's clock passes its time
    let t = now % period;
    for impact in impacts {
        let crossed = if t >= last_t { last_t < impact.t && t >= impact.t } else { t >= impact.t || last_t < impact.t };
        if crossed {
            let p = world_to_screen(impact.at[0], impact.at[1], impact.at[2]);
            if p.visible {
                waves.push(#{ x: p.x, y: p.y, born: now });
            }
        }
    }
    last_t = t;
    if is_mouse_just_pressed(0) {
        waves.push(#{ x: mouse_x(), y: mouse_y(), born: now });
    }

    // Each ring spreads to past the far corner in 3.4 s, easing out, its
    // front thinning as it goes and fading over its last third
    let reach = (w * w + h * h).sqrt();
    let life = 3.4;
    let kept = [];
    for wave in waves {
        let age = now - wave.born;
        if age < life {
            let s = age / life;
            let radius = reach * (1.0 - (1.0 - s) * (1.0 - s));
            let width = h * (0.42 - 0.22 * s);
            let fade = ((1.0 - s) * 3.0).min(1.0);
            field_ring(wave.x, wave.y, radius, width, h * 0.08, fade);
            // A faint line along the front, so the eye finds it
            draw_circle_outline(wave.x, wave.y, radius, 0.75, 1.0, 0.8, 0.25 * fade, 1.5);
            kept.push(wave);
        }
    }
    waves = kept;

    // The flashlight: from the bottom middle toward the cursor
    if is_mouse_pressed(1) {
        let dx = mouse_x() - w * 0.5;
        let dy = mouse_y() - h;
        // Clockwise from straight up, as the field counts it
        let direction = rad_to_deg(atan2(dx, -dy));
        field_cone(w * 0.5, h, direction, 14.0, h * 1.4, h * 0.06);
    }
}
