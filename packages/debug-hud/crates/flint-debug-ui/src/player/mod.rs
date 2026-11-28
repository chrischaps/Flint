//! The debug-hud package's part of the player (#1559, ADR 0137): the
//! [`DebugHud`] its launcher registers, which makes the engine's F3/F4
//! panels at each scene, syncs them each frame, toggles them on F3 and F4,
//! lays them out over the game, paints the corner cards and draws F2's
//! frame-time graph. flint-player names none of it: its `debug-hud` feature
//! turns this crate's `player` feature on, and its build script lists
//! [`PACKAGE`] from the manifest's `[rust] player`.

mod cards;
mod layout;

use crate::widgets::{band_legend, hitch_counters, FrameGraph};
use crate::{
    CameraDebugPanel, CameraPanelConfig, ComponentPanel, GrassDebugPanel, OceanDebugPanel,
    OceanPanelConfig, RenderDebugPanel, StylesDebugPanel, RENDER_DEBUG_PANEL,
};
use flint_core::world_line::WorldLine;
use flint_package::hud::{Cards, DebugHud, GraphView, HudScene, HudUi, Toggle, WorldView};
use flint_package::panels::{DebugPanel, DebugPanels, PanelCtx};
use flint_package::{Package, Registry};
use flint_render::stylize::StyleSettings;

pub use layout::show_debug_panels;

/// The component a scene's camera tuning panel edits.
const CAMERA_TUNING: &str = "camera_tuning";

/// What the launcher lists: the package registers its HUD.
pub static PACKAGE: DebugHudPackage = DebugHudPackage;

/// The debug-hud package.
pub struct DebugHudPackage;

impl Package for DebugHudPackage {
    fn name(&self) -> &'static str {
        "debug-hud"
    }

    fn register(&self, registry: &mut Registry) {
        registry.debug_hud(|| Box::<Hud>::default());
    }
}

/// The run's debug HUD. The panels are the run's [`DebugPanels`]; what is
/// its own is the frame graph's widget, which keeps its ceiling and mesh
/// from frame to frame.
#[derive(Default)]
pub struct Hud {
    graph: FrameGraph,
}

impl DebugHud for Hud {
    fn scene_panels(&mut self, panels: &mut DebugPanels, mut scene: HudScene) {
        // The last scene's go (grass, ocean, particles, styles...);
        // Rendering & Effects (F4) and the game's own belong to the player
        // and are kept, open or closed as they were (#268)
        panels.retain(|p| p.outlives_scene());
        if let Some(grass) = scene.grass.take() {
            let panel = GrassDebugPanel::new(
                grass.config,
                std::path::PathBuf::from(scene.scene_path),
                grass.entity,
            )
            .with_component(grass.component);
            panels.push(Box::new(panel));
        }
        panels.extend(ocean_panel(&scene));
        // A game's controllers, from their schemas (#744)
        component_panels(panels, &scene);
        panels.extend(camera_panel(&scene));
        // The packages' own: the particles package's Particles panel
        // (#1622), where debug-hud made it before, then the Living Props
        // panel (#218)
        panels.extend(std::mem::take(&mut scene.packages));
        // The F3 Styles panel (#268) when the scene uses a style: a
        // `[post_process.styles]` table, or an entity with a `style_mask`
        let has_table = scene.authored.iter().next().is_some();
        let has_mask = !scene
            .world
            .entities_with_component(flint_render::style_mask::STYLE_MASK_COMPONENT)
            .is_empty();
        if has_table || has_mask {
            let panel = StylesDebugPanel::new(Some(scene.authored.clone()))
                .with_asset_scope(Some(scene.assets.clone()))
                .with_scene_file(scene.scene_file.clone());
            panels.push(Box::new(panel));
        }
        // Rendering & Effects (ADR 0053): every scene has a renderer to
        // tune, so it is unconditional; registered closed, F4 summons it.
        // The one that survived a transition is pointed at the new scene.
        let existing = panels
            .iter_mut()
            .filter(|p| p.name() == RENDER_DEBUG_PANEL)
            .find_map(|p| p.as_any_mut().downcast_mut::<RenderDebugPanel>());
        let fresh = existing.is_none();
        let mut made = None;
        let panel = match existing {
            Some(panel) => panel,
            // `show_freeze`: player scripts drive post fields
            None => made.insert(RenderDebugPanel::new(true)),
        };
        panel.set_authored_styles(Some(scene.authored));
        panel.set_asset_scope(Some(scene.assets));
        panel.set_looks(&scene.looks, scene.current_look);
        panel.set_scene_file(scene.scene_file);
        if fresh {
            panel.offer_look_fade(true);
        }
        if let Some(panel) = made {
            panels.push(Box::new(panel));
        }
    }

