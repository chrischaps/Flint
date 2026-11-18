//! What a style is: its descriptor, parameters, options and how they are
//! presented, colours, and Copy TOML (moved from `stylize/mod.rs`, #742).

use super::*;

/// Where in the post chain a style runs; see the module docs for the order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StyleStage {
    /// Linear HDR (`Rgba16Float`) at the scene's render scale, after the
    /// scene pass and before fog, bloom, grade and tonemapping.
    Hdr,
    /// Display-referred colour in the surface format, at output
    /// resolution, after the composite and before FXAA. When the surface
    /// format is sRGB, samples arrive linear and writes are encoded on the
    /// way out: a style that works in display values decodes and encodes
    /// itself ([`StyleFrame::srgb`]).
    Display,
}

impl StyleStage {
    /// Both stages, in chain order.
    pub const ALL: [StyleStage; 2] = [StyleStage::Hdr, StyleStage::Display];
}

/// One numeric parameter a style takes, with the range the F4 menu offers.
/// A value outside it is refused by `--style` and warned about in a scene
/// table (#355): a style clamps it or draws it badly.
///
/// How the menu presents it rides on it too (#510): `.label(..)`,
/// `.help(..)`, `.integer()`, `.when(..)` / `.unless(..)` ([`StyleUi`]).
/// A colour's (`tint`, three parameters `tint_r/_g/_b`) are set on its red
/// channel, where the menu's picker stands; on the other two they mean
/// nothing.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct StyleParam {
    pub name: &'static str,
    pub default: f32,
    pub min: f32,
    pub max: f32,
    /// Its label, help, whole-number stepping and conditions.
    pub ui: StyleUi,
}

impl StyleParam {
    /// A parameter with its default and the range the menu offers:
    /// `StyleParam::new("cell", 8.0, 4.0..=32.0)`. Build every parameter
    /// through this (#366): a field added later gets its default here, so
    /// no style has to change.
    pub const fn new(
        name: &'static str,
        default: f32,
        range: std::ops::RangeInclusive<f32>,
    ) -> Self {
        Self {
            name,
            default,
            min: *range.start(),
            max: *range.end(),
            ui: StyleUi::NONE,
        }
    }

    /// What the F4 menu calls it ("Cell size (pt)"); unset, its name made
    /// readable ("Cell").
    pub const fn label(mut self, label: &'static str) -> Self {
        self.ui.label = label;
        self
    }

    /// One line for the hover text.
    pub const fn help(mut self, help: &'static str) -> Self {
        self.ui.help = help;
        self
    }

    /// Whole numbers only (a cell size, a count): the slider steps by 1
    /// and Copy TOML writes `cell = 8`.
    pub const fn integer(mut self) -> Self {
        self.ui.integer = true;
        self
    }

    /// Read only while option `option` is one of `values` (#365):
    /// `.when("color", &["tint"])`.
    pub const fn when(mut self, option: &'static str, values: &'static [&'static str]) -> Self {
        self.ui = self.ui.condition(option, values, false);
        self
    }

    /// Does nothing while option `option` is one of `values` (#365):
    /// `.unless("color", &["palette"])`.
    pub const fn unless(mut self, option: &'static str, values: &'static [&'static str]) -> Self {
        self.ui = self.ui.condition(option, values, true);
        self
    }

    /// Why `value` won't draw as written, when it lies outside
    /// `min..=max`: "cell=400 is outside its range 4..32". `None` inside.
    pub fn range_error(&self, value: f32) -> Option<String> {
        (value < self.min || value > self.max).then(|| {
            format!(
                "{}={value} is outside its range {}..{}",
                self.name, self.min, self.max
            )
        })
    }

    /// `value` brought into `min..=max`, as a script's setter keeps it,
    /// and, when it was outside, [`range_error`](Self::range_error)'s words
    /// and what it became: "cell=400 is outside its range 4..32; clamped
    /// to 32" (#492), for the script's author.
    pub fn clamp(&self, value: f64) -> (f64, Option<String>) {
        let kept = value.clamp(self.min as f64, self.max as f64);
        let note = self
            .range_error(value as f32)
            .map(|e| format!("{e}; clamped to {}", kept as f32));
        (kept, note)
    }
}

/// One named choice a style takes (a font, a ramp, a colour mode): a
/// string in `[post_process.styles.<name>]`, where a parameter is a number.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct StyleOption {
    pub name: &'static str,
    pub default: &'static str,
    /// The values the F4 menu offers. Empty means any text (a file or set
    /// name). A style still gets whatever the scene says and decides what
    /// a value outside the list means. A user style's list is interned
    /// ([`intern_list`]), so an option stays `Copy` and a game's `static`
    /// can list its options inline.
    pub choices: &'static [&'static str],
    /// Choices found at run time (#322): the scene's own palettes or glyph
    /// sets, or the ramps of the font chosen. When set, its list replaces
    /// `choices` wherever a scene is known ([`Self::choices_in`]), and the
    /// option is open: a value outside either list (a file path) is the
    /// style's to resolve, so `--style` doesn't refuse it.
    pub discover: Option<fn(&ChoiceScope<'_>) -> Vec<String>>,
    /// Its label, help and conditions ([`StyleUi`], #510).
    pub ui: StyleUi,
}

impl StyleOption {
    /// A closed or suggested list with nothing found at run time.
    pub const fn new(
        name: &'static str,
        default: &'static str,
        choices: &'static [&'static str],
    ) -> Self {
        Self {
            name,
            default,
            choices,
            discover: None,
            ui: StyleUi::NONE,
        }
    }

