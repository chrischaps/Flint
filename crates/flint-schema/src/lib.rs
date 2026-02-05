//! Flint Schema - Component and archetype introspection
//!
//! This crate provides the schema system for defining and validating
//! components and archetypes at runtime.

mod archetype;
mod component;
mod registry;
mod validation;

pub use archetype::ArchetypeSchema;
pub use component::{ComponentSchema, FieldSchema, FieldType};
pub use registry::SchemaRegistry;
pub use validation::validate_component_data;
