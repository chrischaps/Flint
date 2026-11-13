//! Loops whose number of turns the IR proves (#424).
//!
//! The commonest loop a style has is a `for` over octaves or kernel taps:
//!
//! ```wgsl
//! for (var i = 0; i < 3; i++) { ... }
//! ```
//!
//! naga lowers it to
//!
//! ```text
//! i = 0;                                   // or the variable's own init
//! loop {
//!     if (i < 3) { } else { break; }       // the test, first in the body
//!     { ...body... }
//!     continuing { i = i + 1; }            // the step
//! }
//! ```
//!
//! When the start, the bound and the step are constants, nothing else
//! writes the counter, and running the counter forward ends the loop within
//! [`LOOP_LIMIT`](super::LOOP_LIMIT) turns, the loop can't hang the GPU, so
//! it needs neither the guard (#411) nor a warning. The turns are counted
//! by running the counter in its own type, wrapping as the GPU does, so
//! `i >= 0u` counting down (which wraps and never ends) is not bounded.
//! `loop { ... continuing { i++; break if i >= n; } }` is recognised too.
//!
//! Anything else stays guarded: `loop {}`, a bound read from a texture or a
//! parameter, a counter the body writes or hands to a function by pointer.

use naga::{BinaryOperator as B, Expression as E, Statement as S};

/// The loops in `f` whose turns are proven to be at most `limit`, by span.
pub(super) fn bounded_loops(
    module: &naga::Module,
    f: &naga::Function,
    limit: u32,
) -> Vec<naga::Span> {
    let mut out = Vec::new();
    walk(module, f, &f.body, limit, &mut out);
    out
}

fn walk(
    module: &naga::Module,
    f: &naga::Function,
    block: &naga::Block,
    limit: u32,
    out: &mut Vec<naga::Span>,
) {
    for (i, (statement, span)) in block.span_iter().enumerate() {
        match statement {
            S::Loop {
                body, continuing, ..
            } => {
                if *span != naga::Span::UNDEFINED
                    && turns(module, f, &block[..i], statement, limit).is_some()
                {
                    out.push(*span);
                }
                walk(module, f, body, limit, out);
                walk(module, f, continuing, limit, out);
            }
            S::Block(b) => walk(module, f, b, limit, out),
            S::If { accept, reject, .. } => {
                walk(module, f, accept, limit, out);
                walk(module, f, reject, limit, out);
            }
            S::Switch { cases, .. } => {
                for case in cases {
                    walk(module, f, &case.body, limit, out);
                }
            }
            _ => {}
        }
    }
}

/// An integer as WGSL holds it: arithmetic wraps, comparisons are its type's.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Int {
    I32(i32),
    U32(u32),
}

impl Int {
    fn zero_like(self) -> Int {
        match self {
            Int::I32(_) => Int::I32(0),
            Int::U32(_) => Int::U32(0),
        }
    }

    fn step(self, op: B, by: Int) -> Option<Int> {
        Some(match (self, op, by) {
            (Int::I32(a), B::Add, Int::I32(b)) => Int::I32(a.wrapping_add(b)),
            (Int::I32(a), B::Subtract, Int::I32(b)) => Int::I32(a.wrapping_sub(b)),
            (Int::U32(a), B::Add, Int::U32(b)) => Int::U32(a.wrapping_add(b)),
            (Int::U32(a), B::Subtract, Int::U32(b)) => Int::U32(a.wrapping_sub(b)),
            _ => return None,
        })
    }

    fn compare(self, op: B, other: Int) -> Option<bool> {
        let order = match (self, other) {
            (Int::I32(a), Int::I32(b)) => a.cmp(&b),
            (Int::U32(a), Int::U32(b)) => a.cmp(&b),
            _ => return None,
        };
        Some(match op {
            B::Less => order.is_lt(),
            B::LessEqual => order.is_le(),
            B::Greater => order.is_gt(),
            B::GreaterEqual => order.is_ge(),
            B::Equal => order.is_eq(),
            B::NotEqual => order.is_ne(),
            _ => return None,
        })
    }
}

