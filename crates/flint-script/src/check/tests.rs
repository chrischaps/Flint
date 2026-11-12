use super::*;
use crate::api::builder::kind::{Num, Str};

fn checker() -> Checker {
    Checker::new(Api::of(&mut crate::ScriptSystem::new()))
}

/// `line:col code message`, one a finding, for asserts that read like the
/// output.
fn found(checker: &Checker, source: &str) -> Vec<String> {
    checker
        .check_source(Path::new("t.rhai"), source, false)
        .iter()
        .map(|d| {
            format!(
                "{}:{} {} {}",
                d.line.unwrap_or(0),
                d.column.unwrap_or(0),
                d.code,
                d.message
            )
        })
        .collect()
}

fn codes_of(checker: &Checker, source: &str) -> Vec<&'static str> {
    checker
        .check_source(Path::new("t.rhai"), source, false)
        .iter()
        .map(|d| d.code)
        .collect()
}

/// The three mistakes #694 is for, each with its file, line and column.
#[test]
fn a_typo_a_wrong_count_and_a_misspelt_callback_are_found_where_they_are() {
    let c = checker();
    let source = "fn on_update() {\n    \
                  let p = get_postion(get_entity(\"crate\"));\n    \
                  draw_line(0.0, 0.0, 10.0, 10.0, 1.0, 1.0, 1.0, 1.0);\n}\n\n\
                  fn on_colision(other) {\n    log(\"hit\");\n}\n";
    let found = c.check_source(Path::new("scripts/t.rhai"), source, false);
    let lines: Vec<String> = found
        .iter()
        .map(|d| {
            format!(
                "{}:{}:{} {} {}",
                d.file.as_deref().map(shown).unwrap_or_default(),
                d.line.unwrap(),
                d.column.unwrap(),
                d.code,
                d.hint.clone().unwrap_or_default()
            )
        })
        .collect();
    assert_eq!(found.len(), 3, "{lines:#?}");
    assert!(
        lines[0].starts_with("scripts/t.rhai:2:13 unknown-function did you mean `get_position`?"),
        "{lines:#?}"
    );
    assert!(
        lines[1].starts_with("scripts/t.rhai:3:5 wrong-arity declared: draw_line(x1: num"),
        "{lines:#?}"
    );
    assert!(found[1]
        .message
        .contains("takes 9 arguments; this call passes 8"));
    assert_eq!(
        lines[2],
        "scripts/t.rhai:6:1 unknown-callback did you mean `on_collision`?"
    );
    assert!(found.iter().all(Diagnostic::is_error));
}

#[test]
fn a_syntax_error_is_at_its_line() {
    let c = checker();
    let found = found(&c, "fn on_init() {\n    let x = ;\n}\n");
    assert_eq!(found.len(), 1);
    assert!(found[0].starts_with("2:13 script-syntax"), "{found:?}");
}

/// Rhai's own functions and methods, closures, function pointers and the
/// file's own functions called as methods are all calls that resolve.
#[test]
fn builtins_methods_and_own_functions_pass() {
    let c = checker();
    let source = r#"
        fn helper(a, b) { a + b }
        fn grow() { this.size += 1; }
        fn on_init() {
            let a = [1, 2, 3];
            a.push(4);
            let n = a.len() + len(a);
            let s = n.to_string().to_upper();
            let doubled = a.map(|x| x * 2);
            let total = doubled.reduce(|sum, x| sum + x, 0);
            let f = Fn("helper");
            let g = |x| x + 1;
            print(f.call(1, 2) + g(3) + helper(1, 2) + abs(-3) + max(1, 2));
            let thing = #{ size: 1, act: |v| v * 2 };
            thing.grow();
            thing.act(2);
            if type_of(s) == "string" && is_def_fn("helper", 2) { debug(s.len()); }
            let m = #{}; m.keys(); `${total}`.trim();
            for (item, i) in a { log(`${item} ${i}`); }
            set_position(get_entity("crate"), 0, 1.5, 0);
        }
    "#;
    assert_eq!(found(&c, source), Vec::<String>::new());
}

