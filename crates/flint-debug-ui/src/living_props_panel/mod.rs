//! F3 "Living Props" panel (#218): every knob of a grove, live, with the warped
//! clock drawn as the curve it is, freeze and single-step, a pose overlay in the
//! world, and presets saved as TOML a scene's `[wooze]` table can name.
//!
//! The panel holds plain data; the host (`flint-player`) mirrors what's true into
//! it each frame while it's showing and drains what it asked for, so this crate
//! never depends on `flint-procgen`.

mod clock;
mod knobs;
mod overlay;
mod state;
mod ui;

use crate::DebugPanel;
use egui::Color32;
use ui::Mark;

pub use knobs::{
    Knob, KnobControl, KnobDesc, KnobFlags, KnobPanel, KnobSection, KnobSources, KnobValue,
    LivingKnobs, KNOB_TABLE,
};
pub use overlay::pick_prop;
pub use state::{
    LivingAbState, LivingHealth, LivingPanelAction, LivingPanelState, LivingPickedState,
    LivingPropRow, LivingSpeciesState, PanelScope,
};

pub const LIVING_PROPS_DEBUG_PANEL: &str = "Living Props";

/// The curve's colour, and the overlay's.
const INK: Color32 = Color32::from_rgb(232, 180, 90);
/// The parts behind the main dot, one per branch level.
const PARTS: Color32 = Color32::from_rgb(150, 120, 74);
const FROZEN: Color32 = Color32::from_rgb(132, 190, 232);

pub struct LivingPropsDebugPanel {
    open: bool,
    dirty: bool,
    state: LivingPanelState,
    /// Which row the clock readout follows.
    focus: usize,
    /// The preset the combo has picked.
    picked: String,
    /// The name to save under.
    save_as: String,
    /// What A and B are picked to be, before a comparison starts. `None` until
    /// the presets on offer are known.
    ab_picks: Option<(String, String)>,
    /// Whose values the knobs turn (#247).
    scope: PanelScope,
    /// What the Props rows are narrowed to: part of a name or a species.
    filter: String,
    /// The tree picked in the world, by its entity's name (#248).
    selected: Option<String>,
    /// Whether the picked tree was on screen when the overlay last painted.
    picked_on_screen: Option<bool>,
    /// Where the panel's own column started last frame, so the picked tree's
    /// window keeps out from under it.
    panel_left: Option<f32>,
    actions: Vec<LivingPanelAction>,
}

impl Default for LivingPropsDebugPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl LivingPropsDebugPanel {
    pub fn new() -> Self {
        Self {
            open: false,
            dirty: false,
            state: LivingPanelState::default(),
            focus: 0,
            picked: String::new(),
            save_as: "grove".to_string(),
            ab_picks: None,
            scope: PanelScope::Global,
            filter: String::new(),
            selected: None,
            picked_on_screen: None,
            panel_left: None,
            actions: Vec::new(),
        }
    }

    /// Live mirror: the host calls this each frame the panel or overlay is showing.
    /// A species that's gone, as after a scene change, gives the grove back.
    pub fn refresh(&mut self, state: LivingPanelState) {
        self.state = state;
        if matches!(self.scope, PanelScope::Species(_)) && self.picked_species().is_none() {
            self.scope = PanelScope::Global;
        }
        if let Some(name) = &self.selected {
            if !self.state.rows.iter().any(|row| &row.name == name) {
                self.selected = None;
                self.picked_on_screen = None;
            }
        }
    }

    /// Pick a tree by its entity's name, as a click on it in the world does
    /// (#248): the panel turns to its species, and its readout follows it. A
    /// name no row has, or `None`, lets go. Returns whether one is picked.
    pub fn select(&mut self, name: Option<&str>) -> bool {
        let found = name.and_then(|name| {
            (self.state.rows.iter().enumerate()).find(|(_, row)| row.name == name)
        });
        let Some((i, row)) = found else {
            self.selected = None;
            self.picked_on_screen = None;
            return false;
        };
        self.scope = PanelScope::Species(row.species.clone());
        self.selected = Some(row.name.clone());
        self.focus = i;
        true
    }

