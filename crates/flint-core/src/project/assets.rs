//! The one asset search (ADR 0114, decision 4; #706).
//!
//! Every kind of asset is looked for in the same places, in the same order:
//!
//! 1. the scene's own directory;
//! 2. each directory above it, up to and including the project root;
//! 3. the mounted engine directory, `<root>/engine/`, when the project has
//!    one (a game keeps the engine as a subtree there). In an engine checkout
//!    the root already is the engine, so there is no third place.
//!
//! At each directory `D` of that walk the candidates are
//! `D/<folder>/<name><extension>`, for each of the kind's folders and then
//! each of its extensions. A folder of `""` is `D` itself. A name that
//! already ends in one of the kind's extensions is taken as written, and a
//! name may carry subfolders (`fx/fire` is
//! `D/particles/fx/fire.particles.toml`). The first candidate that is a file
//! wins.
//!
//! Before the walk comes one step, the same for every kind: **as written**.
//! A name that is a path of its own, absolute or starting with `./` or
//! `../`, is tried first from the scene's directory (an absolute one as it
//! stands), with the kind's extensions. A relative one then goes through the
//! walk like any name; an absolute one is tried nowhere else. No path is
//! tried twice.
//!
//! A kind is data: a label, its folders, its extensions and whether its
//! names are file names ([`KindInfo`]). Nothing else may differ from one
//! kind to the next, and the walk never looks at which kind it is serving.
//! Extensions are matched as spelled, capitals and all, for every kind.
//!
//! Two edges of the walk, the same for every kind:
//!
//! - A scene outside the project's root (`--project` naming another folder)
//!   is searched beside itself, then at the root, then in the engine.
//! - A project found by no marker at all ([`Found::Fallback`]) has the
//!   scene's directory as its root. Every search the engine had before there
//!   were projects also looked one level above the scene, and with no
//!   manifest the engine behaves as it did, so that level is searched too.
//!
//! ```
//! use flint_core::project::{AssetKind, Project};
//! use flint_core::vfs::{self, MemorySource};
//! use std::path::Path;
//! use std::sync::Arc;
//!
//! let mut files = MemorySource::new();
//! files.insert("flint.toml", "");
//! files.insert("particles/fire.particles.toml", "");
//! files.insert("scenes/act1/cave/a.scene.toml", "");
//! vfs::scoped(Arc::new(files), || {
//!     let scene_dir = Path::new("scenes/act1/cave");
//!     let project = Project::discover(scene_dir).unwrap();
//!     let found = project.resolve(AssetKind::Particles, "fire", scene_dir).unwrap();
//!     assert_eq!(found, Path::new("particles/fire.particles.toml"));
//!     let missing = project.resolve(AssetKind::Particles, "ice", scene_dir).unwrap_err();
//!     assert_eq!(missing.tried.len(), 4); // cave, act1, scenes, the root
//! });
//! ```

use super::{anchor, parent_dir, same_dir_key, Found, Project};
use crate::vfs;
use std::collections::HashSet;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// A kind of asset the resolver can find. What a kind *is* lives in its
/// [`KindInfo`]; this is only its name in code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AssetKind {
    /// Particle effects: `particles/<name>.particles.toml`.
    Particles,
    /// Models: `models/<name>.glb`, then `.gltf`.
    Models,
    /// Textures (a material's or a `sprite` component's `texture`): beside
    /// the scene or in `textures/`, by file name, or a bare name that also
    /// tries `.png` and `_color.png`.
    Textures,
    /// UI sprites (`draw_sprite`): `sprites/<file>`, then beside the scene.
    Sprites,
    /// Property clips: `animations/<name>.anim.toml`.
    Animations,
    /// Sprite-sheet clips: `animations/<name>.sprite.toml`.
    SpriteAnimations,
    /// Animator sequences: `animations/<name>.sequence.toml`.
    Sequences,
    /// Sounds: a file named from the scene (`audio/hit.ogg`), or in `audio/`.
    Audio,
    /// Rhai scripts: `scripts/<file>.rhai`; an `import "m"` is
    /// `scripts/lib/m.rhai`.
    Scripts,
    /// Procgen specs: any `*.procgen.toml` under `specs/` or `models/`,
    /// found by its `[meta] name`.
    Specs,
    /// Prefabs: `prefabs/<name>.prefab.toml`.
    Prefabs,
    /// Wooze presets (a scene's `[wooze] preset`, the Living Props panel):
    /// `wooze/<name>.wooze.toml`.
    WoozePresets,
    /// A file a scene names by a path of its own (`config/input.toml`):
    /// that path from the scene's directory, then from each directory above.
    Files,
    /// A particle effect's or emitter's `texture`: a file name in
    /// `particles/`, then beside the scene.
    ParticleTextures,
    /// The scene a transition names (`load_scene("level2.scene.toml")`): a
    /// path from the current scene's directory, then from each above.
    Scenes,
    /// Colour palettes a style names: `palettes/<name>.palette.toml`, or a
    /// path from the scene. The built-in palettes come after the search.
    Palettes,
    /// Glyph sets (the ASCII style's `font`, a `ui_text` font):
    /// `fonts/<name>.glyphs.toml`, or a path from the scene. `builtin` is
    /// never searched for.
    GlyphSets,
    /// Bitmap font descriptions a `ui_text.font` names by path
    /// (`fonts/ui.font.toml`): that path from the scene's directory and
    /// each above, then in `fonts/`.
    Fonts,
    /// The HUD's font files: every `.ttf` and `.otf` in `fonts/`, by file
    /// name, which is what a `fonts/fonts.toml` alias names.
    FontFaces,
    /// Looks (`[post_process] look`, `--look`): `styles/<name>.look.toml`,
    /// or a path from the scene. The built-in looks come after the search.
    Looks,
    /// Paper images (`paper_image`): `papers/<name>.png`, or a path from
    /// the scene.
    Papers,
    /// Stroke strips (`stroke_image`): `strokes/<name>.png`, or a path from
    /// the scene.
    Strokes,
    /// User-authored styles: every `styles/<name>.shader.toml`.
    Styles,
}

/// Everything that tells one kind from another: where its files sit under
/// each directory of the walk, and how their names end.
#[derive(Debug)]
pub struct KindInfo {
    /// What a person types: `flint asset where <label> <name>`.
    pub label: &'static str,
    /// The kind in a sentence: "particle effect".
    pub noun: &'static str,
    /// Folders under each directory of the walk, in the order tried. An
    /// empty string is the directory itself.
    pub folders: &'static [&'static str],
    /// File endings, in the order tried, each with its dot
    /// (`.particles.toml`). A name already ending in one is taken as it is.
    /// An empty string is the name with nothing added.
    pub extensions: &'static [&'static str],
    /// The kind's names are file names (`wall.png`, `audio/hit.ogg`): a name
    /// with a dot in its last part is taken as written whatever follows the
    /// dot, and a listed file's name is its whole file name. Otherwise a
    /// name is a stem and the kind's extensions finish it.
    pub named_by_file: bool,
    /// [`Project::list`] also lists the files in subfolders of the kind's
    /// folders, each named with its subfolder (`lib/util.rhai`), which is
    /// the name [`Project::resolve`] takes for it.
    pub deep: bool,
    /// The kind's files say their own name inside (a procgen spec's
    /// `[meta] name`): this reads it from a file's text. A file is then
    /// found by that name whatever it is called on disk, in the same places
    /// and the same order, and a file that states no name is not listed.
    pub named_inside: Option<InnerName>,
    /// TEMPORARY, for one release (ADR 0114 decision 3, #765): the search
    /// this kind had before the one rule, as the file it would have picked.
    /// `None` once the release has passed, or when the kind never differed.
    old_rule: Option<OldRule>,
    /// TEMPORARY, with `old_rule`: for a kind whose loader takes every file
    /// it can see, the files its old search loaded, in the order it loaded
    /// them. [`Project::list`] compares the two lists, which also catches a
    /// file the old search loaded and the one rule no longer sees. `None`
    /// for a kind that is only ever asked for by name.
    old_list: Option<OldList>,
}

/// A kind's search before the one rule: the file it would have picked for
/// `name` from a scene in `scene_dir`, if any.
type OldRule = fn(name: &str, scene_dir: &Path) -> Option<PathBuf>;

/// A kind's listing before the one rule: every file its loader took for a
/// scene in `scene_dir`, where two of one name the first is the one kept.
type OldList = fn(scene_dir: &Path) -> Vec<PathBuf>;

/// The name a file gives itself, read from its text ([`KindInfo::named_inside`]).
pub type InnerName = fn(text: &str) -> Option<String>;