#[test]
fn own_functions_are_held_to_their_arity() {
    let c = checker();
    let found = found(
        &c,
        "fn helper(a, b) { a + b }\nfn on_init() { helper(1); }\n",
    );
    assert_eq!(found.len(), 1);
    assert!(
        found[0].starts_with("2:16 wrong-arity `helper` takes 2 arguments"),
        "{found:?}"
    );
}

/// `import` goes through the module search; a module's functions are
/// called through its alias, at their arities, and a missing module is
/// named with the paths tried.
#[test]
fn imports_are_followed_and_their_functions_checked() {
    let dir = std::env::temp_dir().join(format!("flint-script-check-{}", std::process::id()));
    let lib = dir.join("scripts/lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(
        lib.join("film.rhai"),
        "fn cut(at) { at }\nprivate fn inner() { 1 }\nfn on_colision_helper() {}\n",
    )
    .unwrap();
    let script = dir.join("scripts/shot.rhai");
    std::fs::write(
        &script,
        "import \"film\" as film;\nimport \"gone\" as gone;\n\
         fn on_init() { film::cut(1); film::cut(); film::inner(); film::cutt(2); gone::x(); }\n",
    )
    .unwrap();
    let c = checker();
    let checked = c.check(vec![ScriptFile::found(script.clone())]);
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(checked.files.len(), 2, "the module is checked too");
    let told: Vec<String> = checked
        .diagnostics
        .iter()
        .map(|d| {
            format!(
                "{}:{} {} {}",
                d.line.unwrap(),
                d.column.unwrap(),
                d.code,
                d.message
            )
        })
        .collect();
    assert_eq!(told.len(), 4, "{told:#?}");
    assert!(
        told[0].starts_with("2:1 missing-module import \"gone\""),
        "{told:#?}"
    );
    assert!(!checked.diagnostics[0].tried.is_empty());
    assert!(
        told[1].contains("wrong-arity `film::cut` takes 1 argument;"),
        "{told:#?}"
    );
    assert!(told[2].contains("unknown-function module `film` has no function `inner`"));
    assert!(told[3].contains("`cutt`"));
    assert_eq!(
        checked.diagnostics[3].hint.as_deref(),
        Some("did you mean `cut`?")
    );
    // A module's `fn on_...` is not a callback the engine would call
    assert!(!told.iter().any(|t| t.contains("unknown-callback")));
}

#[test]
fn a_callback_at_the_wrong_arity_and_near_misses() {
    let c = checker();
    let source = "fn on_update(dt) {}\nfn on_sequence_cue(seq) {}\n\
                  fn on_updte() {}\nfn on_hit(o) {}\nfn onUpdate() {}\n\
                  fn on_spawn() {}\nfn on_jump() {}\nfn on_init() { on_hit(1); }\n";
    let found = found(&c, source);
    assert_eq!(found.len(), 4, "{found:#?}");
    assert!(found[0].starts_with("1:1 callback-arity `fn on_update` has 1 parameter;"));
    assert!(found[1].starts_with("2:1 callback-arity `fn on_sequence_cue`"));
    assert!(found[2].starts_with("3:1 unknown-callback `fn on_updte`"));
    // `on_hit` is called; `on_spawn` and `on_jump` are no callback's typo
    assert!(found[3].starts_with("5:1 unknown-callback `fn onUpdate`"));
}

/// ADR 0116 decision 5: a warning for ints and floats mixed in four or
/// more `num` slots, every frame, literals only.
#[test]
fn mixed_number_literals_warn_in_per_frame_code_only() {
    let c = checker();
    let source = "fn hud() { draw_rect(10, 10, 100.0, 50, 1.0, 1.0, 1.0, 1.0); }\n\
                  fn on_draw_ui() {\n  hud();\n  \
                  draw_rect(10.0, 10.0, 100.0, 50.0, 1.0, 1.0, 1.0, 1.0);\n  \
                  draw_rect(10, 10, 100, 50, 1, 1, 1, 1);\n  let w = 3;\n  \
                  draw_rect(w, 10.0, 100.0, 50.0, 1.0, 1.0, 1.0, 1.0);\n}\n\
                  fn on_init() { draw_rect(10, 10, 100.0, 50, 1.0, 1.0, 1.0, 1.0); }\n";
    let found = c.check_source(Path::new("t.rhai"), source, false);
    assert_eq!(found.len(), 1, "{found:#?}");
    let d = &found[0];
    assert_eq!(d.code, codes::MIXED_NUMBERS);
    assert!(!d.is_error());
    assert_eq!((d.line, d.column), (Some(1), Some(12)));
    assert!(d.message.contains("(3 int, 5 float)"), "{}", d.message);
}

