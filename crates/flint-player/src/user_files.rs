//! Files the player writes for the person playing: input rebinds and save
//! data (#726).
//!
//! On the desktop they are ordinary files under the OS config directory. A
//! browser has no file system to write to, so on wasm32 the same paths are
//! keys in `localStorage` (`flint:<path>`, forward slashes) under a virtual
//! root, [`config_dir`] `/config`. Callers keep working in paths either way.
//!
//! A [`write`] replaces the file in one step (a temp file renamed over it
//! on the desktop, one `setItem` on the web), so a crash mid-write leaves
//! the old file whole.
//!
//! In this crate's own unit tests [`config_dir`] is a folder under the
//! temp directory, so no test can write into the real one.

use std::io;
use std::path::{Path, PathBuf};

/// The directory user files go under: `dirs::config_dir()` on the desktop,
/// the virtual `/config` root on the web.
pub fn config_dir() -> Option<PathBuf> {
    #[cfg(all(not(target_arch = "wasm32"), not(test)))]
    {
        dirs::config_dir()
    }
    #[cfg(all(not(target_arch = "wasm32"), test))]
    {
        Some(std::env::temp_dir().join(format!("flint-test-config-{}", std::process::id())))
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

/// Replace the file at `path` with `contents` in one step, making its
/// directory first on the desktop (`flint_runtime::atomic_file`: a temp
/// file beside it, flushed, renamed over it).
pub fn write(path: &Path, contents: &str) -> io::Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        flint_runtime::atomic_file::write(path, contents)
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

/// The names of the files directly in `dir`, in name order; none when
/// there is no such directory.
pub fn list(dir: &Path) -> Vec<String> {
    #[cfg(not(target_arch = "wasm32"))]
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    #[cfg(target_arch = "wasm32")]
    let mut names = web::list(dir).unwrap_or_default();
    names.sort();
    names
}

/// The name a file set aside from `path` takes: `<name>.bak`, or the first
/// of `<name>.bak.1`, `<name>.bak.2`, ... that is free. An earlier backup
/// is never written over.
pub fn backup_path(path: &Path) -> PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let first = path.with_file_name(format!("{name}.bak"));
    if !exists(&first) {
        return first;
    }
    (1u32..)
        .map(|n| path.with_file_name(format!("{name}.bak.{n}")))
        .find(|p| !exists(p))
        .unwrap_or(first)
}

/// Move the file at `path` to its [`backup_path`], keeping its bytes, and
/// say where it went. On an error the file is where it was.
pub fn set_aside(path: &Path) -> io::Result<PathBuf> {
    let backup = backup_path(path);
    #[cfg(not(target_arch = "wasm32"))]
    std::fs::rename(path, &backup)?;
    #[cfg(target_arch = "wasm32")]
    {
        let text = web::get(path)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "nothing stored"))?;
        web::set(&backup, &text)?;
        web::remove(path)?;
    }
    Ok(backup)
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

    /// The names stored directly under `dir`.
    pub fn list(dir: &Path) -> io::Result<Vec<String>> {
        let storage = storage()?;
        let prefix = format!("{}/", storage_key(dir).trim_end_matches('/'));
        let count = storage.length().map_err(js_err)?;
        let mut names = Vec::new();
        for i in 0..count {
            if let Some(key) = storage.key(i).map_err(js_err)? {
                if let Some(name) = key.strip_prefix(&prefix) {
                    if !name.is_empty() && !name.contains('/') {
                        names.push(name.to_string());
                    }
                }
            }
        }
        Ok(names)
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

    /// #726: a write goes through a temp file, so the file it replaces is
    /// never written into, and no temp file is left.
    #[test]
    fn a_write_replaces_the_file_in_one_step() {
        let dir = std::env::temp_dir().join(format!("flint-user-atomic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("saves").join("default.toml");
        write(&path, "coins = 7\n").unwrap();
        let witness = dir.join("witness");
        std::fs::hard_link(&path, &witness).unwrap();
        write(&path, "coins = 8\n").unwrap();
        assert_eq!(read(&path).unwrap().as_deref(), Some("coins = 8\n"));
        assert_eq!(std::fs::read_to_string(&witness).unwrap(), "coins = 7\n");
        assert_eq!(list(path.parent().unwrap()), ["default.toml"]);
        assert!(list(&dir.join("no-such-folder")).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The `.bak` scheme: `<name>.bak`, then `.bak.1`, `.bak.2`, ..., an
    /// earlier one never written over.
    #[test]
    fn a_file_set_aside_never_replaces_an_earlier_backup() {
        let dir = std::env::temp_dir().join(format!("flint-user-bak-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("default.toml");
        for (text, backup) in [
            ("first", "default.toml.bak"),
            ("second", "default.toml.bak.1"),
            ("third", "default.toml.bak.2"),
        ] {
            write(&path, text).unwrap();
            assert_eq!(set_aside(&path).unwrap(), dir.join(backup));
            assert!(!exists(&path));
            assert_eq!(read(&dir.join(backup)).unwrap().as_deref(), Some(text));
        }
        assert_eq!(
            read(&dir.join("default.toml.bak")).unwrap().as_deref(),
            Some("first")
        );
        assert!(set_aside(&path).is_err(), "nothing there to set aside");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tests_never_see_the_real_config_directory() {
        let dir = config_dir().unwrap();
        assert!(dir.starts_with(std::env::temp_dir()), "{}", dir.display());
    }

    #[test]
    fn storage_keys_use_forward_slashes() {
        assert_eq!(
            storage_key(Path::new("/config\\flint\\input_float.toml")),
            "flint:/config/flint/input_float.toml"
        );
    }
}
