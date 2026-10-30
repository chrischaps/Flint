//! Text L-systems: grammars written as strings, the way they are printed in
//! *The Algorithmic Beauty of Plants* (Prusinkiewicz & Lindenmayer).
//!
//! ```text
//! axiom:  X
//! rules:  X -> F+[[X]-X]-F[-FX]+X
//!         F -> FF
//! ```
//!
//! [`Grammar::expand`] rewrites the axiom; [`interpret`] walks the result with
//! a 3D turtle and returns [`BranchSegment`]s.
//!
//! # Rules
//!
//! A rule is `"<symbol> -> <successor>"`. Several rules may share a
//! predecessor, each with a weight: `"F (0.33) -> F[+F]F"`. When a symbol has
//! more than one rule, one is drawn per occurrence with probability
//! proportional to its weight; a symbol with a single rule never touches the
//! RNG, so deterministic grammars ignore the seed. `→` works as the arrow too,
//! and whitespace is ignored everywhere.
//!
//! A symbol written with arguments, `F(2,0.5)`, is still the symbol `F` to the
//! rewriter: a rule for `F` replaces it, arguments and all, and a symbol with
//! no rule is copied as written.
//!
//! # Turtle symbols
//!
//! | Symbol | Meaning | Argument |
//! |---|---|---|
//! | `F`, `G` | draw forward | `F(length)` or `F(length, radius)` |
//! | `f` | move forward without drawing | `f(length)` |
//! | `+` / `-` | yaw left / right | angle in degrees |
//! | `^` / `&` | pitch up / down | angle |
//! | `/` / `\` | roll clockwise / counter-clockwise | angle |
//! | `\|` | turn around (yaw 180°) | |
//! | `[` / `]` | push / pop the turtle | |
//! | `!` | thin the branch by `radius_decay` | `!(radius)` sets it |
//!
//! Any other symbol (`X`, `A`, `L`...) only drives rewriting; the turtle skips
//! it. Without arguments, a step is `step_length` long and the rotations use
//! the configured angles.
//!
//! Length and radius decay by **nesting level**: every `[` multiplies the
//! default step and the current radius by `length_decay` and `radius_decay`,
//! and `]` restores them. A branch's children are shorter and thinner than it
//! is however many rewriting passes produced them. Explicit arguments are
//! used as written.

use super::TurtleState;
use crate::rng::SeededRng;
use crate::types::BranchSegment;
use crate::{ProcGenError, Result};
use flint_core::spline::rotate_around_axis;
use flint_core::Vec3;
use std::collections::HashMap;

// ─── A. Tokens ──────────────────────────────────────────────────────────────

/// One symbol of an L-system string, with its arguments if it has any.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Token<'a> {
    symbol: char,
    /// The text between the parentheses, if the symbol was written `X(...)`.
    args: Option<&'a str>,
    /// The symbol exactly as written, arguments included.
    text: &'a str,
    /// Byte offset in the source string, for error messages.
    offset: usize,
}

/// Split an L-system string into symbols. Whitespace is skipped.
fn tokenize(s: &str) -> Result<Vec<Token<'_>>> {
    let mut tokens = Vec::with_capacity(s.len());
    let mut chars = s.char_indices().peekable();
    while let Some((offset, symbol)) = chars.next() {
        if symbol.is_whitespace() {
            continue;
        }
        if symbol == '(' || symbol == ')' {
            return Err(syntax_error(s, offset, "parenthesis without a symbol"));
        }
        let mut end = offset + symbol.len_utf8();
        let mut args = None;
        if let Some(&(open, '(')) = chars.peek() {
            let close = s[open..]
                .find(')')
                .map(|i| open + i)
                .ok_or_else(|| syntax_error(s, open, "unclosed '('"))?;
            args = Some(&s[open + 1..close]);
            end = close + 1;
            while chars.peek().is_some_and(|&(i, _)| i < end) {
                chars.next();
            }
        }
        tokens.push(Token {
            symbol,
            args,
            text: &s[offset..end],
            offset,
        });
    }
    Ok(tokens)
}

/// Parse a comma-separated argument list: `"1.5, 0.3"` -> `[1.5, 0.3]`.
fn parse_args(args: &str) -> std::result::Result<Vec<f32>, String> {
    args.split(',')
        .map(|a| {
            let a = a.trim();
            a.parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| format!("'{a}' is not a number"))
        })
        .collect()
}