fn literal(l: naga::Literal) -> Option<Int> {
    match l {
        naga::Literal::I32(v) => Some(Int::I32(v)),
        naga::Literal::U32(v) => Some(Int::U32(v)),
        _ => None,
    }
}

/// `h` as a constant integer: a literal (naga folds `-6`, `2 * 3` and a
/// `let` of them to one) or a module `const`.
fn constant(module: &naga::Module, f: &naga::Function, h: naga::Handle<E>) -> Option<Int> {
    match f.expressions[h] {
        E::Literal(l) => literal(l),
        E::Constant(c) => match module.global_expressions[module.constants[c].init] {
            E::Literal(l) => literal(l),
            _ => None,
        },
        _ => None,
    }
}

fn is_var(
    f: &naga::Function,
    pointer: naga::Handle<E>,
    v: naga::Handle<naga::LocalVariable>,
) -> bool {
    matches!(f.expressions[pointer], E::LocalVariable(x) if x == v)
}

/// The local variable `h` loads, if it is a plain load of one.
fn loaded(f: &naga::Function, h: naga::Handle<E>) -> Option<naga::Handle<naga::LocalVariable>> {
    match f.expressions[h] {
        E::Load { pointer } => match f.expressions[pointer] {
            E::LocalVariable(v) => Some(v),
            _ => None,
        },
        _ => None,
    }
}

/// A comparison of a counter with a constant.
struct Test {
    counter: naga::Handle<naga::LocalVariable>,
    op: B,
    bound: Int,
    counter_left: bool,
}

impl Test {
    fn of(module: &naga::Module, f: &naga::Function, h: naga::Handle<E>) -> Option<Test> {
        let E::Binary { op, left, right } = f.expressions[h] else {
            return None;
        };
        if let (Some(counter), Some(bound)) = (loaded(f, left), constant(module, f, right)) {
            return Some(Test {
                counter,
                op,
                bound,
                counter_left: true,
            });
        }
        let (Some(bound), Some(counter)) = (constant(module, f, left), loaded(f, right)) else {
            return None;
        };
        Some(Test {
            counter,
            op,
            bound,
            counter_left: false,
        })
    }

    fn holds(&self, i: Int) -> Option<bool> {
        if self.counter_left {
            i.compare(self.op, self.bound)
        } else {
            self.bound.compare(self.op, i)
        }
    }
}

/// How often `block` writes `v`: stores to it, and calls handed its pointer.
fn writes(f: &naga::Function, block: &naga::Block, v: naga::Handle<naga::LocalVariable>) -> usize {
    block
        .iter()
        .map(|s| match s {
            S::Store { pointer, .. } => usize::from(is_var(f, *pointer, v)),
            S::Call { arguments, .. } => arguments.iter().filter(|a| is_var(f, **a, v)).count(),
            S::Block(b) => writes(f, b, v),
            S::If { accept, reject, .. } => writes(f, accept, v) + writes(f, reject, v),
            S::Switch { cases, .. } => cases.iter().map(|c| writes(f, &c.body, v)).sum(),
            S::Loop {
                body, continuing, ..
            } => writes(f, body, v) + writes(f, continuing, v),
            _ => 0,
        })
        .sum()
}

fn not_emit(s: &&S) -> bool {
    !matches!(s, S::Emit(_))
}

