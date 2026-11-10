//! Living props in the player (#211): a scene's `living_prop` components become
//! skinned meshes, each with its own bones, and rings of poses the resolver
//! keeps full.
//!
//! Each prop's `model.asset` is rewritten at load from the spec's name to its
//! lineage's mesh, `<spec>@<lineage seed>`, and marked `runtime`: the file
//! loader and the plain procgen path leave it alone from then on. An entity
//! whose spec can't live keeps its model and takes the plain path, as a still
//! tree, with a warning saying why.
//!
//! The mesh goes up through the same `load_skinned_model` a skinned GLB does,
//! and each prop's bones through `update_bone_matrices`, so each one has its
//! own bone buffer. The buffer is the prop's skeleton instance: a pose reaches
//! it as a whole palette from [`LivingRig::palette`], which holds scale the
//! animation system's joint hierarchy can't, so props don't register a
//! `Skeleton` there, and cost nothing per frame while they hold still.
//!
//! Every frame [`upload_blends`] blends each prop's two poses per part on the
//! warped clock (#212) and writes what changed. All of that is on the CPU, so
//! no new shader or vertex data: the leaf cards' jitter fits in the palette
//! (see [`flint_procgen::CardBoil`]) and their tint is a material change.
//!
//! A prop's echoes (#228) blend the same way at their own places on the clock,
//! each into a bone buffer of its own, and the renderer draws them in the
//! transparent pass in the prop's own materials.

use flint_core::components as comp;
use flint_core::toml_util::toml_f32;
use flint_core::wooze::WoozeField;
use flint_ecs::FlintWorld;
use flint_import::{AlphaMode, ImportResult, ImportedMaterial, ImportedMesh, ImportedTexture};
use flint_procgen::{
    AssetKind, BlendFrame, ClockOverride, EchoConfig, EchoOverride, LineageDone, LivingDefaults,
    LivingPropId, LivingPropRequest, LivingProps, LivingRig, LivingScope, LivingShadows,
    LivingStats, ProcGenResolver, SpeciesOverride, Stagger, VarianceConfig, VarianceOverride,
};
use flint_render::model_loader::ModelLoadConfig;
use flint_render::SceneRenderer;

use super::procgen_assets::{AssetSink, ProcGenMode};

/// A scene's `[living]` table as the resolver wants it. A bad value is named
/// and then means its default, since a look lab shouldn't refuse to start over
/// one knob.
pub fn defaults_of(table: Option<&flint_scene::LivingDef>) -> LivingDefaults {
    let Some(table) = table else {
        return LivingDefaults::default();
    };
    let shadows = match table.shadows.as_deref() {
        None | Some("blended") => LivingShadows::Blended,
        Some("drift") => LivingShadows::Drift,
        Some(other) => {
            tracing::warn!(
                "[living] shadows = \"{other}\" isn't \"blended\" or \"drift\"; using blended"
            );
            LivingShadows::Blended
        }
    };
    let mut warn = |problem: String| tracing::warn!("[living] {problem}; using each spec's");
    let stagger = table
        .stagger
        .as_deref()
        .and_then(|name| checked_stagger(name, &mut warn));
    let species = table
        .species
        .iter()
        .map(|(name, def)| {
            let mut warn = |problem: String| {
                tracing::warn!("[living.species.{name}] {problem}; leaving it to the layers below")
            };
            (name.clone(), species_of(def, &mut warn))
        })
        .collect();
    LivingDefaults {
        shadows,
        echoes: echoes_of(table.echoes.as_ref(), &mut warn),
        stagger,
        variance: variance_of(table.variance.as_ref(), &mut warn),
        softness: softness_of(table.softness, &mut warn),
        species,
    }
}

/// A softness (#582) as a scene or a tune gives it: 0 to 1, or `None` with the
/// reason sent to `say`.
pub fn softness_of(value: Option<f32>, say: &mut dyn FnMut(String)) -> Option<f32> {
    let value = value?;
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Some(value)
    } else {
        say(format!("softness = {value} must be in [0.0, 1.0]"));
        None
    }
}

