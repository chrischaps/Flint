//! `flint export web`: any Flint game as a folder a static server can
//! serve (#106, epic #94).
//!
//! ```bash
//! flint export web examples/                   # examples/dist/web: index.html, the player, games/demo/
//! flint export web ../Float --out site         # into site/, beside any game already exported there
//! flint export web . --player play-dist        # a player trunk already built, instead of building one
//! flint export web . --meta title="My Game" --meta line="One sentence."
//! python -m http.server -d examples/dist/web   # then open http://localhost:8000/
//! ```
//!
//! Three steps, the same ones play.chaps.dev's deploy takes:
//!
//! 1. **The player.** `--player <dir>` copies a `trunk build` of
//!    `crates/flint-player` that's already made. Without it the command
//!    builds one with trunk (`--release --cargo-profile web-release`, the
//!    site's build) from the Flint tree this CLI was built from, or
//!    `--engine <dir>`, into `<engine>/target/web-player`. The default keeps
//!    the packer and the player from one tree, so the bundle format they
//!    speak is the same (`flint_core::bundle::FORMAT_VERSION`, checked by
//!    the player at load: a mismatch says so instead of drawing nothing).
//! 2. **The bundle.** `flint bundle` into `games/<id>/`, with the game's
//!    own `bundle.toml` rules (include, exclude, late parts, mounts).
//! 3. **The listing.** `games.json`: this game's entry (from `bundle.toml`'s
//!    `[web]`, then `--meta`), replacing one of the same id and keeping the
//!    rest. A folder holding one game plays it at its bare address; with
//!    several, the bare address is the chooser and `?game=<id>` plays one.
//!
//! A `video` or `poster` that's a path in the game, not a URL, is copied
//! beside the bundle. The output folder must be empty, missing, or a past
//! export (it has a `games.json`); `--clean` empties a past export first.

use super::bundle::{self, BundleArgs, WebMeta};
use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Subcommand)]
pub enum ExportCommands {
    /// A folder any static server can serve: the web player, the game's
    /// bundles under games/<id>/, and games.json
    Web(WebArgs),
}

#[derive(Args)]
pub struct WebArgs {
    /// The game's root folder (where its bundle.toml is, if it has one)
    #[arg(default_value = ".")]
    pub game: PathBuf,

    /// The folder to write (default: dist/web in the game root)
    #[arg(long)]
    pub out: Option<PathBuf>,

    /// A trunk build of crates/flint-player to copy, instead of building one
    #[arg(long, value_name = "DIR")]
    pub player: Option<PathBuf>,

    /// The Flint checkout to build the player from (default: the one this
    /// CLI was built from)
    #[arg(long, value_name = "DIR")]
    pub engine: Option<PathBuf>,

    /// The game's id: its bundle name and its ?game= value (default:
    /// bundle.toml's name, else flint.toml's `[project] id`, else the
    /// folder's name, lowercased)
    #[arg(long)]
    pub name: Option<String>,

    /// The scene the player starts (overrides bundle.toml)
    #[arg(long)]
    pub scene: Option<String>,

    /// A games.json key for this game, over bundle.toml's [web]: title,
    /// line, controls, touch, orientation, page, video, poster, film.
    /// Repeatable; an empty value clears it
    #[arg(long = "meta", value_name = "KEY=VALUE")]
    pub meta: Vec<String>,

    /// Empty the output folder first (only a past export: one with games.json)
    #[arg(long)]
    pub clean: bool,
}

/// What `orientation` may say; unset, a phone turns the game freely.
const ORIENTATIONS: &[&str] = &["landscape"];

pub fn run(cmd: ExportCommands) -> Result<()> {
    match cmd {
        ExportCommands::Web(args) => web(args),
    }
}

