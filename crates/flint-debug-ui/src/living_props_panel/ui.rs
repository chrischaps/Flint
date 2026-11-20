//! The panel itself: its sections, each knob's control as the table
//! describes it, and the marks that say where a value comes from.

use egui::Color32;
use flint_procgen::WoozePreset;

use super::clock::draw_clock;
use super::knobs::{Knob, KnobControl, KnobFlags, KnobPanel, KnobSection, KnobValue, LivingKnobs};
use super::state::{side_name, LivingPanelAction, LivingPropRow, PanelScope};
use super::{LivingPropsDebugPanel, FROZEN, INK, PARTS};
use crate::widgets::{mixed_slider, section};

/// What a `mixed` knob says when hovered, for the whole grove.
pub(super) const MIXED: &str = "The species disagree here, so this shows the first prop's value. \
Moving it sets it for the whole grove, apart from a species that sets its own; the \
knobs beside it stay each species' own.";

/// What a `mixed` knob says when hovered, for one species.
pub(super) const MIXED_SPECIES: &str =
    "This species' trees disagree here: a zone some stand in, a \
tree's own value, or lineages held differently. It shows the first one's. Moving it \
makes it the species' own, on every one of its trees.";

/// What each layer means, for a mark's hover.
pub(super) fn layer_says(layer: &str) -> String {
    match layer {
        "spec" => "spec: the species' .procgen.toml".to_string(),
        "entity" => "entity: a tree's own living_prop component".to_string(),
        "global" => "global: the scene's [living] or [wooze], or this panel at Global".to_string(),
        "species" => "species: this species' own value".to_string(),
        zone => match zone.strip_prefix("zone:") {
            Some(name) => format!("zone:{name}: the wooze zone some of its trees stand in"),
            None => zone.to_string(),
        },
    }
}

/// What one knob's mark says, beside it.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Mark {
    /// Nothing: Global, with no species setting its own.
    Quiet,
    /// At Global: these species set their own, so the knob doesn't reach them.
    Owned(Vec<String>),
    /// At a species: the layers its value comes from.
    From(Vec<String>),
}

/// What an unsaved level's mark says when hovered.
pub(super) const UNSAVED: &str =
    "*: values the scene file doesn't have yet. Commit to File writes \
them; As loaded puts the file's back.";

