//! Parametric L-system engine for branching structures.
//!
//! Produces `Vec<BranchSegment>` from a symbol-rewriting grammar interpreted
//! by a 3D turtle. Both deterministic and stochastic rules are supported.
//!
//! # Architecture
//!
//! 1. **Symbols** — parameterized alphabet (`Forward`, `YawLeft`, `Push`, …)
//! 2. **Rules** — closures that rewrite symbols; first match wins
//! 3. **Expansion** — iterative string rewriting up to `max_depth` iterations
//! 4. **Turtle** — walks the expanded symbol string, emitting `BranchSegment`s
//!
//! The same engine also reads grammars written as text, the way they appear
//! in *The Algorithmic Beauty of Plants*: an axiom like `"X"` and rules like
//! `"X -> F[+X][-X]FX"`. See [`grammar`] for that notation.

pub mod grammar;

pub use grammar::{interpret, Grammar, ProductionRule, TurtleConfig};

use crate::rng::SeededRng;
use crate::types::BranchSegment;
use crate::{ProcGenError, Result};
use flint_core::spline::rotate_around_axis;
use flint_core::Vec3;

// ─── A. Symbol Enum ──────────────────────────────────────────────────────────

/// A single symbol in the parametric L-system alphabet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Symbol {
    /// Draw a branch segment of the given length and radius.
    Forward { length: f32, radius: f32 },
    /// Advance the turtle without drawing.
    Move { length: f32 },
    /// Rotate left around the up axis.
    YawLeft { angle: f32 },
    /// Rotate right around the up axis.
    YawRight { angle: f32 },
    /// Pitch the turtle upward (nose up).
    PitchUp { angle: f32 },
    /// Pitch the turtle downward (nose down).
    PitchDown { angle: f32 },
    /// Roll clockwise around the heading axis.
    RollCw { angle: f32 },
    /// Roll counter-clockwise around the heading axis.
    RollCcw { angle: f32 },
    /// Save turtle state onto the stack.
    Push,
    /// Restore turtle state from the stack.
    Pop,
    /// Extensibility hook — ignored by the turtle interpreter.
    Custom { id: char, param: f32 },
}

// ─── B. LSystemParams + Builder + LSystem ────────────────────────────────────

/// Parameters controlling L-system expansion and interpretation.
#[derive(Debug, Clone)]
pub struct LSystemParams {
    /// Default angle (degrees) used by convenience constructors.
    pub default_angle_deg: f32,
    /// Multiplicative decay applied to segment length each depth level.
    pub length_decay: f32,
    /// Multiplicative decay applied to segment radius each depth level.
    pub radius_decay: f32,
    /// Number of rewriting iterations.
    pub max_depth: u32,
    /// Hard limit on emitted segments (budget guard).
    pub max_segments: u32,
}

impl Default for LSystemParams {
    fn default() -> Self {
        Self {
            default_angle_deg: 25.0,
            length_decay: 0.7,
            radius_decay: 0.7,
            max_depth: 4,
            max_segments: 10_000,
        }
    }
}

/// A rewriting rule: given a symbol and RNG, optionally produce a replacement.
type Rule = Box<dyn Fn(&Symbol, &mut SeededRng) -> Option<Vec<Symbol>> + Send + Sync>;

/// Builder for constructing an `LSystem`.
pub struct LSystemBuilder {
    axiom: Vec<Symbol>,
    params: LSystemParams,
    rules: Vec<Rule>,
}

impl LSystemBuilder {
    /// Start building an L-system from the given axiom and parameters.
    pub fn new(axiom: Vec<Symbol>, params: LSystemParams) -> Self {
        Self {
            axiom,
            params,
            rules: Vec::new(),
        }
    }

    /// Add a rewriting rule. Rules are tested in order; first match wins.
    pub fn rule<F>(mut self, f: F) -> Self
    where
        F: Fn(&Symbol, &mut SeededRng) -> Option<Vec<Symbol>> + Send + Sync + 'static,
    {
        self.rules.push(Box::new(f));
        self
    }

