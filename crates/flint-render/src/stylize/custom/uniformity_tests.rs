//! #573: derivatives under a branch on the pixel, which the browser refuses
//! and naga lets through, are warned about at their line: held to what
//! Chrome's compiler said of each case ([`CHROME_CASES`]).

use super::uniformity::derivatives_under_branches;
use super::*;

/// Every case's style: one param and one option, as the cases were
/// composed for Chrome.
fn case_source() -> StyleSource {
    let def: ShaderStyleDef = toml::from_str(
        r#"
        version = 1
        name = "case"
        shader = "case.wgsl"
        [params.x]
        default = 0.5
        min = 0.0
        max = 1.0
        [options.mode]
        default = "a"
        choices = ["a", "b"]
        "#,
    )
    .unwrap();
    def.check(Path::new("styles/case.shader.toml")).unwrap()
}

fn derivative_warnings(user: &str) -> Vec<String> {
    let report = check(&case_source(), user).unwrap_or_else(|e| panic!("{e}\n{user}"));
    report
        .warnings
        .into_iter()
        .filter(|w| w.contains("derivatives need uniform control flow"))
        .collect()
}

/// Chrome 157 refused 9 of the 17 cases and compiled 8; the check warns
/// about exactly those 9, each at the line and column Tint named, and
/// says nothing of the 8 (a constant loop, a branch on a uniform, a helper
/// marked `@diagnostic(off, derivative_uniformity)`, a loop some pixels
/// leave early with the derivative after it...).
#[test]
fn the_check_warns_where_chrome_refused_and_nowhere_else() {
    for (name, user, refused) in CHROME_CASES {
        let warnings = derivative_warnings(user);
        match refused {
            Some((line, col, builtin)) => {
                assert_eq!(warnings.len(), 1, "{name}: {warnings:#?}");
                let w = &warnings[0];
                assert!(
                    w.contains(&format!("case.wgsl:{line}:{col}: warning: ")),
                    "{name}: Chrome said {line}:{col}\n{w}"
                );
                assert!(w.contains(&format!("`{builtin}`")), "{name}: {w}");
                assert!(w.contains("\n    hint: "), "{name}: {w}");
            }
            None => assert!(warnings.is_empty(), "{name}: Chrome took it\n{warnings:#?}"),
        }
    }
}

/// The frost case from #415 reads as the issue asks: the builtin, the
/// branch's line, the quoted line, and the way out.
#[test]
fn the_frost_case_names_its_branch_and_the_fix() {
    let frost = CHROME_CASES.iter().find(|c| c.0 == "frost").unwrap().1;
    let w = &derivative_warnings(frost)[0];
    assert!(
        w.contains("`fwidth` under the `if` at line 3, whose condition can differ"),
        "{w}"
    );
    assert!(
        w.contains("refuses the style") && w.contains("draws natively"),
        "{w}"
    );
    assert!(
        w.contains("\n    4 |         let w = fwidth(p.uv.x * 40.0);"),
        "{w}"
    );
    assert!(w.contains("hint: take `fwidth` before the `if`"), "{w}");
    assert!(!w.contains("`params` counts"), "{w}");
}

/// A branch on a parameter is a branch on the pixel in the browser:
/// `params` is the prelude's `var<private>`. The warning says so.
#[test]
fn a_branch_on_a_parameter_says_params_is_per_pixel() {
    for name in ["on_param", "switch_option"] {
        let user = CHROME_CASES.iter().find(|c| c.0 == name).unwrap().1;
        let w = &derivative_warnings(user)[0];
        assert!(
            w.contains("(`params` counts: it is per pixel)"),
            "{name}: {w}"
        );
    }
}

/// A helper that takes `fwidth` in its own uniform flow, called under a
/// branch: the warning is at the derivative (where Chrome puts it) and
/// names the call that put it there, which is what to move.
#[test]
fn a_helpers_derivative_names_the_call_under_the_branch() {
    let user = CHROME_CASES
        .iter()
        .find(|c| c.0 == "helper_called")
        .unwrap()
        .1;
    let w = &derivative_warnings(user)[0];
    assert!(
        w.contains("`fwidth` in `edge`, which is called at line 8 under the `if` at line 7"),
        "{w}"
    );
    assert!(w.contains("hint: call `edge` before the `if`"), "{w}");
}

