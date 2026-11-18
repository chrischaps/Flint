//! Derivatives under a branch on the pixel (#573): what the browser's
//! WebGPU refuses and naga lets through.
//!
//! WGSL asks that `dpdx`, `dpdy`, `fwidth` (and their `Coarse`/`Fine`
//! forms) and the samples that take derivatives of their own
//! (`textureSample`, `textureSampleBias`, `textureSampleCompare`) run in
//! uniform control flow: every pixel of a quad takes the same path to them.
//! naga 30 doesn't check it; Chrome's Tint does, and refuses the whole
//! module (`'fwidth' must only be called from uniform control flow`). So a
//! style that branches on a pixel's value and takes a derivative inside the
//! branch draws natively and fails on the web, after the game has shipped.
//!
//! This is a small version of WGSL's uniformity analysis over naga's IR,
//! from the fragment entry point down through every call:
//!
//! - **Values.** A literal, a constant, an override, a read of a
//!   `var<uniform>`, a `var<storage, read>` or a texture's size is uniform;
//!   an operation on uniform values is too. A function's argument is what
//!   its call site passed; the entry point's (the pixel) are not. A
//!   `var<private>` is not (WGSL counts it as able to differ per pixel),
//!   and that includes the prelude's `params`: a branch on a parameter
//!   counts as a branch on the pixel, as it does in the browser. A local
//!   `var` is uniform while every value stored into it is, in uniform
//!   control flow, so a `for` with a constant bound is a uniform loop.
//! - **Control flow** stops being uniform under an `if` or `switch` on a
//!   value that isn't, inside a loop that some pixels leave on different
//!   turns (not after it: there they are together again, as Chrome 157
//!   reads it), and after a `return` only some pixels take. A call made
//!   there runs its whole callee there.
//! - A function marked `@diagnostic(off, derivative_uniformity)` (or with
//!   the rule at `info` or `warning`) isn't flagged, nor are calls to it:
//!   the browser accepts those (the helpers in #558 rely on it).
//!
//! It is conservative where the browser may be cleverer (a value returned
//! from a function counts as uniform only when its arguments are and the
//! callee reads nothing that isn't; image loads count as per-pixel), so a
//! style it flags may yet compile there. Not modelled: a callee writing
//! through a pointer argument (the caller's local is taken as unchanged).
//! `uniformity_tests::CHROME_CASES` holds it to what Chrome 157 said of 17
//! modules, line and column included.

use naga::{
    AddressSpace, Block, DerivativeAxis, DerivativeControl, Expression, Function, Handle, Module,
    SampleLevel, ShaderStage, Span, Statement,
};
use std::collections::{BTreeSet, HashMap};

/// What made control flow stop being uniform, and where.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Why {
    /// An `if` on a value that isn't uniform.
    If,
    /// A `switch` on a value that isn't uniform.
    Switch,
    /// Inside a loop some pixels leave on a different turn.
    Loop,
    /// After a `return` only some pixels take.
    Return,
}

/// [`Why`], at the statement (in the composed text) that did it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Cause {
    pub why: Why,
    pub span: Span,
    /// The condition reads the prelude's `params`, which a reader may
    /// take for uniform: it is a `var<private>`.
    pub params: bool,
}

// A memo key (naga's `Span` is neither `Eq` nor `Hash`)
impl Eq for Cause {}

impl std::hash::Hash for Cause {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.why.hash(state);
        self.span.to_range().hash(state);
        self.params.hash(state);
    }
}

/// A derivative the browser will refuse.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Finding {
    /// The derivative, or the user's call that reaches one.
    pub at: Span,
    /// The builtin: `fwidth`, `dpdxFine`, `textureSample`, ...
    pub what: &'static str,
    /// When `at` is a call: the function called, and where in it the
    /// derivative is (in the composed text).
    pub via: Option<(String, Span)>,
    /// When `at` is in a function that is called from control flow that
    /// isn't uniform, and is uniform in it: that function, and the call
    /// (the outermost one) that put it there.
    pub through: Option<(String, Span)>,
    pub cause: Cause,
}

