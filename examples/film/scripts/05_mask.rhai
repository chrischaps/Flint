// Beat 05, the mask (#263). Only the camera: a slow arc while the input
// script opens F4 and turns the debug view to the style mask and back.

import "film" as film;

fn on_update() {
    let t = total_time();
    film::orbit(0.0, 0.7, 0.0, 5.0, 1.7, film::lerp(0.78, 1.05, t / 9.0));
}
