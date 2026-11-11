//! Every file a scene, prefab, particle effect, glyph set, chunk, user style
//! or script names by path, checked to be there. Lifted from `flint bundle`
//! (#98), where it ran only over the packed files; it reads through
//! `flint_core::vfs`, so the same pass serves the disk.
//!
//! A reference here is a bare string, so its kind is not known: a string
//! that ends in an asset extension is looked for in every asset folder at
//! each step of the project's walk (the naming file's folder, each folder
//! above it up to the root, then the mounted engine). Where the kind *is*
//! known (a scene's `model.asset`, `script.source`...), the scene pass asks
//! the project's one resolver instead (`scene::ASSET_FIELDS`), and this pass
//! leaves those fields of a scene alone.

use super::{scene, Subject};
use flint_core::diagnostic::{codes, Diagnostic};
use flint_core::project::{AssetKind, Project};
use flint_core::vfs::{self, normalize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Extensions that mark a string in a scene or script as a file.
const ASSET_EXTENSIONS: &[&str] = &[
    "glb", "gltf", "png", "jpg", "jpeg", "hdr", "ktx2", "ogg", "wav", "mp3", "flac", "rhai", "ttf",
    "otf", "toml", "wgsl",
];

/// The folders the runtime looks a bare name up in, at each step of the
/// project's walk (the kinds' folders, `flint_core::project::AssetKind`).
const ASSET_DIRS: &[&str] = &[
    "",
    "models",
    "scripts",
    "audio",
    "textures",
    "sprites",
    "particles",
    "wooze",
    "animations",
    "prefabs",
    "fonts",
    "terrain",
    "specs",
    "levels",
    "scenes",
];

/// The TOML files whose strings are read for references.
fn is_scanned_toml(path: &str) -> bool {
    path.ends_with(".scene.toml")
        || path.ends_with(".prefab.toml")
        || path.ends_with(".particles.toml")
        || path.ends_with(".glyphs.toml")
        || path.ends_with(".chunk.toml")
        || path.ends_with(flint_render::stylize::custom::SUFFIX)
}

/// Whether this pass reads `path` at all: one of the TOML kinds, or a script.
pub(super) fn is_scanned(path: &str) -> bool {
    is_scanned_toml(path) || path.ends_with(".rhai")
}

/// Whether a string names a file: a known extension after a non-empty stem,
/// and nothing that can't be in a path. `".ogg"` (a suffix a script appends
/// to) and URLs don't count.
pub(super) fn looks_like_file(s: &str) -> bool {
    if s.contains("://") || s.contains(char::is_whitespace) || s.contains(['{', '}', '"', '*']) {
        return false;
    }
    let name = s.rsplit(['/', '\\']).next().unwrap_or(s);
    // ".scene.toml" names a suffix (a script stripping it), not a file
    if name.starts_with('.') {
        return false;
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) => {
            !stem.is_empty() && ASSET_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
        }
        None => false,
    }
}

/// Where the runtime would look for `name` named from a file in `from_dir`:
/// the project's one walk (ADR 0114), from the naming file's folder through
/// every folder above it to the root, then the mounted engine. The
/// reference is a bare string, so every asset folder is tried at each step
/// where the runtime tries only the kind's. Each path once, in order.
pub(super) fn candidates(name: &str, from_dirs: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |path: String| {
        if !out.contains(&path) {
            out.push(path);
        }
    };
    for dir in from_dirs {
        // `scenes/act1` -> `scenes` -> `` (the root), each once
        for base in Path::new(dir).ancestors() {
            for sub in ASSET_DIRS {
                push(normalize(&base.join(sub).join(name)));
            }
        }
    }
    for root in ["", "engine"] {
        for sub in ASSET_DIRS {
            push(normalize(&Path::new(root).join(sub).join(name)));
        }
    }
    out
}

/// One string that names a file, and where in its document it stands.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Reference {
    pub name: String,
    /// The keys down to the string: `["entities", "raft", "model", "asset"]`.
    pub at: Vec<String>,
}

impl Reference {
    /// The entity, component and dotted field of a string under
    /// `[entities.<entity>.<component>]`; else only the dotted key path.
    fn place(&self, d: Diagnostic) -> Diagnostic {
        match self.at.as_slice() {
            [first, entity, component, field @ ..] if first == "entities" && !field.is_empty() => d
                .entity(entity.clone())
                .component(component.clone())
                .field(field.join(".")),
            [] => d,
            keys => d.field(keys.join(".")),
        }
    }
}

