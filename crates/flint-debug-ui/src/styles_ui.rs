//! The style editor (#268): one collapsible section for every registered
//! style whose settings live in `[post_process.styles.<name>]`, drawn from
//! its descriptor alone, so a new style (watercolor, pencil) shows up here
//! with no UI code of its own.
//!
//! A section has the style's toggle, a row per setting (a combo or text
//! field for an option, a slider for a parameter, one colour picker for an
//! `_r/_g/_b` triple; labels, hover text and whole-number steps come from
//! the descriptor's hints), and four buttons: **Defaults** clears the
//! style's values, **Authored** puts back what the scene says, **Copy
//! TOML** puts the `[post_process.styles.<name>]` block that reproduces
//! what is on screen on the clipboard, so a look tuned by eye goes back
//! into the scene in one paste, and **Commit to scene** (#364) writes that
//! block into the scene file itself, key by key
//! ([`flint_scene::SceneDocument::set_style_table`]), while the host names
//! a file to write.
//!
//! While the host gives it the scene's authored settings, a dot marks each
//! value that differs from them (hover for the scene's value), and the
//! header says "edited".
//!
//! Both the F4 Rendering & Effects panel and the F3 Styles panel draw this
//! editor; it edits a [`PostProcessConfig`]'s `styles` and nothing else.

