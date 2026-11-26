//! F3 "Particles" panel: live alive counts per effect instance, play / stop /
//! burst controls, the global budget and a draw toggle (ADR 0068).
//!
//! The panel holds plain data; its `sync` refreshes the rows each frame
//! while the panel is clean and applies `take_actions()` to the particles
//! when it is dirty.

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

    fn weight(&self) -> u32 {
        14
    }

    /// Apply what was pressed; while nothing was, mirror the live counts.
    fn sync(&mut self, ctx: &mut crate::PanelCtx) {
        let sync = &mut *ctx.particles;
        if self.dirty {
            for action in self.take_actions() {
                use ParticlePanelAction as A;
                match action {
                    A::Play(id) => sync.set_playing(id, true),
                    A::Stop(id) => sync.set_playing(id, false),
                    A::Burst(id, n) => sync.queue_burst(id, n),
                    A::StopDetached(h) => sync.stop_effect(h),
                    A::RestartAll => sync.restart_all(),
                    A::SetBudget(b) => sync.set_budget(b as usize),
                    A::SetRender(on) => ctx.host.particles_render = on,
                }
            }
            self.dirty = false;
            return;
        }
        let rows = sync
            .instances()
            .map(|(key, inst)| {
                let (entity_id, handle, label) = match key {
                    flint_particles::InstanceKey::Entity(id) => (
                        Some(id),
                        0,
                        ctx.world.get_name(id).unwrap_or("?").to_string(),
                    ),
                    flint_particles::InstanceKey::Detached(h) => (None, h, format!("effect #{h}")),
                };
                let effect = match &inst.source {
                    flint_particles::EffectSource::Asset(fx) => Some(fx.name.clone()),
                    flint_particles::EffectSource::Inline => None,
                };
                EmitterRow {
                    entity_id,
                    handle,
                    label,
                    effect,
                    emitters: inst.emitters.len(),
                    alive: inst.alive(),
                    capacity: inst.capacity(),
                    playing: inst.any_emitter_playing(),
                }
            })
            .collect();
        let alive = sync.total_alive();
        let budget = sync.budget() as u32;
        self.refresh(rows, alive, budget);
        self.render_enabled = ctx.host.particles_render;
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
