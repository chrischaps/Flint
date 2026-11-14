//! Flint ECS - Entity Component System with stable IDs
//!
//! This crate wraps hecs with stable entity identifiers and
//! dynamic component storage for schema-defined components.
#![cfg_attr(test, allow(clippy::disallowed_methods))] // unit tests read their files off the disk and assert on TOML as written; the lib build still holds the rules (#761)

mod component;
mod entity;
pub mod motion_hold;
mod view;
mod world;

pub use component::DynamicComponents;
pub use entity::EntityInfo;
pub use view::{ComponentView, ReadReports, KEPT_REPORTS};
pub use world::FlintWorld;
