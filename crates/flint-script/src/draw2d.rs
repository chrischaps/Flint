//! Pure 2D geometry helpers behind the script draw API.
//!
//! Kept free of egui so they can be unit-tested here; the player turns the
//! resulting point lists into `egui::Mesh` / `egui::Shape`s.

use rhai::{Array, Dynamic};

/// Tessellation steps for an annular sector: about one per 3°, clamped to
/// `[6, 180]` so tiny arcs still look round and full rings stay cheap.
pub fn ring_steps(start_deg: f32, end_deg: f32) -> usize {
    let sweep = ring_sweep(start_deg, end_deg);
    ((sweep / 3.0).ceil() as usize).clamp(6, 180)
}

/// Clamp a ring's sweep to `[0, 360]` degrees (`end < start` draws nothing).
pub fn ring_sweep(start_deg: f32, end_deg: f32) -> f32 {
    (end_deg - start_deg).clamp(0.0, 360.0)
}

/// Points of an annular sector as a triangle strip: `2 * (steps + 1)`
/// entries alternating inner, outer along the arc.
///
/// Angles are degrees measured from 12 o'clock, clockwise on screen
/// (0 = up, 90 = right, 180 = down). The sweep `end_deg - start_deg` is
/// clamped to 360; `r_inner` may be 0 for a pie slice.
pub fn ring_points(
    cx: f32,
    cy: f32,
    r_inner: f32,
    r_outer: f32,
    start_deg: f32,
    end_deg: f32,
    steps: usize,
) -> Vec<(f32, f32)> {
    let steps = steps.max(1);
    let sweep = ring_sweep(start_deg, end_deg);
    let mut out = Vec::with_capacity(2 * (steps + 1));
    for i in 0..=steps {
        let deg = start_deg + sweep * (i as f32 / steps as f32);
        let (s, c) = deg.to_radians().sin_cos();
        // 12 o'clock is -y on screen; clockwise sweeps toward +x.
        out.push((cx + r_inner * s, cy - r_inner * c));
        out.push((cx + r_outer * s, cy - r_outer * c));
    }
    out
}

/// Triangle indices for the strip `ring_points` produces.
pub fn ring_indices(steps: usize) -> Vec<u32> {
    let steps = steps.max(1);
    let mut idx = Vec::with_capacity(steps * 6);
    for i in 0..steps as u32 {
        let a = 2 * i; // inner i
        let b = a + 1; // outer i
        let c = a + 2; // inner i+1
        let d = a + 3; // outer i+1
        idx.extend_from_slice(&[a, b, c, b, d, c]);
    }
    idx
}

/// Parse a Rhai array of points for `draw_polygon`. Accepts either an array
/// of `[x, y]` pairs or a flat `[x0, y0, x1, y1, ...]` list; every element
/// may be an INT or a FLOAT. Returns `None` on any malformed entry or an
/// odd flat length.
pub fn parse_points(arr: &Array) -> Option<Vec<[f32; 2]>> {
    if arr.is_empty() {
        return Some(Vec::new());
    }
    if arr[0].is_array() {
        arr.iter()
            .map(|d| {
                let pair = d.clone().try_cast::<Array>()?;
                if pair.len() != 2 {
                    return None;
                }
                Some([dyn_f32(&pair[0])?, dyn_f32(&pair[1])?])
            })
            .collect()
    } else {
        if arr.len() % 2 != 0 {
            return None;
        }
        arr.chunks(2)
            .map(|c| Some([dyn_f32(&c[0])?, dyn_f32(&c[1])?]))
            .collect()
    }
}

fn dyn_f32(d: &Dynamic) -> Option<f32> {
    if let Some(f) = d.clone().try_cast::<f64>() {
        Some(f as f32)
    } else {
        d.clone().try_cast::<i64>().map(|i| i as f32)
    }
}

