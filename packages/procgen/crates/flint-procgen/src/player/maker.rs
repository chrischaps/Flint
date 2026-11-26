//! The procgen package's part of the player (#1557): its `PACKAGE`, the
//! resolver it puts in the run's resources, and the [`AssetMaker`] the
//! kernel calls at a scene's load, for spawned entities, each frame and at
//! a teardown. Until #1557 all of this was `flint-player` code behind its
//! `procgen` feature.

use super::assets::{
    apply_completed, apply_failures, collect_requests, collect_requests_of, discover_specs,
    resolve, GpuSink, Recording,
};
use super::AssetSink;
use crate::{CompletedAsset, DiskCacheConfig, ProcGenCacheConfig, ProcGenResolver};
use flint_package::assets::{AssetMaker, FrameMake, MakeCtx, MakeMode, RunStart};
use flint_package::{Package, Registry, Resources};
use flint_render::model_loader::ModelLoadConfig;

/// What the player's launcher lists for the procgen package.
pub static PACKAGE: Procgen = Procgen;

/// The procgen package, as a launcher lists it.
pub struct Procgen;

impl Package for Procgen {
    fn name(&self) -> &'static str {
        "procgen"
    }

    fn register(&self, registry: &mut Registry) {
        registry
            .run_resources(|resources| {
                resources.insert(ProcGenResolver::new());
            })
            .point(LoadSteps::default())
            .asset_maker(&MAKER);
    }
}

/// A step another package adds to a scene's procgen load, run after the
/// scene's specs are found and before its names are made: living's props,
/// which take their `living_prop` entities' names off the plain path
/// (ADR 0137 section 4, "procgen's resolve steps"). Added at registration:
/// `registry.point_mut::<LoadSteps>()`.
pub type LoadStep = fn(&mut LoadCtx, &mut dyn AssetSink, MakeMode);

/// The steps of a scene's load, in the order they were added.
#[derive(Default)]
pub struct LoadSteps(pub Vec<LoadStep>);

/// What a [`LoadStep`] is lent. The resolver is in `resources`, beside what
/// the step's package keeps there.
pub struct LoadCtx<'a> {
    pub world: &'a mut flint_ecs::FlintWorld,
    pub config: &'a ModelLoadConfig,
    pub resources: &'a mut Resources,
    pub packages: Option<&'a flint_core::package::InUse>,
}

impl LoadCtx<'_> {
    /// The project's `[packages]` leaves `package` out.
    pub fn leaves_out(&self, package: &str) -> bool {
        self.packages.is_some_and(|in_use| !in_use.is_on(package))
    }
}

/// The run's procgen resolver, which the package's registration puts in.
pub fn resolver(resources: &Resources) -> &ProcGenResolver {
    resources
        .get()
        .expect("the procgen package puts its resolver in the run's resources")
}

/// [`resolver`], to change.
pub fn resolver_mut(resources: &mut Resources) -> &mut ProcGenResolver {
    resources
        .get_mut()
        .expect("the procgen package puts its resolver in the run's resources")
}

/// The generators the run's specs are made by, for a game to add its own
/// to (ADR 0119): `None` in a run without the procgen package.
pub fn generators_mut(resources: &mut Resources) -> Option<&mut crate::GeneratorRegistry> {
    resources
        .get_mut::<ProcGenResolver>()
        .map(ProcGenResolver::registry_mut)
}

static MAKER: Maker = Maker;

struct Maker;

impl AssetMaker for Maker {
    /// The disk tier (#181), once: a resolver made with it is kept for the
    /// run, transitions included. The resolver is rebuilt around it and its
    /// generators move across, built-ins and a game's alike (#195).
    fn run_starts(&self, ctx: &mut RunStart) {
        let resolver = resolver_mut(ctx.resources);
        if !ctx.disk || resolver.disk_stats().is_some() {
            return;
        }
        if let Some(disk) = disk_config(ctx.project, ctx.scene_dir) {
            tracing::debug!(target: "load", "[procgen] disk cache: {}", disk.dir.display());
            use_disk(resolver, disk);
        }
    }

