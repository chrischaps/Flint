//! `screen_to_ground` from a script, against a real `Camera`: a marker the
//! script places at the hit must project straight back under the cursor.

use flint_core::Vec3;
use flint_render::Camera;
use flint_script::ScriptSystem;
use rhai::{Array, Dynamic, Map};

/// The logical window. The host feeds scripts the logical size and the
/// logical cursor, whatever the display's scale factor.
const W: f32 = 1280.0;
const H: f32 = 720.0;

fn camera(pitch_deg: f32, orthographic: bool) -> Camera {
    let mut cam = Camera {
        target: Vec3::new(-40.0, 0.0, 65.0),
        distance: 90.0,
        pitch: pitch_deg.to_radians(),
        yaw: 0.6,
        far: 3000.0,
        aspect: W / H,
        orthographic,
        // Small enough that the near plane stays above the ground at 20°.
        ortho_height: if orthographic { 25.0 } else { 0.0 },
        ..Camera::default()
    };
    cam.update_orbit();
    if pitch_deg == 90.0 {
        // Exactly overhead, the case the view basis has to special-case.
        cam.position = Vec3::new(cam.target.x, cam.distance, cam.target.z);
    }
    cam
}

fn system_seeing(cam: &Camera) -> ScriptSystem {
    let mut sys = ScriptSystem::new();
    sys.set_camera_view_proj(cam.view_projection_matrix());
    sys.set_screen_size(W, H);
    sys
}

fn eval(sys: &mut ScriptSystem, src: &str) -> Dynamic {
    sys.rhai_engine_mut()
        .eval::<Dynamic>(src)
        .unwrap_or_else(|e| panic!("{src}: {e}"))
}

#[test]
fn a_marker_at_the_hit_lands_under_the_cursor() {
    for ortho in [false, true] {
        for pitch in [20.0, 45.0, 70.0, 90.0] {
            let cam = camera(pitch, ortho);
            let mut sys = system_seeing(&cam);
            for (mx, my) in [
                (640.0, 360.0),
                (90.0, 700.0),
                (1200.0, 650.0),
                (400.0, 260.0),
            ] {
                sys.set_mouse_position(mx, my);
                let hit = eval(
                    &mut sys,
                    "let g = screen_to_ground(mouse_x(), mouse_y(), 0.0);
                     if g == () { () } else { [g, world_to_screen(g[0], 0.0, g[1])] }",
                );
                let label = format!("ortho {ortho} pitch {pitch} at ({mx}, {my})");
                assert!(!hit.is_unit(), "{label}: no ground under the cursor");
                let pair = hit.cast::<Array>();
                let ground = pair[0].clone().cast::<Array>();
                let screen = pair[1].clone().cast::<Map>();

                // The same answer the Rust API gives.
                let rust = cam.screen_to_ground(mx, my, W, H, 0.0).unwrap();
                let gx = ground[0].as_float().unwrap() as f32;
                let gz = ground[1].as_float().unwrap() as f32;
                assert!(
                    (gx - rust[0]).abs() < 1e-3 && (gz - rust[1]).abs() < 1e-3,
                    "{label}"
                );

                // And back under the cursor, to a hundredth of a point.
                let sx = screen["x"].as_float().unwrap() as f32;
                let sy = screen["y"].as_float().unwrap() as f32;
                assert!(
                    (sx - mx).abs() < 0.01 && (sy - my).abs() < 0.01,
                    "{label}: marker drawn at ({sx}, {sy})"
                );
            }
        }
    }
}

#[test]
fn sky_and_raised_planes() {
    // A shallow perspective view: the top of the window is sky.
    let mut sys = system_seeing(&camera(8.0, false));
    sys.set_mouse_position(640.0, 5.0);
    assert!(eval(&mut sys, "screen_to_ground(mouse_x(), mouse_y(), 0.0)").is_unit());

    // A plane above the ground: the hit sits where that plane crosses the
    // cursor ray, so it projects back under the cursor at its own height.
    let mut sys = system_seeing(&camera(50.0, false));
    sys.set_mouse_position(700.0, 420.0);
    let back = eval(
        &mut sys,
        "let g = screen_to_ground(mouse_x(), mouse_y(), 2.5); world_to_screen(g[0], 2.5, g[1])",
    )
    .cast::<Map>();
    assert!((back["x"].as_float().unwrap() - 700.0).abs() < 0.01);
    assert!((back["y"].as_float().unwrap() - 420.0).abs() < 0.01);
}
