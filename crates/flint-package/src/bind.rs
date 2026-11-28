//! A package's share of a scene the renderer binds (#1623, ADR 0138
//! decision 5).
//!
//! `SceneRenderer::bind_scene` draws only the plain data it is handed. A
//! package whose component the renderer draws builds it here, right after
//! the bind and before the scene's models, in every host that binds a scene:
//! the player (`stage_scene`), the scene viewer and `flint render`. The
//! terrain package is the one so far: its chunks, material and grass, and
//! the ground it hands back ([`SceneGround`]), which the player gives
//! physics' heightfield and the terrain's own `terrain_height`.
//!
//! Not an [`AssetMaker`](crate::assets::AssetMaker): a maker runs only where
//! a host makes assets (the player, and `flint render` only with living
//! props or `--procgen`), and it is lent no [`BoundScene`], whose grass a
//! terrain's yields to a `grass_ground`'s.

use crate::assets::Gpu;
use flint_core::package::InUse;
use flint_ecs::FlintWorld;
pub use flint_geometry::ground::SceneGround;
use flint_render::BoundScene;

/// What a binder is lent: the renderer the scene was just bound into, the
/// world, and what the bind made of it (its grass, which a binder's yields
/// to and fills).
pub struct BindCtx<'a> {
    pub gpu: Gpu<'a>,
    pub world: &'a FlintWorld,
    pub bound: &'a mut BoundScene,
}

/// A package's share of the bind: what it built into the renderer, and the
/// scene's ground when it built one.
pub type BindScene = fn(&mut BindCtx) -> Option<SceneGround>;

/// The binders a host that has no player registry asks (the scene viewer,
/// through `flint_package::cli::points()`): each with its package's name,
/// in the order the packages registered. The CLI fills it from the
/// player's registry.
#[derive(Default, Clone)]
pub struct SceneBinders(pub Vec<(&'static str, BindScene)>);

impl SceneBinders {
    /// Every binder of a package `packages` lets in, run in turn on `ctx`;
    /// the last ground one hands back.
    pub fn bind(&self, packages: Option<&InUse>, ctx: &mut BindCtx) -> Option<SceneGround> {
        bind_with(self.0.iter().copied(), packages, ctx)
    }
}

/// `binders` of the packages `packages` lets in (`None`: every one), run in
/// turn on `ctx`; the last ground one hands back.
pub fn bind_with(
    binders: impl IntoIterator<Item = (&'static str, BindScene)>,
    packages: Option<&InUse>,
    ctx: &mut BindCtx,
) -> Option<SceneGround> {
    let mut ground = None;
    for (package, bind) in binders {
        if packages.is_some_and(|in_use| !in_use.is_on(package)) {
            continue;
        }
        if let Some(made) = bind(ctx) {
            ground = Some(made);
        }
    }
    ground
}
