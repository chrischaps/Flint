use super::*;
use crate::gpu_counts::CountedDevice;

fn source(toml_text: &str, file: &str) -> Result<StyleSource, String> {
    let def: ShaderStyleDef = toml::from_str(toml_text).map_err(|e| e.to_string())?;
    def.check(Path::new(file))
}

/// #396: a shader that reads its own blend opts out of the framework's
/// cross-fade; a mention in a comment doesn't count.
#[test]
fn a_shader_calling_flint_style_blend_blends_itself() {
    assert!(calls_style_blend(
        "fn style(p: StylePixel) -> vec4<f32> {\n    return p.color * flint_style_blend(p.uv);\n}\n"
    ));
    assert!(!calls_style_blend(
        "// no flint_style_blend here: the framework fades me\nfn style(p: StylePixel) -> vec4<f32> {\n    return p.color;\n}\n"
    ));
}

fn duotone() -> StyleSource {
    source(
        r#"
        version = 1
        name = "duotone"
        shader = "duotone.wgsl"
        inputs = ["depth"]
        [params.mix]
        default = 1.0
        min = 0.0
        max = 1.0
        [params.ink_r]
        default = 0.1
        min = 0.0
        max = 1.0
        "#,
        "styles/duotone.shader.toml",
    )
    .unwrap()
}

#[test]
fn descriptions_are_checked() {
    let ok = duotone();
    assert_eq!(ok.stage, StyleStage::Display);
    assert!(ok.inputs.depth && !ok.inputs.mask);
    assert_eq!(ok.label, "Duotone");
    assert_eq!(ok.wgsl_path, Path::new("styles").join("duotone.wgsl"));
    let names: Vec<&str> = ok.params.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["ink_r", "mix"], "uniform order is by name");

    let base = "version = 1\nname = \"x\"\nshader = \"x.wgsl\"\n";
    let file = "styles/x.shader.toml";
    let err = |extra: &str| source(&format!("{base}{extra}"), file).unwrap_err();
    assert!(source(base, "styles/y.shader.toml")
        .unwrap_err()
        .contains("must match"));
    assert!(err("stage = \"late\"\n").contains("\"display\" or \"hdr\""));
    assert!(err("inputs = [\"normals\"]\n").contains("unknown input"));
    assert!(err("[params.flint_x]\ndefault = 0\nmin = 0\nmax = 1\n").contains("flint"));
    // Every table's own keys (#299)
    assert!(err("[params.order]\ndefault = 0\nmin = 0\nmax = 1\n").contains("taken"));
    assert!(err("[params.a]\ndefault = 2\nmin = 0\nmax = 1\n").contains("outside"));
    assert!(err("wobble = 1\n").contains("unknown field"));
    // feeds_bloom (#300): hdr styles only, off by default
    assert!(err("feeds_bloom = true\n").contains("stage = \"hdr\""));
    assert!(!source(base, file).unwrap().feeds_bloom);
    assert!(
        source(
            &format!("{base}stage = \"hdr\"\nfeeds_bloom = true\n"),
            file
        )
        .unwrap()
        .feeds_bloom
    );
    let up = "version = 1\nname = \"x\"\nshader = \"../x.wgsl\"\n";
    assert!(source(up, file).unwrap_err().contains("beside it"));
    let v2 = "version = 2\nname = \"x\"\nshader = \"x.wgsl\"\n";
    assert!(source(v2, file).unwrap_err().contains("version"));
    let builtin = "version = 1\nname = \"ascii\"\nshader = \"x.wgsl\"\n";
    assert!(source(builtin, "styles/ascii.shader.toml")
        .unwrap_err()
        .contains("built-in"));
}

/// #538: `reach` is a mask-reading style's, in points, and becomes pixels
/// rounded out with one for the filter.
#[test]
fn a_reach_is_a_per_entity_styles_and_counts_in_points() {
    let base = "version = 1\nname = \"x\"\nshader = \"x.wgsl\"\n";
    let file = "styles/x.shader.toml";
    let read = |extra: &str| source(&format!("{base}{extra}"), file);
    assert_eq!(read("").unwrap().reach, None);
    assert_eq!(
        read("inputs = [\"mask\"]\nreach = 3\n").unwrap().reach,
        Some(3.0)
    );
    assert!(read("reach = 3\n").unwrap_err().contains("needs inputs"));
    assert!(read("inputs = [\"mask\"]\nreach = -1\n")
        .unwrap_err()
        .contains("0..512"));
    assert!(read("inputs = [\"mask\"]\nreach = 1e9\n")
        .unwrap_err()
        .contains("0..512"));
    assert_eq!(super::pass::reach_pixels(0.0, 1.0), 1);
    assert_eq!(super::pass::reach_pixels(3.0, 1.5), 6);
    assert_eq!(super::pass::reach_pixels(2.5, 2.0), 6);
    assert_eq!(super::pass::reach_pixels(2.0, f32::NAN), 3);
}

#[test]
fn a_mask_reading_style_is_per_entity() {
    let base = "version = 1\nname = \"x\"\nshader = \"x.wgsl\"\n";
    let file = "styles/x.shader.toml";
    assert_eq!(source(base, file).unwrap().component, None);
    let masked = source(&format!("{base}inputs = [\"mask\"]\n"), file).unwrap();
    assert_eq!(masked.component.as_deref(), Some("x_style"));
}

#[test]
fn the_settings_table_holds_a_row_a_variant() {
    // Two params: a 32-byte row, as many rows as the frame has (#740)
    assert_eq!(params_size(2), 32);
    let rows = vec![vec![0.5, 1.0], vec![0.25, 2.0]];
    let words = row_words(2, &rows);
    assert_eq!(words.len(), 2 * 8);
    let f = |i: usize| f32::from_bits(words[i]);
    assert_eq!((f(0), f(1)), (0.5, 1.0), "row 0, the scene's");
    assert_eq!((f(8), f(9)), (0.25, 2.0), "row 1 at the next 32 bytes");
    assert_eq!(row_words(2, &rows[..1]).len(), 8, "row 0 alone");
    // The slots are the table's own buffer, declared only with the mask
    let masked = source(
        "version = 1\nname = \"x\"\nshader = \"x.wgsl\"\ninputs = [\"mask\"]\n",
        "styles/x.shader.toml",
    )
    .unwrap();
    assert!(prelude(&masked).contains("var<storage, read> flint_slots: array<u32>;"));
    let bare = source(
        "version = 1\nname = \"x\"\nshader = \"x.wgsl\"\n",
        "styles/x.shader.toml",
    )
    .unwrap();
    assert!(!prelude(&bare).contains("flint_slots"));
    assert!(prelude(&bare).contains("var<storage, read> flint_params: FlintParamTable;"));
}

#[test]
fn neighbours_settings_are_readable() {
    let s = source(
        "version = 1\nname = \"x\"\nshader = \"x.wgsl\"\ninputs = [\"mask\"]\n\
         [params.glow]\ndefault = 0.5\nmin = 0.0\nmax = 1.0\n",
        "styles/x.shader.toml",
    )
    .unwrap();
    let user = "fn style(p: StylePixel) -> vec4<f32> {\n    \
                let n = flint_params_at(p.uv + flint.texel);\n    \
                return p.color * (params.glow + n.glow);\n}\n";
    let report = check(&s, user).unwrap();
    assert_eq!(report.texture_reads, 1, "flint_params_at reads the mask");
}

const HEAD: &str = "version = 1\nname = \"x\"\n";
const FILE: &str = "styles/x.shader.toml";

fn passes(body: &str) -> Result<StyleSource, String> {
    source(&format!("{HEAD}{body}"), FILE)
}

/// #412: `[[passes]]` are checked when the description is read, and
/// each error names the passes.
#[test]
fn passes_are_checked_and_errors_name_them() {
    let ok = passes(
        r#"
        [[passes]]
        name = "down"
        shader = "down.wgsl"
        target = "small"
        scale = 0.5
        [[passes]]
        shader = "blur.wgsl"
        target = "blurred"
        format = "rgba8"
        scale = 0.5
        reads = ["small"]
        [[passes]]
        name = "out"
        shader = "out.wgsl"
        reads = ["blurred", "small"]
        "#,
    )
    .unwrap();
    let names: Vec<&str> = ok.passes.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["down", "pass2", "out"], "a default name by place");
    assert_eq!(ok.targets.len(), 2);
    assert_eq!(ok.targets[1].format, TargetFormat::Rgba8);
    assert_eq!(ok.targets[0].size(1920, 1080), [960, 540]);
    assert_eq!(ok.passes[2].reads.len(), 2);
    assert!(ok.passes[2].reads.iter().all(|r| !r.previous));
    assert!(ok.notes.is_empty(), "{:?}", ok.notes);
    let files: Vec<_> = ok.wgsl_paths().collect();
    assert_eq!(files[1], Path::new("styles").join("blur.wgsl"));

    let err = |body: &str| passes(body).unwrap_err();
    let pass = |name: &str, extra: &str| {
        format!("[[passes]]\nname = \"{name}\"\nshader = \"{name}.wgsl\"\n{extra}\n")
    };
    // A read before the write names both passes
    let e = err(&format!(
        "{}{}{}",
        pass("a", "target = \"ta\"\nreads = [\"tb\"]"),
        pass("b", "target = \"tb\""),
        pass("c", "reads = [\"ta\", \"tb\"]"),
    ));
    assert!(
        e.contains("pass \"a\" reads target \"tb\" before pass \"b\" writes it"),
        "{e}"
    );
    // A cycle names its passes in order
    let e = err(&format!(
        "{}{}{}",
        pass("a", "target = \"ta\"\nreads = [\"tb\"]"),
        pass("b", "target = \"tb\"\nreads = [\"ta\"]"),
        pass("c", "reads = [\"tb\"]"),
    ));
    assert!(
        e.contains("a cycle") && e.contains("\"a\" -> \"b\" -> \"a\""),
        "{e}"
    );
    // A pass reading its own target without history
    let e = err(&format!(
        "{}{}",
        pass("a", "target = \"ta\"\nreads = [\"ta\"]"),
        pass("b", "reads = [\"ta\"]"),
    ));
    assert!(e.contains("which it writes: a cycle"), "{e}");
    // ... is last frame's with history
    let h = passes(&format!(
        "{}{}",
        pass("a", "target = \"ta\"\nhistory = true\nreads = [\"ta\"]"),
        pass("b", "reads = [\"ta\"]"),
    ))
    .unwrap();
    assert!(h.passes[0].reads[0].previous && !h.passes[1].reads[0].previous);
    assert!(h.has_history());
    // Shape: targets on every pass but the last, none on it
    assert!(err(&pass("a", "target = \"t\"")).contains("is the last"));
    assert!(err(&format!("{}{}", pass("a", ""), pass("b", ""))).contains("writes nothing"));
    assert!(err(&pass("a", "scale = 0.5")).contains("but no target"));
    assert!(err(&format!(
        "{}{}",
        pass("a", "target = \"t\"\nformat = \"rgb565\""),
        pass("b", "")
    ))
    .contains("rgba16f, rgba8 or r32f"));
    assert!(err(&format!(
        "{}{}",
        pass("a", "target = \"t\"\nscale = 2"),
        pass("b", "")
    ))
    .contains("0.0625..1"));
    assert!(err(&format!(
        "{}{}",
        pass("a", "target = \"t\""),
        pass("b", "reads = [\"u\"]")
    ))
    .contains("which no pass writes"));
    assert!(err(&format!(
        "{}{}{}",
        pass("a", "target = \"t\""),
        pass("b", "target = \"t\""),
        pass("c", "")
    ))
    .contains("both write target \"t\""));
    assert!(err(&format!("{}{}", pass("a", ""), pass("a", ""))).contains("two passes"));
    assert!(err("shader = \"x.wgsl\"\n[[passes]]\nshader = \"y.wgsl\"\n").contains("not both"));
    assert!(err("").contains("no shader"));
    assert!(err(&pass("a", "").replace("a.wgsl", "../a.wgsl")).contains("beside it"));
    assert!(
        err(&format!("{}reads = [\"t\"]\nwobble = 1\n", pass("a", ""))).contains("unknown field")
    );
    // A target nothing reads is a note, not an error
    let unread = passes(&format!("{}{}", pass("a", "target = \"t\""), pass("b", ""))).unwrap();
    assert!(
        unread.notes[0].contains("no pass reads it"),
        "{:?}",
        unread.notes
    );
}

