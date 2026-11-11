//! The `data` module: TOML data files a script reads (item tables, level
//! lists), from the project root, parsed once per path and cached for the
//! session. Types are kept exactly: a TOML integer is an int, a float a
//! float. ADR 0116.

use super::builder::kind::{Bool, Map as MapKind, Str};
use super::builder::ScriptApi;
use super::convert::toml_to_dynamic;
use crate::context::{LogLevel, ScriptCallContext, ScriptCommand};
use flint_core::vfs;
use rhai::Dynamic;

/// Read and cache a TOML file from the project root, returning the whole
/// document as a map; `()` on failure, which is also logged.
pub(crate) fn load_data(c: &mut ScriptCallContext, path: &str, reload: bool) -> Dynamic {
    let full = c.project_root_holding("load_data", path).join(path);
    let key = full.to_string_lossy().to_string();
    if reload {
        c.data_cache.remove(&key);
    }
    if let Some(v) = c.data_cache.get(&key) {
        return toml_to_dynamic(v.clone());
    }
    let parsed = vfs::read_to_string(&full)
        .map_err(|e| e.to_string())
        .and_then(|text| text.parse::<toml::Value>().map_err(|e| e.to_string()));
    match parsed {
        Ok(v) => {
            let d = toml_to_dynamic(v.clone());
            c.data_cache.insert(key, v);
            d
        }
        Err(e) => {
            let message = format!("load_data('{}') failed (resolved to {}): {}", path, key, e);
            tracing::warn!("{}", message);
            c.commands.push(ScriptCommand::Log {
                level: LogLevel::Warn,
                message,
            });
            Dynamic::UNIT
        }
    }
}

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("data");

    m.func(
        "load_data",
        "A TOML data file from the project root, parsed once and cached for the session.",
    )
    .arg("path", Str)
    .returns(
        MapKind,
        "the document as a map; () when it is missing or doesn't parse (logged)",
    )
    .run(|call, (path,)| load_data(call.ctx(), &path, false));

    m.func(
        "load_data",
        "A TOML data file from the project root; reload true reads it again in place of the cached parse.",
    )
    .arg("path", Str)
    .arg("reload", Bool)
    .returns(
        MapKind,
        "the document as a map; () when it is missing or doesn't parse (logged)",
    )
    .run(|call, (path, reload)| load_data(call.ctx(), &path, reload));
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhai::{Array, Map};
    use std::sync::Arc;

    /// `load_data` and `load_ui` paths are from the project root (ADR
    /// 0114): the same file from a scene in `scenes/` and from one two
    /// folders deep, which used to look under `scenes/`.
    #[test]
    fn test_root_paths_are_from_the_project_root_at_any_depth() {
        use flint_core::project::Project;
        use flint_core::vfs::{self, MemorySource};
        let mut files = MemorySource::new();
        files.insert("game/flint.toml", "[project]\nid = \"g\"\n");
        files.insert("game/data/x.toml", "n = 7\n");
        files.insert("game/scenes/a.scene.toml", "");
        files.insert("game/scenes/act1/deep/b.scene.toml", "");
        // TEMPORARY (one release): a file only where the root used to be
        // guessed for the deep scene is still read
        files.insert("game/scenes/act1/data/old.toml", "n = 3\n");
        // No marker: the scene folder's parent, as before
        files.insert("loose/levels/c.scene.toml", "");
        files.insert("loose/data/x.toml", "n = 9\n");
        vfs::scoped(Arc::new(files), || {
            let n = |scene: &str, handed: bool, file: &str| -> Option<i64> {
                let mut c = ScriptCallContext::new();
                c.current_scene_path = scene.to_string();
                // With the project handed in by the host, and found from
                // the scene path when it wasn't
                if handed {
                    c.project = Some(Project::for_scene(std::path::Path::new(scene)));
                }
                let d = load_data(&mut c, file, false);
                d.is_map().then(|| d.cast::<Map>()["n"].as_int().unwrap())
            };
            for handed in [true, false] {
                assert_eq!(
                    n("game/scenes/a.scene.toml", handed, "data/x.toml"),
                    Some(7)
                );
                assert_eq!(
                    n("game/scenes/act1/deep/b.scene.toml", handed, "data/x.toml"),
                    Some(7)
                );
                assert_eq!(
                    n(
                        "game/scenes/act1/deep/b.scene.toml",
                        handed,
                        "data/old.toml"
                    ),
                    Some(3)
                );
                assert_eq!(n("game/scenes/a.scene.toml", handed, "data/old.toml"), None);
                assert_eq!(
                    n("loose/levels/c.scene.toml", handed, "data/x.toml"),
                    Some(9)
                );
            }
        });
    }

    #[test]
    fn test_load_data_types_cache_and_missing() {
        let root = std::env::temp_dir().join(format!(
            "flint_data_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        std::fs::write(root.join("data/x.toml"), "n = 7\nf = 1.5\nb = true\ns = \"hi\"\n[nested]\nk = 2\n[[list]]\nid = \"a\"\n[[list]]\nid = \"b\"\n").unwrap();

        let mut c = ScriptCallContext::new();
        c.current_scene_path = root
            .join("scenes/x.scene.toml")
            .to_string_lossy()
            .to_string();

        let d = load_data(&mut c, "data/x.toml", false);
        assert!(d.is_map());
        let m = d.cast::<Map>();
        assert!(m["n"].is_int());
        assert_eq!(m["n"].as_int().unwrap(), 7);
        assert!(m["f"].is_float());
        assert!(m["b"].as_bool().unwrap());
        assert_eq!(m["s"].clone().into_string().unwrap(), "hi");
        assert_eq!(m["nested"].clone().cast::<Map>()["k"].as_int().unwrap(), 2);
        assert_eq!(m["list"].clone().cast::<Array>().len(), 2);
        assert_eq!(c.data_cache.len(), 1);

        // Cached: the file can change on disk and the same parse comes back
        std::fs::write(root.join("data/x.toml"), "n = 8\n").unwrap();
        let d2 = load_data(&mut c, "data/x.toml", false);
        assert_eq!(d2.cast::<Map>()["n"].as_int().unwrap(), 7);
        // ...until a reload is asked for
        let d3 = load_data(&mut c, "data/x.toml", true);
        assert_eq!(d3.cast::<Map>()["n"].as_int().unwrap(), 8);

        // Missing file: unit, and a warning queued for the game log
        let d4 = load_data(&mut c, "data/missing.toml", false);
        assert!(d4.is_unit());
        assert!(c.commands.iter().any(|cmd| matches!(
            cmd,
            ScriptCommand::Log {
                level: LogLevel::Warn,
                ..
            }
        )));

        let _ = std::fs::remove_dir_all(&root);
    }
}
