//! Checking a composed module with naga, the loop guard, and errors placed
//! in the user's file (moved from `custom.rs`, #742).

use super::*;

// ---------------------------------------------------------------------------
// Checking with naga (native)
// ---------------------------------------------------------------------------

/// What a check found: warnings about a shader that compiles, and the
/// text to build.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// Loops in the user's code: each is guarded ([`LOOP_LIMIT`]) unless
    /// the guard is off ([`set_loop_guard`]) or it is [`bounded`](Self::bounded).
    pub loops: usize,
    /// Of [`loops`](Self::loops), those whose turns the IR proves to be at
    /// most [`LOOP_LIMIT`] (#424: a `for` over octaves or kernel taps with a
    /// constant start, bound and step). They can't hang the GPU, so they
    /// are left as written, unguarded and unwarned.
    pub bounded: usize,
    /// Texture reads in the user's code (the helpers' own not counted).
    pub texture_reads: usize,
    pub warnings: Vec<String>,
    /// The WGSL to give wgpu when it isn't the composed text: the module
    /// written back out by naga with every user loop guarded (#411).
    /// `None` when there is no loop to guard or the guard is off. On the
    /// web (no naga there) it is the module a bundle packed for the pass
    /// ([`packed_guarded`], #550), else `None`.
    pub guarded: Option<String>,
}

/// Texture reads a pixel may make before the check warns (a 9x9 blur is
/// 81): past it, a full-screen pass gets expensive fast.
pub const TEXTURE_READ_WARN: usize = 32;

/// The iterations one run of a user loop may make before the guard breaks
/// out of it (#411). Each loop has its own counter, reset where the loop
/// starts, so nested loops multiply: a 64x64 kernel is fine, a `loop {}`
/// stops after 4096 turns and its pixel draws whatever it reached.
pub const LOOP_LIMIT: u32 = 4096;

thread_local! {
    static LOOP_GUARD: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Turn the loop guard off (or back on) for styles checked and built on
/// this thread from now on. It is on by default; off exists so a test can
/// show a guarded build draws what the unguarded one does.
#[doc(hidden)]
pub fn set_loop_guard(on: bool) {
    LOOP_GUARD.with(|g| g.set(on));
}

/// Parse and validate `user` as `source`'s shader, the way it will be
/// built, and guard its loops. `Err` is the message to show, lines in the
/// user's file.
pub fn check(source: &StyleSource, user: &str) -> Result<Report, String> {
    check_pass(source, source.last_pass(), user)
}

/// [`check`] for pass `pass` of `source` (#412).
#[cfg(not(target_arch = "wasm32"))]
pub fn check_pass(source: &StyleSource, pass: usize, user: &str) -> Result<Report, String> {
    let composed = compose_pass(source, pass, user);
    let file = shown(&source.passes[pass].wgsl_path);
    let mut module = naga::front::wgsl::parse_str(&composed.text).map_err(|e| {
        locate(
            &composed,
            &file,
            e.location(&composed.text)
                .map(|l| (l.line_number, l.line_position)),
            e.labels()
                .next()
                .and_then(|(span, _)| span_text(&composed, span)),
            e.message(),
            source,
            pass,
        )
    })?;
    let mut validator =
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), style_capabilities());
    validator.validate(&module).map_err(|e| {
        let mut message = e.as_inner().to_string();
        let mut source_err: Option<&dyn std::error::Error> =
            std::error::Error::source(e.as_inner());
        while let Some(inner) = source_err {
            message.push_str(&format!(": {inner}"));
            source_err = inner.source();
        }
        locate(
            &composed,
            &file,
            e.location(&composed.text)
                .map(|l| (l.line_number, l.line_position)),
            e.spans()
                .next()
                .and_then(|(span, _)| span_text(&composed, *span)),
            &message,
            source,
            pass,
        )
    })?;
    let mut report = inspect(&module, &composed, &file)?;
    let unbounded = report.loops - report.bounded;
    if unbounded > 0 && LOOP_GUARD.with(|g| g.get()) {
        let user = user_range(&composed);
        guard_loops(&mut module, |span| in_range(&user, span));
        // The guard is ours, so a failure here is a framework bug: say so
        // rather than build the unguarded text
        let info = validator.validate(&module).map_err(|e| {
            format!("{file}: the loop guard made an invalid module (a Flint bug, #411): {e}")
        })?;
        let text =
            naga::back::wgsl::write_string(&module, &info, naga::back::wgsl::WriterFlags::empty())
                .map_err(|e| {
                    format!("{file}: writing the guarded module failed (a Flint bug, #411): {e}")
                })?;
        report.guarded = Some(text);
    } else if unbounded > 0 {
        report.warnings.push(format!(
            "{file}: warning: {unbounded} loop{} without a constant bound and the loop guard is \
             off: a loop that never ends hangs the GPU until the driver resets it (TDR)",
            if unbounded == 1 { "" } else { "s" }
        ));
    }
    Ok(report)
}