    fn look_changed(
        &mut self,
        panels: &mut DebugPanels,
        authored: &StyleSettings,
        current: Option<&str>,
    ) {
        for panel in panels.iter_mut() {
            let any = panel.as_any_mut();
            if let Some(rp) = any.downcast_mut::<RenderDebugPanel>() {
                rp.set_authored_styles(Some(authored.clone()));
                rp.set_current_look(current);
            } else if let Some(sp) = any.downcast_mut::<StylesDebugPanel>() {
                sp.set_authored(Some(authored.clone()));
            }
        }
    }

    fn sync(&mut self, panels: &mut DebugPanels, ctx: &mut PanelCtx) {
        for panel in panels.iter_mut() {
            panel.sync(ctx);
        }
        // A style the F4 or F3 editor just wrote into the scene file
        // (#364): both editors get the scene's authored settings after it,
        // so neither marks the committed values as edits
        if let Some(authored) = &ctx.host.committed_styles {
            for panel in panels.iter_mut() {
                let any = panel.as_any_mut();
                if let Some(rp) = any.downcast_mut::<RenderDebugPanel>() {
                    rp.set_authored_styles(Some(authored.clone()));
                } else if let Some(sp) = any.downcast_mut::<StylesDebugPanel>() {
                    sp.set_authored_styles(Some(authored.clone()));
                }
            }
        }
    }

    fn toggle(&mut self, panels: &mut DebugPanels, which: Toggle) -> Option<bool> {
        let mut toggled = None;
        for panel in panels.iter_mut() {
            // F3 is the scene's panels (grass, ocean, ...), not Rendering &
            // Effects, which has its own key and would otherwise flip out
            // of phase with it
            let render = panel.name() == RENDER_DEBUG_PANEL;
            if render == (which == Toggle::Render) {
                panel.toggle();
                toggled = Some(toggled.unwrap_or(false) || panel.is_open());
            }
        }
        toggled
    }

    fn world_lines(
        &mut self,
        panels: &mut DebugPanels,
        view: &WorldView,
        out: &mut Vec<WorldLine>,
    ) {
        // The Ocean Debug panel's CPU/GPU parity probe, when its box is
        // ticked (#749): a cross pinned to the CPU's wave height ahead of
        // the camera, on the clock the renderer's waves run on
        for panel in panels.iter_mut() {
            let Some(ocean_panel) = panel.as_any_mut().downcast_mut::<OceanDebugPanel>() else {
                continue;
            };
            if !ocean_panel.probe_shown() {
                continue;
            }
            let Some(id) = view.world.get_id(ocean_panel.entity_name()) else {
                continue;
            };
            let ocean = view.world.view(id, flint_geometry::components::OCEAN);
            if !ocean.exists() {
                continue;
            }
            out.extend(ocean_panel.probe_lines(&ocean, view.time, view.eye, view.forward));
        }
    }

