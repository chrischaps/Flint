use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct NativeFile {
    path: PathBuf,
}

impl From<PathBuf> for NativeFile {
    fn from(path: PathBuf) -> Self {
        Self { path }
    }
}

impl egui::DroppedFile for NativeFile {
    fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.path).map_err(|err| err.to_string())
    }

    // Flint #945: egui 0.36's trait asks for this on wasm32, and egui-winit
    // 0.36.2 / 0.37.0 implement `bytes` there instead, so the crate does not
    // compile for the web. Winit never delivers a dropped file on the web, so
    // there is nothing to read. The only change from crates.io; see the
    // `[patch.crates-io]` comment in Flint's root Cargo.toml for when to drop it.
    #[cfg(target_arch = "wasm32")]
    fn bytes_async(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, String>> + '_>> {
        Box::pin(async { Err("no file system on the web".to_owned()) })
    }
}
