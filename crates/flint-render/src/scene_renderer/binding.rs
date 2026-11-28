//! The scene a renderer draws (ADR 0118 decision 5, #739): what a host binds
//! once, when a scene loads, so that no host can skip a step of it.
//!
//! - [`SceneBinding::new`] (or [`SceneBinding::over`] a host's own registry)
//!   makes the [`StyleRegistry`] the scene is drawn with: the built-ins, a
//!   game's styles, and the user styles the scene can see (#742). A host
//!   makes one before it reads the scene's `[post_process]`, against
//!   [`SceneBinding::styles`] (a table naming a style it doesn't hold is
//!   dropped), and `bind_scene` hands it to the renderer.
//! - [`SceneRenderer::bind_scene`] gives the renderer the scene's project and
//!   folder, forgets everything per entity and per scene the last one left
//!   (style history, bone buffers and holds, the `[environment]` levers, the
//!   terrain and its grass), then loads the scene's `grass_ground` from its
//!   world. The player, the viewer and `flint render` call it; before #739
//!   each wired these by hand, and the viewer loaded no terrain at all.
//!   The terrain is built by the terrain package since #1615
//!   (`flint_terrain::scene::bind_terrain`, which each host calls next): the
//!   renderer draws the chunks it is handed and links no terrain crate.
//!
//! What changes every frame is [`FrameInputs`](super::FrameInputs), in
//! `frame.rs`.

use super::SceneRenderer;
use crate::stylize::StyleRegistry;
use flint_core::components as comp;
use flint_core::project::AssetScope;
use flint_ecs::FlintWorld;
use flint_schema::{ArchetypeVisual, SchemaRegistry};
use std::collections::HashMap;
use std::path::Path;

/// A scene as a renderer binds it: its project and folder, with the user
/// styles they reach registered. Made once a scene load, before the scene's
/// `[post_process]` is read, and handed to [`SceneRenderer::bind_scene`].
#[derive(Clone, Debug)]
pub struct SceneBinding {
    assets: AssetScope,
    /// The styles the scene is drawn with (#742).
    styles: StyleRegistry,
    style_messages: Vec<String>,
    /// The scene's `[environment]`, whose lighting levers `bind_scene` sets.
    environment: Option<flint_scene::EnvironmentDef>,
    /// Each archetype's placeholder box, from its schema's
    /// `[archetype.<name>.visual]` (#751).
    archetype_visuals: HashMap<String, ArchetypeVisual>,
}

impl SceneBinding {
    /// The scene whose project and folder `assets` names, drawn with the
    /// built-in styles and every `styles/*.shader.toml` the project's
    /// search lists from there (ADR 0112, 0114), the nearest of a name
    /// winning. What went wrong is in
    /// [`style_messages`](Self::style_messages), for the host to say.
    pub fn new(assets: AssetScope) -> Self {
        Self::over(assets, &StyleRegistry::new())
    }

    /// [`new`](Self::new) over `base`, a host's registry (#742): its
    /// built-ins and game styles (#419), with this scene's user styles in
    /// place of any `base` held. A style `base` loaded from the same
    /// unchanged file is shared, so its built pass is kept.
    pub fn over(assets: AssetScope, base: &StyleRegistry) -> Self {
        let (styles, style_messages) = base.for_scene(&assets);
        Self {
            assets,
            styles,
            style_messages,
            environment: None,
            archetype_visuals: HashMap::new(),
        }
    }

    /// The archetypes the scene's entities were made from, in `schemas`:
    /// the box each one's `bounds` draws as when it has nothing else to
    /// draw, its `[archetype.<name>.visual]` (#751). The renderer names no
    /// archetype, so without this every placeholder box is grey.
    pub fn with_archetypes(mut self, schemas: &SchemaRegistry) -> Self {
        self.archetype_visuals = schemas
            .archetypes()
            .into_iter()
            .filter_map(|archetype| Some((archetype.name.clone(), archetype.visual?)))
            .collect();
        self
    }

    /// The scene's `[environment]` table: its `shadows` and
    /// `default_lights` (#627), hemisphere ambient, diffuse wrap,
    /// Oren-Nayar and sheen, set by `bind_scene` over the built-ins. None,
    /// or a key left out, is the built-in. Its skybox is the host's.
    pub fn with_environment(mut self, environment: Option<&flint_scene::EnvironmentDef>) -> Self {
        self.environment = environment.cloned();
        self
    }

    /// [`new`](Self::new) for the scene file at `scene_path`, its project
    /// found from there.
    pub fn for_scene(scene_path: &Path) -> Self {
        Self::new(AssetScope::for_scene(scene_path))
    }

    /// The scene's project and folder.
    pub fn assets(&self) -> &AssetScope {
        &self.assets
    }

    /// What registering the scene's user styles had to say: one line each.
    pub fn style_messages(&self) -> &[String] {
        &self.style_messages
    }

