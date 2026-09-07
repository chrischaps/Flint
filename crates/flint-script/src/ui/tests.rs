//! Unit tests for the data-driven UI system: tokens, runtime style
//! completeness, hot reload, hit testing. No GPU, no Rhai — everything goes
//! through `UiSystem` and the loaders directly.

use super::element::StyleValue;
use super::style::{apply_property, is_known_property, ResolvedStyle, TextAlign};
use super::UiSystem;
use std::path::{Path, PathBuf};

/// A throwaway project root under the OS temp dir, removed on drop.
struct TempProject {
    root: PathBuf,
}

impl TempProject {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "flint_ui_test_{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("ui")).unwrap();
        Self { root }
    }

    fn write(&self, rel: &str, content: &str) -> PathBuf {
        let p = self.root.join(rel);
        std::fs::write(&p, content).unwrap();
        p
    }

    fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

const THEME: &str = r#"
[meta]
name = "Test"

[color]
accent = [1.0, 0.55, 0.15, 1.0]
paper = [0.96, 0.96, 0.98]

[font]
label = "Barlow-SemiBold"
label_tracking = 1.2

[type]
lg = 20

[shape]
radius = 4
"#;

const LAYOUT: &str = r#"
[ui]
style = "ui/test.style.toml"

[elements.root]
type = "panel"
anchor = "top-left"
class = "root"

[elements.header]
type = "text"
parent = "root"
class = "header"
text = "HEADER"

[elements.button]
type = "rect"
anchor = "top-left"
class = "button"
"#;

const STYLE: &str = r#"
[tokens]
import = "ui/theme.toml"
local_pad = [4, 8, 4, 8]
brand = "$color.accent"
nope_ref = "$does_not_exist"

[styles.root]
width_pct = 100
height_pct = 100

[styles.header]
color = "$accent"
font = "$label"
font_size = "$type.lg"
letter_spacing = "$label_tracking"
rounding = "$radius"
padding = "$local_pad"
bg_color = "$brand"
stroke_color = "$paper"
thickness = "$missing_token"
text_align = "center"

[styles.button]
x = 100
y = 50
width = 200
height = 40
"#;

fn project() -> (TempProject, UiSystem, i64) {
    let tp = TempProject::new("ui");
    tp.write("ui/theme.toml", THEME);
    tp.write("ui/test.ui.toml", LAYOUT);
    tp.write("ui/test.style.toml", STYLE);
    let mut sys = UiSystem::new();
    let h = sys.load("ui/test.ui.toml", tp.path());
    assert!(h > 0, "load should succeed");
    (tp, sys, h)
}

fn header_style(sys: &mut UiSystem) -> ResolvedStyle {
    sys.get_rect("header", 1280.0, 720.0)
        .expect("header laid out");
    let doc = &sys.documents[0];
    doc.cached_rects.get("header").unwrap().1.clone()
}

// ── Tokens ──────────────────────────────────────────────────────────────

#[test]
fn tokens_resolve_local_shared_and_qualified() {
    let (_tp, mut sys, _) = project();
    let st = header_style(&mut sys);

    // shared, unqualified
    assert_eq!(st.color, [1.0, 0.55, 0.15, 1.0]);
    // shared string + number
    assert_eq!(st.font.as_deref(), Some("Barlow-SemiBold"));
    assert_eq!(st.letter_spacing, 1.2);
    // section-qualified
    assert_eq!(st.font_size, 20.0);
    // integer token → float property
    assert_eq!(st.rounding, 4.0);
    // local [tokens] array as padding
    assert_eq!(st.padding, [4.0, 8.0, 4.0, 8.0]);
    // local token that is itself a shared reference
    assert_eq!(st.bg_color, [1.0, 0.55, 0.15, 1.0]);
    // 3-component colour gets alpha 1
    assert_eq!(st.stroke_color, [0.96, 0.96, 0.98, 1.0]);
    // plain strings still work
    assert_eq!(st.text_align, TextAlign::Center);
}