    /// Resolve the scene's procgen assets into the renderer: discover
    /// specs, run the other packages' load steps (living props), then
    /// catalog > procgen > file > placeholder for each other name. Runs
    /// before the model loader, which then skips every name this uploaded.
    fn scene(&self, ctx: &mut MakeCtx) {
        let started = web_time::Instant::now();
        // The load report's procgen stage (#1092): the note below is made by
        // hand once the time is known, which a load report doesn't hear
        let _span =
            flint_core::load_report::span(flint_core::load_report::Stage::Procgen, String::new);
        let mut sink = GpuSink {
            renderer: &mut *ctx.gpu.renderer,
            device: ctx.gpu.device,
            queue: ctx.gpu.queue,
            config: ctx.config,
        };
        discover_specs(
            resolver_mut(ctx.resources),
            &ctx.config.project,
            ctx.scene_path,
        );
        if let Some(steps) = ctx.registry.get_point::<LoadSteps>() {
            let mut load = LoadCtx {
                world: &mut *ctx.world,
                config: ctx.config,
                resources: &mut *ctx.resources,
                packages: ctx.packages,
            };
            for step in &steps.0 {
                step(&mut load, &mut sink, ctx.mode);
            }
        }
        let resolver = resolver_mut(ctx.resources);
        let requests = collect_requests(ctx.world, ctx.config, resolver, &sink);
        if requests.is_empty() {
            return;
        }
        let before = resolver.cache_stats();
        // Each mesh's triangles kept for the colliders naming it (#1222)
        let mut recording = Recording {
            sink: &mut sink,
            colliders: &mut *ctx.colliders,
        };
        resolve(&requests, resolver, &mut recording, ctx.mode);
        let stats = resolver.cache_stats();
        let scene_path = ctx.scene_path;
        // A frame note (#1077) when the scene had assets to make or find
        flint_core::frame_notes::note(
            flint_core::frame_notes::NoteKind::Procgen,
            || {
                format!(
                    "{} asset(s) for {}",
                    requests.len(),
                    flint_core::project::shown(std::path::Path::new(scene_path))
                )
            },
            Some(started.elapsed().as_secs_f32() * 1000.0),
        );
        // One line a load: how long procgen held the scene up, and where the
        // assets came from (#181: the disk tier is what makes a relaunch
        // fast). Deferred assets are queued, so they appear in none of the
        // three. Counted into the load's summary as `procgen assets` (#716).
        tracing::debug!(
            target: "load",
            kind = "procgen_asset",
            count = requests.len() as u64,
            "[procgen] {} asset(s) in {:.0} ms: {} generated, {} from disk, {} from memory",
            requests.len(),
            started.elapsed().as_secs_f64() * 1e3,
            stats.misses - before.misses,
            stats.disk_hits - before.disk_hits,
            stats.hits - before.hits,
        );
        tracing::debug!(
            "Procgen cache: {} entries, {} of {} bytes, {} hits / {} misses, {} evicted",
            stats.entry_count,
            stats.total_bytes,
            stats.max_memory_bytes,
            stats.hits,
            stats.misses,
            stats.evictions,
        );
    }

    /// Procgen before the file, for the names it has a spec of, of the
    /// entities `ids` alone (#1107).
    fn spawned(&self, ctx: &mut MakeCtx, ids: &[flint_core::EntityId]) {
        let mut sink = GpuSink {
            renderer: &mut *ctx.gpu.renderer,
            device: ctx.gpu.device,
            queue: ctx.gpu.queue,
            config: ctx.config,
        };
        let resolver = resolver_mut(ctx.resources);
        let requests = collect_requests_of(ctx.world, ids, ctx.config, resolver, &sink);
        if requests.is_empty() {
            return;
        }
        // Each mesh's triangles kept for the colliders naming it (#1222)
        let mut recording = Recording {
            sink: &mut sink,
            colliders: &mut *ctx.colliders,
        };
        resolve(&requests, resolver, &mut recording, ctx.mode);
    }

