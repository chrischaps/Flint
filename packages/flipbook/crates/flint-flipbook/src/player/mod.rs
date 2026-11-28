//! The flipbook package's player part: what a launcher lists ([`PACKAGE`]).
//! It puts the flipbook kind on the animation package's `ClipKinds` point,
//! so each scene's animation (and a capture's) loads the scene's
//! `.sprite.toml` clips and plays them, and it declares the `flipbook`
//! script module, which a small subsystem enables in every scene.

mod module;

use crate::kind::FlipbookKind;
use crate::PACKAGE_NAME;
use flint_animation::player::loading::visible_files;
use flint_animation::player::ClipKinds;
use flint_animation::ClipKind;
use flint_core::components as comp;
use flint_core::project::{AssetKind, Project};
use flint_package::{Package, Registry, SceneCtx, Subsystem};

pub use module::{FlipbookModule, FLIPBOOK_MODULE};

/// The `flipbook` package (`packages/flipbook/package.toml`).
pub static PACKAGE: FlipbookPackage = FlipbookPackage;

pub struct FlipbookPackage;

impl Package for FlipbookPackage {
    fn name(&self) -> &'static str {
        PACKAGE_NAME
    }

    /// Its kind on animation's point (animation registers first: flipbook
    /// requires it), the subsystem that enables its module, its module and
    /// its line for a project that leaves it out.
    fn register(&self, registry: &mut Registry) {
        match registry.point_mut::<ClipKinds>() {
            Some(kinds) => kinds.0.push((crate::kind::KIND, make)),
            None => tracing::warn!(
                "flipbook: the animation package isn't in this run, so sprites play no clips"
            ),
        }
        registry
            .scene_subsystem(for_scene)
            .when_left_out(left_out)
            .script_modules(&[&FLIPBOOK_MODULE]);
    }
}

/// The scene's flipbooks: every `.sprite.toml` it can see, from the same
/// folders as its `.anim.toml` (the farthest first, so the nearest file's
/// clip of a name wins). Each file may hold several clips under
/// `[animation.NAME]`.
fn make(project: &Project, scene_path: &str, say: &mut dyn FnMut(String)) -> Box<dyn ClipKind> {
    let mut kind = FlipbookKind::new();
    for path in visible_files(project, AssetKind::SpriteAnimations, scene_path) {
        match crate::clip::load_sprite_clips_from_file(&path) {
            Ok(clips) => {
                for clip in clips {
                    say(format!(
                        "Loaded sprite animation: {} ({}ms, {} frames)",
                        clip.name,
                        clip.total_duration_ms(),
                        clip.frames.len()
                    ));
                    kind.add_clip(clip);
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to load sprite animation '{}': {:?}",
                    path.display(),
                    e
                );
            }
        }
    }
    Box::new(kind)
}

/// The subsystem that enables the `flipbook` module in every scene (its
/// clips play in the animation subsystem, as a kind).
struct Flipbooks;

impl Subsystem for Flipbooks {
    fn name(&self) -> &'static str {
        PACKAGE_NAME
    }

    fn script_modules(&self) -> &[&'static str] {
        &[module::MODULE]
    }
}

fn for_scene(_ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    Some(Box::new(Flipbooks))
}

/// What a scene with flipbooks hears when the project's `[packages]`
/// leaves `flipbook` out: a sprite with an animator, or the retired
/// `sprite_animator`.
fn left_out(ctx: &SceneCtx) -> Option<String> {
    let in_use = ctx.packages?;
    let animated = |c: &str| {
        ctx.world.entities_with_component(c).iter().any(|&id| {
            ctx.world
                .get_components(id)
                .is_some_and(|comps| comps.has(comp::SPRITE))
        })
    };
    let uses: Vec<&str> = [comp::ANIMATOR, comp::SPRITE_ANIMATOR]
        .into_iter()
        .filter(|c| animated(c))
        .collect();
    flint_package::left_out_line(in_use, PACKAGE_NAME, &uses, "show their first frame")
}
