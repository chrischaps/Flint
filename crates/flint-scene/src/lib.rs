//! Flint Scene - TOML scene serialization
//!
//! This crate handles loading and saving scenes in TOML format.

mod diagnose;
mod edit;
mod format;
mod loader;
pub mod patcher;
mod prefab;
mod saver;

pub use diagnose::{diagnose, diagnose_scene, diagnose_scene_in};
pub use edit::{SceneEdit, SceneScalar};
pub use flint_core::diagnostic::{self, Diagnostic, Severity};
pub use format::{
    fold_legacy_style_keys, CameraDef, EntityDef, PostProcessDef, PrefabFile, PrefabInstance,
    SceneFile, SceneMetadata, LEGACY_STYLE_KEYS,
};
pub use loader::{
    load_scene, load_scene_diagnosed, load_scene_string, load_scene_text, reload_scene,
    reload_scene_string,
};
pub use patcher::SceneDocument;
pub use saver::{save_scene, save_scene_string, update_scene_file};