    fn ui(&mut self, ui: HudUi) -> f32 {
        // What panels paint in the world, under every panel: the Living
        // Props pose overlay (#218) and a picked tree's marks and window
        // (#248)
        for panel in ui.panels.iter_mut() {
            panel.paint_world(ui.ctx, &ui.view_proj);
        }
        show_debug_panels(ui.ctx, ui.panels, ui.draw)
    }

    fn cards(&mut self, ctx: &egui::Context, cards: &Cards) {
        cards::paint(ctx, cards);
    }

    fn graph(&mut self, ui: &mut egui::Ui, graph: &GraphView) {
        self.graph.show(ui, graph.frames, graph.layout);
        ui.add_space(3.0);
        ui.scope(|ui| {
            let size = if graph.full { 10.0 } else { 9.5 };
            ui.style_mut().override_font_id = Some(egui::FontId::monospace(size));
            ui.set_max_width(graph.layout.size.x);
            hitch_counters(ui, &graph.counters, !graph.full);
            if graph.full {
                ui.style_mut().override_font_id = Some(egui::FontId::monospace(8.5));
                band_legend(ui);
            }
        });
    }
}

/// The ocean tuning panel, if the scene has an `ocean` component.
fn ocean_panel(scene: &HudScene) -> Option<Box<dyn DebugPanel>> {
    let world = scene.world;
    let &id = world
        .entities_with_component(flint_geometry::components::OCEAN)
        .iter()
        .next()?;
    let name = world.get_name(id)?.to_string();
    let ocean = world.view(id, flint_geometry::components::OCEAN);
    if !ocean.exists() {
        return None;
    }
    let config = OceanPanelConfig::from_component(&ocean);
    Some(Box::new(OceanDebugPanel::new(
        config,
        std::path::PathBuf::from(scene.scene_path),
        name,
    )))
}

/// The camera tuning panel, if the scene has a `camera_tuning` component.
fn camera_panel(scene: &HudScene) -> Option<Box<dyn DebugPanel>> {
    let world = scene.world;
    let &id = world.entities_with_component(CAMERA_TUNING).iter().next()?;
    let name = world.get_name(id)?.to_string();
    let tuning = world.view(id, CAMERA_TUNING);
    if !tuning.exists() {
        return None;
    }
    let config = CameraPanelConfig::from_component(&tuning);
    Some(Box::new(CameraDebugPanel::new(
        config,
        std::path::PathBuf::from(scene.scene_path),
        name,
    )))
}

/// A panel for each entity that carries a component whose schema puts
/// fields on one (#744): `panel = "status" | "tune" | "trigger"` in the
/// scene's own schema directories, so a game's script-driven controller
/// gets F3 controls with no Rust. In the order of the panels' titles, then
/// the entities' names; with several entities, each title names its entity.
fn component_panels(panels: &mut DebugPanels, scene: &HudScene) {
    let world = scene.world;
    let mut marked: Vec<_> = scene
        .schemas
        .components()
        .into_iter()
        .filter_map(|schema| Some((schema.name.as_str(), schema.panel.as_ref()?)))
        .collect();
    marked.sort_by(|a, b| a.1.title.cmp(&b.1.title).then(a.0.cmp(b.0)));
    for (component, spec) in marked {
        let mut entities: Vec<(String, Option<toml::Value>)> = world
            .entities_with_component(component)
            .iter()
            .filter_map(|&id| {
                let name = world.get_name(id)?.to_string();
                let table = world
                    .get_components(id)
                    .and_then(|c| c.get(component))
                    .cloned();
                Some((name, table))
            })
            .collect();
        entities.sort_by(|a, b| a.0.cmp(&b.0));
        let several = entities.len() > 1;
        for (entity, table) in entities {
            let title = if several {
                format!("{} · {entity}", spec.title)
            } else {
                spec.title.clone()
            };
            panels.push(Box::new(ComponentPanel::new(
                spec.clone(),
                component,
                &entity,
                title,
                table.as_ref(),
                scene.scene_file.clone(),
            )));
        }
    }
}

#[cfg(test)]
mod tests;
