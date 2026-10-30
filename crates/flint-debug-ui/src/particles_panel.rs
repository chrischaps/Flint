//! F3 "Particles" panel: live alive counts per effect instance, play / stop /
//! burst controls, the global budget and a draw toggle (ADR 0068).
//!
//! The panel holds plain data; the host (`flint-player`) refreshes the rows
//! each frame while the panel is clean and drains `take_actions()` when it
//! is dirty, so this crate never depends on `flint-particles`.

use crate::widgets::drag_u32;
use crate::DebugPanel;
use flint_core::EntityId;

pub const PARTICLES_DEBUG_PANEL: &str = "Particles";

/// One live effect instance as shown in the panel.
#[derive(Clone, Debug)]
pub struct EmitterRow {
    /// `Some` for entity-bound instances, `None` for detached `play_effect` ones.
    pub entity_id: Option<EntityId>,
    /// Detached handle when `entity_id` is `None`.
    pub handle: u64,
    /// Entity name or `effect #handle`.
    pub label: String,
    /// Effect asset name, or `None` for an inline `particle_emitter`.
    pub effect: Option<String>,
    pub emitters: usize,
    pub alive: usize,
    pub capacity: usize,
    pub playing: bool,
}

/// What the user asked for; applied by the host.
#[derive(Clone, Debug, PartialEq)]
pub enum ParticlePanelAction {
    Play(EntityId),
    Stop(EntityId),
    Burst(EntityId, u32),
    StopDetached(u64),
    RestartAll,
    SetBudget(u32),
    SetRender(bool),
}

pub struct ParticlesDebugPanel {
    open: bool,
    dirty: bool,
    pub rows: Vec<EmitterRow>,
    pub total_alive: usize,
    pub budget: u32,
    pub render_enabled: bool,
    burst_count: u32,
    actions: Vec<ParticlePanelAction>,
}

impl Default for ParticlesDebugPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl ParticlesDebugPanel {
    pub fn new() -> Self {
        Self {
            open: false,
            dirty: false,
            rows: Vec::new(),
            total_alive: 0,
            budget: 100_000,
            render_enabled: true,
            burst_count: 25,
            actions: Vec::new(),
        }
    }

    /// Live mirror: the host calls this every frame while the panel is clean.
    pub fn refresh(&mut self, rows: Vec<EmitterRow>, total_alive: usize, budget: u32) {
        self.rows = rows;
        self.total_alive = total_alive;
        self.budget = budget;
    }

    /// Drain queued actions (clears the dirty flag's cause).
    pub fn take_actions(&mut self) -> Vec<ParticlePanelAction> {
        std::mem::take(&mut self.actions)
    }

    fn push(&mut self, a: ParticlePanelAction) {
        self.actions.push(a);
        self.dirty = true;
    }
}

impl DebugPanel for ParticlesDebugPanel {
    fn name(&self) -> &str {
        PARTICLES_DEBUG_PANEL
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let emitters: usize = self.rows.iter().map(|r| r.emitters).sum();
        ui.label(format!(
            "alive {}  ·  {} instance{}  ·  {} emitter{}",
            self.total_alive,
            self.rows.len(),
            if self.rows.len() == 1 { "" } else { "s" },
            emitters,
            if emitters == 1 { "" } else { "s" },
        ));

        let mut render = self.render_enabled;
        if ui.checkbox(&mut render, "Draw particles").changed() {
            self.render_enabled = render;
            self.push(ParticlePanelAction::SetRender(render));
        }

        let mut budget = self.budget;
        if drag_u32(ui, "Budget", &mut budget, 100..=1_000_000) {
            self.budget = budget;
            self.push(ParticlePanelAction::SetBudget(budget));
        }

        ui.horizontal(|ui| {
            if ui.button("Restart all").clicked() {
                self.actions.push(ParticlePanelAction::RestartAll);
                self.dirty = true;
            }
            ui.label("Burst");
            ui.add(egui::DragValue::new(&mut self.burst_count).range(1..=1000));
        });

        ui.separator();

        let rows = self.rows.clone();
        let burst_count = self.burst_count;
        egui::Grid::new("particles_rows")
            .num_columns(3)
            .striped(true)
            .show(ui, |ui| {
                for row in &rows {
                    let mut label = row.label.clone();
                    if let Some(fx) = &row.effect {
                        label.push_str(&format!("  ({fx})"));
                    }
                    ui.label(label);
                    ui.label(format!("{}/{}", row.alive, row.capacity));
                    ui.horizontal(|ui| match row.entity_id {
                        Some(id) => {
                            if row.playing {
                                if ui.small_button("stop").on_hover_text("Stop").clicked() {
                                    self.push(ParticlePanelAction::Stop(id));
                                }
                            } else if ui.small_button("play").on_hover_text("Play").clicked() {
                                self.push(ParticlePanelAction::Play(id));
                            }
                            if ui
                                .small_button("burst")
                                .on_hover_text(format!("Emit {burst_count} now"))
                                .clicked()
                            {
                                self.push(ParticlePanelAction::Burst(id, burst_count));
                            }
                        }
                        None => {
                            if ui.small_button("stop").on_hover_text("Stop").clicked() {
                                self.push(ParticlePanelAction::StopDetached(row.handle));
                            }
                        }
                    });
                    ui.end_row();
                }
            });

        if rows.is_empty() {
            ui.weak("No particle_emitter or particle_effect entities in this scene.");
        }
        ui.add_space(4.0);
        ui.weak("Edit effects: flint edit particles/<name>.particles.toml");
    }

    fn is_open(&self) -> bool {
        self.open
    }

    fn toggle(&mut self) {
        self.open = !self.open;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