fn syntax_error(s: &str, offset: usize, reason: &str) -> ProcGenError {
    ProcGenError::InvalidParameter {
        name: "lsystem".into(),
        reason: format!("{reason} at byte {offset} of \"{}\"", clip(s)),
    }
}

/// The string, shortened for an error message.
fn clip(s: &str) -> String {
    const MAX: usize = 60;
    if s.chars().count() <= MAX {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(MAX).collect::<String>())
    }
}

/// Tokenize `s`, parse every argument list, and check that brackets balance.
/// Returns the string with whitespace removed.
fn validate(s: &str, what: &str) -> Result<String> {
    let tokens = tokenize(s)?;
    let mut depth = 0i32;
    for t in &tokens {
        if let Some(args) = t.args {
            parse_args(args).map_err(|e| syntax_error(s, t.offset, &e))?;
        }
        match t.symbol {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth < 0 {
                    return Err(syntax_error(s, t.offset, "']' without a matching '['"));
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(ProcGenError::InvalidParameter {
            name: "lsystem".into(),
            reason: format!("{what} \"{}\" leaves {depth} '[' unclosed", clip(s)),
        });
    }
    Ok(tokens.iter().map(|t| t.text).collect())
}

// ─── B. Rules and Grammar ───────────────────────────────────────────────────

/// A production: every occurrence of `predecessor` is replaced by `successor`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductionRule {
    /// The symbol this rule rewrites.
    pub predecessor: char,
    /// What it becomes. Brackets balance; may be empty (the symbol is erased).
    pub successor: String,
    /// Relative weight among rules with the same predecessor (stochastic
    /// L-systems). Ignored when the predecessor has only one rule.
    pub weight: f32,
}

impl ProductionRule {
    /// A deterministic rule `predecessor -> successor`.
    pub fn new(predecessor: char, successor: &str) -> Result<Self> {
        Self::weighted(predecessor, successor, 1.0)
    }

    /// A rule that competes with others for the same predecessor.
    pub fn weighted(predecessor: char, successor: &str, weight: f32) -> Result<Self> {
        if predecessor.is_whitespace() || "[]()".contains(predecessor) {
            return Err(ProcGenError::InvalidParameter {
                name: "lsystem rule".into(),
                reason: format!("'{predecessor}' cannot be rewritten"),
            });
        }
        if !(weight.is_finite() && weight > 0.0) {
            return Err(ProcGenError::InvalidParameter {
                name: "lsystem rule".into(),
                reason: format!("weight {weight} for '{predecessor}' must be positive"),
            });
        }
        Ok(Self {
            predecessor,
            successor: validate(successor, "successor")?,
            weight,
        })
    }

    /// Parse `"F -> FF"` or, with a weight, `"F (0.33) -> F[+F]F"`.
    pub fn parse(rule: &str) -> Result<Self> {
        let bad = |reason: &str| ProcGenError::InvalidParameter {
            name: "lsystem rule".into(),
            reason: format!("{reason} in \"{}\"", clip(rule)),
        };
        let (lhs, successor) = rule
            .split_once("->")
            .or_else(|| rule.split_once('→'))
            .ok_or_else(|| bad("expected '->'"))?;
        let lhs = lhs.trim();
        let mut chars = lhs.chars();
        let predecessor = chars.next().ok_or_else(|| bad("missing predecessor"))?;
        let rest = chars.as_str().trim();
        let weight = if rest.is_empty() {
            1.0
        } else {
            rest.strip_prefix('(')
                .and_then(|r| r.strip_suffix(')'))
                .and_then(|w| w.trim().parse::<f32>().ok())
                .ok_or_else(|| bad("expected one symbol, optionally followed by a (weight)"))?
        };
        Self::weighted(predecessor, successor, weight)
    }
}

/// A text L-system: an axiom and the rules that rewrite it.
#[derive(Debug, Clone, PartialEq)]
pub struct Grammar {
    /// The starting string.
    pub axiom: String,
    /// Productions, in the order they were added.
    pub rules: Vec<ProductionRule>,
    /// Budget guard: expansion fails once the string grows past this many
    /// bytes. Rewriting is exponential; this stops a typo from eating memory.
    pub max_length: usize,
}

impl Grammar {
    /// Default for [`Grammar::max_length`]: 4 MB of symbols.
    pub const DEFAULT_MAX_LENGTH: usize = 4 << 20;

    /// A grammar with no rules yet.
    pub fn new(axiom: &str) -> Result<Self> {
        Ok(Self {
            axiom: validate(axiom, "axiom")?,
            rules: Vec::new(),
            max_length: Self::DEFAULT_MAX_LENGTH,
        })
    }

    /// Parse an axiom and rules written as `"F -> FF"` strings.
    pub fn parse<S: AsRef<str>>(axiom: &str, rules: &[S]) -> Result<Self> {
        let mut grammar = Self::new(axiom)?;
        for rule in rules {
            grammar.rules.push(ProductionRule::parse(rule.as_ref())?);
        }
        Ok(grammar)
    }

    /// Add a rule.
    pub fn rule(mut self, rule: ProductionRule) -> Self {
        self.rules.push(rule);
        self
    }

    /// Rewrite the axiom `iterations` times.
    pub fn expand(&self, iterations: usize, rng: &mut SeededRng) -> Result<String> {
        // Per predecessor: its successors with their running weight totals.
        let mut choices: HashMap<char, Vec<(f32, &str)>> = HashMap::new();
        for r in &self.rules {
            let list = choices.entry(r.predecessor).or_default();
            let total = list.last().map_or(0.0, |&(t, _)| t);
            list.push((total + r.weight, &r.successor));
        }

        let mut current = self.axiom.clone();
        for _ in 0..iterations {
            let mut next = String::with_capacity(current.len() * 2);
            for token in tokenize(&current)? {
                let replacement = match choices.get(&token.symbol).map(Vec::as_slice) {
                    None => token.text,
                    Some([(_, only)]) => only,
                    Some(list) => {
                        let total = list.last().map_or(0.0, |&(t, _)| t);
                        let pick = rng.next_f32() * total;
                        list.iter()
                            .find(|&&(t, _)| pick < t)
                            .or(list.last())
                            .map_or(token.text, |&(_, s)| s)
                    }
                };
                next.push_str(replacement);
                if next.len() > self.max_length {
                    return Err(ProcGenError::BudgetExceeded(format!(
                        "L-system string passed max_length {} bytes",
                        self.max_length
                    )));
                }
            }
            current = next;
        }
        Ok(current)
    }

    /// Expand and interpret in one call.
    pub fn generate(
        &self,
        iterations: usize,
        config: &TurtleConfig,
        rng: &mut SeededRng,
    ) -> Result<Vec<BranchSegment>> {
        interpret(&self.expand(iterations, rng)?, config)
    }
}

// ─── C. Turtle ──────────────────────────────────────────────────────────────

/// How the turtle reads an expanded string.
#[derive(Debug, Clone, PartialEq)]
pub struct TurtleConfig {
    /// Length of a bare `F` at nesting level 0.
    pub step_length: f32,
    /// Branch radius at nesting level 0.
    pub radius: f32,
    /// Step length multiplier per `[`.
    pub length_decay: f32,
    /// Radius multiplier per `[` (and per `!`).
    pub radius_decay: f32,
    /// Default angle for `+` and `-`, in degrees.
    pub yaw_deg: f32,
    /// Default angle for `^` and `&`, in degrees.
    pub pitch_deg: f32,
    /// Default angle for `/` and `\`, in degrees.
    pub roll_deg: f32,
    /// Direction branches bend toward after each drawn step: down for
    /// gravity, up for phototropism. Ignored while `tropism_strength` is 0.
    pub tropism: Vec3,
    /// How strongly the heading bends toward `tropism`: each step turns it
    /// by `strength * |heading x tropism|` radians, as in ABOP §2.2.
    pub tropism_strength: f32,
    /// Budget guard on emitted segments.
    pub max_segments: u32,
}

impl Default for TurtleConfig {
    fn default() -> Self {
        Self {
            step_length: 1.0,
            radius: 0.1,
            length_decay: 1.0,
            radius_decay: 1.0,
            yaw_deg: 25.0,
            pitch_deg: 25.0,
            roll_deg: 25.0,
            tropism: Vec3::new(0.0, -1.0, 0.0),
            tropism_strength: 0.0,
            max_segments: 100_000,
        }
    }
}

impl TurtleConfig {
    /// The same angle on all three rotation axes.
    pub fn with_angle(mut self, degrees: f32) -> Self {
        self.yaw_deg = degrees;
        self.pitch_deg = degrees;
        self.roll_deg = degrees;
        self
    }
}

/// Turtle state plus the per-branch scale that `[` and `]` save and restore.
#[derive(Clone)]
struct Frame {
    turtle: TurtleState,
    step: f32,
    radius: f32,
}

/// Walk an expanded L-system string with a 3D turtle.
///
/// The turtle starts at the origin heading +Y. A `]` with no matching `[`
/// is an error; a `[` left open at the end is not.
pub fn interpret(expanded: &str, config: &TurtleConfig) -> Result<Vec<BranchSegment>> {
    let positive = |v: f32| v.is_finite() && v > 0.0;
    if !positive(config.length_decay) || !positive(config.radius_decay) {
        return Err(ProcGenError::InvalidParameter {
            name: "TurtleConfig".into(),
            reason: "length_decay and radius_decay must be positive".into(),
        });
    }

    let tropism = config.tropism.normalized();
    let mut segments = Vec::new();
    let mut frame = Frame {
        turtle: TurtleState::default(),
        step: config.step_length,
        radius: config.radius,
    };
    let mut stack: Vec<Frame> = Vec::new();

    for token in tokenize(expanded)? {
        let args = match token.args {
            Some(a) => parse_args(a).map_err(|e| syntax_error(expanded, token.offset, &e))?,
            None => Vec::new(),
        };
        let arg = |i: usize, default: f32| args.get(i).copied().unwrap_or(default);
        let t = &mut frame.turtle;
        match token.symbol {
            'F' | 'G' => {
                let length = arg(0, frame.step);
                let radius = arg(1, frame.radius);
                let end = t.ahead(length);
                segments.push(BranchSegment {
                    start: t.position,
                    end,
                    radius_start: radius,
                    radius_end: radius,
                    depth: t.depth,
                });
                if segments.len() > config.max_segments as usize {
                    return Err(ProcGenError::BudgetExceeded(format!(
                        "segment count {} exceeds max_segments {}",
                        segments.len(),
                        config.max_segments
                    )));
                }
                t.position = end;
                if config.tropism_strength != 0.0 {
                    bend_toward(t, tropism, config.tropism_strength);
                }
            }
            'f' => t.position = t.ahead(arg(0, frame.step)),
            '+' => t.yaw(arg(0, config.yaw_deg)),
            '-' => t.yaw(-arg(0, config.yaw_deg)),
            '^' => t.pitch(arg(0, config.pitch_deg)),
            '&' => t.pitch(-arg(0, config.pitch_deg)),
            '/' => t.roll(arg(0, config.roll_deg)),
            '\\' => t.roll(-arg(0, config.roll_deg)),
            '|' => t.yaw(180.0),
            '!' => frame.radius = arg(0, frame.radius * config.radius_decay),
            '[' => {
                stack.push(frame.clone());
                frame.turtle.depth += 1;
                frame.step *= config.length_decay;
                frame.radius *= config.radius_decay;
            }
            ']' => {
                frame = stack.pop().ok_or_else(|| {
                    syntax_error(expanded, token.offset, "']' without a matching '['")
                })?;
            }
            _ => {}
        }
    }

    Ok(segments)
}

/// Turn the turtle's frame toward `direction` by `strength * |H x T|`.
fn bend_toward(t: &mut TurtleState, direction: Vec3, strength: f32) {
    let axis = t.heading.cross(&direction);
    let sin = axis.length();
    if sin < 1e-6 {
        return;
    }
    let axis = axis * (1.0 / sin);
    let angle = strength * sin;
    t.heading = rotate_around_axis(t.heading, axis, angle);
    t.up = rotate_around_axis(t.up, axis, angle);
    t.right = rotate_around_axis(t.right, axis, angle);
}

// ─── D. Presets ─────────────────────────────────────────────────────────────

/// A named grammar from the literature, with the settings it was drawn with.
#[derive(Debug, Clone, Copy)]
pub struct Preset {
    pub name: &'static str,
    pub axiom: &'static str,
    pub rules: &'static [&'static str],
    pub iterations: usize,
    /// Branching angle, degrees, for all three axes.
    pub angle: f32,
}

