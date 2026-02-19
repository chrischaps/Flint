//! Shared 3D projection math for the viewer — world-to-screen projection,
//! screen-to-world ray unprojection, and geometric intersection utilities.
//!
//! Used by both the spline editor and the transform gizmo.

use flint_render::Camera;

/// Project a world-space point to screen coordinates.
/// Returns None if the point is behind the camera.
pub fn world_to_screen(
    camera: &Camera,
    screen_size: [f32; 2],
    pos: [f32; 3],
) -> Option<egui::Pos2> {
    let vp = camera.view_projection_matrix();
    let clip_x = vp[0][0] * pos[0] + vp[1][0] * pos[1] + vp[2][0] * pos[2] + vp[3][0];
    let clip_y = vp[0][1] * pos[0] + vp[1][1] * pos[1] + vp[2][1] * pos[2] + vp[3][1];
    let clip_w = vp[0][3] * pos[0] + vp[1][3] * pos[1] + vp[2][3] * pos[2] + vp[3][3];

    if clip_w <= 0.001 {
        return None;
    }

    let ndc_x = clip_x / clip_w;
    let ndc_y = clip_y / clip_w;

    Some(egui::pos2(
        (ndc_x + 1.0) * 0.5 * screen_size[0],
        (1.0 - ndc_y) * 0.5 * screen_size[1],
    ))
}

/// Compute a world-space ray from a screen pixel coordinate.
/// Returns (origin, normalized direction).
pub fn screen_to_world_ray(
    camera: &Camera,
    screen_size: [f32; 2],
    sx: f32,
    sy: f32,
) -> ([f32; 3], [f32; 3]) {
    let inv_vp = camera.inverse_view_projection_matrix();

    let ndc_x = (sx / screen_size[0]) * 2.0 - 1.0;
    let ndc_y = 1.0 - (sy / screen_size[1]) * 2.0;

    let near = mat4_transform_point(&inv_vp, [ndc_x, ndc_y, -1.0]);
    let far = mat4_transform_point(&inv_vp, [ndc_x, ndc_y, 1.0]);

    let dx = far[0] - near[0];
    let dy = far[1] - near[1];
    let dz = far[2] - near[2];
    let len = (dx * dx + dy * dy + dz * dz).sqrt();

    let dir = if len > 1e-8 {
        [dx / len, dy / len, dz / len]
    } else {
        [0.0, 0.0, -1.0]
    };

    (near, dir)
}

/// Transform a 3D point by a 4x4 column-major matrix (with perspective divide).
pub fn mat4_transform_point(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    let x = m[0][0] * p[0] + m[1][0] * p[1] + m[2][0] * p[2] + m[3][0];
    let y = m[0][1] * p[0] + m[1][1] * p[1] + m[2][1] * p[2] + m[3][1];
    let z = m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2] + m[3][2];
    let w = m[0][3] * p[0] + m[1][3] * p[1] + m[2][3] * p[2] + m[3][3];

    if w.abs() < 1e-10 {
        [x, y, z]
    } else {
        [x / w, y / w, z / w]
    }
}

/// Compute the distance from a ray to a point.
pub fn ray_point_distance(ray_origin: [f32; 3], ray_dir: [f32; 3], point: [f32; 3]) -> f32 {
    let v = [
        point[0] - ray_origin[0],
        point[1] - ray_origin[1],
        point[2] - ray_origin[2],
    ];
    let t = (v[0] * ray_dir[0] + v[1] * ray_dir[1] + v[2] * ray_dir[2]).max(0.0);
    let closest = [
        ray_origin[0] + ray_dir[0] * t,
        ray_origin[1] + ray_dir[1] * t,
        ray_origin[2] + ray_dir[2] * t,
    ];
    let dx = closest[0] - point[0];
    let dy = closest[1] - point[1];
    let dz = closest[2] - point[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Compute the view-space depth of a point (for draw ordering).
pub fn point_depth(camera: &Camera, pos: [f32; 3]) -> f32 {
    let view = camera.view_matrix();
    let z = view[0][2] * pos[0] + view[1][2] * pos[1] + view[2][2] * pos[2] + view[3][2];
    -z
}

/// Intersect a ray with a plane defined by normal and distance from origin.
/// Returns the intersection point, or None if ray is parallel to the plane.
pub fn ray_plane_intersect(
    ray_o: [f32; 3],
    ray_d: [f32; 3],
    plane_n: [f32; 3],
    plane_d: f32,
) -> Option<[f32; 3]> {
    let denom = plane_n[0] * ray_d[0] + plane_n[1] * ray_d[1] + plane_n[2] * ray_d[2];
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (plane_d
        - plane_n[0] * ray_o[0]
        - plane_n[1] * ray_o[1]
        - plane_n[2] * ray_o[2])
        / denom;
    if t < 0.0 {
        return None;
    }
    Some([
        ray_o[0] + ray_d[0] * t,
        ray_o[1] + ray_d[1] * t,
        ray_o[2] + ray_d[2] * t,
    ])
}