#[test]
fn unknown_token_leaves_property_unset() {
    let (_tp, mut sys, _) = project();
    let st = header_style(&mut sys);
    // `thickness = "$missing_token"` → default stays
    assert_eq!(st.thickness, ResolvedStyle::default().thickness);
    let doc = &sys.documents[0];
    assert!(!doc.styles["header"].properties.contains_key("thickness"));
}

#[test]
fn ui_token_lookup_forms() {
    let (_tp, sys, _) = project();
    assert_eq!(
        sys.token("accent")
            .and_then(|v| v.as_array().map(|a| a.len())),
        Some(4)
    );
    assert!(sys.token("$accent").is_some());
    assert!(sys.token("color.accent").is_some());
    assert!(sys.token("$color.accent").is_some());
    assert_eq!(
        sys.token("label")
            .and_then(|v| v.as_str().map(String::from)),
        Some("Barlow-SemiBold".into())
    );
    assert_eq!(sys.token("radius").and_then(|v| v.as_integer()), Some(4));
    // local table wins and is reachable
    assert!(sys.token("local_pad").is_some());
    // local "$ref" token dereferences into the shared file
    assert!(sys
        .token("brand")
        .and_then(|v| v.as_array().cloned())
        .is_some());
    assert!(sys.token("shape.accent").is_none(), "wrong section");
    assert!(sys.token("nothing").is_none());
    assert!(sys.token("import").is_none(), "import key is not a token");
    assert!(sys.token("").is_none());
}

// ── Runtime style completeness ──────────────────────────────────────────

#[test]
fn apply_property_covers_every_known_property() {
    let mut st = ResolvedStyle::default();
    let f = StyleValue::Float(7.0);
    let c = StyleValue::Color([0.1, 0.2, 0.3, 0.4]);
    let sh = StyleValue::Shadow(1.0, 2.0, [0.0, 0.0, 0.0, 0.5]);
    let b = StyleValue::Bool(true);
    for prop in super::style::KNOWN_PROPERTIES {
        let val = match *prop {
            "color" | "bg_color" | "stroke_color" | "padding" => c.clone(),
            "shadow" => sh.clone(),
            "height_auto" => b.clone(),
            "text_align" => StyleValue::String("right".into()),
            "layout" => StyleValue::String("horizontal".into()),
            "font" => StyleValue::String("Foo".into()),
            _ => f.clone(),
        };
        assert!(apply_property(&mut st, prop, &val), "{prop} rejected");
        assert!(is_known_property(prop));
    }
    assert_eq!(st.text_align, TextAlign::Right);
    assert_eq!(st.layout, super::style::LayoutFlow::Horizontal);
    assert_eq!(st.font.as_deref(), Some("Foo"));
    assert!(st.height_auto);
    assert_eq!(st.width_pct, Some(7.0));
    assert_eq!(st.height_pct, Some(7.0));
    assert_eq!(st.margin_bottom, 7.0);
    assert_eq!(st.thickness, 7.0);
    assert_eq!(st.stroke_width, 7.0);
    assert_eq!(st.padding, [0.1, 0.2, 0.3, 0.4]);
    assert_eq!(st.shadow, Some(([0.0, 0.0, 0.0, 0.5], 1.0, 2.0)));
    assert!(!apply_property(&mut st, "bogus", &f));
    assert!(!is_known_property("bogus"));
}