/// One species' table (#246) as the resolver wants it. Each bad value goes to
/// `say`, by key, and is left unset, so the layers below decide it.
pub fn species_of(
    def: &flint_scene::LivingSpeciesDef,
    say: &mut dyn FnMut(String),
) -> SpeciesOverride {
    let mut knob = |name: &str, value: Option<f32>| {
        value.filter(
            |&value| match flint_core::wooze::check_knob_named(name, value) {
                Ok(_) => true,
                Err(problem) => {
                    say(problem);
                    false
                }
            },
        )
    };
    let clock = ClockOverride {
        amplitude: knob("amplitude", def.amplitude),
        step: knob("step", def.step),
        glide: knob("glide", def.glide),
        jitter: knob("jitter", def.jitter),
    };
    SpeciesOverride {
        clock,
        stagger: def
            .stagger
            .as_deref()
            .and_then(|name| checked_stagger(name, say)),
        echoes: echoes_of(def.echoes.as_ref(), say),
        variance: variance_of(def.variance.as_ref(), say),
        softness: softness_of(def.softness, say),
    }
}

/// Everything wrong with one species' table, for `flint validate`.
pub fn species_problems(def: &flint_scene::LivingSpeciesDef) -> Vec<String> {
    let mut problems = Vec::new();
    species_of(def, &mut |problem| problems.push(problem));
    problems
}

/// A stagger by name, or `None` with the reason sent to `say`.
fn checked_stagger(name: &str, say: &mut dyn FnMut(String)) -> Option<Stagger> {
    let stagger = stagger_of(name);
    if stagger.is_none() {
        say(format!(
            "stagger = \"{name}\" isn't \"none\", \"per_level\" or \"per_branch\""
        ));
    }
    stagger
}

/// A stagger by the name a spec or a scene knows it by.
pub fn stagger_of(name: &str) -> Option<Stagger> {
    toml::Value::String(name.to_string()).try_into().ok()
}

/// The name a spec or a scene knows a stagger by.
pub fn stagger_name(stagger: Stagger) -> &'static str {
    match stagger {
        Stagger::None => "none",
        Stagger::PerLevel => "per_level",
        Stagger::PerBranch => "per_branch",
    }
}

/// A `variance` table as the resolver wants it, with a bad field sent to `say`
/// and then left to the layers below.
pub fn variance_of(
    table: Option<&flint_scene::LivingVarianceDef>,
    say: &mut dyn FnMut(String),
) -> VarianceOverride {
    let Some(table) = table else {
        return VarianceOverride::default();
    };
    let mut fine = |variance: VarianceConfig| match variance.check() {
        Ok(()) => true,
        Err((key, reason)) => {
            say(format!("variance.{key} = {reason}"));
            false
        }
    };
    let base = VarianceConfig::default();
    VarianceOverride {
        amount: table
            .amount
            .filter(|&amount| fine(VarianceConfig { amount, ..base })),
        steady: table
            .steady
            .filter(|&steady| fine(VarianceConfig { steady, ..base })),
        keep_size: table.keep_size,
    }
}

/// The field a scene's `[wooze]` table describes, if it has one. For the same
/// reason as above, what's wrong with it is named and left out, here where
/// `flint validate` fails on it.
pub fn field_of(table: Option<&flint_scene::WoozeDef>) -> Option<WoozeField> {
    let (field, issues) = table?.field();
    for issue in issues {
        tracing::warn!("{issue}; going without it");
    }
    Some(field)
}