impl Preset {
    /// The preset's grammar.
    pub fn grammar(&self) -> Grammar {
        Grammar::parse(self.axiom, self.rules).expect("built-in presets parse")
    }
}

/// Classic plants, most from *The Algorithmic Beauty of Plants* ch. 1.
pub const PRESETS: &[Preset] = &[
    // ABOP fig. 1.24(f): the fractal plant.
    Preset {
        name: "fractal_plant",
        axiom: "X",
        rules: &["X -> F+[[X]-X]-F[-FX]+X", "F -> FF"],
        iterations: 5,
        angle: 22.5,
    },
    // ABOP fig. 1.24(d): a symmetric weed.
    Preset {
        name: "weed",
        axiom: "X",
        rules: &["X -> F[+X]F[-X]+X", "F -> FF"],
        iterations: 6,
        angle: 20.0,
    },
    // ABOP fig. 1.27: a stochastic plant; every seed grows a different one.
    Preset {
        name: "stochastic_plant",
        axiom: "F",
        rules: &[
            "F (0.33) -> F[+F]F[-F]F",
            "F (0.33) -> F[+F]F",
            "F (0.34) -> F[-F]F",
        ],
        iterations: 5,
        angle: 25.7,
    },
    // A 3D bush after ABOP fig. 1.25, leaves left out: three branches per
    // node, rolled 137.5° apart (the golden angle) so no two line up.
    Preset {
        name: "bush",
        axiom: "A",
        rules: &[
            "A -> F[&FA]/(137.5)[&FA]/(137.5)[&FA]",
            "F -> S/(137.5)F",
            "S -> F",
        ],
        iterations: 6,
        angle: 28.0,
    },
];