    /// The same scene over `base` (#742): its user styles loaded again
    /// against `base` (one unchanged since is shared), so a host that binds
    /// a scene again after a hot reload keeps what the reload brought.
    pub fn restyled(mut self, base: &StyleRegistry) -> Self {
        let (styles, messages) = base.for_scene(&self.assets);
        self.styles = styles;
        self.style_messages = messages;
        self
    }

    /// The registry the scene is drawn with: read its `[post_process]`
    /// against this (`post_process_config_from_def`).
    pub fn styles(&self) -> &StyleRegistry {
        &self.styles
    }
}

/// What [`SceneRenderer::bind_scene`] loaded from the scene's world, for the
/// host's own share of it (the grass debug panel, `flint render`'s lines).
#[derive(Default)]
pub struct BoundScene {
    /// The scene's grass: a `grass_ground`'s, or else the terrain's, which
    /// the terrain package sets when it builds the terrain after
    /// `bind_scene` (#1615).
    pub grass: Option<BoundGrass>,
}

/// The grass [`SceneRenderer::bind_scene`] loaded, and where its settings
/// live.
pub struct BoundGrass {
    pub config: flint_geometry::grass::GrassConfig,
    /// The entity whose component holds the `grass.*` fields.
    pub entity_name: String,
    /// That component: `terrain` or `grass_ground`.
    pub component: &'static str,
}

impl SceneRenderer {
    /// Bind the scene `binding` names, whose world is `world`, in place of
    /// whatever the last one left (ADR 0118 decision 5, #739). Call it once a
    /// scene load, after the world is loaded and before its first frame is
    /// prepared; a host's own lever overrides (a flag, a panel) go after it.
    /// In order:
    ///
    /// 1. the project and folder fonts, glyph sets, palettes and looks are
    ///    found from ([`set_asset_scope`](Self::set_asset_scope));
    /// 2. no style history (#277), no bone buffers or motion holds (entity
    ///    ids are never reused, so the last scene's are garbage), and the
    ///    built-in ambient and lighting levers, then the scene's
    ///    `[environment]` over them ([`SceneBinding::with_environment`]),
    ///    and the archetypes' placeholder boxes in place of the last
    ///    scene's ([`SceneBinding::with_archetypes`]);
    /// 3. the last scene's terrain and grass unloaded, then this scene's
    ///    `grass_ground` loaded. The terrain is the host's next call
    ///    (`flint_terrain::scene::bind_terrain`, #1615), which masks its
    ///    chunks when the entity is styled (#310) and grows its grass unless
    ///    a `grass_ground`'s is in (ADR 0089).
    pub fn bind_scene(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &FlintWorld,
        binding: &SceneBinding,
    ) -> BoundScene {
        flint_core::scope!("bind_scene");
        self.set_asset_scope(binding.assets.clone());
        self.styles = binding.styles.clone();
        // A user style's component has no schema file for the loader to
        // check it against (it loads at run time), so it is checked here
        // against the schema its descriptor makes (#556): a misspelt field,
        // a wrong type, an unknown choice, a value past its range (#492)
        for warning in crate::stylize::component_schema::user_style_component_warnings(
            world,
            &self.styles,
            &binding.assets.project.allow,
            None,
        ) {
            tracing::warn!("{warning}");
        }
        self.reset_style_history();
        self.clear_entity_bone_buffers();
        self.reset_ambient();
        self.set_scene_shadows(true);
        self.set_default_lights(true);
        self.lights.forget_warnings();
        // A new scene says its own styles' dropped entities (#740)
        crate::stylize::variant_table::forget_dropped();
        if let Some(env) = &binding.environment {
            self.set_environment(env);
        }
        self.archetype_visuals = binding.archetype_visuals.clone();
        self.clear_terrain();
        self.unload_grass();

        let project = &binding.assets.project;
        let scene_dir = binding.assets.scene_dir.as_path();
        let grass = super::load_grass_ground_in(world, project, scene_dir, device, queue, self)
            .map(|ground| BoundGrass {
                config: ground.config,
                entity_name: ground.entity_name,
                component: comp::GRASS_GROUND,
            });
        BoundScene { grass }
    }
}

impl SceneRenderer {
    /// A scene's `[environment]` levers, over the built-ins: no shadows and
    /// no built-in lights where it says so (#627), and each lever it names.
    fn set_environment(&mut self, env: &flint_scene::EnvironmentDef) {
        self.set_scene_shadows(env.casts_shadows());
        self.set_default_lights(env.uses_default_lights());
        if env.ambient_sky.is_some() || env.ambient_ground.is_some() {
            self.set_ambient(
                env.ambient_sky
                    .unwrap_or(crate::LightUniforms::DEFAULT_AMBIENT_SKY),
                env.ambient_ground
                    .unwrap_or(crate::LightUniforms::DEFAULT_AMBIENT_GROUND),
            );
        }
        if let Some(wrap) = env.diffuse_wrap {
            self.set_diffuse_wrap(wrap);
        }
        if let Some(oren) = env.oren_nayar {
            self.set_oren_nayar(oren);
        }
        if let Some(strength) = env.sheen_strength {
            self.set_sheen(env.sheen_color.unwrap_or([1.0; 3]), strength);
        }
    }
}
