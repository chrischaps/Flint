//! Project fonts for the script HUD.
//!
//! Every `fonts/*.ttf|*.otf` the scene can see by the project's one asset
//! search (ADR 0114; `AssetKind::FontFaces`: `fonts/` in the scene's
//! directory, in each directory above it up to the project root, then in
//! the mounted engine, a nearer file hiding a farther one of its name) is
//! registered with egui under its file stem as a named `FontFamily`. The
//! nearest `fonts/fonts.toml` (optional) adds aliases:
//!
//! ```toml
//! [[font]]
//! name = "display"                     # alias → FontFamily::Name("display")
//! file = "BarlowCondensed-BlackItalic.ttf"
//!
//! [[font]]
//! name = "terminal"                    # a path from the manifest's folder (#1179)
//! file = "ofl/VT323/VT323-Regular.ttf"
//!
//! [egui]                               # optional (ADR 0081)
//! proportional = "body"                # egui's default text face
//! monospace = "code"                   # egui's monospace face
//! ```
//!
//! An alias's `file` is a file name in `fonts/`, or a relative path with a
//! folder in it, read from the manifest's own folder (below it, or beside it
//! with `../`), so a face a glyph set keeps in `fonts/ofl/` is a HUD font
//! without a second copy. A path's face is registered under the alias alone.
//!
//! The `[egui]` names are families (a file stem or an alias). The named face
//! goes first in egui's `Proportional` or `Monospace` family, ahead of egui's
//! own fonts, so a game's egui UI draws in it and keeps egui's fonts for the
//! glyphs it lacks. Every named family keeps egui's proportional fonts as
//! fallbacks the same way.
//!
//! A script names egui's two families as `"proportional"` and `"monospace"`
//! (#1179; [`family_of`]): text in a fixed pitch with no font file of the
//! game's, and the game's own `[egui] monospace` face when it names one. A
//! project family of either name comes first.
//!
//! An extension can add fonts of its own with `PlayerExtension::fonts`,
//! which edits the definitions before Flint installs them, so they survive
//! every reload.
//!
//! Both the renderer (`hud_render`) and the script `measure_text` closure
//! build their layout jobs through [`text_layout_job`] so measurement and
//! drawing can never disagree.

use flint_core::project::{AssetKind, Listed, Project};
use flint_core::toml_util::{check_keys, KeyIssue, Shape};
use flint_core::vfs;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One manifest alias entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontAlias {
    pub name: String,
    pub file: String,
}

/// The keys a `fonts.toml` may hold (#282).
const MANIFEST: Shape = Shape::Table(&[
    (
        "font",
        Shape::Tables(&Shape::Table(&[
            ("name", Shape::Value),
            ("file", Shape::Value),
        ])),
    ),
    (
        "egui",
        Shape::Table(&[("proportional", Shape::Value), ("monospace", Shape::Value)]),
    ),
]);

/// The keys of a `fonts.toml` manifest that will be ignored: misspelled,
/// in the wrong table, or a `[font]` where the format wants `[[font]]`.
/// Empty for a document that doesn't parse (`parse_manifest` says why).
#[cfg(test)]
fn manifest_issues(text: &str) -> Vec<KeyIssue> {
    text.parse::<toml::Value>()
        .map(|doc| manifest_doc_issues(&doc))
        .unwrap_or_default()
}

fn manifest_doc_issues(doc: &toml::Value) -> Vec<KeyIssue> {
    check_keys(doc, &MANIFEST)
}

/// Parse a `fonts.toml` manifest. Entries missing `name` or `file` are
/// dropped with a warning; a malformed document yields an empty list.
/// Keys the format doesn't know are warned about (each one a `KeyIssue`).
pub fn parse_manifest(text: &str) -> Vec<FontAlias> {
    let doc: toml::Value = match text.parse() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("fonts.toml: parse error: {e}");
            return Vec::new();
        }
    };
    for issue in manifest_doc_issues(&doc) {
        tracing::warn!("fonts.toml: {issue}");
    }
    let Some(entries) = doc.get("font").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|e| {
            let name = e.get("name").and_then(|v| v.as_str());
            let file = e.get("file").and_then(|v| v.as_str());
            match (name, file) {
                (Some(n), Some(f)) if !n.is_empty() && !f.is_empty() => Some(FontAlias {
                    name: n.to_string(),
                    file: f.to_string(),
                }),
                _ => {
                    tracing::warn!("fonts.toml: [[font]] entry needs both `name` and `file`: {e}");
                    None
                }
            }
        })
        .collect()
}

/// The `[egui]` table of a `fonts.toml`: which families egui's default
/// families start with. `None` keeps egui's own face.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EguiDefaults {
    pub proportional: Option<String>,
    pub monospace: Option<String>,
}