/// What Flint's WGSL validators grant a shader (#946): naga's defaults and
/// `SHADER_FLOAT16_IN_FLOAT32`, which naga 30 asks of `pack2x16float`,
/// `unpack2x16float` and `quantizeToF16`. They are core WGSL, naga 23 took
/// them with no capability, and wgpu grants this one wherever the adapter has
/// `DownlevelFlags::SHADER_F16_IN_F32`: every backend Flint runs on except
/// Vulkan on Mesa's llvmpipe without `SHADER_F16`, where the style's build
/// reports the error instead. The browser's WebGPU always takes them.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn style_capabilities() -> naga::valid::Capabilities {
    naga::valid::Capabilities::default() | naga::valid::Capabilities::SHADER_FLOAT16_IN_FLOAT32
}

/// The composed text under an error's primary span: for an unknown
/// identifier, naga's label covers the identifier itself.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn span_text(composed: &Composed, span: naga::Span) -> Option<&str> {
    composed.text.get(span.to_range()?)
}

/// The byte range of the user's code in the composed text.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn user_range(composed: &Composed) -> std::ops::Range<usize> {
    let prelude_end = composed
        .text
        .match_indices('\n')
        .nth(composed.prelude_lines.saturating_sub(1))
        .map_or(0, |(i, _)| i + 1);
    let user_end = composed
        .text
        .match_indices('\n')
        .nth(composed.prelude_lines + composed.user_lines - 1)
        .map_or(composed.text.len(), |(i, _)| i + 1);
    prelude_end..user_end
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn in_range(range: &std::ops::Range<usize>, span: naga::Span) -> bool {
    span.to_range()
        .is_some_and(|r| r.start >= range.start && r.start < range.end)
}

