//! The toy package's player part: what a launcher lists.

use crate::TOY_SPRING;
use flint_package::physics::rapier::math::Vector;
use flint_package::{FixedCtx, Lasts, Package, Registry, SceneCtx, Stage, StageCtx, Subsystem};
use flint_script::api::builder::kind::Int;
use flint_script::api::Module;
use flint_script::ScriptModule;

/// The `toy` package (`packages/toy/package.toml`).
pub static PACKAGE: Toy = Toy;

pub struct Toy;

impl Package for Toy {
    fn name(&self) -> &'static str {
        "toy"
    }

    /// Its builder and its script module, declared for every scene and
    /// enabled where a spring is.
    fn register(&self, registry: &mut Registry) {
        registry
            .scene_subsystem(for_scene)
            .script_modules(&[&TOY_MODULE]);
    }
}

/// One spring: the entity whose body it pulls, its stiffness and anchor.
struct Spring {
    entity: flint_core::EntityId,
    k: f32,
    anchor: [f32; 3],
}

/// The springs of a scene, and the fixed steps they have been pulled.
struct Springs {
    springs: Vec<Spring>,
    steps: i64,
}

/// What the module's functions read, set each frame.
#[derive(Default)]
struct Heard {
    steps: i64,
    springs: i64,
}

/// A subsystem for a scene with any `toy_spring`.
fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    let springs: Vec<Spring> = ctx
        .world
        .entities_with_component(TOY_SPRING)
        .iter()
        .map(|&entity| {
            let view = ctx.world.view(entity, TOY_SPRING);
            Spring {
                entity,
                k: view.f32("k", 40.0),
                anchor: view.vec3("anchor", [0.0; 3]),
            }
        })
        .collect();
    if springs.is_empty() {
        return None;
    }
    Some(Box::new(Springs { springs, steps: 0 }))
}

impl Subsystem for Springs {
    fn name(&self) -> &'static str {
        "toy"
    }

    fn lasts(&self) -> Lasts {
        Lasts::Scene
    }

    fn script_modules(&self) -> &[&'static str] {
        &["toy"]
    }

    fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {
        if stage == Stage::PreScripts {
            ctx.script.set_module_state(Heard {
                steps: self.steps,
                springs: self.springs.len() as i64,
            });
        }
    }

    /// Each spring's pull, from where its body is at this step, as an
    /// impulse of the step's length: Rapier's step, right after, integrates
    /// it (ADR 0137 section 3a).
    fn fixed_update(&mut self, ctx: &mut FixedCtx) {
        self.steps += 1;
        for spring in &self.springs {
            let Some(handle) = ctx.physics.body_of(spring.entity) else {
                continue;
            };
            let Some(body) = ctx.physics.bodies_mut().get_mut(handle) else {
                continue;
            };
            let at = *body.translation();
            let [ax, ay, az] = spring.anchor;
            let pull = Vector::new(ax - at.x, ay - at.y, az - at.z) * (spring.k * ctx.dt as f32);
            body.apply_impulse(pull, true);
        }
    }
}

struct ToyModule;

static TOY_MODULE: ToyModule = ToyModule;

impl ScriptModule for ToyModule {
    fn name(&self) -> &'static str {
        "toy"
    }

    fn enabled_when(&self) -> &'static str {
        "the scene has a toy_spring"
    }

    fn declare(&self, m: &mut Module<'_>) {
        m.func(
            "toy_steps",
            "The fixed physics steps the toy's springs have been pulled.",
        )
        .returns(Int, "steps")
        .run(|call, ()| call.ctx().module_state.get_mut::<Heard>().steps);
        m.func("toy_springs", "The springs in the scene.")
            .returns(Int, "springs")
            .run(|call, ()| call.ctx().module_state.get_mut::<Heard>().springs);
    }
}
