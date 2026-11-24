//! Stylize passes (#262): one interface for Kuwahara, ASCII, watercolor,
//! pencil and whatever comes after them.
//!
//! A style is a module in this directory that exports a
//! [`StyleDescriptor`] named `STYLE`, plus one name in [`register_styles!`]
//! at the bottom of this file. The descriptor says where the style sits in
//! the chain ([`StyleStage`]), which parameters it takes, when it is on,
//! and how to build its pipelines; the [`StylePass`] it builds records its
//! draws for one frame from a [`StyleFrame`]. Everything else is the
//! framework's:
//!
//! - **Pipelines** are built the first time a style is switched on
//!   ([`StyleSet::ensure`], from `SceneRenderer::ensure_style_resources`),
//!   under `catch_unwind`, since some drivers crash compiling a shader.
//!   A style that fails stays off and the renderer reports a lost device.
//! - **Targets.** Each stage hands a style its input and an output to draw
//!   into, ping-ponging between two targets the framework owns, so styles
//!   chain. A style's own intermediates come from [`StyleFrame::target`].
//!   All of them live in [`StyleTargets`] inside the post chain's
//!   resources, which `resize_postprocess()` rebuilds, so no style has a
//!   resize hook of its own and none can keep a stale view.
//! - **Bind groups** are cached with the chain (the same [`BindCache`] the
//!   rest of the post chain uses, #139), and samplers come from the chain's
//!   shared linear sampler: a style's bind groups hold no sampler of their
//!   own beyond that one, which keeps the DX12 sampler heap safe.
//! - **Config.** A style reads its parameters from
//!   [`PostProcessConfig::styles`], which a scene sets with
//!   `[post_process.styles.<name>]`; the F4 Rendering & Effects menu draws
//!   its sliders from the descriptor. Every style lives there, Kuwahara
//!   and contour included (#298, #599; a scene's older `kuwahara_*` and
//!   `contour_*` keys are folded into their tables on load,
//!   `flint_scene::fold_legacy_style_keys`).
//!
//! ## Order
//!
//! Styles of one stage run one after another, each reading the one before
//! it, in the order their tables give (`order = <n>` in
//! `[post_process.styles.<name>]`, #299; [`chain_order`]). A style without
//! one stands at its registration index, so a scene that says nothing
//! runs in registration order; ties keep registration order too. The
//! stages sit at two fixed places in the post chain:
//!
//! ```text
//! scene (HDR, render scale)
//!   -> style mask (Rg16Uint: id + blend, while a style reads it; #263, #278)
//!   -> shared edges (Rgba16Float, depth's size, while a style needs them; #301)
//!   -> Hdr styles (Kuwahara)                 linear Rgba16Float, scene resolution
//!   -> SSAO, volumetric (scene depth), bloom (original HDR, or the
//!      Hdr styles' result when one feeds bloom, #300)
//!   -> composite: fog, DoF, bloom add, tonemap, grade, vignette, grain, render modes
//!   -> Display styles (ASCII, pencil, invert) surface format, output resolution
//!   -> FXAA
//!   -> surface
//! ```
//!
//! ## The entity style mask
//!
//! A style that stylizes only chosen entities sets
//! [`StyleDescriptor::uses_mask`] and reads [`StyleFrame::mask`]: one id a
//! pixel, the entity's `style_mask` style ([`StyleFrame::style_id`] finds
//! the style's own), drawn after the scene pass (`crate::style_mask`).
//! `mask_view` is the example, and the `style` debug view.
//!
//! A per-entity style ([`StyleDescriptor::component`], #266) goes further:
//! an entity's own component switches it on and carries its settings, each
//! distinct set an id of its own, read with [`StyleFrame::variants`]. ASCII
//! is the example.
//!
//! ## Shared inputs
//!
//! Some inputs cost enough that two styles computing their own would
//! double the bill. A style lists them in [`StyleDescriptor::needs`] and
//! the framework makes each once a frame, for every style on that asks:
//! [`StyleNeed::Edges`] is the geometry edge texture ([`edges`], #301),
//! handed over as [`StyleFrame::edges`]. `edges_view` draws it, and
//! user styles get it with `inputs = ["edges"]`.
//!
//! An `Hdr` style is shaded by what follows it: fog fades it, bloom haloes
//! its bright parts, the grade tints it (Kuwahara's painterly look wants
//! that). A `Display` style sees the finished picture at output
//! resolution, after the render-scale upscale, which is where the ASCII
//! spike found glyphs have to be drawn to stay crisp (#261, finding 7).
//! Bloom reads the unstylized HDR by default (the light comes from the
//! scene, not the brush strokes), and a style that wants its own bright
//! shapes to glow says [`StyleDescriptor::feeds_bloom`], which a scene
//! overrides with `feeds_bloom` in the style's table (#300): when any HDR
//! style that ran feeds bloom, the threshold pass reads the stage's
//! result. SSAO and volumetrics read depth and shadows only, never colour,
//! so no style changes what they see; the composite lays both over the
//! stylized image.
//!
//! [`BindCache`]: crate::bind_cache::BindCache

pub mod catalog;
pub mod component_schema;
pub mod custom;
pub mod dev_shader;
pub mod edges;
mod fade;
pub mod history;
pub mod images;
pub mod kit;
pub mod ownership;
mod registry;
pub mod reveal;
mod spec;
mod targets;
pub mod text;
pub mod variant_table;

