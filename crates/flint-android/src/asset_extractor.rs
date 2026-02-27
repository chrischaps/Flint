//! APK asset extraction to internal storage.
//!
//! Copies all files from the APK's `assets/` directory to the app's internal
//! data path so that existing `std::fs` code works unchanged.
//!
//! Android's NDK `AAssetDir_getNextFileName()` only enumerates files in a
//! directory — it does NOT list subdirectories. To work around this, the Gradle
//! build generates a `.asset_manifest` file listing every relative path. The
//! extractor reads this manifest and copies each file individually.

use ndk::asset::AssetManager;
use std::ffi::CString;
use std::fs;
use std::io::Read;
use std::path::Path;

const VERSION_MARKER: &str = ".asset_version";
const ASSET_MANIFEST: &str = "asset_manifest.txt";

/// Extract all APK assets to `data_dir` if the version marker is stale or missing.
///
/// `version` should be the app's version code (e.g., from `BuildConfig`).
/// Assets are only re-extracted when the version changes.
pub fn extract_assets(asset_manager: &AssetManager, data_dir: &Path, version: &str) {
    let marker_path = data_dir.join(VERSION_MARKER);

    // Check if assets are already up-to-date
    if let Ok(existing) = fs::read_to_string(&marker_path) {
        if existing.trim() == version {
            log::info!("Assets already extracted (version {version}), skipping");
            return;
        }
    }

    log::info!("Extracting APK assets to {}", data_dir.display());

    // Read the build-time manifest listing all asset paths
    let manifest = read_asset_string(asset_manager, ASSET_MANIFEST);
    let Some(manifest) = manifest else {
        log::error!("No {ASSET_MANIFEST} found in APK assets — cannot extract");
        return;
    };

    let mut count = 0usize;
    for line in manifest.lines() {
        let path = line.trim();
        if path.is_empty() || path == ASSET_MANIFEST {
            continue;
        }

        let dest_file = data_dir.join(path);

        // Ensure parent directory exists
        if let Some(parent) = dest_file.parent() {
            let _ = fs::create_dir_all(parent);
        }

        // Read from APK and write to internal storage
        if let Some(contents) = read_asset_bytes(asset_manager, path) {
            if let Err(e) = fs::write(&dest_file, &contents) {
                log::warn!("Failed to write {}: {e}", dest_file.display());
            } else {
                count += 1;
            }
        } else {
            log::warn!("Asset listed in manifest but not found: {path}");
        }
    }

    // Write version marker
    if let Err(e) = fs::write(&marker_path, version) {
        log::warn!("Failed to write asset version marker: {e}");
    }

    log::info!("Asset extraction complete ({count} files)");
}

/// Read an APK asset file as bytes.
fn read_asset_bytes(asset_manager: &AssetManager, path: &str) -> Option<Vec<u8>> {
    let c_path = CString::new(path).ok()?;
    let mut asset = asset_manager.open(&c_path)?;
    let mut buf = Vec::new();
    asset.read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// Read an APK asset file as a UTF-8 string.
fn read_asset_string(asset_manager: &AssetManager, path: &str) -> Option<String> {
    let bytes = read_asset_bytes(asset_manager, path)?;
    String::from_utf8(bytes).ok()
}
