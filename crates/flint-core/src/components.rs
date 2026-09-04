//! Engine component name constants.
//!
//! Use these instead of bare string literals so the compiler catches typos.

// --- Spatial ---
pub const TRANSFORM: &str = "transform";

// --- Rendering ---
pub const MATERIAL: &str = "material";
pub const MODEL: &str = "model";
pub const BOUNDS: &str = "bounds";
pub const SPRITE: &str = "sprite";
pub const LIGHT: &str = "light";

// --- Scripting & interaction ---
pub const SCRIPT: &str = "script";
pub const INTERACTABLE: &str = "interactable";
pub const HEALTH: &str = "health";

// --- Animation ---
pub const ANIMATOR: &str = "animator";
pub const SKELETON: &str = "skeleton";
pub const SPRITE_ANIMATOR: &str = "sprite_animator";

// --- Physics ---
pub const RIGIDBODY: &str = "rigidbody";
pub const COLLIDER: &str = "collider";
pub const CHARACTER_CONTROLLER: &str = "character_controller";
pub const PLAYER: &str = "player";

// --- Audio ---
pub const AUDIO_SOURCE: &str = "audio_source";
pub const AUDIO_TRIGGER: &str = "audio_trigger";

// --- Particles ---
pub const PARTICLE_EMITTER: &str = "particle_emitter";
pub const PARTICLE_EFFECT: &str = "particle_effect";

// --- Terrain ---
pub const TERRAIN: &str = "terrain";

// --- Ocean ---
pub const OCEAN: &str = "ocean";
pub const OCEAN_CONTACT: &str = "ocean_contact";

// --- Skeletal probes ---
pub const BONE_PROBE: &str = "bone_probe";

// --- Procedural sky ---
pub const SKY: &str = "sky";

// --- Splines ---
pub const SPLINE: &str = "spline";
pub const SPLINE_DATA: &str = "spline_data";
pub const SPLINE_MESH: &str = "spline_mesh";
pub const SPLINE_CHUNK: &str = "spline_chunk";

// --- UI ---
pub const SCREEN_ANCHOR: &str = "screen_anchor";
pub const UI_TEXT: &str = "ui_text";
pub const UI_FILL: &str = "ui_fill";