/// Look up a preset by name.
pub fn preset(name: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.name == name)
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-4
    }

    fn koch() -> Grammar {
        Grammar::parse("F", &["F -> F[+F]F[-F]F"]).unwrap()
    }

    // ── Expansion ───────────────────────────────────────────────────────

    #[test]
    fn expansion_snapshot() {
        let g = koch();
        let mut rng = SeededRng::new(1);
        assert_eq!(g.expand(0, &mut rng).unwrap(), "F");
        assert_eq!(g.expand(1, &mut rng).unwrap(), "F[+F]F[-F]F");
        // Every F becomes 11 symbols; the 6 others are copied:
        // 5 * 11 + 6 = 61.
        let two = g.expand(2, &mut rng).unwrap();
        assert_eq!(two.len(), 61);
        assert!(two.starts_with("F[+F]F[-F]F[+F[+F]F[-F]F]"));
    }

    #[test]
    fn segment_count_matches_grammar() {
        // Five F per F per pass: 5^n segments.
        let g = koch();
        let config = TurtleConfig::default();
        for n in 0..=4 {
            let segs = g.generate(n, &config, &mut SeededRng::new(1)).unwrap();
            assert_eq!(segs.len(), 5usize.pow(n as u32), "after {n} passes");
        }
    }

    #[test]
    fn non_drawing_symbols_rewrite_but_do_not_draw() {
        // ABOP 1.24(f): X steers growth but is never drawn.
        let p = preset("fractal_plant").unwrap();
        let expanded = p.grammar().expand(1, &mut SeededRng::new(1)).unwrap();
        assert_eq!(expanded, "F+[[X]-X]-F[-FX]+X");
        let segs = interpret(&expanded, &TurtleConfig::default()).unwrap();
        assert_eq!(segs.len(), 3);
    }

    #[test]
    fn whitespace_is_ignored() {
        let g = Grammar::parse("F", &["F  ->  F [ + F ] F"]).unwrap();
        assert_eq!(g.rules[0].successor, "F[+F]F");
    }

    #[test]
    fn empty_successor_erases() {
        let g = Grammar::parse("FXF", &["X ->"]).unwrap();
        assert_eq!(g.expand(1, &mut SeededRng::new(1)).unwrap(), "FF");
    }

    #[test]
    fn parametric_symbols_survive_and_rewrite_whole() {
        let g = Grammar::parse("F(2,0.5)A(1)", &["A -> [+F]"]).unwrap();
        assert_eq!(g.expand(1, &mut SeededRng::new(1)).unwrap(), "F(2,0.5)[+F]");
        let g = Grammar::parse("F(2,0.5)", &["F -> FF"]).unwrap();
        assert_eq!(g.expand(1, &mut SeededRng::new(1)).unwrap(), "FF");
    }

    #[test]
    fn budget_guard_stops_runaway_growth() {
        let mut g = Grammar::parse("F", &["F -> FFFFFFFFFF"]).unwrap();
        g.max_length = 1000;
        let err = g.expand(5, &mut SeededRng::new(1)).unwrap_err();
        assert!(matches!(err, ProcGenError::BudgetExceeded(_)), "{err:?}");
    }

    // ── Stochastic rules ────────────────────────────────────────────────

    #[test]
    fn stochastic_is_deterministic_per_seed() {
        let g = preset("stochastic_plant").unwrap().grammar();
        let config = TurtleConfig::default().with_angle(25.7);
        let a = g.generate(4, &config, &mut SeededRng::new(7)).unwrap();
        let b = g.generate(4, &config, &mut SeededRng::new(7)).unwrap();
        assert_eq!(a, b);
        let c = g.generate(4, &config, &mut SeededRng::new(8)).unwrap();
        assert_ne!(a, c, "another seed should grow another plant");
    }

    #[test]
    fn stochastic_weights_are_respected() {
        let g = Grammar::parse("XXXXXXXXXX", &["X (1) -> a", "X (3) -> b"]).unwrap();
        let mut rng = SeededRng::new(3);
        let (mut a, mut b) = (0, 0);
        for _ in 0..400 {
            for c in g.expand(1, &mut rng).unwrap().chars() {
                if c == 'a' {
                    a += 1
                } else {
                    b += 1
                }
            }
        }
        let share = b as f32 / (a + b) as f32;
        assert!((share - 0.75).abs() < 0.03, "b share {share}");
    }

    #[test]
    fn deterministic_rules_leave_the_rng_alone() {
        let mut rng = SeededRng::new(5);
        koch().expand(3, &mut rng).unwrap();
        assert_eq!(rng.next_f32(), SeededRng::new(5).next_f32());
    }

    // ── Rule parsing ────────────────────────────────────────────────────

    #[test]
    fn rule_parsing() {
        let r = ProductionRule::parse("F (0.33) -> F[+F]F").unwrap();
        assert_eq!(r.predecessor, 'F');
        assert_eq!(r.successor, "F[+F]F");
        assert!((r.weight - 0.33).abs() < 1e-6);
        let r = ProductionRule::parse("X → F-X").unwrap();
        assert_eq!((r.predecessor, r.weight), ('X', 1.0));

        for bad in [
            "F FF",          // no arrow
            "-> FF",         // no predecessor
            "FG -> F",       // two predecessors
            "F (x) -> F",    // weight not a number
            "F (0) -> F",    // weight not positive
            "[ -> F",        // brackets aren't rewritable
            "F -> F[+F",     // unbalanced successor
            "F -> F]",       // ditto
            "F -> F(1,",     // unclosed argument list
            "F -> F(1,two)", // bad argument
        ] {
            assert!(ProductionRule::parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    // ── Turtle ──────────────────────────────────────────────────────────

    #[test]
    fn straight_line_is_collinear() {
        let segs = interpret("FFF", &TurtleConfig::default()).unwrap();
        assert_eq!(segs.len(), 3);
        for (i, s) in segs.iter().enumerate() {
            assert!(approx(s.start, Vec3::new(0.0, i as f32, 0.0)));
            assert!(approx(s.end, Vec3::new(0.0, i as f32 + 1.0, 0.0)));
        }
    }

    #[test]
    fn pop_restores_position_and_orientation() {
        // The branch in brackets turns and pitches; after ']' the trunk
        // carries on straight up from where it left off.
        let segs = interpret("F[+^/FF]F", &TurtleConfig::default()).unwrap();
        assert_eq!(segs.len(), 4);
        assert!(!approx(segs[1].direction(), Vec3::UP));
        assert!(approx(segs[3].start, segs[0].end));
        assert!(approx(segs[3].direction(), Vec3::UP));
    }

    #[test]
    fn depth_follows_brackets() {
        let segs = interpret("F[F[F]F]F", &TurtleConfig::default()).unwrap();
        let depths: Vec<u32> = segs.iter().map(|s| s.depth).collect();
        assert_eq!(depths, [0, 1, 2, 1, 0]);
    }

    #[test]
    fn parametric_forward() {
        let segs = interpret("F(2.0, 0.5)", &TurtleConfig::default()).unwrap();
        assert_eq!(segs.len(), 1);
        assert!((segs[0].length() - 2.0).abs() < 1e-6);
        assert_eq!((segs[0].radius_start, segs[0].radius_end), (0.5, 0.5));

        let segs = interpret("F(1.5,0.3)", &TurtleConfig::default()).unwrap();
        assert!((segs[0].length() - 1.5).abs() < 1e-6);
        assert!((segs[0].radius_start - 0.3).abs() < 1e-6);
    }

    #[test]
    fn parametric_turns_and_moves() {
        let segs = interpret("f(3)+(90)F", &TurtleConfig::default()).unwrap();
        assert!(approx(segs[0].start, Vec3::new(0.0, 3.0, 0.0)));
        assert!(approx(segs[0].end, Vec3::new(1.0, 3.0, 0.0)));
    }

    #[test]
    fn rotations_use_their_own_angles() {
        let config = TurtleConfig {
            yaw_deg: 90.0,
            pitch_deg: 45.0,
            roll_deg: 0.0,
            ..Default::default()
        };
        let yaw = interpret("+F", &config).unwrap()[0].direction();
        assert!(approx(yaw, Vec3::new(1.0, 0.0, 0.0)), "{yaw:?}");
        let pitch = interpret("^F", &config).unwrap()[0].direction();
        assert!(
            (pitch.y - 45f32.to_radians().cos()).abs() < 1e-5,
            "{pitch:?}"
        );
        let turned = interpret("|F", &config).unwrap()[0].direction();
        assert!(approx(turned, Vec3::new(0.0, -1.0, 0.0)), "{turned:?}");
    }

    #[test]
    fn decay_per_nesting_level() {
        let config = TurtleConfig {
            step_length: 2.0,
            radius: 0.4,
            length_decay: 0.5,
            radius_decay: 0.5,
            ..Default::default()
        };
        let segs = interpret("F[F[F]]F!F", &config).unwrap();
        let lengths: Vec<f32> = segs.iter().map(|s| s.length()).collect();
        let radii: Vec<f32> = segs.iter().map(|s| s.radius_start).collect();
        assert_eq!(lengths, [2.0, 1.0, 0.5, 2.0, 2.0]);
        assert_eq!(radii, [0.4, 0.2, 0.1, 0.4, 0.2]);
    }

    #[test]
    fn tropism_bends_branches_down() {
        let config = TurtleConfig {
            tropism_strength: 0.3,
            ..Default::default()
        };
        let segs = interpret("+(90)FFFF", &config).unwrap();
        assert!(segs
            .windows(2)
            .all(|w| w[1].direction().y < w[0].direction().y));
        assert!(segs.iter().all(|s| (s.length() - 1.0).abs() < 1e-5));
        // Straight up or down there is nothing to bend.
        let up = interpret("FF", &config).unwrap();
        assert!(approx(up[1].end, Vec3::new(0.0, 2.0, 0.0)));
    }

    #[test]
    fn unmatched_pop_is_an_error() {
        let err = interpret("F]F", &TurtleConfig::default()).unwrap_err();
        assert!(err.to_string().contains("without a matching"), "{err}");
        // An unclosed '[' is fine: the string simply ends inside a branch.
        assert_eq!(interpret("F[F", &TurtleConfig::default()).unwrap().len(), 2);
    }

    #[test]
    fn segment_budget() {
        let config = TurtleConfig {
            max_segments: 3,
            ..Default::default()
        };
        assert!(matches!(
            interpret("FFFF", &config),
            Err(ProcGenError::BudgetExceeded(_))
        ));
    }

    #[test]
    fn presets_parse_and_grow() {
        for p in PRESETS {
            let config = TurtleConfig::default().with_angle(p.angle);
            let segs = p
                .grammar()
                .generate(p.iterations, &config, &mut SeededRng::new(1))
                .unwrap();
            assert!(
                segs.len() > 50,
                "{} grew only {} segments",
                p.name,
                segs.len()
            );
        }
    }
}
