//! Flint Scene - TOML scene serialization
//!
//! This crate handles loading and saving scenes in TOML format.

mod format;
mod loader;
pub mod patcher;
mod prefab;
mod saver;
mod wooze;

pub use format::{
    BoilDef, CameraDef, EntityDef, LivingDef, LivingEchoesDef, LivingSpeciesDef, LivingVarianceDef,
    PostProcessDef, PrefabFile, PrefabInstance, SceneFile, SceneMetadata, WoozeDef, WoozeKnobs,
    WoozePreset,
};
pub use loader::{load_scene, load_scene_string, reload_scene, reload_scene_string};
pub use patcher::SceneDocument;
pub use saver::{save_scene, save_scene_string, update_scene_file};
pub use wooze::{
    apply_preset_file, list_presets, read_preset, write_preset, WoozeIssue, PRESET_SUFFIX,
};