/// An `echoes` table as the resolver wants it, with a bad field sent to `say`
/// and then left to the layers below.
pub fn echoes_of(
    table: Option<&flint_scene::LivingEchoesDef>,
    say: &mut dyn FnMut(String),
) -> EchoOverride {
    let Some(table) = table else {
        return EchoOverride::default();
    };
    let mut fine = |echoes: EchoConfig| match echoes.check() {
        Ok(()) => true,
        Err((key, reason)) => {
            say(format!("echoes.{key} = {reason}"));
            false
        }
    };
    let base = EchoConfig::default();
    EchoOverride {
        count: table
            .count
            .filter(|&count| fine(EchoConfig { count, ..base })),
        spread: table
            .spread
            .filter(|&spread| fine(EchoConfig { spread, ..base })),
        alpha: table
            .alpha
            .filter(|&alpha| fine(EchoConfig { alpha, ..base })),
    }
}

/// A scene's living props with no window and no frame budget, for `flint render`
/// (#216): the same load, the same field and the same step the player takes each
/// frame, so a capture at a game time shows what the player shows at it.
pub struct LivingRun {
    resolver: ProcGenResolver,
    blend: BlendFrame,
    field: Option<WoozeField>,
    /// The clock the field is read on, as the player's `wooze_time`.
    time: f64,
}

impl LivingRun {
    /// Whether `world` has anything for this to run. Everything else about a scene
    /// that doesn't opt in stays as `flint render` has always drawn it.
    pub fn wanted_by(world: &FlintWorld) -> bool {
        world.all_entities().iter().any(|entity| {
            world
                .get_components(entity.id)
                .is_some_and(|comps| comps.get(comp::LIVING_PROP).is_some())
        })
    }

    /// Resolve the scene's procgen assets as the player does at load, before the
    /// model loader, with every lineage built and every ring full.
    pub fn load(
        scene_path: &str,
        living: Option<&flint_scene::LivingDef>,
        wooze: Option<&flint_scene::WoozeDef>,
        world: &mut FlintWorld,
        sink: super::procgen_assets::GpuSink<'_>,
    ) -> Self {
        let mut resolver = ProcGenResolver::new();
        resolver.set_living_defaults(defaults_of(living));
        super::procgen_assets::resolve_scene(
            scene_path,
            world,
            &mut resolver,
            ProcGenMode::Immediate,
            sink,
        );
        Self {
            resolver,
            blend: BlendFrame::default(),
            field: field_of(wooze),
            time: 0.0,
        }
    }

    /// Lay `knobs` over the scene's global wooze values, before the first step. A
    /// scene with no `[wooze]` table is treated as if it had an empty one.
    pub fn change_field(&mut self, knobs: &toml::Table) -> Result<(), String> {
        self.field
            .get_or_insert_with(|| WoozeField::new(flint_core::wooze::WoozeSample::NEUTRAL))
            .set_global(knobs, self.time, 0.0)
    }

    /// Lay a preset file's `[living]` over the scene's, as the player's Load does
    /// (#249): its stagger, echoes and variance set the global layer a field at a
    /// time, and each species it names takes exactly its values. Its `[wooze]`
    /// knobs go through [`Self::change_field`]. What can't be used is returned.
    pub fn apply_preset(&mut self, living: &flint_scene::LivingDef) -> Vec<String> {
        let mut problems = Vec::new();
        let mut say = |problem: String| problems.push(problem);
        let stagger = (living.stagger.as_deref()).and_then(|name| checked_stagger(name, &mut say));
        let echoes = echoes_of(living.echoes.as_ref(), &mut say);
        let variance = variance_of(living.variance.as_ref(), &mut say);
        let softness = softness_of(living.softness, &mut say);
        let species: Vec<(String, SpeciesOverride)> = (living.species.iter())
            .map(|(name, def)| {
                let mut say = |problem: String| say(format!("species '{name}': {problem}"));
                (name.clone(), species_of(def, &mut say))
            })
            .collect();
        let props = self.resolver.living_props_mut();
        if let Some(stagger) = stagger {
            props.tune_stagger(LivingScope::All, stagger);
        }
        props.tune_echoes(LivingScope::All, echoes);
        props.tune_variance(LivingScope::All, variance);
        if let Some(softness) = softness {
            props.tune_softness(LivingScope::All, softness);
        }
        for (name, over) in species {
            if !props.set_species(&name, over) {
                problems.push(format!("no living prop grows from '{name}'"));
            }
        }
        problems
    }

