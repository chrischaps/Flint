//! The looks a project ships (#1272): each `*.look.toml` it holds is read
//! as the player reads it, and its tables for styles that aren't built in
//! are judged against the styles the project can see.
//!
//! A look carries no style registry (#742), so `Look::parse` judges only a
//! built-in style's table; a table naming a user style (a `.shader.toml` in
//! some `styles/` folder) or a game's Rust style is the scene's to judge,
//! and before this a typo there (`[styles.hologramm]`) showed only as a
//! load warning in whatever scene took the look. Here a look is judged
//! against the registry of every folder that can take it: the folder it
//! sits in (the one holding its `styles/`) and each scene folder checked
//! under it. A style none of them holds is a `bad-style-setting` warning on
//! the look's file; a warning, since a game's Rust style is known only to
//! its binary.

use super::scene::Memo;
use super::Subject;
use flint_core::diagnostic::{codes, Diagnostic};
use flint_core::project::shown;
use flint_core::vfs;
use flint_render::look::Look;
use flint_render::stylize::StyleRegistry;
use std::collections::BTreeSet;
use std::path::Path;

/// How a look file's name ends.
const LOOK_SUFFIX: &str = ".look.toml";

/// Every look file among `files`, read and judged.
pub(super) fn check(
    subject: &Subject,
    files: &BTreeSet<String>,
    memo: &mut Memo,
) -> Vec<Diagnostic> {
    let mut found = Vec::new();
    for key in files.iter().filter(|key| key.ends_with(LOOK_SUFFIX)) {
        let path = subject.path(key);
        let stem = path
            .file_name()
            .map(|n| {
                n.to_string_lossy()
                    .trim_end_matches(LOOK_SUFFIX)
                    .to_string()
            })
            .unwrap_or_default();
        let look = vfs::read_to_string(&path)
            .map_err(|e| format!("reading {}: {e}", shown(&path)))
            .and_then(|text| Look::parse(&stem, &text, &shown(&path)));
        match look {
            Ok(look) => {
                let seen = memo.styles_able_to_take(home(&path));
                found.extend(style_diagnostics(&look, &seen));
            }
            Err(e) => found.push(Diagnostic::error(codes::BAD_LOOK, e).file(&path).hint(
                "`flint render --look help` skips it, and a scene naming it draws without it",
            )),
        }
    }
    found
}

/// The folder whose scenes can take the look at `path`: the one holding
/// its `styles/`, or its own folder for a look kept elsewhere. The search
/// for a look walks up from the scene (ADR 0114), trying `<dir>/styles/`
/// and `<dir>/` at each step, so every scene under that folder finds it.
fn home(path: &Path) -> &Path {
    let dir = path.parent().unwrap_or(Path::new(""));
    match dir.file_name() {
        Some(name) if name == "styles" => dir.parent().unwrap_or(Path::new("")),
        _ => dir,
    }
}

/// What `seen` says of `look`'s tables for styles that aren't built in,
/// each a warning on the look's file. The scene pass and the look pass
/// word it the same, so one mistake found by both is listed once.
pub(super) fn style_diagnostics(look: &Look, seen: &[&StyleRegistry]) -> Vec<Diagnostic> {
    if look.is_builtin() {
        return Vec::new();
    }
    look.style_warnings(seen)
        .into_iter()
        .map(|warning| {
            Diagnostic::warning(
                codes::BAD_STYLE_SETTING,
                format!("look '{}': {warning}", look.name),
            )
            .file(&look.source)
            .field("post_process.styles")
            .hint(
                "a scene that takes this look draws it without what this names; \
                 `flint render --list-styles <scene>` lists every style, key and value \
                 (a game's own Rust style is known only to its binary)",
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_looks_home_is_the_folder_holding_its_styles() {
        assert_eq!(
            home(Path::new("game/styles/dusk.look.toml")),
            Path::new("game")
        );
        assert_eq!(home(Path::new("styles/dusk.look.toml")), Path::new(""));
        assert_eq!(
            home(Path::new("game/out/dusk.look.toml")),
            Path::new("game/out")
        );
    }
}