/// Intersect two `[x, y, w, h]` rects; an empty result collapses to zero size.
pub fn intersect_rect(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let x0 = a[0].max(b[0]);
    let y0 = a[1].max(b[1]);
    let x1 = (a[0] + a[2]).min(b[0] + b[2]);
    let y1 = (a[1] + a[3]).min(b[1] + b[3]);
    [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn ring_points_count_and_endpoints() {
        // Quarter ring from 12 o'clock to 3 o'clock, 10 steps.
        let pts = ring_points(100.0, 100.0, 20.0, 40.0, 0.0, 90.0, 10);
        assert_eq!(pts.len(), 2 * 11);
        // First pair sits straight up (0° = 12 o'clock).
        assert!(
            close(pts[0].0, 100.0) && close(pts[0].1, 80.0),
            "{:?}",
            pts[0]
        );
        assert!(
            close(pts[1].0, 100.0) && close(pts[1].1, 60.0),
            "{:?}",
            pts[1]
        );
        // Last pair sits straight right (90° = 3 o'clock, clockwise).
        let n = pts.len();
        assert!(close(pts[n - 2].0, 120.0) && close(pts[n - 2].1, 100.0));
        assert!(close(pts[n - 1].0, 140.0) && close(pts[n - 1].1, 100.0));
        // Midpoint of the arc is at 45°.
        let mid = pts[10];
        assert!(close(mid.0, 100.0 + 20.0 * 0.5f32.sqrt()));
        assert!(close(mid.1, 100.0 - 20.0 * 0.5f32.sqrt()));
    }

    #[test]
    fn ring_sweep_clamps_to_full_circle() {
        let pts = ring_points(0.0, 0.0, 0.0, 10.0, 0.0, 720.0, 4);
        let n = pts.len();
        // A clamped 360° sweep ends where it started.
        assert!(close(pts[1].0, pts[n - 1].0) && close(pts[1].1, pts[n - 1].1));
        assert_eq!(ring_steps(0.0, 720.0), 120);
        assert_eq!(ring_steps(0.0, 1.0), 6);
        assert_eq!(ring_steps(0.0, 30.0), 10);
        assert_eq!(ring_sweep(90.0, 0.0), 0.0);
    }

    #[test]
    fn ring_indices_cover_strip() {
        let idx = ring_indices(3);
        assert_eq!(idx.len(), 18);
        assert_eq!(*idx.iter().max().unwrap(), 7);
        assert_eq!(&idx[..6], &[0, 1, 2, 1, 3, 2]);
    }

    fn arr(items: Vec<Dynamic>) -> Array {
        items.into_iter().collect()
    }

    #[test]
    fn parse_points_nested_pairs_mixed_numbers() {
        let a = arr(vec![
            Dynamic::from(arr(vec![Dynamic::from(1_i64), Dynamic::from(2.5_f64)])),
            Dynamic::from(arr(vec![Dynamic::from(3.0_f64), Dynamic::from(4_i64)])),
        ]);
        assert_eq!(parse_points(&a), Some(vec![[1.0, 2.5], [3.0, 4.0]]));
    }

    #[test]
    fn parse_points_flat_list() {
        let a = arr(vec![
            Dynamic::from(0_i64),
            Dynamic::from(0_i64),
            Dynamic::from(10.0_f64),
            Dynamic::from(0.0_f64),
            Dynamic::from(5_i64),
            Dynamic::from(8.0_f64),
        ]);
        assert_eq!(
            parse_points(&a),
            Some(vec![[0.0, 0.0], [10.0, 0.0], [5.0, 8.0]])
        );
    }

    #[test]
    fn parse_points_rejects_malformed() {
        let odd = arr(vec![
            Dynamic::from(1.0_f64),
            Dynamic::from(2.0_f64),
            Dynamic::from(3.0_f64),
        ]);
        assert_eq!(parse_points(&odd), None);
        let bad_pair = arr(vec![Dynamic::from(arr(vec![Dynamic::from(1.0_f64)]))]);
        assert_eq!(parse_points(&bad_pair), None);
        let text = arr(vec![Dynamic::from("x"), Dynamic::from("y")]);
        assert_eq!(parse_points(&text), None);
        assert_eq!(parse_points(&Array::new()), Some(Vec::new()));
    }

    #[test]
    fn intersect_rect_nested_and_disjoint() {
        assert_eq!(
            intersect_rect([0.0, 0.0, 100.0, 100.0], [50.0, 50.0, 100.0, 100.0]),
            [50.0, 50.0, 50.0, 50.0]
        );
        let r = intersect_rect([0.0, 0.0, 10.0, 10.0], [20.0, 20.0, 10.0, 10.0]);
        assert_eq!(r[2], 0.0);
        assert_eq!(r[3], 0.0);
    }
}