/// Every derivative taken in control flow that isn't uniform, reported at
/// its place in the code `in_user` picks (a derivative in the framework's
/// code is reported at the user's call that reaches it). In the order of
/// the text.
pub(super) fn derivatives_under_branches(
    module: &Module,
    in_user: &dyn Fn(Span) -> bool,
) -> Vec<Finding> {
    let mut an = Analyzer {
        module,
        in_user,
        memo: HashMap::new(),
        reached: BTreeSet::new(),
    };
    let mut found = Vec::new();
    for (i, ep) in module.entry_points.iter().enumerate() {
        if ep.stage != ShaderStage::Fragment {
            continue;
        }
        let args = vec![false; ep.function.arguments.len()];
        found.extend(an.walk_fn(Func::Entry(i), args, None).1);
    }
    // A function nothing calls is still compiled: what it does with its
    // own values counts, its arguments are whatever a caller would pass
    let unreached: Vec<_> = module
        .functions
        .iter()
        .filter(|(h, _)| !an.reached.contains(h) && in_user(module.functions.get_span(*h)))
        .map(|(h, f)| (h, f.arguments.len()))
        .collect();
    for (h, n) in unreached {
        found.extend(an.walk_fn(Func::Fn(h), vec![true; n], None).1);
    }
    let mut seen = BTreeSet::new();
    found.retain(|f| in_user(f.at) && seen.insert((f.at.to_range().map(|r| r.start), f.what)));
    found.sort_by_key(|f| f.at.to_range().map(|r| r.start));
    found
}

/// The name of the builtin `e` is when it takes derivatives.
pub(super) fn derivative_name(e: &Expression) -> Option<&'static str> {
    match e {
        Expression::Derivative { axis, ctrl, .. } => Some(match (axis, ctrl) {
            (DerivativeAxis::X, DerivativeControl::None) => "dpdx",
            (DerivativeAxis::X, DerivativeControl::Coarse) => "dpdxCoarse",
            (DerivativeAxis::X, DerivativeControl::Fine) => "dpdxFine",
            (DerivativeAxis::Y, DerivativeControl::None) => "dpdy",
            (DerivativeAxis::Y, DerivativeControl::Coarse) => "dpdyCoarse",
            (DerivativeAxis::Y, DerivativeControl::Fine) => "dpdyFine",
            (DerivativeAxis::Width, DerivativeControl::None) => "fwidth",
            (DerivativeAxis::Width, DerivativeControl::Coarse) => "fwidthCoarse",
            (DerivativeAxis::Width, DerivativeControl::Fine) => "fwidthFine",
        }),
        Expression::ImageSample {
            gather: None,
            level,
            depth_ref,
            ..
        } => match (level, depth_ref) {
            (SampleLevel::Auto, None) => Some("textureSample"),
            (SampleLevel::Auto, Some(_)) => Some("textureSampleCompare"),
            (SampleLevel::Bias(_), _) => Some("textureSampleBias"),
            _ => None,
        },
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Func {
    Fn(Handle<Function>),
    Entry(usize),
}

/// A function walked: with which arguments uniform, from what flow.
type Call = (Func, Vec<bool>, Option<Cause>);

/// What a walk found: whether what the function returns may differ per
/// pixel, and its findings.
type Walked = (bool, Vec<Finding>);

struct Analyzer<'m> {
    module: &'m Module,
    in_user: &'m dyn Fn(Span) -> bool,
    memo: HashMap<Call, Walked>,
    reached: BTreeSet<Handle<Function>>,
}

impl<'m> Analyzer<'m> {
    fn function(&self, f: Func) -> &'m Function {
        match f {
            Func::Fn(h) => &self.module.functions[h],
            Func::Entry(i) => &self.module.entry_points[i].function,
        }
    }

    /// Walk `func` called with `args` (true: uniform) from control flow
    /// `cf` (`None`: uniform).
    fn walk_fn(&mut self, func: Func, args: Vec<bool>, cf: Option<Cause>) -> Walked {
        let key = (func, args, cf);
        if let Some(done) = self.memo.get(&key) {
            return done.clone();
        }
        if let Func::Fn(h) = func {
            self.reached.insert(h);
        }
        let f = self.function(func);
        let mut walk = FnWalk {
            module: self.module,
            f,
            args: &key.1,
            reports: reports_derivatives(self.module, f),
            locals: vec![false; f.local_variables.len()],
            values: HashMap::new(),
            calls: HashMap::new(),
            returned: None,
            frames: Vec::new(),
            result: false,
            findings: Vec::new(),
            changed: false,
        };
        // Until no local learns it may differ per pixel (each pass can
        // only add to them, so this ends)
        for _ in 0..=f.local_variables.len() + 1 {
            walk.values.clear();
            walk.returned = None;
            walk.frames.clear();
            walk.result = false;
            walk.findings.clear();
            walk.changed = false;
            walk.block(self, &f.body, cf);
            if !walk.changed {
                break;
            }
        }
        let done = (walk.result, walk.findings);
        self.memo.insert(key, done.clone());
        done
    }
}