    /// The same option, with its choices found at run time.
    pub const fn discovered(mut self, discover: fn(&ChoiceScope<'_>) -> Vec<String>) -> Self {
        self.discover = Some(discover);
        self
    }

    /// What the F4 menu calls it ("Colour mode"); unset, its name made
    /// readable.
    pub const fn label(mut self, label: &'static str) -> Self {
        self.ui.label = label;
        self
    }

    /// One line for the hover text.
    pub const fn help(mut self, help: &'static str) -> Self {
        self.ui.help = help;
        self
    }

    /// Read only while option `option` is one of `values` (#365).
    pub const fn when(mut self, option: &'static str, values: &'static [&'static str]) -> Self {
        self.ui = self.ui.condition(option, values, false);
        self
    }

    /// Does nothing while option `option` is one of `values` (#365).
    pub const fn unless(mut self, option: &'static str, values: &'static [&'static str]) -> Self {
        self.ui = self.ui.condition(option, values, true);
        self
    }

    /// Whether `choices` are the only values (#318): a fixed list with
    /// nothing found at run time (a colour mode, a scope). A scene asking
    /// for another is warned at load with the list. Discovered choices are
    /// suggestions (a font's ramps, a palette that may be a file path), and
    /// an empty list takes any text.
    pub fn closed(&self) -> bool {
        self.discover.is_none() && !self.choices.is_empty()
    }

    /// A value this option will never mean: outside a closed list.
    pub fn refuses(&self, value: &str) -> bool {
        self.closed() && !self.choices.contains(&value)
    }

    /// The values to offer for this option in `scope`: what `discover`
    /// finds when the option has it, else its fixed `choices`. The F4 menu
    /// and the unknown-value check (#318) read this one list.
    pub fn choices_in(&self, scope: &ChoiceScope<'_>) -> Vec<String> {
        match self.discover {
            Some(discover) => discover(scope),
            None => self.choices.iter().map(|c| c.to_string()).collect(),
        }
    }
}

/// What an option's run-time choices may depend on (#322): the scene's
/// project and folder, and the style's other options as the menu shows
/// them (the ramps a font has depend on the font).
pub struct ChoiceScope<'a> {
    /// Where the scene's files are searched from (ADR 0114); `None` when
    /// there is no scene, and then only what is built in is offered.
    pub assets: Option<&'a flint_core::project::AssetScope>,
    /// Another option's current value (its default when unset).
    pub option: &'a dyn Fn(&str) -> String,
}

impl<'a> ChoiceScope<'a> {
    /// A scope for `assets` with every other option at its default.
    pub fn defaults(assets: Option<&'a flint_core::project::AssetScope>) -> Self {
        Self {
            assets,
            option: &|_| String::new(),
        }
    }

    /// The names of every file of `kind` the scene can see, sorted: exactly
    /// the files the project's one search finds
    /// ([`Project::list`](flint_core::project::Project::list): the kind's
    /// folder beside the scene, in each directory above it up to the
    /// project root, then in the mounted engine; a nearer name hides a
    /// farther one). Read through the vfs, so a bundle on the web lists the
    /// same files.
    pub fn files(&self, kind: flint_core::project::AssetKind) -> Vec<String> {
        let Some(assets) = self.assets else {
            return Vec::new();
        };
        let mut names: Vec<String> = assets.list(kind).into_iter().map(|l| l.name).collect();
        names.sort();
        names
    }
}

/// `fixed`, then each of `found` not already in it.
pub fn choices_with(fixed: &[&str], found: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = fixed.iter().map(|c| c.to_string()).collect();
    for name in found {
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// How the F4 menu, `--list-styles` and Copy TOML present one parameter,
/// option or colour (#268): a readable label, a hover line, whether only
/// whole numbers make sense, and when the setting is read. Set on the
/// setting itself (#510: `StyleParam::new(..).label(..).help(..)`), a
/// colour's on its red channel. All of it is optional: a setting with none
/// shows its name ("edge_max" -> "Edge max") on a plain slider, so a new
/// style needs none to be tunable.
///
/// A colour is three parameters named `<base>_r`, `<base>_g`, `<base>_b`
/// (each 0..1, display values: what the eye sees, sRGB-encoded): the
/// menu folds them into one colour picker. See [`StyleColor`].
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct StyleUi {
    /// Empty: the name made readable ([`StyleDescriptor::label_for`]).
    pub label: &'static str,
    /// One line for the hover text; empty for none.
    pub help: &'static str,
    /// Whole numbers only (a cell size, a count): the slider steps by 1.
    pub integer: bool,
    /// When the setting matters (#365): up to two conditions on options,
    /// from `when` and `unless`, all of which must hold for the style to
    /// read it. While one fails the F4 menu greys the row out and says
    /// why. None: always read.
    pub conditions: [Option<StyleWhen>; 2],
}

/// One condition of a [`StyleUi`]: the setting is read only while option
/// `option` is one of `values` (or, with `unless`, none of them).
#[derive(Clone, Copy, Debug)]
pub struct StyleWhen {
    pub option: &'static str,
    pub values: &'static [&'static str],
    pub unless: bool,
}

impl StyleWhen {
    /// Whether the setting is read while the option is `current`.
    pub fn holds(&self, current: &str) -> bool {
        self.values.contains(&current) != self.unless
    }
}

impl StyleUi {
    /// No label, help or conditions: what every setting starts with.
    pub const NONE: StyleUi = StyleUi {
        label: "",
        help: "",
        integer: false,
        conditions: [None, None],
    };

    /// Whether it says nothing at all.
    pub fn is_none(&self) -> bool {
        self.label.is_empty()
            && self.help.is_empty()
            && !self.integer
            && self.conditions.iter().all(Option::is_none)
    }

    pub(super) const fn condition(
        self,
        option: &'static str,
        values: &'static [&'static str],
        unless: bool,
    ) -> Self {
        let c = StyleWhen {
            option,
            values,
            unless,
        };
        let conditions = match self.conditions {
            [None, b] => [Some(c), b],
            [a @ Some(_), None] => [a, Some(c)],
            [Some(_), Some(_)] => panic!("a setting holds at most two conditions"),
        };
        Self { conditions, ..self }
    }
}

/// One row of a style's section in the F4 menu, in the order it is drawn:
/// options first (they change what the sliders mean), then parameters,
/// with each `<base>_r/_g/_b` triple folded into a [`Color`](Self::Color)
/// where its red channel stood.
#[derive(Clone, Copy, Debug)]
pub enum StyleControl<'a> {
    Option(&'a StyleOption),
    Param(&'a StyleParam),
    Color {
        /// The name before `_r` (`tint`), the key a scene writes.
        base: &'static str,
        channels: [&'a StyleParam; 3],
    },
}

impl StyleControl<'_> {
    /// The parameter, option or colour base this row edits.
    pub fn name(&self) -> &'static str {
        match self {
            StyleControl::Option(o) => o.name,
            StyleControl::Param(p) => p.name,
            StyleControl::Color { base, .. } => base,
        }
    }
}

/// One colour setting of a style (#315): the parameters `<base>_r`,
/// `<base>_g` and `<base>_b`, each 0..1 in display values, that a scene
/// writes as one key, `tint = [1.0, 0.7, 0.25]` (or `tint = "#ffb340"`),
/// a style reads with [`StyleFrame::color`], F4 shows as one picker and
/// Copy TOML writes back as one array. The channels stay parameters
/// underneath, so a per-entity variant row, a uniform and `--style
/// ascii:tint_g=0.5` see three numbers as they always did.
#[derive(Clone, Copy, Debug)]
pub struct StyleColor<'a> {
    /// The key a scene writes (`tint`).
    pub base: &'static str,
    pub channels: [&'a StyleParam; 3],
}

impl StyleColor<'_> {
    /// The colour the channels' defaults make.
    pub fn default_rgb(&self) -> [f32; 3] {
        self.channels.map(|c| c.default)
    }
}

/// A colour from a TOML value: an array of three numbers (`[1.0, 0.7,
/// 0.25]`, integers allowed) or a `#rrggbb` string (`"#ffb340"`), both in
/// display values, 0..1 per channel.
pub fn color_from_toml(value: &toml::Value) -> Option<[f32; 3]> {
    if let Some(text) = value.as_str() {
        return color_from_hex(text);
    }
    let number = |v: &toml::Value| {
        flint_core::toml_util::toml_f64(v)
            .filter(|n| n.is_finite())
            .map(|n| n as f32)
    };
    match value.as_array()?.as_slice() {
        [r, g, b] => Some([number(r)?, number(g)?, number(b)?]),
        _ => None,
    }
}

/// `#rrggbb` (or `rrggbb`) as 0..1 channels.
pub fn color_from_hex(text: &str) -> Option<[f32; 3]> {
    let hex = text.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?].map(|c| c as f32 / 255.0))
}

