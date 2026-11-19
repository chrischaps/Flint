//! The guarded module a bundle carries (#550): naga's rewrite of a style's
//! pass with every unbounded loop guarded ([`Report::guarded`], #411),
//! written by `flint bundle` beside the `.shader.toml`, so the browser,
//! which has no naga, compiles the guarded text rather than the user's.
//!
//! The file is the module as wgpu builds it natively, after one comment
//! line that names what it was written from:
//!
//! ```text
//! // flint guarded module (#550): styles/spin.wgsl as composed sha256:<hex>
//! ```
//!
//! The hash is of the composed text (prelude, the user's WGSL, entry
//! points) the guard was run on. The web composes the same text from the
//! description and WGSL it reads, and takes the packed module only when
//! the hashes agree: a WGSL or description edited after packing, or a
//! player whose prelude moved since, composes another text, and the pass
//! builds what it composed, unguarded, as before #550. In a bundle nothing
//! changes after packing, so there it never happens.

use super::*;

/// What a guarded module's first line starts with.
const HEADER: &str = "// flint guarded module (#550): ";

/// Where pass `pass` of `source` keeps its guarded module in a bundle:
/// `<stem>.guarded.wgsl` beside the `.shader.toml` (`spin.shader.toml` ->
/// `spin.guarded.wgsl`), or `<stem>.<pass>.guarded.wgsl` for a style of
/// several passes. Its stem, not its WGSL's: two styles may share a WGSL
/// file, and the prelude they compose it with is each its own.
pub fn guarded_path(source: &StyleSource, pass: usize) -> PathBuf {
    let file = source
        .toml_path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = file.strip_suffix(".shader.toml").unwrap_or(&file);
    let name = if source.passes.len() > 1 {
        format!("{stem}.{}.guarded.wgsl", source.passes[pass].name)
    } else {
        format!("{stem}.guarded.wgsl")
    };
    source.toml_path.with_file_name(name)
}

/// The stamp a guarded module carries for `composed`.
fn stamp(composed: &str) -> String {
    flint_core::ContentHash::from_str(composed).to_prefixed_hex()
}

/// Check pass `pass` of `source` with `user` as its WGSL and, when it has a
/// loop to guard, the file a bundle packs for it: its path
/// ([`guarded_path`]) and text, the guarded module under its header.
/// `Ok(None)` for a pass with nothing to guard (no loop, or only loops the
/// IR proves end, #424); `Err` is the check's message. What `flint bundle`
/// calls for every packed style.
#[cfg(not(target_arch = "wasm32"))]
pub fn guarded_file(
    source: &StyleSource,
    pass: usize,
    user: &str,
) -> Result<Option<(PathBuf, String)>, String> {
    let Some(guarded) = check_pass(source, pass, user)?.guarded else {
        return Ok(None);
    };
    let composed = compose_pass(source, pass, user);
    let wgsl = shown(&source.passes[pass].wgsl_path);
    let text = format!(
        "{HEADER}{wgsl} as composed {}\n{guarded}",
        stamp(&composed.text)
    );
    Ok(Some((guarded_path(source, pass), text)))
}

/// The guarded module packed for pass `pass` of `source`, when there is
/// one and it was written from the text `user` composes to now. Read
/// through the vfs; what the web builds in place of the composed text.
/// `None` for no file, a file without the header, or a stale one.
pub fn packed_guarded(source: &StyleSource, pass: usize, user: &str) -> Option<String> {
    let path = guarded_path(source, pass);
    if !flint_core::vfs::is_file(&path) {
        return None;
    }
    let text = flint_core::vfs::read_to_string(&path).ok()?;
    let (header, _) = text.split_once('\n')?;
    let packed = header.strip_prefix(HEADER)?.rsplit(' ').next()?;
    let composed = compose_pass(source, pass, user);
    if packed == stamp(&composed.text) {
        tracing::info!(
            "style '{}': building {}, the loop-guarded module the bundle packed",
            source.name,
            shown(&path)
        );
        Some(text)
    } else {
        tracing::warn!(
            "{}: written from another version of the style's WGSL or prelude, so its loops \
             run unguarded here; pack the bundle again",
            shown(&path)
        );
        None
    }
}
