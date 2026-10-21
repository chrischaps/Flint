//! Text written in the world, drawn with a depth test (ADR 0104).
//!
//! A `WorldText` is a line of text at a world point. Scripts queue them with
//! `draw_world_text`; the player lays them out in the HUD's fonts and draws
//! them over the finished image, before the HUD, hidden wherever the scene's
//! depth is nearer. A shop's name on its sign stays behind the roof in front
//! of it, as `draw_text_opts` at a projected point can't.
//!
//! Without a plane the text is a billboard: it faces the screen at the size
//! asked, in logical points, like HUD text, and takes the point's depth.
//! On a [`TextPlane`] it lies in that plane, as if painted there, and every
//! length (its size, letter spacing, stroke and shadow) is in world units.

/// One depth-tested line of text in the world.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldText {
    /// The anchor, in world units. `align` and `valign` place the text
    /// round it.
    pub at: [f32; 3],
    pub text: String,
    /// The font size: logical points for a billboard, world units (the em)
    /// on a plane.
    pub size: f32,
    /// Display-space (sRGB) RGBA, the same space as the HUD's colours.
    pub color: [f32; 4],
    /// Horizontal placement about `at`: 0 left edge, 1 centre, 2 right edge.
    pub align: u8,
    /// Vertical placement about `at`: 0 top, 1 middle, 2 bottom.
    pub valign: u8,
    /// Outline colour and width, as `draw_text_opts`'s.
    pub stroke: Option<([f32; 4], f32)>,
    /// A family registered from the game's `fonts/`, or the default.
    pub font: Option<String>,
    /// Extra space between letters.
    pub spacing: f32,
    /// Shadow colour and its offset (right, down).
    pub shadow: Option<([f32; 4], f32, f32)>,
    /// The plane it's written on, or `None` for a billboard.
    pub plane: Option<TextPlane>,
}

/// The plane a [`WorldText`] lies in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextPlane {
    /// The way the written face looks. The text reads from this side and
    /// isn't drawn from behind, like paint on one face of a board.
    pub normal: [f32; 3],
    /// Which way is up for the letters. Only its part across the plane
    /// counts, so world up works for any wall or sign.
    pub up: [f32; 3],
}

impl TextPlane {
    /// The letters' right and up directions, unit length, in the plane:
    /// right is the reader's right, facing the written face. `None` if the
    /// normal is zero or `up` lies along it.
    pub fn basis(&self) -> Option<([f32; 3], [f32; 3])> {
        let n = normalize(self.normal)?;
        let right = normalize(cross(self.up, n))?;
        Some((right, cross(n, right)))
    }

    /// Whether `eye` is on the written side of the plane through `at`.
    pub fn faces(&self, at: [f32; 3], eye: [f32; 3]) -> bool {
        let to_eye = [eye[0] - at[0], eye[1] - at[1], eye[2] - at[2]];
        dot(to_eye, self.normal) > 0.0
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> Option<[f32; 3]> {
    let len = dot(v, v).sqrt();
    (len > 1e-6).then(|| [v[0] / len, v[1] / len, v[2] / len])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-5)
    }

    #[test]
    fn a_wall_facing_south_reads_east_and_up() {
        // Facing +z, a reader stands at +z looking toward -z: their right
        // is +x.
        let plane = TextPlane { normal: [0.0, 0.0, 1.0], up: [0.0, 1.0, 0.0] };
        let (right, up) = plane.basis().unwrap();
        assert!(close(right, [1.0, 0.0, 0.0]), "{right:?}");
        assert!(close(up, [0.0, 1.0, 0.0]), "{up:?}");
    }

    #[test]
    fn up_is_squared_to_a_leaning_plane() {
        let plane = TextPlane { normal: [0.0, 1.0, 1.0], up: [0.0, 1.0, 0.0] };
        let (right, up) = plane.basis().unwrap();
        assert!(close(right, [1.0, 0.0, 0.0]), "{right:?}");
        let h = std::f32::consts::FRAC_1_SQRT_2;
        assert!(close(up, [0.0, h, -h]), "{up:?}");
    }

    #[test]
    fn up_along_the_normal_has_no_basis() {
        let flat = TextPlane { normal: [0.0, 1.0, 0.0], up: [0.0, 1.0, 0.0] };
        assert!(flat.basis().is_none());
        let floor = TextPlane { normal: [0.0, 1.0, 0.0], up: [0.0, 0.0, -1.0] };
        let (right, up) = floor.basis().unwrap();
        assert!(close(right, [1.0, 0.0, 0.0]), "{right:?}");
        assert!(close(up, [0.0, 0.0, -1.0]), "{up:?}");
    }

    #[test]
    fn it_faces_only_the_side_its_normal_points_to() {
        let plane = TextPlane { normal: [0.0, 0.0, 1.0], up: [0.0, 1.0, 0.0] };
        assert!(plane.faces([0.0, 0.0, 0.0], [3.0, 1.0, 5.0]));
        assert!(!plane.faces([0.0, 0.0, 0.0], [3.0, 1.0, -5.0]));
        assert!(!plane.faces([0.0, 0.0, 0.0], [3.0, 1.0, 0.0]));
    }
}