#[test]
fn a_literal_no_declaration_takes_is_an_error() {
    let c = checker();
    let codes = codes_of(
        &c,
        "fn on_init() { set_position(\"crate\", 0.0, 1.0, 0.0); play_sound(3); \
         set_position(get_entity(\"crate\"), 0, 1, 0); }\n",
    );
    assert_eq!(codes, [codes::BAD_ARGUMENT, codes::BAD_ARGUMENT]);
    let d = &c.check_source(
        Path::new("t.rhai"),
        "fn on_init() { set_position(\"crate\", 0.0, 1.0, 0.0); }",
        false,
    )[0];
    assert!(
        d.message
            .contains("`set_position` argument 1 (`id: entity`) is a string"),
        "{}",
        d.message
    );
}

/// A game's functions: allowed by name, or known from its listing.
#[test]
fn a_hosts_functions_pass_when_allowed_or_listed() {
    let source = "fn on_init() { game_spawn_wave(3); game_tint(\"red\", 1); }";
    let mut api = Api::of(&mut crate::ScriptSystem::new());
    let before = Checker::new(api.clone()).check_source(Path::new("t.rhai"), source, false);
    assert_eq!(before.len(), 2);
    api.allow("game_spawn_wave");
    api.add("game_tint", 1);
    let after = Checker::new(api).check_source(Path::new("t.rhai"), source, false);
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].code, codes::WRONG_ARITY);
}

/// Decision 3's hook: a declared function marked deprecated warns at each
/// call, and an extension's builder declarations are checked like Flint's.
#[test]
fn a_deprecated_function_warns() {
    let mut system = crate::ScriptSystem::new();
    system
        .script_api()
        .module("game")
        .func("old_paint", "Paint, the old way.")
        .arg("colour", Str)
        .arg("amount", Num)
        .deprecated("use paint(colour, amount)")
        .run_pure(|(_colour, _amount)| {});
    let c = Checker::new(Api::of(&mut system));
    let found = c.check_source(
        Path::new("t.rhai"),
        "fn on_init() { old_paint(\"red\", 1); old_paint(\"red\"); }",
        false,
    );
    assert_eq!(found.len(), 2, "{found:#?}");
    assert_eq!(found[0].code, codes::DEPRECATED_FUNCTION);
    assert_eq!(found[0].hint.as_deref(), Some("use paint(colour, amount)"));
    assert_eq!(found[1].code, codes::WRONG_ARITY);
}

/// The built-in set comes from the engine's metadata, not a hand list.
#[test]
fn the_builtins_are_rhais_packages() {
    let api = Api::of(&mut crate::ScriptSystem::new());
    for name in [
        "len",
        "push",
        "to_string",
        "abs",
        "contains",
        "keys",
        "sqrt",
        "floor",
    ] {
        assert!(api.builtins.contains_key(name), "{name}");
    }
    assert!(
        !api.builtins.contains_key("draw_rect"),
        "Flint's own are declared"
    );
    assert!(api.declared.contains_key("draw_rect"));
    assert!(api.len() > 300);
}

#[test]
fn a_script_dir_is_found_from_a_script() {
    assert_eq!(
        scene_dir_of(Path::new("levels/scripts/a.rhai")),
        Path::new("levels")
    );
    assert_eq!(
        scene_dir_of(Path::new("levels/scripts/lib/m.rhai")),
        Path::new("levels")
    );
    assert_eq!(scene_dir_of(Path::new("a.rhai")), Path::new("."));
    assert!(is_lib(Path::new("scripts/lib/m.rhai")));
    assert_eq!(
        fn_position("let x = 1;\n  private fn on_x(a) {}\n", "on_x"),
        (2, 11)
    );
}
