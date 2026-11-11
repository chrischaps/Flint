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
//! [egui]                               # optional (ADR 0081)
//! proportional = "body"                # egui's default text face
//! monospace = "code"                   # egui's monospace face
//! ```
//!
//! The `[egui]` names are families (a file stem or an alias). The named face
//! goes first in egui's `Proportional` or `Monospace` family, ahead of egui's
//! own fonts, so a game's egui UI draws in it and keeps egui's fonts for the
//! glyphs it lacks. Every named family keeps egui's proportional fonts as
//! fallbacks the same way.
//!
//! An extension can add fonts of its own with `PlayerExtension::fonts`,
//! which edits the definitions before Flint installs them, so they survive
//! every reload.
//!
//! Both the renderer (`hud_render`) and the script `measure_text` closure
//! build their layout jobs through [`text_layout_job`] so measurement and
//! drawing can never disagree.

use flint_core::project::{AssetKind, Listed, Project};
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

/// Parse a `fonts.toml` manifest. Entries missing `name` or `file` are
/// dropped with a warning; a malformed document yields an empty list.
pub fn parse_manifest(text: &str) -> Vec<FontAlias> {
    let doc: toml::Value = match text.parse() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("fonts.toml: parse error: {e}");
            return Vec::new();
        }
    };
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

/// Resolve the family table for a set of font files plus manifest aliases.
/// Keys are family names (file stems first, then aliases); values are the
/// `font_data` key each family points at (always a file stem). Aliases whose
/// file is not among `stems` are dropped with a warning; an alias may not
/// shadow a real stem.
pub fn resolve_families(stems: &[String], aliases: &[FontAlias]) -> BTreeMap<String, String> {
    let mut families: BTreeMap<String, String> =
        stems.iter().map(|s| (s.clone(), s.clone())).collect();
    for alias in aliases {
        let target = Path::new(&alias.file)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !stems.contains(&target) {
            tracing::warn!(
                "fonts.toml: alias `{}` points at `{}`, which is not in fonts/",
                alias.name,
                alias.file
            );
            continue;
        }
        if stems.contains(&alias.name) {
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

    /// Read the files: `(stem -> bytes, aliases, egui defaults)`. Of two
    /// files of one stem (`Body.ttf` near, `Body.otf` far) the nearer is
    /// kept.
    fn read(&self) -> (BTreeMap<String, Vec<u8>>, Vec<FontAlias>, EguiDefaults) {
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
                Ok(bytes) => {
                    data.insert(stem, bytes);
                }
                Err(e) => tracing::warn!("fonts: failed to read {}: {e}", face.path.display()),
            }
        }
        let manifest = self.manifest.as_ref().map(vfs::read_to_string);
        match manifest {
            Some(Ok(text)) => (data, parse_manifest(&text), parse_egui_defaults(&text)),
            _ => (data, Vec::new(), EguiDefaults::default()),
        }
    }
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
    if fonts.faces.is_empty() {
        return egui::FontDefinitions::default();
    }
    let (data, aliases, defaults) = fonts.read();
    let stems: Vec<String> = data.keys().cloned().collect();
    let families = resolve_families(&stems, &aliases);
    if !families.is_empty() {
        let mut folders: Vec<String> = Vec::new();
        for face in &fonts.faces {
            let folder = face.path.parent().unwrap_or(Path::new(""));
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

/// Resolve a script-facing family name to an egui `FontId`, falling back to
/// the default proportional font (and warning once per unknown name).
pub fn resolve_font_id(
    size: f32,
    font: Option<&str>,
    known: &HashSet<String>,
    warned: &mut HashSet<String>,
) -> egui::FontId {
    match font {
        Some(name) if known.contains(name) => {
            egui::FontId::new(size, egui::FontFamily::Name(name.into()))
        }
        Some(name) => {
            if warned.insert(name.to_string()) {
                tracing::warn!(
                    "font `{name}` not found under fonts/ (known: {:?}); using default",
                    known
                );
            }
            egui::FontId::proportional(size)
        }
        None => egui::FontId::proportional(size),
    }
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

    /// ADR 0114: a scene two folders down sees the project's fonts and its
    /// manifest; a `fonts/` nearer the scene adds to them, and hides only
    /// the files it has a namesake for, where the one-folder scan read the
    /// first `fonts/` and nothing else.
    #[test]
    fn a_nested_scene_sees_the_projects_fonts_and_the_nearest_of_a_name() {
        use flint_core::vfs::MemorySource;
        let mut files = MemorySource::new();
        files.insert("game/flint.toml", "");
        files.insert("game/fonts/Body.ttf", "project body");
        files.insert("game/fonts/Display.otf", "project display");
        files.insert("game/fonts/SHOUT.TTF", "capitals");
        files.insert(
            "game/fonts/fonts.toml",
            "[[font]]\nname = \"title\"\nfile = \"Display.otf\"\n",
        );
        files.insert("game/levels/act1/fonts/Display.otf", "act display");
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
            let (data, aliases, _) = deep.read();
            assert_eq!(data["Display"], b"act display");
            assert_eq!(data["Body"], b"project body");
            assert_eq!(data["SHOUT"], b"capitals");
            let stems: Vec<String> = data.keys().cloned().collect();
            let families = resolve_families(&stems, &aliases);
            assert_eq!(families["title"], "Display", "the alias names the file");

            // From levels/ the project's own Display
            let shallow = ProjectFonts::for_scene("game/levels/b.scene.toml");
            assert_eq!(shallow.read().0["Display"], b"project display");
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
