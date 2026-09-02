//! Rendering statistics collected per frame

/// All rendering metrics for a single frame.
///
/// The `collect_stats()` method on `SceneRenderer` populates the draw call
/// and triangle fields. Timing (`fps`, `frame_time_ms`) and `resolution`
/// are set by the caller (player or viewer) since they own the timing source.
#[derive(Debug, Clone, Default)]
pub struct RenderStats {
    // Timing (set by caller)
    pub fps: f32,
    pub frame_time_ms: f32,
    // Totals
    pub draw_calls: u32,
    pub triangles: u32,
    // Per-system breakdown
    pub entity_draws: u32,
    pub skinned_draws: u32,
    pub terrain_draws: u32,
    pub terrain_total_chunks: u32,
    pub transparent_draws: u32,
    pub billboard_draws: u32,
    pub particle_draws: u32,
    pub particle_instances: u32,
    pub sprite_batches: u32,
    pub grass_instances: u32,
    pub grass_draw_calls: u32,
    // Shadow pass (estimated)
    pub shadow_draw_calls: u32,
    pub shadow_triangles: u32,
    // Screen info (set by caller)
    pub resolution: [u32; 2],
}

/// Format a count for display: 1234 → "1.2K", 1234567 → "1.2M", 42 → "42"
pub fn format_count(n: u32) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        format!("{}", n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_count_small_numbers() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(42), "42");
        assert_eq!(format_count(999), "999");
    }

    #[test]
    fn format_count_thousands() {
        assert_eq!(format_count(1000), "1.0K");
        assert_eq!(format_count(1234), "1.2K");
        assert_eq!(format_count(9999), "10.0K");
        assert_eq!(format_count(142_300), "142.3K");
    }

    #[test]
    fn format_count_millions() {
        assert_eq!(format_count(1_000_000), "1.0M");
        assert_eq!(format_count(1_234_567), "1.2M");
        assert_eq!(format_count(48_000_000), "48.0M");
    }

    #[test]
    fn default_stats_all_zeros() {
        let stats = RenderStats::default();
        assert_eq!(stats.fps, 0.0);
        assert_eq!(stats.draw_calls, 0);
        assert_eq!(stats.triangles, 0);
        assert_eq!(stats.terrain_draws, 0);
        assert_eq!(stats.terrain_total_chunks, 0);
        assert_eq!(stats.resolution, [0, 0]);
    }
}