/// The turns `the_loop` makes on every run, if its shape proves them and
/// they are at most `limit`. `before` is what precedes it in its block.
fn turns(
    module: &naga::Module,
    f: &naga::Function,
    before: &[S],
    the_loop: &S,
    limit: u32,
) -> Option<u32> {
    let S::Loop {
        body,
        continuing,
        break_if,
    } = the_loop
    else {
        return None;
    };
    // The test: `break if` after the step, or an `if` that breaks first in
    // the body (a `for`'s), before it. `stay` is the test's value that runs
    // another turn.
    let (test, stay, test_first) = match *break_if {
        Some(c) => (Test::of(module, f, c)?, false, false),
        None => match body.iter().find(not_emit)? {
            S::If {
                condition,
                accept,
                reject,
            } => {
                let breaks = |b: &naga::Block| matches!(b.iter().as_slice(), [S::Break]);
                let stay = if accept.is_empty() && breaks(reject) {
                    true
                } else if breaks(accept) && reject.is_empty() {
                    false
                } else {
                    return None;
                };
                (Test::of(module, f, *condition)?, stay, true)
            }
            _ => return None,
        },
    };
    let v = test.counter;

    // The step: `continuing` is one store to the counter of itself plus or
    // minus a constant, and the body never writes it
    let [S::Store { pointer, value }] = continuing.iter().filter(not_emit).collect::<Vec<_>>()[..]
    else {
        return None;
    };
    if !is_var(f, *pointer, v) || writes(f, body, v) != 0 {
        return None;
    }
    let (op, by) = match f.expressions[*value] {
        E::Binary { op, left, right } if loaded(f, left) == Some(v) => {
            (op, constant(module, f, right)?)
        }
        E::Binary {
            op: B::Add,
            left,
            right,
        } if loaded(f, right) == Some(v) => (B::Add, constant(module, f, left)?),
        _ => return None,
    };

    // The start: a constant stored just before the loop, or the variable's
    // own (or zero) init when only the step ever writes it. A run that
    // begins where an earlier one stopped is further along the same path,
    // so it makes no more turns than the first.
    let start = match before.iter().rev().find(not_emit) {
        Some(S::Store { pointer, value }) if is_var(f, *pointer, v) => constant(module, f, *value)?,
        _ if writes(f, &f.body, v) == 1 => match f.local_variables[v].init {
            Some(init) => constant(module, f, init)?,
            None => test.bound.zero_like(),
        },
        _ => return None,
    };

    // Run the counter forward, in its own type
    let mut i = start;
    let mut turns = 0;
    loop {
        if test_first && test.holds(i)? != stay {
            return Some(turns);
        }
        if turns == limit {
            return None;
        }
        turns += 1;
        i = i.step(op, by)?;
        if !test_first && test.holds(i)? != stay {
            return Some(turns);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The turns of each loop in `fn f`, in the order they start in the
    /// source, `None` where none are proven.
    fn turns_of(body: &str) -> Vec<Option<u32>> {
        let src = format!("const N: i32 = 5;\nfn f() -> f32 {{\n    var acc = 0.0;\n{body}\n    return acc;\n}}\n");
        let module = naga::front::wgsl::parse_str(&src).unwrap_or_else(|e| panic!("{e}\n{src}"));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{e:?}\n{src}"));
        let (_, f) = module.functions.iter().next().unwrap();
        let mut found = Vec::new();
        collect(&module, f, &f.body, &mut found);
        found.sort_by_key(|(start, _)| *start);
        found.into_iter().map(|(_, t)| t).collect()
    }

    fn collect(
        module: &naga::Module,
        f: &naga::Function,
        block: &naga::Block,
        out: &mut Vec<(usize, Option<u32>)>,
    ) {
        for (i, (s, span)) in block.span_iter().enumerate() {
            match s {
                S::Loop {
                    body, continuing, ..
                } => {
                    let start = span.to_range().map_or(0, |r| r.start);
                    out.push((start, turns(module, f, &block[..i], s, 4096)));
                    collect(module, f, body, out);
                    collect(module, f, continuing, out);
                }
                S::Block(b) => collect(module, f, b, out),
                S::If { accept, reject, .. } => {
                    collect(module, f, accept, out);
                    collect(module, f, reject, out);
                }
                _ => {}
            }
        }
    }

    #[test]
    fn for_loops_with_constant_bounds_are_counted() {
        assert_eq!(
            turns_of("for (var i = 0; i < 3; i++) { acc += 1.0; }"),
            [Some(3)]
        );
        assert_eq!(
            turns_of("for (var i = -6; i <= 6; i++) { acc += 1.0; }"),
            [Some(13)]
        );
        assert_eq!(
            turns_of("for (var i = 10u; i > 0u; i -= 2u) { acc += 1.0; }"),
            [Some(5)]
        );
        assert_eq!(
            turns_of("for (var i = 0; i < N; i += 2) { acc += 1.0; }"),
            [Some(3)]
        );
        assert_eq!(
            turns_of("for (var i = 0; 4 > i; i = 1 + i) { acc += 1.0; }"),
            [Some(4)]
        );
        assert_eq!(
            turns_of("for (var i = 0; i != 6; i += 2) { acc += 1.0; }"),
            [Some(3)]
        );
        assert_eq!(
            turns_of("for (var i = 7; i < 3; i++) { acc += 1.0; }"),
            [Some(0)]
        );
        // A kernel: both loops, the inner one's start stored each outer turn
        assert_eq!(
            turns_of(
                "for (var y = -4; y <= 4; y++) {\n  for (var x = -4; x <= 4; x++) { acc += 1.0; }\n}"
            ),
            [Some(9), Some(9)]
        );
        // `while` over a counter declared before it
        // A `while` steps its counter in the body, which this doesn't follow
        assert_eq!(turns_of("var k = 0;\nwhile (k < 8) { k++; }"), [None]);
    }

    #[test]
    fn break_if_after_the_step_is_counted() {
        assert_eq!(
            turns_of(
                "var i = 0;\nloop {\n  acc += 1.0;\n  continuing { i++; break if i >= 4; }\n}"
            ),
            [Some(4)]
        );
        // `break if` sees the stepped counter: 0 -> 1 ends it after one turn
        assert_eq!(
            turns_of("var i = 0;\nloop {\n  acc += 1.0;\n  continuing { i++; break if i > 0; }\n}"),
            [Some(1)]
        );
    }

    #[test]
    fn anything_else_is_not_proven() {
        // No test at all, or a test of something that isn't a counter
        assert_eq!(turns_of("loop { acc += 1.0; }"), [None]);
        assert_eq!(
            turns_of("loop { if acc > 1.0 { break; } acc += 0.1; }"),
            [None]
        );
        // The body writes the counter, or hands it to a function by pointer
        assert_eq!(turns_of("for (var i = 0; i < 3; i++) { i = 0; }"), [None]);
        // Never ends: wrong way, no step, unsigned wrap, or past the limit
        assert_eq!(
            turns_of("for (var i = 0; i < 3; i--) { acc += 1.0; }"),
            [None]
        );
        assert_eq!(
            turns_of("for (var i = 0; i < 3; i += 0) { acc += 1.0; }"),
            [None]
        );
        assert_eq!(
            turns_of("for (var i = 3u; i >= 0u; i--) { acc += 1.0; }"),
            [None]
        );
        assert_eq!(
            turns_of("for (var i = 0; i < 5000; i++) { acc += 1.0; }"),
            [None]
        );
        assert_eq!(
            turns_of("for (var i = 0; i != 5; i += 2) { acc += 1.0; }"),
            [None]
        );
        // A bound that isn't a constant
        assert_eq!(
            turns_of("let n = i32(acc);\nfor (var i = 0; i < n; i++) { acc += 1.0; }"),
            [None]
        );
        // A start that isn't one: written before the loop by something else
        assert_eq!(
            turns_of("var i = 0;\ni = i32(acc);\nfor (; i < 3; i++) { acc += 1.0; }"),
            [None]
        );
    }

    #[test]
    fn a_pointer_handed_to_a_function_is_a_write() {
        let src = "fn reset(p: ptr<function, i32>) { *p = 0; }\n\
                   fn f() -> f32 {\n    var acc = 0.0;\n    \
                   for (var i = 0; i < 3; i++) { reset(&i); acc += 1.0; }\n    return acc;\n}\n";
        let module = naga::front::wgsl::parse_str(src).unwrap();
        let f = module
            .functions
            .iter()
            .find(|(_, f)| f.name.as_deref() == Some("f"))
            .unwrap()
            .1;
        assert!(bounded_loops(&module, f, 4096).is_empty());
    }
}
