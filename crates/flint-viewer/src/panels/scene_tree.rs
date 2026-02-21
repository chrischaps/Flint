//! Hierarchical scene tree panel showing all entities

use std::collections::HashSet;

use flint_core::EntityId;
use flint_ecs::FlintWorld;

/// An entry in the scene tree
struct TreeEntry {
    id: EntityId,
    name: String,
    archetype: String,
    parent_id: Option<EntityId>,
}

/// Scene tree panel — shows entities as a collapsible hierarchy
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
                parent_id: world.get_parent(entity.id),
            });
        }

        // Sort by name for consistent ordering within each level
        self.entries.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// Get the currently selected entity
    pub fn selected_entity(&self) -> Option<EntityId> {
        self.selected
    }

    /// Programmatically select an entity (e.g. from viewport picking)
    pub fn select(&mut self, entity_id: Option<EntityId>) {
        self.selected = entity_id;
    }

    /// Find root entries (no parent)
    fn roots(&self) -> Vec<&TreeEntry> {
        self.entries
            .iter()
            .filter(|e| e.parent_id.is_none())
            .collect()
    }

    /// Find children of a given entity
    fn children_of(&self, parent_id: EntityId) -> Vec<&TreeEntry> {
        self.entries
            .iter()
            .filter(|e| e.parent_id == Some(parent_id))
            .collect()
    }

    /// Build the set of entity IDs visible when a filter is active.
    /// Includes matching entities and all their ancestors.
    fn build_visible_set(&self, filter: &str) -> HashSet<EntityId> {
        let mut visible = HashSet::new();

        for entry in &self.entries {
            if entry.name.to_lowercase().contains(filter)
                || entry.archetype.to_lowercase().contains(filter)
            {
                // Add the matching entity
                visible.insert(entry.id);
                // Walk up the parent chain to make ancestors visible
                let mut current_id = entry.parent_id;
                while let Some(pid) = current_id {
                    if !visible.insert(pid) {
                        break; // Already visited this ancestor
                    }
                    current_id = self
                        .entries
                        .iter()
                        .find(|e| e.id == pid)
                        .and_then(|e| e.parent_id);
                }
            }
        }

        visible
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
        let filtering = !filter.is_empty();

        // Build visible set when filtering
        let visible_set = if filtering {
            Some(self.build_visible_set(&filter))
        } else {
            None
        };

        // Count visible entities
        let visible_count = if let Some(ref vs) = visible_set {
            vs.len()
        } else {
            self.entries.len()
        };

        ui.label(format!(
            "{} / {} entities",
            visible_count,
            self.entries.len()
        ));
        ui.separator();

        // Collect data needed for rendering (avoids borrow issues with &mut self)
        let roots: Vec<(EntityId, String, String)> = self
            .roots()
            .iter()
            .filter(|e| visible_set.as_ref().map_or(true, |vs| vs.contains(&e.id)))
            .map(|e| (e.id, e.name.clone(), e.archetype.clone()))
            .collect();

        egui::ScrollArea::vertical().show(ui, |ui| {
            for (id, name, archetype) in &roots {
                self.render_node(ui, *id, name, archetype, &visible_set, filtering);
            }
        });
    }

    /// Recursively render a tree node
    fn render_node(
        &mut self,
        ui: &mut egui::Ui,
        id: EntityId,
        name: &str,
        archetype: &str,
        visible_set: &Option<HashSet<EntityId>>,
        filtering: bool,
    ) {
        let children: Vec<(EntityId, String, String)> = self
            .children_of(id)
            .iter()
            .filter(|e| visible_set.as_ref().map_or(true, |vs| vs.contains(&e.id)))
            .map(|e| (e.id, e.name.clone(), e.archetype.clone()))
            .collect();

        let is_selected = self.selected == Some(id);
        let label = format!("{} ({})", name, archetype);

        if children.is_empty() {
            // Leaf node — render with indent to align with disclosure triangles
            ui.horizontal(|ui| {
                ui.add_space(18.0); // Match CollapsingHeader triangle width
                let response = ui.selectable_label(is_selected, &label);
                if response.clicked() {
                    self.selected = Some(id);
                }
            });
        } else {
            // Branch node — CollapsingHeader with children
            let default_open = if filtering { true } else { true }; // Always open by default
            let header_response = egui::CollapsingHeader::new(
                egui::RichText::new(&label).color(if is_selected {
                    ui.visuals().selection.stroke.color
                } else {
                    ui.visuals().text_color()
                }),
            )
            .id_salt(id.0)
            .default_open(default_open)
            .open(if filtering { Some(true) } else { None }) // Force open when filtering
            .show(ui, |ui| {
                for (child_id, child_name, child_archetype) in &children {
                    self.render_node(
                        ui,
                        *child_id,
                        child_name,
                        child_archetype,
                        visible_set,
                        filtering,
                    );
                }
            });

            // Select on header click
            if header_response.header_response.clicked() {
                self.selected = Some(id);
            }

            // Draw selection highlight on header if selected
            if is_selected {
                let rect = header_response.header_response.rect;
                ui.painter().rect_stroke(
                    rect,
                    2.0,
                    egui::Stroke::new(1.5, ui.visuals().selection.stroke.color),
                );
            }
        }
    }
}