    /// Validate parameters and produce the final `LSystem`.
    pub fn build(self) -> Result<LSystem> {
        if self.params.max_depth < 1 {
            return Err(ProcGenError::InvalidParameter {
                name: "max_depth".into(),
                reason: "must be >= 1".into(),
            });
        }
        if self.params.length_decay <= 0.0 || self.params.length_decay > 1.0 {
            return Err(ProcGenError::InvalidParameter {
                name: "length_decay".into(),
                reason: "must be in (0.0, 1.0]".into(),
            });
        }
        if self.params.radius_decay <= 0.0 || self.params.radius_decay > 1.0 {
            return Err(ProcGenError::InvalidParameter {
                name: "radius_decay".into(),
                reason: "must be in (0.0, 1.0]".into(),
            });
        }
        Ok(LSystem {
            axiom: self.axiom,
            params: self.params,
            rules: self.rules,
        })
    }
}

/// A compiled L-system ready for expansion and interpretation.
pub struct LSystem {
    axiom: Vec<Symbol>,
    params: LSystemParams,
    rules: Vec<Rule>,
}

impl LSystem {
    /// Iteratively expand the axiom through `max_depth` rewriting passes.
    pub fn expand(&self, rng: &mut SeededRng) -> Vec<Symbol> {
        let mut current = self.axiom.clone();
        for _ in 0..self.params.max_depth {
            let mut next = Vec::with_capacity(current.len() * 2);
            for sym in &current {
                let mut matched = false;
                for rule in &self.rules {
                    if let Some(replacement) = rule(sym, rng) {
                        next.extend(replacement);
                        matched = true;
                        break;
                    }
                }
                if !matched {
                    next.push(*sym);
                }
            }
            current = next;
        }
        current
    }

    /// Expand the axiom and interpret the result with a 3D turtle,
    /// producing branch segments.
    pub fn generate_segments(&self, rng: &mut SeededRng) -> Result<Vec<BranchSegment>> {
        let symbols = self.expand(rng);
        interpret_turtle(&symbols, &self.params)
    }
}

// ─── C. Turtle Interpreter ───────────────────────────────────────────────────

/// Turtle state for walking a symbol string: a position and an orthonormal
/// frame (heading, up, right), plus the bracket nesting depth.
///
/// Shared by [`interpret_turtle`] and the text-grammar interpreter in
/// [`grammar`].
#[derive(Debug, Clone)]
pub(crate) struct TurtleState {
    pub(crate) position: Vec3,
    pub(crate) heading: Vec3,
    pub(crate) up: Vec3,
    pub(crate) right: Vec3,
    pub(crate) depth: u32,
}

impl Default for TurtleState {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            heading: Vec3::new(0.0, 1.0, 0.0), // +Y up
            up: Vec3::new(0.0, 0.0, -1.0),     // -Z forward in screen space
            right: Vec3::new(1.0, 0.0, 0.0),   // +X right
            depth: 0,
        }
    }
}

impl TurtleState {
    /// The point `length` along the heading.
    pub(crate) fn ahead(&self, length: f32) -> Vec3 {
        self.position + self.heading * length
    }

    /// Rotate around the up axis; positive turns left.
    pub(crate) fn yaw(&mut self, degrees: f32) {
        let rad = degrees.to_radians();
        self.heading = rotate_around_axis(self.heading, self.up, rad);
        self.right = rotate_around_axis(self.right, self.up, rad);
    }

    /// Rotate around the right axis; positive pitches the nose up.
    pub(crate) fn pitch(&mut self, degrees: f32) {
        let rad = degrees.to_radians();
        self.heading = rotate_around_axis(self.heading, self.right, rad);
        self.up = rotate_around_axis(self.up, self.right, rad);
    }

    /// Rotate around the heading axis; positive rolls clockwise.
    pub(crate) fn roll(&mut self, degrees: f32) {
        let rad = degrees.to_radians();
        self.up = rotate_around_axis(self.up, self.heading, rad);
        self.right = rotate_around_axis(self.right, self.heading, rad);
    }
}

