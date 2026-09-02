//! Undo/redo system for scene editing

use flint_core::EntityId;

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
}
