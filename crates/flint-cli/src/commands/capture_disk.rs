//! Will a capture fit on its disk (#1196)? A 4K film run is tens of GB
//! (the showcase film's 10,940 frames were 77 GB), and a disk that fills
//! mid-run leaves a take half-written hours in. So `flint capture` asks the
//! disk under `out` for its free space before the first take, and refuses a
//! run whose estimate passes 90% of it.
//!
//! std has no free-space call: it is `GetDiskFreeSpaceExW` on Windows and
//! `statvfs` elsewhere, through `windows-sys` and `libc`, both already in
//! the tree. Where neither is there the run goes ahead unchecked.

use std::path::{Path, PathBuf};

/// The share of the free space a run may fill: the rest is for the
/// system, the encode and whatever else writes meanwhile.
pub const FILL: f64 = 0.9;

/// A take's frames on disk, estimated: a 4K PNG of the showcase film
/// averaged 7 MB (#436); say 8 MB, scaled by area.
pub fn take_bytes(frames: usize, width: u32, height: u32) -> f64 {
    frames as f64 * 8e6 * (width as f64 * height as f64) / (3840.0 * 2160.0)
}

/// The bytes the caller may still write on the disk that holds `path`, or
/// the nearest folder above it that exists (`out` is often not made yet).
/// `None` where the platform can't say.
pub fn free_bytes(path: &Path) -> Option<u64> {
    free_on(&existing_folder(path)?)
}

fn existing_folder(path: &Path) -> Option<PathBuf> {
    let path = std::path::absolute(path).ok()?;
    path.ancestors().find(|p| p.is_dir()).map(Path::to_path_buf)
}

#[cfg(windows)]
fn free_on(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    // SAFETY: a NUL-terminated path, one out-pointer to a live u64, and the
    // two totals we don't want left null, as the call allows
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(free)
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast)] // the field types differ between unixes
fn free_on(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs is plain data, all zeroes a valid value; the call
    // reads a NUL-terminated path and writes only into it
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    let r = unsafe { libc::statvfs(path.as_ptr(), &mut s) };
    (r == 0).then(|| s.f_bavail as u64 * s.f_frsize as u64)
}

#[cfg(not(any(windows, unix)))]
fn free_on(_dir: &Path) -> Option<u64> {
    None
}

/// Bytes the files under `path` hold: a take folder a capture empties
/// before it writes, so its old frames are room for the new.
pub fn bytes_under(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => bytes_under(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// A run's need against its disk's room.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Room {
    /// The most the run holds on the disk at once, estimated
    pub need: f64,
    /// Free on the disk now
    pub free: u64,
    /// What the take folders already hold, which the run frees first
    pub reclaimed: u64,
}

impl Room {
    /// The bytes the run may fill: 90% of the free space, plus the old
    /// takes it replaces.
    pub fn limit(&self) -> f64 {
        FILL * self.free as f64 + self.reclaimed as f64
    }

    pub fn fits(&self) -> bool {
        self.need <= self.limit()
    }

    /// The largest of a half, a quarter or an eighth of the frame (as a
    /// further `--scale`) that fits, and its need. Bytes go with area.
    pub fn scale_that_fits(&self) -> Option<(f64, f64)> {
        [0.5, 0.25, 0.125]
            .into_iter()
            .map(|s| (s, self.need * s * s))
            .find(|&(_, need)| need <= self.limit())
    }
}

/// Bytes for a person: `2.1 TB`, `41.3 GB`, `512 MB`.
pub fn size(bytes: f64) -> String {
    if bytes >= 1e12 {
        format!("{:.1} TB", bytes / 1e12)
    } else if bytes >= 1e9 {
        format!("{:.1} GB", bytes / 1e9)
    } else {
        format!("{:.0} MB", bytes / 1e6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_4k_frame_is_8_mb_and_bytes_go_with_area() {
        assert_eq!(take_bytes(1, 3840, 2160), 8e6);
        assert_eq!(take_bytes(10, 1920, 1080), 2e7);
    }

    #[test]
    fn a_run_fits_within_90_percent_of_the_free_space_plus_what_it_replaces() {
        let gb = 1_000_000_000u64;
        let room = |need: f64, free: u64, reclaimed: u64| Room {
            need,
            free,
            reclaimed,
        };
        assert!(room(9e9, 10 * gb, 0).fits());
        assert!(!room(9.1e9, 10 * gb, 0).fits());
        // A rerun into the same folders frees the old frames first
        assert!(room(20e9, 10 * gb, 12 * gb).fits());
        assert_eq!(
            room(40e9, 10 * gb, 0).scale_that_fits(),
            Some((0.25, 2.5e9))
        );
        assert_eq!(room(1e15, 10 * gb, 0).scale_that_fits(), None);
    }

    #[test]
    fn sizes_read_in_the_unit_that_fits() {
        assert_eq!(size(2.08e15), "2080.0 TB");
        assert_eq!(size(41.3e9), "41.3 GB");
        assert_eq!(size(512e6), "512 MB");
    }

    #[test]
    fn the_disk_under_a_folder_not_made_yet_has_free_space() {
        let dir = std::env::temp_dir().join("flint_capture_disk_never_made/a/b");
        let free = free_bytes(&dir);
        if cfg!(any(windows, unix)) {
            assert!(free.is_some_and(|f| f > 0), "{free:?}");
        }
    }

    #[test]
    fn old_frames_under_a_take_folder_are_counted() {
        let dir = std::env::temp_dir().join(format!("flint_capture_disk_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("shot/main")).unwrap();
        std::fs::write(dir.join("shot/main/frame_0001.png"), [0u8; 300]).unwrap();
        std::fs::write(dir.join("shot/main/player.log"), [0u8; 20]).unwrap();
        assert_eq!(bytes_under(&dir), 320);
        assert_eq!(bytes_under(&dir.join("missing")), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