/// A derivative in code the user didn't write (a prelude helper without
/// the diagnostic) is told at the user's call that reaches it.
#[test]
fn a_derivative_out_of_sight_is_told_at_the_call() {
    let text = "fn helper(f: f32) -> f32 {\n    return fwidth(f);\n}\n\
                // user\n\
                fn mine(x: f32) -> f32 {\n    if (x > 0.5) {\n        return helper(x);\n    }\n    return 0.0;\n}\n\
                @fragment\n\
                fn fs_main(@location(0) v: f32) -> @location(0) vec4<f32> {\n    return vec4<f32>(mine(v));\n}\n";
    let module = naga::front::wgsl::parse_str(text).unwrap();
    let user = text.find("// user").unwrap()..text.find("@fragment").unwrap();
    let in_user = |span: naga::Span| span.to_range().is_some_and(|r| user.contains(&r.start));
    let found = derivatives_under_branches(&module, &in_user);
    assert_eq!(found.len(), 1, "{found:#?}");
    let f = &found[0];
    assert_eq!(f.what, "fwidth");
    assert_eq!(f.at.location(text).line_number, 7, "at the call in `mine`");
    let (name, inner) = f.via.as_ref().expect("via the helper");
    assert_eq!(name, "helper");
    assert_eq!(inner.location(text).line_number, 2, "the helper's own line");
    assert_eq!(f.cause.why, super::uniformity::Why::If);
    assert_eq!(f.cause.span.location(text).line_number, 6);
}

/// The templates `flint style new` writes warn about nothing (and the
/// shipped examples, which take `fwidth` before any branch, are held by
/// `example_styles_warn_about_nothing`).
#[test]
fn the_templates_take_no_derivative_under_a_branch() {
    for stage in [StyleStage::Display, StyleStage::Hdr] {
        let inputs = Inputs {
            depth: true,
            mask: true,
            edges: true,
        };
        let t = template("utest_uniform", stage, inputs);
        let source = parse_source(&t.toml, Path::new("styles/utest_uniform.shader.toml")).unwrap();
        let report = check(&source, &t.wgsl).unwrap_or_else(|e| panic!("{e}"));
        assert!(
            !report
                .warnings
                .iter()
                .any(|w| w.contains("uniform control flow")),
            "{:?}",
            report.warnings
        );
    }
}

/// The cases #573 compiled in Chrome 157.0.0.0 (`createShaderModule` and
/// `getCompilationInfo` on each composed module): the name, the user's
/// WGSL, and where Tint refused it (line and column in the user's file,
/// and the builtin it named), or `None` where it compiled.
type ChromeCase = (
    &'static str,
    &'static str,
    Option<(usize, u32, &'static str)>,
);

pub(super) const CHROME_CASES: &[ChromeCase] = &[
    // #415's frost: fwidth under a branch on the pixel's luma
    (
        "frost",
        r#"fn style(p: StylePixel) -> vec4<f32> {
    var c = p.color.rgb;
    if (flint_luma(p.color.rgb) > 0.6) {
        let w = fwidth(p.uv.x * 40.0);
        c = mix(c, vec3<f32>(1.0), clamp(w, 0.0, 1.0));
    }
    return vec4<f32>(c, p.color.a);
}
"#,
        Some((4, 17, "fwidth")),
    ),
    // the fix: fwidth first, the branch after
    (
        "hoisted",
        r#"fn style(p: StylePixel) -> vec4<f32> {
    var c = p.color.rgb;
    let w = fwidth(p.uv.x * 40.0);
    if (flint_luma(p.color.rgb) > 0.6) {
        c = mix(c, vec3<f32>(1.0), clamp(w, 0.0, 1.0));
    }
    return vec4<f32>(c, p.color.a);
}
"#,
        None,
    ),
    // fwidth under a branch on a parameter (params is var<private>)
    (
        "on_param",
        r#"fn style(p: StylePixel) -> vec4<f32> {
    var c = p.color.rgb;
    if (params.x > 0.5) {
        c = c * fwidth(p.uv.x * 40.0);
    }
    return vec4<f32>(c, p.color.a);
}
"#,
        Some((4, 17, "fwidth")),
    ),
    // fwidth under a branch on flint.time (a uniform)
    (
        "on_uniform",
        r#"fn style(p: StylePixel) -> vec4<f32> {
    var c = p.color.rgb;
    if (flint.time > 1.0) {
        c = c * fwidth(p.uv.x * 40.0);
    }
    return vec4<f32>(c, p.color.a);
}
"#,
        None,
    ),
    // dpdx in a for with a constant bound
    (
        "const_loop",
        r#"fn style(p: StylePixel) -> vec4<f32> {
    var acc = 0.0;
    for (var i = 0; i < 4; i++) {
        acc += dpdx(p.uv.x * f32(i));
    }
    return vec4<f32>(p.color.rgb * acc, p.color.a);
}
"#,
        None,
    ),
    // dpdx in a for bounded by a parameter
    (
        "param_loop",
        r#"fn style(p: StylePixel) -> vec4<f32> {
    var acc = 0.0;
    for (var i = 0; i < i32(params.x * 8.0); i++) {
        acc += dpdx(p.uv.x * f32(i));
    }
    return vec4<f32>(p.color.rgb * acc, p.color.a);
}
"#,
        Some((4, 16, "dpdx")),
    ),
    // fwidth after a return only some pixels take
    (
        "early_return",
        r#"fn style(p: StylePixel) -> vec4<f32> {
    if (p.uv.x > 0.5) {
        return p.color;
    }
    let w = fwidth(p.uv.y * 40.0);
    return vec4<f32>(p.color.rgb * w, p.color.a);
}
"#,
        Some((5, 13, "fwidth")),
    ),
    // fwidth after a loop some pixels leave early
    (
        "after_break",
        r#"fn style(p: StylePixel) -> vec4<f32> {
    var n = 0;
    for (var i = 0; i < 8; i++) {
        if (p.color.r > f32(i) * 0.1) { break; }
        n += 1;
    }
    let w = fwidth(p.uv.y * 40.0);
    return vec4<f32>(p.color.rgb * w * f32(n), p.color.a);
}
"#,
        None,
    ),
    // a helper takes fwidth, called under a pixel branch
    (
        "helper_called",
        r#"fn edge(f: f32) -> f32 {
    return clamp(fract(f) / max(fwidth(f), 1e-4), 0.0, 1.0);
}