/// Whether `f` reports a derivative in control flow that isn't uniform:
/// unless `@diagnostic(<off|info|warning>, derivative_uniformity)` is on
/// it (or on the module), when the browser takes it.
fn reports_derivatives(module: &Module, f: &Function) -> bool {
    use naga::diagnostic_filter::{
        FilterableTriggeringRule, Severity, StandardFilterableTriggeringRule,
    };
    for leaf in [f.diagnostic_filter_leaf, module.diagnostic_filter_leaf] {
        let mut node = leaf;
        while let Some(h) = node {
            let n = &module.diagnostic_filters[h];
            if n.inner.triggering_rule
                == FilterableTriggeringRule::Standard(
                    StandardFilterableTriggeringRule::DerivativeUniformity,
                )
            {
                return n.inner.new_severity == Severity::Error;
            }
            node = n.parent;
        }
    }
    true
}

enum Frame {
    Loop { span: Span, left: Option<Cause> },
    Switch,
}

struct FnWalk<'m, 'a> {
    module: &'m Module,
    f: &'m Function,
    args: &'a [bool],
    reports: bool,
    /// Locals that may differ per pixel.
    locals: Vec<bool>,
    /// This pass's answers: may the expression differ per pixel?
    values: HashMap<Handle<Expression>, bool>,
    /// A call's result expression -> whether it may differ per pixel.
    calls: HashMap<Handle<Expression>, bool>,
    /// A `return` some pixels took and some didn't.
    returned: Option<Cause>,
    frames: Vec<Frame>,
    /// Whether what the function returns may differ per pixel.
    result: bool,
    findings: Vec<Finding>,
    changed: bool,
}

impl FnWalk<'_, '_> {
    /// Control flow here, given the block's own: also not uniform after a
    /// non-uniform `return`, or in a loop some pixels leave early.
    fn here(&self, cf: Option<Cause>) -> Option<Cause> {
        cf.or(self.returned).or_else(|| {
            self.frames.iter().find_map(|f| match f {
                Frame::Loop { left, .. } => *left,
                Frame::Switch => None,
            })
        })
    }