/// Collect every string in a TOML value that names a file, plus each
/// `model.asset` (a bare model name, as `<name>.glb`). `typed` leaves out
/// what the scene pass checks through the resolver: the fields of
/// [`scene::ASSET_FIELDS`] under `[entities.*]`, and the scene tables' own.
pub(super) fn toml_refs(value: &toml::Value, typed: bool) -> Vec<Reference> {
    fn walk(value: &toml::Value, at: &mut Vec<String>, typed: bool, out: &mut Vec<Reference>) {
        match value {
            toml::Value::String(s) if looks_like_file(s) => out.push(Reference {
                name: s.clone(),
                at: at.clone(),
            }),
            toml::Value::Table(t) => {
                let key = at.last().cloned().unwrap_or_default();
                for (k, v) in t {
                    at.push(k.clone());
                    let skip = typed && scene::is_asset_field(at);
                    let bare_model = key == "model"
                        && k == "asset"
                        && v.as_str().is_some_and(|name| !looks_like_file(name));
                    if skip {
                        // the scene pass asks the resolver
                    } else if bare_model {
                        out.push(Reference {
                            name: format!("{}.glb", v.as_str().unwrap_or_default()),
                            at: at.clone(),
                        });
                    } else {
                        walk(v, at, typed, out);
                    }
                    at.pop();
                }
            }
            toml::Value::Array(a) => a.iter().for_each(|v| walk(v, at, typed, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(value, &mut Vec::new(), typed, &mut out);
    out
}

/// String literals in a Rhai script that name files. This is all `flint
/// check` knows about a script today; `flint script check` (#694) is where
/// compiling it and checking its calls will plug in, as another pass over
/// the same files.
pub(super) fn rhai_refs(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(start) = rest.find('"') {
        let after = &rest[start + 1..];
        let Some(end) = after.find(['"', '\n']) else {
            break;
        };
        let literal = &after[..end];
        if looks_like_file(literal) {
            out.push(literal.to_string());
        }
        rest = &after[(end + 1).min(after.len())..];
    }
    // `// "foo.png"` in a comment is still worth packing; no need to skip.
    out
}

pub(super) fn parent_of(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .unwrap_or_default()
}

/// Every reference in `files` (paths under the subject's root) that names
/// nothing there: a `missing-file` naming the file it is in, where in that
/// file, and every path tried. A file that should be TOML and isn't is a
/// `toml-parse`, except a scene, whose own pass has said so.
pub(super) fn check(subject: &Subject, files: &BTreeSet<String>) -> Vec<Diagnostic> {
    let scene_dirs: Vec<String> = files
        .iter()
        .filter(|p| p.ends_with(".scene.toml"))
        .map(|p| parent_of(p))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let is_file = |key: &str| vfs::is_file(subject.path(key));
    // A bare model name may be a glTF, or a procgen spec the player builds:
    // among the files in hand (a bundle's are all there is), or where the
    // project's search finds one from the naming file.
    let model_ok = |name: &str, from: &str| {
        let stem = name.trim_end_matches(".glb");
        let in_hand = files.iter().any(|p| {
            p.ends_with(&format!("/{stem}.gltf"))
                || p.ends_with(&format!("/{stem}.procgen.toml"))
                || p == &format!("{stem}.procgen.toml")
        });
        in_hand || {
            let from = subject.path(from);
            let dir = from.parent().unwrap_or(Path::new("."));
            let project = Project::for_scene(&from);
            [AssetKind::Models, AssetKind::Specs]
                .iter()
                .any(|kind| project.explain(*kind, stem, dir).found.is_some())
        }
    };

    let mut found = Vec::new();
    for path in files {
        let shown = subject.path(path);
        // A scene this run checks has its typed fields asked of the resolver
        let is_scene = subject.scenes.contains(path);
        let refs: Vec<Reference> = if is_scanned_toml(path) {
            let Ok(bytes) = vfs::read(&shown) else {
                continue; // a scene's own pass reports a scene it can't read
            };
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            match text.parse::<toml::Value>() {
                Ok(value) => toml_refs(&value, is_scene),
                Err(_) if is_scene => continue,
                Err(e) => {
                    found.push(
                        Diagnostic::error(codes::TOML_PARSE, e.to_string().trim_end()).file(&shown),
                    );
                    continue;
                }
            }
        } else if path.ends_with(".rhai") {
            let Ok(bytes) = vfs::read(&shown) else {
                continue;
            };
            rhai_refs(&String::from_utf8_lossy(&bytes))
                .into_iter()
                .map(|name| Reference {
                    name,
                    at: Vec::new(),
                })
                .collect()
        } else {
            continue;
        };

        let mut from_dirs = vec![parent_of(path)];
        if path.ends_with(".rhai") {
            from_dirs.extend(scene_dirs.iter().cloned());
        }
        let mut seen = BTreeSet::new();
        for reference in refs {
            if !seen.insert(reference.name.clone()) {
                continue;
            }
            let name = &reference.name;
            let tried = candidates(name, &from_dirs);
            if tried.iter().any(|c| is_file(c)) || (name.ends_with(".glb") && model_ok(name, path))
            {
                continue;
            }
            let what = if path.ends_with(".rhai") {
                "a string in this script names"
            } else {
                "names"
            };
            found.push(
                reference
                    .place(Diagnostic::error(
                        codes::MISSING_FILE,
                        format!("{what} `{name}`, which is in none of the asset folders"),
                    ))
                    .file(&shown)
                    .tried(tried.iter().map(|c| subject.path(c)))
                    .hint(
                        "add the file, or correct the name; `flint asset where <kind> <name>` \
                         shows where one kind of asset is searched for",
                    ),
            );
        }
    }
    found
}

/// The path of a found file as a key under the subject's root, if it is
/// under it: what a single scene's check follows to the scripts, prefabs
/// and effects the scene names.
pub(super) fn key_under(root: &Path, path: &Path) -> Option<String> {
    let rel: PathBuf = if root.as_os_str().is_empty() {
        path.to_path_buf()
    } else {
        path.strip_prefix(root).ok()?.to_path_buf()
    };
    Some(normalize(&rel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::{Schemas, Subject};
    use flint_core::vfs::MemorySource;
    use std::sync::Arc;

    #[test]
    fn file_strings() {
        assert!(looks_like_file("audio/wash_loop_1.ogg"));
        assert!(looks_like_file("raft.rhai"));
        assert!(looks_like_file("textures\\rain_drop.png"));
        assert!(!looks_like_file(".ogg"));
        assert!(!looks_like_file(".scene.toml"));
        assert!(!looks_like_file("audio/music_"));
        assert!(!looks_like_file("https://x.com/a.png"));
        assert!(!looks_like_file("Press any key.png now"));
        assert!(!looks_like_file("1.5"));
    }

    #[test]
    fn rhai_literals() {
        let src = r#"
            play_sound("audio/splash_" + n + ".ogg");
            let t = "textures/rain_drop.png"; // "c.png"
            draw_sprite("title_setdrift.png", 0.0);
            let s = "no newline
        "#;
        assert_eq!(
            rhai_refs(src),
            ["textures/rain_drop.png", "c.png", "title_setdrift.png"]
        );
    }

    /// What the pass finds in `files`, as a bundle holds them: `(file,
    /// name)` for each missing reference.
    fn missing(files: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut source = MemorySource::new();
        for (path, text) in files {
            source.insert(*path, *text);
        }
        let keys: BTreeSet<String> = files.iter().map(|(p, _)| p.to_string()).collect();
        let subject = Subject::packed(keys.iter().cloned().collect(), Schemas::Listed(Vec::new()));
        // As the bundle runs it: the scenes' typed fields are the scene
        // pass's, so here every file is read as a bundle's prefab would be
        let subject = Subject {
            scenes: Vec::new(),
            ..subject
        };
        vfs::scoped(Arc::new(source), || {
            check(&subject, &keys)
                .into_iter()
                .map(|d| {
                    let name = d.message.split('`').nth(1).unwrap_or_default().to_string();
                    (flint_core::project::shown(&d.file.unwrap()), name)
                })
                .collect()
        })
    }

    fn pair(file: &str, name: &str) -> (String, String) {
        (file.to_string(), name.to_string())
    }

    #[test]
    fn references_resolve_like_the_runtime() {
        let found = missing(&[
            (
                "prefabs/float.prefab.toml",
                r#"
                [prefab]
                name = "Float"
                [entities.raft.model]
                asset = "raft"
                [entities.raft.script]
                source = "raft.rhai"
                [entities.rain.audio_source]
                file = "audio/rain_loop.ogg"
                [entities.tree.model]
                asset = "tree"
                [entities.lost.model]
                asset = "whale"
                "#,
            ),
            ("models/raft.glb", ""),
            ("specs/tree.procgen.toml", ""),
            (
                "scripts/raft.rhai",
                r#"play_sound("audio/creak_1.ogg"); draw_sprite("logo.png")"#,
            ),
            ("audio/rain_loop.ogg", ""),
            ("audio/creak_1.ogg", ""),
        ]);
        assert_eq!(
            found,
            [
                pair("prefabs/float.prefab.toml", "whale.glb"),
                pair("scripts/raft.rhai", "logo.png"),
            ]
        );
    }

    /// The check walks as the runtime does (ADR 0114): from the naming
    /// file's folder through every folder above it to the root, then the
    /// engine. It used to stop one level above the scene.
    #[test]
    fn references_from_a_nested_file_resolve_up_to_the_root_and_the_engine() {
        let found = missing(&[
            (
                "scenes/act1/cave/deep.chunk.toml",
                r#"
                [entities.raft.model]
                asset = "raft"
                [entities.raft.script]
                source = "raft.rhai"
                [entities.wall.material]
                texture = "textures/wall.png"
                [entities.near.material]
                texture = "textures/near.png"
                [entities.lost.material]
                texture = "textures/lost.png"
                "#,
            ),
            ("models/raft.glb", ""),
            ("scripts/raft.rhai", ""),
            ("engine/textures/wall.png", ""),
            ("scenes/act1/textures/near.png", ""),
            // A sibling branch is not on the way up
            ("scenes/act2/textures/lost.png", ""),
        ]);
        assert_eq!(
            found,
            [pair(
                "scenes/act1/cave/deep.chunk.toml",
                "textures/lost.png"
            )]
        );
    }

    #[test]
    fn a_glyph_set_must_have_its_font() {
        let found = missing(&[
            (
                "fonts/terminal.glyphs.toml",
                "[font]\nfile = \"Mono.ttf\"\n[ramps.a]\nglyphs = \" .\"\n",
            ),
            (
                "fonts/crt.glyphs.toml",
                "[font]\nfile = \"crt.png\"\ncell = [8, 8]\n",
            ),
            ("fonts/Mono.ttf", ""),
        ]);
        assert_eq!(found, [pair("fonts/crt.glyphs.toml", "crt.png")]);
    }

    /// A missing file says where in its document it is named, and every
    /// path that was tried.
    #[test]
    fn a_missing_file_names_its_place_and_the_paths_tried() {
        let mut source = MemorySource::new();
        source.insert(
            "prefabs/kart.prefab.toml",
            "[prefab]\nname = \"kart\"\n[entities.body.material]\ntexture = \"paint.png\"\n",
        );
        let keys: BTreeSet<String> = ["prefabs/kart.prefab.toml".to_string()].into();
        let subject = Subject::packed(keys.iter().cloned().collect(), Schemas::Listed(Vec::new()));
        let found = vfs::scoped(Arc::new(source), || check(&subject, &keys));
        assert_eq!(found.len(), 1);
        let d = &found[0];
        assert_eq!(d.code, codes::MISSING_FILE);
        assert_eq!(d.entity.as_deref(), Some("body"));
        assert_eq!(d.component.as_deref(), Some("material"));
        assert_eq!(d.field.as_deref(), Some("texture"));
        let tried: Vec<String> = d
            .tried
            .iter()
            .map(|p| flint_core::project::shown(p))
            .collect();
        assert_eq!(tried[0], "prefabs/paint.png");
        assert!(
            tried.contains(&"textures/paint.png".to_string()),
            "{tried:?}"
        );
        assert!(
            tried.contains(&"engine/textures/paint.png".to_string()),
            "{tried:?}"
        );
        let unique: BTreeSet<&String> = tried.iter().collect();
        assert_eq!(unique.len(), tried.len(), "each path once");
    }
}