/// Interpret expanded symbols with a 3D turtle, emitting `BranchSegment`s.
fn interpret_turtle(symbols: &[Symbol], params: &LSystemParams) -> Result<Vec<BranchSegment>> {
    let mut segments = Vec::new();
    let mut state = TurtleState::default();
    let mut stack: Vec<TurtleState> = Vec::new();

    for sym in symbols {
        match *sym {
            Symbol::Forward { length, radius } => {
                let end = state.ahead(length);
                let radius_end = radius * params.radius_decay;
                segments.push(BranchSegment {
                    start: state.position,
                    end,
                    radius_start: radius,
                    radius_end,
                    depth: state.depth,
                });
                if segments.len() > params.max_segments as usize {
                    return Err(ProcGenError::BudgetExceeded(format!(
                        "segment count {} exceeds max_segments {}",
                        segments.len(),
                        params.max_segments
                    )));
                }
                state.position = end;
            }
            Symbol::Move { length } => state.position = state.ahead(length),
            Symbol::YawLeft { angle } => state.yaw(angle),
            Symbol::YawRight { angle } => state.yaw(-angle),
            Symbol::PitchUp { angle } => state.pitch(angle),
            Symbol::PitchDown { angle } => state.pitch(-angle),
            Symbol::RollCw { angle } => state.roll(angle),
            Symbol::RollCcw { angle } => state.roll(-angle),
            Symbol::Push => {
                stack.push(state.clone());
                state.depth += 1;
            }
            Symbol::Pop => {
                state = stack.pop().ok_or_else(|| ProcGenError::InvalidParameter {
                    name: "Pop".into(),
                    reason: "stack underflow — Pop without matching Push".into(),
                })?;
            }
            Symbol::Custom { .. } => {
                // Extensibility hook — ignored by the turtle.
            }
        }
    }

    Ok(segments)
}

// ─── D. Convenience Constructors ─────────────────────────────────────────────

/// Create a deterministic symmetric binary tree L-system.
///
/// Axiom: `Forward(trunk_length, trunk_radius)`
/// Rule: `Forward(l, r) → Forward(l, r) Push YawLeft Forward(l*decay, r*decay) Pop Push YawRight Forward(l*decay, r*decay) Pop`
pub fn binary_tree_lsystem(
    trunk_length: f32,
    trunk_radius: f32,
    angle_deg: f32,
    params: LSystemParams,
) -> Result<LSystem> {
    let length_decay = params.length_decay;
    let radius_decay = params.radius_decay;

    LSystemBuilder::new(
        vec![Symbol::Forward {
            length: trunk_length,
            radius: trunk_radius,
        }],
        params,
    )
    .rule(move |sym, _rng| {
        if let Symbol::Forward { length, radius } = sym {
            let child_len = length * length_decay;
            let child_rad = radius * radius_decay;
            Some(vec![
                Symbol::Forward {
                    length: *length,
                    radius: *radius,
                },
                Symbol::Push,
                Symbol::YawLeft { angle: angle_deg },
                Symbol::Forward {
                    length: child_len,
                    radius: child_rad,
                },
                Symbol::Pop,
                Symbol::Push,
                Symbol::YawRight { angle: angle_deg },
                Symbol::Forward {
                    length: child_len,
                    radius: child_rad,
                },
                Symbol::Pop,
            ])
        } else {
            None
        }
    })
    .build()
}