static PARTICLES: KindInfo = KindInfo {
    label: "particles",
    noun: "particle effect",
    folders: &["particles"],
    extensions: &[".particles.toml"],
    named_by_file: false,
    deep: false,
    named_inside: None,
    old_rule: Some(old_particles),
    old_list: None,
};

static MODELS: KindInfo = KindInfo {
    label: "models",
    noun: "model",
    folders: &["models"],
    extensions: &[".glb", ".gltf"],
    named_by_file: false,
    deep: false,
    named_inside: None,
    old_rule: Some(old_models),
    old_list: None,
};

static TEXTURES: KindInfo = KindInfo {
    label: "textures",
    noun: "texture",
    folders: &["", "textures"],
    extensions: &["", ".png", "_color.png"],
    named_by_file: true,
    deep: false,
    named_inside: None,
    old_rule: Some(old_textures),
    old_list: None,
};

static SPRITES: KindInfo = KindInfo {
    label: "sprites",
    noun: "UI sprite",
    folders: &["sprites", ""],
    extensions: &[],
    named_by_file: true,
    deep: false,
    named_inside: None,
    old_rule: Some(old_sprites),
    old_list: None,
};

static ANIMATIONS: KindInfo = KindInfo {
    label: "animations",
    noun: "animation clip",
    folders: &["animations"],
    extensions: &[".anim.toml"],
    named_by_file: false,
    deep: false,
    named_inside: None,
    old_rule: Some(old_animations),
    old_list: Some(old_animation_list),
};

static SPRITE_ANIMATIONS: KindInfo = KindInfo {
    label: "sprite-animations",
    noun: "sprite animation file",
    folders: &["animations"],
    extensions: &[".sprite.toml"],
    named_by_file: false,
    deep: false,
    named_inside: None,
    old_rule: Some(old_sprite_animations),
    old_list: Some(old_sprite_animation_list),
};

static SEQUENCES: KindInfo = KindInfo {
    label: "sequences",
    noun: "animation sequence",
    folders: &["animations"],
    extensions: &[".sequence.toml"],
    named_by_file: false,
    deep: false,
    named_inside: None,
    old_rule: Some(old_sequences),
    old_list: Some(old_sequence_list),
};

static AUDIO: KindInfo = KindInfo {
    label: "audio",
    noun: "sound",
    folders: &["", "audio"],
    extensions: &[".ogg", ".wav", ".mp3", ".flac"],
    named_by_file: true,
    deep: false,
    named_inside: None,
    old_rule: Some(old_audio),
    old_list: Some(old_audio_list),
};

static SCRIPTS: KindInfo = KindInfo {
    label: "scripts",
    noun: "script",
    folders: &["scripts"],
    extensions: &[".rhai"],
    named_by_file: true,
    deep: true,
    named_inside: None,
    old_rule: Some(old_scripts),
    old_list: None,
};

static SPECS: KindInfo = KindInfo {
    label: "specs",
    noun: "procgen spec",
    folders: &["specs", "models"],
    extensions: &[".procgen.toml"],
    named_by_file: false,
    deep: true,
    named_inside: Some(spec_name),
    old_rule: Some(old_specs),
    old_list: Some(old_spec_list),
};

static PREFABS: KindInfo = KindInfo {
    label: "prefabs",
    noun: "prefab",
    folders: &["prefabs"],
    extensions: &[".prefab.toml"],
    named_by_file: false,
    deep: false,
    named_inside: None,
    old_rule: Some(old_prefabs),
    old_list: None,
};

static WOOZE_PRESETS: KindInfo = KindInfo {
    label: "wooze",
    noun: "wooze preset",
    folders: &["wooze"],
    extensions: &[".wooze.toml"],
    named_by_file: false,
    deep: false,
    named_inside: None,
    old_rule: Some(old_wooze_presets),
    old_list: None,
};

/// TEMPORARY. Wooze presets before ADR 0114: `wooze/<name>.wooze.toml`
/// beside the scene, then in `../wooze/` (`flint_scene::wooze::preset_dirs`
/// as it was, which spelled the level above as `<scene dir>/..`).
fn old_wooze_presets(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    // The same folder, spelled without the `..` where the path has a parent
    let above = match scene_dir.file_name() {
        Some(_) if scene_dir != Path::new(".") => scene_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default(),
        _ => scene_dir.join(".."),
    };
    [scene_dir.join("wooze"), above.join("wooze")]
        .into_iter()
        .map(|dir| dir.join(format!("{name}.wooze.toml")))
        .find(|p| vfs::is_file(p))
}

static FILES: KindInfo = KindInfo {
    label: "files",
    noun: "file",
    folders: &[""],
    extensions: &[],
    named_by_file: true,
    deep: false,
    named_inside: None,
    old_rule: Some(old_from_scene),
    old_list: None,
};

/// TEMPORARY. A file named by a path before ADR 0114: that path from the
/// scene's directory, then from the one above.
fn old_from_scene(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    old_two_levels(scene_dir)
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|p| vfs::exists(p))
}

static PARTICLE_TEXTURES: KindInfo = KindInfo {
    label: "particle-textures",
    noun: "particle texture",
    folders: &["particles", ""],
    extensions: &[],
    named_by_file: true,
    deep: false,
    named_inside: None,
    old_rule: Some(old_particle_textures),
    old_list: None,
};

static SCENES: KindInfo = KindInfo {
    label: "scenes",
    noun: "scene",
    folders: &[""],
    extensions: &[],
    named_by_file: true,
    deep: false,
    named_inside: None,
    old_rule: Some(old_scenes),
    old_list: None,
};

static PALETTES: KindInfo = KindInfo {
    label: "palettes",
    noun: "palette",
    folders: &["palettes", ""],
    extensions: &[".palette.toml"],
    named_by_file: false,
    deep: false,
    named_inside: None,
    old_rule: Some(old_palettes),
    old_list: None,
};

static GLYPH_SETS: KindInfo = KindInfo {
    label: "glyph-sets",
    noun: "glyph set",
    folders: &["fonts", ""],
    extensions: &[".glyphs.toml"],
    named_by_file: false,
    deep: false,
    named_inside: None,
    old_rule: Some(old_glyph_sets),
    old_list: None,
};

static FONTS: KindInfo = KindInfo {
    label: "fonts",
    noun: "bitmap font",
    folders: &["", "fonts"],
    extensions: &[".font.toml"],
    named_by_file: false,
    deep: false,
    named_inside: None,
    old_rule: Some(old_fonts),
    old_list: None,
};

static FONT_FACES: KindInfo = KindInfo {
    label: "font-faces",
    noun: "font file",
    folders: &["fonts"],
    // Both cases, as the one-folder scan took them (extensions match as
    // spelled, and a font copied from Windows is `ARIAL.TTF`)
    extensions: &[".ttf", ".otf", ".TTF", ".OTF"],
    named_by_file: true,
    deep: false,
    named_inside: None,
    old_rule: Some(old_font_faces),
    old_list: Some(old_font_face_list),
};

static LOOKS: KindInfo = KindInfo {
    label: "looks",
    noun: "look",
    folders: &["styles", ""],
    extensions: &[".look.toml"],
    named_by_file: false,
    deep: false,
    named_inside: None,
    old_rule: Some(old_looks),
    old_list: None,
};

static PAPERS: KindInfo = KindInfo {
    label: "papers",
    noun: "paper image",
    folders: &["papers", ""],
    extensions: &[".png", ".jpg", ".jpeg"],
    named_by_file: false,
    deep: false,
    named_inside: None,
    old_rule: Some(old_papers),
    old_list: None,
};

static STROKES: KindInfo = KindInfo {
    label: "strokes",
    noun: "stroke image",
    folders: &["strokes", ""],
    extensions: &[".png", ".jpg", ".jpeg"],
    named_by_file: false,
    deep: false,
    named_inside: None,
    old_rule: Some(old_strokes),
    old_list: None,
};

static STYLES: KindInfo = KindInfo {
    label: "styles",
    noun: "user style",
    folders: &["styles"],
    extensions: &[".shader.toml"],
    named_by_file: false,
    deep: false,
    named_inside: None,
    old_rule: Some(old_styles),
    old_list: Some(old_style_list),
};

/// TEMPORARY. Where the style assets were looked for before ADR 0114: the
/// scene's directory, the one above, then the working directory (`.`), the
/// one old search besides a transition's that looked there
/// (`ChoiceScope::search`, the ASCII and palette styles' `search_for` as
/// they were).
fn old_style_search(scene_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = old_two_levels(scene_dir);
    // Spelled as nothing, so a file there is `palettes/x`, not `./palettes/x`
    dirs.push(PathBuf::new());
    dirs
}