    /// The field, if the scene has one or has been given one.
    pub fn field(&self) -> Option<&WoozeField> {
        self.field.as_ref()
    }

    /// Where the field is kept, for a script's change to it (#8). A change that
    /// gives a scene with none a field puts it here.
    pub(crate) fn field_slot(&mut self) -> &mut Option<WoozeField> {
        &mut self.field
    }

    /// The props, for a script's tune (#8).
    pub(crate) fn props_mut(&mut self) -> &mut LivingProps {
        self.resolver.living_props_mut()
    }

    /// Move the clocks on by `dt` and make whatever poses that calls for. A step of
    /// 0, as the player's first frame is, reads the field without moving.
    pub fn step(&mut self, dt: f64) {
        self.time += dt;
        let wooze = self.field.as_ref().map(|field| (field, self.time));
        // Load built every lineage, and no new ones turn up.
        self.resolver.step_living_now(dt, wooze);
    }

    /// The clock, in seconds.
    pub fn time(&self) -> f64 {
        self.time
    }

    /// The props as they stand.
    pub fn props(&self) -> &LivingProps {
        self.resolver.living_props()
    }

    /// Blend every prop where its clock stands and write what moved, as a frame of
    /// the player does.
    pub fn upload(
        &mut self,
        renderer: &mut SceneRenderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        upload_blends(&mut self.resolver, &mut self.blend, renderer, device, queue);
    }
}

/// What reads the wooze field besides living props, with no window, for `flint
/// render`: the shading boil (#214), and shallow wooze (#215) if asked. It steps as
/// the player's frame does, so a capture at a game time shows what the player shows.
pub struct WoozeClocks {
    field: WoozeField,
    time: f64,
    boil: flint_core::wooze::BoilClock,
    shallow: Option<super::shallow_wooze::ShallowWooze>,
    grass: super::shallow_wooze::GrassWooze,
}

impl WoozeClocks {
    /// The clocks of a scene's `[wooze]` table with `knobs` laid over it, or `None`
    /// if the scene has no table and nothing is laid over it, so nothing samples.
    pub fn of_scene(
        table: Option<&flint_scene::WoozeDef>,
        knobs: Option<&toml::Table>,
        shallow: bool,
    ) -> Result<Option<Self>, String> {
        let mut field = table.map(|def| def.field().0);
        if let Some(knobs) = knobs {
            field
                .get_or_insert_with(|| WoozeField::new(flint_core::wooze::WoozeSample::NEUTRAL))
                .set_global(knobs, 0.0, 0.0)?;
        }
        Ok(field.map(|field| Self {
            field,
            time: 0.0,
            boil: Default::default(),
            shallow: shallow.then(Default::default),
            grass: Default::default(),
        }))
    }

    /// The clocks of `field` as it stands at `time`: the field a script gave a
    /// scene that had none (#8).
    pub(crate) fn of_field(field: WoozeField, time: f64, shallow: bool) -> Self {
        Self {
            field,
            time,
            boil: Default::default(),
            shallow: shallow.then(Default::default),
            grass: Default::default(),
        }
    }

    /// Change the field at the time its clock has reached, as a script's
    /// command does (#8). `change` gets the field in the slot shape
    /// [`LivingRun::field_slot`] has, so one change goes to both the same way.
    pub(crate) fn change_field(&mut self, change: impl FnOnce(&mut Option<WoozeField>, f64)) {
        let mut slot = Some(self.field.clone());
        change(&mut slot, self.time);
        if let Some(field) = slot {
            self.field = field;
        }
    }