/// Create a stochastic tree L-system with randomized branching angles.
///
/// Each `Forward` symbol branches with angle = `base_angle ± rng * angle_variation`.
pub fn stochastic_tree_lsystem(
    trunk_length: f32,
    trunk_radius: f32,
    base_angle: f32,
    angle_variation: f32,
    params: LSystemParams,
) -> Result<LSystem> {
    let length_decay = params.length_decay;
    let radius_decay = params.radius_decay;

    LSystemBuilder::new(
        vec![Symbol::Forward {
            length: trunk_length,
            radius: trunk_radius,
        }],
        params,
    )
    .rule(move |sym, rng| {
        if let Symbol::Forward { length, radius } = sym {
            let child_len = length * length_decay;
            let child_rad = radius * radius_decay;
            let left_angle = base_angle + rng.next_range(-angle_variation, angle_variation);
            let right_angle = base_angle + rng.next_range(-angle_variation, angle_variation);
            // Roll the branching plane by a random angle so branches spread
            // in 3D rather than staying in a single plane.
            let roll_angle = rng.next_range(30.0, 150.0);
            Some(vec![
                Symbol::Forward {
                    length: *length,
                    radius: *radius,
                },
                Symbol::RollCw { angle: roll_angle },
                Symbol::Push,
                Symbol::YawLeft { angle: left_angle },
                Symbol::Forward {
                    length: child_len,
                    radius: child_rad,
                },
                Symbol::Pop,
                Symbol::Push,
                Symbol::YawRight { angle: right_angle },
                Symbol::Forward {
                    length: child_len,
                    radius: child_rad,
                },
                Symbol::Pop,
            ])
        } else {
            None
        }
    })
    .build()
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn default_params(max_depth: u32) -> LSystemParams {
        LSystemParams {
            default_angle_deg: 30.0,
            length_decay: 0.7,
            radius_decay: 0.7,
            max_depth,
            max_segments: 10_000,
        }
    }

    // ── 1. Rewriting correctness ────────────────────────────────────────

    #[test]
    fn binary_tree_depth1_expansion() {
        let sys = binary_tree_lsystem(5.0, 1.0, 30.0, default_params(1)).unwrap();
        let mut rng = SeededRng::new(42);
        let symbols = sys.expand(&mut rng);

        // Forward → Forward Push YawLeft Forward Pop Push YawRight Forward Pop
        assert_eq!(symbols.len(), 9);
        assert!(matches!(symbols[0], Symbol::Forward { .. }));
        assert!(matches!(symbols[1], Symbol::Push));
        assert!(matches!(symbols[2], Symbol::YawLeft { .. }));
        assert!(matches!(symbols[3], Symbol::Forward { .. }));
        assert!(matches!(symbols[4], Symbol::Pop));
        assert!(matches!(symbols[5], Symbol::Push));
        assert!(matches!(symbols[6], Symbol::YawRight { .. }));
        assert!(matches!(symbols[7], Symbol::Forward { .. }));
        assert!(matches!(symbols[8], Symbol::Pop));
    }

    // ── 2. Segment counts for binary tree ───────────────────────────────

    #[test]
    fn binary_tree_segment_counts() {
        // Each Forward rewrites to Forward + [+Forward] + [-Forward] = 3 Forwards.
        // After d rewriting passes, Forward count is 3^d.
        for d in 1..=4u32 {
            let sys = binary_tree_lsystem(5.0, 1.0, 30.0, default_params(d)).unwrap();
            let mut rng = SeededRng::new(42);
            let segments = sys.generate_segments(&mut rng).unwrap();
            let expected = 3u32.pow(d);
            assert_eq!(
                segments.len(),
                expected as usize,
                "depth {d}: expected {expected} segments, got {}",
                segments.len()
            );
        }
    }

    // ── 3. Push/Pop restores position ───────────────────────────────────

    #[test]
    fn push_pop_restores_position() {
        // After branching, the second branch starts from the same position
        // as the first branch — both originate from the trunk tip.
        let sys = binary_tree_lsystem(5.0, 1.0, 30.0, default_params(1)).unwrap();
        let mut rng = SeededRng::new(42);
        let segments = sys.generate_segments(&mut rng).unwrap();

        // segments[0] = trunk, segments[1] = left branch, segments[2] = right branch
        assert_eq!(segments.len(), 3);
        let trunk_end = segments[0].end;
        let left_start = segments[1].start;
        let right_start = segments[2].start;

        assert!(
            vec3_approx_eq(trunk_end, left_start),
            "left branch should start at trunk end"
        );
        assert!(
            vec3_approx_eq(trunk_end, right_start),
            "right branch should start at trunk end"
        );
    }

    // ── 4. Stack underflow ──────────────────────────────────────────────

    #[test]
    fn stack_underflow_error() {
        let sys = LSystemBuilder::new(vec![Symbol::Pop], default_params(1))
            .build()
            .unwrap();
        let mut rng = SeededRng::new(42);
        let result = sys.generate_segments(&mut rng);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, ProcGenError::InvalidParameter { .. }),
            "expected InvalidParameter, got {err:?}"
        );
    }

    // ── 5. Budget exceeded ──────────────────────────────────────────────

    #[test]
    fn budget_exceeded() {
        let params = LSystemParams {
            max_segments: 10,
            max_depth: 6,
            ..default_params(6)
        };
        let sys = binary_tree_lsystem(5.0, 1.0, 30.0, params).unwrap();
        let mut rng = SeededRng::new(42);
        let result = sys.generate_segments(&mut rng);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, ProcGenError::BudgetExceeded(..)),
            "expected BudgetExceeded, got {err:?}"
        );
    }

    // ── 6. Geometric correctness: straight trunk ────────────────────────

    #[test]
    fn straight_trunk_geometry() {
        // A single Forward(5, 1) with no rules should produce one segment
        // from (0,0,0) to (0,5,0) since heading starts as +Y.
        let params = default_params(1);
        let sys = LSystemBuilder::new(
            vec![Symbol::Forward {
                length: 5.0,
                radius: 1.0,
            }],
            params,
        )
        .build()
        .unwrap();

        let mut rng = SeededRng::new(42);
        let segments = sys.generate_segments(&mut rng).unwrap();

        assert_eq!(segments.len(), 1);
        assert!(vec3_approx_eq(segments[0].start, Vec3::ZERO));
        assert!(vec3_approx_eq(segments[0].end, Vec3::new(0.0, 5.0, 0.0)));
    }

    // ── 7. Depth tracking ───────────────────────────────────────────────

    #[test]
    fn depth_tracking() {
        let sys = binary_tree_lsystem(5.0, 1.0, 30.0, default_params(1)).unwrap();
        let mut rng = SeededRng::new(42);
        let segments = sys.generate_segments(&mut rng).unwrap();

        // Trunk is depth 0, branches are depth 1
        assert_eq!(segments[0].depth, 0, "trunk should be depth 0");
        assert_eq!(segments[1].depth, 1, "left branch should be depth 1");
        assert_eq!(segments[2].depth, 1, "right branch should be depth 1");
    }

    // ── 8. Radius decay ────────────────────────────────────────────────

    #[test]
    fn radius_decay_applied() {
        let params = LSystemParams {
            radius_decay: 0.5,
            ..default_params(1)
        };
        let sys = LSystemBuilder::new(
            vec![Symbol::Forward {
                length: 5.0,
                radius: 2.0,
            }],
            params,
        )
        .build()
        .unwrap();

        let mut rng = SeededRng::new(42);
        let segments = sys.generate_segments(&mut rng).unwrap();

        assert_eq!(segments[0].radius_start, 2.0);
        assert!(
            (segments[0].radius_end - 1.0).abs() < 1e-6,
            "radius_end should be 2.0 * 0.5 = 1.0"
        );
    }

    // ── 9. Determinism ──────────────────────────────────────────────────

    #[test]
    fn determinism_same_seed() {
        let params = default_params(3);
        let sys = stochastic_tree_lsystem(5.0, 1.0, 30.0, 10.0, params).unwrap();

        let mut rng1 = SeededRng::new(42);
        let seg1 = sys.generate_segments(&mut rng1).unwrap();

        let mut rng2 = SeededRng::new(42);
        let seg2 = sys.generate_segments(&mut rng2).unwrap();

        assert_eq!(seg1.len(), seg2.len());
        for (a, b) in seg1.iter().zip(seg2.iter()) {
            assert!(vec3_approx_eq(a.start, b.start));
            assert!(vec3_approx_eq(a.end, b.end));
            assert!((a.radius_start - b.radius_start).abs() < 1e-6);
        }
    }

    #[test]
    fn different_seeds_differ() {
        let params = default_params(3);
        let sys = stochastic_tree_lsystem(5.0, 1.0, 30.0, 10.0, params).unwrap();

        let mut rng1 = SeededRng::new(42);
        let seg1 = sys.generate_segments(&mut rng1).unwrap();

        let mut rng2 = SeededRng::new(999);
        let seg2 = sys.generate_segments(&mut rng2).unwrap();

        // At least one segment should differ in position
        let any_differ = seg1
            .iter()
            .zip(seg2.iter())
            .any(|(a, b)| !vec3_approx_eq(a.end, b.end));
        assert!(any_differ, "different seeds should produce different trees");
    }

    // ── 10. Rotation correctness ────────────────────────────────────────

    #[test]
    fn yaw_left_90_rotates_heading() {
        let params = default_params(1);
        let sys = LSystemBuilder::new(
            vec![
                Symbol::YawLeft { angle: 90.0 },
                Symbol::Forward {
                    length: 5.0,
                    radius: 1.0,
                },
            ],
            params,
        )
        .build()
        .unwrap();

        let mut rng = SeededRng::new(42);
        let segments = sys.generate_segments(&mut rng).unwrap();

        assert_eq!(segments.len(), 1);
        let start = segments[0].start;
        let end = segments[0].end;

        // Yaw turns around the up axis (-Z), and a positive turn carries the
        // heading from +Y to +X.
        assert!(vec3_approx_eq(start, Vec3::ZERO));
        assert!(
            vec3_approx_eq(end, Vec3::new(5.0, 0.0, 0.0)),
            "yaw left 90 should head along +X, got {end:?}"
        );
    }

    // ── Builder validation ──────────────────────────────────────────────

    #[test]
    fn builder_rejects_zero_max_depth() {
        let params = LSystemParams {
            max_depth: 0,
            ..Default::default()
        };
        let result = LSystemBuilder::new(vec![], params).build();
        assert!(result.is_err());
    }

    #[test]
    fn builder_rejects_invalid_decay() {
        let params = LSystemParams {
            length_decay: 0.0, // invalid: must be > 0
            ..Default::default()
        };
        let result = LSystemBuilder::new(vec![], params).build();
        assert!(result.is_err());

        let params2 = LSystemParams {
            radius_decay: 1.5, // invalid: must be <= 1.0
            ..Default::default()
        };
        let result2 = LSystemBuilder::new(vec![], params2).build();
        assert!(result2.is_err());
    }

    // ── BranchSegment helpers ───────────────────────────────────────────

    #[test]
    fn branch_segment_helpers() {
        let seg = BranchSegment {
            start: Vec3::ZERO,
            end: Vec3::new(0.0, 3.0, 4.0),
            radius_start: 2.0,
            radius_end: 1.0,
            depth: 0,
        };
        assert!((seg.length() - 5.0).abs() < 1e-6);
        assert!((seg.mean_radius() - 1.5).abs() < 1e-6);
        let dir = seg.direction();
        assert!((dir.y - 0.6).abs() < 1e-6);
        assert!((dir.z - 0.8).abs() < 1e-6);
    }

    #[test]
    fn branch_segment_degenerate() {
        let seg = BranchSegment {
            start: Vec3::ZERO,
            end: Vec3::ZERO,
            radius_start: 1.0,
            radius_end: 1.0,
            depth: 0,
        };
        assert!(seg.length() < 1e-6);
        assert_eq!(seg.direction(), Vec3::ZERO);
    }

    // ── Helper ──────────────────────────────────────────────────────────

    fn vec3_approx_eq(a: Vec3, b: Vec3) -> bool {
        (a.x - b.x).abs() < 1e-4 && (a.y - b.y).abs() < 1e-4 && (a.z - b.z).abs() < 1e-4
    }
}