impl LivingPropsDebugPanel {
    /// Global, or a species, picked with one click. A species with values of
    /// its own wears a dot.
    pub(super) fn scope_ui(&mut self, ui: &mut egui::Ui) {
        if self.state.species.is_empty() {
            return;
        }
        let mut picked = self.scope.clone();
        ui.horizontal_wrapped(|ui| {
            ui.weak("Tuning");
            let global = unsaved_text("Global".to_string(), self.state.unsaved_global);
            let hover = "The whole grove, under any species' own value";
            ui.selectable_value(&mut picked, PanelScope::Global, global)
                .on_hover_text(with_unsaved(hover, self.state.unsaved_global));
            for species in &self.state.species {
                let own = if species.has_own() { " \u{2022}" } else { "" };
                let text = format!("{} {}{own}", species.name, species.props);
                let text = unsaved_text(text, species.unsaved);
                let unsaved = species.unsaved;
                ui.selectable_value(&mut picked, PanelScope::Species(species.name.clone()), text)
                    .on_hover_text(with_unsaved(
                        &format!(
                            "{}: {} trees. A knob moved here is the species' own: it beats the \
                         grove's, the field and its zones{}",
                            species.name,
                            species.props,
                            if own.is_empty() {
                                "."
                            } else {
                                ". The dot: it already has some."
                            }
                        ),
                        unsaved,
                    ));
            }
        });
        self.scope = picked;
        let Some(species) = self.picked_species().cloned() else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(
                INK,
                format!("{}: its own values beat the grove's", species.name),
            );
            if ui
                .add_enabled(species.has_own(), egui::Button::new("Clear all").small())
                .on_hover_text("Every value of its own gone: its trees take the grove's again")
                .clicked()
            {
                self.push(LivingPanelAction::Clear(species.name.clone(), None));
            }
        });
        if self.state.overlay {
            ui.weak("The pose overlay labels only its trees.");
        }
    }

    /// The tree picked in the world, or how to pick one (#248). One picked off
    /// screen says so here, since its window can't be drawn beside it.
    pub(super) fn picked_ui(&mut self, ui: &mut egui::Ui) {
        if self.state.rows.is_empty() {
            return;
        }
        let Some(row) = self.picked_row().cloned() else {
            ui.weak("Click a tree to pick it, or F8 for the one at the view centre.");
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.weak("Picked");
            ui.colored_label(INK, egui::RichText::new(&row.name).strong());
            ui.weak(&row.species);
            if self.picked_on_screen == Some(false) {
                ui.colored_label(FROZEN, "off screen")
                    .on_hover_text("Turn to it to see its window, or Shift+F8 for the next tree");
            }
            if ui
                .small_button("\u{d7}")
                .on_hover_text("Let it go (Esc)")
                .clicked()
            {
                self.select(None);
            }
        });
    }

    pub(super) fn clock_section(&mut self, ui: &mut egui::Ui) {
        if let Some(row) = self.focused().cloned() {
            draw_clock(ui, &row, &self.state.knobs.stagger, self.state.frozen, 76.0);
            ui.monospace(format!(
                "{}  pose {} > {}  {:>3.0}%",
                row.name,
                row.pose,
                row.pose + 1,
                row.weight * 100.0
            ));
        }
        ui.horizontal(|ui| {
            let frozen = self.state.frozen;
            if ui.selectable_label(frozen, "Freeze").clicked() {
                self.state.frozen = !frozen;
                self.push(LivingPanelAction::Freeze(!frozen));
            }
            if ui
                .add_enabled(frozen, egui::Button::new("Step"))
                .on_hover_text("Exactly one beat of the warped clock")
                .clicked()
            {
                self.push(LivingPanelAction::Step);
            }
            let mut overlay = self.state.overlay;
            if ui
                .checkbox(&mut overlay, "Pose overlay")
                .on_hover_text("Each prop's lineage and poses, in the world")
                .changed()
            {
                self.state.overlay = overlay;
                self.push(LivingPanelAction::Overlay(overlay));
            }
        });
        let health = self.state.health;
        let starved = health.frames_starved + health.blends_starved;
        let busy = if health.waiting > 0 {
            format!("{} waiting on poses", health.waiting)
        } else if health.reholding {
            "holding poses to a new variance".to_string()
        } else if health.rings_short > 0 {
            format!("{} rings filling", health.rings_short)
        } else {
            "rings full".to_string()
        };
        let line = format!("{} props: {busy}", self.state.rows.len());
        if starved > 0 {
            ui.colored_label(
                Color32::from_rgb(232, 120, 90),
                format!(
                    "{line}. {} frames short of a pose, {} blends kept their bones",
                    health.frames_starved, health.blends_starved
                ),
            );
        } else {
            ui.weak(line);
        }
    }

    pub(super) fn knobs_ui(&mut self, ui: &mut egui::Ui) {
        let species = self.picked_species().cloned();
        let (mut k, mixed) = match &species {
            Some(species) => (species.knobs.clone(), species.mixed),
            None => (self.state.knobs.clone(), self.state.mixed),
        };
        let before = k.clone();
        let says = if species.is_some() {
            MIXED_SPECIES
        } else {
            MIXED
        };
        let marks: Vec<Mark> = Knob::ALL.iter().map(|knob| self.mark(*knob)).collect();
        let mut touched = KnobFlags::default();
        let mut clear: Option<Knob> = None;
        // A knob, then its mark: where its value comes from, at a species, or
        // which species it doesn't reach, at Global.
        let mut row =
            |ui: &mut egui::Ui, knob: Knob, add: &mut dyn FnMut(&mut egui::Ui) -> bool| {
                ui.horizontal(|ui| {
                    let moved = add(ui);
                    if paint_mark(ui, &marks[knob.index()]) {
                        clear = Some(knob);
                    }
                    moved
                })
                .inner
            };

        section(ui, "Clock", true, |ui| {
            for knob in Knob::in_section(KnobSection::Clock) {
                let moved = row(ui, knob, &mut |ui| {
                    control(ui, knob, &mut k, mixed.get(knob), says, "living_panel")
                });
                touched.set_flag(knob, moved);
            }
            if species.is_none() && self.state.no_field && mixed.any_clock() {
                ui.weak(
                    "No [wooze] table: each species runs its spec's clock. Turning any of \
                     these gives the scene one field, from the first prop's clock, so all \
                     four even out across the grove.",
                );
            }
        });

        if species.is_some() {
            // What a species can't set, still in view, and why.
            section(ui, "Whole scene only", true, |ui| {
                ui.add_enabled_ui(false, |ui| boil_ui(ui, &mut k.clone()));
                ui.weak(
                    "The boil is one screen pass over the whole frame, so every species \
                     boils together. Tune it at Global.",
                );
                ui.add_enabled(
                    false,
                    egui::Label::new(format!("shadows: {}", self.state.shadows)),
                );
                ui.weak(
                    "Shadows are the scene's [living] shadows, one choice for every living \
                     prop, so a species can't set them.",
                );
            });
        } else {
            section(ui, "Shading boil", true, |ui| {
                boil_ui(ui, &mut k);
                let (paint, ink) = (self.state.paint_pass, self.state.ink_pass);
                if !paint && !ink {
                    ui.weak("Needs Kuwahara for paint and contour for ink: F4.");
                } else if !paint {
                    ui.weak("Ink only. Paint needs Kuwahara: F4.");
                } else if !ink {
                    ui.weak("Paint only. Ink needs contour: F4.");
                }
            });
        }

        section(ui, "Echoes", true, |ui| {
            for knob in Knob::in_section(KnobSection::Echoes) {
                let moved = row(ui, knob, &mut |ui| {
                    control(ui, knob, &mut k, mixed.get(knob), says, "living_panel")
                });
                touched.set_flag(knob, moved);
            }
        });

        section(ui, "Variance", true, |ui| {
            for knob in Knob::in_section(KnobSection::Variance) {
                let moved = row(ui, knob, &mut |ui| {
                    control(ui, knob, &mut k, mixed.get(knob), says, "living_panel")
                });
                touched.set_flag(knob, moved);
            }
            if ui
                .small_button("Unheld")
                .on_hover_text("amount 1, steady 0, keep size off: each pose as it grew")
                .clicked()
            {
                k.variance_amount = 1.0;
                k.variance_steady = 0.0;
                k.keep_size = false;
                touched.variance_amount = true;
                touched.variance_steady = true;
                touched.keep_size = true;
            }
        });

        section(ui, "Silhouette", true, |ui| {
            let knob = Knob::Softness;
            let moved = row(ui, knob, &mut |ui| {
                control(ui, knob, &mut k, mixed.get(knob), says, "living_panel")
            });
            touched.set_flag(knob, moved);
            ui.weak("Feathers the body's outline at rest; the cards keep their edges.");
        });

        // A touch says a knob moved where its number alone can't: a `mixed` one
        // set to the value it showed, or Unheld on a knob already there.
        let moved = k.moved(&before, &touched);
        if moved != WoozePreset::default() {
            // What moved is now the same on every prop it reached, and at a
            // species, its own, until the host says so itself next frame.
            let set = KnobFlags::set_by(&moved);
            match &species {
                Some(picked) => {
                    if let Some(own) =
                        (self.state.species.iter_mut()).find(|s| s.name == picked.name)
                    {
                        own.knobs = k;
                        own.mixed = mixed.without(&set);
                        for knob in Knob::ALL.into_iter().filter(|knob| set.get(*knob)) {
                            own.sources.set(knob, vec!["species".to_string()]);
                        }
                    }
                    let scope = PanelScope::Species(picked.name.clone());
                    self.push(LivingPanelAction::Set(scope, moved));
                }
                None => {
                    self.state.knobs = k;
                    self.state.mixed = mixed.without(&set);
                    self.push(LivingPanelAction::Set(PanelScope::Global, moved));
                }
            }
        }
        if let (Some(knob), Some(picked)) = (clear, &species) {
            self.push(LivingPanelAction::Clear(picked.name.clone(), Some(knob)));
        }
    }

    pub(super) fn presets_ui(&mut self, ui: &mut egui::Ui) {
        section(ui, "Presets", true, |ui| {
            if !self.state.preset_names.contains(&self.picked) {
                self.picked = self.state.preset_names.first().cloned().unwrap_or_default();
            }
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("living_preset")
                    .selected_text(self.picked.clone())
                    .show_ui(ui, |ui| {
                        for name in self.state.preset_names.clone() {
                            ui.selectable_value(&mut self.picked, name.clone(), name);
                        }
                    });
                if ui
                    .add_enabled(!self.picked.is_empty(), egui::Button::new("Load"))
                    .clicked()
                {
                    self.push(LivingPanelAction::Load(self.picked.clone()));
                }
                if ui
                    .button("As loaded")
                    .on_hover_text("Every knob back as the scene set it")
                    .clicked()
                {
                    self.push(LivingPanelAction::Reset);
                }
            });
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.save_as).desired_width(120.0));
                if ui
                    .button("Save")
                    .on_hover_text(
                        "A preset: the grove's knobs, and each species' own values as \
                         [living.species.<spec>] tables",
                    )
                    .clicked()
                {
                    self.push(LivingPanelAction::Save(self.save_as.clone()));
                }
            });
            self.commit_ui(ui);
            if let Some(status) = &self.state.status {
                ui.label(egui::RichText::new(status).small());
            }
        });
    }

    /// Commit to File (#249), and how much it would write.
    pub(super) fn commit_ui(&mut self, ui: &mut egui::Ui) {
        let unsaved = self.state.unsaved.clone();
        ui.horizontal_wrapped(|ui| {
            let hover = if unsaved.is_empty() {
                "The scene file already gives every value as it stands.".to_string()
            } else {
                let mut lines: Vec<String> = unsaved.iter().take(12).cloned().collect();
                if unsaved.len() > 12 {
                    lines.push(format!("and {} more", unsaved.len() - 12));
                }
                format!(
                    "Writes into the scene file, each on its own line, the rest of the \
                     file as it is:\n{}",
                    lines.join("\n")
                )
            };
            if ui
                .add_enabled(!unsaved.is_empty(), egui::Button::new("Commit to File"))
                .on_hover_text(hover.clone())
                .on_disabled_hover_text(hover)
                .clicked()
            {
                self.push(LivingPanelAction::Commit);
            }
            match unsaved.len() {
                0 => ui.weak("the scene has it all"),
                1 => ui.colored_label(INK, "* 1 value unsaved"),
                n => ui.colored_label(INK, format!("* {n} values unsaved")),
            };
        });
    }

    /// Two presets on the same grove: pick them, compare, flip, split.
    pub(super) fn ab_ui(&mut self, ui: &mut egui::Ui) {
        section(ui, "A/B", true, |ui| {
            let names = self.state.preset_names.clone();
            if let Some(ab) = self.state.ab.clone() {
                ui.horizontal(|ui| {
                    let tuned = if ab.showing_b { "B" } else { "A" };
                    ui.label(format!("A {}  ·  B {}", side_name(&ab.a), side_name(&ab.b)));
                    ui.weak(format!("tuning {tuned}"));
                });
                ui.horizontal(|ui| {
                    if ui
                        .button(if ab.showing_b { "Show A" } else { "Show B" })
                        .on_hover_text("F6: the other preset, same camera, same moment")
                        .clicked()
                    {
                        self.push(LivingPanelAction::Flip);
                    }
                    let mut split = ab.split;
                    if ui
                        .checkbox(&mut split, "Split")
                        .on_hover_text("F7: A on the left, B on the right, at once")
                        .changed()
                    {
                        self.push(LivingPanelAction::Split(split));
                    }
                    if ui
                        .button("Stop")
                        .on_hover_text("Keep the side that's shown")
                        .clicked()
                    {
                        self.push(LivingPanelAction::StopComparing);
                    }
                });
                if ab.split {
                    ui.weak("The knobs above turn the side with the dot.");
                }
                return;
            }
            let (mut a, mut b) = self.ab_picks.clone().unwrap_or_else(|| {
                let b = names
                    .iter()
                    .find(|n| *n == "linklater")
                    .or(names.first())
                    .cloned()
                    .unwrap_or_default();
                (String::new(), b)
            });
            let was = (a.clone(), b.clone());
            for (letter, pick, with_now) in [("A", &mut a, true), ("B", &mut b, false)] {
                egui::ComboBox::from_label(letter)
                    .selected_text(side_name(pick).to_string())
                    .show_ui(ui, |ui| {
                        if with_now {
                            ui.selectable_value(pick, String::new(), "as it was")
                                .on_hover_text("The knobs as they stand when it starts");
                        }
                        for name in &names {
                            ui.selectable_value(pick, name.clone(), name.as_str());
                        }
                    });
            }
            // The first frame only learns the defaults; a pick is a change.
            if (a.clone(), b.clone()) != was {
                self.push(LivingPanelAction::PickAb(a.clone(), b.clone()));
            }
            self.ab_picks = Some((a.clone(), b.clone()));
            if ui
                .add_enabled(!b.is_empty(), egui::Button::new("Compare"))
                .on_hover_text("F6 flips between them, F7 splits the view")
                .clicked()
            {
                self.push(LivingPanelAction::Compare(a, b));
            }
        });
    }

    /// Every prop, grouped by species (#247), narrowed by a filter on its name or
    /// its species'. A group's `tune` picks that species above.
    pub(super) fn rows_ui(&mut self, ui: &mut egui::Ui) {
        section(ui, "Props", false, |ui| {
            ui.horizontal(|ui| {
                ui.weak("filter");
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text("name or species")
                        .desired_width(140.0),
                );
                if !self.filter.is_empty() && ui.small_button("\u{d7}").clicked() {
                    self.filter.clear();
                }
            });
            let filter = self.filter.to_lowercase();
            let rows = self.state.rows.clone();
            let mut groups: Vec<(&str, Vec<(usize, &LivingPropRow)>)> = Vec::new();
            for (i, row) in rows.iter().enumerate() {
                let shown = filter.is_empty()
                    || row.name.to_lowercase().contains(&filter)
                    || row.species.to_lowercase().contains(&filter);
                if !shown {
                    continue;
                }
                match groups
                    .iter_mut()
                    .find(|(species, _)| *species == row.species)
                {
                    Some((_, members)) => members.push((i, row)),
                    None => groups.push((&row.species, vec![(i, row)])),
                }
            }
            groups.sort_by(|a, b| a.0.cmp(b.0));
            if groups.is_empty() {
                ui.weak("No prop matches.");
            }
            egui::ScrollArea::vertical()
                .id_salt("living_rows")
                .max_height(260.0)
                .show(ui, |ui| {
                    for (species, members) in groups {
                        self.species_rows(ui, species, &members, !filter.is_empty());
                    }
                });
        });
    }

    /// One species' rows, under a header that folds them and can pick it.
    pub(super) fn species_rows(
        &mut self,
        ui: &mut egui::Ui,
        species: &str,
        members: &[(usize, &LivingPropRow)],
        filtering: bool,
    ) {
        let picked = self.scope == PanelScope::Species(species.to_string());
        let id = ui.make_persistent_id(("living_species_rows", species));
        let mut fold =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, picked);
        if filtering {
            fold.set_open(true);
        }
        fold.show_header(ui, |ui| {
            let title = egui::RichText::new(format!("{species}  {}", members.len()));
            ui.label(if picked {
                title.strong().color(INK)
            } else {
                title
            });
            if ui
                .selectable_label(picked, egui::RichText::new("tune").small())
                .on_hover_text("Turn this species' own values above")
                .clicked()
            {
                self.scope = if picked {
                    PanelScope::Global
                } else {
                    PanelScope::Species(species.to_string())
                };
            }
        })
        .body(|ui| {
            egui::Grid::new(("living_rows_grid", species))
                .num_columns(4)
                .striped(true)
                .show(ui, |ui| {
                    ui.weak("prop");
                    ui.weak("lineage");
                    ui.weak("pose");
                    ui.weak("ring");
                    ui.end_row();
                    for (i, row) in members {
                        let picked = self.selected.as_deref() == Some(row.name.as_str());
                        if ui
                            .selectable_label(picked || self.focus == *i, &row.name)
                            .on_hover_text("Pick it, as a click on it in the world would")
                            .clicked()
                        {
                            self.focus = *i;
                            self.select(Some(&row.name));
                        }
                        ui.monospace(row.lineage_seed.to_string());
                        ui.monospace(format!("{} {:.0}%", row.pose, row.weight * 100.0));
                        ui.monospace(format!(
                            "{}+{}{}",
                            row.ring_first,
                            row.ring_ready,
                            if row.waiting { " wait" } else { "" }
                        ));
                        ui.end_row();
                    }
                });
        });
    }

    /// What the picked tree reads, and its species' sliders.
    pub(super) fn picked_window(&mut self, ui: &mut egui::Ui, row: &LivingPropRow) {
        ui.set_max_width(270.0);
        // Smaller than the panel's, so it covers as little of the grove as it can.
        let style = ui.style_mut();
        style.spacing.slider_width = 104.0;
        style.spacing.item_spacing.y = 2.0;
        style.spacing.interact_size.y = 15.0;
        for (text, size) in [
            (egui::TextStyle::Body, 12.0),
            (egui::TextStyle::Button, 12.0),
            (egui::TextStyle::Monospace, 11.0),
        ] {
            if let Some(font) = style.text_styles.get_mut(&text) {
                font.size = size;
            }
        }
        ui.horizontal(|ui| {
            ui.colored_label(INK, egui::RichText::new(&row.name).strong());
            ui.weak(format!("{}  L{}", row.species, row.lineage_seed));
        });
        let picked = (self.state.picked.clone()).filter(|p| p.name == row.name);
        let stagger = picked
            .as_ref()
            .map_or_else(|| "none".to_string(), |p| p.knobs.stagger.clone());
        draw_clock(ui, row, &stagger, self.state.frozen, 54.0);
        ui.monospace(format!(
            "pose {} > {}  {:>3.0}%",
            row.pose,
            row.pose + 1,
            row.weight * 100.0
        ));
        let Some(picked) = picked else {
            return;
        };
        ui.label(egui::RichText::new(zones_line(&picked.zones)).small());
        ui.separator();
        let kin = self
            .state
            .rows
            .iter()
            .filter(|r| r.species == row.species)
            .count();
        ui.weak(format!(
            "These set {}'s own values: its {kin} trees, no others.",
            row.species
        ));
        let mut k = picked.knobs.clone();
        let before = k.clone();
        let mut touched = KnobFlags::default();
        let mut clear = None;
        for knob in Knob::ALL {
            ui.horizontal(|ui| {
                let moved = control(ui, knob, &mut k, false, "", "living_picked");
                touched.set_flag(knob, moved);
                if paint_mark(ui, &Mark::From(picked.sources.of(knob).to_vec())) {
                    clear = Some(knob);
                }
            });
        }
        let mut moved = k.moved(&before, &touched);
        moved.boil = None;
        if moved != WoozePreset::default() {
            // What moved is the species' own now, until the host says so itself.
            if let Some(shown) = self.state.picked.as_mut() {
                let set = KnobFlags::set_by(&moved);
                shown.knobs = k;
                for knob in Knob::ALL.into_iter().filter(|knob| set.get(*knob)) {
                    shown.sources.set(knob, vec!["species".to_string()]);
                }
            }
            let scope = PanelScope::Species(row.species.clone());
            self.push(LivingPanelAction::Set(scope, moved));
        }
        if let Some(knob) = clear {
            self.push(LivingPanelAction::Clear(row.species.clone(), Some(knob)));
        }
        let mut keys = String::from("Esc lets go \u{b7} Shift+F8 the next tree");
        if !self.open {
            keys.push_str(" \u{b7} F3 to tune");
        }
        ui.weak(egui::RichText::new(keys).small());
    }
}

