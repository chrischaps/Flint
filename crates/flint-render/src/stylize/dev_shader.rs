//! Hot reload of the built-in styles' WGSL, for whoever is tuning one
//! (#418).
//!
//! A built-in style's shader is compiled in: [`wgsl!`] concatenates its
//! files with `include_str!` and remembers their names. With
//! `FLINT_STYLE_SHADER_DIR` set (natively; it names the folder the names
//! are relative to, `crates/flint-render/src/stylize` in this checkout),
//! [`kit::shader`](super::kit::shader) reads those files from that folder
//! instead, every one of them (a shared `ascii_common.wgsl` or
//! `../palette.wgsl` included), and [`StyleSet`](super::StyleSet) looks at
//! their stamps every few frames and builds again each style that read a
//! file that moved. So a shader tweak shows in the running viewer or
//! player within a second, with no rebuild of the engine.
//!
//! What's read is checked with naga first. A file that doesn't parse or
//! validate is an error naming `file:line:col` with the line quoted
//! ([`errors`], which the viewer and player draw over the frame), and the
//! shader that was last good (else the compiled-in one) is built instead,
//! so the picture stays as it was. A shader naga accepts that wgpu still
//! refuses (an entry point renamed, a binding the Rust side doesn't make)
//! panics while the style is built; the style is then built once more
//! from the last good shaders and the panic is its error.
//!
//! Unset, or on the web, nothing here reads a file: the compiled-in text
//! is the shader, as in every shipping build.

/// A built-in shader: its compiled-in text and the files it was
/// concatenated from, relative to `crates/flint-render/src/stylize`.
/// Made by [`wgsl!`]; a bare `&'static str` converts to one with no files,
/// which is never read from disk.
#[derive(Clone, Copy, Debug)]
pub struct Wgsl {
    pub text: &'static str,
    pub files: &'static [&'static str],
}

impl From<&'static str> for Wgsl {
    fn from(text: &'static str) -> Self {
        Self { text, files: &[] }
    }
}

/// A [`Wgsl`] from one or more files beside the calling module, in order:
/// `wgsl!("ascii_common.wgsl", "ascii.wgsl")` is their `include_str!`s
/// concatenated, plus the names, so a dev build can read them again.
macro_rules! wgsl {
    ($($file:literal),+ $(,)?) => {
        $crate::stylize::dev_shader::Wgsl {
            text: concat!($(include_str!($file)),+),
            files: &[$($file),+],
        }
    };
}
pub(crate) use wgsl;

/// The variable that turns it on.
pub const VAR: &str = "FLINT_STYLE_SHADER_DIR";

/// How many [`StyleSet::ensure`](super::StyleSet::ensure) calls (frames)
/// pass between looks at the files' stamps: a quarter second at 60 fps.
pub const POLL_FRAMES: u32 = 15;