#[test]
fn set_style_accepts_token_strings_and_new_props() {
    let (_tp, mut sys, _) = project();

    sys.set_style("button", "color", StyleValue::String("$paper".into()));
    sys.set_style(
        "button",
        "stroke_width",
        StyleValue::String("$radius".into()),
    );
    sys.set_style("button", "font", StyleValue::String("$font.label".into()));
    sys.set_style("button", "text_align", StyleValue::String("right".into()));
    sys.set_style("button", "height_auto", StyleValue::Bool(true));
    sys.set_style("button", "width_pct", StyleValue::Float(50.0));
    sys.set_style("button", "layout", StyleValue::String("horizontal".into()));
    sys.set_style("button", "padding", StyleValue::Color([1.0, 2.0, 3.0, 4.0]));
    // unknown token: ignored, no override stored
    sys.set_style("button", "rounding", StyleValue::String("$nope".into()));
    // unknown prop: warned once, ignored
    sys.set_style("button", "glow", StyleValue::Float(1.0));
    sys.set_style("button", "glow", StyleValue::Float(2.0));

    sys.get_rect("button", 1280.0, 720.0).unwrap();
    let (rect, st) = sys.documents[0].cached_rects["button"].clone();
    assert_eq!(st.color, [0.96, 0.96, 0.98, 1.0]);
    assert_eq!(st.stroke_width, 4.0);
    assert_eq!(st.font.as_deref(), Some("Barlow-SemiBold"));
    assert_eq!(st.text_align, TextAlign::Right);
    assert!(st.height_auto);
    assert_eq!(st.layout, super::style::LayoutFlow::Horizontal);
    assert_eq!(st.padding, [1.0, 2.0, 3.0, 4.0]);
    assert_eq!(rect.w, 640.0, "width_pct override applies to layout");
    assert_eq!(st.rounding, 0.0);

    let ov = &sys.documents[0]
        .find_element("button")
        .unwrap()
        .style_overrides;
    assert!(!ov.contains_key("rounding"));
    assert!(!ov.contains_key("glow"));
    assert_eq!(sys.warned_props.len(), 1);
}

// ── Hot reload ──────────────────────────────────────────────────────────

#[test]
fn reload_reparses_and_preserves_overrides() {
    let (tp, mut sys, handle) = project();

    sys.set_text("header", "LIVE");
    sys.set_color("header", 0.0, 1.0, 0.0, 1.0);
    sys.set_visible("button", false);
    sys.set_style("button", "x", StyleValue::Float(321.0));
    sys.set_class("button", "root");

    assert_eq!(sys.poll_reload(), 0, "nothing changed yet");

    // Change the theme (accent) and the layout (add an element, drop the
    // button's class). Content lengths differ, so the fingerprint changes
    // even if the filesystem mtime granularity is coarse.
    tp.write(
        "ui/theme.toml",
        &THEME.replace(
            "accent = [1.0, 0.55, 0.15, 1.0]",
            "accent = [0.0, 0.0, 1.0, 1.0]",
        ),
    );
    tp.write(
        "ui/test.ui.toml",
        &format!(
            "{LAYOUT}\n[elements.extra]\ntype = \"text\"\nparent = \"root\"\nclass = \"header\"\ntext = \"NEW\"\n"
        ),
    );

    assert_eq!(sys.poll_reload(), 1);
    assert_eq!(sys.poll_reload(), 0, "stamps refreshed after reload");
    assert_eq!(sys.handle_map.get(&handle), Some(&0), "handle survives");

    // New element present, token change visible through the style
    assert!(sys.exists("extra"));
    let st = header_style(&mut sys);
    assert_eq!(
        st.color,
        [0.0, 1.0, 0.0, 1.0],
        "script colour override still wins"
    );
    let doc = &sys.documents[0];
    let hdr_class = doc.styles["header"].resolve();
    assert_eq!(
        hdr_class.color,
        [0.0, 0.0, 1.0, 1.0],
        "new token value parsed"
    );

    // Overrides restored
    let header = doc.find_element("header").unwrap();
    assert_eq!(header.effective_text(), "LIVE");
    let button = doc.find_element("button").unwrap();
    assert!(!button.visible);
    assert_eq!(button.effective_class(), "root");
    assert_eq!(
        button.style_overrides.get("x"),
        Some(&StyleValue::Float(321.0))
    );
}

