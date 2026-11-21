//! Flint Scene - TOML scene serialization
//!
//! This crate handles loading and saving scenes in TOML format.
#![cfg_attr(test, allow(clippy::disallowed_methods))] // unit tests read their files off the disk and assert on TOML as written; the lib build still holds the rules (#761)

mod diagnose;
mod edit;
mod format;
mod loader;
pub mod patcher;
mod place;
mod prefab;
mod saver;

pub use diagnose::{
    asset_references, declared_assets, diagnose, diagnose_all, diagnose_scene, diagnose_scene_in,
    AssetReference, Located, REFUSES_THE_LOAD,
};
pub use edit::{SceneEdit, SceneScalar};
pub use flint_core::diagnostic::{self, Diagnostic, Severity};
pub use format::{
    fold_legacy_style_keys, CameraDef, EntityDef, EnvironmentDef, PostProcessDef, PrefabFile,
    PrefabInstance, SceneFile, SceneMetadata, LEGACY_STYLE_KEYS,
};
pub use loader::{
    load_failed, load_scene, load_scene_after, load_scene_diagnosed, load_scene_string,
    load_scene_text, load_scene_text_diagnosed, reload_scene, reload_scene_string,
};
pub use patcher::{FieldHome, SceneDocument};
pub use place::place_table_diagnostics;
pub use saver::{save_scene, save_scene_string, update_scene_file};