/// #412: at most four targets, and together no more than four
/// full-size rgba16f (history counting twice).
#[test]
fn targets_are_capped_in_count_and_memory() {
    let chain = |targets: &[&str]| {
        let mut body = String::new();
        for (i, t) in targets.iter().enumerate() {
            body.push_str(&format!(
                "[[passes]]\nshader = \"p{i}.wgsl\"\ntarget = \"t{i}\"\n{t}\n"
            ));
        }
        let reads: Vec<String> = (0..targets.len()).map(|i| format!("\"t{i}\"")).collect();
        body.push_str(&format!(
            "[[passes]]\nshader = \"last.wgsl\"\nreads = [{}]\n",
            reads.join(", ")
        ));
        passes(&body)
    };
    assert!(
        chain(&[""; 4]).is_ok(),
        "four full-size rgba16f: the budget"
    );
    assert!(chain(&[""; 5]).unwrap_err().contains("at most 4"));
    assert!(
        chain(&["history = true", "", ""]).is_ok(),
        "16 + 8 + 8: on the line"
    );
    let e = chain(&["history = true", "", "", "format = \"rgba8\""]).unwrap_err();
    assert!(e.contains("budget is 32"), "{e}");
    assert!(chain(&[
        "history = true",
        "",
        "scale = 0.5",
        "format = \"r32f\"\nscale = 0.5"
    ])
    .is_ok());
}

/// #412: every pass's prelude validates, whatever it reads: each
/// format, this frame's or last frame's, with and without other inputs.
#[test]
fn pass_preludes_validate_for_every_read() {
    for inputs in ["[]", "[\"depth\", \"mask\", \"edges\"]"] {
        for stage in ["display", "hdr"] {
            let s = passes(&format!(
                "stage = \"{stage}\"\ninputs = {inputs}\n\
                 [[passes]]\nshader = \"a.wgsl\"\ntarget = \"h\"\nhistory = true\nreads = [\"h\"]\n\
                 [[passes]]\nshader = \"b.wgsl\"\ntarget = \"f\"\nformat = \"r32f\"\nscale = 0.5\nreads = [\"h\"]\n\
                 [[passes]]\nshader = \"c.wgsl\"\ntarget = \"e\"\nformat = \"rgba8\"\nreads = [\"f\", \"h\"]\n\
                 [[passes]]\nshader = \"d.wgsl\"\nreads = [\"e\", \"f\", \"h\"]\n"
            ))
            .unwrap();
            for i in 0..s.passes.len() {
                check_pass(&s, i, PASSTHROUGH)
                    .unwrap_or_else(|e| panic!("{stage} {inputs} pass {i}: {e}"));
            }
            // The helpers of what the last pass reads
            let user = "fn style(p: StylePixel) -> vec4<f32> {\n    \
                        return flint_target_e(p.uv) + flint_target_f(p.uv) \
                        + flint_target_h_load(vec2<i32>(p.pos)) \
                        + vec4<f32>(flint_target_f_size(), flint.pass_size) * flint.history;\n}\n";
            let report = check_pass(&s, 3, user).unwrap();
            assert_eq!(report.texture_reads, 3, "the size isn't a read");
            // A target the pass doesn't read gets a hint
            let e = check_pass(&s, 0, user).unwrap_err();
            assert!(e.contains("add \"e\" to reads of pass \"pass1\""), "{e}");
        }
    }
}

/// #412 with #413: a style's images and the targets a pass reads share
/// group 3, the images first, so neither moves the other.
#[test]
fn images_and_targets_share_group_three() {
    let s = passes(
        "[images.paper]\nfile = \"paper.png\"\n\
         [[passes]]\nshader = \"a.wgsl\"\ntarget = \"t\"\n\
         [[passes]]\nshader = \"b.wgsl\"\nreads = [\"t\"]\n",
    )
    .unwrap();
    let first = prelude_pass(&s, 0);
    assert!(first.contains("@group(3) @binding(0) var flint_image_texture_paper"));
    assert!(
        !first.contains("flint_target_t"),
        "the first pass reads no target"
    );
    let last = prelude_pass(&s, 1);
    assert!(last.contains("@group(3) @binding(0) var flint_image_texture_paper"));
    assert!(
        last.contains("@group(3) @binding(1) var flint_target_t_texture"),
        "the target after the image"
    );
    let user = "fn style(p: StylePixel) -> vec4<f32> {\n    \
                return flint_image_paper(p.uv) * flint_target_t(p.uv);\n}\n";
    check_pass(&s, 1, user).unwrap();
    check_pass(&s, 0, PASSTHROUGH).unwrap();
}

#[test]
fn params_round_up_to_vec4s_and_a_pad() {
    assert_eq!(params_size(0), 16);
    assert_eq!(params_size(1), 32);
    assert_eq!(params_size(4), 32);
    assert_eq!(params_size(5), 48);
    assert_eq!(std::mem::size_of::<FlintFrameUniforms>(), 144);
}

#[test]
fn passthrough_and_prelude_validate_for_every_shape() {
    for stage in ["display", "hdr"] {
        for inputs in [
            "[]",
            "[\"depth\"]",
            "[\"mask\"]",
            "[\"color\", \"depth\", \"mask\"]",
            "[\"edges\"]",
            "[\"depth\", \"mask\", \"edges\"]",
        ] {
            let s = source(
                &format!(
                    "version = 1\nname = \"x\"\nshader = \"x.wgsl\"\nstage = \"{stage}\"\n\
                     inputs = {inputs}\n[params.a]\ndefault = 0\nmin = 0\nmax = 1\n"
                ),
                "styles/x.shader.toml",
            )
            .unwrap();
            let report = check(&s, PASSTHROUGH).unwrap_or_else(|e| panic!("{stage} {inputs}: {e}"));
            assert_eq!(report.loops, 0);
        }
    }
}

#[test]
fn errors_point_into_the_users_file() {
    let s = duotone();
    let user = "fn style(p: StylePixel) -> vec4<f32> {\n    let x = 1.0\n    return p.color;\n}\n";
    let e = check(&s, user).unwrap_err();
    assert!(e.starts_with(&format!("{}:", shown(&s.wgsl_path))), "{e}");
    // The missing `;` is reported at the line after it (line 3)
    assert!(e.contains(":3:") || e.contains(":2:"), "{e}");
    assert!(e.contains("return p.color") || e.contains("let x"), "{e}");

    // A validation error (a type mismatch), not a parse error
    let user = "fn style(p: StylePixel) -> vec4<f32> {\n    return p.color.rgb;\n}\n";
    let e = check(&s, user).unwrap_err();
    assert!(e.contains(":2:") || e.contains("style"), "{e}");
}

#[test]
fn helpers_of_undeclared_inputs_get_a_hint() {
    let s = duotone(); // depth, no mask
    let user = "fn style(p: StylePixel) -> vec4<f32> {\n    \
                return p.color * flint_style_blend(p.uv);\n}\n";
    let e = check(&s, user).unwrap_err();
    assert!(e.contains("add \"mask\" to inputs"), "{e}");
    let user = "fn style(p: StylePixel) -> vec4<f32> {\n    \
                return p.color * flint_view_depth(p.uv);\n}\n";
    check(&s, user).unwrap();
}

#[test]
fn the_contract_is_enforced() {
    let s = duotone();
    let e = check(
        &s,
        "fn shade(p: StylePixel) -> vec4<f32> { return p.color; }\n",
    )
    .unwrap_err();
    assert!(e.contains("no `fn style"), "{e}");
    let user = "@group(3) @binding(0) var extra: texture_2d<f32>;\n\
                fn style(p: StylePixel) -> vec4<f32> { return p.color; }\n";
    let e = check(&s, user).unwrap_err();
    assert!(e.contains("declares a binding") && e.contains(":1"), "{e}");
}

#[test]
fn loops_and_reads_are_counted() {
    let s = duotone();
    let user = r#"
fn style(p: StylePixel) -> vec4<f32> {
var acc = vec4<f32>(0.0);
for (var i = 0; i < 4; i++) {
    for (var j = 0; j < 4; j++) {
        acc += flint_sample(p.uv + vec2<f32>(f32(i), f32(j)) * flint.texel);
    }
}
return acc / 16.0 * params.mix;
}
"#;
    let report = check(&s, user).unwrap();
    assert_eq!(report.loops, 2);
    assert_eq!(report.texture_reads, 1, "one call site, written out once");
    // #424: both have constant bounds, so neither needs the guard
    assert_eq!(report.bounded, 2);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.guarded, None, "nothing to guard");
}

/// #424: with the guard off, a loop whose bound isn't a constant is
/// warned about and a three-octave `for` is not; with it on (always,
/// outside tests) neither is, and only the first is guarded.
#[test]
fn only_a_loop_without_a_constant_bound_is_warned_about() {
    let s = duotone();
    let octaves = r#"
fn style(p: StylePixel) -> vec4<f32> {
var n = 0.0;
var amp = 0.5;
var uv = p.uv * 4.0;
for (var i = 0; i < 3; i++) {
    n += amp * flint_sample(uv).r;
    uv *= 2.0;
    amp *= 0.5;
}
return vec4<f32>(p.color.rgb * n * params.mix, p.color.a);
}
"#;
    let until = r#"
fn style(p: StylePixel) -> vec4<f32> {
var x = p.color.r;
loop {
    if x > 1.0 { break; }
    x += 0.1;
}
return vec4<f32>(vec3<f32>(x * params.mix), 1.0);
}
"#;
    set_loop_guard(false);
    let off = (check(&s, octaves), check(&s, until));
    set_loop_guard(true);
    let (octaves_off, until_off) = (off.0.unwrap(), off.1.unwrap());
    assert!(
        octaves_off.warnings.is_empty(),
        "{:?}",
        octaves_off.warnings
    );
    assert_eq!(until_off.warnings.len(), 1, "{:?}", until_off.warnings);
    assert!(
        until_off.warnings[0].contains("1 loop without a constant bound"),
        "{}",
        until_off.warnings[0]
    );

    let octaves_on = check(&s, octaves).unwrap();
    assert_eq!((octaves_on.loops, octaves_on.bounded), (1, 1));
    assert!(octaves_on.warnings.is_empty() && octaves_on.guarded.is_none());
    let until_on = check(&s, until).unwrap();
    assert_eq!((until_on.loops, until_on.bounded), (1, 0));
    assert!(until_on.warnings.is_empty(), "{:?}", until_on.warnings);
    let text = until_on.guarded.expect("guarded");
    assert_eq!(text.matches(&format!(">= {LOOP_LIMIT}u")).count(), 1);
}

/// #424: the example styles (frozen in the fixtures since #984) load with no warning, and their
/// constant-bound loops (orton's 13-tap blurs) are left as written;
/// radial_blur's loop runs to a parameter, so it is still guarded.
#[test]
#[allow(clippy::disallowed_methods)] // a test reads its own files off the disk
fn example_styles_warn_about_nothing() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/scenes/styles");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if !path.to_string_lossy().ends_with(".shader.toml") {
            continue;
        }
        let report = check_file(&path).unwrap_or_else(|e| panic!("{e}"));
        assert!(
            report.warnings.is_empty(),
            "{}: {:?}",
            path.display(),
            report.warnings
        );
        let name = path.file_name().unwrap().to_string_lossy();
        match &*name {
            "orton.shader.toml" => assert_eq!((report.loops, report.bounded), (2, 2)),
            "radial_blur.shader.toml" => assert_eq!((report.loops, report.bounded), (1, 0)),
            _ => {}
        }
        checked += 1;
    }
    assert!(checked >= 9, "only {checked} styles in {}", dir.display());
}

