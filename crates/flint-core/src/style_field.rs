//! The style field (#859): a screen field the game fills each frame, which a
//! style's blend can follow (`blend_source = "field"`).
//!
//! The field is a handful of soft shapes laid over the screen: a disc, a
//! ring, a cone. Its value at a pixel is the strongest shape's there, 0..1.
//! A prop whose style follows it is drawn at its blend times that value, so
//! a shockwave ring spreading from an impact turns the props it crosses to
//! text and lets them go behind it, and a flashlight cone shows what things
//! really are wherever it points.
//!
//! The shapes are analytic, like the tear's mask: the style mask pass works
//! each one out at its own pixels, so the field costs no texture, no pass
//! and nothing at all on a frame that fills none. Scripts fill it with
//! `field_disc` / `field_ring` / `field_cone` (immediate mode: a frame's
//! field is what was drawn that frame), and the host hands it to the
//! renderer with the frame's inputs.
//!
//! Positions are in screen uv (0..1 across, 0..1 down) and every length is
//! a fraction of the screen's height, so a disc is round at any aspect and
//! a field means the same at any resolution or render scale.

use serde::{Deserialize, Serialize};

/// The most shapes a frame's field holds; past it the rest are dropped,
/// said once by whoever fills it.
pub const MAX_FIELD_SHAPES: usize = 16;

/// One shape of the field. Lengths are fractions of the screen's height;
/// `soft` is how far past the shape's edge its value falls to 0 (0 is a
/// hard edge); `strength` (0..1) is its value inside.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum FieldShape {
    /// Full inside `radius` of `center`.
    Disc {
        center: [f32; 2],
        radius: f32,
        soft: f32,
        strength: f32,
    },
    /// Full within `width / 2` of the circle of `radius` round `center`:
    /// a shockwave's front.
    Ring {
        center: [f32; 2],
        radius: f32,
        width: f32,
        soft: f32,
        strength: f32,
    },
    /// Full within `spread` radians either side of `direction` and within
    /// `length` of `apex`: a flashlight's beam. `direction` is radians
    /// clockwise from straight up the screen, as the HUD's ring sectors
    /// count degrees from 12 o'clock.
    Cone {
        apex: [f32; 2],
        direction: f32,
        spread: f32,
        length: f32,
        soft: f32,
        strength: f32,
    },
}

impl FieldShape {
    /// Which shape it is, as the mask shader numbers them.
    pub fn kind(&self) -> u32 {
        match self {
            FieldShape::Disc { .. } => 0,
            FieldShape::Ring { .. } => 1,
            FieldShape::Cone { .. } => 2,
        }
    }

    /// The shape's value at `uv` on a screen `aspect` (width / height)
    /// wide: what the mask shader works out, for tests and for a host that
    /// wants to ask the field on the CPU.
    pub fn value_at(&self, uv: [f32; 2], aspect: f32) -> f32 {
        let at = |c: [f32; 2]| [(uv[0] - c[0]) * aspect, uv[1] - c[1]];
        let len = |q: [f32; 2]| (q[0] * q[0] + q[1] * q[1]).sqrt();
        let (outside, soft, strength) = match *self {
            FieldShape::Disc {
                center,
                radius,
                soft,
                strength,
            } => (len(at(center)) - radius, soft, strength),
            FieldShape::Ring {
                center,
                radius,
                width,
                soft,
                strength,
            } => (
                (len(at(center)) - radius).abs() - width * 0.5,
                soft,
                strength,
            ),
            FieldShape::Cone {
                apex,
                direction,
                spread,
                length,
                soft,
                strength,
            } => {
                let q = at(apex);
                let d = len(q);
                let off = wrap_angle(q[0].atan2(-q[1]) - direction).abs();
                // Past the beam's side by the arc's length, or past its end
                (((off - spread) * d).max(d - length), soft, strength)
            }
        };
        edge(outside, soft) * strength.clamp(0.0, 1.0)
    }
}

/// `x` wrapped to -pi..pi.
fn wrap_angle(x: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    x - tau * ((x + std::f32::consts::PI) / tau).floor()
}

/// 1 at or inside a shape's edge (`outside <= 0`), falling smoothly to 0
/// `soft` past it.
fn edge(outside: f32, soft: f32) -> f32 {
    let t = (outside / soft.max(1e-5)).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

/// A frame's field: the shapes, at most [`MAX_FIELD_SHAPES`]. Empty is no
/// field, and a prop following it draws as at blend 0.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StyleField {
    shapes: Vec<FieldShape>,
    /// Shapes asked for past the cap this frame.
    dropped: usize,
}

impl StyleField {
    /// Add a shape; past [`MAX_FIELD_SHAPES`] it is counted in
    /// [`dropped`](Self::dropped) instead. A shape of strength 0 adds
    /// nothing and is left out.
    pub fn push(&mut self, shape: FieldShape) {
        let strength = match shape {
            FieldShape::Disc { strength, .. }
            | FieldShape::Ring { strength, .. }
            | FieldShape::Cone { strength, .. } => strength,
        };
        if strength <= 0.0 || strength.is_nan() {
            return;
        }
        if self.shapes.len() < MAX_FIELD_SHAPES {
            self.shapes.push(shape);
        } else {
            self.dropped += 1;
        }
    }