fn web(args: WebArgs) -> Result<()> {
    let root = args.game.as_path();
    if !root.is_dir() {
        bail!("{} is not a folder", root.display());
    }
    let config = bundle::read_config(root)?;
    // bundle.toml's name, else the project manifest's id (ADR 0114)
    let project = bundle::read_project(root)?;
    let id = match args.name.clone().or(config.name.clone()).or(project.id) {
        Some(name) => {
            if !valid_id(&name) {
                bail!("the game's id {name:?} must be lowercase letters, digits, - and _");
            }
            name
        }
        None => {
            let folder = std::fs::canonicalize(root)?
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            id_from(&folder).with_context(|| {
                format!("no id to be made from the folder name {folder:?}: pass --name <id>")
            })?
        }
    };

    let mut meta = config.web.clone();
    apply_meta(&mut meta, &args.meta)?;
    if let Some(o) = &meta.orientation {
        if !ORIENTATIONS.contains(&o.as_str()) {
            bail!("orientation must be one of {ORIENTATIONS:?}, not {o:?}");
        }
    }

    let out = args.out.clone().unwrap_or_else(|| root.join("dist/web"));
    check_out(&out, root)?;
    let player = match &args.player {
        Some(dir) => dir.clone(),
        None => build_player(args.engine.as_deref())?,
    };
    check_player(&player)?;

    // Everything checks before anything is removed: pack into a staging
    // folder, then move it into place.
    let games = out.join("games");
    let staging = out.join(format!(".games-{id}.staging"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    let packed = bundle::pack(BundleArgs {
        game: root.to_path_buf(),
        out_dir: Some(staging.clone()),
        scene: args.scene.clone(),
        name: Some(id.clone()),
        list: false,
        dry_run: false,
    })
    .inspect_err(|_| {
        let _ = std::fs::remove_dir_all(&staging);
        remove_if_empty(&out);
    })?;
    if meta.title.is_none() {
        meta.title = Some(scene_title(&root.join(&packed.scene)).unwrap_or_else(|| id.clone()));
    }
    copy_media(&mut meta, root, &staging, &id)?;

    if args.clean && out.join("games.json").is_file() {
        for entry in std::fs::read_dir(&out)? {
            let path = entry?.path();
            if path == staging {
                continue;
            }
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }
        }
    }
    copy_dir(&player, &out)?;
    let dest = games.join(&id);
    if dest.exists() {
        std::fs::remove_dir_all(&dest)?;
    }
    std::fs::create_dir_all(&games)?;
    std::fs::rename(&staging, &dest)
        .with_context(|| format!("moving the bundle into {}", dest.display()))?;

    let listing = out.join("games.json");
    let existing = match std::fs::read_to_string(&listing) {
        Ok(text) => Some(
            serde_json::from_str(&text)
                .with_context(|| format!("reading {}", listing.display()))?,
        ),
        Err(_) => None,
    };
    let (merged, count) = merge_listing(existing, &id, &meta)?;
    std::fs::write(&listing, serde_json::to_string_pretty(&merged)? + "\n")?;

    let size: u64 = packed.files.iter().map(|(_, s)| s).sum();
    println!();
    println!(
        "exported {id} to {} ({} in {} bundle{}; {count} game{} listed)",
        out.display(),
        mb(size),
        packed.files.len(),
        if packed.files.len() == 1 { "" } else { "s" },
        if count == 1 { "" } else { "s" },
    );
    println!("  serve it:  python -m http.server -d {}", out.display());
    if count == 1 {
        println!("  then open: http://localhost:8000/");
    } else {
        println!("  then open: http://localhost:8000/?game={id}");
    }
    Ok(())
}

fn mb(bytes: u64) -> String {
    format!("{:.2} MB", bytes as f64 / 1_000_000.0)
}

/// A bundle name the page's `?game=` takes (`flint_web.rs`'s `game_bundle`).
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// A folder name as an id: lowercased, anything else a `-`.
fn id_from(folder: &str) -> Option<String> {
    let mut id = String::new();
    for c in folder.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
            id.push(c);
        } else if !id.ends_with('-') {
            id.push('-');
        }
    }
    let id = id.trim_matches('-').to_string();
    valid_id(&id).then_some(id)
}

