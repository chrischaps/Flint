//! Hierarchical scene tree panel showing all entities

use flint_core::EntityId;
use flint_ecs::FlintWorld;

/// An entry in the scene tree
struct TreeEntry {
    id: EntityId,
    name: String,
    archetype: String,
}

/// Scene tree panel — shows entities in a scrollable list
#[derive(Default)]
pub struct SceneTree {
    entries: Vec<TreeEntry>,
    selected: Option<EntityId>,
    filter_text: String,
}

impl SceneTree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild the tree from the current world state
    pub fn update(&mut self, world: &FlintWorld) {
        self.entries.clear();

        for entity in world.all_entities() {
            self.entries.push(TreeEntry {
                id: entity.id,
                name: entity.name.clone(),
                archetype: entity.archetype.clone().unwrap_or_else(|| "unknown".to_string()),
            });
        }

        // Sort by name for consistent ordering
        self.entries.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// Get the currently selected entity
    pub fn selected_entity(&self) -> Option<EntityId> {
        self.selected
    }

    /// Draw the scene tree UI
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Scene Tree");
        ui.separator();

        // Search filter
        ui.horizontal(|ui| {
            ui.label("\u{1F50D}");
            ui.text_edit_singleline(&mut self.filter_text);
            if !self.filter_text.is_empty() && ui.small_button("\u{2715}").clicked() {
                self.filter_text.clear();
            }
        });
        ui.separator();

        let filter = self.filter_text.to_lowercase();
        let filtered: Vec<&TreeEntry> = if filter.is_empty() {
            self.entries.iter().collect()
        } else {
            self.entries
                .iter()
                .filter(|e| {
                    e.name.to_lowercase().contains(&filter)
                        || e.archetype.to_lowercase().contains(&filter)
                })
                .collect()
        };

        ui.label(format!(
            "{} / {} entities",
            filtered.len(),
            self.entries.len()
        ));
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            for entry in &filtered {
                let is_selected = self.selected == Some(entry.id);
                let label = format!("{} ({})", entry.name, entry.archetype);

                let response = ui.selectable_label(is_selected, &label);
                if response.clicked() {
                    self.selected = Some(entry.id);
                }
            }
        });
    }
}
