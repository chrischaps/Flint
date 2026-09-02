//! Procedural generators that produce game-ready assets.
//!
//! Each generator module builds on the algorithms in [`crate::algorithms`] to
//! produce meshes, textures, and materials for specific asset types.

pub mod creature;
pub mod texture;
pub mod tree;
pub mod util;

use crate::registry::GeneratorRegistry;

/// Register all built-in generators with the given registry.
pub fn register_built_in_generators(registry: &mut GeneratorRegistry) {
    registry.register(Box::new(tree::TreeGenerator));
    registry.register(Box::new(texture::TextureGenerator));
    registry.register(Box::new(creature::CreatureGenerator));
}