fn apply_meta(meta: &mut WebMeta, pairs: &[String]) -> Result<()> {
    for pair in pairs {
        let Some((key, value)) = pair.split_once('=') else {
            bail!("--meta {pair:?}: write it KEY=VALUE");
        };
        let Some(slot) = meta.get_mut(key.trim()) else {
            bail!(
                "--meta: no key {key:?}; the keys are {}",
                WebMeta::KEYS.join(", ")
            );
        };
        *slot = (!value.is_empty()).then(|| value.to_string());
    }
    Ok(())
}

/// The start scene's `[scene] name`, the gate's title when `[web]` gives none.
fn scene_title(scene: &Path) -> Option<String> {
    let value: toml::Value = std::fs::read_to_string(scene).ok()?.parse().ok()?;
    let name = value.get("scene")?.get("name")?.as_str()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// A folder this command may write into: missing, empty or a past export,
/// and never the game itself or a folder holding it.
fn check_out(out: &Path, root: &Path) -> Result<()> {
    if let (Ok(o), Ok(r)) = (std::fs::canonicalize(out), std::fs::canonicalize(root)) {
        if r.starts_with(&o) {
            bail!(
                "--out {} holds the game itself; write the export somewhere else",
                out.display()
            );
        }
    }
    if !out.exists() {
        return Ok(());
    }
    if !out.is_dir() {
        bail!("--out {} is a file", out.display());
    }
    let empty = std::fs::read_dir(out)?.next().is_none();
    if !empty && !out.join("games.json").is_file() {
        bail!(
            "{} isn't empty and isn't an export (no games.json); pick an empty folder",
            out.display()
        );
    }
    Ok(())
}

fn remove_if_empty(dir: &Path) {
    if std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_none()) {
        let _ = std::fs::remove_dir(dir);
    }
}

/// A trunk build of the player: its page and its wasm.
fn check_player(dir: &Path) -> Result<()> {
    if !dir.join("index.html").is_file() {
        bail!(
            "{} has no index.html: give --player a `trunk build` of crates/flint-player",
            dir.display()
        );
    }
    let wasm = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .any(|e| e.path().extension().is_some_and(|x| x == "wasm"));
    if !wasm {
        bail!("{} has an index.html but no .wasm", dir.display());
    }
    Ok(())
}

/// Build the web player with trunk, as play.chaps.dev's deploy does.
fn build_player(engine: Option<&Path>) -> Result<PathBuf> {
    let engine = match engine {
        Some(dir) => dir.to_path_buf(),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
    };
    let crate_dir = engine.join("crates/flint-player");
    if !crate_dir.join("Trunk.toml").is_file() {
        bail!(
            "no Flint source at {} to build the web player from: pass --player <a trunk build> \
             or --engine <a Flint checkout>",
            engine.display()
        );
    }
    let engine = std::fs::canonicalize(&engine)?;
    let dist = engine.join("target").join("web-player");
    println!(
        "building the web player with trunk (release, web-release) into {}",
        dist.display()
    );
    let status = Command::new("trunk")
        .args([
            "build",
            "--release",
            "--cargo-profile",
            "web-release",
            "--dist",
        ])
        .arg(&dist)
        .current_dir(engine.join("crates/flint-player"))
        // trunk reads NO_COLOR=1 as its own --no-color flag and refuses "1"
        .env_remove("NO_COLOR")
        .status();
    match status {
        Ok(s) if s.success() => Ok(dist),
        Ok(s) => bail!("trunk build failed ({s})"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!(
            "trunk isn't installed: `cargo install trunk --locked` (and `rustup target add \
             wasm32-unknown-unknown`), or pass --player <a trunk build>"
        ),
        Err(e) => Err(e).context("running trunk"),
    }
}

/// Copy a folder's contents over another's, leaving what's already there.
/// A player's own `games/` or `games.json` (a site built into its dist)
/// aren't the player's: they stay behind.
fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == "games" || name == "games.json" {
            continue;
        }
        copy_tree(&entry.path(), &to.join(&name))?;
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
    } else {
        std::fs::copy(from, to).with_context(|| format!("copying {}", from.display()))?;
    }
    Ok(())
}

