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
use flint_core::project::{AssetKind, AssetScope, Project};
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
        // a music suite's stems (#382: `music_session.manifest` names one)
        || path.ends_with(".suite.toml")
        || path.ends_with(flint_render::stylize::custom::SUFFIX)
}

/// Whether this pass reads `path` at all: one of the TOML kinds, a script,
/// or a glTF model (#382: its textures and buffers may be files beside it).
pub(super) fn is_scanned(path: &str) -> bool {
    is_scanned_toml(path) || path.ends_with(".rhai") || is_gltf(path)
}

fn is_gltf(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".glb") || lower.ends_with(".gltf")
}

/// The files a glTF model names by URI: its external buffers and images,
/// percent-decoded. A `data:` URI is held inside, and a URL is nobody's to
/// pack. Bytes that are not a glTF name nothing.
pub(super) fn gltf_uris(bytes: &[u8]) -> Vec<String> {
    // A .glb: a 12-byte header, then the JSON chunk's length and type
    let json = if bytes.starts_with(b"glTF") {
        let Some(len) = bytes.get(12..16) else {
            return Vec::new();
        };
        let len = u32::from_le_bytes([len[0], len[1], len[2], len[3]]) as usize;
        match (bytes.get(16..20), bytes.get(20..20 + len)) {
            (Some(b"JSON"), Some(json)) => json,
            _ => return Vec::new(),
        }
    } else {
        bytes
    };
    let Ok(doc) = serde_json::from_slice::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for list in ["buffers", "images"] {
        for entry in doc
            .get(list)
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            if let Some(uri) = entry.get("uri").and_then(|u| u.as_str()) {
                if !uri.starts_with("data:") && !uri.contains("://") {
                    out.push(percent_decoded(uri));
                }
            }
        }
    }
    out
}

