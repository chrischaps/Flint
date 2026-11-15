//! The editors' native file dialogs (#221).
//!
//! With the `dialogs` feature (the default) this is rfd's `FileDialog`. On
//! Linux rfd brings an async D-Bus stack (zbus, for the desktop portal) that
//! a machine with no desktop has no use for, so a headless build leaves it
//! out and gets this
//! stand-in, the same builder with nothing behind it: every pick is a
//! cancel, and it says once on stderr why no dialog opened.

#[cfg(feature = "dialogs")]
pub use rfd::FileDialog;

#[cfg(not(feature = "dialogs"))]
pub use stand_in::FileDialog;

#[cfg(not(feature = "dialogs"))]
mod stand_in {
    use std::path::{Path, PathBuf};
    use std::sync::Once;

    #[derive(Default)]
    pub struct FileDialog;

    impl FileDialog {
        pub fn new() -> Self {
            Self
        }

        pub fn set_title(self, _title: impl Into<String>) -> Self {
            self
        }

        pub fn add_filter(self, _name: impl Into<String>, _extensions: &[impl ToString]) -> Self {
            self
        }

        pub fn set_file_name(self, _name: impl Into<String>) -> Self {
            self
        }

        pub fn set_directory<P: AsRef<Path>>(self, _path: P) -> Self {
            self
        }

        pub fn pick_file(self) -> Option<PathBuf> {
            no_dialog()
        }

        pub fn save_file(self) -> Option<PathBuf> {
            no_dialog()
        }
    }

    fn no_dialog() -> Option<PathBuf> {
        static SAID: Once = Once::new();
        SAID.call_once(|| {
            eprintln!(
                "flint: no file dialog: this build left out the `dialogs` feature \
                 (rebuild flint-cli with default features to get one)"
            );
        });
        None
    }
}
