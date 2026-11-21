//! Undo/redo system for scene editing

use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::HashMap;

/// A single field-level edit
#[derive(Debug, Clone)]
pub struct EditAction {
    pub entity_id: EntityId,
    pub component: String,
    pub field: String,
    pub old_value: toml::Value,
    pub new_value: toml::Value,
}

/// A group of edits that form one undoable operation
#[derive(Debug, Clone)]
pub struct UndoCommand {
    pub actions: Vec<EditAction>,
    pub description: String,
}

/// Undo/redo stack with bounded depth
pub struct UndoStack {
    undo: Vec<UndoCommand>,
    redo: Vec<UndoCommand>,
    max_depth: usize,
}

impl UndoStack {
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            max_depth: 100,
        }
    }

    /// Push a new command onto the undo stack (clears redo)
    pub fn push(&mut self, command: UndoCommand) {
        self.undo.push(command);
        self.redo.clear();
        if self.undo.len() > self.max_depth {
            self.undo.remove(0);
        }
    }

    /// Pop the last undo command, returning it for the caller to apply old_values
    pub fn undo(&mut self) -> Option<UndoCommand> {
        let cmd = self.undo.pop()?;
        self.redo.push(cmd.clone());
        Some(cmd)
    }

    /// Pop the last redo command, returning it for the caller to apply new_values
    pub fn redo(&mut self) -> Option<UndoCommand> {
        let cmd = self.redo.pop()?;
        self.undo.push(cmd.clone());
        Some(cmd)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_description(&self) -> Option<&str> {
        self.undo.last().map(|c| c.description.as_str())
    }

    pub fn redo_description(&self) -> Option<&str> {
        self.redo.last().map(|c| c.description.as_str())
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// The file was read anew (#1046): carry each step from `old`, the world
    /// it was made in, to `new`, the reloaded one. A reload spawns every
    /// entity under a new id, so an action finds its entity again by name.
    ///
    /// A step stays only while it still says what is on screen: walking the
    /// undo steps from the newest, an action is kept when its field in `new`
    /// holds the value the step left there (then the value before it is what
    /// the next older step on that field must have left), and the redo steps
    /// the same way from the next one to redo. So an agent's change to a
    /// field the human edited ends that field's history (undo never throws
    /// the agent's value away), and a field the agent didn't touch keeps its
    /// whole history. An action whose entity or field is gone goes too, and
    /// a step with no action left. Returns how many steps were dropped.
    pub fn carry_over(&mut self, old: &FlintWorld, new: &FlintWorld) -> usize {
        let before = self.undo.len() + self.redo.len();
        carry(&mut self.undo, old, new, Side::Undo);
        carry(&mut self.redo, old, new, Side::Redo);
        before - self.undo.len() - self.redo.len()
    }
}

#[derive(Clone, Copy)]
enum Side {
    Undo,
    Redo,
}

/// [`UndoStack::carry_over`] for one stack, its top (next to apply) last.
fn carry(stack: &mut Vec<UndoCommand>, old: &FlintWorld, new: &FlintWorld, side: Side) {
    // (entity name, component, field) -> what the world holds there once
    // every newer step on the stack is applied; `None`: the history is broken
    let mut expected: HashMap<(String, String, String), Option<toml::Value>> = HashMap::new();
    for cmd in stack.iter_mut().rev() {
        cmd.actions.retain_mut(|action| {
            let Some(name) = old.get_name(action.entity_id) else {
                return false;
            };
            let Some(id) = new.get_id(name) else {
                return false;
            };
            let key = (
                name.to_string(),
                action.component.clone(),
                action.field.clone(),
            );
            let there = expected.entry(key).or_insert_with(|| {
                new.get_components(id)
                    .and_then(|c| c.get_field(&action.component, &action.field))
                    .cloned()
            });
            let (left, before) = match side {
                Side::Undo => (&action.new_value, &action.old_value),
                Side::Redo => (&action.old_value, &action.new_value),
            };
            let holds = there
                .as_ref()
                .is_some_and(|v| flint_core::toml_util::reads_same(v, left));
            *there = holds.then(|| before.clone());
            action.entity_id = id;
            holds
        });
    }
    stack.retain(|cmd| !cmd.actions.is_empty());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A world as a load makes it: `crate` at x, `lamp` at intensity
    fn world(x: f64, intensity: f64) -> FlintWorld {
        let mut world = FlintWorld::new();
        let id = world.spawn("crate").unwrap();
        world.get_components_mut(id).unwrap().set_field(
            "transform",
            "position",
            toml::Value::Array(vec![x.into(), 0.0.into(), 0.0.into()]),
        );
        let lamp = world.spawn("lamp").unwrap();
        world
            .get_components_mut(lamp)
            .unwrap()
            .set_field("light", "intensity", intensity.into());
        world
    }

    fn at(x: f64) -> toml::Value {
        toml::Value::Array(vec![x.into(), 0.0.into(), 0.0.into()])
    }

    fn step(
        world: &FlintWorld,
        name: &str,
        c: &str,
        f: &str,
        old: toml::Value,
        new: toml::Value,
    ) -> UndoCommand {
        UndoCommand {
            actions: vec![EditAction {
                entity_id: world.get_id(name).unwrap(),
                component: c.into(),
                field: f.into(),
                old_value: old,
                new_value: new,
            }],
            description: format!("Edit {c}.{f}"),
        }
    }

    /// The new world's ids are past the old one's, as a reload's are (#688)
    fn reloaded(old: &FlintWorld, x: f64, intensity: f64) -> FlintWorld {
        let mut new = FlintWorld::after(old);
        let id = new.spawn("crate").unwrap();
        new.get_components_mut(id)
            .unwrap()
            .set_field("transform", "position", at(x));
        let lamp = new.spawn("lamp").unwrap();
        new.get_components_mut(lamp)
            .unwrap()
            .set_field("light", "intensity", intensity.into());
        new
    }

    /// The human moved the crate 0 -> 1 -> 2 and saved; an agent then
    /// changed the lamp in the file. Both moves still undo, on the new ids
    #[test]
    fn an_agents_edit_elsewhere_keeps_the_humans_history() {
        let old = world(2.0, 1.0);
        let mut stack = UndoStack::new();
        stack.push(step(
            &old,
            "crate",
            "transform",
            "position",
            at(0.0),
            at(1.0),
        ));
        stack.push(step(
            &old,
            "crate",
            "transform",
            "position",
            at(1.0),
            at(2.0),
        ));
        let new = reloaded(&old, 2.0, 5.0);
        assert_eq!(stack.carry_over(&old, &new), 0);
        let top = stack.undo().unwrap();
        assert_eq!(top.actions[0].entity_id, new.get_id("crate").unwrap());
        assert_eq!(top.actions[0].old_value, at(1.0));
        assert_eq!(stack.undo().unwrap().actions[0].old_value, at(0.0));
        assert!(!stack.can_undo());
    }

    /// The agent moved the crate the human moved: undo can't put back a
    /// value the agent replaced, so that history ends; the lamp's stays
    #[test]
    fn an_agents_edit_to_the_same_field_ends_its_history() {
        let old = world(1.0, 3.0);
        let mut stack = UndoStack::new();
        stack.push(step(
            &old,
            "lamp",
            "light",
            "intensity",
            1.0.into(),
            3.0.into(),
        ));
        stack.push(step(
            &old,
            "crate",
            "transform",
            "position",
            at(0.0),
            at(1.0),
        ));
        let new = reloaded(&old, 7.0, 3.0);
        assert_eq!(stack.carry_over(&old, &new), 1);
        let top = stack.undo().unwrap();
        assert_eq!(top.actions[0].field, "intensity");
        assert_eq!(top.actions[0].entity_id, new.get_id("lamp").unwrap());
        assert!(!stack.can_undo());
    }

    /// An entity the agent deleted takes its steps with it; an integer the
    /// file says for a float, or a float read back a hair off, still holds
    #[test]
    fn a_gone_entity_drops_and_numbers_compare_as_the_file_reads_them() {
        let old = world(1.0, 3.0);
        let mut stack = UndoStack::new();
        stack.push(step(
            &old,
            "lamp",
            "light",
            "intensity",
            1.0.into(),
            3.0.into(),
        ));
        stack.push(step(
            &old,
            "crate",
            "transform",
            "position",
            at(0.0),
            at(1.000_000_01),
        ));
        let mut new = FlintWorld::after(&old);
        let id = new.spawn("crate").unwrap();
        new.get_components_mut(id).unwrap().set_field(
            "transform",
            "position",
            toml::Value::Array(vec![1.into(), 0.into(), 0.into()]),
        );
        assert_eq!(stack.carry_over(&old, &new), 1);
        assert_eq!(stack.undo().unwrap().actions[0].field, "position");
        assert!(!stack.can_undo());
    }

    /// A step undone before the reload is still there to redo while the
    /// file says what the undo left
    #[test]
    fn redo_survives_a_reload_too() {
        let old = world(0.0, 1.0);
        let mut stack = UndoStack::new();
        stack.push(step(
            &old,
            "crate",
            "transform",
            "position",
            at(0.0),
            at(1.0),
        ));
        stack.undo();
        let new = reloaded(&old, 0.0, 9.0);
        assert_eq!(stack.carry_over(&old, &new), 0);
        let redo = stack.redo().unwrap();
        assert_eq!(redo.actions[0].entity_id, new.get_id("crate").unwrap());
        assert_eq!(redo.actions[0].new_value, at(1.0));
    }
}