    /// Move on by `dt`, with the viewer at `viewer`. Returns what shallow wooze draws
    /// with now, for `SceneRenderer::set_shallow_wooze`.
    pub fn step(
        &mut self,
        world: &FlintWorld,
        viewer: [f32; 3],
        dt: f64,
    ) -> &[(flint_core::EntityId, flint_render::ShallowWoozeDraw)] {
        self.time += dt;
        let sample = self.field.sample(viewer, self.time);
        self.boil.tick(dt, &sample);
        match &mut self.shallow {
            Some(shallow) => shallow.advance(world, &self.field, self.time, dt),
            None => &[],
        }
    }

    /// Move woozy grass (#252) on by the `dt` the last [`Self::step`] took, with the
    /// viewer at `viewer`. Grass `config` doesn't ask for it stays on the wall clock.
    pub fn step_grass(
        &mut self,
        config: Option<&flint_terrain::GrassConfig>,
        viewer: [f32; 3],
        dt: f64,
    ) {
        self.grass
            .advance(config, &self.field, viewer, self.time, dt);
    }

    /// The grass's clock and comb as of the last [`Self::step_grass`].
    pub fn grass(&self) -> &super::shallow_wooze::GrassWooze {
        &self.grass
    }

    /// The boil's drawing where its clock now stands.
    pub fn boil(&self, viewer: [f32; 3], on: u32) -> flint_core::wooze::BoilFrame {
        self.boil.frame(&self.field.sample(viewer, self.time), on)
    }

    /// The boil's drawing at `time`, in one tick at the tempo of that moment, whatever
    /// the clock says: `--boil-time`.
    pub fn boil_at(&self, viewer: [f32; 3], time: f64, on: u32) -> flint_core::wooze::BoilFrame {
        let sample = self.field.sample(viewer, time);
        let mut clock = flint_core::wooze::BoilClock::default();
        clock.tick(time, &sample);
        clock.frame(&sample, on)
    }
}

/// One frame's blending and upload for every prop, timed for `--exit-after`.
/// The clock moved in `process_living`, so this draws where it now stands.
pub fn upload_blends(
    resolver: &mut ProcGenResolver,
    frame: &mut BlendFrame,
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) {
    upload_props(resolver.living_props_mut(), frame, renderer, device, queue);
}

/// [`upload_blends`] for any set of props: the scene's, or the other side of an
/// A/B (#238), whose bones go into the same buffers.
pub fn upload_props(
    props: &mut LivingProps,
    frame: &mut BlendFrame,
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) {
    if props.is_empty() {
        return;
    }
    let started = web_time::Instant::now();
    let mut writes = 0;
    for i in 0..props.props().len() {
        let id = LivingPropId(i);
        let blend = props.blend_prop(id, frame);
        if let Some(count) = blend.echo_count {
            renderer.set_entity_echo_count(blend.entity, count);
        }
        let lighten = props.prop(id).echo_blend() == flint_procgen::EchoBlend::Lighten;
        renderer.set_entity_echo_lighten(blend.entity, lighten);
        if let Some(first) = blend.first_bone {
            renderer.write_entity_bones(device, queue, blend.entity, first, &frame.bones);
            writes += 1;
        }
        if blend.shadow_changed {
            renderer.write_entity_shadow_bones(device, queue, blend.entity, &frame.shadow_bones);
            writes += 1;
        }
        for (part, tint) in &frame.tints {
            renderer.set_entity_part_tint(blend.entity, *part, *tint);
        }
        // The body's silhouette (#582); the cards keep their edges. A write of
        // what's there already changes nothing, so it can go every frame.
        if let Some((_, rig)) = props.rig_of(id) {
            let softness = props.softness(id);
            for part in rig.body_parts() {
                renderer.set_entity_part_softness(blend.entity, part, softness);
            }
        }
        for echo in 0..props.prop(id).echo_count() {
            if let Some(first) = props.blend_echo(id, echo, frame) {
                let opacity = props.prop(id).echo(echo).opacity;
                renderer.write_entity_echo_bones(
                    device,
                    queue,
                    blend.entity,
                    echo,
                    opacity,
                    first,
                    &frame.bones,
                );
                writes += 1;
            }
        }
    }
    props.record_blend(started.elapsed().as_secs_f64() * 1e3, writes);
}

