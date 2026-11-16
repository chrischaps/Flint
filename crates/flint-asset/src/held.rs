//! A project's asset catalog with the content store behind it: what the
//! checks ask before the files for a model or a texture (#804).
//!
//! An asset registered in `assets/*.asset.toml` is there when its content
//! is in the store (`.flint/assets`), whatever the file search finds. Both
//! `flint_scene::diagnose` (so `flint validate` and every load) and
//! `flint check` ask through this, so the two say the same thing about a
//! scene. Read through the vfs, so a packed bundle answers as the disk does.

use crate::{AssetCatalog, ContentStore};
use flint_core::project::Project;
use std::path::PathBuf;

/// Where a project keeps its catalog, from its root.
pub const CATALOG_DIR: &str = "assets";
/// Where a project keeps its content store, from its root.
pub const STORE_DIR: &str = ".flint/assets";

/// A project's catalog and content store, loaded once.
pub struct HeldAssets {
    catalog: AssetCatalog,
    store: ContentStore,
}

impl HeldAssets {
    /// The catalog under `project`'s root and the store beside it. `None`
    /// when the project has no catalog, or one of its files doesn't parse
    /// (the player then loads none either).
    pub fn of(project: &Project) -> Option<Self> {
        let catalog_dir = under_root(project, CATALOG_DIR);
        if !flint_core::vfs::is_dir(&catalog_dir) {
            return None;
        }
        let catalog = AssetCatalog::load_from_directory(&catalog_dir).ok()?;
        Some(Self {
            catalog,
            store: ContentStore::new(under_root(project, STORE_DIR)),
        })
    }

    /// The stored file of the asset the catalog registers as `name`.
    pub fn path_of(&self, name: &str) -> Option<PathBuf> {
        let meta = self.catalog.get(name)?;
        let hash = flint_core::ContentHash::from_prefixed_hex(&meta.hash)?;
        self.store.get(&hash)
    }

    /// Whether the catalog registers `name` and the store holds its content.
    pub fn holds(&self, name: &str) -> bool {
        self.path_of(name).is_some()
    }
}

/// `rel` (`/`-separated) under the project's root, a segment at a time so
/// the separators are the platform's.
pub(crate) fn under_root(project: &Project, rel: &str) -> PathBuf {
    let mut p = project.root.clone();
    p.extend(rel.split('/'));
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::vfs::{self, MemorySource};
    use std::path::Path;
    use std::sync::Arc;

    const HASH: &str = "ab12000000000000000000000000000000000000000000000000000000000000";

    /// What `HeldAssets::of` says about `oak_table` in a project of `files`:
    /// `None` with no catalog.
    fn holds_oak_table(files: &[(&str, &str)]) -> Option<bool> {
        let mut source = MemorySource::new();
        source.insert("game/flint.toml", "");
        for (path, text) in files {
            source.insert(*path, *text);
        }
        vfs::scoped(Arc::new(source), || {
            let project = Project::discover(Path::new("game")).unwrap();
            let held = HeldAssets::of(&project)?;
            assert!(!held.holds("ghost"), "a name no sidecar registers");
            Some(held.holds("oak_table"))
        })
    }

    fn sidecar() -> String {
        format!(
            "[asset]\nname = \"oak_table\"\ntype = \"mesh\"\nhash = \"sha256:{HASH}\"\n\
             source_path = \"oak_table.glb\"\n"
        )
    }

    #[test]
    fn a_registered_asset_is_held_only_with_its_content() {
        let sidecar = sidecar();
        let sidecar_file = ("game/assets/oak_table.asset.toml", sidecar.as_str());
        let stored = format!("game/.flint/assets/ab/{HASH}.glb");
        assert_eq!(
            holds_oak_table(&[sidecar_file, (stored.as_str(), "glTF")]),
            Some(true)
        );
        // Registered, and nothing in the store: not there
        assert_eq!(holds_oak_table(&[sidecar_file]), Some(false));
    }

    #[test]
    fn no_catalog_is_none() {
        assert_eq!(holds_oak_table(&[]), None);
    }
}
