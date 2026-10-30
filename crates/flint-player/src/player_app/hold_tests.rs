//! `PlayerApp::hold_at` (#102): with a fixed frame step, game time stops at
//! the held second and stays there, frame after frame. Headless: no window.

use super::PlayerApp;
use flint_ecs::FlintWorld;

#[test]
fn game_time_stops_at_hold_at() {
    let mut app = PlayerApp::new(
        FlintWorld::new(),
        "hold_test.scene.toml".into(),
        false,
        None,
        None,
    );
    app.clock.frame_step = Some(0.25);
    app.hold_at = Some(0.5);

    // The first tick only starts the clock; then 0.25 a frame.
    let mut times = Vec::new();
    for _ in 0..6 {
        app.tick();
        times.push(app.clock.total_time);
    }
    assert_eq!(times, [0.0, 0.25, 0.5, 0.5, 0.5, 0.5]);
    assert_eq!(app.clock.delta_time, 0.0, "held frames step by nothing");
}