/// The zones a tree stands in, as its window says it.
pub(super) fn zones_line(zones: &[(String, f32)]) -> String {
    if zones.is_empty() {
        return "in no zone: the grove's field".to_string();
    }
    let said: Vec<String> = zones
        .iter()
        .map(|(name, weight)| {
            if *weight >= 0.999 {
                name.clone()
            } else {
                format!("{name} {:.0}%", weight * 100.0)
            }
        })
        .collect();
    format!("in zone {}", said.join(", "))
}

/// A scope's name, with a `*` and in ink if it has values the file doesn't.
pub(super) fn unsaved_text(text: String, unsaved: bool) -> egui::RichText {
    if unsaved {
        egui::RichText::new(format!("{text} *")).color(INK)
    } else {
        egui::RichText::new(text)
    }
}

/// A scope's hover, saying what its `*` means if it has one.
pub(super) fn with_unsaved(hover: &str, unsaved: bool) -> String {
    if unsaved {
        format!("{hover}\n\n{UNSAVED}")
    } else {
        hover.to_string()
    }
}

/// One knob's control, as its row in the table describes it: a slider, a
/// combo of named choices or a checkbox. `mixed` shows it as `mixed`, saying
/// `says` when hovered; otherwise its hover is the row's help. `salt` keeps two
/// of them, in the panel and beside a picked tree, apart. Returns whether a
/// hand moved it.
pub(super) fn control(
    ui: &mut egui::Ui,
    which: Knob,
    k: &mut LivingKnobs,
    mixed: bool,
    says: &str,
    salt: &str,
) -> bool {
    use egui::Slider;
    let desc = which.desc();
    let hover = if mixed { says } else { desc.help };
    match (desc.control, k.value_mut(which)) {
        (
            KnobControl::Slider {
                label,
                min,
                max,
                log,
            },
            KnobValue::F32(value),
        ) => {
            let slider = Slider::new(value, min as f32..=max as f32);
            mixed_slider(ui, slider.logarithmic(log).text(label), mixed, hover)
        }
        (
            KnobControl::Slider {
                label,
                min,
                max,
                log,
            },
            KnobValue::U32(value),
        ) => {
            let slider = Slider::new(value, min as u32..=max as u32);
            mixed_slider(ui, slider.logarithmic(log).text(label), mixed, hover)
        }
        (KnobControl::Choice { label, choices }, KnobValue::Name(value)) => {
            let shown = if mixed {
                "mixed".to_string()
            } else {
                value.clone()
            };
            let combo = egui::ComboBox::new((salt, label), label)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    let mut picked = false;
                    for (name, says) in choices {
                        picked |= ui
                            .selectable_value(value, name.to_string(), *name)
                            .on_hover_text(*says)
                            .clicked();
                    }
                    picked
                });
            combo.response.on_hover_text(hover);
            combo.inner.unwrap_or(false)
        }
        (KnobControl::Check { label }, KnobValue::Bool(value)) => {
            let check = ui.add(egui::Checkbox::new(value, label).indeterminate(mixed));
            let changed = check.changed();
            check.on_hover_text(hover);
            changed
        }
        (control, _) => {
            debug_assert!(
                fits(control, which),
                "{which:?}'s control doesn't fit its value"
            );
            false
        }
    }
}

