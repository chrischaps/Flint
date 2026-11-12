// Seeded stars (#635). Each second, random() scatters a new sky of stars,
// and keeps the old ones fading. Under `--frame-step` (or `--seed N`) the
// sequence is the same every run, so a capture at any moment is too; with
// neither it comes from the wall clock and every run draws its own sky.
//
// It also asks for an entity the scene doesn't have, the way a scene
// builder that hasn't made it yet would: the setter does nothing, and the
// log says so once.

let stars = [];
let next_sky = 0.0;
let skies = 0;

fn on_init() {
    // No such entity: get_entity gives -1, and this warns once
    let comet = get_entity("comet");
    set_position(comet, 0.0, 10.0, 0.0);
}

fn on_update() {
    let t = total_time();
    if t >= next_sky {
        next_sky += 1.0;
        skies += 1;
        let first = random();
        for i in 0..24 {
            stars.push(#{
                x: random(),
                y: random() * 0.8,
                r: random_range(1.5, 5.0),
                hue: random(),
                born: t,
            });
        }
        log(`sky ${skies}: first draw ${first}`);
        // A comet each sky, still not there
        set_position(get_entity("comet"), random(), 0.0, 0.0);
    }
    // Only the last three skies
    while stars.len() > 72 {
        stars.remove(0);
    }
}

fn on_draw_ui() {
    let sw = screen_width();
    let sh = screen_height();
    let t = total_time();
    draw_rect(0.0, 0.0, sw, sh, 0.02, 0.02, 0.06, 1.0);
    for s in stars {
        let age = t - s.born;
        let a = 1.0 - age / 3.0;
        if a <= 0.0 {
            continue;
        }
        // Warm to cold by hue
        let r = 0.75 + 0.25 * s.hue;
        let g = 0.8;
        let b = 1.0 - 0.3 * s.hue;
        draw_circle(s.x * sw, s.y * sh, s.r * 2.6, r, g, b, 0.12 * a);
        draw_circle(s.x * sw, s.y * sh, s.r, r, g, b, a);
    }
    draw_text(sw * 0.5, sh * 0.88, `sky ${skies}`, 22.0, #{
        color: [0.7, 0.72, 0.9, 1.0],
        align: "center",
        spacing: 3.0,
    });
}
