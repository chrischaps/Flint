//! Modified-time stamps of the files a cache read, for hot reload (#323).
//!
//! A cache records every path an entry depended on: the file it loaded, the
//! files it references, and the candidates searched before it (absent ones
//! too, so a file that appears, or a typo fixed on disk, is noticed).
//! Native only: hot reload reads the disk with `std::fs`, never the vfs, and
//! a web bundle never changes, so on wasm nothing is recorded or changes.

use std::path::Path;

/// The paths a cache depends on, each with the modified time it had when
/// recorded (`None`: it didn't exist).
#[derive(Default)]
pub(crate) struct FileStamps {
    #[cfg(not(target_arch = "wasm32"))]
    files: Vec<(std::path::PathBuf, Option<(std::time::SystemTime, u64)>)>,
}

impl FileStamps {
    /// Record `path` as it is now, unless it is already recorded.
    pub(crate) fn watch(&mut self, path: &Path) {
        #[cfg(not(target_arch = "wasm32"))]
        if !self.files.iter().any(|(p, _)| p == path) {
            self.files.push((path.to_path_buf(), modified(path)));
        }
        #[cfg(target_arch = "wasm32")]
        let _ = path;
    }

    /// Whether any recorded path appeared, vanished or was modified since.
    pub(crate) fn changed(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        return self.files.iter().any(|(p, t)| modified(p) != *t);
        #[cfg(target_arch = "wasm32")]
        false
    }

    /// Forget every path.
    pub(crate) fn clear(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        self.files.clear();
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// The modified time and length (a coarse clock can miss a quick edit's
/// time; its length often differs).
fn modified(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}
