//! TOML value coercion utilities.
//!
//! TOML integers and floats are distinct types — `1` parses as integer, `1.0` as
//! float. These helpers coerce either form into Rust numeric types so callers
//! don't have to repeat the `as_float().or_else(|| as_integer().map(...))` dance.

use crate::Vec3;

/// Coerce a TOML integer or float to `f64`.
pub fn toml_f64(v: &toml::Value) -> Option<f64> {
    v.as_float().or_else(|| v.as_integer().map(|i| i as f64))
}

/// Coerce a TOML integer or float to `f32`.
pub fn toml_f32(v: &toml::Value) -> Option<f32> {
    toml_f64(v).map(|f| f as f32)
}

/// Extract `[f32; 3]` from a TOML array of 3+ numbers.
pub fn toml_vec3(v: &toml::Value) -> Option<[f32; 3]> {
    let arr = v.as_array()?;
    if arr.len() >= 3 {
        Some([toml_f32(&arr[0])?, toml_f32(&arr[1])?, toml_f32(&arr[2])?])
    } else {
        None
    }
}

/// Extract `[f32; 4]` from a TOML array of 4+ numbers.
pub fn toml_vec4(v: &toml::Value) -> Option<[f32; 4]> {
    let arr = v.as_array()?;
    if arr.len() >= 4 {
        Some([
            toml_f32(&arr[0])?,
            toml_f32(&arr[1])?,
            toml_f32(&arr[2])?,
            toml_f32(&arr[3])?,
        ])
    } else {
        None
    }
}

/// Extract an RGBA color from a TOML array of 3 (alpha defaults to 1.0) or 4 numbers.
pub fn toml_color(v: &toml::Value) -> Option<[f32; 4]> {
    let arr = v.as_array()?;
    if arr.len() < 3 {
        return None;
    }
    let r = toml_f32(&arr[0])?;
    let g = toml_f32(&arr[1])?;
    let b = toml_f32(&arr[2])?;
    let a = if arr.len() >= 4 {
        toml_f32(&arr[3]).unwrap_or(1.0)
    } else {
        1.0
    };
    Some([r, g, b, a])
}

/// Coerce every element of a `&[Value]` slice to `f32`.
pub fn toml_f32_slice(arr: &[toml::Value]) -> Option<Vec<f32>> {
    arr.iter().map(toml_f32).collect()
}

/// Extract a [`Vec3`] from a TOML value that is either an array `[x, y, z]` or
/// a table `{ x = .., y = .., z = .. }`.
pub fn toml_to_vec3(v: &toml::Value) -> Option<Vec3> {
    if let Some(arr) = v.as_array() {
        if arr.len() >= 3 {
            return Some(Vec3::new(
                toml_f32(&arr[0])?,
                toml_f32(&arr[1])?,
                toml_f32(&arr[2])?,
            ));
        }
    }
    if let Some(table) = v.as_table() {
        let x = table.get("x").and_then(toml_f32)?;
        let y = table.get("y").and_then(toml_f32)?;
        let z = table.get("z").and_then(toml_f32)?;
        return Some(Vec3::new(x, y, z));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml::Value;

    #[test]
    fn f64_from_float() {
        let v: Value = 3.14.into();
        assert_eq!(toml_f64(&v), Some(3.14));
    }

    #[test]
    fn f64_from_integer() {
        let v: Value = 42.into();
        assert_eq!(toml_f64(&v), Some(42.0));
    }

    #[test]
    fn f64_from_string_returns_none() {
        let v: Value = "hello".into();
        assert_eq!(toml_f64(&v), None);
    }

    #[test]
    fn f32_coercion() {
        let v: Value = 1.5.into();
        assert_eq!(toml_f32(&v), Some(1.5f32));
        let v: Value = 7.into();
        assert_eq!(toml_f32(&v), Some(7.0f32));
    }

    #[test]
    fn vec3_from_array() {
        let v: Value = toml::toml! { x = [1.0, 2, 3.5] }.get("x").unwrap().clone();
        assert_eq!(toml_vec3(&v), Some([1.0, 2.0, 3.5]));
    }

    #[test]
    fn vec3_too_short() {
        let v: Value = toml::toml! { x = [1.0, 2.0] }.get("x").unwrap().clone();
        assert_eq!(toml_vec3(&v), None);
    }

    #[test]
    fn vec4_from_array() {
        let v: Value = toml::toml! { x = [1, 2, 3, 4] }.get("x").unwrap().clone();
        assert_eq!(toml_vec4(&v), Some([1.0, 2.0, 3.0, 4.0]));
    }

    #[test]
    fn color_rgb_defaults_alpha() {
        let v: Value = toml::toml! { c = [0.5, 0.6, 0.7] }
            .get("c")
            .unwrap()
            .clone();
        assert_eq!(toml_color(&v), Some([0.5, 0.6, 0.7, 1.0]));
    }

    #[test]
    fn color_rgba() {
        let v: Value = toml::toml! { c = [0.1, 0.2, 0.3, 0.4] }
            .get("c")
            .unwrap()
            .clone();
        assert_eq!(toml_color(&v), Some([0.1, 0.2, 0.3, 0.4]));
    }

    #[test]
    fn f32_slice() {
        let arr = vec![Value::from(1), Value::from(2.5), Value::from(3)];
        assert_eq!(toml_f32_slice(&arr), Some(vec![1.0, 2.5, 3.0]));
    }

    #[test]
    fn f32_slice_with_bad_element() {
        let arr = vec![Value::from(1), Value::from("x")];
        assert_eq!(toml_f32_slice(&arr), None);
    }

    #[test]
    fn to_vec3_from_array() {
        let v: Value = toml::toml! { p = [1, 2.0, 3] }.get("p").unwrap().clone();
        assert_eq!(toml_to_vec3(&v), Some(Vec3::new(1.0, 2.0, 3.0)));
    }

    #[test]
    fn to_vec3_from_table() {
        let v: Value = toml::toml! {
            [p]
            x = 4
            y = 5.0
            z = 6
        }
        .get("p")
        .unwrap()
        .clone();
        assert_eq!(toml_to_vec3(&v), Some(Vec3::new(4.0, 5.0, 6.0)));
    }

    #[test]
    fn to_vec3_invalid() {
        let v: Value = "not a vec3".into();
        assert_eq!(toml_to_vec3(&v), None);
    }
}
