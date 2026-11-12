//! Engine component name constants.
//!
//! Use these instead of bare string literals so the compiler catches typos.
//! A feature crate owns the names of its own components (#754):
//! `flint_geometry::components` (ocean, water body, splines) and
//! `flint_wooze::components` (shallow wooze), so adding one rebuilds that
//! crate's readers, not the engine. `LIVING_PROP` and `MUSIC_SESSION` stay
//! here: a player built without living props or music still names them to
//! say so (`flint-player`'s `missing_features.rs`).

// --- Spatial ---
pub const TRANSFORM: &str = "transform";

// --- Rendering ---
pub const MATERIAL: &str = "material";
/// Per-glTF-material base-colour overrides (`{ <material name> = [r,g,b(,a)] }`),
/// honoured on the entity or any ancestor. See `SceneRenderer::ecs_material_override`.
pub const MATERIAL_OVERRIDES: &str = "material_overrides";
pub const MODEL: &str = "model";
pub const BOUNDS: &str = "bounds";
pub const SPRITE: &str = "sprite";
pub const LIGHT: &str = "light";
/// `visibility { visible = false }` keeps an entity and its descendants out of
/// every draw, shadow, light and ray target (#628). See `FlintWorld::hidden_entities`.
pub const VISIBILITY: &str = "visibility";

// --- Scripting & interaction ---
pub const SCRIPT: &str = "script";
pub const INTERACTABLE: &str = "interactable";

// --- Animation ---
pub const ANIMATOR: &str = "animator";
pub const SKELETON: &str = "skeleton";
pub const SPRITE_ANIMATOR: &str = "sprite_animator";
pub const IK_TWO_BONE: &str = "ik_two_bone";
pub const LIVING_PROP: &str = "living_prop";

// --- Physics ---
pub const RIGIDBODY: &str = "rigidbody";
pub const COLLIDER: &str = "collider";
pub const JOINT: &str = "joint";
pub const CHARACTER_CONTROLLER: &str = "character_controller";

// --- Audio ---
pub const AUDIO_SOURCE: &str = "audio_source";
pub const AUDIO_TRIGGER: &str = "audio_trigger";
/// A scene's suite (#745): its stems play on their buses, sample-locked, and
/// scripts read the beat; with a `chart`, the rhythm layer judges it too.
/// Read by flint-player's music subsystem.
pub const MUSIC_SESSION: &str = "music_session";

// --- Particles ---
pub const PARTICLE_EMITTER: &str = "particle_emitter";
pub const PARTICLE_EFFECT: &str = "particle_effect";

// --- Terrain ---
pub const TERRAIN: &str = "terrain";
/// Compute grass on a game's own ground: a mask over a rectangle (ADR 0089).
pub const GRASS_GROUND: &str = "grass_ground";

// --- Skeletal probes ---
pub const BONE_PROBE: &str = "bone_probe";

// --- Procedural sky ---
pub const SKY: &str = "sky";

// --- UI ---
pub const SCREEN_ANCHOR: &str = "screen_anchor";
pub const UI_TEXT: &str = "ui_text";
pub const UI_FILL: &str = "ui_fill";
