//! `set_post` (#853): one setter over the `[post_process]` keys, and the
//! setters it replaced, each still asking for exactly what it did.

use crate::context::PostProcessOverrides;
use crate::engine::ScriptEngine;
use flint_schema::post_effects::PostValue;

/// What `src` asks of the frame's post-processing, or the script error.
fn asks(src: &str) -> Result<PostProcessOverrides, String> {
    let engine = ScriptEngine::new();
    engine
        .rhai_engine()
        .eval::<()>(src)
        .map_err(|e| e.to_string())?;
    let c = crate::lock_or_recover(&engine.ctx);
    Ok(c.requests.post.clone())
}

/// Each old setter and the `set_post` calls that ask the same. The test
/// below holds every row; a row is the changelog's mapping too.
const OLD_AND_NEW: &[(&str, &str)] = &[
    (
        "set_vignette(0.4)",
        "set_post(\"vignette_enabled\", true); set_post(\"vignette_intensity\", 0.4)",
    ),
    (
        "set_vignette(0.0)",
        "set_post(\"vignette_enabled\", false); set_post(\"vignette_intensity\", 0.0)",
    ),
    (
        "set_bloom_intensity(0.3)",
        "set_post(\"bloom_intensity\", 0.3)",
    ),
    ("set_exposure(1.4)", "set_post(\"exposure\", 1.4)"),
    (
        "set_chromatic_aberration(0.01)",
        "set_post(\"chromatic_aberration\", 0.01)",
    ),
    ("set_radial_blur(0.2)", "set_post(\"radial_blur\", 0.2)"),
    ("set_ssao_intensity(2)", "set_post(\"ssao_intensity\", 2)"),
    ("set_fog_density(0.05)", "set_post(\"fog_density\", 0.05)"),
    (
        "set_fog_color(0.2, 0.3, 0.4)",
        "set_post(\"fog_color\", [0.2, 0.3, 0.4])",
    ),
    ("set_desaturation(0.85)", "set_post(\"desaturate\", 0.85)"),
    ("set_dof(0.6)", "set_post(\"dof_strength\", 0.6)"),
    (
        "set_dof_focus(12.0, 3.0)",
        "set_post(\"dof_focus_distance\", 12.0); set_post(\"dof_focus_range\", 3.0)",
    ),
    ("set_tear(0.7)", "set_post(\"tear\", 0.7)"),
    (
        "set_tear_shape(5.0, true)",
        "set_post(\"tear_scale\", 5.0); set_post(\"tear_iris\", true)",
    ),
];

#[test]
fn each_old_setter_asks_what_its_set_post_calls_ask() {
    for (old, new) in OLD_AND_NEW {
        let before = asks(old).unwrap_or_else(|e| panic!("{old}: {e}"));
        let after = asks(new).unwrap_or_else(|e| panic!("{new}: {e}"));
        assert!(!before.is_empty(), "{old} asks for something");
        assert_eq!(before, after, "{old} against {new}");
    }
}

#[test]
fn set_post_takes_every_kind_of_key() {
    let post = asks(
        "set_post(\"vignette_intensity\", 0.4); set_post(\"fxaa\", true); \
         set_post(\"ssao_samples\", 16); set_post(\"grade_gain\", [1, 0.9, 0.8]); \
         set_post(\"exposure\", 2)",
    )
    .unwrap();
    assert_eq!(post.get("vignette_intensity"), Some(PostValue::F32(0.4)));
    assert_eq!(post.get("fxaa"), Some(PostValue::Bool(true)));
    assert_eq!(post.get("ssao_samples"), Some(PostValue::U32(16)));
    assert_eq!(
        post.get("grade_gain"),
        Some(PostValue::Rgb([1.0, 0.9, 0.8]))
    );
    assert_eq!(post.f32("exposure"), Some(2.0), "an int is a number");
}

#[test]
fn the_last_value_a_key_is_given_wins() {
    let post = asks("set_post(\"exposure\", 2.0); set_exposure(0.5)").unwrap();
    assert_eq!(post.f32("exposure"), Some(0.5));
}

#[test]
fn an_unknown_key_is_a_script_error_listing_the_keys() {
    let e = asks("set_post(\"bloom_intensty\", 0.2)").unwrap_err();
    assert!(
        e.contains("set_post: `bloom_intensty` is no [post_process] key"),
        "{e}"
    );
    assert!(e.contains("did you mean `bloom_intensity`?"), "{e}");
    for key in ["exposure", "vignette_intensity", "fog_color", "tear_iris"] {
        assert!(e.contains(key), "lists {key}: {e}");
    }
    // An effect's name gets its own keys first
    let e = asks("set_post(\"vignette\", 0.4)").unwrap_err();
    assert!(
        e.contains(
            "`vignette` is an effect, not a [post_process] key: its keys are \
             vignette_enabled, vignette_intensity, vignette_smoothness"
        ),
        "{e}"
    );
}

#[test]
fn a_value_of_the_wrong_kind_is_a_script_error_naming_the_key() {
    let e = asks("set_post(\"fog_color\", 0.5)").unwrap_err();
    assert!(
        e.contains("set_post: `fog_color` is three numbers: [r, g, b], not 0.5"),
        "{e}"
    );
    let e = asks("set_post(\"fxaa\", 1)").unwrap_err();
    assert!(e.contains("`fxaa` is a bool: true or false"), "{e}");
    let e = asks("set_post(\"ssao_samples\", 2.5)").unwrap_err();
    assert!(e.contains("`ssao_samples` is a whole number"), "{e}");
    let e = asks("set_post(\"exposure\", ())").unwrap_err();
    assert!(e.contains("`exposure` can't take a ()"), "{e}");
    // and asks for nothing
    let engine = ScriptEngine::new();
    let _ = engine.rhai_engine().eval::<()>("set_post(\"fxaa\", 1)");
    assert!(crate::lock_or_recover(&engine.ctx).requests.post.is_empty());
}

#[test]
fn the_old_setters_are_listed_deprecated_and_set_post_is_not() {
    let engine = ScriptEngine::new();
    let registry = engine.api_registry();
    let entry = |name: &str| {
        registry
            .entries()
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("{name} is declared"))
            .clone()
    };
    assert_eq!(entry("set_post").deprecated, None);
    for (old, _) in OLD_AND_NEW {
        let name = old.split('(').next().unwrap();
        let note = entry(name)
            .deprecated
            .unwrap_or_else(|| panic!("{name} is deprecated"));
        assert!(note.contains("set_post(\""), "{name}: {note}");
    }
}