    /// Walk `block` from control flow `cf`.
    fn block(&mut self, an: &mut Analyzer, block: &Block, cf: Option<Cause>) {
        for (statement, span) in block.span_iter() {
            let here = self.here(cf);
            match statement {
                Statement::Emit(range) => {
                    for h in range.clone() {
                        if let (Some(what), Some(cause)) =
                            (derivative_name(&self.f.expressions[h]), here)
                        {
                            if self.reports {
                                self.findings.push(Finding {
                                    at: self.f.expressions.get_span(h),
                                    what,
                                    via: None,
                                    through: None,
                                    cause,
                                });
                            }
                        }
                    }
                }
                Statement::Block(b) => self.block(an, b, here),
                Statement::If {
                    condition,
                    accept,
                    reject,
                } => {
                    let inner = here.or_else(|| {
                        self.varies(*condition).then(|| Cause {
                            why: Why::If,
                            span: *span,
                            params: self.reads_params(*condition),
                        })
                    });
                    self.block(an, accept, inner);
                    self.block(an, reject, inner);
                }
                Statement::Switch { selector, cases } => {
                    let inner = here.or_else(|| {
                        self.varies(*selector).then(|| Cause {
                            why: Why::Switch,
                            span: *span,
                            params: self.reads_params(*selector),
                        })
                    });
                    self.frames.push(Frame::Switch);
                    for case in cases {
                        self.block(an, &case.body, inner);
                    }
                    self.frames.pop();
                }
                Statement::Loop {
                    body,
                    continuing,
                    break_if,
                } => {
                    self.frames.push(Frame::Loop {
                        span: *span,
                        left: None,
                    });
                    // Once some pixels leave on a different turn, every
                    // turn after (the whole body, read again) is non-uniform
                    loop {
                        let before = self.left();
                        self.block(an, body, self.here(here));
                        self.block(an, continuing, self.here(here));
                        if let Some(b) = break_if {
                            if self.here(here).is_some() || self.varies(*b) {
                                self.leave(*span);
                            }
                        }
                        if self.left() == before {
                            break;
                        }
                    }
                    // Past the loop the pixels are together again: Chrome
                    // 157 takes a derivative after a loop some leave early
                    self.frames.pop();
                }
                Statement::Break => match self.frames.last() {
                    Some(Frame::Loop { span, .. }) if here.is_some() => {
                        let span = *span;
                        self.leave(span);
                    }
                    _ => {}
                },
                Statement::Continue => {
                    let span = self.frames.iter().rev().find_map(|f| match f {
                        Frame::Loop { span, .. } => Some(*span),
                        Frame::Switch => None,
                    });
                    if let (Some(span), Some(_)) = (span, here) {
                        self.leave(span);
                    }
                }
                Statement::Return { value } => {
                    if here.is_some() {
                        self.result = true;
                        self.returned.get_or_insert(Cause {
                            why: Why::Return,
                            span: *span,
                            params: false,
                        });
                    }
                    if let Some(v) = value {
                        self.result |= self.varies(*v);
                    }
                }
                Statement::Store { pointer, value } => {
                    if let Some(local) = self.local_root(*pointer) {
                        let varies =
                            here.is_some() || self.varies(*value) || self.pointer_varies(*pointer);
                        if varies && !self.locals[local] {
                            self.locals[local] = true;
                            self.changed = true;
                        }
                    }
                }
                Statement::Call {
                    function,
                    arguments,
                    result,
                } => {
                    let args: Vec<bool> = arguments.iter().map(|a| !self.varies(*a)).collect();
                    let (varies, found) = an.walk_fn(Func::Fn(*function), args, here);
                    let callee = &self.module.functions[*function];
                    for mut finding in found {
                        // A derivative out of the user's sight is told at
                        // the user's call that reaches it
                        if !(an.in_user)(finding.at) && (an.in_user)(*span) {
                            finding.via = Some((
                                callee.name.clone().unwrap_or_default(),
                                finding.via.map_or(finding.at, |(_, at)| at),
                            ));
                            finding.at = *span;
                        } else if finding.via.is_none() && here == Some(finding.cause) {
                            // Uniform in the callee, not where it is called:
                            // set again at each caller up, so the outermost
                            // call (the one under the branch) is kept
                            finding.through =
                                Some((callee.name.clone().unwrap_or_default(), *span));
                        }
                        if !self.findings.contains(&finding) {
                            self.findings.push(finding);
                        }
                    }
                    if let Some(r) = result {
                        self.calls.insert(*r, varies);
                    }
                }
                _ => {}
            }
        }
    }

    fn left(&self) -> Option<Cause> {
        self.frames.iter().rev().find_map(|f| match f {
            Frame::Loop { left, .. } => Some(*left),
            Frame::Switch => None,
        })?
    }

    /// Some pixels leave the innermost loop (the one at `span`) early.
    fn leave(&mut self, span: Span) {
        for f in self.frames.iter_mut().rev() {
            if let Frame::Loop { span: s, left } = f {
                if *s == span && left.is_none() {
                    *left = Some(Cause {
                        why: Why::Loop,
                        span,
                        params: false,
                    });
                }
                return;
            }
        }
    }

    /// Whether `h` reads the prelude's private `params`.
    fn reads_params(&self, h: Handle<Expression>) -> bool {
        let mut stack = vec![h];
        let mut seen = BTreeSet::new();
        while let Some(h) = stack.pop() {
            if !seen.insert(h) {
                continue;
            }
            let e = &self.f.expressions[h];
            if let Expression::GlobalVariable(g) = e {
                let g = &self.module.global_variables[*g];
                if g.space == AddressSpace::Private && g.name.as_deref() == Some("params") {
                    return true;
                }
            }
            stack.extend(operands(e));
        }
        false
    }

    /// The local `pointer` points into, if it does.
    fn local_root(&self, pointer: Handle<Expression>) -> Option<usize> {
        match &self.f.expressions[pointer] {
            Expression::LocalVariable(l) => Some(l.index()),
            Expression::Access { base, .. } | Expression::AccessIndex { base, .. } => {
                self.local_root(*base)
            }
            _ => None,
        }
    }