/// Parse the `[egui]` table of a `fonts.toml` manifest. A missing table, a
/// malformed document or a non-string value yields `None` for that slot
/// (`parse_manifest` already warns about a parse error).
pub fn parse_egui_defaults(text: &str) -> EguiDefaults {
    let Ok(doc) = text.parse::<toml::Value>() else {
        return EguiDefaults::default();
    };
    let Some(table) = doc.get("egui") else {
        return EguiDefaults::default();
    };
    let slot = |key: &str| match table.get(key) {
        Some(toml::Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(other) => {
            tracing::warn!("fonts.toml: [egui] {key} should be a family name, not {other}");
            None
        }
        None => None,
    };
    EguiDefaults {
        proportional: slot("proportional"),
        monospace: slot("monospace"),
    }
}

/// The `font_data` key an alias's `file` names: the file's stem for a file
/// in `fonts/`, or, for a relative path with a folder in it (#1179), the
/// path itself with forward slashes (`ofl/VT323/VT323-Regular.ttf`), which
/// no stem can be. `None` for an absolute path, which a manifest may not
/// name.
fn alias_key(file: &str) -> Option<String> {
    let spelled = file.replace('\\', "/");
    let path = Path::new(&spelled);
    if spelled.starts_with('/') || path.is_absolute() || path.has_root() || spelled.contains(':') {
        return None;
    }
    let key = vfs::normalize(path);
    if key.contains('/') {
        return Some(key);
    }
    Path::new(&key)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
}

/// Resolve the family table for a set of font files plus manifest aliases.
/// `keys` are the `font_data` keys read: file stems, and the paths aliases
/// named (`alias_key`). Families are the stems, then the aliases; values
/// are the key each family points at. Aliases whose file was not read are
/// dropped with a warning; an alias may not shadow a real stem.
pub fn resolve_families(keys: &[String], aliases: &[FontAlias]) -> BTreeMap<String, String> {
    let stems: Vec<&String> = keys.iter().filter(|k| !k.contains('/')).collect();
    let mut families: BTreeMap<String, String> =
        stems.iter().map(|s| ((*s).clone(), (*s).clone())).collect();
    for alias in aliases {
        let Some(target) = alias_key(&alias.file) else {
            tracing::warn!(
                "fonts.toml: alias `{}` names `{}`, an absolute path; a file is named from the manifest's folder",
                alias.name,
                alias.file
            );
            continue;
        };
        if !keys.contains(&target) {
            let place = if target.contains('/') {
                "which could not be read"
            } else {
                "which is not in fonts/"
            };
            tracing::warn!(
                "fonts.toml: alias `{}` points at `{}`, {place}",
                alias.name,
                alias.file
            );
            continue;
        }
        if stems.contains(&&alias.name) {
            tracing::warn!(
                "fonts.toml: alias `{}` shadows a font file of the same name; ignored",
                alias.name
            );
            continue;
        }
        families.insert(alias.name.clone(), target);
    }
    families
}

/// The path a scene names its font manifest by: a file found like any
/// file a scene names by a path of its own (`AssetKind::Files`).
pub const FONT_MANIFEST: &str = "fonts/fonts.toml";

/// The font files and the manifest a scene sees, by the project's one
/// asset search (ADR 0114).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectFonts {
    /// Every `.ttf` / `.otf` in a `fonts/` the scene can see, nearest
    /// first, a nearer file hiding a farther one of its name.
    pub faces: Vec<Listed>,
    /// The nearest `fonts/fonts.toml`, if there is one.
    pub manifest: Option<PathBuf>,
}

impl ProjectFonts {
    /// What a scene in `scene_dir` of `project` sees.
    pub fn find(project: &Project, scene_dir: &Path) -> Self {
        Self {
            faces: project.list(AssetKind::FontFaces, scene_dir),
            manifest: project
                .resolve(AssetKind::Files, FONT_MANIFEST, scene_dir)
                .ok(),
        }
    }

    /// What the scene at `scene_path` sees, in the project found from it.
    pub fn for_scene(scene_path: &str) -> Self {
        let scene = Path::new(scene_path);
        let dir = scene.parent().filter(|p| !p.as_os_str().is_empty());
        Self::find(
            &Project::for_scene(scene),
            dir.unwrap_or_else(|| Path::new(".")),
        )
    }

