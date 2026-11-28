//! What the host built that stands still and can be stood on, as plain
//! data (#1617): the triangles swept along a scene's splines and each
//! terrain's height grid. The kernel builds them with the renderer at a
//! scene's entry and leaves them in the run's resources as [`Surfaces`];
//! the physics package takes them at its build and makes them static
//! colliders. A build without physics leaves them unread.
//!
//! And [`MadeTriangles`]: the triangles of every mesh an asset maker made
//! (a procgen model), by name, kept for the run as the renderer keeps the
//! meshes (#1222), so a collider that names one has its triangles.

use crate::assets::MadeMeshes;
use flint_core::EntityId;
pub use flint_geometry::triangles::Triangles;
use std::collections::HashMap;
use std::sync::Arc;

/// A fixed mesh's triangles, in the world: a spline's swept road.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceMesh {
    pub entity: EntityId,
    pub vertices: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub friction: f32,
    pub restitution: f32,
}

/// A terrain's heights as a grid where its transform puts it (#1219).
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceGrid {
    pub entity: EntityId,
    /// Samples along X.
    pub columns: usize,
    /// Samples along Z.
    pub rows: usize,
    /// Row by row: `heights[row * columns + column]`.
    pub heights: Vec<f32>,
    /// The X extent the columns span.
    pub width: f32,
    /// The Z extent the rows span.
    pub depth: f32,
    /// Where column 0, row 0 at height 0 is in the world.
    pub origin: [f32; 3],
    pub friction: f32,
    pub restitution: f32,
}

/// The scene's surfaces, in the order the host built them: the splines'
/// meshes first, then the terrains. Replaced at each scene's entry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Surfaces {
    pub meshes: Vec<SurfaceMesh>,
    pub grids: Vec<SurfaceGrid>,
}

/// The triangles of the meshes the asset makers made, by name: what the
/// makers write through [`MadeMeshes`], kept in the run's resources.
#[derive(Debug, Default)]
pub struct MadeTriangles {
    made: HashMap<String, Arc<Triangles>>,
    generation: u64,
}

impl MadeTriangles {
    /// The triangles made under `name`, if a maker made a mesh of it.
    pub fn get(&self, name: &str) -> Option<&Arc<Triangles>> {
        self.made.get(name)
    }

    /// Bumped whenever a mesh is made or forgotten: one comparison tells a
    /// reader whether anything changed.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

impl MadeMeshes for MadeTriangles {
    fn made(&mut self, name: &str, positions: Vec<[f32; 3]>, triangles: Vec<[u32; 3]>) {
        self.made.insert(
            name.to_string(),
            Arc::new(Triangles {
                positions,
                triangles,
            }),
        );
        self.generation += 1;
    }

    fn forget(&mut self, name: &str) {
        if self.made.remove(name).is_some() {
            self.generation += 1;
        }
    }
}