/// TEMPORARY. A palette or glyph set before ADR 0114 (`PaletteCache` and
/// `GlyphSetCache::candidates` as they were): a name ending in the kind's
/// extension as a path from each directory of [`old_style_search`] and then
/// as written; any other as `<folder>/<name><extension>` in each.
fn old_set_file(name: &str, scene_dir: &Path, folder: &str, extension: &str) -> Option<PathBuf> {
    let search = old_style_search(scene_dir);
    let candidates: Vec<PathBuf> = if name.ends_with(extension) {
        let mut c: Vec<PathBuf> = search.iter().map(|d| d.join(name)).collect();
        c.push(PathBuf::from(name));
        c
    } else {
        search
            .iter()
            .map(|d| d.join(folder).join(format!("{name}{extension}")))
            .collect()
    };
    candidates.into_iter().find(|p| vfs::is_file(p))
}

fn old_palettes(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    old_set_file(name, scene_dir, "palettes", ".palette.toml")
}

/// TEMPORARY. As the ASCII style found a glyph set. (A `ui_text` font and
/// the look measurer searched the same two directories and not the working
/// directory, so for them this old rule can name a file they never read.)
fn old_glyph_sets(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    old_set_file(name, scene_dir, "fonts", ".glyphs.toml")
}

/// TEMPORARY. A `.font.toml` before ADR 0114: the path `ui_text.font` gave,
/// from the scene's directory and nowhere else (`BitmapFont::resolve` as it
/// was).
fn old_fonts(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    let file = scene_dir.join(name);
    vfs::is_file(&file).then_some(file)
}

/// TEMPORARY. The HUD's fonts before ADR 0114: every `.ttf` / `.otf` in the
/// one `fonts/` that existed, beside the scene or else one level up
/// (`find_fonts_dir` as it was), by name.
fn old_font_face_list(scene_dir: &Path) -> Vec<PathBuf> {
    let Some(dir) = old_first_dir(scene_dir, "fonts") else {
        return Vec::new();
    };
    let mut files = vfs::read_dir(&dir).unwrap_or_default();
    files.retain(|p| FONT_FACES.name_of(p).is_some());
    files.sort();
    files
}

fn old_font_faces(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    let file = old_first_dir(scene_dir, "fonts")?.join(name);
    vfs::is_file(&file).then_some(file)
}

/// TEMPORARY. A look before ADR 0114 (`flint_render::look::find` as it
/// was): `<name>.look.toml` in `styles/` beside the scene, one level up,
/// then in the working directory's `styles/`; a name ending `.toml` was a
/// path from the working directory and nothing else.
fn old_looks(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    if name.ends_with(".toml") {
        let as_written = PathBuf::from(name);
        return vfs::is_file(&as_written).then_some(as_written);
    }
    let mut dirs: Vec<PathBuf> = old_two_levels(scene_dir)
        .into_iter()
        .map(|d| d.join("styles"))
        .collect();
    dirs.push(PathBuf::from("styles"));
    dirs.into_iter()
        .map(|dir| dir.join(format!("{name}.look.toml")))
        .find(|p| vfs::is_file(p))
}

/// TEMPORARY. A paper or stroke image before ADR 0114
/// (`stylize::images::candidates` as it was): the name as a path from each
/// directory of [`old_style_search`]; then, for a bare name (no folder, no
/// extension), `<folder>/<name>.png` in each. An absolute name was itself.
fn old_style_image(name: &str, scene_dir: &Path, folder: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() {
        return vfs::is_file(path).then(|| path.to_path_buf());
    }
    let search = old_style_search(scene_dir);
    let mut candidates: Vec<PathBuf> = search.iter().map(|d| d.join(path)).collect();
    if !name.contains(['/', '\\']) && path.extension().is_none() {
        let file = format!("{name}.png");
        candidates.extend(search.iter().map(|d| d.join(folder).join(&file)));
    }
    candidates.into_iter().find(|p| vfs::is_file(p))
}

fn old_papers(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    old_style_image(name, scene_dir, "papers")
}

fn old_strokes(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    old_style_image(name, scene_dir, "strokes")
}

/// TEMPORARY. User styles before ADR 0114: every `*.shader.toml` in
/// `styles/` beside the scene, then in `styles/` one level up, by name in
/// each; of two of one name the first read was kept
/// (`stylize::custom::load_dir` as it was, for one scene: across scenes the
/// first scene loaded decided, #426).
fn old_style_list(scene_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in old_two_levels(scene_dir) {
        let mut files = vfs::read_dir(dir.join("styles")).unwrap_or_default();
        files.retain(|p| STYLES.name_of(p).is_some());
        files.sort();
        out.extend(files);
    }
    out
}

fn old_styles(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    old_style_list(scene_dir)
        .into_iter()
        .find(|file| STYLES.name_of(file).as_deref() == Some(name))
}

/// TEMPORARY. A particle texture before ADR 0114: in the one `particles/`
/// effects were read from (beside the scene, else one level up), then
/// beside the scene, then one level up
/// (`flint_particles::texture_search_dirs` as it was).
fn old_particle_textures(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = old_first_dir(scene_dir, "particles").into_iter().collect();
    dirs.extend(old_two_levels(scene_dir));
    dirs.into_iter()
        .map(|dir| dir.join(name))
        .find(|p| vfs::exists(p))
}

/// TEMPORARY. A transition's target before ADR 0114: the path as written,
/// which is from the working directory, if that exists; then from the
/// scene's directory; then from the one above (`resolve_scene_path` as it
/// was). The one old search that looked at the working directory.
fn old_scenes(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    let as_written = PathBuf::from(name);
    if vfs::exists(&as_written) {
        return Some(as_written);
    }
    old_from_scene(name, scene_dir)
}

/// A procgen spec's own name: `[meta] name`, which is what a scene's
/// `model.asset` or `texture` asks for.
fn spec_name(text: &str) -> Option<String> {
    let doc = text.parse::<toml::Table>().ok()?;
    Some(doc.get("meta")?.get("name")?.as_str()?.to_string())
}

/// TEMPORARY. The first of `<folder>/` beside the scene or one level up
/// that is a directory: several kinds read that one folder and no other.
fn old_first_dir(scene_dir: &Path, folder: &str) -> Option<PathBuf> {
    let local = scene_dir.join(folder);
    if vfs::is_dir(&local) {
        return Some(local);
    }
    let up = scene_dir.parent()?.join(folder);
    vfs::is_dir(&up).then_some(up)
}

/// TEMPORARY. Scripts before ADR 0114: `scripts/` beside the scene if that
/// directory exists, else `scripts/` one level up if that exists, and only
/// that one directory, so a `scripts/` beside a scene hid the project's
/// (`flint_script::sync::load_scripts_from_scene` as it was). An `import`
/// was `lib/<module>.rhai` in the same one directory.
fn old_scripts(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    let file = old_first_dir(scene_dir, "scripts")?.join(name);
    vfs::is_file(&file).then_some(file)
}

/// TEMPORARY. Specs before ADR 0114, most wanted first: `models/` beside
/// the scene, `specs/` beside it, `specs/` one level up, each with its
/// subfolders; of two specs of one name in a folder the one read later won
/// (`spec_dirs` and `ProcGenResolver::discover_and_index` as they were:
/// the parent was read first and overwritten by what was nearer).
fn old_spec_list(scene_dir: &Path) -> Vec<PathBuf> {
    fn visit(dir: &Path, out: &mut Vec<PathBuf>) {
        for path in vfs::read_dir(dir).unwrap_or_default() {
            if vfs::is_dir(&path) {
                visit(&path, out);
            } else if SPECS.name_of(&path).is_some() {
                out.push(path);
            }
        }
    }
    let mut dirs = vec![scene_dir.join("models"), scene_dir.join("specs")];
    dirs.extend(scene_dir.parent().map(|p| p.join("specs")));
    let mut out = Vec::new();
    for dir in dirs {
        let mut files = Vec::new();
        visit(&dir, &mut files);
        files.reverse();
        out.extend(files);
    }
    out
}

fn old_specs(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    old_spec_list(scene_dir)
        .into_iter()
        .find(|file| SPECS.name_at(file).as_deref() == Some(name))
}

/// TEMPORARY. Prefabs before ADR 0114: `prefabs/<name>.prefab.toml` beside
/// the scene, then one level up (`resolve_prefab_path` as it was).
fn old_prefabs(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    old_two_levels(scene_dir)
        .into_iter()
        .map(|dir| dir.join("prefabs").join(format!("{name}.prefab.toml")))
        .find(|p| vfs::exists(p))
}