/// Read an entity's `living_prop` component into a request for `spec_name`.
/// A bad field is named in the error, rather than quietly meaning its default.
pub fn read_request(
    entity: flint_core::EntityId,
    entity_name: &str,
    position: [f32; 3],
    spec_name: &str,
    component: &toml::Value,
) -> Result<LivingPropRequest, String> {
    let lineage_seed = match component.get("lineage_seed") {
        None => None,
        Some(v) => Some(
            v.as_integer()
                .and_then(|seed| u64::try_from(seed).ok())
                .ok_or_else(|| format!("lineage_seed {v} isn't a seed: it must be 0 or more"))?,
        ),
    };
    let pose_ring = match component.get("pose_ring") {
        None => None,
        Some(v) => Some(
            v.as_integer()
                .and_then(|ring| u32::try_from(ring).ok())
                .filter(|ring| *ring >= 2)
                .ok_or_else(|| {
                    format!("pose_ring {v} is too small to blend: it must be 2 or more")
                })?,
        ),
    };
    let phase_offset = match component.get("phase_offset") {
        None => None,
        Some(v) => Some(
            toml_f32(v)
                .filter(|phase| phase.is_finite())
                .ok_or_else(|| format!("phase_offset {v} isn't a number"))?,
        ),
    };
    let stagger = match component.get("stagger") {
        None => None,
        Some(v) => Some(v.clone().try_into::<Stagger>().map_err(|_| {
            format!("stagger {v} isn't one of \"none\", \"per_level\", \"per_branch\"")
        })?),
    };
    Ok(LivingPropRequest {
        entity,
        entity_name: entity_name.to_string(),
        position,
        spec_name: spec_name.to_string(),
        lineage_seed,
        pose_ring,
        phase_offset,
        stagger,
    })
}

/// Turn the world's `living_prop` entities into living props: register each
/// with the resolver, point its model at its lineage's mesh, and either build
/// everything now or queue it behind placeholders.
pub fn resolve(
    world: &mut FlintWorld,
    config: &ModelLoadConfig,
    resolver: &mut ProcGenResolver,
    sink: &mut dyn AssetSink,
    mode: ProcGenMode,
) {
    // Not a single scan per frame: this is load, and it's all the cost a scene
    // with no living props pays.
    let mut assets: Vec<(flint_core::EntityId, String)> = Vec::new();
    for entity in world.all_entities() {
        let Some(comps) = world.get_components(entity.id) else {
            continue;
        };
        let Some(component) = comps.get(comp::LIVING_PROP) else {
            continue;
        };
        let name = &entity.name;
        let Some(spec_name) = comps
            .get(comp::MODEL)
            .and_then(|m| m.get("asset"))
            .and_then(|a| a.as_str())
        else {
            tracing::warn!("Living prop '{name}' has no model.asset naming its spec; ignoring it");
            continue;
        };
        if config.overrides.contains_key(spec_name) {
            tracing::warn!(
                "Living prop '{name}': the catalog has '{spec_name}', and a catalog entry can't \
                 live, so it stays a still model"
            );
            continue;
        }
        let position = world
            .get_world_position(entity.id)
            .map_or([0.0; 3], |p| [p.x, p.y, p.z]);
        let added = read_request(entity.id, name, position, spec_name, component)
            .and_then(|request| resolver.add_living_prop(request));
        match added {
            Ok((_, asset)) => assets.push((entity.id, asset)),
            Err(reason) => {
                tracing::warn!("Living prop '{name}' stays a still model: {reason}");
            }
        }
    }
    if assets.is_empty() {
        return;
    }

    for (id, asset) in &assets {
        let model = world
            .get_components_mut(*id)
            .and_then(|comps| comps.get_mut(comp::MODEL))
            .and_then(|model| model.as_table_mut());
        if let Some(model) = model {
            model.insert("asset".into(), toml::Value::String(asset.clone()));
            model.insert("runtime".into(), toml::Value::Boolean(true));
        }
    }

    let props = resolver.living_props();
    eprintln!(
        "[living] {} prop(s) on {} lineage(s)",
        props.props().len(),
        props.lineage_count()
    );
    match mode {
        ProcGenMode::Immediate => {
            let started = web_time::Instant::now();
            let done = resolver.make_living_now();
            apply_lineages(&done, sink);
            let props = resolver.living_props();
            let poses: usize = props.props().iter().map(|p| p.ready()).sum();
            eprintln!(
                "[living] {} lineage(s) and {poses} poses in {:.0} ms",
                done.len(),
                started.elapsed().as_secs_f64() * 1e3,
            );
        }
        ProcGenMode::Deferred => {
            let mut uploaded: Vec<&str> = Vec::new();
            for (_, asset) in &assets {
                if !uploaded.contains(&asset.as_str()) {
                    sink.upload_placeholder(asset, AssetKind::Model);
                    uploaded.push(asset);
                }
            }
        }
    }
}

