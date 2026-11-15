//! Writing a file so that it is either the old one or the new one, never a
//! part of either (#726).
//!
//! [`write`] puts the text in `<name>.tmp` beside the target, flushes it to
//! the disk, and renames it over the target. A rename inside one directory
//! replaces the name in one step on every desktop the engine runs on
//! (`MoveFileExW(MOVEFILE_REPLACE_EXISTING)` on Windows, `rename(2)`
//! elsewhere), so a power cut, a crash or a full disk leaves the file that
//! was there. When the rename is refused (the target is open elsewhere
//! without delete sharing, or is read-only) the temp file is removed, the
//! target is as it was, and the caller gets the error.
//!
//! Native only in effect: a browser has no files, and the player's
//! `user_files` writes one `localStorage` entry there, which is a single
//! step already.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The temp file a write to `path` goes through: `<name>.tmp` beside it.
pub fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

/// Replace `path` with `contents` in one step, making its directory first.
pub fn write(path: &Path, contents: &str) -> io::Result<()> {
    write_with(path, |file| file.write_all(contents.as_bytes()))
}

/// [`write`], with the bytes produced by `fill`. An error from `fill` (or
/// from the flush, or the rename) leaves `path` untouched and no temp file.
pub fn write_with(
    path: &Path,
    fill: impl FnOnce(&mut std::fs::File) -> io::Result<()>,
) -> io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let temp = temp_path(path);
    let written = (|| {
        let mut file = std::fs::File::create(&temp)?;
        fill(&mut file)?;
        // On the disk before the name changes: a rename that outlives a
        // power cut must not point at bytes that didn't
        file.sync_all()
    })();
    let done = written.and_then(|()| std::fs::rename(&temp, path));
    if done.is_err() {
        let _ = std::fs::remove_file(&temp);
        return done;
    }
    sync_dir(path);
    Ok(())
}

/// Flush the directory entry too, where a directory can be opened (not on
/// Windows, where the rename is journaled). Best effort.
fn sync_dir(path: &Path) {
    #[cfg(unix)]
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        if let Ok(dir) = std::fs::File::open(dir) {
            let _ = dir.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flint_atomic_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_write_replaces_the_file_and_leaves_no_temp() {
        let dir = temp_dir("replace");
        let path = dir.join("deep").join("default.toml");
        write(&path, "coins = 7\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "coins = 7\n");
        // Over an existing file: the case Windows' rename must handle
        write(&path, "coins = 8\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "coins = 8\n");
        assert_eq!(names(path.parent().unwrap()), ["default.toml"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The bytes go to `<name>.tmp`, and the target is whole at every
    /// moment before the rename: what a power cut mid-write would leave.
    #[test]
    fn the_target_is_whole_while_the_temp_file_is_being_written() {
        let dir = temp_dir("midway");
        let path = dir.join("default.toml");
        std::fs::write(&path, "coins = 7\n").unwrap();
        write_with(&path, |file| {
            file.write_all(b"coins = ")?;
            file.sync_all()?;
            assert_eq!(names(&dir), ["default.toml", "default.toml.tmp"]);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "coins = 7\n");
            assert_eq!(
                std::fs::read_to_string(temp_path(&path)).unwrap(),
                "coins = "
            );
            file.write_all(b"8\n")
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "coins = 8\n");
        assert_eq!(names(&dir), ["default.toml"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_write_that_fails_midway_leaves_the_old_file_and_no_temp() {
        let dir = temp_dir("fails");
        let path = dir.join("default.toml");
        std::fs::write(&path, "coins = 7\n").unwrap();
        let err = write_with(&path, |file| {
            file.write_all(b"coins = ")?;
            Err(io::Error::other("the disk filled up"))
        })
        .unwrap_err();
        assert_eq!(err.to_string(), "the disk filled up");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "coins = 7\n");
        assert_eq!(names(&dir), ["default.toml"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A rename that is refused (here: the target is a directory) is an
    /// error, with the target as it was and the temp file gone.
    #[test]
    fn a_refused_rename_is_an_error_and_leaves_the_target() {
        let dir = temp_dir("refused");
        let path = dir.join("default.toml");
        std::fs::create_dir_all(path.join("inside")).unwrap();
        assert!(write(&path, "coins = 8\n").is_err());
        assert!(path.join("inside").is_dir());
        assert_eq!(names(&dir), ["default.toml"]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