/// TEMPORARY (see [`KindInfo::old_rule`]). Particles before ADR 0114:
/// `particles/` beside the scene if that directory exists, else `particles/`
/// one level up if that exists, and only that one directory, whatever it
/// holds (`flint_particles::resolve_particles_dir` as it was).
fn old_particles(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    let local = scene_dir.join("particles");
    let dir = if vfs::is_dir(&local) {
        local
    } else {
        let up = scene_dir.parent()?.join("particles");
        vfs::is_dir(&up).then_some(up)?
    };
    let file = dir.join(format!("{name}.particles.toml"));
    vfs::is_file(&file).then_some(file)
}

/// TEMPORARY. The scene's directory and the one above it, which is as far
/// as any search went before ADR 0114.
fn old_two_levels(scene_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![scene_dir.to_path_buf()];
    dirs.extend(scene_dir.parent().map(Path::to_path_buf));
    dirs
}

/// TEMPORARY. Models before ADR 0114: `models/<name>.glb` then `.gltf`
/// beside the scene, then the same one level up; the extension was always
/// added (`flint_render::model_loader::resolve_model_path` as it was).
fn old_models(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    old_two_levels(scene_dir)
        .into_iter()
        .flat_map(|dir| ["glb", "gltf"].map(|ext| dir.join("models").join(format!("{name}.{ext}"))))
        .find(|p| vfs::exists(p))
}

/// TEMPORARY. Textures before ADR 0114: the scene's directory, its
/// `textures/`, the directory above, its `textures/`; in each a name with a
/// dot anywhere as written, any other bare, then `.png`, then `_color.png`
/// (`resolve_texture_path` as it was).
fn old_textures(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    let names: Vec<String> = if name.contains('.') {
        vec![name.to_string()]
    } else {
        vec![
            name.to_string(),
            format!("{name}.png"),
            format!("{name}_color.png"),
        ]
    };
    for level in old_two_levels(scene_dir) {
        for dir in [level.clone(), level.join("textures")] {
            for name in &names {
                let path = dir.join(name);
                if vfs::exists(&path) {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// TEMPORARY. UI sprites before ADR 0114: `sprites/<name>` beside the
/// scene, `sprites/<name>` one level up, then `<name>` beside the scene
/// (`PlayerApp::load_ui_texture` as it was).
fn old_sprites(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    let mut places = vec![scene_dir.join("sprites").join(name)];
    places.extend(scene_dir.parent().map(|p| p.join("sprites").join(name)));
    places.push(scene_dir.join(name));
    places.into_iter().find(|p| vfs::exists(p))
}

/// TEMPORARY. The one `animations/` the three animation loaders read before
/// ADR 0114: beside the scene if that directory exists, else one level up
/// if that exists, and only that one directory.
fn old_animations_dir(scene_dir: &Path) -> Option<PathBuf> {
    let local = scene_dir.join("animations");
    if vfs::is_dir(&local) {
        return Some(local);
    }
    let up = scene_dir.parent()?.join("animations");
    vfs::is_dir(&up).then_some(up)
}

/// TEMPORARY. The file named `<name><ext>` in the old `animations/`.
fn old_animation_file(name: &str, ext: &str, scene_dir: &Path) -> Option<PathBuf> {
    let file = old_animations_dir(scene_dir)?.join(format!("{name}{ext}"));
    vfs::is_file(&file).then_some(file)
}

/// TEMPORARY. Every file ending `ext` in the old `animations/`, by name.
fn old_animation_files(ext: &str, scene_dir: &Path) -> Vec<PathBuf> {
    let Some(dir) = old_animations_dir(scene_dir) else {
        return Vec::new();
    };
    let mut files = vfs::read_dir(&dir).unwrap_or_default();
    files.retain(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(ext))
    });
    files.sort();
    files
}

fn old_animations(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    old_animation_file(name, ".anim.toml", scene_dir)
}
fn old_animation_list(scene_dir: &Path) -> Vec<PathBuf> {
    old_animation_files(".anim.toml", scene_dir)
}
fn old_sprite_animations(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    old_animation_file(name, ".sprite.toml", scene_dir)
}
fn old_sprite_animation_list(scene_dir: &Path) -> Vec<PathBuf> {
    old_animation_files(".sprite.toml", scene_dir)
}
fn old_sequences(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    old_animation_file(name, ".sequence.toml", scene_dir)
}
fn old_sequence_list(scene_dir: &Path) -> Vec<PathBuf> {
    old_animation_files(".sequence.toml", scene_dir)
}

/// TEMPORARY. A sound named by an `audio_source` before ADR 0114: its
/// `file` as a path from the scene's directory, then from the one above
/// (`load_audio_from_world` as it was).
fn old_audio(name: &str, scene_dir: &Path) -> Option<PathBuf> {
    old_two_levels(scene_dir)
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|p| vfs::exists(p))
}

/// TEMPORARY. The preload before ADR 0114: every `.ogg`, `.wav`, `.mp3` and
/// `.flac` in `audio/` beside the scene, then in `audio/` one level up; both
/// folders, a file name in the first hiding the same one in the second.
fn old_audio_list(scene_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in old_two_levels(scene_dir) {
        let mut files = vfs::read_dir(dir.join("audio")).unwrap_or_default();
        files.retain(|p| AUDIO.name_of(p).is_some());
        files.sort();
        out.extend(files);
    }
    out
}

impl AssetKind {
    /// Every kind, in the order `flint asset where --help` lists them.
    pub const ALL: &'static [AssetKind] = &[
        AssetKind::Particles,
        AssetKind::Models,
        AssetKind::Textures,
        AssetKind::Sprites,
        AssetKind::Animations,
        AssetKind::SpriteAnimations,
        AssetKind::Sequences,
        AssetKind::Audio,
        AssetKind::Scripts,
        AssetKind::Specs,
        AssetKind::Prefabs,
        AssetKind::WoozePresets,
        AssetKind::Files,
        AssetKind::ParticleTextures,
        AssetKind::Scenes,
        AssetKind::Palettes,
        AssetKind::GlyphSets,
        AssetKind::Fonts,
        AssetKind::FontFaces,
        AssetKind::Looks,
        AssetKind::Papers,
        AssetKind::Strokes,
        AssetKind::Styles,
    ];

    pub fn info(self) -> &'static KindInfo {
        match self {
            AssetKind::Particles => &PARTICLES,
            AssetKind::Models => &MODELS,
            AssetKind::Textures => &TEXTURES,
            AssetKind::Sprites => &SPRITES,
            AssetKind::Animations => &ANIMATIONS,
            AssetKind::SpriteAnimations => &SPRITE_ANIMATIONS,
            AssetKind::Sequences => &SEQUENCES,
            AssetKind::Audio => &AUDIO,
            AssetKind::Scripts => &SCRIPTS,
            AssetKind::Specs => &SPECS,
            AssetKind::Prefabs => &PREFABS,
            AssetKind::WoozePresets => &WOOZE_PRESETS,
            AssetKind::Files => &FILES,
            AssetKind::ParticleTextures => &PARTICLE_TEXTURES,
            AssetKind::Scenes => &SCENES,
            AssetKind::Palettes => &PALETTES,
            AssetKind::GlyphSets => &GLYPH_SETS,
            AssetKind::Fonts => &FONTS,
            AssetKind::FontFaces => &FONT_FACES,
            AssetKind::Looks => &LOOKS,
            AssetKind::Papers => &PAPERS,
            AssetKind::Strokes => &STROKES,
            AssetKind::Styles => &STYLES,
        }
    }

    /// The kind's label: `particles`.
    pub fn label(self) -> &'static str {
        self.info().label
    }

    /// The kind a label names, without case.
    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|k| k.label().eq_ignore_ascii_case(label))
    }
}

impl fmt::Display for AssetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl KindInfo {
    /// The kind's extensions that add something to a name.
    fn endings(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.extensions.iter().copied().filter(|e| !e.is_empty())
    }

    /// The file names a name stands for: itself when it already ends in one
    /// of the kind's extensions (or the kind has none, or its names are file
    /// names and this one has a dot in its last part), else the name with
    /// each extension in turn.
    pub fn file_names(&self, name: &str) -> Vec<String> {
        let last = name.rsplit(['/', '\\']).next().unwrap_or(name);
        if self.extensions.is_empty()
            || self.endings().any(|e| name.ends_with(e))
            || (self.named_by_file && last.contains('.'))
        {
            return vec![name.to_string()];
        }
        self.extensions
            .iter()
            .map(|e| format!("{name}{e}"))
            .collect()
    }

