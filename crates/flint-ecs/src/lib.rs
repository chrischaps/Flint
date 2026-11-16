//! Flint ECS - Entity Component System with stable IDs
//!
//! Stable entity identifiers, dynamic component storage for
//! schema-defined components, the parent hierarchy and a component index,
//! in maps of its own (#691: it once wrapped a hecs world that stored
//! nothing).
#![cfg_attr(test, allow(clippy::disallowed_methods))] // unit tests read their files off the disk and assert on TOML as written; the lib build still holds the rules (#761)

mod component;
mod entity;
pub mod motion_hold;
mod view;
mod world;

pub use component::{ComponentsMut, DynamicComponents};
pub use entity::EntityInfo;
pub use view::{ComponentView, ReadReports, KEPT_REPORTS};
pub use world::FlintWorld;
