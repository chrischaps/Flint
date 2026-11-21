//! Flint Schema - Component and archetype introspection
//!
//! This crate provides the schema system for defining and validating
//! components and archetypes at runtime.
#![cfg_attr(test, allow(clippy::disallowed_methods))] // unit tests read their files off the disk and assert on TOML as written; the lib build still holds the rules (#761)

mod archetype;
mod component;
pub mod lint;
pub mod panel;
pub mod post_effects;
mod registry;
mod validation;

pub use archetype::{ArchetypeSchema, ArchetypeVisual};
pub use component::{
    type_name_of, ComponentSchema, FieldSchema, FieldType, TableSchema, OLD_TYPE_NAMES, TYPE_NAMES,
};
pub use flint_core::diagnostic::{self, Diagnostic, Severity};
pub use flint_core::project::{SchemaDirs, SchemaDirsError, SchemaSource};
pub use panel::{PanelRole, PanelRow, PanelSpec, PanelValue};
#[allow(deprecated)]
pub use registry::discover_schema_dirs;
pub use registry::{ArchetypeOrigin, SchemaOrigin, SchemaRegistry};
pub use validation::{
    references, validate_component, validate_component_data, validate_table, Reference,
    ReferenceKind,
};