    /// A file's asset name: its file name less the kind's extension, or the
    /// whole file name for a kind named by file. `None` for a file of
    /// another kind.
    pub fn name_of(&self, file: &Path) -> Option<String> {
        let file_name = file.file_name()?.to_str()?;
        if self.endings().next().is_none() {
            return Some(file_name.to_string());
        }
        let stem = self
            .endings()
            .find_map(|e| file_name.strip_suffix(e))
            .filter(|stem| !stem.is_empty())?;
        Some(if self.named_by_file { file_name } else { stem }.to_string())
    }

    /// A file's asset name wherever the kind keeps it: the one written in
    /// the file for a kind [named inside](Self::named_inside), else
    /// [`name_of`](Self::name_of). `None` for a file of another kind, or
    /// one that states no name.
    pub fn name_at(&self, file: &Path) -> Option<String> {
        match self.named_inside {
            Some(read) => {
                self.name_of(file)?;
                read(&vfs::read_to_string(file).ok()?)
            }
            None => self.name_of(file),
        }
    }
}

/// No file of this kind and name anywhere the engine looks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotFound {
    pub kind: AssetKind,
    pub name: String,
    /// Every path tried, in the order tried.
    pub tried: Vec<PathBuf>,
}

impl fmt::Display for NotFound {
    /// One line saying what is missing, then each path tried on its own.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} '{}' not found. Tried:",
            self.kind.info().noun,
            self.name
        )?;
        for path in &self.tried {
            write!(f, "\n  {}", shown(path))?;
        }
        Ok(())
    }
}

impl std::error::Error for NotFound {}

/// TEMPORARY, for one release (ADR 0114 decision 3, #765): the kind's old
/// search would have picked another file than the one rule does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderChange {
    pub kind: AssetKind,
    pub name: String,
    /// The file the old rule picked.
    pub old: PathBuf,
    /// The file the one rule picks; `None` when it finds none (the old file
    /// lies outside the project).
    pub new: Option<PathBuf>,
}

impl fmt::Display for OrderChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} '{}': the search order changed (ADR 0114). Before, the engine loaded {}; ",
            self.kind.info().noun,
            self.name,
            shown(&self.old)
        )?;
        match &self.new {
            Some(new) => write!(
                f,
                "now it loads {}. Move or rename one of them if that is the wrong file",
                shown(new)
            ),
            None => f.write_str(
                "now it loads none: that file is outside the project. Move it into the \
                 project, or put a flint.toml at the folder that holds both",
            ),
        }
    }
}

impl OrderChange {
    /// Whether this is the first time this process has seen this exact
    /// change (kind, name and both files). The log uses it to speak once.
    pub fn first_time(&self) -> bool {
        static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
        // `|` is in no file name the key is built from on any platform
        // that matters here; a clash would only cost one warning.
        let key = format!(
            "{}|{}|{}|{}",
            self.kind,
            self.name,
            same_dir_key(&self.old),
            self.new.as_deref().map(same_dir_key).unwrap_or_default(),
        );
        SEEN.get_or_init(Default::default)
            .lock()
            .map(|mut seen| seen.insert(key))
            .unwrap_or(false)
    }

    /// Warn about it, once a process for each distinct change. Through
    /// `tracing`, which every host sends to stderr or the browser console:
    /// never stdout, which `flint render --report json` owns. For a loader
    /// that works from [`Project::explain`], which logs nothing itself.
    pub fn log(&self) {
        if self.first_time() {
            tracing::warn!("{self}");
        }
    }
}

/// The whole answer to "which file?": what `flint asset where` prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub kind: AssetKind,
    pub name: String,
    /// The file the engine loads, if there is one.
    pub found: Option<PathBuf>,
    /// The paths tried and not found, in order: all that came before
    /// `found`, or every candidate when nothing was found.
    pub tried: Vec<PathBuf>,
    /// TEMPORARY (see [`OrderChange`]).
    pub order_change: Option<OrderChange>,
}

/// One file [`Project::list`] found: the name a scene would use for it, and
/// where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub name: String,
    pub path: PathBuf,
}

impl Project {
    /// The file the engine loads for `name`, an asset of `kind`, in a scene
    /// whose directory is `scene_dir`; or everywhere it looked. The one
    /// search rule is in the module docs. Paths keep the spelling of
    /// `scene_dir` (relative stays relative) for as long as it lasts.
    pub fn resolve(
        &self,
        kind: AssetKind,
        name: &str,
        scene_dir: &Path,
    ) -> Result<PathBuf, NotFound> {
        let resolution = self.explain(kind, name, scene_dir);
        if let Some(change) = &resolution.order_change {
            change.log();
        }
        match resolution.found {
            Some(path) => Ok(path),
            None => Err(NotFound {
                kind,
                name: resolution.name,
                tried: resolution.tried,
            }),
        }
    }

    /// [`resolve`](Self::resolve) with its working shown and nothing logged.
    pub fn explain(&self, kind: AssetKind, name: &str, scene_dir: &Path) -> Resolution {
        let mut tried = Vec::new();
        let mut found = None;
        if kind.info().named_inside.is_some() && as_written(name) == Written::Name {
            // Found by the name its file states: the nearest visible file
            // that states this one. What was tried is the folders read
            // before it, or all of them.
            found = self
                .visible(kind, scene_dir)
                .into_iter()
                .find(|listed| listed.name == name)
                .map(|listed| listed.path);
            tried = self.places(kind, scene_dir, false);
            if let Some(found) = &found {
                let before = tried.iter().position(|dir| found.starts_with(dir));
                tried.truncate(before.unwrap_or(0));
            }
        } else {
            for path in self.candidates(kind, name, scene_dir) {
                if vfs::is_file(&path) {
                    found = Some(path);
                    break;
                }
                tried.push(path);
            }
        }
        let order_change = order_change(kind, name, scene_dir, found.as_deref());
        Resolution {
            kind,
            name: name.to_string(),
            found,
            tried,
            order_change,
        }
    }

    /// Every path [`explain`](Self::explain) would try for `name`, in order
    /// and each once: the as-written step, then the walk. (A kind named
    /// inside its files is not found by path: `explain` reads the folders
    /// of [`search_dirs`](Self::search_dirs) for it instead.)
    pub fn candidates(&self, kind: AssetKind, name: &str, scene_dir: &Path) -> Vec<PathBuf> {
        let names = kind.info().file_names(name);
        let written = as_written(name);
        let mut out: Vec<PathBuf> = Vec::new();
        let mut push = |path: PathBuf| {
            if !out.contains(&path) {
                out.push(path);
            }
        };
        if written != Written::Name {
            let from = if scene_dir.as_os_str().is_empty() {
                Path::new(".")
            } else {
                scene_dir
            };
            for file_name in &names {
                // `join` leaves an absolute name as it stands.
                push(under(from, file_name));
            }
        }
        if written != Written::Absolute {
            for dir in self.search_dirs(kind, scene_dir) {
                for file_name in &names {
                    // `under`, so a file at the root of a relative walk is
                    // `wall.png` and not `./wall.png`.
                    push(under(&dir, file_name));
                }
            }
        }
        out
    }

    /// Every file of `kind` a scene in `scene_dir` can see, by the same
    /// rule: a nearer file hides a farther one of the same name. Nearest
    /// first, and by name within a directory. Only files directly in a
    /// kind's named folders are listed (and in their subfolders for a
    /// [deep](KindInfo::deep) kind, a folder's own files before its
    /// subfolders'), never the walk's own directories (a folder of `""`),
    /// which hold whatever else is beside a scene.
    pub fn list(&self, kind: AssetKind, scene_dir: &Path) -> Vec<Listed> {
        let (out, changes) = self.listing(kind, scene_dir);
        for change in changes {
            change.log();
        }
        out
    }

    /// TEMPORARY (see [`OrderChange`]): where a kind's old search listed
    /// another set of files than [`list`](Self::list) does, without logging.
    pub fn list_order_changes(&self, kind: AssetKind, scene_dir: &Path) -> Vec<OrderChange> {
        self.listing(kind, scene_dir).1
    }

