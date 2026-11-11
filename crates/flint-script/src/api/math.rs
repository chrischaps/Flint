//! The `math` module: clamps, interpolation, trigonometry, angles and the
//! Y-up direction helpers (ADR 0116). `random`, `random_range` and
//! `seed_random` are declared into it by [`crate::random::ScriptRandom`].
//!
//! Several share a name with one of Rhai's built-ins (`abs`, `min`, `max`,
//! `sin`, `sqrt`, `floor`, `PI`...). Rhai tries a call's exact argument
//! types everywhere before any converting signature, and this engine's own
//! functions before its packages, so a float call reaches the function here
//! (as before the builder), an all-int `abs`, `min` or `max` still reaches
//! Rhai's integer one and returns an int, and the converting signature
//! catches the rest: `sin(1)`, `clamp(5, 0, 10)`.

use super::builder::kind::{Map, Num};
use super::builder::ScriptApi;
use rhai::Dynamic;
use std::f64::consts::{PI, TAU};

fn xyz(x: f64, y: f64, z: f64) -> rhai::Map {
    let mut map = rhai::Map::new();
    map.insert("x".into(), Dynamic::from(x));
    map.insert("y".into(), Dynamic::from(y));
    map.insert("z".into(), Dynamic::from(z));
    map
}

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("math");

    m.func("clamp", "The value kept within min..max.")
        .arg("value", Num)
        .arg("min", Num)
        .arg("max", Num)
        .returns(Num, "")
        .run(|_, (value, min, max)| value.clamp(min, max));

    m.func("lerp", "Linear interpolation from a to b by t (0 a, 1 b).")
        .arg("a", Num)
        .arg("b", Num)
        .arg("t", Num)
        .returns(Num, "")
        .run(|_, (a, b, t)| a + (b - a) * t);

    m.func("sin", "Sine of radians.")
        .arg("x", Num)
        .returns(Num, "")
        .run(|_, (x,)| x.sin());
    m.func("cos", "Cosine of radians.")
        .arg("x", Num)
        .returns(Num, "")
        .run(|_, (x,)| x.cos());
    m.func(
        "abs",
        "Absolute value (an int gives an int back, by Rhai's own abs).",
    )
    .arg("x", Num)
    .returns(Num, "")
    .run(|_, (x,)| x.abs());
    m.func("sqrt", "Square root.")
        .arg("x", Num)
        .returns(Num, "")
        .run(|_, (x,)| x.sqrt());
    m.func("floor", "The largest whole number not above x, as a float.")
        .arg("x", Num)
        .returns(Num, "")
        .run(|_, (x,)| x.floor());
    m.func("ceil", "The smallest whole number not below x, as a float.")
        .arg("x", Num)
        .returns(Num, "")
        .run(|_, (x,)| x.ceil());
    m.func("min", "The smaller of two (two ints give an int back).")
        .arg("a", Num)
        .arg("b", Num)
        .returns(Num, "")
        .run(|_, (a, b)| a.min(b));
    m.func("max", "The larger of two (two ints give an int back).")
        .arg("a", Num)
        .arg("b", Num)
        .returns(Num, "")
        .run(|_, (a, b)| a.max(b));
    m.func("atan2", "The angle of (x, y) from the +x axis, radians.")
        .arg("y", Num)
        .arg("x", Num)
        .returns(Num, "-PI..PI")
        .run(|_, (y, x)| y.atan2(x));

    m.func("PI", "The constant pi, 3.14159...")
        .returns(Num, "")
        .run(|_, ()| PI);
    m.func("TAU", "The constant tau = 2 pi, 6.28318...")
        .returns(Num, "")
        .run(|_, ()| TAU);

    m.func("deg_to_rad", "Degrees as radians.")
        .arg("degrees", Num)
        .returns(Num, "")
        .run(|_, (degrees,)| degrees * PI / 180.0);
    m.func("rad_to_deg", "Radians as degrees.")
        .arg("radians", Num)
        .returns(Num, "")
        .run(|_, (radians,)| radians * 180.0 / PI);

    // Direction helpers: the Y-up, right-handed coordinate system, FORWARD
    // (0, 0, -1) and RIGHT (1, 0, 0) turned by yaw about Y
    m.func(
        "forward_from_yaw",
        "The forward direction (-Z at yaw 0) turned by a yaw in degrees about +Y.",
    )
    .arg("yaw_degrees", Num)
    .returns(Map, "#{x, y, z}, y 0")
    .run(|_, (yaw,)| {
        let r = yaw * PI / 180.0;
        xyz(-r.sin(), 0.0, -r.cos())
    });
    m.func(
        "right_from_yaw",
        "The right direction (+X at yaw 0) turned by a yaw in degrees about +Y.",
    )
    .arg("yaw_degrees", Num)
    .returns(Map, "#{x, y, z}, y 0")
    .run(|_, (yaw,)| {
        let r = yaw * PI / 180.0;
        xyz(r.cos(), 0.0, -r.sin())
    });

    m.func("wrap_angle", "Degrees brought into [0, 360).")
        .arg("degrees", Num)
        .returns(Num, "")
        .run(|_, (degrees,)| ((degrees % 360.0) + 360.0) % 360.0);
}

#[cfg(test)]
mod tests {
    use crate::engine::ScriptEngine;

    fn eval<T: Clone + Send + Sync + 'static>(src: &str) -> T {
        ScriptEngine::new().rhai_engine().eval::<T>(src).unwrap()
    }

    #[test]
    fn floats_as_before() {
        assert!((eval::<f64>("clamp(5.0, 0.0, 3.0)") - 3.0).abs() < 1e-10);
        assert!((eval::<f64>("lerp(0.0, 10.0, 0.5)") - 5.0).abs() < 1e-10);
        assert!(eval::<f64>("sin(0.0)").abs() < 1e-10);
        assert!((eval::<f64>("cos(0.0)") - 1.0).abs() < 1e-10);
        assert_eq!(eval::<f64>("atan2(1.0, 0.0)"), std::f64::consts::FRAC_PI_2);
        assert_eq!(eval::<f64>("wrap_angle(-90.0)"), 270.0);
        assert_eq!(eval::<f64>("forward_from_yaw(0.0).z"), -1.0);
    }

    #[test]
    fn ints_work_where_numbers_are_wanted() {
        assert_eq!(eval::<f64>("clamp(5, 0, 3)"), 3.0);
        assert_eq!(eval::<f64>("lerp(0, 10, 0.5)"), 5.0);
        assert_eq!(eval::<f64>("sin(0)"), 0.0);
        assert_eq!(eval::<f64>("sqrt(9)"), 3.0);
        assert_eq!(eval::<f64>("deg_to_rad(180)"), std::f64::consts::PI);
        assert_eq!(eval::<f64>("wrap_angle(-90)"), 270.0);
        assert_eq!(eval::<f64>("random_range(2, 2)"), 2.0);
    }

    #[test]
    fn rhai_integer_built_ins_still_answer_for_ints() {
        // Rhai's own integer abs/min/max are found before the converting
        // signature, so an int comes back, as before
        assert_eq!(eval::<i64>("abs(-3)"), 3);
        assert_eq!(eval::<i64>("min(2, 7)"), 2);
        assert_eq!(eval::<i64>("max(2, 7)"), 7);
        assert_eq!(eval::<f64>("max(2, 7.5)"), 7.5);
        assert_eq!(eval::<f64>("abs(-2.5)"), 2.5);
    }
}