pub use ascii::{edge_view_thresholds, EDGE_VIEW_LEGEND};
pub use component_schema::{add_component_schemas, component_schema};
pub use edges::EDGE_FORMAT;
pub use registry::{intern, intern_list, List, Style, StyleRegistry};
pub use reveal::{RevealShape, StyleReveal};
pub use spec::{
    apply_style_spec, apply_style_specs, check_style_specs, style_spec_help, StyleSpec,
};
pub use targets::{HeldTargets, StyleTargets, StyleTexture, TargetEntry, TargetSummary};
pub use text::{GlyphCell, GlyphText};

use crate::bind_cache::BindCache;
use crate::postprocess::PostProcessConfig;
use crate::style_mask::StyleIds;
use std::collections::BTreeMap;
use std::sync::Arc;

mod descriptor;
mod frame;
mod set;
mod settings;
pub use descriptor::*;
pub use frame::*;
pub use set::*;
pub use settings::*;

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Declare the style modules and the registry in one list: adding a style
/// is its module plus its name here. Order is a stage's default chain
/// order; a scene's `order` keys can rearrange it ([`chain_order`]).
macro_rules! register_styles {
    ($($module:ident),* $(,)?) => {
        $(mod $module;)*
        static REGISTRY: &[&StyleDescriptor] = &[$(&$module::STYLE),*];
    };
}

mod blend;
mod reality;

register_styles!(
    kuwahara,
    watercolor,
    ascii,
    pencil,
    palette,
    display_bloom,
    invert,
    mask_view,
    edges_view,
    // Last of the built-ins, so the ink goes over every other HDR style
    // (Kuwahara's paint) as the old post pass did (#599)
    contour,
    // The reality styles (#746, render modes 1-5 until then). Registered
    // after the rest so no older style's index (which a scene's `order`
    // counts against) moves, and each placed by `default_order` first of
    // the display chain, where the composite drew them: a prop's ASCII or
    // pencil reads the code, the blood or the wire world they made
    matrix,
    blood,
    drunk,
    tron,
    underwater
);

/// The styles compiled into the engine, in their default chain order. A
/// [`StyleRegistry`] starts from these; game and user styles are a
/// registry's own (#742).
pub fn builtins() -> &'static [&'static StyleDescriptor] {
    REGISTRY
}

/// The built-in style called `name`.
pub fn builtin(name: &str) -> Option<&'static StyleDescriptor> {
    REGISTRY.iter().copied().find(|d| d.name == name)
}

/// Whether `name` is one of the styles compiled into the engine.
pub fn is_builtin(name: &str) -> bool {
    builtin(name).is_some()
}

/// Where the style called `name` stands in `styles`: an index into the
/// owner table the ownership pass reads (#384).
pub(crate) fn registry_index(styles: &StyleRegistry, d: &StyleDescriptor) -> Option<usize> {
    styles.index_of(d.name)
}