    /// [`list`](Self::list) alone, with nothing compared or logged.
    fn visible(&self, kind: AssetKind, scene_dir: &Path) -> Vec<Listed> {
        let info = kind.info();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for dir in self.places(kind, scene_dir, false) {
            let mut files = Vec::new();
            files_under(&dir, "", info.deep, &mut files);
            for (subfolder, path) in files {
                let name = match info.named_inside {
                    Some(_) => info.name_at(&path),
                    None => info.name_of(&path).map(|n| format!("{subfolder}{n}")),
                };
                let Some(name) = name else {
                    continue;
                };
                if seen.insert(name.clone()) {
                    out.push(Listed { name, path });
                }
            }
        }
        out
    }

    fn listing(&self, kind: AssetKind, scene_dir: &Path) -> (Vec<Listed>, Vec<OrderChange>) {
        let info = kind.info();
        let out = self.visible(kind, scene_dir);
        let mut changes = Vec::new();
        if info.old_list.is_none() {
            for Listed { name, path } in &out {
                changes.extend(order_change(kind, name, scene_dir, Some(path)));
            }
        }
        // A kind loaded as a whole: compare the two lists, which also sees
        // a file the old search loaded and this one doesn't reach.
        if let Some(old_list) = info.old_list {
            let mut old_seen = HashSet::new();
            for old in old_list(scene_dir) {
                let Some(name) = info.name_at(&old) else {
                    continue;
                };
                if !old_seen.insert(name.clone()) {
                    continue; // the old search kept the first of a name too
                }
                let new = out.iter().find(|l| l.name == name).map(|l| l.path.clone());
                if new
                    .as_deref()
                    .is_some_and(|new| same_dir_key(new) == same_dir_key(&old))
                {
                    continue;
                }
                changes.push(OrderChange {
                    kind,
                    name,
                    old,
                    new,
                });
            }
        }
        (out, changes)
    }

    /// The directories searched for `kind` from a scene in `scene_dir`, in
    /// order, whether or not they exist: the kind's folders under each
    /// directory of the walk, then under the engine mount.
    pub fn search_dirs(&self, kind: AssetKind, scene_dir: &Path) -> Vec<PathBuf> {
        self.places(kind, scene_dir, true)
    }

    /// [`search_dirs`](Self::search_dirs), with or without the walk's own
    /// directories (a kind's folder of `""`).
    fn places(&self, kind: AssetKind, scene_dir: &Path, bare: bool) -> Vec<PathBuf> {
        let folders = kind.info().folders;
        let (mut levels, root) = self.levels(scene_dir);
        levels.extend(self.engine_under(&root));
        let mut out = Vec::with_capacity(levels.len() * folders.len());
        for level in &levels {
            for folder in folders {
                if bare || !folder.is_empty() {
                    out.push(under(level, folder));
                }
            }
        }
        out
    }

    /// The mounted engine directory: `<root>/engine` when it is a directory
    /// and this project isn't the engine checkout itself.
    pub fn engine_mount(&self) -> Option<PathBuf> {
        self.engine_under(&self.root)
    }

    /// [`engine_mount`](Self::engine_mount) with the root spelled as the
    /// caller has it.
    fn engine_under(&self, root: &Path) -> Option<PathBuf> {
        if self.found == Found::Engine {
            return None;
        }
        let engine = under(root, "engine");
        vfs::is_dir(&engine).then_some(engine)
    }

    /// The directories of the walk: the scene's, then each above it up to
    /// the root. See the module docs for a scene outside the root and for a
    /// project with no marker. With them, the root in the walk's own
    /// spelling (relative when the scene's directory was).
    fn levels(&self, scene_dir: &Path) -> (Vec<PathBuf>, PathBuf) {
        let start = if scene_dir.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            scene_dir.to_path_buf()
        };
        let root_key = same_dir_key(&self.root);
        let mut out = Vec::new();
        let mut reached = false;
        let mut root = self.root.clone();
        let mut dir = start.clone();
        // No tree is this deep; the bound only keeps a strange path finite.
        for _ in 0..128 {
            if same_dir_key(&dir) == root_key {
                root = dir.clone();
                out.push(dir);
                reached = true;
                break;
            }
            let above = parent_dir(&dir).or_else(|| {
                // The spelling ran out (`.`, or it ends in `..`): carry on
                // from where it really is, if that is anywhere.
                let real = fold(&anchor(&dir));
                parent_dir(&real).filter(|_| real != dir)
            });
            out.push(dir);
            match above {
                Some(above) => dir = above,
                None => break,
            }
        }
        if !reached {
            // The scene isn't under the root: beside itself, then the root.
            out.truncate(1);
        }
        if self.found == Found::Fallback && out.len() == 1 {
            let above = parent_dir(&start).or_else(|| parent_dir(&fold(&anchor(&start))));
            out.extend(above);
        }
        if !reached {
            out.push(self.root.clone());
        }
        let mut seen = HashSet::new();
        out.retain(|d| seen.insert(same_dir_key(d)));
        (out, root)
    }

    /// The file a scene names by a path of its own ([`AssetKind::Files`]: a
    /// heightmap, a skybox, a spline), for a loader that carries on: the
    /// file found, or a warning that says `what` was asked for and lists
    /// every path tried, and the path beside the scene for the loader's own
    /// error to name.
    pub fn file_or_beside(&self, what: &str, name: &str, scene_dir: &Path) -> PathBuf {
        self.resolve(AssetKind::Files, name, scene_dir)
            .unwrap_or_else(|not_found| {
                tracing::warn!("{what}: {not_found}");
                scene_dir.join(name)
            })
    }

    /// The project a scene belongs to, for a loader that has to carry on
    /// whatever the manifest says: as [`for_path`](Self::for_path), and when
    /// the manifest can't be read, a warning and the project without it.
    pub fn for_scene(scene: &Path) -> Project {
        Self::for_path(scene).unwrap_or_else(|e| {
            tracing::warn!("{e}; searching for assets as if there were no manifest");
            let dir = scene.parent().filter(|p| !p.as_os_str().is_empty());
            Project::bare(
                dir.map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from(".")),
            )
        })
    }
}

/// A project and the directory its searches start from: what a loader that
/// outlives one lookup holds (the renderer's styles, a debug panel's lists),
/// so the project is discovered once a scene and not once a lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetScope {
    pub project: Project,
    pub scene_dir: PathBuf,
}

impl AssetScope {
    pub fn new(project: Project, scene_dir: impl Into<PathBuf>) -> Self {
        Self {
            project,
            scene_dir: scene_dir.into(),
        }
    }

    /// The scope of the scene at `scene`: its project
    /// ([`Project::for_scene`]) and its directory.
    pub fn for_scene(scene: &Path) -> Self {
        let dir = scene.parent().filter(|p| !p.as_os_str().is_empty());
        Self::new(
            Project::for_scene(scene),
            dir.unwrap_or_else(|| Path::new(".")),
        )
    }

    /// The scope of a scene in `scene_dir`, for a host that kept only the
    /// directory.
    pub fn for_dir(scene_dir: &Path) -> Self {
        let dir = if scene_dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            scene_dir
        };
        let project = Project::for_path(dir).unwrap_or_else(|e| {
            tracing::warn!("{e}; searching for assets as if there were no manifest");
            Project::bare(dir.to_path_buf())
        });
        Self::new(project, dir)
    }

    /// [`Project::resolve`] from this scope's directory.
    pub fn resolve(&self, kind: AssetKind, name: &str) -> Result<PathBuf, NotFound> {
        self.project.resolve(kind, name, &self.scene_dir)
    }

    /// [`Project::explain`] from this scope's directory.
    pub fn explain(&self, kind: AssetKind, name: &str) -> Resolution {
        self.project.explain(kind, name, &self.scene_dir)
    }

    /// [`Project::list`] from this scope's directory.
    pub fn list(&self, kind: AssetKind) -> Vec<Listed> {
        self.project.list(kind, &self.scene_dir)
    }

    /// [`Project::search_dirs`] from this scope's directory.
    pub fn search_dirs(&self, kind: AssetKind) -> Vec<PathBuf> {
        self.project.search_dirs(kind, &self.scene_dir)
    }
}

impl Resolution {
    /// Every path the answer depends on, for hot reload: the ones tried
    /// before the file found (one appearing there takes over) and the file
    /// itself; every candidate when nothing was found.
    pub fn watched(&self) -> impl Iterator<Item = &PathBuf> {
        self.tried.iter().chain(&self.found)
    }
}

/// A path for a message or a report: with forward slashes on every platform,
/// so a walk begun from `scenes/act1` is not spelled `scenes/act1\particles`
/// on Windows, and a report reads the same everywhere.
pub fn shown(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// How a name is spelled, for the as-written step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Written {
    /// A plain name, maybe with subfolders: the walk finds it.
    Name,
    /// A path from the scene: it starts with `./` or `../`.
    FromScene,
    /// A path from a root or a drive: it is one file and no search.
    Absolute,
}