    /// Read the files: `(key -> bytes, aliases, egui defaults, paths read
    /// for aliases)`. Of two files of one stem (`Body.ttf` near, `Body.otf`
    /// far) the nearer is kept. A file an alias names by a path (#1179) is
    /// read from the manifest's folder and keyed by that path.
    fn read(
        &self,
    ) -> (
        BTreeMap<String, Vec<u8>>,
        Vec<FontAlias>,
        EguiDefaults,
        Vec<PathBuf>,
    ) {
        let mut data = BTreeMap::new();
        for face in &self.faces {
            let Some(stem) = face.path.file_stem() else {
                continue;
            };
            let stem = stem.to_string_lossy().into_owned();
            if data.contains_key(&stem) {
                continue;
            }
            match vfs::read(&face.path) {
                Ok(bytes) if parses(&bytes, &face.path) => {
                    data.insert(stem, bytes);
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("fonts: failed to read {}: {e}", face.path.display()),
            }
        }
        let Some(manifest) = &self.manifest else {
            return (data, Vec::new(), EguiDefaults::default(), Vec::new());
        };
        let text = match vfs::read_to_string(manifest) {
            Ok(text) => text,
            Err(e) => {
                tracing::warn!("fonts: failed to read {}: {e}", manifest.display());
                return (data, Vec::new(), EguiDefaults::default(), Vec::new());
            }
        };
        let aliases = parse_manifest(&text);
        let folder = manifest.parent().unwrap_or(Path::new(""));
        let mut linked = Vec::new();
        for alias in &aliases {
            let Some(key) = alias_key(&alias.file).filter(|k| k.contains('/')) else {
                continue;
            };
            if data.contains_key(&key) {
                continue;
            }
            let path = PathBuf::from(vfs::normalize(&folder.join(&key)));
            match vfs::read(&path) {
                Ok(bytes) if parses(&bytes, &path) => {
                    data.insert(key, bytes);
                    linked.push(path);
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(
                    "fonts.toml: alias `{}`: failed to read {}: {e}",
                    alias.name,
                    path.display()
                ),
            }
        }
        (data, aliases, parse_egui_defaults(&text), linked)
    }
}

/// Whether egui can open `bytes` as a font face; a warning naming `path`
/// when it can't. egui opens face 0 of every file it is given with skrifa when
/// the fonts are installed, and panics on the first it can't (#1262: an empty
/// `fonts/Mono.ttf` took `flint render --scripts` down). So a face is opened
/// here the same way first, and one that fails is left out: its family is
/// unknown, and text in it draws in egui's default face (`resolve_font_id`).
/// A farther face of the same stem (`Body.otf` behind a broken `Body.ttf`)
/// is read in its place, as for a file that can't be read at all.
fn parses(bytes: &[u8], path: &Path) -> bool {
    match face_error(bytes) {
        None => true,
        Some(e) => {
            tracing::warn!(
                "fonts: skipped {}: not a font face egui can read ({e}); its text draws in egui's default face",
                flint_core::project::shown(path)
            );
            false
        }
    }
}

/// Why `bytes` is no font face, or `None`. skrifa opens a file by its table
/// directory alone and reads each table when asked, so a file cut short
/// opens and draws nothing; the three tables every face has (`head`, `maxp`,
/// `cmap`) are read too, so a truncated file is named here as well.
fn face_error(bytes: &[u8]) -> Option<skrifa::raw::ReadError> {
    use skrifa::raw::TableProvider;
    let font = match skrifa::FontRef::from_index(bytes, 0) {
        Ok(font) => font,
        Err(e) => return Some(e),
    };
    font.head()
        .err()
        .or_else(|| font.maxp().err())
        .or_else(|| font.cmap().err())
}

/// egui's definitions with a project's fonts added: each file under its
/// stem, a named family per stem and alias, and `defaults` first in egui's
/// own families. Every named family falls back to egui's proportional fonts.
/// `families` is `resolve_families`' table.
pub fn build_definitions(
    data: BTreeMap<String, Vec<u8>>,
    families: &BTreeMap<String, String>,
    defaults: &EguiDefaults,
) -> egui::FontDefinitions {
    let mut defs = egui::FontDefinitions::default();
    let fallbacks = defs
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    for (stem, bytes) in data {
        defs.font_data
            .insert(stem, Arc::new(egui::FontData::from_owned(bytes)));
    }
    for (family, target) in families {
        let mut list = vec![target.clone()];
        list.extend(fallbacks.iter().cloned());
        defs.families
            .insert(egui::FontFamily::Name(family.as_str().into()), list);
    }
    for (slot, key, name) in [
        (
            egui::FontFamily::Proportional,
            "proportional",
            &defaults.proportional,
        ),
        (
            egui::FontFamily::Monospace,
            "monospace",
            &defaults.monospace,
        ),
    ] {
        let Some(name) = name else { continue };
        let Some(target) = families.get(name) else {
            tracing::warn!("fonts.toml: [egui] {key} = `{name}` is not a font in fonts/");
            continue;
        };
        let list = defs.families.entry(slot).or_default();
        list.retain(|f| f != target);
        list.insert(0, target.clone());
    }
    defs
}

/// The project's font definitions for a scene path (see `build_definitions`).
/// With no `fonts/` directory these are egui's defaults, so a scene change
/// to a font-less project drops stale families.
pub fn project_font_definitions(scene_path: &str) -> egui::FontDefinitions {
    font_definitions(&ProjectFonts::for_scene(scene_path))
}

/// The font definitions for what a scene sees (see `build_definitions`):
/// egui's defaults when it sees no font file.
pub fn font_definitions(fonts: &ProjectFonts) -> egui::FontDefinitions {
    if fonts.faces.is_empty() && fonts.manifest.is_none() {
        return egui::FontDefinitions::default();
    }
    let (data, aliases, defaults, linked) = fonts.read();
    let keys: Vec<String> = data.keys().cloned().collect();
    let families = resolve_families(&keys, &aliases);
    if !families.is_empty() {
        let mut folders: Vec<String> = Vec::new();
        for file in fonts.faces.iter().map(|f| &f.path).chain(&linked) {
            let folder = file.parent().unwrap_or(Path::new(""));
            let folder = flint_core::project::shown(folder);
            if !folders.contains(&folder) {
                folders.push(folder);
            }
        }
        tracing::info!(
            "fonts: loaded {} famil{} from {}: {}",
            families.len(),
            if families.len() == 1 { "y" } else { "ies" },
            folders.join(", "),
            families
                .keys()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    build_definitions(data, &families, &defaults)
}

/// The family names scripts may refer to: every named family in `defs`,
/// the project's and any an extension added.
pub fn script_families(defs: &egui::FontDefinitions) -> HashSet<String> {
    defs.families
        .keys()
        .filter_map(|f| match f {
            egui::FontFamily::Name(name) => Some(name.to_string()),
            _ => None,
        })
        .collect()
}

/// The name a script gives egui's monospace family (#1179).
pub const MONOSPACE: &str = "monospace";
/// The name a script gives egui's proportional family, the default.
pub const PROPORTIONAL: &str = "proportional";

/// The family a script's `font` names, or `None` for a name nothing has: a
/// named family `is_named` knows (a project file, an alias, an extension's),
/// then [`MONOSPACE`] and [`PROPORTIONAL`] for egui's own two. Drawing
/// (`resolve_font_id`) and measuring (`text_measurer`) both ask it, so the
/// two agree on every name.
pub fn family_of(name: &str, is_named: impl Fn(&str) -> bool) -> Option<egui::FontFamily> {
    if is_named(name) {
        return Some(egui::FontFamily::Name(name.into()));
    }
    match name {
        MONOSPACE => Some(egui::FontFamily::Monospace),
        PROPORTIONAL => Some(egui::FontFamily::Proportional),
        _ => None,
    }
}

/// Resolve a script-facing family name to an egui `FontId`, falling back to
/// the default proportional font (and warning once per unknown name).
pub fn resolve_font_id(
    size: f32,
    font: Option<&str>,
    known: &HashSet<String>,
    warned: &mut HashSet<String>,
) -> egui::FontId {
    let Some(name) = font else {
        return egui::FontId::proportional(size);
    };
    match family_of(name, |n| known.contains(n)) {
        Some(family) => egui::FontId::new(size, family),
        None => {
            if warned.insert(name.to_string()) {
                let mut names: Vec<&str> = known.iter().map(|s| s.as_str()).collect();
                names.sort_unstable();
                names.extend([MONOSPACE, PROPORTIONAL]);
                tracing::warn!(
                    "font `{name}` not found under fonts/ (known: {}); using default",
                    names.join(", ")
                );
            }
            egui::FontId::proportional(size)
        }
    }
}

/// Scripts' `measure_text`, and the UI documents' layout (#731), laid out
/// with the font stack `ctx` draws the HUD with. Until `ready` says egui's
/// fonts are built (its first pass), and for a family the definitions
/// don't have, it estimates or falls back instead of panicking; `None` is a
/// context whose fonts are known to be built (a headless HUD's).
pub fn text_measurer(
    ctx: egui::Context,
    ready: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> flint_script::context::TextMeasurer {
    Arc::new(
        move |text: &str, size: f32, font: Option<&str>, spacing: f32| {
            if ready
                .as_ref()
                .is_some_and(|r| !r.load(std::sync::atomic::Ordering::Relaxed))
            {
                let chars = text.chars().count() as f32;
                return (chars * size * 0.6 + spacing * (chars - 1.0).max(0.0), size);
            }
            ctx.fonts_mut(|f| {
                let families = &f.definitions().families;
                let family = font
                    .and_then(|n| {
                        family_of(n, |n| {
                            families.contains_key(&egui::FontFamily::Name(n.into()))
                        })
                    })
                    .unwrap_or(egui::FontFamily::Proportional);
                let job = text_layout_job(
                    text,
                    egui::FontId::new(size, family),
                    egui::Color32::WHITE,
                    spacing,
                );
                let galley = f.layout_job(job);
                (galley.size().x, galley.size().y)
            })
        },
    )
}

/// The one layout-job builder shared by the HUD renderer and `measure_text`:
/// single line, no wrapping, optional extra letter spacing.
pub fn text_layout_job(
    text: &str,
    font_id: egui::FontId,
    color: egui::Color32,
    letter_spacing: f32,
) -> egui::text::LayoutJob {
    let format = egui::TextFormat {
        font_id,
        color,
        extra_letter_spacing: letter_spacing,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), format);
    job.wrap = egui::text::TextWrapping::no_max_width();
    job.break_on_newline = false;
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A face egui reads (its own first font) with `tag` after its last
    /// table, so a test can tell which file was read: skrifa reads only what
    /// the table directory points at.
    fn face(tag: &str) -> Vec<u8> {
        let defaults = egui::FontDefinitions::default();
        let first = defaults.font_data.values().next().expect("egui's fonts");
        let mut bytes = first.font.to_vec();
        bytes.extend_from_slice(tag.as_bytes());
        bytes
    }

    /// The check refuses no real face: every font egui ships passes it, and
    /// an empty file, text and a face cut after its first 64 or 4096 bytes
    /// each fail with the reader's reason.
    #[test]
    fn egui_s_own_faces_pass_and_broken_ones_say_why() {
        for (name, data) in egui::FontDefinitions::default().font_data {
            assert!(face_error(&data.font).is_none(), "{name}");
        }
        for len in [0, 64, 4096] {
            let mut cut = face("");
            cut.truncate(len);
            let why = face_error(&cut).expect("a face cut short");
            assert!(!why.to_string().is_empty(), "{len}");
        }
        assert!(face_error(b"not a font").is_some());
    }

    /// #1262: a face egui can't parse (an empty file, a truncated one, text
    /// with a font's name) is left out with a warning, where egui panicked
    /// on it when the fonts were installed. Its family is unknown, so its
    /// text falls back to egui's default face; a farther face of its stem is
    /// read in its place; an alias's path to one is dropped too; and the
    /// definitions install and lay text out.
    #[test]
    fn a_face_egui_cannot_parse_is_skipped_and_the_rest_install() {
        use flint_core::vfs::MemorySource;
        let mut files = MemorySource::new();
        files.insert("game/flint.toml", "");
        files.insert("game/fonts/Body.ttf", face("body"));
        files.insert("game/fonts/Mono.ttf", "");
        files.insert("game/fonts/Display.otf", face("project display"));
        let mut truncated = face("cut");
        truncated.truncate(64);
        files.insert("game/levels/fonts/Display.ttf", truncated);
        files.insert("game/fonts/ofl/Broken.ttf", "not a font");
        files.insert(
            "game/fonts/fonts.toml",
            "[[font]]\nname = \"terminal\"\nfile = \"ofl/Broken.ttf\"\n\n[egui]\nmonospace = \"Mono\"\n",
        );
        files.insert("game/levels/a.scene.toml", "");
        vfs::scoped(Arc::new(files), || {
            let fonts = ProjectFonts::for_scene("game/levels/a.scene.toml");
            assert_eq!(
                fonts.faces.len(),
                4,
                "the empty and truncated files are found"
            );
            let (data, _, _, linked) = fonts.read();
            let keys: Vec<&str> = data.keys().map(|k| k.as_str()).collect();
            assert_eq!(
                keys,
                ["Body", "Display"],
                "Mono and the alias's path left out"
            );
            assert_eq!(
                data["Display"],
                face("project display"),
                "the farther Display"
            );
            assert!(linked.is_empty());

            let defs = font_definitions(&fonts);
            let families = script_families(&defs);
            assert!(families.contains("Body") && families.contains("Display"));
            assert!(!families.contains("Mono") && !families.contains("terminal"));
            assert_eq!(
                defs.families[&egui::FontFamily::Monospace],
                egui::FontDefinitions::default().families[&egui::FontFamily::Monospace],
                "[egui] monospace names a face that isn't there: egui's stays"
            );

            // What panicked: egui building the fonts at the first frame.
            let ctx = egui::Context::default();
            ctx.set_fonts(defs);
            let _ = flint_debug_ui::run_headless_frame(&ctx, egui::RawInput::default(), |_| {});
            let mut warned = HashSet::new();
            let id = resolve_font_id(20.0, Some("Mono"), &families, &mut warned);
            assert_eq!(
                id.family,
                egui::FontFamily::Proportional,
                "egui's default face"
            );
            let width = ctx.fonts_mut(|f| {
                f.layout_job(text_layout_job("SCORE 42", id, egui::Color32::WHITE, 0.0))
                    .size()
                    .x
            });
            assert!(width > 0.0);
        });
    }

    /// ADR 0114: a scene two folders down sees the project's fonts and its
    /// manifest; a `fonts/` nearer the scene adds to them, and hides only
    /// the files it has a namesake for, where the one-folder scan read the
    /// first `fonts/` and nothing else.
    #[test]
    fn a_nested_scene_sees_the_projects_fonts_and_the_nearest_of_a_name() {
        use flint_core::vfs::MemorySource;
        let mut files = MemorySource::new();
        files.insert("game/flint.toml", "");
        files.insert("game/fonts/Body.ttf", face("project body"));
        files.insert("game/fonts/Display.otf", face("project display"));
        files.insert("game/fonts/SHOUT.TTF", face("capitals"));
        files.insert(
            "game/fonts/fonts.toml",
            "[[font]]\nname = \"title\"\nfile = \"Display.otf\"\n",
        );
        files.insert("game/levels/act1/fonts/Display.otf", face("act display"));
        files.insert("game/levels/act1/cave/a.scene.toml", "");
        files.insert("game/levels/b.scene.toml", "");
        vfs::scoped(Arc::new(files), || {
            let deep = ProjectFonts::for_scene("game/levels/act1/cave/a.scene.toml");
            let paths: Vec<String> = deep
                .faces
                .iter()
                .map(|f| flint_core::project::shown(&f.path))
                .collect();
            assert_eq!(
                paths,
                [
                    "game/levels/act1/fonts/Display.otf",
                    "game/fonts/Body.ttf",
                    "game/fonts/SHOUT.TTF",
                ]
            );
            assert_eq!(
                deep.manifest.as_deref(),
                Some(Path::new("game/fonts/fonts.toml"))
            );
            let (data, aliases, _, _) = deep.read();
            assert_eq!(data["Display"], face("act display"));
            assert_eq!(data["Body"], face("project body"));
            assert_eq!(data["SHOUT"], face("capitals"));
            let stems: Vec<String> = data.keys().cloned().collect();
            let families = resolve_families(&stems, &aliases);
            assert_eq!(families["title"], "Display", "the alias names the file");

            // From levels/ the project's own Display
            let shallow = ProjectFonts::for_scene("game/levels/b.scene.toml");
            assert_eq!(shallow.read().0["Display"], face("project display"));
            assert_ne!(deep, shallow, "another set: the player reads it again");
        });
    }

    #[test]
    fn manifest_parses_aliases_and_skips_incomplete_entries() {
        let text = r#"
[[font]]
name = "display"
file = "BarlowCondensed-BlackItalic.ttf"

[[font]]
name = "body"
file = "Barlow-Medium.ttf"

[[font]]
name = "missing-file"

[[font]]
file = "NoName.ttf"
"#;
        let aliases = parse_manifest(text);
        assert_eq!(
            aliases,
            vec![
                FontAlias {
                    name: "display".into(),
                    file: "BarlowCondensed-BlackItalic.ttf".into()
                },
                FontAlias {
                    name: "body".into(),
                    file: "Barlow-Medium.ttf".into()
                },
            ]
        );
    }

    #[test]
    fn manifest_keys_it_ignores_are_named() {
        // #282: each of these used to change nothing, without a word
        let text = r#"
proportional = "body"

[[font]]
name = "body"
fiel = "Barlow-Medium.ttf"

[egui]
monospce = "code"
"#;
        let issues: Vec<String> = manifest_issues(text)
            .iter()
            .map(|i| i.to_string())
            .collect();
        assert_eq!(issues.len(), 3, "{issues:#?}");
        let all = issues.join("\n");
        assert!(all.contains("`proportional` in the top level") && all.contains("[egui]"));
        assert!(
            all.contains("`fiel` in [[font]] #1") && all.contains("`file`?"),
            "{all}"
        );
        assert!(
            all.contains("`monospce` in [egui]") && all.contains("`monospace`?"),
            "{all}"
        );

        // One `[font]` where the format wants an array of them
        let single = manifest_issues("[font]\nname = \"a\"\nfile = \"a.ttf\"\n");
        assert_eq!(single.len(), 1);
        assert!(
            single[0].to_string().contains("`[[font]]`"),
            "{}",
            single[0]
        );

        let clean = "[[font]]\nname = \"a\"\nfile = \"a.ttf\"\n[egui]\nproportional = \"a\"\n";
        assert!(manifest_issues(clean).is_empty());
    }

    #[test]
    fn manifest_garbage_is_empty() {
        assert!(parse_manifest("this is = not [[ toml").is_empty());
        assert!(parse_manifest("").is_empty());
    }

    #[test]
    fn aliases_resolve_to_stems_and_bad_ones_drop() {
        let stems = vec![
            "Barlow-Medium".to_string(),
            "BarlowCondensed-BlackItalic".to_string(),
        ];
        let aliases = vec![
            FontAlias {
                name: "display".into(),
                file: "BarlowCondensed-BlackItalic.ttf".into(),
            },
            FontAlias {
                name: "ghost".into(),
                file: "NotThere.otf".into(),
            },
            // may not shadow a real file
            FontAlias {
                name: "Barlow-Medium".into(),
                file: "BarlowCondensed-BlackItalic.ttf".into(),
            },
        ];
        let fams = resolve_families(&stems, &aliases);
        assert_eq!(fams.len(), 3);
        assert_eq!(fams["Barlow-Medium"], "Barlow-Medium");
        assert_eq!(
            fams["BarlowCondensed-BlackItalic"],
            "BarlowCondensed-BlackItalic"
        );
        assert_eq!(fams["display"], "BarlowCondensed-BlackItalic");
        assert!(!fams.contains_key("ghost"));
    }

    #[test]
    fn egui_defaults_parse() {
        let text = r#"
[[font]]
name = "body"
file = "Body.ttf"

[egui]
proportional = "body"
monospace = 3
"#;
        assert_eq!(
            parse_egui_defaults(text),
            EguiDefaults {
                proportional: Some("body".into()),
                monospace: None,
            }
        );
        assert_eq!(parse_egui_defaults(""), EguiDefaults::default());
        assert_eq!(parse_egui_defaults("not [[ toml"), EguiDefaults::default());
    }

    /// The named face leads egui's family and egui's fonts stay behind it;
    /// named families fall back to egui's proportional fonts; an unknown
    /// default leaves egui's family alone.
    #[test]
    fn definitions_put_the_named_defaults_first() {
        let egui_defaults = egui::FontDefinitions::default();
        let proportional = &egui_defaults.families[&egui::FontFamily::Proportional];
        let monospace = &egui_defaults.families[&egui::FontFamily::Monospace];

        let data: BTreeMap<String, Vec<u8>> = [
            ("Body".to_string(), vec![0u8]),
            ("Title".to_string(), vec![1u8]),
        ]
        .into();
        let stems: Vec<String> = data.keys().cloned().collect();
        let families = resolve_families(
            &stems,
            &[FontAlias {
                name: "body".into(),
                file: "Body.ttf".into(),
            }],
        );
        let defs = build_definitions(
            data,
            &families,
            &EguiDefaults {
                proportional: Some("body".into()),
                monospace: Some("ghost".into()),
            },
        );

        let p = &defs.families[&egui::FontFamily::Proportional];
        assert_eq!(p[0], "Body");
        assert_eq!(&p[1..], &proportional[..]);
        assert_eq!(&defs.families[&egui::FontFamily::Monospace], monospace);
        let title = &defs.families[&egui::FontFamily::Name("Title".into())];
        assert_eq!(title[0], "Title");
        assert_eq!(&title[1..], &proportional[..]);
        assert!(defs.font_data.contains_key("Body"));

        let mut names: Vec<String> = script_families(&defs).into_iter().collect();
        names.sort();
        assert_eq!(names, ["Body", "Title", "body"]);
    }

    #[test]
    fn unknown_font_falls_back_and_warns_once() {
        let known: HashSet<String> = ["display".to_string()].into_iter().collect();
        let mut warned = HashSet::new();
        let id = resolve_font_id(20.0, Some("display"), &known, &mut warned);
        assert_eq!(id.family, egui::FontFamily::Name("display".into()));
        let id = resolve_font_id(20.0, Some("nope"), &known, &mut warned);
        assert_eq!(id.family, egui::FontFamily::Proportional);
        resolve_font_id(20.0, Some("nope"), &known, &mut warned);
        assert_eq!(warned.len(), 1);
        let id = resolve_font_id(12.0, None, &known, &mut warned);
        assert_eq!(id, egui::FontId::proportional(12.0));
    }

    /// #1179: `"monospace"` and `"proportional"` are egui's two families, a
    /// project family of the name comes first, and neither is warned about.
    #[test]
    fn egui_families_have_names_scripts_can_give() {
        let mut warned = HashSet::new();
        let none = HashSet::new();
        let id = resolve_font_id(18.0, Some("monospace"), &none, &mut warned);
        assert_eq!(id, egui::FontId::monospace(18.0));
        let id = resolve_font_id(18.0, Some("proportional"), &none, &mut warned);
        assert_eq!(id, egui::FontId::proportional(18.0));
        assert!(warned.is_empty(), "{warned:?}");

        let own: HashSet<String> = ["monospace".to_string()].into_iter().collect();
        let id = resolve_font_id(18.0, Some("monospace"), &own, &mut warned);
        assert_eq!(id.family, egui::FontFamily::Name("monospace".into()));
        assert_eq!(family_of("Monospace", |_| false), None, "names are exact");
    }

    /// #1179's done-when: one `measure_text` of fixed-pitch text in egui's
    /// monospace face with no font file of the game's: every glyph the same
    /// width, and the width the HUD's own layout of it draws.
    #[test]
    fn monospace_text_measures_as_it_draws_one_cell_a_glyph() {
        let ctx = egui::Context::default();
        let _ = flint_debug_ui::run_headless_frame(&ctx, egui::RawInput::default(), |_| {});
        let measure = text_measurer(ctx.clone(), None);
        let narrow = measure("iiiiiiii", 20.0, Some("monospace"), 0.0);
        let wide = measure("MMMMMMMM", 20.0, Some("monospace"), 0.0);
        let toml = measure("seed = 42", 20.0, Some("monospace"), 0.0);
        assert!(narrow.0 > 0.0);
        assert_eq!(narrow.0, wide.0, "a fixed pitch");
        let cell = narrow.0 / 8.0;
        // egui rounds a galley's width to its pixel grid: a hundredth of a point
        assert!((toml.0 - cell * 9.0).abs() < 0.05, "{toml:?} vs {cell}");

        // The proportional face is not fixed: `i` is narrower than `M`.
        let p_narrow = measure("iiiiiiii", 20.0, Some("proportional"), 0.0);
        let p_wide = measure("MMMMMMMM", 20.0, Some("proportional"), 0.0);
        assert!(p_narrow.0 < p_wide.0 * 0.6, "{p_narrow:?} {p_wide:?}");
        assert_eq!(p_narrow, measure("iiiiiiii", 20.0, None, 0.0));

        // What the HUD draws: the same job through `resolve_font_id`.
        let id = resolve_font_id(
            20.0,
            Some("monospace"),
            &HashSet::new(),
            &mut HashSet::new(),
        );
        let drawn = ctx.fonts_mut(|f| {
            f.layout_job(text_layout_job("seed = 42", id, egui::Color32::WHITE, 0.0))
                .size()
        });
        assert_eq!((drawn.x, drawn.y), toml);
    }

    /// #1179: an alias's `file` may be a path from the manifest's folder,
    /// below it or beside it, so a face a glyph set keeps in `fonts/ofl/`
    /// is a HUD font without a second copy. It is registered under the
    /// alias alone; a path that can't be read, or an absolute one, drops.
    #[test]
    fn an_alias_names_a_face_below_or_beside_fonts_by_its_path() {
        use flint_core::vfs::MemorySource;
        let mut files = MemorySource::new();
        files.insert("game/flint.toml", "");
        files.insert("game/fonts/Body.ttf", face("body"));
        files.insert("game/fonts/ofl/VT323/VT323-Regular.ttf", face("vt323"));
        files.insert("game/vendor/Cutive.ttf", face("cutive"));
        files.insert(
            "game/fonts/fonts.toml",
            r#"
[[font]]
name = "terminal"
file = "ofl/VT323/VT323-Regular.ttf"

[[font]]
name = "typewriter"
file = "../vendor/Cutive.ttf"

[[font]]
name = "ghost"
file = "ofl/Nope/Nope.ttf"

[[font]]
name = "rooted"
file = "/etc/fonts/x.ttf"

[egui]
monospace = "terminal"
"#,
        );
        files.insert("game/levels/a.scene.toml", "");
        vfs::scoped(Arc::new(files), || {
            let fonts = ProjectFonts::for_scene("game/levels/a.scene.toml");
            let (data, aliases, defaults, linked) = fonts.read();
            assert_eq!(data["ofl/VT323/VT323-Regular.ttf"], face("vt323"));
            assert_eq!(data["../vendor/Cutive.ttf"], face("cutive"));
            assert_eq!(data.len(), 3, "{:?}", data.keys().collect::<Vec<_>>());
            let shown: Vec<String> = linked
                .iter()
                .map(|p| flint_core::project::shown(p))
                .collect();
            assert_eq!(
                shown,
                [
                    "game/fonts/ofl/VT323/VT323-Regular.ttf",
                    "game/vendor/Cutive.ttf"
                ]
            );

            let keys: Vec<String> = data.keys().cloned().collect();
            let families = resolve_families(&keys, &aliases);
            let names: Vec<&str> = families.keys().map(|s| s.as_str()).collect();
            assert_eq!(
                names,
                ["Body", "terminal", "typewriter"],
                "no path is a family"
            );
            assert_eq!(families["terminal"], "ofl/VT323/VT323-Regular.ttf");

            let defs = build_definitions(data, &families, &defaults);
            let mono = &defs.families[&egui::FontFamily::Monospace];
            assert_eq!(mono[0], "ofl/VT323/VT323-Regular.ttf", "[egui] reaches it");
            assert_eq!(font_definitions(&fonts).families.len(), defs.families.len());
        });
    }

    /// A manifest whose only faces are paths still loads: no font file in
    /// `fonts/` itself is no reason to skip it.
    #[test]
    fn a_manifest_of_paths_alone_loads() {
        use flint_core::vfs::MemorySource;
        let mut files = MemorySource::new();
        files.insert("game/flint.toml", "");
        files.insert("game/fonts/ofl/VT323/VT323-Regular.ttf", face("vt323"));
        files.insert(
            "game/fonts/fonts.toml",
            "[[font]]\nname = \"terminal\"\nfile = \"ofl/VT323/VT323-Regular.ttf\"\n",
        );
        files.insert("game/a.scene.toml", "");
        vfs::scoped(Arc::new(files), || {
            let fonts = ProjectFonts::for_scene("game/a.scene.toml");
            assert!(fonts.faces.is_empty());
            let defs = font_definitions(&fonts);
            assert!(script_families(&defs).contains("terminal"));
        });
    }

    #[test]
    fn alias_keys_are_stems_or_paths() {
        assert_eq!(alias_key("Body.ttf").as_deref(), Some("Body"));
        assert_eq!(alias_key("./Body.ttf").as_deref(), Some("Body"));
        assert_eq!(
            alias_key("ofl\\VT323\\VT323-Regular.ttf").as_deref(),
            Some("ofl/VT323/VT323-Regular.ttf")
        );
        assert_eq!(alias_key("a/../B.ttf").as_deref(), Some("B"));
        assert_eq!(alias_key("/abs/x.ttf"), None);
        assert_eq!(alias_key("C:/fonts/x.ttf"), None);
    }

    #[test]
    fn layout_job_carries_spacing_and_no_wrap() {
        let job = text_layout_job(
            "0000",
            egui::FontId::proportional(10.0),
            egui::Color32::WHITE,
            1.5,
        );
        assert_eq!(job.sections.len(), 1);
        assert_eq!(job.sections[0].format.extra_letter_spacing, 1.5);
        assert!(!job.break_on_newline);
        assert_eq!(job.wrap.max_width, f32::INFINITY);
    }
}