#[test]
fn reload_with_broken_layout_keeps_old_document() {
    let (tp, mut sys, _) = project();
    sys.set_text("header", "KEEP");
    tp.write("ui/test.ui.toml", "[ui\nthis is not toml");
    assert_eq!(sys.poll_reload(), 0);
    assert_eq!(
        sys.poll_reload(),
        0,
        "broken state is remembered, no re-parse spam"
    );
    assert!(sys.exists("header"));
    assert_eq!(
        sys.documents[0]
            .find_element("header")
            .unwrap()
            .effective_text(),
        "KEEP"
    );
    // Fixing the file reloads again
    tp.write("ui/test.ui.toml", LAYOUT);
    assert_eq!(sys.poll_reload(), 1);
}

#[test]
fn reload_drops_overrides_for_removed_elements() {
    let (tp, mut sys, _) = project();
    sys.set_text("button", "x");
    tp.write(
        "ui/test.ui.toml",
        &LAYOUT.replace("[elements.button]", "[elements.other_button]"),
    );
    assert_eq!(sys.poll_reload(), 1);
    assert!(!sys.exists("button"));
    assert!(sys.exists("other_button"));
    assert!(sys.documents[0]
        .find_element("other_button")
        .unwrap()
        .text_override
        .is_none());
}

// ── Hit test / load failure ─────────────────────────────────────────────

#[test]
fn hit_uses_resolved_rect() {
    let (_tp, mut sys, _) = project();
    // button: x=100 y=50 w=200 h=40
    assert!(sys.hit("button", 100.0, 50.0, 1280.0, 720.0));
    assert!(sys.hit("button", 299.0, 89.0, 1280.0, 720.0));
    assert!(
        !sys.hit("button", 300.0, 89.0, 1280.0, 720.0),
        "right edge exclusive"
    );
    assert!(!sys.hit("button", 99.0, 60.0, 1280.0, 720.0));
    assert!(!sys.hit("button", 150.0, 90.0, 1280.0, 720.0));
    assert!(!sys.hit("missing", 0.0, 0.0, 1280.0, 720.0));
    // width_pct root covers the whole screen
    assert!(sys.hit("root", 1279.0, 719.0, 1280.0, 720.0));
    assert!(!sys.hit("root", 1280.0, 719.0, 1280.0, 720.0));
}

#[test]
fn load_missing_layout_returns_minus_one() {
    let tp = TempProject::new("missing");
    let mut sys = UiSystem::new();
    assert_eq!(sys.load("ui/nope.ui.toml", tp.path()), -1);
    assert!(sys.documents.is_empty());
}

#[test]
fn shared_tokens_via_string_and_tokens_file_keys() {
    for header in [
        "tokens = \"ui/theme.toml\"
",
        "tokens_file = \"ui/theme.toml\"
",
    ] {
        let tp = TempProject::new("keys");
        tp.write("ui/theme.toml", THEME);
        tp.write("ui/test.ui.toml", LAYOUT);
        tp.write(
            "ui/test.style.toml",
            &format!(
                "{header}[styles.button]
color = \"$accent\"
"
            ),
        );
        let mut sys = UiSystem::new();
        assert!(sys.load("ui/test.ui.toml", tp.path()) > 0);
        assert_eq!(
            sys.documents[0].styles["button"].resolve().color,
            [1.0, 0.55, 0.15, 1.0],
            "{header}"
        );
    }
}

#[test]
fn style_without_tokens_still_loads() {
    let tp = TempProject::new("plain");
    tp.write("ui/test.ui.toml", LAYOUT);
    tp.write(
        "ui/test.style.toml",
        "[styles.button]\nx = 1\ncolor = [1, 0, 0, 1]\n",
    );
    let mut sys = UiSystem::new();
    assert!(sys.load("ui/test.ui.toml", tp.path()) > 0);
    assert!(sys.token("accent").is_none());
    let (_, st) = {
        sys.get_rect("button", 100.0, 100.0).unwrap();
        sys.documents[0].cached_rects["button"].clone()
    };
    assert_eq!(st.color, [1.0, 0.0, 0.0, 1.0]);
}
