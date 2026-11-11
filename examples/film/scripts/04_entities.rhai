// Beat 04, some things in type (#266). The camera trucks across the
// still life while the barrel turns on its stand, so its object-grid
// glyphs travel with it and the screen-grid props' glyphs don't.

import "film" as film;

fn on_update() {
    let t = total_time();
    if film::portrait() {
        // From the barrel's far side, where the barrel, the figure and the
        // crate stand closest together across the narrow frame
        film::orbit_portrait(0.0, 0.6, -0.3, 7.6, 2.15,
            film::lerp(1.66, 1.92, film::ease(t / 9.0)), 44.0);
    } else {
        film::dolly([4.2, 1.45, 2.4], [2.2, 1.35, 4.0], [0.4, 0.7, -0.3], [-0.4, 0.75, -0.2],
            film::ease(t / 9.0));
    }
    // A quarter turn over the beat
    set_rotation(get_entity("barrel"), 0.0, -20.0 + 10.0 * t, 0.0);
}
