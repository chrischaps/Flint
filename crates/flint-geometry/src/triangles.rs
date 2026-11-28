//! A mesh as bare triangles: points and the index triples between them, in
//! the mesh's own space. What a made mesh leaves for the colliders that
//! name it (#1222) and what physics builds a `mesh` or `convex` collider
//! from (`flint_physics::ColliderMesh`, #729, #1617): plain data both sides
//! read, so neither names the other.

/// Points and the triangles between them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Triangles {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
}