    pub fn shapes(&self) -> &[FieldShape] {
        &self.shapes
    }

    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    /// How many shapes this frame asked for past the cap.
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// Forget every shape, keeping the allocation.
    pub fn clear(&mut self) {
        self.shapes.clear();
        self.dropped = 0;
    }

    /// Hold exactly `shapes` (the first [`MAX_FIELD_SHAPES`] of them),
    /// keeping the allocation.
    pub fn set(&mut self, shapes: &[FieldShape]) {
        self.clear();
        for &shape in shapes {
            self.push(shape);
        }
    }

    /// The field's value at `uv`: its strongest shape's.
    pub fn value_at(&self, uv: [f32; 2], aspect: f32) -> f32 {
        self.shapes
            .iter()
            .map(|s| s.value_at(uv, aspect))
            .fold(0.0, f32::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(radius: f32) -> FieldShape {
        FieldShape::Ring {
            center: [0.5, 0.5],
            radius,
            width: 0.1,
            soft: 0.05,
            strength: 1.0,
        }
    }

    #[test]
    fn a_ring_is_full_on_its_circle_and_empty_inside_and_out() {
        let r = ring(0.3);
        // On the circle, to the right of the centre (aspect 1)
        assert_eq!(r.value_at([0.8, 0.5], 1.0), 1.0);
        assert_eq!(r.value_at([0.5, 0.5], 1.0), 0.0, "the centre is past it");
        assert_eq!(r.value_at([0.95, 0.5], 1.0), 0.0, "outside");
        let rim = r.value_at([0.5 + 0.3 + 0.05 + 0.025, 0.5], 1.0);
        assert!(rim > 0.0 && rim < 1.0, "the soft rim falls off: {rim}");
    }

    #[test]
    fn lengths_are_the_screens_height_so_a_disc_is_round_at_any_aspect() {
        let disc = FieldShape::Disc {
            center: [0.5, 0.5],
            radius: 0.2,
            soft: 0.0,
            strength: 1.0,
        };
        // On a 2:1 screen 0.2 of the height is 0.1 of the width
        assert_eq!(disc.value_at([0.59, 0.5], 2.0), 1.0);
        assert_eq!(disc.value_at([0.61, 0.5], 2.0), 0.0);
        assert_eq!(disc.value_at([0.5, 0.69], 2.0), 1.0);
        assert_eq!(disc.value_at([0.5, 0.71], 2.0), 0.0);
    }

    #[test]
    fn a_cone_lights_along_its_direction_and_not_behind_it() {
        // From the bottom middle, pointing straight up, 20 degrees either side
        let cone = FieldShape::Cone {
            apex: [0.5, 1.0],
            direction: 0.0,
            spread: 20f32.to_radians(),
            length: 0.8,
            soft: 0.0,
            strength: 1.0,
        };
        assert_eq!(cone.value_at([0.5, 0.5], 1.0), 1.0, "straight ahead");
        assert_eq!(cone.value_at([0.5, 0.1], 1.0), 0.0, "past its length");
        assert_eq!(cone.value_at([0.9, 0.9], 1.0), 0.0, "off to the side");
        // Turned 90 degrees clockwise it points right
        let right = FieldShape::Cone {
            apex: [0.0, 0.5],
            direction: 90f32.to_radians(),
            spread: 20f32.to_radians(),
            length: 0.8,
            soft: 0.0,
            strength: 1.0,
        };
        assert_eq!(right.value_at([0.5, 0.5], 1.0), 1.0);
        assert_eq!(right.value_at([0.5, 0.1], 1.0), 0.0);
    }

    #[test]
    fn the_field_is_its_strongest_shape_and_holds_at_most_the_cap() {
        let mut field = StyleField::default();
        assert!(field.is_empty());
        field.push(FieldShape::Disc {
            center: [0.5, 0.5],
            radius: 0.1,
            soft: 0.0,
            strength: 0.4,
        });
        field.push(ring(0.0));
        assert_eq!(field.value_at([0.5, 0.5], 1.0), 1.0, "the ring at 0 wins");
        for _ in 0..MAX_FIELD_SHAPES {
            field.push(ring(0.2));
        }
        assert_eq!(field.shapes().len(), MAX_FIELD_SHAPES);
        assert_eq!(field.dropped(), 2);
        field.push(FieldShape::Disc {
            center: [0.5, 0.5],
            radius: 0.1,
            soft: 0.0,
            strength: 0.0,
        });
        assert_eq!(field.dropped(), 2, "a shape of strength 0 is no shape");
        field.clear();
        assert!(field.is_empty() && field.dropped() == 0);
    }
}
