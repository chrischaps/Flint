//! The `terrain` module: the scene terrain's height (ADR 0116). The terrain
//! package's since #1623 (ADR 0138 decision 3): it was flint-script's,
//! reading a sampler the player installed.

use flint_package::bind::SceneGround;
use flint_script::api::builder::kind::Num;
use flint_script::api::Module;
use flint_script::ScriptModule;
use std::sync::Arc;

/// The module's name.
pub const MODULE: &str = "terrain";

/// The `terrain` module, declared by the package and enabled by its
/// subsystem, which every scene has.
pub struct TerrainModule;

pub static TERRAIN_MODULE: TerrainModule = TerrainModule;

impl ScriptModule for TerrainModule {
    fn name(&self) -> &'static str {
        MODULE
    }

    fn enabled_when(&self) -> &'static str {
        "the terrain package is in the build (every scene)"
    }

    fn declare(&self, m: &mut Module<'_>) {
        m.func(
            "terrain_height",
            "The world-space height of the terrain at world (x, z).",
        )
        .arg("x", Num)
        .arg("z", Num)
        .returns(Num, "metres; 0.0 when no terrain is loaded")
        .run(|call, (x, z)| {
            call.ctx()
                .module_state
                .get_mut::<TerrainHeight>()
                .at(x as f32, z as f32) as f64
        });
    }
}

/// The module's state: the scene's heights, set by the subsystem at the
/// scene's build from the ground the host kept, and cleared at its unload.
#[derive(Default, Clone)]
pub struct TerrainHeight(Option<Arc<Samples>>);

/// What `terrain_height` samples: the heightmap's samples spread over the
/// terrain's world size, as the player sampled them before #1623.
struct Samples {
    heights: Vec<f32>,
    columns: u32,
    rows: u32,
    size: [f32; 2],
    height_scale: f32,
}

impl TerrainHeight {
    /// The heights of `ground`, or none.
    pub fn of(ground: Option<&SceneGround>) -> Self {
        Self(ground.map(|g| {
            Arc::new(Samples {
                heights: g.heightmap.clone_heights(),
                columns: g.heightmap.width,
                rows: g.heightmap.depth,
                size: g.size,
                height_scale: g.height_scale,
            })
        }))
    }

    /// The terrain's height at world (x, z); 0.0 with no terrain.
    ///
    /// The bits the player's sampler gave since it was written: (x, z) is
    /// read from the heightmap's corner (the terrain's transform is not
    /// taken off), and a point off the map extrapolates its edge cell.
    pub fn at(&self, x: f32, z: f32) -> f32 {
        let Some(s) = &self.0 else {
            return 0.0;
        };
        let (hm_w, hm_d) = (s.columns, s.rows);
        // Convert world coords to normalized UV
        let u = x / s.size[0];
        let v = z / s.size[1];
        // Bilinear sample from heights array
        let fx = u * (hm_w as f32 - 1.0);
        let fz = v * (hm_d as f32 - 1.0);
        let ix = (fx as u32).min(hm_w - 2);
        let iz = (fz as u32).min(hm_d - 2);
        let tx = fx - ix as f32;
        let tz = fz - iz as f32;
        let idx = |col: u32, row: u32| -> f32 { s.heights[(row * hm_w + col) as usize] };
        let h00 = idx(ix, iz);
        let h10 = idx(ix + 1, iz);
        let h01 = idx(ix, iz + 1);
        let h11 = idx(ix + 1, iz + 1);
        let h = h00 * (1.0 - tx) * (1.0 - tz)
            + h10 * tx * (1.0 - tz)
            + h01 * (1.0 - tx) * tz
            + h11 * tx * tz;
        h * s.height_scale
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_geometry::heightmap::{HeightGrid, Heightmap};

    fn ground(heights: Vec<f32>, n: u32, size: f32, scale: f32) -> SceneGround {
        SceneGround {
            entity: flint_core::EntityId::from_raw(1),
            name: "hills".into(),
            grid: HeightGrid {
                columns: 0,
                rows: 0,
                heights: Vec::new(),
                width: size,
                depth: size,
            },
            origin: [0.0; 3],
            heightmap: Heightmap::from_raw(heights, n, n),
            size: [size, size],
            height_scale: scale,
            chunks: 1,
        }
    }

    #[test]
    fn no_terrain_is_level_ground() {
        assert_eq!(TerrainHeight::default().at(3.0, -4.0), 0.0);
    }

    #[test]
    fn a_terrain_is_sampled_from_its_corner_over_its_world_size() {
        // 3x3 samples over 10 x 10 m, 0 at the corner rising to 1 across x
        let heights = vec![0.0, 0.5, 1.0, 0.0, 0.5, 1.0, 0.0, 0.5, 1.0];
        let h = TerrainHeight::of(Some(&ground(heights, 3, 10.0, 20.0)));
        assert_eq!(h.at(0.0, 0.0), 0.0);
        assert_eq!(h.at(10.0, 5.0), 20.0);
        assert!((h.at(2.5, 7.0) - 5.0).abs() < 1e-5);
    }
}