/// A colour as command-line text (#315): `#rrggbb`, or `r/g/b` with each
/// channel a number (`0.3/1/0.4`). `--style` keys are split on commas, so
/// a colour there can't be `r,g,b`.
pub fn color_from_text(text: &str) -> Option<[f32; 3]> {
    let text = text.trim();
    if text.starts_with('#') {
        return color_from_hex(text);
    }
    let parts: Vec<f32> = text
        .split('/')
        .map(|p| p.trim().parse::<f32>().ok().filter(|n| n.is_finite()))
        .collect::<Option<_>>()?;
    <[f32; 3]>::try_from(parts).ok()
}

/// What the framework needs to build a style.
#[derive(Clone, Copy)]
pub struct StyleDevice<'a> {
    pub device: &'a wgpu::Device,
    /// The style being built: lets one `create` serve many descriptors
    /// (user-authored styles, #274, all share [`custom`]'s).
    pub descriptor: &'a StyleDescriptor,
    /// The same style as the registry holds it, with what was loaded for a
    /// user style.
    pub style: &'a Style,
    /// The format the style's output is drawn in: `Rgba16Float` for an
    /// [`Hdr`](StyleStage::Hdr) style, the surface's for a
    /// [`Display`](StyleStage::Display) one.
    pub format: wgpu::TextureFormat,
}