use flint_render::stylize::{
    self, ChoiceScope, StyleControl, StyleDescriptor, StyleOption, StyleSettings, StyleStage,
};
use flint_render::PostProcessConfig;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The "Feeds bloom" checkbox of an HDR style (#300): whether bloom
/// reads the style's result or the unstylized scene. Returns whether it
/// changed. Draws nothing for a display style, which runs after bloom.
pub fn feeds_bloom_checkbox(
    ui: &mut egui::Ui,
    style: &'static StyleDescriptor,
    pp: &mut PostProcessConfig,
) -> bool {
    if style.stage != StyleStage::Hdr {
        return false;
    }
    let mut feeds = style.feeds_bloom_in(pp);
    let changed = ui
        .checkbox(&mut feeds, "Feeds bloom")
        .on_hover_text(
            "On: bloom glows from this style's output, so its own bright shapes halo. \
             Off: bloom reads the scene under the style, so halos sit where the original \
             bright pixels were. Scene key: feeds_bloom",
        )
        .changed();
    if changed {
        // Back at the default, say nothing: the scene's TOML stays clean
        let choice = (feeds != style.feeds_bloom).then_some(feeds);
        pp.styles.set_feeds_bloom(style.name, choice);
    }
    changed
}

/// What one frame of the editor changed.
#[derive(Debug, Clone, Copy, Default)]
pub struct StylesEdit {
    /// A style setting changed: upload the config.
    pub changed: bool,
    /// A style was switched on: the host builds its pipelines
    /// (`ensure_style_resources`).
    pub needs_resources: bool,
    /// This style was just written to the scene file (#364): what the
    /// config says of it is now what the scene says, so the host makes its
    /// authored settings match (`StyleSettings::restore` from the config).
    pub committed: Option<&'static str>,
}

impl StylesEdit {
    fn merge(&mut self, other: StylesEdit) {
        self.changed |= other.changed;
        self.needs_resources |= other.needs_resources;
        self.committed = other.committed.or(self.committed);
    }
}

/// The sliders' width in a style's grid, in points.
const SLIDER_WIDTH: f32 = 130.0;

/// How long the "copied" note stays after Copy TOML, in seconds.
const COPIED_NOTE_SECS: f64 = 1.5;

/// The editor's own state across frames: the text being typed into
/// free-text options, and the last Copy TOML.
#[derive(Default)]
pub struct StylesEditor {
    /// What is being typed in each free-text option (style, option). A
    /// text field over a fresh copy of the config's value every frame
    /// would lose each keystroke; the buffer keeps it until Enter or focus
    /// loss commits it.
    text: HashMap<(&'static str, &'static str), String>,
    copied_at: Option<(&'static str, f64)>,
    /// The last block Copy TOML put on the clipboard.
    pub last_copied: Option<String>,
    /// How many values the last frame marked as differing from the scene.
    pub marked: usize,
    /// The scene's folder, where options find their run-time choices (its
    /// palettes and glyph sets, #322); set by the host on scene load.
    scene_dir: Option<PathBuf>,
    /// Each option's run-time choices, by (style, option) and the other
    /// options' values they were found under, so a combo lists the folder
    /// once rather than every frame. Dropped when the scene changes.
    choices: HashMap<(&'static str, &'static str), (String, Vec<String>)>,
    /// The scene file Commit to scene writes (#364); `None` (a scene from
    /// a bundle, or a host with no file) hides the button.
    scene_file: Option<PathBuf>,
    /// Why Commit to scene is greyed out right now, when it is (the
    /// viewer's unsaved entity edits, which the reload would drop).
    commit_blocked: Option<String>,
    /// The last commit's note beside the buttons: the style, when, and its
    /// error if it failed.
    commit_note: Option<(&'static str, f64, Option<String>)>,
}

impl StylesEditor {
    /// The scene's folder, where options look for its palettes and glyph
    /// sets (#322); `None` offers the built-ins only. Calling it again (a
    /// scene load or reload) finds new files.
    pub fn set_scene_dir(&mut self, scene_dir: Option<PathBuf>) {
        self.scene_dir = scene_dir;
        self.choices.clear();
    }

    /// The scene file Commit to scene writes into (#364). `None` hides the
    /// button: a scene played from a bundle has no file to write.
    pub fn set_scene_file(&mut self, scene_file: Option<PathBuf>) {
        self.scene_file = scene_file;
    }

    /// The scene file Commit to scene writes, when there is one.
    pub fn scene_file(&self) -> Option<&Path> {
        self.scene_file.as_deref()
    }

    /// Grey out Commit to scene, saying why on hover; `None` allows it.
    pub fn set_commit_blocked(&mut self, reason: Option<String>) {
        self.commit_blocked = reason;
    }

    /// Write style `style`'s settings in `pp` into the scene file and make
    /// `pp` say exactly what the file now says of it (the values at their
    /// defaults dropped), so it and the authored settings compare equal.
    pub fn commit(
        &self,
        style: &'static StyleDescriptor,
        pp: &mut PostProcessConfig,
    ) -> Result<(), String> {
        let path = self
            .scene_file
            .as_deref()
            .ok_or_else(|| "No scene file to write".to_string())?;
        let committed = commit_style(path, style, pp)?;
        pp.styles.restore(style.name, &committed);
        Ok(())
    }

    /// The scene folder the editor looks in.
    pub fn scene_dir(&self) -> Option<&Path> {
        self.scene_dir.as_deref()
    }

    /// The values the combo offers for `o`: its fixed choices, or what its
    /// `discover` finds beside the scene, cached until another option of
    /// the style (the font, for ramps) or the scene changes.
    pub fn choices(
        &mut self,
        style: &'static StyleDescriptor,
        o: &StyleOption,
        pp: &PostProcessConfig,
    ) -> Vec<String> {
        if o.discover.is_none() {
            return o.choices.iter().map(|c| c.to_string()).collect();
        }
        let others: String = style
            .options
            .iter()
            .filter(|other| other.name != o.name)
            .map(|other| format!("{}={};", other.name, style.option(pp, other.name)))
            .collect();
        if let Some((key, found)) = self.choices.get(&(style.name, o.name)) {
            if *key == others {
                return found.clone();
            }
        }
        let option = |name: &str| style.option(pp, name).to_string();
        let scope = ChoiceScope {
            scene_dir: self.scene_dir.as_deref(),
            option: &option,
        };
        let found = o.choices_in(&scope);
        self.choices
            .insert((style.name, o.name), (others, found.clone()));
        found
    }

    /// Draw every table style's section, editing `pp.styles`. `authored`
    /// is the scene's own `[post_process.styles]`, when the host knows it.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        pp: &mut PostProcessConfig,
        authored: Option<&StyleSettings>,
    ) -> StylesEdit {
        let mut edit = StylesEdit::default();
        self.marked = 0;
        edit.changed |= chain_ui(ui, pp);
        let mut any = false;
        for style in stylize::registry().iter().copied() {
            any = true;
            edit.merge(self.section(ui, style, pp, authored));
        }
        if !any {
            ui.weak("No styles registered");
        }
        if !pp.enabled && stylize::registry().iter().any(|s| s.is_enabled(pp)) {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "Post-processing is off: styles don't draw",
            );
        }
        edit
    }

    fn section(
        &mut self,
        ui: &mut egui::Ui,
        style: &'static StyleDescriptor,
        pp: &mut PostProcessConfig,
        authored: Option<&StyleSettings>,
    ) -> StylesEdit {
        let mut edit = StylesEdit::default();
        let mut on = pp.styles.is_enabled(style.name);
        let id = ui.make_persistent_id(("style section", style.name));
        let state =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, on);
        state
            .show_header(ui, |ui| {
                if ui.checkbox(&mut on, style.label).changed() {
                    pp.styles.set_enabled(style.name, on);
                    edit.changed = true;
                    edit.needs_resources |= on;
                }
                ui.weak(match style.stage {
                    StyleStage::Hdr => "hdr",
                    StyleStage::Display => "display",
                })
                .on_hover_text(match style.stage {
                    StyleStage::Hdr => "Runs on the scene's HDR, before fog, bloom and tonemapping",
                    StyleStage::Display => {
                        "Runs on the finished picture at output resolution, before FXAA"
                    }
                });
                // A user-authored style (ADR 0112, #275): where it lives, and
                // whether it compiles
                if let Some(source) = stylize::custom::source(style.name) {
                    ui.weak("user").on_hover_text(format!(
                        "{}\n{}\nSaving either file reloads it",
                        source.toml_path.display(),
                        source.wgsl_path.display()
                    ));
                    if let Some((_, error)) = stylize::custom::errors()
                        .into_iter()
                        .find(|(name, _)| name == style.name)
                    {
                        ui.colored_label(ui.visuals().error_fg_color, "error")
                            .on_hover_text(error);
                    }
                }
                if let Some(authored) = authored {
                    if !pp.styles.same_style(style.name, authored) {
                        ui.colored_label(ui.visuals().warn_fg_color, "edited")
                            .on_hover_text(format!(
                                "Differs from the scene's [post_process.styles.{}]",
                                style.name
                            ));
                    }
                }
            })
            .body(|ui| {
                ui.add_enabled_ui(on, |ui| {
                    ui.spacing_mut().slider_width = SLIDER_WIDTH;
                    egui::Grid::new(("style grid", style.name))
                        .num_columns(4)
                        .spacing([8.0, 4.0])
                        .show(ui, |ui| {
                            for control in style.controls() {
                                edit.changed |= self.row(ui, style, control, pp, authored);
                                ui.end_row();
                            }
                        });
                    edit.changed |= feeds_bloom_checkbox(ui, style, pp);
                });
                ui.horizontal(|ui| {
                    if ui
                        .small_button("Defaults")
                        .on_hover_text("Every value back to the style's default")
                        .clicked()
                    {
                        pp.styles.reset(style.name);
                        edit.changed = true;
                    }
                    if let Some(authored) = authored {
                        let same = pp.styles.same_style(style.name, authored);
                        if ui
                            .add_enabled(!same, egui::Button::new("Authored").small())
                            .on_hover_text("Back to what the scene says, on/off included")
                            .clicked()
                        {
                            let was_on = pp.styles.is_enabled(style.name);
                            pp.styles.restore(style.name, authored);
                            edit.changed = true;
                            edit.needs_resources |= !was_on && pp.styles.is_enabled(style.name);
                        }
                    }
                    if ui
                        .small_button("Copy TOML")
                        .on_hover_text(
                            "Copy the [post_process.styles] block that reproduces this look \
                             (only the values that differ from the defaults)",
                        )
                        .clicked()
                    {
                        let text = stylize::style_toml(style, pp);
                        ui.ctx().copy_text(text.clone());
                        self.last_copied = Some(text);
                        self.copied_at = Some((style.name, ui.input(|i| i.time)));
                    }
                    if let Some((name, at)) = self.copied_at {
                        let now = ui.input(|i| i.time);
                        if name == style.name && now - at < COPIED_NOTE_SECS {
                            ui.weak("copied");
                            ui.ctx().request_repaint();
                        }
                    }
                    edit.merge(self.commit_button(ui, style, pp));
                });
            });
        edit
    }

    /// The Commit to scene button and its note, while there is a scene
    /// file to write.
    fn commit_button(
        &mut self,
        ui: &mut egui::Ui,
        style: &'static StyleDescriptor,
        pp: &mut PostProcessConfig,
    ) -> StylesEdit {
        let mut edit = StylesEdit::default();
        let Some(path) = self.scene_file.as_deref() else {
            return edit;
        };
        let file = path.file_name().map_or_else(
            || path.display().to_string(),
            |f| f.to_string_lossy().into_owned(),
        );
        let button = ui
            .add_enabled(
                self.commit_blocked.is_none(),
                egui::Button::new("Commit to scene").small(),
            )
            .on_hover_text(format!(
                "Write this look into {file}'s [post_process.styles.{}], keeping its comments \
                 (values back at their default are removed)",
                style.name
            ));
        let button = match &self.commit_blocked {
            Some(reason) => button.on_disabled_hover_text(reason.as_str()),
            None => button,
        };
        let now = ui.input(|i| i.time);
        if button.clicked() {
            match self.commit(style, pp) {
                Ok(()) => {
                    edit.changed = true;
                    edit.committed = Some(style.name);
                    self.commit_note = Some((style.name, now, None));
                }
                Err(e) => {
                    tracing::error!("Commit to scene failed: {e}");
                    self.commit_note = Some((style.name, now, Some(e)));
                }
            }
        }
        if let Some((name, at, error)) = &self.commit_note {
            if *name == style.name {
                match error {
                    Some(e) => {
                        ui.colored_label(ui.visuals().error_fg_color, "not written")
                            .on_hover_text(e.as_str());
                    }
                    None if now - at < COPIED_NOTE_SECS => {
                        ui.weak("written");
                        ui.ctx().request_repaint();
                    }
                    None => {}
                }
            }
        }
        edit
    }

    /// One setting's row of four grid cells (label, widget, reset,
    /// authored dot); true when it changed.
    fn row(
        &mut self,
        ui: &mut egui::Ui,
        style: &'static StyleDescriptor,
        control: StyleControl,
        pp: &mut PostProcessConfig,
        authored: Option<&StyleSettings>,
    ) -> bool {
        let label = style.label_for(control.name());
        let help = style.hint(control.name()).map_or("", |h| h.help);
        let with_help = |r: egui::Response| {
            if help.is_empty() {
                r
            } else {
                r.on_hover_text(help)
            }
        };
        let mut changed = false;
        match control {
            StyleControl::Option(o) => {
                let current = style.option(pp, o.name).to_string();
                let scene = authored.map(|a| {
                    a.option(style.name, o.name)
                        .unwrap_or(o.default)
                        .to_string()
                });
                let mut picked = None;
                let choices = self.choices(style, o, pp);
                {
                    with_help(ui.label(&label));
                    if choices.is_empty() {
                        let key = (style.name, o.name);
                        let buffer = self.text.entry(key).or_insert_with(|| current.clone());
                        let r = ui.add(
                            egui::TextEdit::singleline(buffer)
                                .id(text_id(style.name, o.name))
                                .desired_width(120.0)
                                .hint_text(o.default),
                        );
                        if r.lost_focus() {
                            if *buffer != current {
                                picked = Some(buffer.clone());
                            }
                        } else if !r.has_focus() && *buffer != current {
                            // Not being typed in: mirror the config
                            *buffer = current.clone();
                        }
                        with_help(r);
                    } else {
                        let shown = if current.is_empty() {
                            "(default)"
                        } else {
                            current.as_str()
                        };
                        egui::ComboBox::from_id_salt((style.name, o.name))
                            .selected_text(shown)
                            .show_ui(ui, |ui| {
                                if o.default.is_empty()
                                    && ui
                                        .selectable_label(current.is_empty(), "(default)")
                                        .clicked()
                                {
                                    picked = Some(String::new());
                                }
                                for choice in &choices {
                                    if ui
                                        .selectable_label(current == *choice, choice.as_str())
                                        .clicked()
                                    {
                                        picked = Some(choice.clone());
                                    }
                                }
                                // A value the scene names that the list
                                // doesn't (a palette by its path) stays
                                // pickable
                                if !current.is_empty() && !choices.contains(&current) {
                                    let _ = ui.selectable_label(true, &current);
                                }
                            });
                    }
                    if reset_cell(ui, current != o.default, &format!("{:?}", o.default)) {
                        picked = Some(o.default.to_string());
                    }
                    self.marked += dot_cell(
                        ui,
                        scene.filter(|s| *s != current).map(|s| format!("{s:?}")),
                    );
                }
                if let Some(value) = picked.filter(|v| *v != current) {
                    pp.styles.set_option(style.name, o.name, &value);
                    self.text.remove(&(style.name, o.name));
                    changed = true;
                }
            }
            StyleControl::Param(p) => {
                let integer = style.hint(p.name).is_some_and(|h| h.integer);
                let mut value = style.param(pp, p.name);
                let scene = authored.map(|a| a.param(style.name, p.name).unwrap_or(p.default));
                {
                    with_help(ui.label(&label));
                    let mut slider = egui::Slider::new(&mut value, p.min..=p.max);
                    if integer {
                        slider = slider.step_by(1.0).fixed_decimals(0);
                    }
                    changed |= with_help(ui.add(slider)).changed();
                    if reset_cell(ui, value != p.default, &number(p.default, integer)) {
                        value = p.default;
                        changed = true;
                    }
                    self.marked += dot_cell(
                        ui,
                        scene.filter(|s| *s != value).map(|s| number(s, integer)),
                    );
                }
                if changed {
                    pp.styles.set_param(style.name, p.name, value);
                }
            }
            StyleControl::Color { channels, .. } => {
                let mut rgb = channels.map(|c| style.param(pp, c.name));
                let defaults = channels.map(|c| c.default);
                let scene = authored
                    .map(|a| channels.map(|c| a.param(style.name, c.name).unwrap_or(c.default)));
                {
                    with_help(ui.label(&label));
                    // Style colours are display values (what the eye sees,
                    // as the display-stage shaders work), so the picker is
                    // the sRGB one; its bytes replace the value only when
                    // the user picks, so an untouched colour stays exact
                    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                    let mut srgb = rgb.map(byte);
                    ui.horizontal(|ui| {
                        if with_help(ui.color_edit_button_srgb(&mut srgb)).changed() {
                            rgb = srgb.map(|b| b as f32 / 255.0);
                            changed = true;
                        }
                        ui.weak(format!("{:.2} {:.2} {:.2}", rgb[0], rgb[1], rgb[2]));
                    });
                    if reset_cell(ui, rgb != defaults, &rgb_text(defaults)) {
                        rgb = defaults;
                        changed = true;
                    }
                    self.marked += dot_cell(ui, scene.filter(|s| *s != rgb).map(rgb_text));
                }
                if changed {
                    for (c, v) in channels.iter().zip(rgb) {
                        pp.styles
                            .set_param(style.name, c.name, v.clamp(c.min, c.max));
                    }
                }
            }
        }
        changed
    }
}