/// Send up finished lineages: the mesh once, and rest bones for each prop. One
/// that couldn't be made leaves a placeholder under its name.
pub fn apply_lineages(done: &[LineageDone], sink: &mut dyn AssetSink) {
    for lineage in done {
        match &lineage.rig {
            Ok(rig) => {
                sink.upload_living(&lineage.asset_name, rig);
                for entity in &lineage.entities {
                    sink.upload_bones(*entity, &lineage.asset_name, &lineage.rest_palette);
                }
            }
            Err(reason) => {
                tracing::warn!(
                    "Couldn't build living lineage '{}': {reason}; showing a placeholder",
                    lineage.asset_name
                );
                sink.upload_placeholder(&lineage.asset_name, AssetKind::Model);
            }
        }
    }
}

/// A rig's rest mesh as a model import, split by sub-mesh, so a procgen skeleton
/// goes up the same path as a skinned GLB. Each part keeps only the vertices it
/// uses, not the whole shared buffer: a tree has six parts.
pub fn import_of(name: &str, rig: &LivingRig) -> ImportResult {
    let mesh = &rig.mesh;
    let mut result = ImportResult {
        asset_meta: flint_asset::AssetMeta {
            name: name.to_string(),
            asset_type: flint_asset::AssetType::Mesh,
            hash: String::new(),
            source_path: None,
            format: None,
            properties: Default::default(),
            tags: Vec::new(),
        },
        meshes: Vec::with_capacity(mesh.submeshes.len()),
        textures: Vec::new(),
        materials: Vec::with_capacity(mesh.submeshes.len()),
        skeletons: Vec::new(),
        skeletal_clips: Vec::new(),
        node_clips: Vec::new(),
        nodes: Vec::new(),
        root_nodes: Vec::new(),
    };
    let fallback = flint_procgen::MaterialData::default();
    for (i, sub) in mesh.submeshes.iter().enumerate() {
        let start = (sub.index_start as usize).min(mesh.indices.len());
        let end = (start + sub.index_count as usize).min(mesh.indices.len());
        let indices = &mesh.indices[start..end];
        let (Some(lo), Some(hi)) = (indices.iter().min(), indices.iter().max()) else {
            continue;
        };
        let span = *lo as usize..=*hi as usize;
        let vertices = &mesh.vertices[span.clone()];
        let source = mesh.materials.get(sub.material_index).unwrap_or(&fallback);

        let mut material = ImportedMaterial {
            name: source.name.clone(),
            base_color: source.base_color,
            metallic: source.metallic,
            roughness: source.roughness,
            use_vertex_color: true,
            alpha_mode: AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            ..Default::default()
        };
        if let Some(normal) = &source.normal_map {
            let texture = format!("part{i}_normal");
            result.textures.push(ImportedTexture {
                name: texture.clone(),
                width: normal.width,
                height: normal.height,
                format: "rgba8".to_string(),
                data: normal.pixels.clone(),
            });
            material.normal_texture = Some(texture);
        }
        result.materials.push(material);

        result.meshes.push(ImportedMesh {
            name: format!("{name}_part{i}"),
            positions: vertices.iter().map(|v| v.position).collect(),
            normals: vertices.iter().map(|v| v.normal).collect(),
            uvs: vertices.iter().map(|v| v.uv).collect(),
            indices: indices.iter().map(|ix| ix - lo).collect(),
            material_index: Some(result.materials.len() - 1),
            joint_indices: Some(span.clone().map(|v| rig.runtime_joints(v).0).collect()),
            joint_weights: Some(span.map(|v| rig.runtime_joints(v).1).collect()),
            skin_index: Some(0),
            lod: None,
        });
    }
    result
}