/// An input the framework computes for the styles that ask for it
/// ([`StyleDescriptor::needs`]), once a frame however many do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StyleNeed {
    /// The shared geometry edges ([`edges`]): silhouette and crease
    /// strength and the direction across them, from the depth alone, read
    /// as [`StyleFrame::edges`].
    Edges,
    /// The shared edges' depth steps alone (#743): `r` and the direction,
    /// with `g` (the creases) left 0 on a frame where no style that is
    /// on asks for [`Edges`](StyleNeed::Edges). Contour's, whose lines
    /// are silhouettes: the pass then needs a 3x3 patch, not 5x5 and
    /// normals.
    EdgeSteps,
}

/// Whether a style reads the entity style mask (#263), and when (#350).
#[derive(Clone, Copy)]
pub enum MaskUse {
    /// Never: the mask is drawn for it only if it is per-entity and an
    /// entity names it.
    Never,
    /// Whenever the style is on.
    Always,
    /// While the style is on and this says so for the frame's config and
    /// ids: ASCII's outlines read it only with `edges > 0` and something
    /// styled to outline.
    When(fn(&PostProcessConfig, &StyleIds) -> bool),
}

impl MaskUse {
    /// Whether the style may read the mask at all (`--list-styles`).
    pub fn ever(self) -> bool {
        !matches!(self, MaskUse::Never)
    }

    /// Whether it reads the mask under `config` with these ids.
    pub fn now(self, config: &PostProcessConfig, ids: &StyleIds) -> bool {
        match self {
            MaskUse::Never => false,
            MaskUse::Always => true,
            MaskUse::When(f) => f(config, ids),
        }
    }
}

impl std::fmt::Debug for MaskUse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            MaskUse::Never => "Never",
            MaskUse::Always => "Always",
            MaskUse::When(_) => "When(..)",
        })
    }
}