pub use imp::*;

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::Wgsl;
    use flint_core::file_watch::{self, Stamp};
    use std::borrow::Cow;
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Component, Path, PathBuf};
    use std::sync::{Mutex, RwLock};

    /// The folder, once read from the environment; [`set_dir`] replaces it.
    static DIR: RwLock<Option<Option<PathBuf>>> = RwLock::new(None);

    /// Every file read so far, with its stamp when it was read.
    static WATCHED: Mutex<BTreeMap<PathBuf, Option<Stamp>>> = Mutex::new(BTreeMap::new());

    /// Per shader (its files, joined): the text last built from disk, and
    /// what is wrong with the files now.
    static SHADERS: Mutex<BTreeMap<String, Shader>> = Mutex::new(BTreeMap::new());

    /// Per style: the panic of its last build from disk, if it panicked.
    static BUILD_ERRORS: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

    #[derive(Default)]
    struct Shader {
        last_good: Option<String>,
        /// The style it was read for, and its error.
        error: Option<(String, String)>,
    }

    /// What one style's build read.
    #[derive(Default)]
    struct Build {
        name: String,
        files: BTreeSet<PathBuf>,
        /// Texts that checked, made good only if the whole build succeeds.
        pending: Vec<(String, String)>,
        /// Build from the last good texts, reading nothing.
        fallback: bool,
    }

    thread_local! {
        static BUILD: RefCell<Option<Build>> = const { RefCell::new(None) };
    }

    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The folder shaders are read from, `None` when it's off.
    pub fn dir() -> Option<PathBuf> {
        if let Some(dir) = DIR.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
            return dir.clone();
        }
        let mut slot = DIR.write().unwrap_or_else(|e| e.into_inner());
        slot.get_or_insert_with(|| {
            let dir = std::env::var_os(super::VAR)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)?;
            if !flint_core::vfs::is_file(dir.join("invert.wgsl")) {
                tracing::warn!(
                    "{}={}: no invert.wgsl there; it should name crates/flint-render/src/stylize",
                    super::VAR,
                    dir.display()
                );
            }
            // A warning, so a variable left set is seen
            tracing::warn!(
                "{} is set: the built-in styles' WGSL is read from {} and hot-reloads",
                super::VAR,
                dir.display()
            );
            Some(dir)
        })
        .clone()
    }

    /// Whether shaders are read from disk.
    pub fn enabled() -> bool {
        dir().is_some()
    }

    /// Read shaders from `dir` from now on (`None`: the compiled-in
    /// ones), whatever the environment says, and forget what was read.
    /// For tests and tools; a host leaves it to the variable.
    pub fn set_dir(dir: Option<PathBuf>) {
        *DIR.write().unwrap_or_else(|e| e.into_inner()) = Some(dir);
        lock(&WATCHED).clear();
        lock(&SHADERS).clear();
        lock(&BUILD_ERRORS).clear();
    }

    /// `a/b/../c` as `a/c`, so an error names `src/palette.wgsl` rather
    /// than `src/stylize/../palette.wgsl`.
    fn tidy(path: &Path) -> PathBuf {
        let mut out = PathBuf::new();
        for c in path.components() {
            match c {
                Component::ParentDir
                    if matches!(out.components().next_back(), Some(Component::Normal(_))) =>
                {
                    out.pop();
                }
                Component::CurDir => {}
                c => out.push(c.as_os_str()),
            }
        }
        out
    }

    /// The text `kit::shader` builds `src` from: the compiled-in text,
    /// unless shaders are read from disk (module docs).
    pub fn source(label: &str, src: Wgsl) -> Cow<'static, str> {
        let Some(dir) = dir() else {
            return Cow::Borrowed(src.text);
        };
        if src.files.is_empty() {
            return Cow::Borrowed(src.text);
        }
        let key = src.files.join(" + ");
        let paths: Vec<PathBuf> = src.files.iter().map(|f| tidy(&dir.join(f))).collect();
        // Stamp before reading: a write between the two reloads again
        {
            let mut watched = lock(&WATCHED);
            for p in &paths {
                watched.insert(p.clone(), file_watch::stamp(p));
            }
        }
        let (name, fallback) = BUILD.with(|b| match b.borrow_mut().as_mut() {
            Some(build) => {
                build.files.extend(paths.iter().cloned());
                (build.name.clone(), build.fallback)
            }
            None => (label.to_string(), false),
        });
        let keep = || -> Cow<'static, str> {
            match lock(&SHADERS).get(&key).and_then(|s| s.last_good.clone()) {
                Some(text) => Cow::Owned(text),
                None => Cow::Borrowed(src.text),
            }
        };
        if fallback {
            return keep();
        }
        let mut parts = Vec::with_capacity(paths.len());
        for p in &paths {
            match flint_core::vfs::read_to_string(p) {
                Ok(text) => parts.push(text),
                Err(e) => {
                    let message = format!(
                        "{}: can't read it ({e}); the last good shader stays",
                        p.display()
                    );
                    fail(&key, &name, message);
                    return keep();
                }
            }
        }
        let text = parts.concat();
        match check(&text, &paths, &parts) {
            Ok(()) => {
                lock(&SHADERS).entry(key.clone()).or_default().error = None;
                let committed = BUILD.with(|b| match b.borrow_mut().as_mut() {
                    Some(build) => {
                        build.pending.push((key.clone(), text.clone()));
                        false
                    }
                    None => true,
                });
                if committed {
                    lock(&SHADERS).entry(key).or_default().last_good = Some(text.clone());
                }
                Cow::Owned(text)
            }
            Err(message) => {
                fail(&key, &name, message);
                keep()
            }
        }
    }

    fn fail(key: &str, name: &str, message: String) {
        let mut shaders = lock(&SHADERS);
        let shader = shaders.entry(key.to_string()).or_default();
        let new = shader.error.as_ref().map(|(_, m)| m) != Some(&message);
        if new {
            tracing::warn!("style '{name}': {message}");
        }
        shader.error = Some((name.to_string(), message));
    }

    /// naga's parse and validation of `text`, the files `parts` (read from
    /// `paths`) concatenated, with an error placed in the file it is in.
    pub(super) fn check(text: &str, paths: &[PathBuf], parts: &[String]) -> Result<(), String> {
        let place = |offset: Option<u32>, message: &str| -> String {
            let Some(offset) = offset else {
                let names: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
                return format!("{}: {message}", names.join(" + "));
            };
            let mut start = 0usize;
            for (path, part) in paths.iter().zip(parts) {
                let end = start + part.len();
                let at = offset as usize;
                if at < end || end == text.len() {
                    let local = at.saturating_sub(start).min(part.len());
                    let before = &part[..local];
                    let line = before.matches('\n').count() + 1;
                    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
                    let column = part[line_start..local].chars().count() + 1;
                    let quoted = part[line_start..].lines().next().unwrap_or("").trim_end();
                    return format!(
                        "{}:{line}:{column}: {message}\n    {line} | {quoted}",
                        path.display()
                    );
                }
                start = end;
            }
            format!("{}: {message}", paths[0].display())
        };
        let module = naga::front::wgsl::parse_str(text)
            .map_err(|e| place(e.location(text).map(|l| l.offset), e.message()))?;
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            // What user styles get too: pack2x16float and quantizeToF16
            // (ascii_edge_cells.wgsl) need SHADER_FLOAT16_IN_FLOAT32, which
            // wgpu grants wherever the adapter has SHADER_F16_IN_F32 (#945, #946)
            crate::stylize::custom::style_capabilities(),
        )
        .validate(&module)
        .map_err(|e| {
            let mut message = e.as_inner().to_string();
            let mut inner: Option<&dyn std::error::Error> = std::error::Error::source(e.as_inner());
            while let Some(i) = inner {
                message.push_str(&format!(": {i}"));
                inner = i.source();
            }
            place(e.location(text).map(|l| l.offset), &message)
        })?;
        Ok(())
    }

    /// What a panic says, for a build's error.
    fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
        if let Some(s) = panic.downcast_ref::<String>() {
            s.clone()
        } else if let Some(s) = panic.downcast_ref::<&str>() {
            s.to_string()
        } else {
            "a panic".to_string()
        }
    }

    fn run<T>(
        name: &str,
        fallback: bool,
        make: &dyn Fn() -> std::thread::Result<T>,
    ) -> (std::thread::Result<T>, Build) {
        BUILD.with(|b| {
            *b.borrow_mut() = Some(Build {
                name: name.to_string(),
                fallback,
                ..Default::default()
            })
        });
        let made = make();
        let build = BUILD.with(|b| b.borrow_mut().take()).unwrap_or_default();
        (made, build)
    }

    /// Build style (or shared pass) `name` with `make`, which catches its
    /// own panic, noting the files its shaders read: the second value,
    /// for [`changed`]. Off, it is `make()` and no files. A build from
    /// disk that panics is built again from the last good shaders
    /// (module docs).
    pub fn build<T>(
        name: &str,
        make: impl Fn() -> std::thread::Result<T>,
    ) -> (std::thread::Result<T>, Vec<PathBuf>) {
        if !enabled() {
            return (make(), Vec::new());
        }
        let (made, build) = run(name, false, &make);
        let files: Vec<PathBuf> = build.files.into_iter().collect();
        match made {
            Ok(pass) => {
                let mut shaders = lock(&SHADERS);
                for (key, text) in build.pending {
                    shaders.entry(key).or_default().last_good = Some(text);
                }
                lock(&BUILD_ERRORS).remove(name);
                (Ok(pass), files)
            }
            Err(panic) if !files.is_empty() => {
                let message = format!(
                    "building it from {} panicked; the last good shaders stay:\n    {}",
                    files
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                    panic_text(panic.as_ref()).trim()
                );
                tracing::warn!("style '{name}': {message}");
                lock(&BUILD_ERRORS).insert(name.to_string(), message);
                let (again, _) = run(name, true, &make);
                (again, files)
            }
            Err(panic) => (Err(panic), files),
        }
    }

    /// The files read whose stamps moved since they were read (or since
    /// the last call), which it records as seen.
    pub fn changed() -> BTreeSet<PathBuf> {
        let mut watched = lock(&WATCHED);
        let mut moved = BTreeSet::new();
        for (path, stamp) in watched.iter_mut() {
            let now = file_watch::stamp(path);
            if now != *stamp {
                *stamp = now;
                moved.insert(path.clone());
            }
        }
        moved
    }

    /// What is wrong with the built-in shaders read from disk, as
    /// (style, message), for a host to draw beside
    /// [`custom::errors`](super::super::custom::errors). Empty when it's
    /// off or all of them are good.
    pub fn errors() -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for shader in lock(&SHADERS).values() {
            if let Some(e) = &shader.error {
                if !out.contains(e) {
                    out.push(e.clone());
                }
            }
        }
        for (name, message) in lock(&BUILD_ERRORS).iter() {
            out.push((name.clone(), message.clone()));
        }
        out.sort();
        out
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::Wgsl;
    use std::borrow::Cow;
    use std::path::PathBuf;

    pub fn dir() -> Option<PathBuf> {
        None
    }

    pub fn enabled() -> bool {
        false
    }

    pub fn source(_label: &str, src: Wgsl) -> Cow<'static, str> {
        Cow::Borrowed(src.text)
    }

    pub fn build<T>(
        _name: &str,
        make: impl Fn() -> std::thread::Result<T>,
    ) -> (std::thread::Result<T>, Vec<PathBuf>) {
        (make(), Vec::new())
    }

    pub fn changed() -> std::collections::BTreeSet<PathBuf> {
        Default::default()
    }

    pub fn errors() -> Vec<(String, String)> {
        Vec::new()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::imp::check;
    use std::path::PathBuf;

    #[test]
    fn an_error_names_the_file_and_line_it_is_in() {
        let paths = [PathBuf::from("common.wgsl"), PathBuf::from("style.wgsl")];
        let parts = [
            "fn helper() -> f32 {\n    return 1.0;\n}\n".to_string(),
            "fn other() -> f32 {\n    let x = 2.0\n    return x;\n}\n".to_string(),
        ];
        let text = parts.concat();
        let e = check(&text, &paths, &parts).unwrap_err();
        // The parser stops at the `return` after the missing `;`. The place
        // is Flint's; the words are naga's (naga 30: "expected `;`, found
        // "return""), so only the token is held
        assert!(e.starts_with("style.wgsl:3:5: expected "), "{e}");
        assert!(e.lines().next().is_some_and(|l| l.contains(';')), "{e}");
        assert!(e.ends_with("    3 |     return x;"), "{e}");
        let good = [parts[0].clone(), parts[0].replace("helper", "two")];
        assert!(check(&good.concat(), &paths, &good).is_ok());
    }

    #[test]
    fn a_validation_error_is_placed_too() {
        let paths = [PathBuf::from("a.wgsl"), PathBuf::from("b.wgsl")];
        let parts = [
            "fn g() -> f32 {\n    return 1.0;\n}\n".to_string(),
            "\nfn f() -> f32 {\n    return vec2<f32>(1.0);\n}\n".to_string(),
        ];
        let e = check(&parts.concat(), &paths, &parts).unwrap_err();
        // naga places a bad return at its function
        assert!(
            e.starts_with("b.wgsl:2:1: Function [1] 'f' is invalid"),
            "{e}"
        );
    }
}