/// #411: every user loop gets a counter of its own (reset where it
/// starts) and a `break if` at [`LOOP_LIMIT`], the module still
/// validates, and the prelude's own code is left alone.
#[test]
fn every_user_loop_is_guarded() {
    let s = duotone();
    let user = r#"
fn spin(x: f32) -> f32 {
var y = x;
loop {
    y = y + 1.0;
}
return y;
}
fn style(p: StylePixel) -> vec4<f32> {
var acc = 0.0;
var k = 0;
while (true) {
    k += 1;
    if (k > 3) { continue; }
    for (var i = 0; i < 9; i++) { acc += 1.0; }
}
loop {
    acc += 1.0;
    continuing { break if acc > 2.0; }
}
return p.color * spin(acc);
}
"#;
    let report = check(&s, user).unwrap();
    assert_eq!(report.loops, 4);
    // The `for` over 9 is proven to end (#424) and left as written
    assert_eq!(report.bounded, 1);
    let text = report.guarded.expect("guarded");
    // It parses and validates as naga wrote it
    let module = naga::front::wgsl::parse_str(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .unwrap();
    let limit = format!(">= {LOOP_LIMIT}u");
    assert_eq!(text.matches(&limit).count(), 3, "{text}");
    assert_eq!(text.matches("break if").count(), 3, "{text}");
    // The user's own break_if is kept, or-ed with the guard
    assert!(text.contains("> 2f) || ("), "{text}");
    // The prelude has no loops; the written module none we didn't add
    assert_eq!(text.matches("loop {").count(), 4, "{text}");
}

#[test]
fn user_lines_map_back() {
    let s = duotone();
    let c = compose(&s, "a\nb\n");
    assert_eq!(c.user_lines, 2);
    assert_eq!(c.user_line(c.prelude_lines + 1), Some(1));
    assert_eq!(c.user_line(c.prelude_lines + 2), Some(2));
    assert_eq!(c.user_line(c.prelude_lines + 3), None);
    assert_eq!(c.user_line(1), None);
    assert_eq!(c.text.lines().nth(c.prelude_lines), Some("a"));
}

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    pollster::block_on(async {
        let instance = crate::gpu_select::instance();
        let adapter = crate::gpu_select::request_adapter(&instance, None).await?;
        adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .ok()
    })
}

/// Hot reload on a real device: a good build, a broken edit that keeps
/// the last good pipeline and reports, and a fix the poll picks up.
#[test]
fn hot_reload_keeps_the_last_good_build() {
    let mut reg = super::super::StyleRegistry::new();
    let Some((device, _queue)) = device() else {
        crate::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let dir = std::env::temp_dir().join(format!("flint-user-reload-{}", std::process::id()));
    let styles = dir.join("styles");
    std::fs::create_dir_all(&styles).unwrap();
    let wgsl = styles.join("reload.wgsl");
    std::fs::write(
        styles.join("utest_reload.shader.toml"),
        "version = 1\nname = \"utest_reload\"\nshader = \"reload.wgsl\"\ninputs = [\"depth\", \"mask\"]\n",
    )
    .unwrap();
    std::fs::write(&wgsl, PASSTHROUGH).unwrap();
    assert!(reg.load_dir(&dir).is_empty());
    let style = reg.style("utest_reload").unwrap();
    let pass = build(&StyleDevice {
        device: &device,
        descriptor: style,
        style,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
    });
    let error = || {
        reg.errors()
            .into_iter()
            .find(|(n, _)| n == "utest_reload")
            .map(|(_, e)| e)
    };
    assert_eq!(error(), None);
    assert!(pass.passes[0].build.lock().unwrap().pipeline.is_some());

    // Break it on disk; the poll notices within POLL_FRAMES frames
    std::thread::sleep(std::time::Duration::from_millis(30));
    std::fs::write(
        &wgsl,
        "fn style(p: StylePixel) -> vec4<f32> {\n    return 1.0;\n}\n",
    )
    .unwrap();
    for _ in 0..POLL_FRAMES {
        pass.poll(&device);
    }
    let e = error().expect("the broken edit is reported");
    assert!(e.contains("reload.wgsl:"), "{e}");
    assert!(
        pass.passes[0].build.lock().unwrap().pipeline.is_some(),
        "last good build kept"
    );

    // Fix it: the error clears
    std::thread::sleep(std::time::Duration::from_millis(30));
    std::fs::write(
        &wgsl,
        "fn style(p: StylePixel) -> vec4<f32> {\n    return p.color * flint_style_blend(p.uv) + flint_depth(p.uv);\n}\n",
    )
    .unwrap();
    for _ in 0..POLL_FRAMES {
        pass.poll(&device);
    }
    assert_eq!(error(), None);
    let _ = std::fs::remove_dir_all(&dir);
}

fn message(
    message_type: wgpu::CompilationMessageType,
    text: &str,
    at: Option<(u32, u32)>,
) -> wgpu::CompilationMessage {
    wgpu::CompilationMessage {
        message: text.to_string(),
        message_type,
        location: at.map(|(line_number, line_position)| wgpu::SourceLocation {
            line_number,
            line_position,
            offset: 0,
            length: 1,
        }),
    }
}

/// The browser's report names lines of the composed module; the error
/// shown names the user's file and line, quoted, as naga's does (#415).
#[test]
fn browser_messages_point_into_the_users_file() {
    use wgpu::CompilationMessageType::{Error, Warning};
    let s = duotone();
    let user = "fn style(p: StylePixel) -> vec4<f32> {\n    return p.color * glow;\n}\n";
    let c = compose(&s, user);
    let line = c.prelude_lines as u32 + 2;
    let info = wgpu::CompilationInfo {
        messages: vec![
            message(Warning, "unused variable", Some((line, 5))),
            message(Error, "unresolved identifier 'glow'", Some((line, 22))),
            message(Error, "a second one", Some((line, 1))),
        ],
    };
    let e = compiler_error(&s, &c, &info).unwrap();
    let file = shown(&s.wgsl_path);
    assert!(
        e.starts_with(&format!("{file}:2:22: unresolved identifier 'glow'")),
        "{e}"
    );
    assert!(
        e.contains("2 | ") && e.contains("return p.color * glow;"),
        "{e}"
    );
    assert!(e.contains("(and 1 more)"), "{e}");

    // Warnings alone are no error
    let info = wgpu::CompilationInfo {
        messages: vec![message(Warning, "unused variable", Some((line, 5)))],
    };
    assert_eq!(compiler_error(&s, &c, &info), None);

    // A line in the generated code, and no location at all
    let info = wgpu::CompilationInfo {
        messages: vec![message(Error, "bad param", Some((3, 1)))],
    };
    let e = compiler_error(&s, &c, &info).unwrap();
    assert!(e.contains("in the generated code, line 3"), "{e}");
    let info = wgpu::CompilationInfo {
        messages: vec![message(Error, "Getting compilation info failed", None)],
    };
    assert_eq!(
        compiler_error(&s, &c, &info).unwrap(),
        format!("{file}: Getting compilation info failed")
    );
}

/// Which pipeline this is (wgpu hashes resources by identity).
fn identity(p: &wgpu::RenderPipeline) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.hash(&mut h);
    h.finish()
}

/// A future that answers on its `polls`th poll, as a browser promise
/// settles a frame or two after the pipeline was made.
struct Late<T> {
    polls: u32,
    value: Option<T>,
}

impl<T: Unpin> std::future::Future for Late<T> {
    type Output = T;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<T> {
        self.polls -= 1;
        if self.polls == 0 {
            std::task::Poll::Ready(self.value.take().unwrap())
        } else {
            std::task::Poll::Pending
        }
    }
}

