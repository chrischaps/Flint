// Beat 07, fade and dissolve (#278). Only the blends move:
//
//   0.8-3.8  the barrel dissolves into glyphs
//   2.6-5.0  the figure cross-fades into blocks
//   6.6-9.4  both go back the way they came

import "film" as film;

fn on_update() {
    let t = total_time();
    if film::portrait() {
        // From the barrel's far side: the barrel near, the figure beyond
        // the table, both inside the narrow frame
        film::orbit_portrait(0.0, 0.6, -0.3, 7.4, 2.0, film::lerp(1.68, 1.9, t / 10.0), 44.0);
    } else {
        film::orbit(0.0, 0.75, -0.2, 4.2, 1.35, film::lerp(0.62, 0.92, t / 10.0));
    }
    set_style_param(get_entity("barrel"), "blend", film::pulse(t, 0.8, 3.0, 9.4, 2.8));
    set_style_param(get_entity("figure"), "blend", film::pulse(t, 2.6, 2.4, 9.4, 2.4));
}
