//! A sink that writes down what it was asked, in order, for tests of the
//! code that feeds a sink: procgen's own resolution order, and the living
//! props' load in the player, which hands their lineages to the same sink.
//! No renderer stands behind it.

use super::AssetSink;
use crate::{AssetKind, ImageData, LivingRig, MeshData};
use flint_core::EntityId;
use std::collections::{HashMap, HashSet};

/// What the renderer was asked to do, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Mesh(String, usize),
    Image(String),
    Placeholder(String, AssetKind),
    File(String, AssetKind),
    Living(String),
    Bones(EntityId, String, usize),
}

/// An [`AssetSink`] that records each call as an [`Event`].
#[derive(Default)]
pub struct Recorder {
    /// Each uploaded mesh's vertex count, by name (0 for a placeholder).
    pub meshes: HashMap<String, usize>,
    files: HashSet<(String, bool)>,
    pub events: Vec<Event>,
}

impl Recorder {
    /// A file the file loader would find for `name`.
    pub fn with_file(mut self, name: &str, kind: AssetKind) -> Self {
        self.files.insert((name.into(), kind == AssetKind::Model));
        self
    }
}

impl AssetSink for Recorder {
    fn has_mesh(&self, name: &str) -> bool {
        self.meshes.contains_key(name)
    }
    fn has_file(&self, name: &str, kind: AssetKind) -> bool {
        self.files
            .contains(&(name.to_string(), kind == AssetKind::Model))
    }
    fn upload_mesh(&mut self, name: &str, mesh: &MeshData) {
        self.meshes.insert(name.into(), mesh.vertices.len());
        self.events
            .push(Event::Mesh(name.into(), mesh.vertices.len()));
    }
    fn upload_living(&mut self, name: &str, rig: &LivingRig) {
        self.meshes.insert(name.into(), rig.mesh.vertices.len());
        self.events.push(Event::Living(name.into()));
    }
    fn upload_bones(&mut self, entity: EntityId, name: &str, palette: &[[[f32; 4]; 4]]) {
        self.events
            .push(Event::Bones(entity, name.into(), palette.len()));
    }
    fn upload_image(&mut self, name: &str, _image: &ImageData) {
        self.events.push(Event::Image(name.into()));
    }
    fn upload_placeholder(&mut self, name: &str, kind: AssetKind) {
        if kind == AssetKind::Model {
            self.meshes.insert(name.into(), 0);
        }
        self.events.push(Event::Placeholder(name.into(), kind));
    }
    fn load_file(&mut self, name: &str, kind: AssetKind) -> bool {
        self.events.push(Event::File(name.into(), kind));
        true
    }
}
