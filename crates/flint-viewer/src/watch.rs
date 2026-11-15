//! What `flint edit --watch` reloads the scene for (#429).
//!
//! The watcher is recursive over the scene's folder, so it sees a user
//! style's sources (`styles/<name>.wgsl`, `styles/<name>.shader.toml`)
//! change. Neither needs the world rebuilt: the style's pass polls its
//! own WGSL and builds it again in place, and
//! [`reload_changed_for`](flint_render::stylize::custom::reload_changed_for),
//! which `check_reload` calls a few times a second, reads an edited
//! description again and says when the scene's settings must be re-read.
//! A scene reload for them was slow on a big scene and dropped skinned
//! meshes (#331), so the watcher leaves them to those two.

use flint_core::project::AssetScope;
use std::path::{Path, PathBuf};

/// A file a user style is made of, by its name: its WGSL or its
/// description.
pub fn is_style_source_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    name.ends_with(".wgsl") || name.ends_with(flint_render::stylize::custom::SUFFIX)
}

/// The style folders a scene's user styles load from, in the spellings a
/// watcher event can come in: as found (made absolute) and canonical.
#[derive(Debug, Clone, Default)]
pub struct StyleSources {
    dirs: Vec<PathBuf>,
}

impl StyleSources {
    /// The folders the style search walks from `scene_dir`, the ones
    /// `load_dir` registered the scene's styles from.
    pub fn for_scene_dir(scene_dir: &Path) -> Self {
        let scope = AssetScope::for_dir(scene_dir);
        Self::new(flint_render::stylize::custom::style_dirs(&scope))
    }

    /// These folders, whether or not they exist yet.
    pub fn new(dirs: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut out = Vec::new();
        for dir in dirs {
            for spelling in spellings(&dir) {
                if !out.contains(&spelling) {
                    out.push(spelling);
                }
            }
        }
        Self { dirs: out }
    }

    /// Whether `path` is a style's WGSL or description inside one of the
    /// style folders (at any depth: a description may name a shader in a
    /// subfolder).
    pub fn owns(&self, path: &Path) -> bool {
        if !is_style_source_name(path) {
            return false;
        }
        spellings_of_file(path)
            .iter()
            .any(|p| self.dirs.iter().any(|dir| p.starts_with(dir)))
    }

    /// Whether a batch of changed paths needs the scene parsed and built
    /// again: any path that isn't a style source does. An empty batch
    /// doesn't.
    pub fn wants_scene_reload<'a>(&self, paths: impl IntoIterator<Item = &'a Path>) -> bool {
        paths.into_iter().any(|p| !self.owns(p))
    }
}

/// `dir` made absolute, and canonical when it exists.
fn spellings(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(2);
    if let Ok(abs) = std::path::absolute(dir) {
        out.push(abs);
    }
    if let Ok(canon) = dir.canonicalize() {
        if !out.contains(&canon) {
            out.push(canon);
        }
    }
    if out.is_empty() {
        out.push(dir.to_path_buf());
    }
    out
}

/// A file's spellings: its parent's (which still exists when the file
/// was just removed or renamed away) with its name joined on.
fn spellings_of_file(path: &Path) -> Vec<PathBuf> {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => spellings(parent)
            .into_iter()
            .map(|p| p.join(name))
            .collect(),
        _ => spellings(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under the system temp dir, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "flint-viewer-watch-{tag}-{}-{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    #[test]
    fn a_styles_wgsl_or_description_reloads_nothing() {
        let tmp = TempDir::new("sources");
        let styles = tmp.0.join("styles");
        let wgsl = styles.join("duotone.wgsl");
        let desc = styles.join("duotone.shader.toml");
        let nested = styles.join("lib").join("noise.wgsl");
        for p in [&wgsl, &desc, &nested] {
            touch(p);
        }
        let sources = StyleSources::new([styles.clone()]);
        assert!(sources.owns(&wgsl));
        assert!(sources.owns(&desc));
        assert!(sources.owns(&nested), "a shader in a subfolder");
        assert!(!sources.wants_scene_reload([wgsl.as_path(), desc.as_path()]));
    }

    #[test]
    fn everything_else_still_reloads_the_scene() {
        let tmp = TempDir::new("others");
        let styles = tmp.0.join("styles");
        let wgsl = styles.join("duotone.wgsl");
        touch(&wgsl);
        let scene = tmp.0.join("demo.scene.toml");
        let look = styles.join("dusk.look.toml");
        let image = styles.join("paper.png");
        // A WGSL outside every style folder is some other asset's
        let stray = tmp.0.join("shaders").join("water.wgsl");
        for p in [&scene, &look, &image, &stray] {
            touch(p);
        }
        let sources = StyleSources::new([styles]);
        for p in [&scene, &look, &image, &stray] {
            assert!(!sources.owns(p), "{}", p.display());
        }
        // A batch with one scene edit among the style saves reloads
        assert!(sources.wants_scene_reload([wgsl.as_path(), scene.as_path()]));
    }

    #[test]
    fn a_removed_wgsl_is_still_the_styles() {
        let tmp = TempDir::new("removed");
        let styles = tmp.0.join("styles");
        std::fs::create_dir_all(&styles).unwrap();
        let sources = StyleSources::new([styles.clone()]);
        assert!(sources.owns(&styles.join("gone.wgsl")));
    }

    #[test]
    fn a_relative_folder_matches_an_absolute_event() {
        let tmp = TempDir::new("relative");
        let styles = tmp.0.join("styles");
        let wgsl = styles.join("x.wgsl");
        touch(&wgsl);
        let cwd = std::env::current_dir().unwrap();
        // The same folder spelled through `..` segments from the cwd
        let mut rel = PathBuf::new();
        let depth = cwd
            .components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .count();
        for _ in 0..depth {
            rel.push("..");
        }
        let rel = rel.join(
            styles
                .strip_prefix(styles.ancestors().last().unwrap())
                .unwrap(),
        );
        let sources = StyleSources::new([rel]);
        assert!(sources.owns(&wgsl.canonicalize().unwrap()));
        assert!(sources.owns(&wgsl));
    }

    #[test]
    fn the_scenes_own_styles_folder_is_searched() {
        let tmp = TempDir::new("scene");
        let wgsl = tmp.0.join("styles").join("x.wgsl");
        touch(&wgsl);
        touch(&tmp.0.join("demo.scene.toml"));
        let sources = StyleSources::for_scene_dir(&tmp.0);
        assert!(sources.owns(&wgsl));
        assert!(!sources.owns(&tmp.0.join("demo.scene.toml")));
    }
}