fn style(p: StylePixel) -> vec4<f32> {
    var c = p.color.rgb;
    if (p.color.g > 0.3) {
        c = c * edge(p.uv.x * 20.0);
    }
    return vec4<f32>(c, p.color.a);
}
"#,
        Some((2, 33, "fwidth")),
    ),
    // the same helper with @diagnostic(off, derivative_uniformity)
    (
        "helper_diag",
        r#"@diagnostic(off, derivative_uniformity)
fn edge(f: f32) -> f32 {
    return clamp(fract(f) / max(fwidth(f), 1e-4), 0.0, 1.0);
}

fn style(p: StylePixel) -> vec4<f32> {
    var c = p.color.rgb;
    if (p.color.g > 0.3) {
        c = c * edge(p.uv.x * 20.0);
    }
    return vec4<f32>(c, p.color.a);
}
"#,
        None,
    ),
    // a helper branches on its argument; style passes a constant
    (
        "arg_uniform",
        r#"fn soft(k: f32, f: f32) -> f32 {
    if (k > 0.5) {
        return fwidth(f);
    }
    return 0.0;
}

fn style(p: StylePixel) -> vec4<f32> {
    return vec4<f32>(p.color.rgb * soft(1.0, p.uv.x), p.color.a);
}
"#,
        None,
    ),
    // the same helper, passed the pixel's red
    (
        "arg_pixel",
        r#"fn soft(k: f32, f: f32) -> f32 {
    if (k > 0.5) {
        return fwidth(f);
    }
    return 0.0;
}

fn style(p: StylePixel) -> vec4<f32> {
    return vec4<f32>(p.color.rgb * soft(p.color.r, p.uv.x), p.color.a);
}
"#,
        Some((3, 16, "fwidth")),
    ),
    // a helper nothing calls branches on its argument
    (
        "uncalled",
        r#"fn soft(k: f32, f: f32) -> f32 {
    if (k > 0.5) {
        return fwidth(f);
    }
    return 0.0;
}

fn style(p: StylePixel) -> vec4<f32> {
    return p.color;
}
"#,
        None,
    ),
    // textureSample (implicit derivatives) under a pixel branch
    (
        "sample_branch",
        r#"fn style(p: StylePixel) -> vec4<f32> {
    if (p.color.b > 0.5) {
        return textureSample(flint_input, flint_sampler, p.uv);
    }
    return p.color;
}
"#,
        Some((3, 16, "textureSample")),
    ),
    // dpdy in a switch on an option
    (
        "switch_option",
        r#"fn style(p: StylePixel) -> vec4<f32> {
    var c = p.color.rgb;
    switch (params.mode) {
        case FLINT_MODE_B: { c = c * dpdy(p.uv.y); }
        default: {}
    }
    return vec4<f32>(c, p.color.a);
}
"#,
        Some((4, 38, "dpdy")),
    ),
    // a var set under a pixel branch, then branched on
    (
        "var_pixel",
        r#"fn style(p: StylePixel) -> vec4<f32> {
    var k = 0.0;
    if (p.uv.x > 0.5) { k = 1.0; }
    var c = p.color.rgb;
    if (k > 0.5) { c = c * fwidth(p.uv.y); }
    return vec4<f32>(c, p.color.a);
}
"#,
        Some((5, 28, "fwidth")),
    ),
    // a var set from flint.time, then branched on
    (
        "var_uniform",
        r#"fn style(p: StylePixel) -> vec4<f32> {
    var k = 0.0;
    k = flint.time;
    var c = p.color.rgb;
    if (k > 0.5) { c = c * fwidth(p.uv.y); }
    return vec4<f32>(c, p.color.a);
}
"#,
        None,
    ),
];