fn as_written(name: &str) -> Written {
    let path = Path::new(name);
    if path.is_absolute() || path.has_root() || name.starts_with('/') {
        return Written::Absolute;
    }
    match path.components().next() {
        Some(Component::CurDir | Component::ParentDir) => Written::FromScene,
        Some(Component::Prefix(_)) => Written::Absolute,
        _ => Written::Name,
    }
}

/// The files in `dir` by name, each with the subfolder it is in (`""`, or
/// `lib/`); with `deep`, then those of each subfolder, by name.
fn files_under(dir: &Path, subfolder: &str, deep: bool, out: &mut Vec<(String, PathBuf)>) {
    let Ok(mut entries) = vfs::read_dir(dir) else {
        return;
    };
    entries.sort();
    let mut folders = Vec::new();
    for path in entries {
        if deep && vfs::is_dir(&path) {
            folders.push(path);
        } else {
            out.push((subfolder.to_string(), path));
        }
    }
    for folder in folders {
        if let Some(name) = folder.file_name().and_then(|n| n.to_str()) {
            files_under(&folder, &format!("{subfolder}{name}/"), deep, out);
        }
    }
}

/// `dir/rel`, with `.` left out of the spelling.
fn under(dir: &Path, rel: &str) -> PathBuf {
    if rel.is_empty() {
        dir.to_path_buf()
    } else if dir == Path::new(".") {
        PathBuf::from(rel)
    } else {
        dir.join(rel)
    }
}