/// Whether a row's control can turn its value: a slider a number, a choice a
/// name, a checkbox a flag. Every row's does (`tests::every_knob_has_a_control_that_fits`).
pub(super) fn fits(control: KnobControl, which: Knob) -> bool {
    let mut k = LivingKnobs::default();
    matches!(
        (control, k.value_mut(which)),
        (
            KnobControl::Slider { .. },
            KnobValue::F32(_) | KnobValue::U32(_)
        ) | (KnobControl::Choice { .. }, KnobValue::Name(_))
            | (KnobControl::Check { .. }, KnobValue::Bool(_))
    )
}

/// The boil's knobs: on or off, paint, ink, and its cadence.
pub(super) fn boil_ui(ui: &mut egui::Ui, k: &mut LivingKnobs) {
    ui.checkbox(&mut k.boil_on, "Boil");
    ui.add_enabled_ui(k.boil_on, |ui| {
        ui.add(egui::Slider::new(&mut k.boil_paint, 0.0..=1.0).text("paint"));
        ui.add(egui::Slider::new(&mut k.boil_ink, 0.0..=2.0).text("ink"));
        ui.horizontal(|ui| {
            ui.label("on");
            for (cadence, name) in [(1, "ones"), (2, "twos"), (3, "threes")] {
                ui.selectable_value(&mut k.boil_cadence, cadence, name);
            }
        });
    });
}