/// The web's path, driven natively: a build the compiler hasn't
/// answered for draws nothing new and reports nothing; when the answer
/// comes frames later, an error lands in `reg.errors()` with the user's
/// line and the last good build keeps drawing; a clean answer swaps
/// the new build in (#415).
#[test]
fn a_late_compiler_answer_settles_on_a_later_frame() {
    let mut reg = super::super::StyleRegistry::new();
    let Some((device, _queue)) = device() else {
        crate::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let dir = std::env::temp_dir().join(format!("flint-user-late-{}", std::process::id()));
    let styles = dir.join("styles");
    std::fs::create_dir_all(&styles).unwrap();
    std::fs::write(
        styles.join("utest_late.shader.toml"),
        "version = 1\nname = \"utest_late\"\nshader = \"late.wgsl\"\n",
    )
    .unwrap();
    std::fs::write(styles.join("late.wgsl"), PASSTHROUGH).unwrap();
    assert!(reg.load_dir(&dir).is_empty());
    let style = reg.style("utest_late").unwrap();
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let pass = build(&StyleDevice {
        device: &device,
        descriptor: style,
        style,
        format,
    });
    let error = || {
        reg.errors()
            .into_iter()
            .find(|(n, _)| n == "utest_late")
            .map(|(_, e)| e)
    };
    assert_eq!(error(), None);
    let source = pass.loaded.source.clone();
    let user = "fn style(p: StylePixel) -> vec4<f32> {\n    return p.color * glow;\n}\n";
    let composed = compose(&source, user);
    let line = composed.prelude_lines as u32 + 2;
    let (_, made) = pipeline(
        &device,
        &pass.passes[0].layout,
        &compose(&source, PASSTHROUGH).text,
        format,
    );
    let first = pass.passes[0]
        .build
        .lock()
        .unwrap()
        .pipeline
        .as_ref()
        .map(identity)
        .unwrap();
    let made_id = identity(&made);
    pass.passes[0].build.lock().unwrap().pending = Some(Pending {
        pipeline: made,
        composed: composed.clone(),
        info: Settling::new(Late {
            polls: 3,
            value: Some(wgpu::CompilationInfo {
                messages: vec![message(
                    wgpu::CompilationMessageType::Error,
                    "unresolved identifier 'glow'",
                    Some((line, 22)),
                )],
            }),
        }),
        scope: Settling::new(Late {
            polls: 2,
            value: Some(None),
        }),
    });
    let settle = || {
        let mut build = pass.passes[0].build.lock().unwrap();
        pass.settle(0, &mut build);
        build.pending.is_some()
    };
    assert!(settle(), "frame 1: still waiting");
    assert!(settle(), "frame 2: the scope is in, the report isn't");
    assert_eq!(error(), None, "nothing to say until both answer");
    assert!(!settle(), "frame 3: settled");
    let e = error().expect("the browser's error is reported");
    assert!(
        e.contains("late.wgsl:2:22: unresolved identifier 'glow'"),
        "{e}"
    );
    assert_eq!(
        pass.passes[0]
            .build
            .lock()
            .unwrap()
            .pipeline
            .as_ref()
            .map(identity)
            .unwrap(),
        first,
        "the last good build keeps drawing"
    );

    // A clean answer, a frame late: the new build draws, the error clears
    let (_, made) = pipeline(
        &device,
        &pass.passes[0].layout,
        &compose(&source, PASSTHROUGH).text,
        format,
    );
    let made_id_2 = identity(&made);
    assert_ne!(made_id, made_id_2);
    pass.passes[0].build.lock().unwrap().pending = Some(Pending {
        pipeline: made,
        composed,
        info: Settling::new(Late {
            polls: 2,
            value: Some(wgpu::CompilationInfo { messages: vec![] }),
        }),
        scope: Settling::new(std::future::ready(None)),
    });
    assert!(settle());
    assert!(error().is_some(), "the old error stands while waiting");
    assert!(!settle());
    assert_eq!(error(), None);
    assert_eq!(
        pass.passes[0]
            .build
            .lock()
            .unwrap()
            .pipeline
            .as_ref()
            .map(identity)
            .unwrap(),
        made_id_2
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn load_dir_registers_styles_and_keeps_broken_ones_as_passthroughs() {
    let mut reg = super::super::StyleRegistry::new();
    let dir = std::env::temp_dir().join(format!("flint-user-styles-{}", std::process::id()));
    let styles = dir.join("styles");
    std::fs::create_dir_all(&styles).unwrap();
    std::fs::write(
        styles.join("utest_good.shader.toml"),
        "version = 1\nname = \"utest_good\"\nshader = \"good.wgsl\"\n\
         [params.k]\ndefault = 0.5\nmin = 0\nmax = 1\nlabel = \"Kay\"\n",
    )
    .unwrap();
    std::fs::write(styles.join("good.wgsl"), PASSTHROUGH).unwrap();
    std::fs::write(
        styles.join("utest_bad.shader.toml"),
        "version = 1\nname = \"utest_bad\"\nshader = \"bad.wgsl\"\n",
    )
    .unwrap();
    std::fs::write(
        styles.join("bad.wgsl"),
        "fn style(p: StylePixel) -> f32 {\n  return 1.0;\n}\n",
    )
    .unwrap();
    std::fs::write(styles.join("notes.toml"), "ignored = true\n").unwrap();

    let messages = reg.load_dir(&dir);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("bad.wgsl"), "{messages:?}");
    let good = reg.find("utest_good").expect("registered");
    assert_eq!(good.param(&Default::default(), "k"), 0.5);
    assert_eq!(good.label_for("k"), "Kay");
    assert!(
        reg.find("utest_bad").is_some(),
        "a broken shader still registers"
    );
    assert!(reg.errors().iter().any(|(n, _)| n == "utest_bad"));
    assert!(!reg.errors().iter().any(|(n, _)| n == "utest_good"));
    // A scene table naming it reads its params now
    let tables: BTreeMap<String, toml::Table> = toml::from_str("[utest_good]\nk = 0.25\n").unwrap();
    let (settings, warnings) = super::super::StyleSettings::from_tables(&tables, &reg);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(settings.param("utest_good", "k"), Some(0.25));
    // Loading again is a no-op. The registry is this test's own (#742),
    // so it holds the built-ins and these two, nothing a test beside it
    // loaded
    assert_eq!(reg.len(), super::super::builtins().len() + 2);
    assert!(reg.load_dir(&dir).is_empty());
    assert_eq!(reg.len(), super::super::builtins().len() + 2);
    let _ = std::fs::remove_dir_all(&dir);
}

fn game_pass(_: &super::super::StyleDevice) -> Box<dyn super::super::StylePass> {
    unreachable!("never built")
}

static GAME_TAKEN: StyleDescriptor = StyleDescriptor::new(
    "utest_game_owned",
    "Game owned",
    StyleStage::Display,
    game_pass,
);

/// #419: a `.shader.toml` can't take a name a game registered in Rust,
/// and the game's descriptor stays where it was.
#[test]
fn a_user_style_cannot_take_a_game_styles_name() {
    let mut reg = super::super::StyleRegistry::new();
    reg.register(&GAME_TAKEN).unwrap();
    let dir = std::env::temp_dir().join(format!("flint-game-owned-{}", std::process::id()));
    let styles = dir.join("styles");
    std::fs::create_dir_all(&styles).unwrap();
    std::fs::write(
        styles.join("utest_game_owned.shader.toml"),
        "version = 1\nname = \"utest_game_owned\"\nshader = \"s.wgsl\"\n",
    )
    .unwrap();
    std::fs::write(styles.join("s.wgsl"), PASSTHROUGH).unwrap();
    let messages = reg.load_dir(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("is a style the game registers in Rust")),
        "{messages:?}"
    );
    let found = reg.find("utest_game_owned").unwrap();
    assert!(std::ptr::eq(found, &GAME_TAKEN));
    // Nor can a reload poll put one in its place
    let reload = reg.reload_changed(&dir);
    assert!(!reload.changed(), "{reload:?}");
    assert!(std::ptr::eq(
        reg.find("utest_game_owned").unwrap(),
        &GAME_TAKEN
    ));
    assert!(!reg.is_user_style("utest_game_owned"));
}

/// #426, by ADR 0114's one rule: of two user styles of one name, the
/// one in the `styles/` nearest the scene wins. A scene two folders
/// down finds a style where the scene-then-parent search found none;
/// the next scene, in another folder, gets its own nearest, in the same
/// registry slot; and a nearer file that appears later takes over at
/// the next reload poll, and gives the name back when it goes.
/// The tests that poll for edits need no lock and no filter (#742):
/// a poll reads the styles of its own registry, which no test beside it
/// can load into, so what it reports is exactly the test's own.
#[test]
fn the_nearest_styles_folder_wins_a_name() {
    let mut reg = super::super::StyleRegistry::new();
    let root = std::env::temp_dir().join(format!("flint-user-426-{}", std::process::id()));
    let cave = root.join("levels/act/cave");
    let other = root.join("other");
    std::fs::create_dir_all(&cave).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(root.join("flint.toml"), "").unwrap();
    let write = |folder: &str, k: f32| {
        let styles = root.join(folder).join("styles");
        std::fs::create_dir_all(&styles).unwrap();
        std::fs::write(
            styles.join("utest_near.shader.toml"),
            format!(
                "version = 1\nname = \"utest_near\"\nshader = \"near.wgsl\"\n\
                 [params.k]\ndefault = {k}\nmin = 0\nmax = 9\n"
            ),
        )
        .unwrap();
        std::fs::write(styles.join("near.wgsl"), PASSTHROUGH).unwrap();
        styles.join("utest_near.shader.toml")
    };
    let at_root = write("", 1.5);
    let in_levels = write("levels", 2.5);
    let k = |reg: &super::super::StyleRegistry| {
        reg.find("utest_near")
            .unwrap()
            .param(&Default::default(), "k")
    };
    let from = |reg: &super::super::StyleRegistry| {
        reg.user("utest_near").unwrap().source.toml_path.clone()
    };
    let cave_scope = AssetScope::for_dir(&cave);
    let other_scope = AssetScope::for_dir(&other);
    // What the loader takes is what the resolver lists
    let listed = cave_scope.list(AssetKind::Styles);
    assert_eq!(listed.len(), 1);
    assert!(flint_core::project::same_path(&listed[0].path, &in_levels));

    assert!(reg.load_for(&cave_scope).is_empty());
    assert_eq!(
        k(&reg),
        2.5,
        "levels/styles is nearer the cave than the root's"
    );
    assert!(flint_core::project::same_path(&from(&reg), &in_levels));
    let index = reg.index_of("utest_near").unwrap();

    // Another scene, whose nearest is the root's
    assert!(reg.load_for(&other_scope).is_empty());
    assert_eq!(k(&reg), 1.5);
    assert!(flint_core::project::same_path(&from(&reg), &at_root));
    assert_eq!(
        reg.index_of("utest_near").unwrap(),
        index,
        "one slot for the name"
    );
    // ... and back, every time, in whichever order scenes load
    assert!(reg.load_for(&cave_scope).is_empty());
    assert_eq!(k(&reg), 2.5);
    assert!(reg.load_for(&cave_scope).is_empty());
    assert_eq!(k(&reg), 2.5);

    // A nearer one appears while the scene is open
    assert!(!reg.reload_changed_for(&cave_scope).changed());
    let in_act = write("levels/act", 3.5);
    let reload = reg.reload_changed_for(&cave_scope);
    assert_eq!(reload.replaced, vec!["utest_near".to_string()]);
    assert!(reload.added.is_empty());
    assert_eq!(k(&reg), 3.5);
    assert_eq!(reg.index_of("utest_near").unwrap(), index);
    assert!(!reg.reload_changed_for(&cave_scope).changed(), "settled");
    // It goes: the next one out has the name again
    std::fs::remove_file(&in_act).unwrap();
    let reload = reg.reload_changed_for(&cave_scope);
    assert_eq!(reload.replaced, vec!["utest_near".to_string()]);
    assert_eq!(k(&reg), 2.5);
    let _ = std::fs::remove_dir_all(&root);
}

/// #426 and #742: each scene is drawn with a registry of its own
/// ([`StyleRegistry::for_scene`]), so a style only an earlier scene's
/// folder had is no style of the next: its table is "unknown", as when
/// the scene loads first. Two scenes whose folders each have a style of
/// one name hold one each, side by side, and a scene that comes back to
/// a folder shares the install it loaded there (same file, unchanged).
#[test]
fn each_scene_holds_only_the_user_styles_it_sees() {
    use super::super::StyleRegistry;
    let root = std::env::temp_dir().join(format!("flint-unseen-426-{}", std::process::id()));
    let near = root.join("near");
    let far = root.join("far");
    std::fs::create_dir_all(near.join("styles")).unwrap();
    std::fs::create_dir_all(&far).unwrap();
    std::fs::write(root.join("flint.toml"), "").unwrap();
    let write = |dir: &Path, k: f32| {
        std::fs::create_dir_all(dir.join("styles")).unwrap();
        std::fs::write(
            dir.join("styles/utest_unseen.shader.toml"),
            format!(
                "version = 1\nname = \"utest_unseen\"\nshader = \"unseen.wgsl\"\n\
                 [params.k]\ndefault = {k}\nmin = 0\nmax = 9\n"
            ),
        )
        .unwrap();
        std::fs::write(dir.join("styles/unseen.wgsl"), PASSTHROUGH).unwrap();
    };
    write(&near, 1.0);
    let tables: BTreeMap<String, toml::Table> = ["utest_unseen", "invert"]
        .into_iter()
        .map(|n| (n.to_string(), toml::Table::new()))
        .collect();
    let unknown = |reg: &StyleRegistry| {
        super::super::StyleSettings::from_tables(&tables, reg)
            .1
            .into_iter()
            .filter(|w| w.contains("unknown style 'utest_unseen'"))
            .count()
    };
    let k = |reg: &StyleRegistry| {
        reg.find("utest_unseen")
            .map(|d| d.param(&Default::default(), "k"))
    };

    let near_scope = AssetScope::for_dir(&near);
    let far_scope = AssetScope::for_dir(&far);
    let (near_reg, messages) = StyleRegistry::new().for_scene(&near_scope);
    assert!(messages.is_empty(), "{messages:?}");
    assert_eq!(k(&near_reg), Some(1.0));
    assert_eq!(unknown(&near_reg), 0);

    // The next scene, in a folder with no styles/: not its style
    let (far_reg, _) = near_reg.for_scene(&far_scope);
    assert_eq!(k(&far_reg), None, "only what the far scene sees");
    assert_eq!(unknown(&far_reg), 1);
    // ... while the near scene's registry still draws it
    assert_eq!(k(&near_reg), Some(1.0));

    // Back: the same file, unchanged, is the same install
    let (again, _) = far_reg.for_scene(&near_scope);
    let (kept, _) = near_reg.for_scene(&near_scope);
    assert!(kept
        .style("utest_unseen")
        .unwrap()
        .same(near_reg.style("utest_unseen").unwrap()));
    assert_eq!(k(&again), Some(1.0));

    // The far scene gets a style of that name of its own: two of one
    // name in the process, from two folders, each its registry's
    write(&far, 7.0);
    let (far_own, messages) = near_reg.for_scene(&far_scope);
    assert!(messages.is_empty(), "{messages:?}");
    assert_eq!(k(&far_own), Some(7.0));
    assert_eq!(k(&near_reg), Some(1.0), "the near scene's is untouched");
    let from = |reg: &StyleRegistry| reg.user("utest_unseen").unwrap().source.toml_path.clone();
    assert!(
        from(&far_own).starts_with(&far)
            || from(&far_own).ends_with("far/styles/utest_unseen.shader.toml")
    );
    assert_ne!(from(&far_own), from(&near_reg));
    let _ = std::fs::remove_dir_all(&root);
}
/// #275: an edited `.shader.toml` replaces its style at the same
/// registry index with the new parameters; a broken edit keeps the old
/// one and reports; a new file in the folder is picked up; and the
/// scene's settings are read again for what changed.
#[test]
fn toml_edits_replace_the_style_in_place() {
    let mut reg = super::super::StyleRegistry::new();
    let dir = std::env::temp_dir().join(format!("flint-user-toml-{}", std::process::id()));
    let styles = dir.join("styles");
    std::fs::create_dir_all(&styles).unwrap();
    let toml_path = styles.join("utest_edit.shader.toml");
    std::fs::write(
        &toml_path,
        "version = 1\nname = \"utest_edit\"\nshader = \"edit.wgsl\"\n\
         [params.a]\ndefault = 0.5\nmin = 0\nmax = 1\n",
    )
    .unwrap();
    std::fs::write(styles.join("edit.wgsl"), PASSTHROUGH).unwrap();
    assert!(reg.load_dir(&dir).is_empty());
    let index = reg.index_of("utest_edit").unwrap();
    let old = reg.style("utest_edit").unwrap().clone();
    let old_bind = reg.user("utest_edit").unwrap().names.passes[0].frame;
    let first = reg.reload_changed(&dir);
    assert!(first.replaced.is_empty(), "nothing changed yet: {first:?}");

    // A new parameter, a new label
    std::thread::sleep(std::time::Duration::from_millis(30));
    std::fs::write(
        &toml_path,
        "version = 1\nname = \"utest_edit\"\nlabel = \"Edited\"\nshader = \"edit.wgsl\"\n\
         [params.a]\ndefault = 0.5\nmin = 0\nmax = 1\n\
         [params.b]\ndefault = 2\nmin = 0\nmax = 4\n",
    )
    .unwrap();
    let reload = reg.reload_changed(&dir);
    assert_eq!(reload.replaced, vec!["utest_edit".to_string()]);
    assert!(reload.changed());
    let new = reg.style("utest_edit").unwrap().clone();
    assert!(!new.same(&old), "a new install");
    assert_ne!(new.generation, old.generation);
    assert_eq!(
        reg.index_of("utest_edit").unwrap(),
        index,
        "at the same index"
    );
    // Its bind groups keep their names (#742: interned once) and are
    // told apart by the generation `StyleFrame` mixes into each key: the
    // old ones hold the old pass's buffers
    assert_eq!(
        reg.user("utest_edit").unwrap().names.passes[0].frame,
        old_bind
    );
    assert_eq!(new.label, "Edited");
    assert_eq!(new.param(&Default::default(), "b"), 2.0);
    assert_eq!(reg.source("utest_edit").unwrap().params.len(), 2);

    // The scene's table, read again, now keeps `b`
    let tables: BTreeMap<String, toml::Table> = toml::from_str("[utest_edit]\nb = 3\n").unwrap();
    let (styles_read, warnings) = super::super::StyleSettings::from_tables(&tables, &reg);
    assert!(warnings.is_empty(), "{warnings:?}");
    let authored = crate::postprocess::PostProcessConfig {
        styles: styles_read,
        ..Default::default()
    };
    let mut live = crate::postprocess::PostProcessConfig::default();
    live.styles.set_enabled("invert", true); // the host's own edit, kept
    refresh_settings(&mut live, &authored, &reload);
    assert_eq!(live.styles.param("utest_edit", "b"), Some(3.0));
    assert!(live.styles.is_enabled("utest_edit"));
    assert!(live.styles.is_enabled("invert"));

    // A broken edit keeps the style as it was and says why
    std::thread::sleep(std::time::Duration::from_millis(30));
    std::fs::write(
        &toml_path,
        "version = 1\nname = \"utest_edit\"\nshader = \n",
    )
    .unwrap();
    let reload = reg.reload_changed(&dir);
    assert!(reload.replaced.is_empty());
    assert_eq!(reload.messages.len(), 1, "{:?}", reload.messages);
    assert!(reg.style("utest_edit").unwrap().same(&new));
    assert!(reg.errors().iter().any(|(n, _)| n == "utest_edit"));
    // ... and isn't read again until it changes once more
    assert!(reg.reload_changed(&dir).messages.is_empty());

    // A new file is added
    std::fs::write(
        styles.join("utest_added.shader.toml"),
        "version = 1\nname = \"utest_added\"\nshader = \"edit.wgsl\"\n",
    )
    .unwrap();
    let reload = reg.reload_changed(&dir);
    assert_eq!(reload.added, vec!["utest_added".to_string()]);
    assert!(reg.find("utest_added").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

/// #425: a `.shader.toml` that never registers is in [`errors`] by its
/// file's style name, from the load that refused it: reported once,
/// gone when the file is fixed, back when it breaks, gone when it's
/// deleted. The scene's table for it still warns "unknown style".
#[test]
fn a_style_that_fails_to_load_is_an_error_until_it_loads_or_goes() {
    let mut reg = super::super::StyleRegistry::new();
    let dir = std::env::temp_dir().join(format!("flint-user-425-{}", std::process::id()));
    let styles = dir.join("styles");
    std::fs::create_dir_all(&styles).unwrap();
    std::fs::write(styles.join("unloaded.wgsl"), PASSTHROUGH).unwrap();
    let toml_path = styles.join("utest_unloaded.shader.toml");
    let write = |stage: &str| {
        std::fs::write(
            &toml_path,
            format!(
                "version = 1\nname = \"utest_unloaded\"\nstage = \"{stage}\"\n\
                 shader = \"unloaded.wgsl\"\n"
            ),
        )
        .unwrap();
    };
    let error = |reg: &super::super::StyleRegistry| {
        reg.errors()
            .into_iter()
            .find(|(n, _)| n == "utest_unloaded")
            .map(|(_, e)| e)
    };

    // The first mistake: a typo in the stage
    write("hrd");
    let messages = reg.load_dir(&dir);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(reg.find("utest_unloaded").is_none());
    let shown = error(&reg).expect("the load failure is an error");
    assert!(
        shown.starts_with(&flint_core::project::shown(&toml_path)),
        "{shown}"
    );
    assert!(shown.contains("stage = \"hrd\""), "{shown}");
    assert_eq!(
        reg.load_errors()
            .iter()
            .filter(|(n, _)| n == "utest_unloaded")
            .count(),
        1
    );
    // The scene's table for it is warned of as before
    let tables: BTreeMap<String, toml::Table> =
        toml::from_str("[utest_unloaded]\nblend = 1\n").unwrap();
    let (_, warnings) = super::super::StyleSettings::from_tables(&tables, &reg);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    // The poll doesn't read it again, or say it again, until it changes
    assert!(reg.reload_changed(&dir).messages.is_empty());
    assert_eq!(error(&reg), Some(shown.clone()));

    // Fixed: it loads at the next poll and the error goes
    std::thread::sleep(std::time::Duration::from_millis(30));
    write("hdr");
    let reload = reg.reload_changed(&dir);
    assert_eq!(reload.added, vec!["utest_unloaded".to_string()]);
    assert_eq!(error(&reg), None);
    assert!(reg.find("utest_unloaded").is_some());
    let _ = std::fs::remove_dir_all(&dir);

    // A new file in another folder that never loads, then is deleted
    let other = std::env::temp_dir().join(format!("flint-user-425b-{}", std::process::id()));
    let other_styles = other.join("styles");
    std::fs::create_dir_all(&other_styles).unwrap();
    let gone = other_styles.join("utest_gone.shader.toml");
    std::fs::write(
        &gone,
        "version = 1\nname = \"utest_went\"\nshader = \"x.wgsl\"\n",
    )
    .unwrap();
    let reload = reg.reload_changed(&other);
    assert_eq!(reload.messages.len(), 1, "{:?}", reload.messages);
    let (_, message) = reg
        .errors()
        .into_iter()
        .find(|(n, _)| n == "utest_gone")
        .unwrap();
    assert!(message.contains("must match the file name"), "{message}");
    std::fs::remove_file(&gone).unwrap();
    assert!(reg.reload_changed(&other).messages.is_empty());
    assert!(reg.errors().iter().all(|(n, _)| n != "utest_gone"));
    let _ = std::fs::remove_dir_all(&other);
}

/// #742: a reload replaces the install, and the old one (its
/// descriptor, parameter lists and description) is freed with the last
/// holder: here the registry alone, so at once. A hundred reloads leave
/// one install alive and the interned names where they were.
#[test]
fn a_reload_frees_the_old_install() {
    let mut reg = super::super::StyleRegistry::new();
    let dir = std::env::temp_dir().join(format!("flint-user-freed-{}", std::process::id()));
    let styles = dir.join("styles");
    std::fs::create_dir_all(&styles).unwrap();
    let toml_path = styles.join("utest_freed.shader.toml");
    let write = |k: u32| {
        std::fs::write(
            &toml_path,
            format!(
                "version = 1\nname = \"utest_freed\"\nshader = \"freed.wgsl\"\n\
                 [params.k]\ndefault = {}\nmin = 0\nmax = 1000\n",
                k % 2
            ),
        )
        .unwrap();
    };
    write(0);
    std::fs::write(styles.join("freed.wgsl"), PASSTHROUGH).unwrap();
    assert!(reg.load_dir(&dir).is_empty());
    let first = Arc::downgrade(&reg.user("utest_freed").unwrap());
    let mut olds = Vec::new();
    for save in 1..=100u32 {
        olds.push(Arc::downgrade(&reg.user("utest_freed").unwrap()));
        write(save);
        // Each save a new stamp, without waiting on the file clock
        let file = std::fs::File::options()
            .write(true)
            .open(&toml_path)
            .unwrap();
        file.set_modified(
            std::time::SystemTime::now() + std::time::Duration::from_secs(u64::from(save)),
        )
        .unwrap();
        let reload = reg.reload_changed(&dir);
        assert_eq!(
            reload.replaced,
            vec!["utest_freed".to_string()],
            "save {save}"
        );
    }
    assert!(first.upgrade().is_none(), "the first install was freed");
    assert!(
        olds.iter().all(|w| w.upgrade().is_none()),
        "every replaced install was freed"
    );
    let now = reg.user("utest_freed").unwrap();
    assert_eq!(Arc::strong_count(&now), 2, "the registry's, and this one");
    // Its names are the same strings every install (interned once)
    assert!(std::ptr::eq(
        now.descriptor.name,
        super::super::intern("utest_freed")
    ));
    let _ = std::fs::remove_dir_all(&dir);
}
/// #275: the renderer's [`StyleSet`](super::super::StyleSet) throws a
/// pass away when its descriptor is replaced, and builds the new one.
#[test]
fn a_replaced_descriptor_is_built_again() {
    let mut reg = super::super::StyleRegistry::new();
    let Some((device, _queue)) = device() else {
        crate::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let dir = std::env::temp_dir().join(format!("flint-user-set-{}", std::process::id()));
    let styles = dir.join("styles");
    std::fs::create_dir_all(&styles).unwrap();
    let toml_path = styles.join("utest_set.shader.toml");
    std::fs::write(
        &toml_path,
        "version = 1\nname = \"utest_set\"\nshader = \"set.wgsl\"\n",
    )
    .unwrap();
    std::fs::write(styles.join("set.wgsl"), PASSTHROUGH).unwrap();
    assert!(reg.load_dir(&dir).is_empty());
    let mut config = crate::postprocess::PostProcessConfig {
        enabled: true,
        ..Default::default()
    };
    config.styles.set_enabled("utest_set", true);
    let ids = crate::style_mask::StyleIds::default();
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut set = super::super::StyleSet::default();
    assert!(set.ensure(&device, format, &config, &ids, &reg).is_empty());
    let active = |set: &super::super::StyleSet, reg: &super::super::StyleRegistry| {
        set.active(StyleStage::Display, &config, &ids, reg)
            .iter()
            .filter(|(d, _)| d.name == "utest_set")
            .map(|(d, _)| d.generation)
            .collect::<Vec<_>>()
    };
    let before = active(&set, &reg);
    assert_eq!(before.len(), 1);
    // What the first install was loaded as, held by the registry and
    // the pass built from it
    let first = Arc::downgrade(&reg.user("utest_set").unwrap());

    std::thread::sleep(std::time::Duration::from_millis(30));
    std::fs::write(
        &toml_path,
        "version = 1\nname = \"utest_set\"\nshader = \"set.wgsl\"\n\
         [params.k]\ndefault = 1\nmin = 0\nmax = 1\n",
    )
    .unwrap();
    assert_eq!(reg.reload_changed(&dir).replaced.len(), 1);
    assert!(active(&set, &reg).is_empty(), "the stale pass isn't drawn");
    assert!(
        first.upgrade().is_some(),
        "the old pass still holds the old install"
    );
    assert!(set.ensure(&device, format, &config, &ids, &reg).is_empty());
    let after = active(&set, &reg);
    assert_eq!(after.len(), 1);
    assert_ne!(before, after, "built from the new install");
    // A hot reload frees the old descriptor (#742): once the pass built
    // from it is built again, nothing holds it
    assert!(first.upgrade().is_none(), "the old install was freed");
    let _ = std::fs::remove_dir_all(&dir);
}

fn printed() -> StyleSource {
    source(
        r#"
        version = 1
        name = "printed"
        shader = "printed.wgsl"
        [params.grain]
        default = 1.0
        min = 0.0
        max = 4.0
        [options.finish]
        default = "matte"
        choices = ["matte", "gloss", "semi-gloss"]
        [options.grade]
        kind = "image"
        default = "luts/warm.png"
        choices = ["luts/warm.png", "luts/cold.png"]
        [images.paper]
        file = "paper.png"
        srgb = true
        "#,
        "styles/printed.shader.toml",
    )
    .unwrap()
}

/// #413: options and images in a description, and what it may not say.
#[test]
fn options_and_images_are_checked() {
    let s = printed();
    let names: Vec<&str> = s.options.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["finish", "grade"], "uniform order is by name");
    assert_eq!(s.uniform_fields(), 3);
    let (_, finish) = &s.options[0];
    assert_eq!(finish.kind, OptionKind::Choice);
    assert_eq!(finish.index("gloss"), 1);
    assert_eq!(
        finish.index("plaid"),
        0,
        "an unknown value draws as the default"
    );
    let images: Vec<(&str, Option<&str>, bool)> = s
        .images
        .iter()
        .map(|i| (i.name.as_str(), i.file.as_deref(), i.srgb))
        .collect();
    assert_eq!(
        images,
        [("grade", None, false), ("paper", Some("paper.png"), true)],
        "binding order is by name; an image option binds too"
    );

    let base = "version = 1\nname = \"x\"\nshader = \"x.wgsl\"\n";
    let file = "styles/x.shader.toml";
    let err = |extra: &str| source(&format!("{base}{extra}"), file).unwrap_err();
    assert!(err("[options.m]\ndefault = \"a\"\n").contains("lists its choices"));
    assert!(
        err("[options.m]\ndefault = \"c\"\nchoices = [\"a\", \"b\"]\n")
            .contains("not one of its choices")
    );
    assert!(err("[options.m]\ndefault = \"a\"\nchoices = [\"a\", \"a\"]\n").contains("twice"));
    assert!(
        err("[options.m]\nkind = \"font\"\ndefault = \"a\"\nchoices = [\"a\"]\n")
            .contains("\"choice\" or \"image\"")
    );
    assert!(
        err("[options.m]\ndefault = \"a\"\nchoices = [\"a\"]\nsrgb = true\n")
            .contains("image option")
    );
    assert!(err("[images.p]\nfile = \"../p.png\"\n").contains("no .."));
    assert!(err("[images.p]\nfile = \"p.hdr\"\n").contains("png, jpg"));
    assert!(err("[images.flint_p]\nfile = \"p.png\"\n").contains("flint"));
    assert!(
        err("[images.p]\nfile = \"p.png\"\n[params.p]\ndefault = 0\nmin = 0\nmax = 1\n")
            .contains("parameter's name")
    );
    assert!(err(
        "[images.p]\nfile = \"p.png\"\n[options.p]\nkind = \"image\"\ndefault = \"p.png\"\n"
    )
    .contains("option's name"));
    assert!(err("[options.enabled]\ndefault = \"a\"\nchoices = [\"a\"]\n").contains("taken"));
    let many: String = (0..=MAX_IMAGES)
        .map(|i| format!("[images.i{i}]\nfile = \"i.png\"\n"))
        .collect();
    assert!(err(&many).contains("at most"));
}

/// #413 with #411: a style with images, options and a loop is written
/// back out guarded, and the guarded module still validates with its
/// group 3 images and option constants.
#[test]
fn guarded_images_and_options_validate() {
    let s = printed();
    let user = r#"
fn style(p: StylePixel) -> vec4<f32> {
var acc = vec3<f32>(0.0);
// A bound read from a parameter, so the loop is guarded (#424)
for (var i = 0; i < 4 + i32(params.grain); i++) {
    acc += flint_image_tiled_paper(p.uv + vec2<f32>(f32(i)) * flint.texel).rgb;
}
var c = flint_lut_grade(p.color.rgb) * acc * 0.25;
if (params.finish == FLINT_FINISH_GLOSS) {
    c *= params.grain;
}
return vec4<f32>(c, 1.0);
}
"#;
    let report = check(&s, user).unwrap_or_else(|e| panic!("{e}"));
    let guarded = report.guarded.expect("a loop is guarded");
    assert!(guarded.contains("@group(3) @binding(1)"), "{guarded}");
    let module = naga::front::wgsl::parse_str(&guarded).unwrap_or_else(|e| panic!("{e}"));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("{e:?}"));
}

/// #558: `mips` is asked for on an `[images]` table or an image option,
/// and refused on a choice option; only an image option with no `choices`
/// is a folder the reload poll lists again.
#[test]
fn mips_and_discovered_choices_are_read() {
    let base = "version = 1\nname = \"x\"\nshader = \"x.wgsl\"\n";
    let file = "styles/x.shader.toml";
    let s = source(
        &format!(
            "{base}[images.paper]\nfile = \"paper.png\"\nmips = true\n\
             [options.grade]\nkind = \"image\"\ndefault = \"luts/a.png\"\n\
             [options.sheet]\nkind = \"image\"\ndefault = \"s/a.png\"\nmips = true\n\
             choices = [\"s/a.png\", \"s/b.png\"]\n"
        ),
        file,
    )
    .unwrap();
    let mips: Vec<(&str, bool)> = s.images.iter().map(|i| (i.name.as_str(), i.mips)).collect();
    assert_eq!(mips, [("grade", false), ("paper", true), ("sheet", true)]);
    let discovered: Vec<(&str, bool)> = s
        .options
        .iter()
        .map(|(n, o)| (n.as_str(), o.discovered))
        .collect();
    assert_eq!(discovered, [("grade", true), ("sheet", false)]);
    let err = source(
        &format!("{base}[options.m]\ndefault = \"a\"\nchoices = [\"a\"]\nmips = true\n"),
        file,
    )
    .unwrap_err();
    assert!(err.contains("mips is for an image option"), "{err}");
}

/// #558: an image with mips gets helpers that pick a level from the
/// screen and `_level` forms, and they validate called from a branch (the
/// level function waives the derivative rule for itself). An image
/// without mips keeps its level-0 text, with none of them.
#[test]
fn mipped_image_helpers_validate_in_a_branch() {
    let base = "version = 1\nname = \"x\"\nshader = \"x.wgsl\"\n";
    let mipped = source(
        &format!("{base}[images.paper]\nfile = \"paper.png\"\nmips = true\n"),
        "styles/x.shader.toml",
    )
    .unwrap();
    let text = prelude(&mipped);
    assert!(text.contains("@diagnostic(off, derivative_uniformity)\nfn flint_image_lod_paper"));
    assert!(text.contains("fn flint_image_tiled_level_paper(uv: vec2<f32>, level: f32)"));
    assert!(
        text.contains("fn flint_lut_paper"),
        "the LUT reads level 0 still"
    );
    let user = r#"
fn style(p: StylePixel) -> vec4<f32> {
    let uv = p.pos / (flint_image_size_paper() * 0.25);
    var c = p.color.rgb;
    if (p.color.r > 0.5) {
        c *= flint_image_tiled_paper(uv).rgb * flint_image_paper(p.uv).a;
    } else {
        c *= flint_image_tiled_level_paper(uv, 2.5).rgb + flint_image_level_paper(p.uv, 1.0).rgb;
    }
    return vec4<f32>(c * flint_image_lod_paper(uv), 1.0);
}
"#;
    check(&mipped, user).unwrap_or_else(|e| panic!("{e}"));

    let plain = source(
        &format!("{base}[images.paper]\nfile = \"paper.png\"\n"),
        "styles/x.shader.toml",
    )
    .unwrap();
    let text = prelude(&plain);
    assert!(!text.contains("flint_image_lod_paper"), "{text}");
    assert!(text.contains(
        "fn flint_image_paper(uv: vec2<f32>) -> vec4<f32> {\n    \
         return textureSampleLevel(flint_image_texture_paper, flint_sampler, uv, 0.0);"
    ));
}

/// #558 on a real device: with mips the upload is the whole chain, level
/// 0 the file's own texels and each level below their average (a 4x4
/// checker of black and white is mid grey at its 1x1 level); without, one
/// level.
#[test]
fn a_mipped_image_uploads_its_chain() {
    let Some((device, queue)) = device() else {
        crate::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let dir = std::env::temp_dir().join(format!("flint-user-mips-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("checker.png");
    image::RgbaImage::from_fn(4, 4, |x, y| {
        let v = if (x + y) % 2 == 0 { 0 } else { 255 };
        image::Rgba([v, v, v, 255])
    })
    .save(&path)
    .unwrap();
    let one = upload_texture(&device, &queue, &path, false, false, "one").unwrap();
    assert_eq!(one.mip_level_count(), 1);
    let chain = upload_texture(&device, &queue, &path, false, true, "chain").unwrap();
    assert_eq!(chain.mip_level_count(), 3, "4x4, 2x2, 1x1");
    // Read the 1x1 level and a texel of level 0 back
    let read = |level: u32| -> [u8; 4] {
        let buffer = device.create_buffer_counted(&wgpu::BufferDescriptor {
            label: None,
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &chain,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(1),
                },
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        crate::wait_for_gpu(&device).expect("the read-back");
        let bytes = buffer.slice(..).get_mapped_range().unwrap();
        [bytes[0], bytes[1], bytes[2], bytes[3]]
    };
    assert_eq!(read(0), [0, 0, 0, 255], "level 0 is the file's");
    let grey = read(2);
    assert!(
        (127..=128).contains(&grey[0]) && grey[0] == grey[1] && grey[3] == 255,
        "{grey:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// #558: an image option that lists its default's folder lists it again
/// while it runs: a file added there or deleted is a new install with the
/// new choices, reported as `rechosen` (the host keeps its settings), and
/// a folder that holds still installs nothing. An option with `choices`
/// isn't listed.
#[test]
fn an_image_options_folder_is_listed_again() {
    let dir = std::env::temp_dir().join(format!("flint-user-folder-{}", std::process::id()));
    let styles = dir.join("styles");
    std::fs::create_dir_all(styles.join("luts")).unwrap();
    std::fs::create_dir_all(styles.join("sheets")).unwrap();
    let png = |path: &Path| {
        image::RgbaImage::from_pixel(2, 1, image::Rgba([9, 9, 9, 255]))
            .save(path)
            .unwrap()
    };
    png(&styles.join("luts").join("a.png"));
    png(&styles.join("luts").join("b.png"));
    png(&styles.join("sheets").join("s.png"));
    std::fs::write(
        styles.join("utest_folder.shader.toml"),
        "version = 1\nname = \"utest_folder\"\nshader = \"folder.wgsl\"\n\
         [options.grade]\nkind = \"image\"\ndefault = \"luts/a.png\"\n\
         [options.sheet]\nkind = \"image\"\ndefault = \"sheets/s.png\"\n\
         choices = [\"sheets/s.png\"]\n",
    )
    .unwrap();
    std::fs::write(styles.join("folder.wgsl"), PASSTHROUGH).unwrap();
    let mut reg = super::super::StyleRegistry::new();
    assert!(reg.load_dir(&dir).is_empty());
    let choices = |reg: &super::super::StyleRegistry, option: &str| -> Vec<String> {
        let d = reg.find("utest_folder").unwrap();
        let o = d.options.iter().find(|o| o.name == option).unwrap();
        o.choices.iter().map(|c| c.to_string()).collect()
    };
    assert_eq!(choices(&reg, "grade"), ["luts/a.png", "luts/b.png"]);
    assert!(!reg.reload_changed(&dir).changed(), "nothing moved");

    // A LUT dropped in while it runs is offered, and nothing else is said
    png(&styles.join("luts").join("c.png"));
    std::fs::write(styles.join("luts").join("notes.txt"), "not an image").unwrap();
    png(&styles.join("sheets").join("t.png"));
    let generation = reg.find("utest_folder").unwrap().generation;
    let reload = reg.reload_changed(&dir);
    assert_eq!(reload.rechosen, ["utest_folder"], "{reload:?}");
    assert!(reload.replaced.is_empty() && reload.added.is_empty());
    assert!(reload.messages.is_empty(), "{:?}", reload.messages);
    assert!(reload.changed());
    assert_eq!(
        choices(&reg, "grade"),
        ["luts/a.png", "luts/b.png", "luts/c.png"]
    );
    assert_eq!(
        choices(&reg, "sheet"),
        ["sheets/s.png"],
        "listed, not found"
    );
    assert_ne!(reg.find("utest_folder").unwrap().generation, generation);
    assert!(!reg.reload_changed(&dir).changed(), "settled");

    // One deleted is no longer a choice; the default stays one even gone
    std::fs::remove_file(styles.join("luts").join("b.png")).unwrap();
    std::fs::remove_file(styles.join("luts").join("a.png")).unwrap();
    assert_eq!(reg.reload_changed(&dir).rechosen, ["utest_folder"]);
    assert_eq!(choices(&reg, "grade"), ["luts/a.png", "luts/c.png"]);
    assert!(!reg.reload_changed(&dir).changed(), "settled");
    let _ = std::fs::remove_dir_all(&dir);
}

/// #413 with #414: a per-entity style's options sit in every row of its
/// settings table, after the floats, and its prelude validates.
#[test]
fn per_entity_options_ride_in_each_row() {
    let s = source(
        "version = 1\nname = \"x\"\nshader = \"x.wgsl\"\ninputs = [\"mask\"]\n\
         [params.a]\ndefault = 0.5\nmin = 0\nmax = 1\n\
         [options.finish]\ndefault = \"matte\"\nchoices = [\"matte\", \"gloss\"]\n\
         [images.paper]\nfile = \"p.png\"\n",
        "styles/x.shader.toml",
    )
    .unwrap();
    let user = "fn style(p: StylePixel) -> vec4<f32> {\n    \
                let n = flint_params_at(p.uv + flint.texel);\n    \
                let gloss = f32(params.finish == FLINT_FINISH_GLOSS) + f32(n.finish);\n    \
                return flint_image_paper(p.uv) * params.a * gloss;\n}\n";
    check(&s, user).unwrap_or_else(|e| panic!("{e}"));
    let rows = vec![vec![0.5, f32::from_bits(0)], vec![0.25, f32::from_bits(1)]];
    let words = row_words(s.uniform_fields(), &rows);
    let stride = params_size(s.uniform_fields()) / 4;
    assert_eq!(words[1], 0, "row 0: matte");
    assert_eq!(words[stride + 1], 1, "row 1: gloss");
    assert_eq!(f32::from_bits(words[stride]), 0.25);
}

/// #413: the prelude declares each image's helpers and each option's
/// index and constants, and naga takes a shader that uses them all.
#[test]
fn images_and_options_validate() {
    let s = printed();
    let prelude = prelude(&s);
    assert!(prelude.contains("finish: u32,"), "{prelude}");
    assert!(prelude.contains("const FLINT_FINISH_GLOSS: u32 = 1u;"));
    assert!(prelude.contains("const FLINT_FINISH_SEMI_GLOSS: u32 = 2u;"));
    assert!(prelude.contains("@group(3) @binding(0) var flint_image_texture_grade"));
    assert!(prelude.contains("@group(3) @binding(1) var flint_image_texture_paper"));
    let user = r#"
fn style(p: StylePixel) -> vec4<f32> {
var c = flint_lut_grade(p.color.rgb);
let paper = flint_image_tiled_paper(p.pos / flint_image_size_paper());
if (params.finish == FLINT_FINISH_GLOSS) {
    c = c * 1.1;
}
let edge = flint_image_paper(p.uv).a;
return vec4<f32>(c * paper.rgb * edge * params.grain, 1.0);
}
"#;
    let report = check(&s, user).unwrap_or_else(|e| panic!("{e}"));
    // LUT 2 + tiled 4 + clamped 1 + size 0
    assert_eq!(report.texture_reads, 7);

    // An image the style doesn't declare: named, with the ones it has
    let user = "fn style(p: StylePixel) -> vec4<f32> {\n    \
                return flint_image_brush(p.uv);\n}\n";
    let e = check(&s, user).unwrap_err();
    assert!(
        e.contains("[images.<name>]") && e.contains("grade, paper"),
        "{e}"
    );
}

/// #946: the hints go by the identifier an error is about, not its
/// words. naga 23 quoted it 'like this', naga 30 `like this`, and a
/// browser compiler may only point its span at it: each gets the
/// `[images.<name>]` hint, and an error about another name doesn't.
#[test]
fn hints_follow_the_identifier_not_the_wording() {
    let s = printed();
    let user = "fn style(p: StylePixel) -> vec4<f32> {
                    return flint_image_brush(p.uv);
}
";
    let c = compose(&s, user);
    let offset = c.text.find("flint_image_brush").unwrap() as u32;
    let line = c.prelude_lines as u32 + 2;
    let report = |text: &str| {
        let info = wgpu::CompilationInfo {
            messages: vec![wgpu::CompilationMessage {
                message: text.to_string(),
                message_type: wgpu::CompilationMessageType::Error,
                location: Some(wgpu::SourceLocation {
                    line_number: line,
                    line_position: 12,
                    offset,
                    length: "flint_image_brush".len() as u32,
                }),
            }],
        };
        compiler_error(&s, &c, &info).unwrap()
    };
    for text in [
        "no definition in scope for identifier: 'flint_image_brush'",
        "no definition in scope for identifier: `flint_image_brush`",
        "unresolved call target \"flint_image_brush\"",
        "unknown identifier",
    ] {
        let e = report(text);
        assert!(
            e.contains("[images.<name>]") && e.contains("grade, paper"),
            "{text}: {e}"
        );
    }
    // Named only in passing, inside a longer identifier: no hint
    assert_eq!(named("not_flint_image_x", None), ["not_flint_image_x"]);
    let e = check(
        &s,
        "fn style(p: StylePixel) -> vec4<f32> {
return p.color * glow;
}
",
    )
    .unwrap_err();
    assert!(!e.contains("hint"), "{e}");
}

/// #946: `pack2x16float`, `unpack2x16float` and `quantizeToF16` are
/// core WGSL; naga 30 asks for `SHADER_FLOAT16_IN_FLOAT32`, which wgpu
/// grants from `DownlevelFlags::SHADER_F16_IN_F32`, so `flint style
/// check` takes them as the device will.
#[test]
fn half_float_packing_validates() {
    let s = duotone();
    let user = "fn style(p: StylePixel) -> vec4<f32> {
                    let packed = pack2x16float(p.color.rg);
                    let rg = unpack2x16float(packed);
                    return vec4<f32>(rg, quantizeToF16(p.color.b), p.color.a);
}
";
    check(&s, user).unwrap_or_else(|e| panic!("{e}"));
}

/// #413: an image option with no list offers its default's folder; the
/// list is fixed at load, so a scene naming another file is warned
/// (#318) and `--style` refuses it.
#[test]
fn image_options_find_their_folder_and_warn_on_strangers() {
    let mut reg = super::super::StyleRegistry::new();
    let dir = std::env::temp_dir().join(format!("flint-user-images-{}", std::process::id()));
    let styles = dir.join("styles");
    std::fs::create_dir_all(styles.join("papers")).unwrap();
    let png = |path: &Path| {
        image::RgbaImage::from_pixel(2, 2, image::Rgba([200, 180, 150, 255]))
            .save(path)
            .unwrap()
    };
    png(&styles.join("papers").join("cotton.png"));
    png(&styles.join("papers").join("kraft.png"));
    std::fs::write(styles.join("papers").join("notes.txt"), "not an image").unwrap();
    std::fs::write(
        styles.join("utest_paper.shader.toml"),
        "version = 1\nname = \"utest_paper\"\nshader = \"paper.wgsl\"\n\
         [options.paper]\nkind = \"image\"\ndefault = \"papers/cotton.png\"\n\
         [images.missing]\nfile = \"nowhere.png\"\n",
    )
    .unwrap();
    std::fs::write(styles.join("paper.wgsl"), PASSTHROUGH).unwrap();
    let messages = reg.load_dir(&dir);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains("nowhere.png") && messages[0].contains("image not found"),
        "{messages:?}"
    );
    assert!(reg
        .errors()
        .iter()
        .any(|(n, e)| n == "utest_paper" && e.contains("nowhere.png")));
    let d = reg.find("utest_paper").unwrap();
    let option = d.options.iter().find(|o| o.name == "paper").unwrap();
    assert_eq!(*option.choices, ["papers/cotton.png", "papers/kraft.png"]);
    assert!(option.closed());

    let tables: BTreeMap<String, toml::Table> =
        toml::from_str("[utest_paper]\npaper = \"papers/kraft.png\"\n").unwrap();
    let (settings, warnings) = super::super::StyleSettings::from_tables(&tables, &reg);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        settings.option("utest_paper", "paper"),
        Some("papers/kraft.png")
    );
    let tables: BTreeMap<String, toml::Table> =
        toml::from_str("[utest_paper]\npaper = \"papers/linen.png\"\n").unwrap();
    let (_, warnings) = super::super::StyleSettings::from_tables(&tables, &reg);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("unknown value 'papers/linen.png'")
            && warnings[0].contains("papers/kraft.png"),
        "{warnings:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// #413 on a real device: images load when first drawn, follow the
/// scene's choice, read again when their file changes, and keep the
/// last good picture when a file breaks.
#[test]
fn images_load_follow_choices_and_reload() {
    let Some((device, queue)) = device() else {
        crate::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let dir = std::env::temp_dir().join(format!("flint-user-image-gpu-{}", std::process::id()));
    let styles = dir.join("styles");
    std::fs::create_dir_all(styles.join("luts")).unwrap();
    let png = |path: &Path, w: u32, v: u8| {
        image::RgbaImage::from_pixel(w, 1, image::Rgba([v, v, v, 255]))
            .save(path)
            .unwrap()
    };
    png(&styles.join("paper.png"), 4, 200);
    png(&styles.join("luts").join("a.png"), 2, 10);
    png(&styles.join("luts").join("b.png"), 2, 20);
    let toml_path = styles.join("utest_imgs.shader.toml");
    std::fs::write(
        &toml_path,
        "version = 1\nname = \"utest_imgs\"\nshader = \"imgs.wgsl\"\n\
         [options.grade]\nkind = \"image\"\ndefault = \"luts/a.png\"\n\
         [images.paper]\nfile = \"paper.png\"\n",
    )
    .unwrap();
    let source = read_source(&toml_path).unwrap();
    let mut set = ImageSet::new(&device, &source).unwrap();
    let choice = std::cell::RefCell::new(String::new());
    let choose = |_: &str| choice.borrow().clone();

    let first = set.update(&device, &queue, &choose, false);
    assert!(first.ready && first.error.is_none(), "{first:?}");
    assert_eq!(first.loaded, ["luts/a.png", "paper.png"]);
    set.bind_group(&device);
    assert!(set.bind.is_some());
    let again = set.update(&device, &queue, &choose, true);
    assert!(again.loaded.is_empty(), "nothing changed: {again:?}");
    assert!(set.bind.is_some(), "the bind group is kept");

    // The scene picks the other LUT
    *choice.borrow_mut() = "luts/b.png".into();
    let picked = set.update(&device, &queue, &choose, false);
    assert_eq!(picked.loaded, ["luts/b.png"]);
    assert!(set.bind.is_none(), "bound again over the new view");

    // The paper changes on disk: read again at the next poll only
    std::thread::sleep(std::time::Duration::from_millis(30));
    png(&styles.join("paper.png"), 8, 120);
    assert!(set
        .update(&device, &queue, &choose, false)
        .loaded
        .is_empty());
    assert_eq!(
        set.update(&device, &queue, &choose, true).loaded,
        ["paper.png"]
    );

    // A broken paper keeps the last good one and says so, once
    std::thread::sleep(std::time::Duration::from_millis(30));
    std::fs::write(styles.join("paper.png"), b"not a png").unwrap();
    let broken = set.update(&device, &queue, &choose, true);
    assert!(broken.ready, "the last good paper still binds");
    assert!(broken
        .error
        .as_deref()
        .is_some_and(|e| e.contains("paper.png")));
    assert!(set.update(&device, &queue, &choose, true).loaded.is_empty());
    std::thread::sleep(std::time::Duration::from_millis(30));
    png(&styles.join("paper.png"), 4, 90);
    let fixed = set.update(&device, &queue, &choose, true);
    assert_eq!(fixed.loaded, ["paper.png"]);
    assert!(fixed.error.is_none());

    // A missing image never loaded: not ready (the pass draws through)
    let mut source = source;
    source.images.push(ImageSource {
        name: "zz".into(),
        file: Some("gone.png".into()),
        srgb: false,
        mips: false,
    });
    let mut set = ImageSet::new(&device, &source).unwrap();
    let missing = set.update(&device, &queue, &choose, false);
    assert!(!missing.ready);
    assert!(missing
        .error
        .as_deref()
        .is_some_and(|e| e.contains("gone.png")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// #275: a web bundle is a `MemorySource`; styles load from it through
/// the vfs exactly as from disk, and a shader missing from it is an
/// error (`flint bundle` reports it) rather than a crash.
#[test]
fn styles_load_from_a_bundle() {
    let mut reg = super::super::StyleRegistry::new();
    let source: flint_core::vfs::MemorySource = [
        (
            "styles/utest_bundled.shader.toml",
            "version = 1\nname = \"utest_bundled\"\nshader = \"bundled.wgsl\"\n\
             [params.k]\ndefault = 0.5\nmin = 0\nmax = 1\n"
                .as_bytes()
                .to_vec(),
        ),
        ("styles/bundled.wgsl", PASSTHROUGH.as_bytes().to_vec()),
        (
            "styles/utest_unpacked.shader.toml",
            "version = 1\nname = \"utest_unpacked\"\nshader = \"missing.wgsl\"\n"
                .as_bytes()
                .to_vec(),
        ),
        ("levels/a.scene.toml", b"[scene]\nname = \"a\"\n".to_vec()),
    ]
    .into_iter()
    .collect();
    let messages = flint_core::vfs::scoped(Arc::new(source), || reg.load_dir(Path::new("levels")));
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("missing.wgsl"), "{messages:?}");
    let bundled = reg.find("utest_bundled").expect("found via ../styles");
    assert_eq!(bundled.param(&Default::default(), "k"), 0.5);
    assert!(reg.errors().iter().any(|(n, _)| n == "utest_unpacked"));
    assert!(!reg.errors().iter().any(|(n, _)| n == "utest_bundled"));
}

/// `flint style new` (#416): every stage and set of inputs writes a
/// description that checks and a shader that compiles with no warning,
/// lists exactly the helpers its inputs give, and runs a check from
/// the files on disk.
#[test]
fn templates_check_for_every_stage_and_input() {
    for stage in [StyleStage::Display, StyleStage::Hdr] {
        for bits in 0..8u8 {
            let inputs = Inputs {
                depth: bits & 1 != 0,
                mask: bits & 2 != 0,
                edges: bits & 4 != 0,
            };
            let t = template("utest_new", stage, inputs);
            let path = Path::new("styles/utest_new.shader.toml");
            let source = parse_source(&t.toml, path).unwrap();
            assert_eq!(source.stage, stage);
            assert_eq!(source.inputs, inputs);
            assert_eq!(source.params.len(), 1);
            let report = check(&source, &t.wgsl).unwrap_or_else(|e| panic!("{e}\n{}", t.wgsl));
            assert!(report.warnings.is_empty(), "{:?}", report.warnings);
            assert_eq!(t.wgsl.contains("flint_view_depth"), inputs.depth);
            assert_eq!(t.wgsl.contains("flint_style_blend"), inputs.mask);
            assert_eq!(t.wgsl.contains("flint_edge("), inputs.edges);
            // Uncommenting the input lines still compiles
            let live = t.wgsl.replace("    // let ", "    let ");
            check(&source, &live).unwrap_or_else(|e| panic!("{e}\n{live}"));
        }
    }
    // From disk, as `flint style check` runs it
    let dir = std::env::temp_dir().join(format!("flint-style-new-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let t = template("utest_disk", StyleStage::Display, Inputs::default());
    let toml_path = dir.join("utest_disk.shader.toml");
    std::fs::write(&toml_path, &t.toml).unwrap();
    std::fs::write(dir.join("utest_disk.wgsl"), &t.wgsl).unwrap();
    assert!(check_file(&toml_path).unwrap().warnings.is_empty());
    std::fs::write(
        dir.join("utest_disk.wgsl"),
        "fn style(p: StylePixel) -> vec4<f32> {\n    retrun p.color;\n}\n",
    )
    .unwrap();
    let err = check_file(&toml_path).unwrap_err();
    assert!(err.contains("utest_disk.wgsl:2:"), "{err}");
    std::fs::remove_file(dir.join("utest_disk.wgsl")).unwrap();
    assert!(check_file(&toml_path)
        .unwrap_err()
        .contains("utest_disk.wgsl"));
    let _ = std::fs::remove_dir_all(&dir);
    // A built-in's name is refused before anything is written
    let t = template("ascii", StyleStage::Display, Inputs::default());
    assert!(parse_source(&t.toml, Path::new("styles/ascii.shader.toml"))
        .unwrap_err()
        .contains("built-in"));
}

/// #567: what `locate` writes, taken apart by `error_parts`: the place,
/// the message, the quoted line and each hint; put back together they
/// say what the text said.
#[test]
fn a_checks_error_comes_apart_into_its_place_and_hints() {
    let s = duotone(); // depth, no mask
    let user = "fn style(p: StylePixel) -> vec4<f32> {\n    \
                return p.color * flint_style_blend(p.uv);\n}\n";
    let e = check(&s, user).unwrap_err();
    let parts = error_parts(&e, "styles/duotone.shader.toml");
    assert_eq!(parts.len(), 1, "{e}\n{parts:?}");
    let p = &parts[0];
    assert_eq!(p.file, "styles/duotone.wgsl", "{e}");
    assert_eq!(p.line, Some(2), "{e}");
    assert!(p.col.is_some(), "{e}");
    assert!(
        !p.message.is_empty() && !p.message.contains('\n'),
        "{e}\n{p:?}"
    );
    assert!(
        p.source
            .as_deref()
            .is_some_and(|s| s.contains("flint_style_blend")),
        "{e}\n{p:?}"
    );
    assert_eq!(
        p.hints,
        ["add \"mask\" to inputs in the .shader.toml"],
        "{e}"
    );
    assert!(
        e.starts_with(&format!(
            "{}:{}:{}: {}\n",
            p.file,
            p.line.unwrap(),
            p.col.unwrap(),
            p.message
        )),
        "{e}\n{p:?}"
    );
}

/// #567: several passes' errors, a place with a line only, a drive in a
/// path, a description's TOML error and text that names no file.
#[test]
fn errors_of_several_passes_and_descriptions_come_apart() {
    let text = "C:/game/styles/a.wgsl:3:7: unknown identifier\n    3 | let x = y;\n\
                styles/a_blur.wgsl:12: `t` declares a binding\n    \
                hint: no pass writes a target \"t\"";
    let parts = error_parts(text, "styles/a.shader.toml");
    assert_eq!(parts.len(), 2, "{parts:?}");
    assert_eq!(
        (parts[0].file.as_str(), parts[0].line, parts[0].col),
        ("C:/game/styles/a.wgsl", Some(3), Some(7))
    );
    assert_eq!(parts[0].message, "unknown identifier");
    assert_eq!(parts[0].source.as_deref(), Some("let x = y;"));
    assert_eq!(
        (parts[1].file.as_str(), parts[1].line, parts[1].col),
        ("styles/a_blur.wgsl", Some(12), None)
    );
    assert_eq!(parts[1].hints, ["no pass writes a target \"t\""]);

    // A description's TOML error has its place in its message
    let e = parse_source(
        "version = 1\nstage = [\n",
        Path::new("styles/b.shader.toml"),
    )
    .unwrap_err();
    let parts = error_parts(&e, "styles/b.shader.toml");
    assert_eq!(parts.len(), 1, "{e}\n{parts:?}");
    assert_eq!(parts[0].file, "styles/b.shader.toml");
    assert!(
        parts[0].line.is_some() && parts[0].col.is_some(),
        "{e}\n{parts:?}"
    );

    // Nothing leads with a style's file: one error in the fallback
    let parts = error_parts("something odd", "styles/c.shader.toml");
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].file, "styles/c.shader.toml");
    assert_eq!(parts[0].message, "something odd");
}

/// #567: a style's files are named with `/` however its folder was given
/// (`demo/styles\x.wgsl` before, on Windows).
#[cfg(windows)]
#[test]
fn a_styles_files_are_named_with_forward_slashes() {
    let user = "fn style(p: StylePixel) -> vec4<f32> {\n    return p.colour;\n}\n";
    for given in [r"demo\styles\x.shader.toml", "demo/styles/x.shader.toml"] {
        let s = source("version = 1\nname = \"x\"\nshader = \"x.wgsl\"\n", given).unwrap();
        let e = check(&s, user).unwrap_err();
        assert!(e.starts_with("demo/styles/x.wgsl:2:"), "{given}: {e}");
    }
}

/// `path` spelled from the working directory, as a player run from a
/// project's root spells its styles (`styles/x.shader.toml`): `same_path`
/// does its disk work only for relative paths.
fn from_cwd(path: &std::path::Path) -> std::path::PathBuf {
    let cwd = std::env::current_dir().unwrap();
    let mut up = std::path::PathBuf::new();
    let mut base = cwd.as_path();
    while !path.starts_with(base) {
        up.push("..");
        base = base.parent().expect("a common root");
    }
    up.join(path.strip_prefix(base).unwrap())
}

/// A vfs that counts what it is asked, over the disk.
#[derive(Default)]
struct Counting(std::sync::atomic::AtomicUsize);

impl flint_core::vfs::FileSource for Counting {
    fn read(&self, path: &std::path::Path) -> std::io::Result<Vec<u8>> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        flint_core::vfs::DiskSource.read(path)
    }
    fn read_dir(&self, path: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        flint_core::vfs::DiskSource.read_dir(path)
    }
    fn kind(&self, path: &std::path::Path) -> Option<flint_core::vfs::EntryKind> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        flint_core::vfs::DiskSource.kind(path)
    }
}

/// #1121: the user-style reload poll runs a few times a second on the
/// frame thread. For styles it has loaded it asks the disk nothing beyond
/// `file_watch` (the player's watcher thread answers that): it compares a
/// listed file with the loaded ones by spelling before `same_path`, whose
/// working out reads the working directory and stats it, and works out its
/// style folders once a scope. In the looks tour, with relative paths, that
/// working out took 16-37 ms a style when other threads were busy on
/// files, and stopped the game 150-260 ms a poll. With the old
/// `loaded_from` this test counts 2 vfs calls for each loaded style a
/// listed file is compared with and passed over.
#[test]
fn the_reload_poll_asks_nothing_of_the_disk_for_styles_it_has() {
    let root = std::env::temp_dir().join(format!("flint-user-1121-{}", std::process::id()));
    let styles = root.join("styles");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&styles).unwrap();
    std::fs::write(root.join("flint.toml"), "").unwrap();
    std::fs::write(styles.join("poll.wgsl"), PASSTHROUGH).unwrap();
    for name in [
        "utest_poll_a",
        "utest_poll_b",
        "utest_poll_c",
        "utest_poll_d",
    ] {
        std::fs::write(
            styles.join(format!("{name}.shader.toml")),
            format!("version = 1\nname = \"{name}\"\nshader = \"poll.wgsl\"\n"),
        )
        .unwrap();
    }
    let scope = AssetScope::for_dir(&from_cwd(&root));
    let mut reg = super::super::StyleRegistry::new();
    assert!(reg.load_for(&scope).is_empty());
    assert_eq!(reg.users().count(), 4);
    // The first poll works the folders out
    assert!(!reg.reload_changed_for(&scope).changed());

    let counting = std::sync::Arc::new(Counting::default());
    let source: std::sync::Arc<dyn flint_core::vfs::FileSource> = counting.clone();
    flint_core::vfs::scoped(source, || {
        for _ in 0..10 {
            assert!(!reg.reload_changed_for(&scope).changed());
        }
    });
    let asked = counting.0.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(asked, 0, "10 polls asked the vfs {asked} times");

    // And it still sees an edit
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(
        styles.join("utest_poll_c.shader.toml"),
        "version = 1\nname = \"utest_poll_c\"\nshader = \"poll.wgsl\"\n[params.k]\ndefault = 1\nmin = 0\nmax = 2\n",
    )
    .unwrap();
    let reload = reg.reload_changed_for(&scope);
    assert_eq!(reload.replaced, vec!["utest_poll_c".to_string()]);
    let _ = std::fs::remove_dir_all(&root);
}
