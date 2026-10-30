//! Files the player writes for the person playing: input rebinds today.
//!
//! On the desktop they are ordinary files under the OS config directory. A
//! browser has no file system to write to, so on wasm32 the same paths are
//! keys in `localStorage` (`flint:<path>`, forward slashes) under a virtual
//! root, [`config_dir`] `/config`. Callers keep working in paths either way.

use std::io;
use std::path::{Path, PathBuf};

/// The directory user files go under: `dirs::config_dir()` on the desktop,
/// the virtual `/config` root on the web.
pub fn config_dir() -> Option<PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        dirs::config_dir()
    }
    #[cfg(target_arch = "wasm32")]
    {
        Some(PathBuf::from("/config"))
    }
}

/// Whether there is a file at `path`.
pub fn exists(path: &Path) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        path.exists()
    }
    #[cfg(target_arch = "wasm32")]
    {
        web::get(path).ok().flatten().is_some()
    }
}

/// The file's text, or `None` if there is none.
pub fn read(path: &Path) -> io::Result<Option<String>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        web::get(path)
    }
}

/// Write `contents` to `path`, making its directory first on the desktop.
pub fn write(path: &Path, contents: &str) -> io::Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)
    }
    #[cfg(target_arch = "wasm32")]
    {
        web::set(path, contents)
    }
}

/// Remove the file at `path`, if there is one.
pub fn remove(path: &Path) -> io::Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        web::remove(path)
    }
}

/// The `localStorage` key for `path`: `flint:` and the path with forward
/// slashes, so `/config/flint/input_float.toml` reads plainly in devtools.
pub fn storage_key(path: &Path) -> String {
    format!("flint:{}", path.to_string_lossy().replace('\\', "/"))
}

#[cfg(target_arch = "wasm32")]
mod web {
    use super::storage_key;
    use std::io;
    use std::path::Path;

    // localStorage throws in a private window with storage blocked, and is
    // absent in workers; both read as an io error, never a panic.
    fn storage() -> io::Result<web_sys::Storage> {
        web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "localStorage unavailable"))
    }

    fn js_err(e: impl std::fmt::Debug) -> io::Error {
        io::Error::other(format!("localStorage: {e:?}"))
    }

    pub fn get(path: &Path) -> io::Result<Option<String>> {
        storage()?.get_item(&storage_key(path)).map_err(js_err)
    }

    // Quota exceeded (about 5 MB per origin) comes back as an error here.
    pub fn set(path: &Path, contents: &str) -> io::Result<()> {
        storage()?
            .set_item(&storage_key(path), contents)
            .map_err(js_err)
    }

    pub fn remove(path: &Path) -> io::Result<()> {
        storage()?.remove_item(&storage_key(path)).map_err(js_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_round_trips_and_is_gone_after_remove() {
        let dir = std::env::temp_dir().join(format!("flint-user-files-{}", std::process::id()));
        let path = dir.join("flint").join("input_test.toml");
        assert!(!exists(&path));
        assert_eq!(read(&path).unwrap(), None);
        write(&path, "version = 1\n").unwrap();
        assert!(exists(&path));
        assert_eq!(read(&path).unwrap().as_deref(), Some("version = 1\n"));
        remove(&path).unwrap();
        remove(&path).unwrap();
        assert!(!exists(&path));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn storage_keys_use_forward_slashes() {
        assert_eq!(
            storage_key(Path::new("/config\\flint\\input_float.toml")),
            "flint:/config/flint/input_float.toml"
        );
    }
}