/// Layers as a mark says them, short enough to sit beside a knob: the report's
/// labels, with two or more zones counted, `global · 3 zones`. The hover has
/// each one.
pub(super) fn short_mark(layers: &[String]) -> String {
    let (zones, mut said): (Vec<&String>, Vec<&String>) =
        layers.iter().partition(|l| l.starts_with("zone:"));
    let counted = format!("{} zones", zones.len());
    match zones.len() {
        0 => {}
        1 => said.push(zones[0]),
        _ => said.push(&counted),
    }
    let said: Vec<&str> = said.iter().map(|s| s.as_str()).collect();
    said.join(" \u{b7} ")
}

/// A knob's mark, after it. Returns whether its clear was pressed.
///
/// At Global it's quiet unless a species sets its own value, and then says how
/// many, since the knob doesn't reach them. At a species it names the layers its
/// value comes from, as `flint render --report json` does, and the species' own
/// gets a clear beside it.
pub(super) fn paint_mark(ui: &mut egui::Ui, mark: &Mark) -> bool {
    let small = |text: String, colour: Color32| egui::RichText::new(text).small().color(colour);
    match mark {
        Mark::Quiet => false,
        Mark::Owned(species) => {
            ui.label(small(format!("{} own", species.len()), PARTS))
                .on_hover_text(format!(
                    "{} {} their own, so this doesn't reach {}. Pick one under Tuning to \
                     see or clear it.",
                    species.join(", "),
                    if species.len() == 1 { "sets" } else { "set" },
                    if species.len() == 1 { "it" } else { "them" },
                ));
            false
        }
        Mark::From(layers) if layers.iter().any(|layer| layer == "species") => {
            ui.label(small("species".to_string(), INK))
                .on_hover_text(layer_says("species"));
            ui.add(egui::Button::new(small("\u{d7}".to_string(), INK)).small())
                .on_hover_text("Clear: its trees take the grove's value again")
                .clicked()
        }
        Mark::From(layers) => {
            let hover: Vec<String> = layers.iter().map(|layer| layer_says(layer)).collect();
            let faint = ui.visuals().weak_text_color();
            ui.label(small(short_mark(layers), faint))
                .on_hover_text(hover.join("\n"));
            false
        }
    }
}
