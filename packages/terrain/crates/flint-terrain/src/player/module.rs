//! The `terrain` module: the scene terrain's height (ADR 0116). The terrain
//! package's since #1623 (ADR 0138 decision 3): it was flint-script's,
//! reading a sampler the player installed.

use flint_geometry::heightmap::Heightmap;
use flint_package::bind::SceneGround;
use flint_script::api::builder::kind::{Array, Num};
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
            "The world-space height of the terrain's drawn surface at world (x, z), wherever its transform puts it.",
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

        m.func(
            "terrain_live_fields",
            "The terrain fields a script can change while the game runs, with set_field(id, \"terrain\", field, v) or tween(id, \"terrain.<field>\", v, seconds): every grass.* key (grass.enabled false stops the grass, true brings it back), texture_tile, metallic and roughness. They reach the next frame drawn.",
        )
        .returns(Array, "the field names, as a scene file writes them")
        .run(|_call, ()| -> rhai::Array {
            super::LIVE_FIELDS
                .iter()
                .map(|f| rhai::Dynamic::from(f.to_string()))
                .collect()
        });
    }
}

/// The module's state: the scene's heights, set by the subsystem at the
/// scene's build from the ground the host kept, and cleared at its unload.
#[derive(Default, Clone)]
pub struct TerrainHeight(Option<Arc<Samples>>);

/// What `terrain_height` samples: the heightmap spread over the terrain's
/// world size, where the entity's transform puts its first corner.
struct Samples {
    heightmap: Heightmap,
    origin: [f32; 3],
    size: [f32; 2],
    height_scale: f32,
}

impl TerrainHeight {
    /// The heights of `ground`, or none.
    pub fn of(ground: Option<&SceneGround>) -> Self {
        Self(ground.map(|g| {
            Arc::new(Samples {
                heightmap: g.heightmap.clone(),
                origin: g.origin,
                size: g.size,
                height_scale: g.height_scale,
            })
        }))
    }

    /// The terrain's height at world (x, z); 0.0 with no terrain.
    ///
    /// The drawn surface (#1691): (x, z) is taken into the terrain's own
    /// frame by its transform's position, sampled as `sample_world` and the
    /// mesh sample it, and the position's y added. A point off the map reads
    /// the nearest edge's height. Until #1691 it read world (x, z) from the
    /// map's corner, right only for a terrain at the origin.
    pub fn at(&self, x: f32, z: f32) -> f32 {
        let Some(s) = &self.0 else {
            return 0.0;
        };
        let u = (x - s.origin[0]) / s.size[0];
        let v = (z - s.origin[2]) / s.size[1];
        s.heightmap.sample(u, v) * s.height_scale + s.origin[1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_geometry::heightmap::{HeightGrid, Heightmap};

    fn ground(heights: Vec<f32>, n: u32, size: f32, scale: f32) -> SceneGround {
        placed(heights, n, size, scale, [0.0; 3])
    }

    fn placed(heights: Vec<f32>, n: u32, size: f32, scale: f32, origin: [f32; 3]) -> SceneGround {
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
            origin,
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

    /// A terrain away from the origin, centred and sunk as a game puts an
    /// island (#1691): `terrain_height` is the drawn surface there, which is
    /// `sample_world` in the terrain's own frame, moved by its transform.
    #[test]
    fn a_placed_terrain_reads_the_surface_its_transform_puts_there() {
        use crate::{HeightmapFiles, Terrain, TerrainConfig};
        let n = 9u32;
        let heights: Vec<f32> = (0..n * n)
            .map(|i| ((i * 37 % 101) as f32) / 100.0)
            .collect();
        let (size, scale, origin) = (64.0, 20.0, [-32.0, -5.0, -32.0]);
        let h = TerrainHeight::of(Some(&placed(heights.clone(), n, size, scale, origin)));
        let config = TerrainConfig {
            heightmap_path: String::new(),
            width: size,
            depth: size,
            height_scale: scale,
            chunk_resolution: 4,
            texture_tile: 1.0,
            splat_map_path: String::new(),
            layer_textures: Default::default(),
            metallic: 0.0,
            roughness: 1.0,
            grass: None,
        };
        let map = Heightmap::from_raw(heights, n, n);
        let drawn =
            |x: f32, z: f32| map.sample_world(x - origin[0], z - origin[2], &config) + origin[1];
        for (x, z) in [
            (0.0, 0.0),
            (-31.0, -17.5),
            (-3.25, 12.0),
            (20.0, -30.0),
            (31.9, 31.9),
            (-32.0, 32.0),
        ] {
            let (got, want) = (h.at(x, z), drawn(x, z));
            assert!(
                (got - want).abs() < 1e-4,
                "({x}, {z}): {got} vs the surface's {want}"
            );
        }
        // Every vertex of the drawn mesh, where the transform puts it
        let terrain = Terrain::generate(&map, &config);
        for p in terrain.chunks.iter().flat_map(|c| &c.positions) {
            let (x, y, z) = (p[0] + origin[0], p[1] + origin[1], p[2] + origin[2]);
            let got = h.at(x, z);
            assert!((got - y).abs() < 1e-4, "vertex ({x}, {y}, {z}): {got}");
        }
        // Off the map, the edge's height: the drawn surface ends there
        assert!((h.at(-100.0, 0.0) - drawn(-32.0, 0.0)).abs() < 1e-4);
        assert!((h.at(0.0, 500.0) - drawn(0.0, 32.0)).abs() < 1e-4);
    }
}