/// A registered style: what it is called, where it runs and how to make it.
/// Built with [`StyleDescriptor::new`] and its setters, never as a literal
/// (#366); `non_exhaustive` holds other crates to that.
#[non_exhaustive]
#[derive(Clone)]
pub struct StyleDescriptor {
    /// The name in `[post_process.styles.<name>]`; lowercase, no spaces.
    pub name: &'static str,
    /// What the F4 menu and `--gpu-time` call it.
    pub label: &'static str,
    pub stage: StyleStage,
    /// Parameters and their defaults, read with [`StyleFrame::param`].
    pub params: List<StyleParam>,
    /// Named choices and their defaults, read with [`StyleFrame::option`].
    pub options: List<StyleOption>,
    /// Which install of the style this is (#742): 0 for a built-in or a
    /// game's style, a new number each time a user style's `.shader.toml`
    /// is loaded or reloaded. A style is its name and generation: the built
    /// passes, kept bind groups and pipelines of an older install are never
    /// taken for this one's.
    pub generation: u64,
    /// Whether the style reads the entity style mask (#263). While any
    /// such style is on and [`MaskUse::now`] holds, the renderer draws the
    /// mask each frame and hands it over as [`StyleFrame::mask`]; while
    /// none is, the mask isn't made.
    pub uses_mask: MaskUse,
    /// Whether the style reads the style albedo (#869): the opaque
    /// surfaces' own colour before light, drawn after the scene pass while
    /// any such style is on and [`MaskUse::now`] holds (the same `Never` /
    /// `Always` / `When` as the mask), handed over as
    /// [`StyleFrame::albedo`]. The picture divided by it is the light.
    pub uses_albedo: MaskUse,
    /// Framework-made inputs the style reads (#301). While any style that
    /// asks for one is on, the framework makes it once a frame for all of
    /// them: [`StyleNeed::Edges`] is [`StyleFrame::edges`].
    pub needs: &'static [StyleNeed],
    /// When the style reads the shared edges, if not whenever it is on
    /// (#743, [`StyleDescriptor::needs_edges_when`]): ASCII only while its
    /// `edges` are on or the edge view draws them.
    pub edges_when: Option<fn(&StyleFrame<'_>) -> bool>,
    /// The component an entity takes this style with, its fields being the
    /// style's parameters and options (`ascii_style`, #266). A style with
    /// one is per-entity: an entity carrying it (or a `style_mask` naming
    /// the style) switches the style on for that entity's pixels alone,
    /// with no `[post_process.styles]` table, and the style reads each
    /// pixel's settings from [`StyleFrame::variants`] by its mask id.
    pub component: Option<&'static str>,
    /// Whether bloom reads this style's result by default (#300). Off, the
    /// bloom threshold reads the unstylized scene, so a halo sits where
    /// the original bright pixels were (Kuwahara's look: light from the
    /// scene, not the brush). On, bloom reads the HDR stage's result, so a
    /// style's own bright shapes glow. Only an [`Hdr`](StyleStage::Hdr)
    /// style can feed bloom, which runs before the display stage. A scene
    /// overrides it with `feeds_bloom` in the style's table
    /// ([`StyleDescriptor::feeds_bloom_in`]).
    pub feeds_bloom: bool,
    /// Whether the style leaves the pixels another style owns (#384): an
    /// entity that asks for another running style (`style_mask { style =
    /// "pencil" }` in an ASCII frame) keeps the input there, put back by
    /// the framework after the style draws ([`ownership`]). True for every
    /// style but those that look at the whole picture on purpose: the
    /// debug views and a glow over the finished frame
    /// ([`draws_over_entities`](Self::draws_over_entities)).
    pub yields: bool,
    /// Whether the style draws its own entities' `blend` (#396). Off (the
    /// default), the framework cross-fades: after the style's pass, every
    /// pixel of an entity drawn in this style at a blend below 1 (the
    /// mask's `.g`, #278) is mixed back toward the style's input by that
    /// much, so a mask-reading style fades with no blend code of its own.
    /// A style that does something better than a cross-fade (ASCII's
    /// `transition = "dissolve"`) sets it with
    /// [`blends_itself`](StyleDescriptor::blends_itself) and reads `.g`
    /// itself.
    pub blends_itself: bool,
    /// Whether the style draws a blend itself for some settings (#879):
    /// asked of the whole frame (`None`: the table's `blend`, which the
    /// stage would otherwise draw as a mix, [`blend`]) and of each entity
    /// variant (`Some`: the mask's `.g`, which the framework's cross-fade
    /// would otherwise mix). Pencil's `transition = "draw"` answers yes, and
    /// draws the blend as a front lines are drawn up to. `None` (the
    /// default) leaves every blend to the framework.
    pub draws_blend:
        Option<fn(&PostProcessConfig, Option<&crate::style_mask::StyleVariant>) -> bool>,
    /// Where it stands in its stage's chain when a scene says nothing
    /// (#746): `None` is its registration index ([`chain_order`]). Lets a
    /// style registered late run early without moving any other's index.
    pub default_order: Option<f32>,
    /// Build the style's pipelines. May panic on a broken driver; the
    /// framework catches it.
    pub create: fn(&StyleDevice) -> Box<dyn StylePass>,
}

/// Building a descriptor (#366): `StyleDescriptor::new` with what every
/// style has, then a setter for each thing it differs in, all `const` so
/// the result is a `static`:
///
/// ```ignore
/// pub(super) static STYLE: StyleDescriptor =
///     StyleDescriptor::new("invert", "Invert", StyleStage::Hdr, create)
///         .params(&[StyleParam::new("amount", 1.0, 0.0..=1.0)]);
/// ```
///
/// A field added to the descriptor gets its default in `new`, so no style
/// module has to change; build descriptors only this way.
impl StyleDescriptor {
    /// A style with no parameters, options, mask or needs, not
    /// per-entity, not feeding bloom.
    pub const fn new(
        name: &'static str,
        label: &'static str,
        stage: StyleStage,
        create: fn(&StyleDevice) -> Box<dyn StylePass>,
    ) -> Self {
        Self {
            name,
            label,
            stage,
            params: List::EMPTY,
            options: List::EMPTY,
            generation: 0,
            uses_mask: MaskUse::Never,
            uses_albedo: MaskUse::Never,
            needs: &[],
            edges_when: None,
            component: None,
            feeds_bloom: false,
            yields: true,
            blends_itself: false,
            draws_blend: None,
            default_order: None,
            create,
        }
    }

    /// Its parameters ([`StyleParam::new`]).
    pub const fn params(mut self, params: &'static [StyleParam]) -> Self {
        // A `const fn` can't drop the list it replaces; the one `new` made
        // is static, so forgetting it frees nothing
        std::mem::forget(std::mem::replace(&mut self.params, List::Static(params)));
        self
    }

    /// Its named choices ([`StyleOption::new`]).
    pub const fn options(mut self, options: &'static [StyleOption]) -> Self {
        std::mem::forget(std::mem::replace(&mut self.options, List::Static(options)));
        self
    }

    /// Whether and when it reads the style mask.
    pub const fn uses_mask(mut self, uses_mask: MaskUse) -> Self {
        self.uses_mask = uses_mask;
        self
    }

