//! Flint Scene - TOML scene serialization
//!
//! This crate handles loading and saving scenes in TOML format.

mod commit;
mod diagnose;
mod format;
mod loader;
pub mod patcher;
mod prefab;
mod saver;
mod wooze;

pub use commit::{commit_look, look_edits, CommitReport, SceneEdit, SceneScalar};
pub use diagnose::{diagnose, diagnose_references, diagnose_scene, ENTITY_REFERENCES};
pub use flint_core::diagnostic::{self, Diagnostic, Severity};
pub use format::{
    fold_legacy_style_keys, BoilDef, CameraDef, EntityDef, LivingDef, LivingEchoesDef,
    LivingSpeciesDef, LivingVarianceDef, PostProcessDef, PrefabFile, PrefabInstance, SceneFile,
    SceneMetadata, WoozeDef, WoozeKnobs, WoozePreset, LEGACY_STYLE_KEYS,
};
pub use loader::{
    load_scene, load_scene_diagnosed, load_scene_string, load_scene_text, reload_scene,
    reload_scene_string,
};
pub use patcher::SceneDocument;
pub use saver::{save_scene, save_scene_string, update_scene_file};
pub use wooze::{
    apply_preset_file, list_presets, list_presets_in, preset_save_dir, read_preset, read_preset_in,
    write_preset, write_preset_in, WoozeIssue, PRESET_SUFFIX,
};