/// `path` with `.` dropped and each `..` folded into what precedes it.
fn fold(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// TEMPORARY (see [`OrderChange`]): what the kind's old rule would have
/// picked, when that is another file than `new`.
fn order_change(
    kind: AssetKind,
    name: &str,
    scene_dir: &Path,
    new: Option<&Path>,
) -> Option<OrderChange> {
    let old = (kind.info().old_rule?)(name, scene_dir)?;
    if new.is_some_and(|new| same_dir_key(new) == same_dir_key(&old)) {
        return None;
    }
    Some(OrderChange {
        kind,
        name: name.to_string(),
        old,
        new: new.map(Path::to_path_buf),
    })
}

#[cfg(test)]
#[path = "assets_kinds_tests.rs"]
mod kinds_tests;

#[cfg(test)]
#[path = "assets_style_tests.rs"]
mod style_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::MemorySource;
    use std::sync::Arc;

    const FIRE: &str = "fire.particles.toml";

    /// Run `f` over a memory source holding `files`; a path ending in `/`
    /// is a directory.
    fn with_files<R>(files: &[&str], f: impl FnOnce() -> R) -> R {
        let mut source = MemorySource::new();
        for path in files {
            if path.ends_with('/') {
                source.insert(format!("{path}.keep"), "");
            } else {
                source.insert(path, "");
            }
        }
        vfs::scoped(Arc::new(source), f)
    }

    fn project(start: &str) -> Project {
        Project::discover_with_home(Path::new(start), None).unwrap()
    }

    fn paths(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    fn fire(project: &Project, scene_dir: &str) -> Result<PathBuf, NotFound> {
        project.resolve(AssetKind::Particles, "fire", Path::new(scene_dir))
    }

    #[test]
    fn scene_beats_parent_beats_root_beats_engine() {
        let all = [
            "game/flint.toml",
            "game/scenes/act1/a.scene.toml",
            "game/scenes/act1/particles/fire.particles.toml",
            "game/scenes/particles/fire.particles.toml",
            "game/particles/fire.particles.toml",
            "game/engine/particles/fire.particles.toml",
        ];
        let expect = [
            "game/scenes/act1/particles",
            "game/scenes/particles",
            "game/particles",
            "game/engine/particles",
        ];
        // Take the nearest away each time: the next one is found, and the
        // ones taken away are the tried list, in order.
        for gone in 0..expect.len() {
            let files: Vec<&str> = all
                .iter()
                .copied()
                .filter(|f| !expect[..gone].iter().any(|dir| f.starts_with(dir)))
                .chain(["game/engine/.keep"])
                .collect();
            with_files(&files, || {
                let p = project("game/scenes/act1/a.scene.toml");
                let r = p.explain(AssetKind::Particles, "fire", Path::new("game/scenes/act1"));
                assert_eq!(r.found, Some(Path::new(expect[gone]).join(FIRE)));
                let missed: Vec<PathBuf> = expect[..gone]
                    .iter()
                    .map(|d| Path::new(d).join(FIRE))
                    .collect();
                assert_eq!(r.tried, missed);
            });
        }
    }

    #[test]
    fn a_scene_two_folders_deep_finds_the_projects_particles() {
        with_files(
            &[
                "game/flint.toml",
                "game/particles/fire.particles.toml",
                "game/scenes/act1/cave/a.scene.toml",
                "game/scenes/b.scene.toml",
            ],
            || {
                let deep = project("game/scenes/act1/cave/a.scene.toml");
                let shallow = project("game/scenes/b.scene.toml");
                let want = Path::new("game/particles").join(FIRE);
                assert_eq!(fire(&deep, "game/scenes/act1/cave").unwrap(), want);
                assert_eq!(fire(&shallow, "game/scenes").unwrap(), want);
            },
        );
    }

    #[test]
    fn not_found_lists_every_path_in_the_order_tried() {
        with_files(
            &[
                "game/flint.toml",
                "game/engine/schemas/",
                "game/scenes/act1/a.scene.toml",
            ],
            || {
                let p = project("game/scenes/act1/a.scene.toml");
                let err = fire(&p, "game/scenes/act1").unwrap_err();
                assert_eq!(err.kind, AssetKind::Particles);
                assert_eq!(err.name, "fire");
                let dirs = [
                    "game/scenes/act1/particles",
                    "game/scenes/particles",
                    "game/particles",
                    "game/engine/particles",
                ];
                let want: Vec<PathBuf> = dirs.iter().map(|d| Path::new(d).join(FIRE)).collect();
                assert_eq!(err.tried, want);
                let text = err.to_string();
                let lines: Vec<&str> = text.lines().collect();
                assert_eq!(lines[0], "particle effect 'fire' not found. Tried:");
                assert_eq!(lines.len(), 5, "{text}");
                for (line, path) in lines[1..].iter().zip(&want) {
                    assert_eq!(*line, format!("  {}", shown(path)));
                }
            },
        );
    }

    #[test]
    fn no_engine_folder_no_engine_place_and_an_engine_checkout_has_none() {
        with_files(&["game/flint.toml", "game/scenes/a.scene.toml"], || {
            let p = project("game/scenes/a.scene.toml");
            assert_eq!(p.engine_mount(), None);
            assert_eq!(
                p.search_dirs(AssetKind::Particles, Path::new("game/scenes")),
                paths(&["game/scenes/particles", "game/particles"])
            );
        });
        // The engine's own checkout, with an `engine/` folder that is not
        // a mount of anything.
        with_files(
            &[
                "flint/crates/flint-core/Cargo.toml",
                "flint/engine/particles/fire.particles.toml",
                "flint/examples/a.scene.toml",
            ],
            || {
                let p = project("flint/examples/a.scene.toml");
                assert_eq!(p.found, Found::Engine);
                assert_eq!(p.engine_mount(), None);
                assert!(fire(&p, "flint/examples").is_err());
            },
        );
    }

    #[test]
    fn names_with_an_extension_or_a_subfolder() {
        with_files(
            &[
                "game/flint.toml",
                "game/particles/fire.particles.toml",
                "game/particles/fx/spark.particles.toml",
                "game/scenes/a.scene.toml",
            ],
            || {
                let p = project("game/scenes/a.scene.toml");
                let dir = Path::new("game/scenes");
                let k = AssetKind::Particles;
                assert_eq!(
                    p.resolve(k, "fire.particles.toml", dir).unwrap(),
                    Path::new("game/particles").join(FIRE)
                );
                assert_eq!(
                    p.resolve(k, "fx/spark", dir).unwrap(),
                    Path::new("game/particles").join("fx/spark.particles.toml")
                );
                // Never the bare name, and never doubled.
                let err = p.resolve(k, "ice.particles.toml", dir).unwrap_err();
                assert!(err
                    .tried
                    .iter()
                    .all(|t| t.ends_with("particles/ice.particles.toml")));
            },
        );
    }

    #[test]
    fn inside_a_bundle_paths_are_the_bundles_own() {
        // A bundle's root is `.`: the manifest and the folders sit at the
        // top of the file source, and engine files are mounted at `engine/`.
        with_files(
            &[
                "flint.toml",
                "engine/schemas/",
                "engine/particles/dust.particles.toml",
                "particles/fire.particles.toml",
                "scenes/act1/a.scene.toml",
            ],
            || {
                let p = project("scenes/act1/a.scene.toml");
                assert_eq!(p.root, Path::new("."));
                let dir = Path::new("scenes/act1");
                assert_eq!(
                    fire(&p, "scenes/act1").unwrap(),
                    Path::new("particles").join(FIRE)
                );
                assert_eq!(
                    p.search_dirs(AssetKind::Particles, dir),
                    paths(&[
                        "scenes/act1/particles",
                        "scenes/particles",
                        "particles",
                        "engine/particles"
                    ])
                );
                assert_eq!(
                    p.resolve(AssetKind::Particles, "dust", dir).unwrap(),
                    Path::new("engine/particles").join("dust.particles.toml")
                );
                // A scene at the bundle's root.
                assert_eq!(fire(&p, "").unwrap(), Path::new("particles").join(FIRE));
                assert_eq!(fire(&p, ".").unwrap(), Path::new("particles").join(FIRE));
            },
        );
    }

    #[test]
    fn a_project_with_no_marker_still_looks_one_level_up() {
        with_files(
            &[
                "loose/particles/fire.particles.toml",
                "loose/scenes/a.scene.toml",
            ],
            || {
                let p = project("loose/scenes/a.scene.toml");
                assert_eq!(p.found, Found::Fallback);
                assert_eq!(p.root, Path::new("loose/scenes"));
                assert_eq!(
                    fire(&p, "loose/scenes").unwrap(),
                    Path::new("loose/particles").join(FIRE)
                );
                assert_eq!(
                    p.search_dirs(AssetKind::Particles, Path::new("loose/scenes")),
                    paths(&["loose/scenes/particles", "loose/particles"])
                );
            },
        );
    }

    #[test]
    fn a_scene_outside_the_root_is_searched_beside_itself_then_at_the_root() {
        with_files(
            &[
                "game/flint.toml",
                "game/particles/fire.particles.toml",
                "elsewhere/deep/a.scene.toml",
            ],
            || {
                let p = project("game");
                assert_eq!(
                    p.search_dirs(AssetKind::Particles, Path::new("elsewhere/deep")),
                    paths(&["elsewhere/deep/particles", "game/particles"])
                );
            },
        );
    }

    #[test]
    fn listing_shadows_by_name_nearest_first() {
        with_files(
            &[
                "game/flint.toml",
                "game/engine/particles/dust.particles.toml",
                "game/engine/particles/fire.particles.toml",
                "game/particles/fire.particles.toml",
                "game/particles/rain.particles.toml",
                "game/particles/notes.txt",
                "game/scenes/particles/fire.particles.toml",
                "game/scenes/a.scene.toml",
            ],
            || {
                let p = project("game/scenes/a.scene.toml");
                let listed = p.list(AssetKind::Particles, Path::new("game/scenes"));
                let got: Vec<(String, PathBuf)> =
                    listed.into_iter().map(|l| (l.name, l.path)).collect();
                assert_eq!(
                    got,
                    [
                        (
                            "fire".to_string(),
                            Path::new("game/scenes/particles").join(FIRE)
                        ),
                        (
                            "rain".to_string(),
                            Path::new("game/particles").join("rain.particles.toml")
                        ),
                        (
                            "dust".to_string(),
                            Path::new("game/engine/particles").join("dust.particles.toml")
                        ),
                    ]
                );
                // Each listed file is the one `resolve` gives for its name.
                for l in p.list(AssetKind::Particles, Path::new("game/scenes")) {
                    assert_eq!(
                        p.resolve(AssetKind::Particles, &l.name, Path::new("game/scenes")),
                        Ok(l.path)
                    );
                }
            },
        );
    }

    #[test]
    fn the_order_change_fires_when_the_old_rule_picked_another_file() {
        // The scene sits at the project root, and the old rule's "one
        // level up" is outside the project.
        with_files(
            &[
                "work/particles/fire.particles.toml",
                "work/game/flint.toml",
                "work/game/a.scene.toml",
            ],
            || {
                let p = project("work/game/a.scene.toml");
                let dir = Path::new("work/game");
                let r = p.explain(AssetKind::Particles, "fire", dir);
                assert_eq!(r.found, None);
                let change = r.order_change.expect("the old rule found one");
                assert_eq!(change.old, Path::new("work/particles").join(FIRE));
                assert_eq!(change.new, None);
                let text = change.to_string();
                assert!(text.contains("the search order changed"), "{text}");
                assert!(text.contains("outside the project"), "{text}");
            },
        );
        // The same, with an engine copy the new rule falls through to: the
        // warning names both files.
        with_files(
            &[
                "work/particles/fire.particles.toml",
                "work/game/flint.toml",
                "work/game/engine/particles/fire.particles.toml",
                "work/game/a.scene.toml",
            ],
            || {
                let p = project("work/game/a.scene.toml");
                let r = p.explain(AssetKind::Particles, "fire", Path::new("work/game"));
                let change = r.order_change.expect("the old rule picked another");
                let new = Path::new("work/game/engine/particles").join(FIRE);
                assert_eq!(change.new.as_deref(), Some(new.as_path()));
                let text = change.to_string();
                assert!(text.contains(&shown(&change.old)), "{text}");
                let shown = shown(change.new.as_ref().unwrap());
                assert!(text.contains(&shown), "{text}");
            },
        );
    }

    #[test]
    fn the_order_change_stays_quiet_when_both_rules_agree_or_the_old_found_none() {
        // Both pick the file beside the scene.
        with_files(
            &[
                "game/flint.toml",
                "game/scenes/particles/fire.particles.toml",
                "game/particles/fire.particles.toml",
                "game/scenes/a.scene.toml",
            ],
            || {
                let p = project("game/scenes/a.scene.toml");
                let r = p.explain(AssetKind::Particles, "fire", Path::new("game/scenes"));
                assert!(r.found.is_some());
                assert_eq!(r.order_change, None);
            },
        );
        // Both pick the one a level up.
        with_files(
            &[
                "game/flint.toml",
                "game/particles/fire.particles.toml",
                "game/scenes/a.scene.toml",
            ],
            || {
                let p = project("game/scenes/a.scene.toml");
                let r = p.explain(AssetKind::Particles, "fire", Path::new("game/scenes"));
                assert_eq!(r.order_change, None);
            },
        );
        // The old rule stopped at the first `particles/` that existed and
        // found nothing there; the new one finds the project's. Something
        // found where nothing was is not a change of order.
        with_files(
            &[
                "game/flint.toml",
                "game/scenes/particles/rain.particles.toml",
                "game/particles/fire.particles.toml",
                "game/scenes/a.scene.toml",
            ],
            || {
                let p = project("game/scenes/a.scene.toml");
                let r = p.explain(AssetKind::Particles, "fire", Path::new("game/scenes"));
                assert_eq!(r.found, Some(Path::new("game/particles").join(FIRE)));
                assert_eq!(r.order_change, None);
            },
        );
    }

    #[test]
    fn a_change_is_new_once_a_process() {
        let change = OrderChange {
            kind: AssetKind::Particles,
            name: "only_in_this_test".into(),
            old: PathBuf::from("a/particles/only_in_this_test.particles.toml"),
            new: None,
        };
        assert!(change.first_time());
        assert!(!change.first_time());
        let other = OrderChange {
            new: Some(PathBuf::from(
                "b/particles/only_in_this_test.particles.toml",
            )),
            ..change.clone()
        };
        assert!(other.first_time());
        assert!(!other.first_time());
    }

    #[test]
    fn kinds_are_data() {
        for kind in AssetKind::ALL {
            let info = kind.info();
            assert_eq!(AssetKind::from_label(info.label), Some(*kind));
            assert!(!info.folders.is_empty());
        }
        assert_eq!(
            AssetKind::from_label("PARTICLES"),
            Some(AssetKind::Particles)
        );
        assert_eq!(AssetKind::from_label("nope"), None);
        let info = AssetKind::Particles.info();
        assert_eq!(info.file_names("fire"), ["fire.particles.toml"]);
        assert_eq!(info.file_names("a.particles.toml"), ["a.particles.toml"]);
        assert_eq!(
            info.name_of(Path::new("x/fire.particles.toml")).as_deref(),
            Some("fire")
        );
        assert_eq!(info.name_of(Path::new("x/fire.toml")), None);
    }
}