    /// The tree picked in the world, by name, if one is.
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// Whether the picked tree was on screen when the overlay last painted:
    /// `None` with nothing picked or before the first paint.
    pub fn picked_on_screen(&self) -> Option<bool> {
        self.selected.as_ref().and(self.picked_on_screen)
    }

    /// Pick the next tree by name, or the one before with `forward` false, as
    /// the cycle key does when a click missed. With none picked, the first.
    pub fn select_next(&mut self, forward: bool) -> bool {
        let mut names: Vec<&str> = self.state.rows.iter().map(|r| r.name.as_str()).collect();
        if names.is_empty() {
            return false;
        }
        names.sort_unstable();
        let at = (self.selected.as_deref()).and_then(|name| names.iter().position(|n| *n == name));
        let n = names.len();
        let next = match (at, forward) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
        };
        let name = names[next].to_string();
        self.select(Some(&name))
    }

    /// The picked tree's row, if one is picked and the host knows it.
    fn picked_row(&self) -> Option<&LivingPropRow> {
        let name = self.selected.as_ref()?;
        self.state.rows.iter().find(|row| &row.name == name)
    }

    /// Whose values the knobs turn.
    pub fn scope(&self) -> &PanelScope {
        &self.scope
    }

    /// Turn the grove's knobs or one species', as a click on the selector would.
    pub fn set_scope(&mut self, scope: PanelScope) {
        self.scope = scope;
    }

    /// The species picked, if one is and the host knows it.
    fn picked_species(&self) -> Option<&LivingSpeciesState> {
        match &self.scope {
            PanelScope::Global => None,
            PanelScope::Species(name) => self.state.species.iter().find(|s| &s.name == name),
        }
    }

    /// What a knob's mark says, for the scope picked.
    fn mark(&self, knob: Knob) -> Mark {
        match self.picked_species() {
            Some(species) => Mark::From(species.sources.of(knob).to_vec()),
            None => {
                let owned: Vec<String> = (self.state.species.iter())
                    .filter(|s| s.sources.is_own(knob))
                    .map(|s| s.name.clone())
                    .collect();
                if owned.is_empty() {
                    Mark::Quiet
                } else {
                    Mark::Owned(owned)
                }
            }
        }
    }

    /// What's been asked for since the last drain.
    pub fn take_actions(&mut self) -> Vec<LivingPanelAction> {
        std::mem::take(&mut self.actions)
    }

    /// What the panel last heard from the host.
    pub fn state(&self) -> &LivingPanelState {
        &self.state
    }

    /// Queue an action as a click would, for tests that drive the host.
    pub fn send_for_test(&mut self, action: LivingPanelAction) {
        self.push(action);
    }

    fn push(&mut self, action: LivingPanelAction) {
        self.actions.push(action);
        self.dirty = true;
    }

    /// The row the readout follows, if the scene has any: one of the picked
    /// species', if one is.
    fn focused(&self) -> Option<&LivingPropRow> {
        if let Some(row) = self.picked_row() {
            return Some(row);
        }
        let rows = &self.state.rows;
        let row = rows.get(self.focus).or_else(|| rows.first());
        match &self.scope {
            PanelScope::Species(name) if row.is_some_and(|row| &row.species != name) => {
                rows.iter().find(|row| &row.species == name).or(row)
            }
            _ => row,
        }
    }

    /// Whether the overlay labels this row: every row, or the picked species'.
    fn labelled(&self, row: &LivingPropRow) -> bool {
        match &self.scope {
            PanelScope::Global => true,
            PanelScope::Species(name) => &row.species == name,
        }
    }
}

impl DebugPanel for LivingPropsDebugPanel {
    fn name(&self) -> &str {
        LIVING_PROPS_DEBUG_PANEL
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.panel_left = Some(ui.max_rect().left());
        self.picked_ui(ui);
        self.clock_section(ui);
        ui.separator();
        self.scope_ui(ui);
        self.knobs_ui(ui);
        self.presets_ui(ui);
        self.ab_ui(ui);
        self.rows_ui(ui);
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

#[cfg(test)]
mod tests;
