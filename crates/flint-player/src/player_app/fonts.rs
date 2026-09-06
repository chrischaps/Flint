//! Project fonts for the script HUD.
//!
//! Scans `<project>/fonts/*.ttf|*.otf` (non-recursive; the project root is
//! the scene file's parent directory, or its parent — the same rule
//! `load_ui_texture` uses for `sprites/`) and registers every file with egui
//! under its file stem as a named `FontFamily`. An optional
//! `<project>/fonts/fonts.toml` manifest adds aliases:
//!
//! ```toml
//! [[font]]
//! name = "display"                     # alias → FontFamily::Name("display")
//! file = "BarlowCondensed-BlackItalic.ttf"
//! ```
//!
//! Both the renderer (`hud_render`) and the script `measure_text` closure
//! build their layout jobs through [`text_layout_job`] so measurement and
//! drawing can never disagree.

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

/// The project root for a scene path: `scene_dir/fonts` if it exists,
/// else `scene_dir/../fonts`. Returns the `fonts/` directory candidate that
/// exists, or `None`.
pub fn find_fonts_dir(scene_path: &str) -> Option<PathBuf> {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let mut candidates = vec![scene_dir.join("fonts")];
    if let Some(parent) = scene_dir.parent() {
        candidates.push(parent.join("fonts"));
    }
    candidates.into_iter().find(|p| p.is_dir())
}

/// Scan a `fonts/` directory: read every `.ttf`/`.otf` (non-recursive) and
/// the optional manifest. Returns `(stem → bytes, aliases)`.
fn scan_fonts_dir(dir: &Path) -> (BTreeMap<String, Vec<u8>>, Vec<FontAlias>) {
    let mut data = BTreeMap::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("fonts: cannot read {}: {e}", dir.display());
            return (data, Vec::new());
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if ext != "ttf" && ext != "otf" {
            continue;
        }
        let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        match std::fs::read(&path) {
            Ok(bytes) => {
                data.insert(stem, bytes);
            }
            Err(e) => tracing::warn!("fonts: failed to read {}: {e}", path.display()),
        }
    }
    let manifest = dir.join("fonts.toml");
    let aliases = match std::fs::read_to_string(&manifest) {
        Ok(text) => parse_manifest(&text),
        Err(_) => Vec::new(),
    };
    (data, aliases)
}

/// Load the project's fonts into `ctx` and return the set of family names
/// scripts may refer to. With no `fonts/` directory the egui defaults are
/// restored (so a scene change to a font-less project drops stale families).
pub fn install_project_fonts(ctx: &egui::Context, scene_path: &str) -> HashSet<String> {
    let mut defs = egui::FontDefinitions::default();
    let mut known = HashSet::new();

    if let Some(dir) = find_fonts_dir(scene_path) {
        let (data, aliases) = scan_fonts_dir(&dir);
        let stems: Vec<String> = data.keys().cloned().collect();
        let families = resolve_families(&stems, &aliases);
        for (stem, bytes) in data {
            defs.font_data
                .insert(stem, Arc::new(egui::FontData::from_owned(bytes)));
        }
        for (family, target) in &families {
            defs.families.insert(
                egui::FontFamily::Name(family.as_str().into()),
                vec![target.clone()],
            );
            known.insert(family.clone());
        }
        if known.is_empty() {
            tracing::info!("fonts: {} contains no .ttf/.otf files", dir.display());
        } else {
            let mut names: Vec<&String> = known.iter().collect();
            names.sort();
            tracing::info!(
                "fonts: loaded {} famil{} from {}: {}",
                names.len(),
                if names.len() == 1 { "y" } else { "ies" },
                dir.display(),
                names
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    ctx.set_fonts(defs);
    known
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