/// Write style `style`'s settings in `pp` into the scene file at `path`
/// (#364): the block Copy TOML would copy, merged key by key into the
/// file's `[post_process.styles.<name>]`. Returns the settings the file now
/// holds for the style, read back from that block.
pub fn commit_style(
    path: &Path,
    style: &'static StyleDescriptor,
    pp: &PostProcessConfig,
) -> Result<StyleSettings, String> {
    let block = stylize::style_toml(style, pp);
    let mut doc = flint_scene::SceneDocument::from_file(path)?;
    doc.set_style_table(style.name, &block, &style.table_keys())?;
    doc.save(path)?;
    let parsed: toml::Table =
        toml::from_str(&block).map_err(|e| format!("Style block doesn't parse: {e}"))?;
    let table = parsed
        .get("post_process")
        .and_then(|p| p.get("styles"))
        .and_then(|s| s.get(style.name))
        .and_then(|t| t.as_table())
        .cloned()
        .unwrap_or_default();
    let tables = std::collections::BTreeMap::from([(style.name.to_string(), table)]);
    let (settings, warnings) = StyleSettings::from_tables(&tables);
    for warning in warnings {
        tracing::warn!("Committed style block: {warning}");
    }
    Ok(settings)
}

/// The styles on in `stage`, in the order its chain runs them (#299).
pub fn chain_of(stage: StyleStage, pp: &PostProcessConfig) -> Vec<(&'static StyleDescriptor, f32)> {
    stylize::stage_chain(stage, pp)
        .into_iter()
        .filter(|(d, _)| d.is_enabled(pp))
        .collect()
}