    /// The frame-budgeted queue: what it finished goes to the renderer,
    /// when there is one. Living props move on after it, at
    /// `Stage::Procgen`, in whatever it left of the budget.
    fn frame(&self, ctx: &mut FrameMake) {
        flint_core::scope!("tick_procgen");
        use flint_core::frame_notes::{self, NoteKind};
        // Whether the queue made anything is known only after: timed by
        // hand, noted (#1077) when it did
        let began = frame_notes::listening().then(web_time::Instant::now);
        // A load's procgen stage too (#1092), forgotten when nothing came
        let span =
            flint_core::load_report::span(flint_core::load_report::Stage::Procgen, String::new);
        let resolver = resolver_mut(ctx.resources);
        let completed = resolver.process_frame(ctx.camera);
        let failures = resolver.take_failures();
        if completed.is_empty() && failures.is_empty() {
            span.discard();
            return;
        }
        let detail = began.map(|_| note(&completed, failures.len()));
        if let Some(gpu) = ctx.gpu.as_mut() {
            // Catalog names never reach the queue, so the file fallback
            // needs only the scene's own search paths.
            let config = ModelLoadConfig::in_project(ctx.project.clone(), ctx.scene_path);
            let mut sink = GpuSink {
                renderer: &mut *gpu.renderer,
                device: gpu.device,
                queue: gpu.queue,
                config: &config,
            };
            // Each mesh's triangles kept for the colliders naming it (#1222)
            let mut recording = Recording {
                sink: &mut sink,
                colliders: &mut *ctx.colliders,
            };
            apply_completed(&completed, &mut recording);
            apply_failures(&failures, &mut recording);
        }
        if let (Some(began), Some(detail)) = (began, detail) {
            let ms = began.elapsed().as_secs_f32() * 1000.0;
            frame_notes::note(NoteKind::Procgen, || detail, Some(ms));
        }
    }

    /// A model still on the queue (`MakeMode::Deferred`) has no triangles
    /// yet: they come when it's made, and physics waits for them quietly.
    fn pending(&self, resources: &Resources, name: &str) -> bool {
        resources
            .get::<ProcGenResolver>()
            .is_some_and(|resolver| resolver.is_pending(name))
    }

    fn scene_gone(&self, resources: &mut Resources) {
        if let Some(resolver) = resources.get_mut::<ProcGenResolver>() {
            resolver.clear_queue();
        }
    }
}

/// Give the resolver a disk tier (#181): rebuilt around it, its generators
/// moved across (built-ins and a game's alike, #195). Never rebuild it with
/// `with_config`, which drops a game's generators.
pub fn use_disk(resolver: &mut ProcGenResolver, disk: DiskCacheConfig) {
    let registry = std::mem::take(resolver.registry_mut());
    *resolver = ProcGenResolver::with_registry(
        ProcGenCacheConfig {
            disk: Some(disk),
            ..Default::default()
        },
        registry,
    );
}

/// Where the disk tier goes for a scene in `scene_dir` (#181):
/// `FLINT_PROCGEN_CACHE=<dir>` if set (`0`, `off`, `false`, `no`: no disk
/// tier), else `.flint/procgen-cache/` at the scene's project root
/// (`Project::root_for`, ADR 0114), so every scene of a project shares one
/// cache and the `.flint/` it makes marks the root that was already in use.
pub fn disk_config(
    project: &flint_core::project::Project,
    scene_dir: &std::path::Path,
) -> Option<DiskCacheConfig> {
    match flint_core::env::PROCGEN_CACHE.get() {
        Some(flint_core::env::Value::Off) => return None,
        Some(flint_core::env::Value::Path(dir)) => return Some(DiskCacheConfig::new(dir)),
        _ => {}
    }
    Some(DiskCacheConfig::in_project(&project.root_for(scene_dir)))
}

/// `2 generated: oak_tree, stone_wall; 1 failed`, for a frame note.
fn note(completed: &[CompletedAsset], failed: usize) -> String {
    let mut line = String::new();
    if !completed.is_empty() {
        let names: Vec<&str> = completed.iter().map(|c| c.spec_name.as_str()).collect();
        line = format!("{} generated: {}", completed.len(), names.join(", "));
    }
    if failed > 0 {
        if !line.is_empty() {
            line.push_str("; ");
        }
        line.push_str(&format!("{failed} failed"));
    }
    line
}
