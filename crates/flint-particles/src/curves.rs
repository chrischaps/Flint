//! Simple value-over-lifetime interpolation (start → end linear)

/// Linear interpolation between two floats
pub fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Linear interpolation between two RGBA colors
pub fn lerp_color(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        lerp_f32(a[0], b[0], t),
        lerp_f32(a[1], b[1], t),
        lerp_f32(a[2], b[2], t),
        lerp_f32(a[3], b[3], t),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_f32_endpoints() {
        assert!((lerp_f32(0.0, 10.0, 0.0) - 0.0).abs() < 1e-6);
        assert!((lerp_f32(0.0, 10.0, 1.0) - 10.0).abs() < 1e-6);
        assert!((lerp_f32(0.0, 10.0, 0.5) - 5.0).abs() < 1e-6);
    }

    #[test]
    fn lerp_color_midpoint() {
        let white = [1.0, 1.0, 1.0, 1.0];
        let black = [0.0, 0.0, 0.0, 0.0];
        let mid = lerp_color(white, black, 0.5);
        for c in &mid {
            assert!((*c - 0.5).abs() < 1e-6);
        }
    }
}