    /// Whether what `pointer` points at may differ per pixel.
    fn pointer_varies(&mut self, pointer: Handle<Expression>) -> bool {
        match &self.f.expressions[pointer] {
            Expression::Access { base, index } => {
                let (base, index) = (*base, *index);
                self.pointer_varies(base) || self.varies(index)
            }
            Expression::AccessIndex { base, .. } => self.pointer_varies(*base),
            Expression::LocalVariable(l) => self.locals[l.index()],
            Expression::GlobalVariable(g) => match self.module.global_variables[*g].space {
                AddressSpace::Uniform | AddressSpace::Handle | AddressSpace::Immediate => false,
                AddressSpace::Storage { access } => access.contains(naga::StorageAccess::STORE),
                _ => true,
            },
            Expression::FunctionArgument(i) => {
                !self.args.get(*i as usize).copied().unwrap_or(false)
            }
            _ => true,
        }
    }

    /// Whether the value of `h` may differ per pixel.
    fn varies(&mut self, h: Handle<Expression>) -> bool {
        if let Some(v) = self.values.get(&h) {
            return *v;
        }
        let v = match &self.f.expressions[h] {
            Expression::Literal(_)
            | Expression::Constant(_)
            | Expression::Override(_)
            | Expression::ZeroValue(_)
            | Expression::GlobalVariable(_)
            | Expression::LocalVariable(_) => false,
            Expression::FunctionArgument(i) => {
                !self.args.get(*i as usize).copied().unwrap_or(false)
            }
            Expression::Load { pointer } => self.pointer_varies(*pointer),
            Expression::Compose { components, .. } => {
                let c = components.clone();
                c.into_iter().any(|e| self.varies(e))
            }
            Expression::Access { base, index } => {
                let (b, i) = (*base, *index);
                self.varies(b) || self.varies(i)
            }
            Expression::AccessIndex { base: e, .. }
            | Expression::Splat { value: e, .. }
            | Expression::Swizzle { vector: e, .. }
            | Expression::Unary { expr: e, .. }
            | Expression::As { expr: e, .. }
            | Expression::Relational { argument: e, .. }
            | Expression::ArrayLength(e) => {
                let e = *e;
                self.varies(e)
            }
            Expression::Binary { left, right, .. } => {
                let (l, r) = (*left, *right);
                self.varies(l) || self.varies(r)
            }
            Expression::Select {
                condition,
                accept,
                reject,
            } => {
                let (c, a, r) = (*condition, *accept, *reject);
                self.varies(c) || self.varies(a) || self.varies(r)
            }
            Expression::Math {
                arg,
                arg1,
                arg2,
                arg3,
                ..
            } => {
                let all = [Some(*arg), *arg1, *arg2, *arg3];
                all.into_iter().flatten().any(|e| self.varies(e))
            }
            Expression::ImageQuery { image, query } => {
                let level = match query {
                    naga::ImageQuery::Size { level } => *level,
                    _ => None,
                };
                let image = *image;
                self.varies(image) || level.is_some_and(|l| self.varies(l))
            }
            Expression::CallResult(_) => self.calls.get(&h).copied().unwrap_or(true),
            // Texture reads, derivatives and the rest: per pixel
            _ => true,
        };
        self.values.insert(h, v);
        v
    }
}

/// The expressions `e` reads (for [`FnWalk::reads_params`]).
fn operands(e: &Expression) -> Vec<Handle<Expression>> {
    match e {
        Expression::Compose { components, .. } => components.clone(),
        Expression::Access { base, index } => vec![*base, *index],
        Expression::AccessIndex { base: e, .. }
        | Expression::Splat { value: e, .. }
        | Expression::Swizzle { vector: e, .. }
        | Expression::Unary { expr: e, .. }
        | Expression::As { expr: e, .. }
        | Expression::Relational { argument: e, .. }
        | Expression::ArrayLength(e)
        | Expression::Load { pointer: e } => vec![*e],
        Expression::Binary { left, right, .. } => vec![*left, *right],
        Expression::Select {
            condition,
            accept,
            reject,
        } => vec![*condition, *accept, *reject],
        Expression::Math {
            arg,
            arg1,
            arg2,
            arg3,
            ..
        } => [Some(*arg), *arg1, *arg2, *arg3]
            .into_iter()
            .flatten()
            .collect(),
        _ => Vec::new(),
    }
}
