//! Flint Schema - Component and archetype introspection
//!
//! This crate provides the schema system for defining and validating
//! components and archetypes at runtime.

mod archetype;
mod component;
pub mod lint;
mod registry;
mod validation;

pub use archetype::ArchetypeSchema;
pub use component::{
    type_name_of, ComponentSchema, FieldSchema, FieldType, TableSchema, OLD_TYPE_NAMES, TYPE_NAMES,
};
pub use flint_core::diagnostic::{self, Diagnostic, Severity};
pub use flint_core::project::{SchemaDirs, SchemaDirsError, SchemaSource};
#[allow(deprecated)]
pub use registry::discover_schema_dirs;
pub use registry::SchemaRegistry;
pub use validation::{
    references, validate_component, validate_component_data, validate_table, Reference,
    ReferenceKind,
};
