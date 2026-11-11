//! Flint ECS - Entity Component System with stable IDs
//!
//! This crate wraps hecs with stable entity identifiers and
//! dynamic component storage for schema-defined components.

mod component;
mod entity;
mod view;
mod world;

pub use component::DynamicComponents;
pub use entity::EntityInfo;
pub use view::{ComponentView, ReadReports, KEPT_REPORTS};
pub use world::FlintWorld;