    /// Whether and when it reads the style albedo (#869).
    pub const fn uses_albedo(mut self, uses_albedo: MaskUse) -> Self {
        self.uses_albedo = uses_albedo;
        self
    }

    /// The framework-made inputs it reads.
    pub const fn needs(mut self, needs: &'static [StyleNeed]) -> Self {
        self.needs = needs;
        self
    }

    /// Reads the shared edges ([`StyleNeed::Edges`]) on the frames `when`
    /// says (#743), so a frame where it doesn't skips the edge pass. The
    /// pass is built while the style is on either way.
    pub const fn needs_edges_when(mut self, when: fn(&StyleFrame<'_>) -> bool) -> Self {
        self.needs = &[StyleNeed::Edges];
        self.edges_when = Some(when);
        self
    }

    /// Per-entity, through `component` (`ascii_style`).
    pub const fn component(mut self, component: &'static str) -> Self {
        self.component = Some(component);
        self
    }

    /// Whether bloom reads its result by default.
    pub const fn feeds_bloom(mut self, feeds_bloom: bool) -> Self {
        self.feeds_bloom = feeds_bloom;
        self
    }

    /// Draws over every pixel, other styles' entities included
    /// ([`yields`](Self::yields) off): a debug view, or a finishing glow.
    pub const fn draws_over_entities(mut self) -> Self {
        self.yields = false;
        self
    }

    /// It honours its entities' `blend` itself, so the framework's
    /// cross-fade leaves it alone (#396).
    pub const fn blends_itself(mut self) -> Self {
        self.blends_itself = true;
        self
    }

    /// It draws the blends `when` says itself (#879), the whole frame's or
    /// an entity variant's ([`draws_blend`](Self::draws_blend)); the stage's
    /// mix and the framework's cross-fade leave those alone.
    pub const fn draws_blend(
        mut self,
        when: fn(&PostProcessConfig, Option<&crate::style_mask::StyleVariant>) -> bool,
    ) -> Self {
        self.draws_blend = Some(when);
        self
    }

    /// Its place in its stage's chain when a scene gives none
    /// ([`default_order`](Self::default_order)), in the units a table's
    /// `order` uses: registration indices.
    pub const fn default_order(mut self, order: f32) -> Self {
        self.default_order = Some(order);
        self
    }
}

impl StyleDescriptor {
    /// Whether `config` switches this style on (post-processing as a whole
    /// aside).
    pub fn is_enabled(&self, config: &PostProcessConfig) -> bool {
        config.styles.is_enabled(self.name)
    }

    /// Whether this style runs this frame: `config` draws it over the
    /// frame ([`Self::covers_frame`]), or it is per-entity and an entity
    /// asks for it (#266) at a blend above 0 (#400). Never while `config`
    /// has it off everywhere (#376, `--style <name>:off`).
    pub fn is_on(&self, config: &PostProcessConfig, ids: &StyleIds) -> bool {
        if config.styles.is_off_everywhere(self.name) {
            return false;
        }
        self.covers_frame(config) || (self.component.is_some() && ids.requests(self.name))
    }

    /// Whether `config` draws this style over the whole frame: its table
    /// is on and isn't a per-entity style's `scope = "entities"` preset,
    /// which draws only where its entities ask (#400).
    pub fn covers_frame(&self, config: &PostProcessConfig) -> bool {
        self.is_enabled(config) && self.option(config, "scope") != "entities"
    }

    /// Whether this style reads the style mask this frame: its `uses_mask`
    /// says so for this config, or it is per-entity and an entity names
    /// it, opt-outs included.
    pub fn reads_mask(&self, config: &PostProcessConfig, ids: &StyleIds) -> bool {
        self.is_on(config, ids)
            && (self.uses_mask.now(config, ids)
                || (self.component.is_some() && ids.mentions(self.name)))
    }

    /// Whether the framework cross-fades this style's entities by their
    /// blend this frame (#396): it reads the mask and doesn't blend
    /// itself.
    pub fn framework_fades(&self, config: &PostProcessConfig, ids: &StyleIds) -> bool {
        !self.blends_itself && self.reads_mask(config, ids)
    }

    /// Whether the style draws this blend itself (#879): the whole frame's
    /// (`variant` `None`) or an entity variant's. Then the stage draws the
    /// table's blend as 1 and the cross-fade passes over the variant's
    /// pixels.
    pub fn draws_own_blend(
        &self,
        config: &PostProcessConfig,
        variant: Option<&crate::style_mask::StyleVariant>,
    ) -> bool {
        self.draws_blend.is_some_and(|when| when(config, variant))
    }

    /// Whether the style reads the shared edges ([`StyleNeed::Edges`]).
    pub fn needs_edges(&self) -> bool {
        self.needs.contains(&StyleNeed::Edges) || self.needs.contains(&StyleNeed::EdgeSteps)
    }

    /// Whether it reads the shared edges' creases, not only their depth
    /// steps ([`StyleNeed::EdgeSteps`]).
    pub fn needs_creases(&self) -> bool {
        self.needs.contains(&StyleNeed::Edges)
    }