/// A `video` or `poster` that's a path in the game, not a URL, goes beside
/// the bundle, and the listing names it there.
fn copy_media(meta: &mut WebMeta, root: &Path, into: &Path, id: &str) -> Result<()> {
    let root = std::fs::canonicalize(root)?;
    for key in ["video", "poster"] {
        let slot = meta.get_mut(key).unwrap();
        let Some(value) = slot.clone() else {
            continue;
        };
        if value.starts_with("https://") || value.starts_with("http://") {
            continue;
        }
        let path = std::fs::canonicalize(root.join(&value))
            .ok()
            .filter(|p| p.is_file() && p.starts_with(&root));
        let Some(path) = path else {
            bail!("{key} {value:?} is not a file in the game");
        };
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        std::fs::copy(&path, into.join(&name))?;
        *slot = Some(format!("games/{id}/{name}"));
    }
    Ok(())
}

/// `games.json` with this game's entry in place of one of the same id, or
/// added at the end; and how many games it lists.
fn merge_listing(
    existing: Option<serde_json::Value>,
    id: &str,
    meta: &WebMeta,
) -> Result<(serde_json::Value, usize)> {
    let mut entry = serde_json::Map::new();
    entry.insert("id".into(), id.into());
    if let serde_json::Value::Object(fields) = serde_json::to_value(meta)? {
        entry.extend(fields);
    }
    let mut root = existing.unwrap_or_else(|| serde_json::json!({ "games": [] }));
    let Some(games) = root.get_mut("games").and_then(|g| g.as_array_mut()) else {
        bail!("games.json has no \"games\" list");
    };
    let entry = serde_json::Value::Object(entry);
    match games
        .iter_mut()
        .find(|g| g.get("id").and_then(|v| v.as_str()) == Some(id))
    {
        Some(slot) => *slot = entry,
        None => games.push(entry),
    }
    let count = games.len();
    Ok((root, count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_from_folders() {
        assert_eq!(id_from("Float").as_deref(), Some("float"));
        assert_eq!(id_from("FlintKart").as_deref(), Some("flintkart"));
        assert_eq!(id_from("My Game!").as_deref(), Some("my-game"));
        assert_eq!(id_from("signal_106").as_deref(), Some("signal_106"));
        assert_eq!(id_from("..."), None);
        assert!(valid_id("kart") && !valid_id("Kart") && !valid_id("a/b") && !valid_id(""));
    }

    #[test]
    fn meta_flags_set_and_clear_keys() {
        let mut meta = WebMeta {
            title: Some("Old".into()),
            film: Some("A film".into()),
            ..Default::default()
        };
        apply_meta(
            &mut meta,
            &[
                "title=New = Better".into(),
                "film=".into(),
                "line=Hi".into(),
            ],
        )
        .unwrap();
        assert_eq!(
            meta.title.as_deref(),
            Some("New = Better"),
            "only the first = splits"
        );
        assert_eq!(meta.film, None, "an empty value clears");
        assert_eq!(meta.line.as_deref(), Some("Hi"));
        assert!(apply_meta(&mut meta, &["colour=red".into()]).is_err());
        assert!(apply_meta(&mut meta, &["title".into()]).is_err());
    }

    #[test]
    fn the_listing_replaces_an_id_and_keeps_the_rest_in_order() {
        let meta = |t: &str| WebMeta {
            title: Some(t.into()),
            line: Some("l".into()),
            ..Default::default()
        };
        let (one, n) = merge_listing(None, "float", &meta("SET + DRIFT")).unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            one,
            serde_json::json!({ "games": [{ "id": "float", "title": "SET + DRIFT", "line": "l" }] })
        );
        let (two, n) = merge_listing(Some(one), "kart", &meta("Flint Kart")).unwrap();
        assert_eq!(n, 2);
        let (again, n) = merge_listing(Some(two), "float", &meta("Float, again")).unwrap();
        assert_eq!(n, 2);
        let ids: Vec<_> = again["games"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, ["float", "kart"], "replaced in place, not moved");
        assert_eq!(again["games"][0]["title"], "Float, again");
        let keys: Vec<_> = again["games"][0]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(keys.len(), 3, "unset keys stay out: {keys:?}");
    }
}