/// The `[living]` lines `--exit-after` prints, so a run can be read headlessly:
/// whether the rings stayed full and whether they kept to the budget.
pub fn report(
    stats: &LivingStats,
    prop_count: usize,
    lineages: usize,
    ring: usize,
    budget_ms: f64,
) -> [String; 3] {
    let avg = stats.total_ms / stats.frames.max(1) as f64;
    [
        format!(
            "[living] {prop_count} props on {lineages} lineages, {ring} poses a ring: {} of {} frames \
             ended full, never more than {} short, {} frames missing a pose in use",
            stats.frames_full, stats.frames, stats.most_short, stats.frames_starved,
        ),
        format!(
            "[living] {} poses made, {} from cache, {} failed: avg {avg:.2} ms/frame, worst {:.2} ms, \
             budget {budget_ms:.0} ms, over it on {} frames",
            stats.poses_made, stats.poses_from_cache, stats.pose_failures, stats.worst_ms,
            stats.frames_over_budget,
        ),
        format!(
            "[living] blending and upload: avg {:.3} ms/frame, worst {:.2} ms, over 1 ms on {} \
             frames, {:.1} bone writes a frame, {} kept their last bones for want of a pose",
            stats.blend_total_ms / stats.blend_frames.max(1) as f64,
            stats.blend_worst_ms,
            stats.blend_frames_over_1ms,
            stats.bone_writes as f64 / stats.blend_frames.max(1) as f64,
            stats.blends_starved,
        ),
    ]
}

/// The `[living]` line for a run whose knobs were turned, if any were.
pub fn knobs_report(stats: &LivingStats, props: &LivingProps) -> Option<String> {
    let waiting = props.props().iter().filter(|p| p.is_waiting()).count();
    (stats.poses_reheld > 0 || waiting > 0).then(|| {
        format!(
            "[living] knobs: {} poses held again at a new variance, {waiting} props still waiting \
             on poses for new clocks",
            stats.poses_reheld,
        )
    })
}

/// The `[wooze]` line `--exit-after` prints: how far apart the field has put the
/// props, knob by knob, so that a run says whether its zones reached anything.
pub fn wooze_report(field: &WoozeField, props: &LivingProps) -> String {
    let range = |read: fn(&flint_core::wooze::WoozeSample) -> f32| {
        props
            .props()
            .iter()
            .map(|p| read(&p.wooze()))
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), value| {
                (lo.min(value), hi.max(value))
            })
    };
    let [step, glide, amplitude, jitter] = [
        range(|s| s.step),
        range(|s| s.glide),
        range(|s| s.amplitude),
        range(|s| s.jitter),
    ];
    format!(
        "[wooze] {} zones over {} props: step {:.2}..{:.2}, glide {:.2}..{:.2}, amplitude \
         {:.2}..{:.2}, jitter {:.2}..{:.2}",
        field.zones().len(),
        props.props().len(),
        step.0,
        step.1,
        glide.0,
        glide.1,
        amplitude.0,
        amplitude.1,
        jitter.0,
        jitter.1,
    )
}

#[cfg(test)]
#[path = "living_props_tests.rs"]
mod tests;