    /// Whether it reads them on this frame: it needs them, and its
    /// [`edges_when`](Self::edges_when), if it has one, says so.
    pub fn wants_edges(&self, frame: &StyleFrame<'_>) -> bool {
        self.needs_edges() && self.edges_when.is_none_or(|when| when(frame))
    }

    /// Whether bloom reads this style's result under `config` (#300): the
    /// scene's `feeds_bloom`, or the descriptor's default. Always false
    /// for a display style.
    pub fn feeds_bloom_in(&self, config: &PostProcessConfig) -> bool {
        self.stage == StyleStage::Hdr
            && config
                .styles
                .feeds_bloom(self.name)
                .unwrap_or(self.feeds_bloom)
    }

    /// Parameter `name`'s value in `config`, or its default.
    pub fn param(&self, config: &PostProcessConfig, name: &str) -> f32 {
        let default = self
            .params
            .iter()
            .find(|p| p.name == name)
            .map_or(0.0, |p| p.default);
        config.styles.param(self.name, name).unwrap_or(default)
    }

    /// Option `name`'s value in `config`, or its default.
    pub fn option<'c>(&self, config: &'c PostProcessConfig, name: &str) -> &'c str {
        let default = self
            .options
            .iter()
            .find(|o| o.name == name)
            .map_or("", |o| o.default);
        config.styles.option(self.name, name).unwrap_or(default)
    }

    /// How setting `name` is presented: an option's or a parameter's own
    /// [`StyleUi`], and a colour base's (`tint`) its red channel's. A
    /// colour's channels have none of their own (`tint_r` reads as
    /// "Tint r"), and a name that is no setting has none.
    pub fn ui(&self, name: &str) -> Option<&StyleUi> {
        let options: &[StyleOption] = &self.options;
        let params: &[StyleParam] = &self.params;
        if let Some(o) = options.iter().find(|o| o.name == name) {
            return Some(&o.ui);
        }
        if let Some(p) = params.iter().find(|p| p.name == name) {
            return self.color_of(name).is_none().then_some(&p.ui);
        }
        self.color_setting(name).map(|c| &c.channels[0].ui)
    }

    /// Why setting `name` does nothing under `config`, or `None` while it
    /// is read (#365): its [`when`](StyleParam::when) names an option whose
    /// current value isn't one of those that read it. The line is what the
    /// F4 menu shows on the greyed-out row.
    pub fn unused_reason(&self, config: &PostProcessConfig, name: &str) -> Option<String> {
        let ui = self.ui(name)?;
        ui.conditions.iter().flatten().find_map(|c| {
            let current = self.option(config, c.option);
            if c.holds(current) {
                return None;
            }
            let label = self.label_for(c.option);
            let shown = if current.is_empty() {
                "(default)"
            } else {
                current
            };
            Some(if c.unless {
                format!("Unused while {label} is {shown}")
            } else {
                format!(
                    "Unused while {label} is {shown}: only {} reads it",
                    c.values.join(" or ")
                )
            })
        })
    }

    /// What the F4 menu calls setting `name`: its label, or the name made
    /// readable (`tint_r` -> `Tint r`).
    pub fn label_for(&self, name: &str) -> String {
        match self.ui(name) {
            Some(ui) if !ui.label.is_empty() => ui.label.to_string(),
            _ => humanize(name),
        }
    }