#[cfg(test)]
mod shared_wgsl_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn never_built(_: &StyleDevice) -> Box<dyn StylePass> {
        unreachable!("never built")
    }

    static GAME_WASH: StyleDescriptor =
        StyleDescriptor::new("utest_game_wash", "Wash", StyleStage::Hdr, never_built)
            .params(&[StyleParam::new("amount", 0.5, 0.0..=1.0)])
            .component("utest_game_wash_style");

    /// #419: a game's Rust style joins the registry after the built-ins,
    /// is read from a scene table and a `--style` word, is listed as the
    /// game's, and registering it again is a no-op.
    #[test]
    fn a_registered_game_style_is_a_style_like_a_builtin() {
        let mut styles = StyleRegistry::new();
        styles.register(&GAME_WASH).unwrap();
        styles
            .register(&GAME_WASH)
            .expect("the same descriptor again changes nothing");
        assert_eq!(
            styles
                .iter()
                .filter(|d| d.name == "utest_game_wash")
                .count(),
            1
        );
        let at = styles.index_of("utest_game_wash").unwrap();
        assert!(at >= REGISTRY.len(), "after the built-ins");
        assert!(styles.is_game_style("utest_game_wash") && styles.is_native("utest_game_wash"));
        assert!(!is_builtin("utest_game_wash"));
        // Another registry knows nothing of it (#742)
        assert!(StyleRegistry::new().find("utest_game_wash").is_none());

        let tables: BTreeMap<String, toml::Table> =
            toml::from_str("[utest_game_wash]\namount = 0.75\n").unwrap();
        let (settings, warnings) = StyleSettings::from_tables(&tables, &styles);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(settings.param("utest_game_wash", "amount"), Some(0.75));

        let mut config = PostProcessConfig::default();
        let errors =
            apply_style_specs(&mut config, &["utest_game_wash:amount=0.2".into()], &styles);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(config.styles.is_enabled("utest_game_wash"));
        assert_eq!(config.styles.param("utest_game_wash", "amount"), Some(0.2));

        let listing = catalog::listing_in(&styles, &GAME_WASH, None);
        assert_eq!(listing.source, "game");
    }

    #[test]
    fn register_refuses_taken_names_components_and_bad_names() {
        let mut styles = StyleRegistry::new();
        static BUILTIN: StyleDescriptor =
            StyleDescriptor::new("ascii", "Mine", StyleStage::Display, never_built);
        let e = styles.register(&BUILTIN).unwrap_err();
        assert!(e.contains("is a built-in style"), "{e}");
        assert_eq!(styles.find("ascii").unwrap().label, "ASCII");

        static FIRST: StyleDescriptor = StyleDescriptor::new(
            "utest_game_twice",
            "First",
            StyleStage::Display,
            never_built,
        );
        static SECOND: StyleDescriptor = StyleDescriptor::new(
            "utest_game_twice",
            "Second",
            StyleStage::Display,
            never_built,
        );
        styles.register(&FIRST).unwrap();
        let e = styles.register(&SECOND).unwrap_err();
        assert!(e.contains("already registered"), "{e}");
        assert_eq!(styles.find("utest_game_twice").unwrap().label, "First");

        static TAKES_ASCIIS: StyleDescriptor =
            StyleDescriptor::new("utest_game_comp", "Comp", StyleStage::Display, never_built)
                .component("ascii_style");
        let e = styles.register(&TAKES_ASCIIS).unwrap_err();
        assert!(
            e.contains("\"ascii_style\" is already style \"ascii\"'s"),
            "{e}"
        );
        assert!(styles.find("utest_game_comp").is_none());

        for bad in ["", "Sepia", "9lives", "two words", "dash-ed"] {
            // Leaked: a game's descriptor is 'static, and a refused one is
            // never kept
            let d: &'static StyleDescriptor = Box::leak(Box::new(StyleDescriptor::new(
                bad,
                "Bad",
                StyleStage::Display,
                never_built,
            )));
            let e = styles.register(d).unwrap_err();
            assert!(e.contains("lowercase"), "{bad:?}: {e}");
        }
    }
    /// #365: ASCII's tint and palette settings say they're unused outside
    /// their colour modes, and background outside the grounds and the
    /// colour mode that read it.
    #[test]
    fn hints_say_when_a_setting_is_unused() {
        let ascii = builtin("ascii").expect("ascii registered");
        let mut pp = PostProcessConfig::default();
        pp.styles.set_enabled("ascii", true);
        // Default colour mode is source: neither tint nor palette is read
        let tint = ascii
            .unused_reason(&pp, "tint")
            .expect("tint unused in source");
        assert!(tint.contains("Colour mode is source"), "{tint}");
        assert!(tint.contains("only tint reads it"), "{tint}");
        for name in ["palette", "chroma", "palette_dither"] {
            assert!(ascii.unused_reason(&pp, name).is_some(), "{name}");
        }
        // Settings without conditions are always read
        for name in ["cell", "glow", "color", "ground", "font"] {
            assert_eq!(ascii.unused_reason(&pp, name), None, "{name}");
        }
        assert_eq!(ascii.unused_reason(&pp, "background"), None);

        pp.styles.set_option("ascii", "color", "tint");
        assert_eq!(ascii.unused_reason(&pp, "tint"), None);
        assert!(ascii.unused_reason(&pp, "palette").is_some());

        pp.styles.set_option("ascii", "color", "palette");
        assert!(ascii.unused_reason(&pp, "tint").is_some());
        for name in ["palette", "chroma", "palette_dither"] {
            assert_eq!(ascii.unused_reason(&pp, name), None, "{name}");
        }
        // The palette brings its own ground colour
        let bg = ascii
            .unused_reason(&pp, "background")
            .expect("palette ground");
        assert_eq!(bg, "Unused while Colour mode is palette");

        pp.styles.set_option("ascii", "color", "mono");
        pp.styles.set_option("ascii", "ground", "black");
        assert!(ascii.unused_reason(&pp, "background").is_some());
        pp.styles.set_option("ascii", "ground", "inverse");
        assert_eq!(ascii.unused_reason(&pp, "background"), None);
    }

    #[test]
    fn a_setting_holds_its_conditions_in_order() {
        const P: StyleParam = StyleParam::new("x", 0.0, 0.0..=1.0)
            .label("X")
            .when("a", &["1"])
            .unless("b", &["2"]);
        let [Some(first), Some(second)] = P.ui.conditions else {
            panic!("two conditions");
        };
        assert!(first.holds("1") && !first.holds("3"));
        assert!(!second.holds("2") && second.holds("3"));
        const O: StyleOption = StyleOption::new("o", "", &[]).unless("a", &["1"]);
        assert!(O.ui.conditions[0].is_some_and(|c| c.unless) && O.ui.conditions[1].is_none());
        assert!(StyleParam::new("y", 0.0, 0.0..=1.0).ui.is_none());
        assert!(StyleOption::new("y", "", &[]).ui.is_none());
    }

    /// #342: two 120 Hz frames keep what one 60 Hz frame does, one 30 Hz
    /// frame what two do, and a dt that says nothing reads as 1/60.
    #[test]
    fn per_60hz_weights_keep_the_same_share_per_second() {
        let w = 0.6;
        assert!((per_60hz(w, 1.0 / 60.0) - w).abs() < 1e-6);
        assert!((per_60hz(w, 1.0 / 120.0).powi(2) - w).abs() < 1e-6);
        assert!((per_60hz(w, 1.0 / 30.0) - w * w).abs() < 1e-6);
        for dt in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(per_60hz(w, dt), per_60hz(w, 1.0 / 60.0), "dt {dt}");
        }
        assert_eq!(per_60hz(0.0, 1.0 / 144.0), 0.0);
    }

    #[test]
    fn registry_names_are_unique_and_lowercase() {
        let names = StyleRegistry::new().names();
        for (i, a) in names.iter().enumerate() {
            assert!(a.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{a}");
            assert!(!names[i + 1..].contains(a), "{a} registered twice");
        }
        assert!(names.contains(&"kuwahara") && names.contains(&"invert"));
    }

    #[test]
    fn param_defaults_sit_in_their_ranges() {
        for d in builtins() {
            for p in &d.params {
                assert!(
                    p.min <= p.default && p.default <= p.max,
                    "{}.{}",
                    d.name,
                    p.name
                );
            }
        }
    }

    #[test]
    fn settings_ui_is_where_the_menu_reads_it() {
        for d in builtins() {
            // A colour's label lives on its red channel (#510): one on green
            // or blue would never be shown
            for c in d.colors() {
                for p in &c.channels[1..] {
                    assert!(
                        p.ui.is_none(),
                        "{}: {}'s label, help or conditions go on {}",
                        d.name,
                        p.name,
                        c.channels[0].name
                    );
                }
            }
            // Every condition names one of the style's options, and a value
            // that option can take
            let uis = d.options.iter().map(|o| (o.name, &o.ui));
            for (name, ui) in uis.chain(d.params.iter().map(|p| (p.name, &p.ui))) {
                for c in ui.conditions.iter().flatten() {
                    let option = d.options.iter().find(|o| o.name == c.option);
                    let option = option.unwrap_or_else(|| {
                        panic!("{}.{name}: condition on no option {}", d.name, c.option)
                    });
                    for v in c.values {
                        assert!(
                            !option.closed() || option.choices.contains(v),
                            "{}.{name}: {} is never {v}",
                            d.name,
                            c.option
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn controls_fold_rgb_triples_and_keep_every_setting() {
        let ascii = builtin("ascii").unwrap();
        let rows = ascii.controls();
        // Options first
        let first_param = rows
            .iter()
            .position(|r| !matches!(r, StyleControl::Option(_)))
            .unwrap();
        assert_eq!(first_param, ascii.options.len());
        // tint_r/g/b became one colour row
        let colors: Vec<_> = rows
            .iter()
            .filter_map(|r| match r {
                StyleControl::Color { base, channels } => Some((*base, channels.map(|c| c.name))),
                _ => None,
            })
            .collect();
        assert_eq!(
            colors,
            vec![
                ("tint", ["tint_r", "tint_g", "tint_b"]),
                // #351
                (
                    "edge_color",
                    ["edge_color_r", "edge_color_g", "edge_color_b"]
                ),
            ]
        );
        // Every param is reachable exactly once
        for d in builtins() {
            let mut seen: Vec<&str> = Vec::new();
            for r in d.controls() {
                match r {
                    StyleControl::Param(p) => seen.push(p.name),
                    StyleControl::Color { channels, .. } => {
                        seen.extend(channels.iter().map(|c| c.name))
                    }
                    StyleControl::Option(_) => {}
                }
            }
            seen.sort();
            let mut all: Vec<&str> = d.params.iter().map(|p| p.name).collect();
            all.sort();
            assert_eq!(seen, all, "{}", d.name);
        }
    }

    #[test]
    fn a_colour_is_one_key_in_a_scene() {
        let ascii = builtin("ascii").unwrap();
        let colors = ascii.colors();
        // The tint, and the edge ink's colour (#351)
        assert_eq!(colors.len(), 2);
        assert_eq!(colors[0].base, "tint");
        assert_eq!(colors[1].base, "edge_color");
        assert_eq!(colors[0].default_rgb(), [0.35, 1.0, 0.45]);
        assert_eq!(ascii.color_of("tint_g").unwrap().base, "tint");
        assert!(ascii.color_of("glow").is_none());
        assert!(ascii.color_setting("glow").is_none());
        let keys = ascii.setting_keys();
        assert!(keys.contains(&"tint") && !keys.iter().any(|k| k.starts_with("tint_")));

        let read = |text: &str| {
            let tables: BTreeMap<String, toml::Table> = toml::from_str(text).unwrap();
            let (styles, warnings) =
                StyleSettings::from_tables(&tables, &crate::stylize::StyleRegistry::new());
            let config = PostProcessConfig {
                styles,
                ..Default::default()
            };
            (ascii.color(&config, "tint"), warnings)
        };
        // An array, integers allowed
        assert_eq!(
            read("[ascii]\ntint = [1.0, 0.7, 0]\n"),
            ([1.0, 0.7, 0.0], vec![])
        );
        // A hex string, in display values
        let (hex, warnings) = read("[ascii]\ntint = \"#ff8000\"\n");
        assert!(warnings.is_empty());
        assert_eq!(hex, [1.0, 128.0 / 255.0, 0.0]);
        // A channel on its own still works, the rest default
        assert_eq!(read("[ascii]\ntint_g = 0.2\n").0, [0.35, 0.2, 0.45]);
        // Not a colour: a warning that says what is, and the default holds
        for bad in ["tint = [1.0, 0.5]", "tint = \"green\"", "tint = 3"] {
            let (rgb, warnings) = read(&format!("[ascii]\n{bad}\n"));
            assert_eq!(rgb, [0.35, 1.0, 0.45], "{bad}");
            assert!(warnings[0].contains("[r, g, b]"), "{bad}: {warnings:?}");
        }
        // An unknown key lists the colour once, by its key
        let (_, warnings) = read("[ascii]\nhue = 1\n");
        assert!(
            warnings[0].contains("tint") && !warnings[0].contains("tint_r"),
            "{warnings:?}"
        );
    }

    #[test]
    fn colour_text_is_hex_or_slashes() {
        assert_eq!(color_from_text("#00ff00"), Some([0.0, 1.0, 0.0]));
        assert_eq!(color_from_text("0.3/1/0.4"), Some([0.3, 1.0, 0.4]));
        assert_eq!(color_from_text(" 1 / 0 / 0 "), Some([1.0, 0.0, 0.0]));
        for bad in ["", "#0f0", "#gg0000", "1/2", "1/2/3/4", "red", "nan/0/0"] {
            assert_eq!(color_from_text(bad), None, "{bad}");
        }
    }

    #[test]
    fn labels_come_from_the_setting_or_the_name() {
        let ascii = builtin("ascii").unwrap();
        assert_eq!(ascii.label_for("cell"), "Cell size (pt)");
        assert!(ascii.is_integer("cell"));
        assert!(!ascii.is_integer("glow"));
        assert_eq!(ascii.label_for("color"), "Colour mode");
        // A colour's are its red channel's; the channels read as themselves
        assert_eq!(ascii.label_for("tint"), "Tint");
        assert_eq!(ascii.help_for("tint"), "The ink for colour mode tint");
        assert_eq!(ascii.label_for("tint_r"), "Tint r");
        assert_eq!(ascii.help_for("tint_r"), "");
        assert!(ascii.ui("no_such_setting").is_none());
        assert_eq!(ascii.label_for("no_such_setting"), "No such setting");
        assert_eq!(humanize("tint_r"), "Tint r");
        assert_eq!(humanize(""), "");
    }

    #[test]
    fn style_toml_writes_only_changes_and_reads_back() {
        let ascii = builtin("ascii").unwrap();
        let mut config = PostProcessConfig::default();
        config.styles.set_enabled("ascii", true);
        assert_eq!(style_toml(ascii, &config), "[post_process.styles.ascii]\n");
        config.styles.set_param("ascii", "cell", 12.0);
        config.styles.set_param("ascii", "glow", 0.35);
        config.styles.set_param("ascii", "tint_r", 1.0);
        config.styles.set_option("ascii", "color", "palette");
        config.styles.set_option("ascii", "font", "builtin"); // the default
        let text = style_toml(ascii, &config);
        assert_eq!(
            text,
            "[post_process.styles.ascii]\ncolor = \"palette\"\ncell = 12\ntint = [1.0, 1.0, 0.45]\nglow = 0.35\n"
        );
        // It is the scene's syntax: parse it the way a scene is read
        let doc: toml::Table = text.parse().unwrap();
        let tables: BTreeMap<String, toml::Table> = doc["post_process"]["styles"]
            .as_table()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_table().unwrap().clone()))
            .collect();
        let (read, warnings) =
            StyleSettings::from_tables(&tables, &crate::stylize::StyleRegistry::new());
        assert!(warnings.is_empty(), "{warnings:?}");
        let back = PostProcessConfig {
            styles: read,
            ..Default::default()
        };
        for p in &ascii.params {
            assert_eq!(
                ascii.param(&back, p.name),
                ascii.param(&config, p.name),
                "{}",
                p.name
            );
        }
        for o in &ascii.options {
            assert_eq!(
                ascii.option(&back, o.name),
                ascii.option(&config, o.name),
                "{}",
                o.name
            );
        }
        // An off style says so
        config.styles.set_enabled("ascii", false);
        assert!(style_toml(ascii, &config).contains("enabled = false\n"));
    }

    #[test]
    fn reset_restore_and_compare() {
        let mut live = StyleSettings::default();
        live.set_enabled("ascii", true);
        live.set_param("ascii", "cell", 16.0);
        live.set_option("ascii", "ramp", "blocks");
        live.reset("ascii");
        assert!(live.is_enabled("ascii"));
        assert_eq!(live.param("ascii", "cell"), None);
        assert_eq!(live.option("ascii", "ramp"), None);

        let mut authored = StyleSettings::default();
        authored.set_enabled("ascii", true);
        authored.set_param("ascii", "cell", 10.0);
        assert!(!live.same_style("ascii", &authored));
        live.set_param("ascii", "glow", 1.0);
        live.restore("ascii", &authored);
        assert!(live.same_style("ascii", &authored));
        assert_eq!(live.param("ascii", "glow"), None);
        // Restoring a style the scene never set removes it
        live.set_enabled("invert", true);
        live.restore("invert", &authored);
        assert!(!live.is_enabled("invert"));
        assert!(live.same_style("invert", &StyleSettings::default()));
    }

    #[test]
    fn solo_keeps_only_the_named_style() {
        // The display stage as a scene with ASCII and the mask view has it
        let display = |names: &[&str]| -> Vec<(&'static StyleDescriptor, ())> {
            builtins()
                .iter()
                .filter(|d| names.contains(&d.name))
                .map(|d| (*d, ()))
                .collect()
        };
        let names = |styles: Vec<(&'static StyleDescriptor, ())>| -> Vec<&str> {
            styles.iter().map(|(d, _)| d.name).collect()
        };
        let on = ["ascii", "mask_view"];
        // The style debug view (#334): the mask view alone, reading the
        // stage's input, not ASCII's glyphs
        assert_eq!(names(solo(display(&on), Some("mask_view"))), ["mask_view"]);
        // An HDR stage without it runs nothing
        assert!(solo(display(&["kuwahara"]), Some("mask_view")).is_empty());
        // Otherwise the chain as it was
        assert_eq!(names(solo(display(&on), None)), ["ascii", "mask_view"]);
    }

    #[test]
    fn order_keys_rearrange_a_stage_and_default_to_registration() {
        let names = |config: &PostProcessConfig| -> Vec<&str> {
            StyleRegistry::new()
                .stage_chain(StyleStage::Display, config)
                .iter()
                .map(|(d, _)| d.name)
                .filter(|n| ["watercolor", "ascii", "pencil", "invert"].contains(n))
                .collect()
        };
        // Nothing said: registration order, which is today's
        let mut config = PostProcessConfig::default();
        assert_eq!(names(&config), ["watercolor", "ascii", "pencil", "invert"]);
        // ... but for a descriptor's own default place (#746: the reality
        // styles, registered last, run first)
        let mut registered: Vec<&str> = builtins()
            .iter()
            .filter(|d| d.stage == StyleStage::Display && d.default_order.is_none())
            .map(|d| d.name)
            .collect();
        let placed = ["matrix", "blood", "drunk", "tron", "underwater"];
        registered.splice(0..0, placed);
        let chain: Vec<&str> = StyleRegistry::new()
            .stage_chain(StyleStage::Display, &config)
            .iter()
            .map(|(d, _)| d.name)
            .collect();
        assert_eq!(chain, registered);

        // Watercolor over ASCII, from the scene's syntax
        let tables: BTreeMap<String, toml::Table> = toml::from_str(
            r#"
            [watercolor]
            order = 10
            [ascii]
            order = 2.5
            [invert]
            enabled = false
            order = "first"
            "#,
        )
        .unwrap();
        let (styles, warnings) =
            StyleSettings::from_tables(&tables, &crate::stylize::StyleRegistry::new());
        assert_eq!(warnings, vec!["styles.invert.order must be a number"]);
        assert_eq!(styles.order("watercolor"), Some(10.0));
        config.styles = styles;
        assert_eq!(names(&config), ["ascii", "pencil", "invert", "watercolor"]);
        // A tie keeps registration order: pencil (3) and ascii at 3
        config.styles.set_order("ascii", Some(3.0));
        assert_eq!(names(&config), ["ascii", "pencil", "invert", "watercolor"]);
        // Defaults puts it back
        config.styles.reset("watercolor");
        config.styles.set_order("ascii", None);
        assert_eq!(names(&config), ["watercolor", "ascii", "pencil", "invert"]);
        // Ordering a style it has no table for doesn't switch it on
        config.styles.set_order("pencil", Some(0.0));
        assert!(!config.styles.is_enabled("pencil"));
        assert_eq!(names(&config)[0], "pencil");
    }

    #[test]
    fn style_toml_keeps_the_order() {
        let watercolor = builtin("watercolor").unwrap();
        let mut config = PostProcessConfig::default();
        config.styles.set_enabled("watercolor", true);
        config.styles.set_order("watercolor", Some(5.0));
        assert_eq!(
            style_toml(watercolor, &config),
            "[post_process.styles.watercolor]\norder = 5\n"
        );
        config.styles.set_order("watercolor", Some(0.5));
        assert!(style_toml(watercolor, &config).contains("order = 0.5\n"));
    }

    #[test]
    fn no_style_names_a_setting_after_a_reserved_key() {
        for d in builtins() {
            for key in RESERVED_KEYS {
                assert!(
                    d.params.iter().all(|p| p.name != key)
                        && d.options.iter().all(|o| o.name != key),
                    "{}.{key}",
                    d.name
                );
            }
        }
    }

    #[test]
    fn plan_ping_pongs_and_ends_where_asked() {
        use Slot::*;
        assert_eq!(plan(0, true), vec![]);
        assert_eq!(plan(1, false), vec![(Input, Ping(0))]);
        assert_eq!(plan(1, true), vec![(Input, Final)]);
        assert_eq!(
            plan(3, false),
            vec![(Input, Ping(0)), (Ping(0), Ping(1)), (Ping(1), Ping(0))]
        );
        assert_eq!(
            plan(3, true),
            vec![(Input, Ping(0)), (Ping(0), Ping(1)), (Ping(1), Final)]
        );
        // No step reads the view it writes
        for n in 0..6 {
            for to_final in [false, true] {
                assert!(plan(n, to_final).iter().all(|(s, d)| s != d));
            }
        }
    }

    /// #302: the display stage reuses its input, and with FXAA on its
    /// intermediate, so no chain holds more than two full-size textures.
    #[test]
    fn the_display_plan_draws_through_two_textures_at_most() {
        use Slot::*;
        assert_eq!(display_plan(0, false), (Final, vec![]));
        assert_eq!(display_plan(1, false), (Input, vec![(Input, Final)]));
        assert_eq!(display_plan(1, true), (Input, vec![(Input, Final)]));
        assert_eq!(
            display_plan(2, false),
            (Input, vec![(Input, Ping(0)), (Ping(0), Final)])
        );
        assert_eq!(
            display_plan(2, true),
            (Final, vec![(Final, Input), (Input, Final)])
        );
        assert_eq!(
            display_plan(3, false),
            (
                Input,
                vec![(Input, Ping(0)), (Ping(0), Input), (Input, Final)]
            )
        );
        for n in 0..8 {
            for owned in [false, true] {
                let (first, steps) = display_plan(n, owned);
                assert_eq!(steps.len(), n);
                // Each style reads what the one before wrote (the first,
                // what the composite drew) and none reads what it writes
                let mut wrote = first;
                for (s, d) in &steps {
                    assert_eq!(*s, wrote, "{n} styles, fxaa {owned}");
                    assert_ne!(s, d);
                    wrote = *d;
                }
                assert_eq!(wrote, Final);
                assert!(steps.iter().all(|(s, _)| owned || *s != Final));
                assert!(steps.iter().all(|(s, d)| *s != Ping(1) && *d != Ping(1)));
                // Textures of the stage's own, plus FXAA's
                let full = plan_textures(&steps, true) + owned as usize;
                assert!(full <= 2, "{n} styles, fxaa {owned}: {full}");
            }
        }
        assert_eq!(plan_textures(&display_plan(1, false).1, true), 1);
        assert_eq!(plan_textures(&display_plan(1, true).1, true), 1);
        assert_eq!(plan_textures(&display_plan(2, true).1, true), 1);
        assert_eq!(plan_textures(&display_plan(3, false).1, true), 2);
        assert_eq!(plan_textures(&plan(3, false), false), 2);
    }

    #[test]
    fn tables_switch_styles_on_and_warn_on_unknowns() {
        let tables: BTreeMap<String, toml::Table> = toml::from_str(
            r#"
            [invert]
            strength = 0.5
            [nope]
            x = 1
            "#,
        )
        .unwrap();
        let (settings, warnings) =
            StyleSettings::from_tables(&tables, &crate::stylize::StyleRegistry::new());
        assert!(settings.is_enabled("invert"));
        assert_eq!(settings.param("invert", "strength"), Some(0.5));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("unknown style 'nope'"), "{warnings:?}");

        let tables: BTreeMap<String, toml::Table> =
            toml::from_str("[invert]\nenabled = false\nstrength = 1\nwobble = 3\n").unwrap();
        let (settings, warnings) =
            StyleSettings::from_tables(&tables, &crate::stylize::StyleRegistry::new());
        assert!(!settings.is_enabled("invert"));
        assert_eq!(
            settings.param("invert", "strength"),
            Some(1.0),
            "integers read as numbers"
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("no parameter 'wobble'"));
    }

    #[test]
    fn a_table_value_outside_its_range_warns_and_is_kept() {
        let tables: BTreeMap<String, toml::Table> =
            toml::from_str("[invert]\nstrength = 2\n[ascii]\ncell = 8\ntint = [0.3, 1.5, 0.4]\n")
                .unwrap();
        let (settings, warnings) =
            StyleSettings::from_tables(&tables, &crate::stylize::StyleRegistry::new());
        // Kept as written, as a stray option is: F4 shows it
        assert_eq!(settings.param("invert", "strength"), Some(2.0));
        assert_eq!(settings.param("ascii", "tint_g"), Some(1.5));
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings
                .iter()
                .any(|w| w.starts_with("styles.invert.strength=2 is outside its range 0..1")),
            "{warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("styles.ascii.tint: tint_g=1.5 is outside its range 0..1")),
            "{warnings:?}"
        );
    }

    /// #746: a whole-frame style follows the frame's tear by its table's
    /// `blend_source`, from a scene, `--style` and a script alike; Copy TOML
    /// writes it back, a reset takes it off, and with no tear the style
    /// leaves the chain.
    #[test]
    fn a_whole_frame_style_follows_the_tear_by_its_table() {
        let tables: BTreeMap<String, toml::Table> = toml::from_str(
            "[matrix]\nblend_source = \"tear\"\n[tron]\nblend_source = \"own\"\n[blood]\nblend_source = \"torn\"\n",
        )
        .unwrap();
        let (settings, warnings) =
            StyleSettings::from_tables(&tables, &crate::stylize::StyleRegistry::new());
        assert!(settings.follows_tear("matrix"));
        assert!(!settings.follows_tear("tron"));
        assert_eq!(
            warnings,
            ["styles.blood.blend_source must be \"own\" or \"tear\""]
        );

        let mut config = PostProcessConfig {
            styles: settings,
            ..Default::default()
        };
        assert!(config.styles.shows("matrix"));
        assert!(
            !config.styles.shows_in("matrix", &config),
            "no tear: out of the chain"
        );
        assert!(
            config.styles.shows_in("tron", &config),
            "its own blend needs no tear"
        );
        config.tear = 0.4;
        assert!(config.styles.shows_in("matrix", &config));
        let matrix = builtin("matrix").unwrap();
        assert!(style_toml(matrix, &config).contains("blend_source = \"tear\""));
        config.styles.reset("matrix");
        assert!(!config.styles.follows_tear("matrix"));

        apply_style_spec(
            &mut config,
            "tron:blend_source=tear",
            &crate::stylize::StyleRegistry::new(),
        )
        .unwrap();
        assert!(config.styles.follows_tear("tron") && config.styles.is_enabled("tron"));
        assert!(apply_style_spec(
            &mut config,
            "tron:blend_source=half",
            &crate::stylize::StyleRegistry::new()
        )
        .is_err());
        crate::post_style::set_post_style_param(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            "tron",
            "blend_source",
            "own".into(),
        )
        .unwrap();
        assert!(!config.styles.follows_tear("tron"));
        assert!(crate::post_style::set_post_style_param(
            &mut config,
            &crate::stylize::StyleRegistry::new(),
            "tron",
            "blend_source",
            1.0.into()
        )
        .is_err());
    }

    /// #746: the five reality styles are registered, display styles, and
    /// run first of the display chain, where the composite drew them.
    #[test]
    fn the_reality_styles_run_first_of_the_display_chain() {
        let config = PostProcessConfig::default();
        let chain: Vec<&str> = StyleRegistry::new()
            .stage_chain(StyleStage::Display, &config)
            .iter()
            .map(|(d, _)| d.name)
            .collect();
        assert_eq!(
            chain[..5],
            ["matrix", "blood", "drunk", "tron", "underwater"]
        );
        for name in crate::render_mode::STYLES {
            let d = builtin(name).unwrap();
            assert_eq!(d.stage, StyleStage::Display, "{name}");
            assert!(
                !d.yields,
                "{name} draws over every pixel, as the composite did"
            );
            assert!(!d.is_enabled(&config), "{name} is off until asked for");
        }
    }

    #[test]
    fn feeds_bloom_reads_from_the_table_for_hdr_styles_only() {
        let kuwahara = builtin("kuwahara").unwrap();
        // Kuwahara's default is unchanged: bloom reads the original scene
        assert!(!kuwahara.feeds_bloom);
        assert!(!kuwahara.feeds_bloom_in(&PostProcessConfig::default()));
        let tables: BTreeMap<String, toml::Table> = toml::from_str(
            "[kuwahara]\nfeeds_bloom = true\n[ascii]\nfeeds_bloom = true\n[invert]\nfeeds_bloom = 1\n",
        )
        .unwrap();
        let (settings, warnings) =
            StyleSettings::from_tables(&tables, &crate::stylize::StyleRegistry::new());
        let config = PostProcessConfig {
            styles: settings.clone(),
            ..Default::default()
        };
        assert!(kuwahara.feeds_bloom_in(&config));
        // A table switches its style on, Kuwahara's too since #298
        assert!(kuwahara.is_enabled(&config));
        assert!(
            warnings.iter().all(|w| !w.contains("kuwahara")),
            "{warnings:?}"
        );
        // ASCII draws after bloom: warned, ignored
        assert_eq!(settings.feeds_bloom("ascii"), None);
        assert!(!builtin("ascii").unwrap().feeds_bloom_in(&config));
        assert!(warnings
            .iter()
            .any(|w| w.contains("ascii") && w.contains("display")));
        assert!(warnings
            .iter()
            .any(|w| w.contains("invert.feeds_bloom must be true or false")));
        // Copy TOML writes it back, and Defaults clears it
        assert!(style_toml(kuwahara, &config).contains("feeds_bloom = true"));
        let mut reset = settings;
        reset.reset("kuwahara");
        assert_eq!(reset.feeds_bloom("kuwahara"), None);
    }

    #[test]
    fn tables_read_options_as_strings() {
        let tables: BTreeMap<String, toml::Table> =
            toml::from_str("[ascii]\ncell = 12\nramp = \"blocks\"\ncolor = \"tint\"\nfont = 3\n")
                .unwrap();
        let (settings, warnings) =
            StyleSettings::from_tables(&tables, &crate::stylize::StyleRegistry::new());
        assert!(settings.is_enabled("ascii"));
        assert_eq!(settings.param("ascii", "cell"), Some(12.0));
        assert_eq!(settings.option("ascii", "ramp"), Some("blocks"));
        assert_eq!(settings.option("ascii", "color"), Some("tint"));
        assert_eq!(warnings, vec!["styles.ascii.font must be a string"]);

        let ascii = builtin("ascii").unwrap();
        let mut config = PostProcessConfig::default();
        assert_eq!(ascii.option(&config, "font"), "builtin");
        assert_eq!(ascii.option(&config, "color"), "source");
        config.styles = settings;
        assert_eq!(ascii.option(&config, "ramp"), "blocks");
        assert_eq!(ascii.option(&config, "nope"), "");

        // An unknown key names the options too
        let tables: BTreeMap<String, toml::Table> =
            toml::from_str("[ascii]\nglyphs = 1\n").unwrap();
        let (_, warnings) =
            StyleSettings::from_tables(&tables, &crate::stylize::StyleRegistry::new());
        assert!(warnings[0].contains("ramp"), "{warnings:?}");
    }

    #[test]
    fn closed_options_warn_on_unknown_values_at_load() {
        // #318: a colour mode is one of a fixed few; "plaid" is warned with
        // the list and the default it draws as, and is kept as written
        let tables: BTreeMap<String, toml::Table> =
            toml::from_str("[ascii]\ncolor = \"plaid\"\n").unwrap();
        let (settings, warnings) =
            StyleSettings::from_tables(&tables, &crate::stylize::StyleRegistry::new());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("styles.ascii.color")
                && warnings[0].contains("'plaid'")
                && warnings[0].contains("source, tint, raw")
                && warnings[0].contains("drawing as 'source'"),
            "{warnings:?}"
        );
        assert_eq!(settings.option("ascii", "color"), Some("plaid"));

        // Every closed option of every built-in style warns the same way
        for d in builtins() {
            for o in d.options.iter().filter(|o| o.closed()) {
                assert!(
                    !o.choices.is_empty(),
                    "{}.{} closed on nothing",
                    d.name,
                    o.name
                );
                assert!(o.choices.contains(&o.default), "{}.{}", d.name, o.name);
                let toml = format!("[{}]\n{} = \"plaid\"\n", d.name, o.name);
                let tables: BTreeMap<String, toml::Table> = toml::from_str(&toml).unwrap();
                let (_, warnings) =
                    StyleSettings::from_tables(&tables, &crate::stylize::StyleRegistry::new());
                assert_eq!(warnings.len(), 1, "{}.{}: {warnings:?}", d.name, o.name);
            }
        }

        // A known value is quiet, and open lists only suggest: a glyph set's
        // own ramp and a palette file load without a word
        let tables: BTreeMap<String, toml::Table> = toml::from_str(
            "[ascii]\ncolor = \"palette\"\nramp = \"runes\"\n\
             palette = \"palettes/dusk.palette.toml\"\nfont = \"mono\"\n\
             [pencil]\ndirection = \"tensor\"\n",
        )
        .unwrap();
        let (_, warnings) =
            StyleSettings::from_tables(&tables, &crate::stylize::StyleRegistry::new());
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn descriptor_params_fall_back_to_defaults() {
        let invert = builtin("invert").unwrap();
        let mut config = PostProcessConfig::default();
        assert!(!invert.is_enabled(&config));
        assert_eq!(invert.param(&config, "strength"), 1.0);
        config.styles.set_enabled("invert", true);
        config.styles.set_param("invert", "strength", 0.25);
        assert!(invert.is_enabled(&config));
        assert_eq!(invert.param(&config, "strength"), 0.25);

        let kuwahara = builtin("kuwahara").unwrap();
        assert!(!kuwahara.is_enabled(&config));
        assert_eq!(kuwahara.param(&config, "radius"), 4.0);
        config.styles.set_enabled("kuwahara", true);
        assert!(kuwahara.is_enabled(&config));
    }
}