/// Give every loop in the functions `in_user` picks a counter of its own,
/// so none runs more than [`LOOP_LIMIT`] times (#411):
///
/// ```text
/// guard = 0u;                      // before the loop: every run starts at 0
/// loop {
///     ...body...
///     continuing {
///         ...its own continuing...
///         guard = guard + 1u;
///         break if (its own break_if) || guard >= LOOP_LIMIT;
///     }
/// }
/// ```
///
/// `continue` jumps to `continuing`, so every turn counts. A loop that
/// ends on its own within the limit runs exactly as written. A loop whose
/// turns the IR proves to be within it (`bounded`, #424) is left alone: the
/// guard would never fire, and the plain loop is one the driver can unroll.
/// Returns the number of loops guarded.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn guard_loops(
    module: &mut naga::Module,
    in_user: impl Fn(naga::Span) -> bool,
) -> usize {
    let u32_ty = module.types.insert(
        naga::Type {
            name: None,
            inner: naga::TypeInner::Scalar(naga::Scalar::U32),
        },
        naga::Span::UNDEFINED,
    );
    let mut guarded = 0;
    let handles: Vec<_> = module
        .functions
        .iter()
        .filter(|(h, _)| in_user(module.functions.get_span(*h)))
        .map(|(h, _)| h)
        .collect();
    for h in handles {
        let skip = bounded::bounded_loops(module, &module.functions[h], LOOP_LIMIT);
        let f = &mut module.functions[h];
        let mut body = std::mem::take(&mut f.body);
        guard_block(&mut body, f, u32_ty, &skip, &mut guarded);
        f.body = body;
    }
    guarded
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn guard_block(
    block: &mut naga::Block,
    f: &mut naga::Function,
    u32_ty: naga::Handle<naga::Type>,
    skip: &[naga::Span],
    count: &mut usize,
) {
    use naga::{BinaryOperator as B, Expression as E, Literal, Statement as S};
    let old = std::mem::take(block);
    for (mut statement, span) in old.span_into_iter() {
        match &mut statement {
            S::Loop {
                body,
                continuing,
                break_if,
            } => {
                guard_block(body, f, u32_ty, skip, count);
                guard_block(continuing, f, u32_ty, skip, count);
                if skip.contains(&span) {
                    block.push(statement, span);
                    continue;
                }
                let var = f.local_variables.append(
                    naga::LocalVariable {
                        name: Some(format!("flint_loop_guard_{count}")),
                        ty: u32_ty,
                        init: None,
                    },
                    span,
                );
                *count += 1;
                // Literals and the variable's pointer need no Emit
                let pointer = f.expressions.append(E::LocalVariable(var), span);
                let zero = f.expressions.append(E::Literal(Literal::U32(0)), span);
                let one = f.expressions.append(E::Literal(Literal::U32(1)), span);
                let limit = f
                    .expressions
                    .append(E::Literal(Literal::U32(LOOP_LIMIT)), span);
                block.push(
                    S::Store {
                        pointer,
                        value: zero,
                    },
                    span,
                );
                let load = f.expressions.append(E::Load { pointer }, span);
                let next = f.expressions.append(
                    E::Binary {
                        op: B::Add,
                        left: load,
                        right: one,
                    },
                    span,
                );
                continuing.push(S::Emit(naga::Range::new_from_bounds(load, next)), span);
                continuing.push(
                    S::Store {
                        pointer,
                        value: next,
                    },
                    span,
                );
                let done = f.expressions.append(
                    E::Binary {
                        op: B::GreaterEqual,
                        left: next,
                        right: limit,
                    },
                    span,
                );
                let last = match *break_if {
                    Some(own) => f.expressions.append(
                        E::Binary {
                            op: B::LogicalOr,
                            left: own,
                            right: done,
                        },
                        span,
                    ),
                    None => done,
                };
                continuing.push(S::Emit(naga::Range::new_from_bounds(done, last)), span);
                *break_if = Some(last);
            }
            S::Block(b) => guard_block(b, f, u32_ty, skip, count),
            S::If { accept, reject, .. } => {
                guard_block(accept, f, u32_ty, skip, count);
                guard_block(reject, f, u32_ty, skip, count);
            }
            S::Switch { cases, .. } => {
                for case in cases {
                    guard_block(&mut case.body, f, u32_ty, skip, count);
                }
            }
            _ => {}
        }
        block.push(statement, span);
    }
}

/// The identifiers an error is about, read from its structure rather than
/// its wording (#946): the source text under its span when that is one
/// identifier, and every identifier its message names, however the compiler
/// quotes them (naga 23 wrote `'x'`, naga 30 writes `` `x` ``, the browser's
/// compilers have their own).
pub(super) fn named<'a>(message: &'a str, subject: Option<&'a str>) -> Vec<&'a str> {
    let ident = |t: &&str| t.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_');
    let subject = subject
        .map(str::trim)
        .filter(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .filter(ident);
    subject
        .into_iter()
        .chain(
            message
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .filter(ident),
        )
        .collect()
}

/// `file:line:col: message`, with the line remapped into the user's file
/// and the offending line quoted; a hint when the user called a helper of
/// an input they didn't declare.
///
/// `at` is the composed module's 1-based (line, column) and `subject` the
/// composed text under the error's span: naga's natively, the browser
/// compiler's on the web (#415). The hints go by the identifiers these
/// name ([`named`]), not by the message's wording.
pub(super) fn locate(
    composed: &Composed,
    file: &str,
    at: Option<(u32, u32)>,
    subject: Option<&str>,
    message: &str,
    source: &StyleSource,
    pass: usize,
) -> String {
    let names = named(message, subject);
    let mut out = match at {
        Some((line_number, line_position)) => match composed.user_line(line_number as usize) {
            Some(line) => {
                let text = composed
                    .text
                    .lines()
                    .nth(line_number as usize - 1)
                    .unwrap_or("");
                format!(
                    "{file}:{line}:{line_position}: {message}\n    {line} | {}",
                    text.trim_end()
                )
            }
            None if names.contains(&"style") => format!(
                "{file}: no `fn style(p: StylePixel) -> vec4<f32>`; the style draws what it returns"
            ),
            None => format!(
                "{file}: {message} (in the generated code, line {line_number}; check the parameter names)"
            ),
        },
        None => format!("{file}: {message}"),
    };
    let missing = |helpers: &[&str]| {
        names
            .iter()
            .any(|n| helpers.iter().any(|h| n.starts_with(h)))
    };
    if !source.inputs.depth && missing(&["flint_depth", "flint_view_depth", "flint_world_pos"]) {
        out.push_str("\n    hint: add \"depth\" to inputs in the .shader.toml");
    }
    if !source.inputs.mask && missing(&["flint_mask_id", "flint_style_blend"]) {
        out.push_str("\n    hint: add \"mask\" to inputs in the .shader.toml");
    }
    if !source.inputs.edges && missing(&["flint_edges", "flint_edge"]) {
        out.push_str("\n    hint: add \"edges\" to inputs in the .shader.toml");
    }
    if missing(&["flint_image_", "flint_lut_"]) {
        let names: Vec<&str> = source.images.iter().map(|i| i.name.as_str()).collect();
        out.push_str(&format!(
            "\n    hint: an image is declared as [images.<name>] or an option with \
             kind = \"image\"; this style's images: {}",
            if names.is_empty() {
                "none".to_string()
            } else {
                names.join(", ")
            }
        ));
    }
    // A helper of a target this pass doesn't read (#412)
    if let Some(name) = names.iter().find_map(|n| n.strip_prefix("flint_target_")) {
        let name = ["_size", "_load"]
            .iter()
            .find_map(|s| name.strip_suffix(s))
            .unwrap_or(name);
        let reads = &source.passes[pass].reads;
        if !reads.iter().any(|r| source.targets[r.target].name == name) {
            let pass_name = &source.passes[pass].name;
            if source.targets.iter().any(|t| t.name == name) {
                out.push_str(&format!(
                    "\n    hint: add \"{name}\" to reads of pass \"{pass_name}\" in the .shader.toml"
                ));
            } else {
                out.push_str(&format!("\n    hint: no pass writes a target \"{name}\""));
            }
        }
    }
    out
}

/// The checks a parse doesn't make: the user defined `style`, declared no
/// bindings or entry points of their own, and how many loops and texture
/// reads their code has.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn inspect(
    module: &naga::Module,
    composed: &Composed,
    file: &str,
) -> Result<Report, String> {
    let user = user_range(composed);
    let in_user = |span: naga::Span| in_range(&user, span);
    let line_of = |span: naga::Span| {
        let at = span.location(&composed.text);
        composed
            .user_line(at.line_number as usize)
            .map_or(String::new(), |l| format!(":{l}"))
    };

    let style = module
        .functions
        .iter()
        .find(|(_, f)| f.name.as_deref() == Some("style"));
    let Some((handle, _)) = style else {
        return Err(format!(
            "{file}: no `fn style(p: StylePixel) -> vec4<f32>`; the style draws what it returns"
        ));
    };
    let _ = handle;
    for (h, var) in module.global_variables.iter() {
        if var.binding.is_some() && in_user(module.global_variables.get_span(h)) {
            return Err(format!(
                "{file}{}: `{}` declares a binding; a style reads only the prelude's \
                 (flint_input, params, depth, mask, edges, images, and the targets in reads)",
                line_of(module.global_variables.get_span(h)),
                var.name.as_deref().unwrap_or("?")
            ));
        }
    }
    if module.entry_points.len() != 2 {
        return Err(format!(
            "{file}: declares an entry point; the framework writes vs_main and fs_main"
        ));
    }

    let mut report = Report::default();
    for (h, f) in module.functions.iter() {
        if !in_user(module.functions.get_span(h)) {
            continue;
        }
        report.loops += count_loops(&f.body);
        report.bounded += bounded::bounded_loops(module, f, LOOP_LIMIT).len();
        report.texture_reads += f
            .expressions
            .iter()
            .filter(|(_, e)| {
                matches!(
                    e,
                    naga::Expression::ImageSample { .. } | naga::Expression::ImageLoad { .. }
                )
            })
            .count();
        // Calls to the sampling helpers count as reads too
        report.texture_reads += f
            .body
            .iter()
            .map(|s| count_helper_calls(s, module))
            .sum::<usize>();
    }
    if report.texture_reads > TEXTURE_READ_WARN {
        report.warnings.push(format!(
            "{file}: warning: {} texture reads written out; each runs once a pixel",
            report.texture_reads
        ));
    }
    // What the browser refuses and naga lets through (#573)
    for found in uniformity::derivatives_under_branches(module, &in_user) {
        report
            .warnings
            .push(derivative_warning(composed, file, &found));
    }
    Ok(report)
}

/// `file:line:col: warning: ...` for a derivative the browser will refuse
/// (#573), the line quoted as an error's is, with how to move it.
#[cfg(not(target_arch = "wasm32"))]
fn derivative_warning(composed: &Composed, file: &str, found: &uniformity::Finding) -> String {
    use uniformity::Why;
    let line_of = |span: naga::Span| {
        let at = span.location(&composed.text);
        (composed.user_line(at.line_number as usize), at)
    };
    let (line, at) = line_of(found.at);
    let text = composed
        .text
        .lines()
        .nth((at.line_number as usize).saturating_sub(1))
        .unwrap_or("")
        .trim_end();
    let what = found.what;
    let subject = match (&found.via, &found.through) {
        (Some((name, inner)), _) => match line_of(*inner).0 {
            Some(l) => format!("a call to `{name}`, which takes `{what}` at line {l},"),
            None => format!("a call to `{name}`, which takes `{what}`,"),
        },
        (None, Some((name, call))) => match line_of(*call).0 {
            Some(l) => format!("`{what}` in `{name}`, which is called at line {l}"),
            None => format!("`{what}` in `{name}`, which is called"),
        },
        (None, None) => format!("`{what}`"),
    };
    let place = match line_of(found.cause.span).0 {
        Some(l) => format!(" at line {l}"),
        None => String::new(),
    };
    let (where_, before) = match found.cause.why {
        Why::If => (
            format!("under the `if`{place}, whose condition can differ from pixel to pixel"),
            "the `if`",
        ),
        Why::Switch => (
            format!("under the `switch`{place}, whose selector can differ from pixel to pixel"),
            "the `switch`",
        ),
        Why::Loop => (
            format!("in the loop{place}, which pixels can leave on different turns"),
            "the loop",
        ),
        Why::Return => (
            format!("after the `return`{place}, which only some pixels take"),
            "the `return`",
        ),
    };
    let params = if found.cause.params {
        " (`params` counts: it is per pixel)"
    } else {
        ""
    };
    let taken = match (&found.via, &found.through) {
        (Some((name, _)), _) | (None, Some((name, _))) => format!("call `{name}`"),
        (None, None) => format!("take `{what}`"),
    };
    let at_text = match line {
        Some(line) => format!("{file}:{line}:{}", at.line_position),
        None => file.to_string(),
    };
    let mut out = format!(
        "{at_text}: warning: {subject} {where_}{params}: the browser's WebGPU refuses the \
         style (derivatives need uniform control flow), though it draws natively"
    );
    if let Some(line) = line {
        out.push_str(&format!("\n    {line} | {text}"));
    }
    out.push_str(&format!(
        "\n    hint: {taken} before {before} and use the result there, or put \
         @diagnostic(off, derivative_uniformity) on the function if a quad's pixels always agree"
    ));
    out
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn count_loops(block: &naga::Block) -> usize {
    use naga::Statement as S;
    block
        .iter()
        .map(|s| match s {
            S::Loop {
                body, continuing, ..
            } => 1 + count_loops(body) + count_loops(continuing),
            S::Block(b) => count_loops(b),
            S::If { accept, reject, .. } => count_loops(accept) + count_loops(reject),
            S::Switch { cases, .. } => cases.iter().map(|c| count_loops(&c.body)).sum(),
            _ => 0,
        })
        .sum()
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn count_helper_calls(statement: &naga::Statement, module: &naga::Module) -> usize {
    use naga::Statement as S;
    let block = |b: &naga::Block| {
        b.iter()
            .map(|s| count_helper_calls(s, module))
            .sum::<usize>()
    };
    match statement {
        S::Call { function, .. } => {
            let name = module.functions[*function].name.as_deref().unwrap_or("");
            if name.starts_with("flint_image_size_")
                || (name.starts_with("flint_target_") && name.ends_with("_size"))
            {
                return 0;
            }
            // A tiled read is four loads, a LUT two samples
            if name.starts_with("flint_image_tiled_") {
                return 4;
            }
            if name.starts_with("flint_lut_") {
                return 2;
            }
            usize::from(
                name.starts_with("flint_image_")
                    || name.starts_with("flint_target_")
                    || matches!(
                        name,
                        "flint_sample"
                            | "flint_depth"
                            | "flint_view_depth"
                            | "flint_world_pos"
                            | "flint_mask_id"
                            | "flint_style_blend"
                            | "flint_params_at"
                            | "flint_edges"
                            | "flint_edge"
                    ),
            )
        }
        S::Loop {
            body, continuing, ..
        } => block(body) + block(continuing),
        S::Block(b) => block(b),
        S::If { accept, reject, .. } => block(accept) + block(reject),
        S::Switch { cases, .. } => cases.iter().map(|c| block(&c.body)).sum(),
        _ => 0,
    }
}

/// On the web there is no naga in the player: wgpu builds naga's WGSL front
/// end for wasm, but only its `webgl` backend calls it, so the linker drops
/// it and checking here would add it to the download (#415). The browser's
/// own WGSL compiler reports instead, a frame or two later
/// ([`compiler_error`], read when the build settles).
///
/// The loop guard can't run here either, so a bundle carries its result:
/// `flint bundle` packs each pass's guarded module beside the style
/// (#550), and when one was written from the text this pass composes to,
/// it is what gets built ([`packed_guarded`]).
#[cfg(target_arch = "wasm32")]
pub fn check_pass(source: &StyleSource, pass: usize, user: &str) -> Result<Report, String> {
    Ok(Report {
        guarded: packed_guarded(source, pass, user),
        ..Report::default()
    })
}

/// The first error in a compiler's report on `composed` (the browser's
/// `getCompilationInfo()` on the web), as `file:line:col: message` with the
/// line in the user's file, as [`check`] gives natively (#415). `None`
/// when it reported none.
pub fn compiler_error(
    source: &StyleSource,
    composed: &Composed,
    info: &wgpu::CompilationInfo,
) -> Option<String> {
    compiler_error_pass(source, source.last_pass(), composed, info)
}

/// [`compiler_error`] for pass `pass` of `source` (#412).
pub fn compiler_error_pass(
    source: &StyleSource,
    pass: usize,
    composed: &Composed,
    info: &wgpu::CompilationInfo,
) -> Option<String> {
    let errors: Vec<_> = info
        .messages
        .iter()
        .filter(|m| m.message_type == wgpu::CompilationMessageType::Error)
        .collect();
    let first = errors.first()?;
    let file = shown(&source.passes[pass].wgsl_path);
    let at = first
        .location
        .map(|l| (l.line_number, l.line_position))
        .filter(|(line, _)| *line > 0);
    let subject = first.location.and_then(|l| {
        let start = l.offset as usize;
        composed.text.get(start..start + l.length as usize)
    });
    let mut out = locate(
        composed,
        &file,
        at,
        subject,
        first.message.trim(),
        source,
        pass,
    );
    if errors.len() > 1 {
        out.push_str(&format!("\n    (and {} more)", errors.len() - 1));
    }
    Some(out)
}