/// Run `chain[i]` one place earlier, swapping its order with the style
/// before it. Equal orders (both at their defaults' tie) step the moved one
/// just in front instead, so the move always shows.
pub fn move_earlier(
    pp: &mut PostProcessConfig,
    chain: &[(&'static StyleDescriptor, f32)],
    i: usize,
) {
    let ((a, oa), (b, ob)) = (chain[i - 1], chain[i]);
    if oa == ob {
        pp.styles.set_order(b.name, Some(oa - 0.5));
    } else {
        pp.styles.set_order(b.name, Some(oa));
        pp.styles.set_order(a.name, Some(ob));
    }
}

/// The chain line of each stage with a style on: its styles in running
/// order, with buttons to move one earlier or later. True when the order
/// changed.
fn chain_ui(ui: &mut egui::Ui, pp: &mut PostProcessConfig) -> bool {
    let mut changed = false;
    for (stage, name) in [(StyleStage::Hdr, "HDR"), (StyleStage::Display, "Display")] {
        let chain = chain_of(stage, pp);
        if chain.is_empty() {
            continue;
        }
        let mut swap = None;
        ui.horizontal_wrapped(|ui| {
            ui.weak(format!("{name} chain:")).on_hover_text(
                "The styles on in this stage, first to last: each reads the one before. \
                 `order = <n>` in a style's table sets its place (lower runs first); \
                 without one a style stands at its registration index",
            );
            for (i, (style, order)) in chain.iter().enumerate() {
                if i > 0 {
                    ui.weak("→");
                    if ui
                        .small_button("swap")
                        .on_hover_text(format!(
                            "Run {} before {}",
                            style.label,
                            chain[i - 1].0.label
                        ))
                        .clicked()
                    {
                        swap = Some(i);
                    }
                }
                let authored = pp.styles.order(style.name).is_some();
                ui.label(style.label).on_hover_text(if authored {
                    format!("order = {}", number(*order, false))
                } else {
                    format!("order {} (its registration index)", number(*order, false))
                });
            }
        });
        if let Some(i) = swap {
            move_earlier(pp, &chain, i);
            changed = true;
        }
    }
    changed
}

/// The id of a free-text option's field.
fn text_id(style: &str, option: &str) -> egui::Id {
    egui::Id::new(("style option text", style, option))
}

/// The reset cell: a small button that puts a value back to its default
/// while it differs (true on click), else an empty cell.
fn reset_cell(ui: &mut egui::Ui, differs: bool, default: &str) -> bool {
    if !differs {
        ui.label("");
        return false;
    }
    ui.small_button("↺")
        .on_hover_text(format!("Back to the default, {default}"))
        .clicked()
}

/// The authored cell: a dot when the scene says something else (hover for
/// what), else an empty cell.
fn dot_cell(ui: &mut egui::Ui, scene: Option<String>) -> usize {
    match scene {
        Some(scene) => {
            // Painted: egui's default fonts have no filled-circle glyph
            let size = ui.spacing().interact_size.y;
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(size * 0.6, size), egui::Sense::hover());
            ui.painter()
                .circle_filled(rect.center(), size * 0.16, ui.visuals().warn_fg_color);
            response.on_hover_text(format!("The scene says {scene}"));
            1
        }
        None => {
            ui.label("");
            0
        }
    }
}

fn number(value: f32, integer: bool) -> String {
    if integer {
        format!("{value:.0}")
    } else {
        format!("{value:.3}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

fn rgb_text(rgb: [f32; 3]) -> String {
    format!("{:.2}, {:.2}, {:.2}", rgb[0], rgb[1], rgb[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every piece of text the editor drew in one frame.
    fn drawn_text(
        editor: &mut StylesEditor,
        pp: &mut PostProcessConfig,
        authored: Option<&StyleSettings>,
    ) -> Vec<String> {
        let ctx = egui::Context::default();
        let mut out = Vec::new();
        // Two passes: collapsing state settles on the first
        for _ in 0..2 {
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    editor.ui(ui, pp, authored);
                });
            });
            out.clear();
            for clipped in output.shapes {
                collect_text(&clipped.shape, &mut out);
            }
        }
        out
    }

    fn collect_text(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => out.push(t.galley.job.text.clone()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect_text(s, out)),
            _ => {}
        }
    }

    fn all_on() -> PostProcessConfig {
        let mut pp = PostProcessConfig::default();
        for s in stylize::registry().iter() {
            pp.styles.set_enabled(s.name, true);
        }
        pp
    }

    #[test]
    fn every_registered_setting_gets_a_labelled_row() {
        let mut editor = StylesEditor::default();
        let mut pp = all_on();
        let text = drawn_text(&mut editor, &mut pp, None);
        for style in stylize::registry().iter() {
            assert!(text.contains(&style.label.to_string()), "{}", style.label);
            for control in style.controls() {
                let label = style.label_for(control.name());
                assert!(text.contains(&label), "{}: {label} not drawn", style.name);
            }
        }
        // Drawing changes nothing by itself
        assert_eq!(pp.styles, all_on().styles);
    }

    #[test]
    fn edits_against_the_scene_are_marked() {
        let mut editor = StylesEditor::default();
        let mut authored = StyleSettings::default();
        authored.set_enabled("ascii", true);
        let mut pp = PostProcessConfig {
            styles: authored.clone(),
            ..Default::default()
        };
        let clean = drawn_text(&mut editor, &mut pp, Some(&authored));
        assert!(!clean.contains(&"edited".to_string()));
        assert_eq!(editor.marked, 0, "{clean:?}");

        pp.styles.set_param("ascii", "cell", 16.0);
        let edited = drawn_text(&mut editor, &mut pp, Some(&authored));
        assert!(edited.contains(&"edited".to_string()));
        assert_eq!(editor.marked, 1);
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// A style with a free-text option (no choices, none found), as a
    /// file-named setting is. Registered nowhere; drawn by hand.
    static TEXT_STYLE: StyleDescriptor = StyleDescriptor {
        name: "text_test",
        label: "Text test",
        stage: StyleStage::Display,
        params: &[],
        options: &[StyleOption::new("file", "", &[])],
        uses_mask: flint_render::stylize::MaskUse::Never,
        needs: &[],
        hints: &[],
        component: None,
        feeds_bloom: false,
        create: |_| unreachable!("never drawn"),
    };

    #[test]
    fn typing_a_file_name_survives_frames_and_commits_on_enter() {
        // The field used to be over a fresh copy of the config each
        // frame, so every keystroke was gone by the next one
        let ctx = egui::Context::default();
        let mut editor = StylesEditor::default();
        let mut pp = PostProcessConfig::default();
        pp.styles.set_enabled(TEXT_STYLE.name, true);
        let mut frame = |pp: &mut PostProcessConfig, events: Vec<egui::Event>| {
            let mut edit = StylesEdit::default();
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    edit.merge(editor.section(ui, &TEXT_STYLE, pp, None));
                });
            });
            edit
        };
        frame(&mut pp, vec![]);
        ctx.memory_mut(|m| m.request_focus(text_id("text_test", "file")));
        frame(&mut pp, vec![]);
        frame(&mut pp, vec![key(egui::Key::A, egui::Modifiers::COMMAND)]);
        for c in ["t", "e", "r", "m"] {
            let edit = frame(&mut pp, vec![egui::Event::Text(c.into())]);
            assert!(!edit.changed, "typing doesn't apply until Enter");
        }
        assert_eq!(pp.styles.option("text_test", "file"), None);
        let edit = frame(&mut pp, vec![key(egui::Key::Enter, egui::Modifiers::NONE)]);
        assert!(edit.changed);
        assert_eq!(pp.styles.option("text_test", "file"), Some("term"));
    }

    #[test]
    fn the_scene_folder_adds_its_palettes_fonts_and_ramps() {
        let mut src = flint_core::vfs::MemorySource::new();
        src.insert(
            "demo/palettes/tidepool.palette.toml",
            "colors = [\"#0a3d62\", \"#3c6382\"]\n",
        );
        src.insert(
            "demo/fonts/plain.glyphs.toml",
            "[ramps.dots]\nglyphs = \" .:\"\n",
        );
        flint_core::vfs::scoped(std::sync::Arc::new(src), || {
            let ascii = stylize::find("ascii").unwrap();
            let option = |name: &str| ascii.options.iter().find(|o| o.name == name).unwrap();
            let mut editor = StylesEditor::default();
            let mut pp = all_on();
            // No scene: the built-ins
            let palettes = editor.choices(ascii, option("palette"), &pp);
            assert!(!palettes.iter().any(|p| p == "tidepool"), "{palettes:?}");
            editor.set_scene_dir(Some(PathBuf::from("demo")));
            let palettes = editor.choices(ascii, option("palette"), &pp);
            assert!(palettes.iter().any(|p| p == "tidepool"), "{palettes:?}");
            assert!(palettes.iter().any(|p| p == "phosphor"), "built-ins stay");
            let fonts = editor.choices(ascii, option("font"), &pp);
            assert_eq!(fonts, ["builtin", "plain"]);
            // The ramp list is the chosen font's, found again when it changes
            let ramps = editor.choices(ascii, option("ramp"), &pp);
            assert!(ramps.iter().any(|r| r == "classic"), "{ramps:?}");
            pp.styles.set_option("ascii", "font", "plain");
            assert_eq!(editor.choices(ascii, option("ramp"), &pp), ["dots"]);
            // A fixed list is the descriptor's own
            assert_eq!(
                editor.choices(ascii, option("color"), &pp),
                option("color").choices
            );
        });
    }

    #[test]
    fn the_chain_line_lists_the_stage_in_order_and_swaps() {
        let mut pp = PostProcessConfig::default();
        pp.styles.set_enabled("watercolor", true);
        pp.styles.set_enabled("ascii", true);
        pp.styles.set_enabled("invert", true);
        let names = |pp: &PostProcessConfig| -> Vec<&str> {
            chain_of(StyleStage::Display, pp)
                .iter()
                .map(|(d, _)| d.name)
                .collect()
        };
        assert_eq!(names(&pp), ["watercolor", "ascii", "invert"]);
        let text = drawn_text(&mut StylesEditor::default(), &mut pp, None);
        assert!(text.contains(&"Display chain:".to_string()), "{text:?}");
        assert!(!text.iter().any(|t| t == "HDR chain:"), "nothing on in HDR");

        // ASCII before watercolor: the two swap orders
        let chain = chain_of(StyleStage::Display, &pp);
        move_earlier(&mut pp, &chain, 1);
        assert_eq!(names(&pp), ["ascii", "watercolor", "invert"]);
        assert_eq!(pp.styles.order("ascii"), Some(1.0));
        assert_eq!(pp.styles.order("watercolor"), Some(2.0));
        // A tie still moves
        pp.styles.set_order("invert", Some(2.0));
        assert_eq!(names(&pp), ["ascii", "watercolor", "invert"]);
        let chain = chain_of(StyleStage::Display, &pp);
        move_earlier(&mut pp, &chain, 2);
        assert_eq!(names(&pp), ["ascii", "invert", "watercolor"]);
    }

    /// A scene's `[post_process.styles]` as a fresh load reads it.
    fn styles_in_file(path: &Path) -> StyleSettings {
        let text = std::fs::read_to_string(path).unwrap();
        let file: toml::Table = toml::from_str(&text).unwrap();
        let tables = file
            .get("post_process")
            .and_then(|p| p.get("styles"))
            .and_then(|s| s.as_table())
            .map(|s| {
                s.iter()
                    .map(|(k, v)| (k.clone(), v.as_table().unwrap().clone()))
                    .collect()
            })
            .unwrap_or_default();
        let (settings, warnings) = StyleSettings::from_tables(&tables);
        assert!(warnings.is_empty(), "{warnings:?}");
        settings
    }

    #[test]
    fn commit_writes_the_look_and_clears_the_marks() {
        let dir = std::env::temp_dir().join(format!("flint-commit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("room.scene.toml");
        std::fs::write(
            &path,
            "# A dim room\n[scene]\nname = \"Room\"\n\n[post_process]\nbloom_intensity = 0.1 # soft\n\n\
             # The look\n[post_process.styles.ascii]\ncell = 12 # tuned by eye\nglow = 0.6\n",
        )
        .unwrap();
        let mut authored = styles_in_file(&path);
        let mut pp = PostProcessConfig {
            styles: authored.clone(),
            ..Default::default()
        };
        // Tuned in F4: a new cell and ramp, glow back at its default, a tint
        let ascii = stylize::find("ascii").unwrap();
        pp.styles.set_param("ascii", "cell", 9.0);
        pp.styles.set_option("ascii", "ramp", "blocks");
        let glow = ascii.params.iter().find(|p| p.name == "glow").unwrap();
        pp.styles.set_param("ascii", "glow", glow.default);
        pp.styles.set_color("ascii", "tint", [0.35, 1.0, 0.45]);
        let shown = pp.clone();

        let mut editor = StylesEditor::default();
        editor.set_scene_file(Some(path.clone()));
        editor.commit(ascii, &mut pp).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# A dim room"), "{text}");
        assert!(text.contains("bloom_intensity = 0.1 # soft"), "{text}");
        assert!(
            text.contains("# The look\n[post_process.styles.ascii]"),
            "{text}"
        );
        assert!(text.contains("cell = 9 # tuned by eye"), "{text}");
        assert!(!text.contains("glow"), "{text}");

        // Loaded fresh, the scene draws what was on screen
        let fresh = styles_in_file(&path);
        assert!(fresh.same_style("ascii", &pp.styles));
        for p in ascii.params {
            assert_eq!(
                ascii.param(&shown, p.name),
                ascii.param(&pp, p.name),
                "{}",
                p.name
            );
        }
        for o in ascii.options {
            assert_eq!(
                ascii.option(&shown, o.name),
                ascii.option(&pp, o.name),
                "{}",
                o.name
            );
        }

        // The host's authored settings follow the commit: nothing is marked
        authored.restore("ascii", &pp.styles);
        let text = drawn_text(&mut editor, &mut pp, Some(&authored));
        assert!(text.contains(&"Commit to scene".to_string()), "{text:?}");
        assert!(!text.contains(&"edited".to_string()), "{text:?}");
        assert_eq!(editor.marked, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_scene_file_no_commit_button() {
        let mut pp = all_on();
        let text = drawn_text(&mut StylesEditor::default(), &mut pp, None);
        assert!(!text.contains(&"Commit to scene".to_string()));
        assert!(text.contains(&"Copy TOML".to_string()));
    }

    #[test]
    fn numbers_print_short() {
        assert_eq!(number(8.0, true), "8");
        assert_eq!(number(0.35, false), "0.35");
        assert_eq!(number(1.0, false), "1");
    }
}