    /// Setting `name`'s hover line; empty for none.
    pub fn help_for(&self, name: &str) -> &'static str {
        self.ui(name).map_or("", |ui| ui.help)
    }

    /// Whether setting `name` takes whole numbers only.
    pub fn is_integer(&self, name: &str) -> bool {
        self.ui(name).is_some_and(|ui| ui.integer)
    }

    /// The colour whose key is `base` (`tint`), if the style has its three
    /// channels.
    pub fn color_setting(&self, base: &str) -> Option<StyleColor<'_>> {
        let params: &[StyleParam] = &self.params;
        let find = |c: &str| {
            let name = format!("{base}_{c}");
            params.iter().find(|p| p.name == name)
        };
        let (r, g, b) = (find("r")?, find("g")?, find("b")?);
        // The red channel's name, less `_r`, is the 'static base
        let base = &r.name[..r.name.len() - 2];
        Some(StyleColor {
            base,
            channels: [r, g, b],
        })
    }

    /// The style's colours (#315), in the order their red channels stand.
    pub fn colors(&self) -> Vec<StyleColor<'_>> {
        self.params
            .iter()
            .filter_map(|p| p.name.strip_suffix("_r"))
            .filter_map(|base| self.color_setting(base))
            .collect()
    }

    /// The colour a parameter is a channel of, if it is one.
    pub fn color_of(&self, param: &str) -> Option<StyleColor<'_>> {
        ["_r", "_g", "_b"]
            .iter()
            .find_map(|c| param.strip_suffix(c))
            .and_then(|base| self.color_setting(base))
    }

    /// Colour `base` in `config`, each channel as set or its default. A
    /// name that is no colour of this style reads black.
    pub fn color(&self, config: &PostProcessConfig, base: &str) -> [f32; 3] {
        match self.color_setting(base) {
            Some(c) => c.channels.map(|p| self.param(config, p.name)),
            None => [0.0; 3],
        }
    }

    /// The keys a scene's table for this style takes besides `enabled` and
    /// `order`: options, then parameters with each colour's channels folded
    /// into its one key (`tint`), for error messages.
    pub fn setting_keys(&self) -> Vec<&'static str> {
        let mut keys: Vec<&'static str> = self.options.iter().map(|o| o.name).collect();
        for p in &self.params {
            match self.color_of(p.name) {
                Some(c) if !keys.contains(&c.base) => keys.push(c.base),
                Some(_) => {}
                None => keys.push(p.name),
            }
        }
        keys
    }

    /// Every key a scene's table for this style can hold: the
    /// framework's ([`RESERVED_KEYS`]: `enabled`, `order`, `feeds_bloom`,
    /// `blend` and the reveal's), the options, each parameter (colour
    /// channels one by one) and each colour's own key. A commit to the
    /// scene (#364) owns these and leaves any other key alone, so a reveal
    /// switched off in F4 leaves the file with it (#875).
    pub fn table_keys(&self) -> Vec<&'static str> {
        let mut keys = RESERVED_KEYS.to_vec();
        keys.extend(self.options.iter().map(|o| o.name));
        keys.extend(self.params.iter().map(|p| p.name));
        keys.extend(self.colors().iter().map(|c| c.base));
        keys
    }

    /// The rows of this style's F4 section, in drawing order.
    pub fn controls(&self) -> Vec<StyleControl<'_>> {
        let mut rows: Vec<StyleControl> = self.options.iter().map(StyleControl::Option).collect();
        for p in &self.params {
            match self.color_of(p.name) {
                Some(c) if std::ptr::eq(c.channels[0], p) => rows.push(StyleControl::Color {
                    base: c.base,
                    channels: c.channels,
                }),
                Some(_) => {}
                None => rows.push(StyleControl::Param(p)),
            }
        }
        rows
    }
}

/// `tint_r` -> `Tint r`: a setting's name as a label.
pub(super) fn humanize(name: &str) -> String {
    let spaced = name.replace('_', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Style `descriptor`'s settings in `config` as the scene TOML that would
/// set them (#268), for the F4 menu's Copy TOML: the table header, and
/// only the values that differ from the defaults. A style that is off says
/// so, since a table on its own switches its style on.
pub fn style_toml(descriptor: &StyleDescriptor, config: &PostProcessConfig) -> String {
    let mut out = format!("[post_process.styles.{}]\n", descriptor.name);
    if !descriptor.is_enabled(config) {
        out.push_str("enabled = false\n");
    }
    if let Some(order) = config.styles.order(descriptor.name) {
        if order.fract() == 0.0 {
            out.push_str(&format!("order = {}\n", order as i64));
        } else {
            out.push_str(&format!("order = {}\n", toml_float(order)));
        }
    }
    let feeds = descriptor.feeds_bloom_in(config);
    if descriptor.stage == StyleStage::Hdr && feeds != descriptor.feeds_bloom {
        out.push_str(&format!("feeds_bloom = {feeds}\n"));
    }
    let blend = config.styles.blend(descriptor.name);
    if blend < 1.0 {
        out.push_str(&format!("blend = {}\n", toml_float(blend)));
    }
    if config.styles.follows_tear(descriptor.name) {
        out.push_str("blend_source = \"tear\"\n");
    }
    if let Some(setting) = config.styles.get(descriptor.name) {
        out.push_str(&setting.reveal.toml_lines());
    }
    for o in &descriptor.options {
        let value = descriptor.option(config, o.name);
        if value != o.default {
            out.push_str(&format!("{} = {:?}\n", o.name, value));
        }
    }
    for p in &descriptor.params {
        // A colour is one array key, where its red channel stands (#315)
        if let Some(c) = descriptor.color_of(p.name) {
            let value = descriptor.color(config, c.base);
            if std::ptr::eq(c.channels[0], p) && value != c.default_rgb() {
                let [r, g, b] = value.map(toml_float);
                out.push_str(&format!("{} = [{r}, {g}, {b}]\n", c.base));
            }
            continue;
        }
        let value = descriptor.param(config, p.name);
        if value != p.default {
            let integer = descriptor.is_integer(p.name) && value.fract() == 0.0;
            if integer {
                out.push_str(&format!("{} = {}\n", p.name, value as i64));
            } else {
                out.push_str(&format!("{} = {}\n", p.name, toml_float(value)));
            }
        }
    }
    out
}

/// An `f32` as a TOML float: the shortest spelling that reads back the
/// same, with a `.0` so it stays a float.
pub(super) fn toml_float(value: f32) -> String {
    let text = format!("{value}");
    if text.contains(['.', 'e', 'E']) || !value.is_finite() {
        text
    } else {
        format!("{text}.0")
    }
}