/// `%20` as a space, and so on: a glTF URI is a URI.
fn percent_decoded(uri: &str) -> String {
    let bytes = uri.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `dir/rel` with `.` and `..` folded away, as a key under the root, or
/// `None` when it climbs above the root.
pub(super) fn joined(dir: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in dir.split('/').chain(rel.split(['/', '\\'])) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
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

/// Whether a string a script holds names a sound file: what `play_sound`
/// and `play_sound_at` take.
fn is_sound_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    AssetKind::Audio
        .info()
        .extensions
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// A script's sound that none of `scopes` (each scene that may run the
/// script) finds as the player looks for it (#975): the player's preload
/// keys only the files of the `audio/` folders a scene sees, and a name it
/// didn't cache is looked for, on its first play, by this same search.
/// `None` when one of them finds it.
fn missing_sound(name: &str, scopes: &[AssetScope]) -> Option<Diagnostic> {
    let mut tried: Vec<PathBuf> = Vec::new();
    for scope in scopes {
        let resolution = scope.explain(AssetKind::Audio, name);
        if resolution.found.is_some() {
            return None;
        }
        for path in resolution.tried {
            if !tried.contains(&path) {
                tried.push(path);
            }
        }
    }
    Some(
        Diagnostic::error(
            codes::MISSING_FILE,
            format!("a string in this script names the sound `{name}`, which no scene finds"),
        )
        .tried(tried)
        .hint(
            "a sound a script plays is found as an `audio_source.file` is: a path from the \
             scene's folder or one above it, up to the project root, or a file in an `audio/` \
             there; `flint asset where audio <name> --scene <scene>` shows the search",
        ),
    )
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
    let scenes: BTreeSet<&String> = files
        .iter()
        .chain(&subject.scenes)
        .filter(|p| p.ends_with(".scene.toml"))
        .collect();
    let scene_dirs: Vec<String> = scenes
        .iter()
        .map(|p| parent_of(p))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    // A script's sound is found by the player's search from the scene
    // that runs it (#975), the one an `audio_source.file` takes: each
    // scene's project and folder, once per folder.
    let sound_scopes: Vec<AssetScope> = {
        let mut seen = BTreeSet::new();
        scenes
            .iter()
            .filter(|scene| seen.insert(parent_of(scene)))
            .map(|scene| AssetScope::for_scene(&subject.path(scene)))
            .collect()
    };
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
        } else if is_gltf(path) {
            // The importer reads these from beside the model, and fails
            // the whole model when one isn't there
            let Ok(bytes) = vfs::read(&shown) else {
                continue;
            };
            let dir = parent_of(path);
            for uri in gltf_uris(&bytes) {
                let target = joined(&dir, &uri);
                if target.as_deref().is_some_and(is_file) {
                    continue;
                }
                let d = Diagnostic::error(
                    codes::MISSING_FILE,
                    format!("the model names `{uri}`, which is not there"),
                )
                .file(&shown)
                .hint(
                    "a glTF loads its textures and buffers from beside it, and without one \
                     the whole model is skipped: add the file, or embed it (a .glb can hold them)",
                );
                found.push(match target {
                    Some(target) => d.tried([subject.path(&target)]),
                    None => d,
                });
            }
            continue;
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
            if path.ends_with(".rhai") && is_sound_file(name) {
                let scopes = match sound_scopes.as_slice() {
                    // No scene in hand: as if one stood beside the script
                    [] => vec![AssetScope::for_scene(&shown.with_extension("scene.toml"))],
                    scopes => scopes.to_vec(),
                };
                if let Some(d) = missing_sound(name, &scopes) {
                    found.push(d.file(&shown));
                }
                continue;
            }
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

    /// A .glb: the header, then its JSON as the one chunk.
    fn glb(json: &str) -> Vec<u8> {
        let mut json = json.as_bytes().to_vec();
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let mut out = b"glTF".to_vec();
        out.extend(2u32.to_le_bytes());
        out.extend(((20 + json.len()) as u32).to_le_bytes());
        out.extend((json.len() as u32).to_le_bytes());
        out.extend(b"JSON");
        out.extend(json);
        out
    }

    #[test]
    fn gltf_uris_are_its_files_outside_it() {
        let json = r#"{"buffers": [{"uri": "data:application/octet-stream;base64,AAAA"},
            {"uri": "bar.bin"}], "images": [{"uri": "../textures/old%20oak.png"},
            {"bufferView": 3}, {"uri": "https://x.com/a.png"}]}"#;
        let want = ["bar.bin", "../textures/old oak.png"];
        assert_eq!(gltf_uris(json.as_bytes()), want);
        assert_eq!(gltf_uris(&glb(json)), want);
        assert!(gltf_uris(b"").is_empty());
        assert!(gltf_uris(b"glTF").is_empty());
        assert_eq!(
            joined("models", "../textures/a.png").as_deref(),
            Some("textures/a.png")
        );
        assert_eq!(joined("", "./a.bin").as_deref(), Some("a.bin"));
        assert_eq!(joined("models", "../../a.png"), None);
    }

    /// #382: the tavern's models name their textures in `../textures/`,
    /// and a model whose image isn't packed is skipped whole by the player.
    #[test]
    fn a_model_must_have_the_files_it_names() {
        let mut source = MemorySource::new();
        let json = r#"{"images": [{"uri": "../textures/tavern/plank.jpg"},
            {"uri": "../textures/tavern/iron.jpg"}]}"#;
        source.insert("levels/models/bar.glb", glb(json));
        source.insert("levels/textures/tavern/plank.jpg", "jpg");
        let keys: BTreeSet<String> = ["levels/models/bar.glb".to_string()].into();
        let subject = Subject::packed(keys.iter().cloned().collect(), Schemas::Listed(Vec::new()));
        let found = vfs::scoped(Arc::new(source), || check(&subject, &keys));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, codes::MISSING_FILE);
        assert!(found[0].message.contains("../textures/tavern/iron.jpg"));
        let tried: Vec<String> = found[0]
            .tried
            .iter()
            .map(|p| flint_core::project::shown(p))
            .collect();
        assert_eq!(tried, ["levels/textures/tavern/iron.jpg"]);
    }

    /// #382: a music suite names its stems, from the project root.
    #[test]
    fn a_music_suite_must_have_its_stems() {
        let found = missing(&[
            (
                "levels/music/clock.suite.toml",
                "[suite]\nid = \"c\"\n[buses.pad]\nfile = \"levels/music/stems/pad.flac\"\n\
                 [buses.bell]\nfile = \"levels/music/stems/bell.flac\"\n",
            ),
            ("levels/music/stems/pad.flac", ""),
        ]);
        assert_eq!(
            found,
            [pair(
                "levels/music/clock.suite.toml",
                "levels/music/stems/bell.flac"
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

    /// A short real WAV, for the engine to decode.
    fn wav() -> Vec<u8> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut bytes = std::io::Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(&mut bytes, spec).unwrap();
        for i in 0..2_000 {
            writer.write_sample(((i % 50) * 400) as i16).unwrap();
        }
        writer.finalize().unwrap();
        bytes.into_inner()
    }

    /// #975: `flint check` and the player agree on whether a script's sound
    /// plays. Each case is asked of both: the check over the game's files,
    /// and the audio engine with the scene's search set as the player sets
    /// it, playing the name as the script wrote it. A path from the scene
    /// to a file outside `audio/` used to pass the check and be silent; a
    /// sound in a folder only the check looked in (beside the script, in
    /// `models/`) passed too and never played.
    #[test]
    fn a_scripts_sound_passes_the_check_exactly_when_the_player_plays_it() {
        // (the name the script plays, the file there is, whether it plays)
        let cases = [
            ("audio/bell.wav", Some("audio/bell.wav"), true),
            ("../sfx/door.wav", Some("sfx/door.wav"), true),
            ("levels/../sfx/door.wav", Some("sfx/door.wav"), true),
            ("click.wav", Some("scripts/click.wav"), false),
            ("thud.wav", Some("models/thud.wav"), false),
            ("gone.wav", None, false),
        ];
        let script: String = cases
            .iter()
            .map(|(name, ..)| format!("play_sound(\"{name}\");\n"))
            .collect();
        let mut source = MemorySource::new();
        source.insert("flint.toml", "[project]\nid = \"sounds\"\n");
        source.insert(
            "levels/a.scene.toml",
            "[entities.speaker.script]\nsource = \"speaker.rhai\"\n",
        );
        source.insert("scripts/speaker.rhai", script);
        let mut keys: BTreeSet<String> = ["levels/a.scene.toml", "scripts/speaker.rhai"]
            .into_iter()
            .map(String::from)
            .collect();
        for (_, file, _) in &cases {
            if let Some(file) = file {
                source.insert(*file, wav());
                keys.insert(file.to_string());
            }
        }
        let subject = Subject::packed(keys.iter().cloned().collect(), Schemas::Listed(Vec::new()));
        let subject = Subject {
            scenes: Vec::new(),
            ..subject
        };

        vfs::scoped(Arc::new(source), || {
            let refused: Vec<String> = check(&subject, &keys)
                .into_iter()
                .map(|d| d.message.split('`').nth(1).unwrap_or_default().to_string())
                .collect();
            let mut engine = flint_audio::engine::AudioEngine::silent();
            engine.start_remote(flint_audio::remote::Link::default());
            engine.set_sound_search(Some(AssetScope::for_scene(Path::new(
                "levels/a.scene.toml",
            ))));
            for (name, _, plays) in cases {
                let played = engine
                    .play_non_spatial(name, 1.0, 1.0, false, flint_audio::Bus::SFX)
                    .is_ok();
                let passed = !refused.iter().any(|r| r == name);
                assert_eq!(played, plays, "the engine on `{name}`");
                assert_eq!(passed, plays, "the check on `{name}`");
            }
        });
    }
}
