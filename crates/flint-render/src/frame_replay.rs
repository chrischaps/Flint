//! What a captured frame needs besides the world to be drawn again (#1086,
//! ADR 0126 decision 4): the state the renderer holds that the world doesn't.
//!
//! - **Skinning, from the GPU.** Every entity's bone palette, shadow palette
//!   and echo palettes with their opacities. Living props write their bones
//!   with `write_entity_bones`, which keeps no CPU copy, so the palettes are
//!   copied out of the bone buffers (made `COPY_SRC` for it) into staging
//!   buffers recorded in the frame's own encoder, after the frame's draws, and
//!   read back as [`FrameCapture`](crate::frame_capture::FrameCapture)'s
//!   images are: mapped once the frame is submitted, polled on the frames
//!   after, never waited on by a window. An asset's shared palette (an entity
//!   that never had its own) is kept on the CPU already and is copied from
//!   there.
//! - **What the renderer keeps across frames**, as data: motion holds' kept
//!   matrices and ticks (a fresh `MotionHold` would draw the live matrix),
//!   part tints, softness and which entities' echoes only lighten, the drawn
//!   config's style settings, the quality, MSAA, render scale, debug mode,
//!   the selected entity and whether the floor grid was drawn.
//!
//! Everything is keyed by entity **name** (ADR 0126: ids move between a
//! captured world and its replay). A host starts one with
//! [`SceneRenderer::capture_replay_state`] after the frame's last draw is
//! recorded, and takes the palettes with [`ReplayStateCapture::read`] once
//! [`ReplayStateCapture::poll`] says they are mapped. Nothing here runs on a
//! frame nobody captures.
//!
//! The files a host writes from it, and their layout, are in
//! [`ReplayRenderer`]'s and [`PaletteEntry`]'s docs; `inputs.json` holds the
//! [`ReplayRenderer`] beside the host's own inputs.
//!
//! [`SceneRenderer::capture_replay_state`]: crate::SceneRenderer::capture_replay_state

use crate::gpu_counts::CountedDevice;
use flint_ecs::FlintWorld;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

/// One bone matrix, column-major as the shaders read it.
pub type BoneMatrix = [[f32; 4]; 4];

/// Which palette of an entity a [`PaletteEntry`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PaletteKind {
    /// The bones it is drawn with (`write_entity_bones`, `update_bone_matrices`).
    #[default]
    Bones,
    /// The bones its shadow is drawn with, when they differ.
    Shadow,
    /// One of a living prop's echoes (`write_entity_echo_bones`).
    Echo,
}

/// One palette in `replay/skinning.bin`: `count` matrices of 16 little-endian
/// f32 (64 bytes each, column-major), starting `offset` bytes in. The file is
/// the palettes one after another, in this index's order, with nothing
/// between; the index is `inputs.json`'s `renderer.skinning`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaletteEntry {
    pub entity: String,
    pub kind: PaletteKind,
    /// An echo's place in its entity's echoes, in draw order.
    pub echo: Option<usize>,
    /// An echo's opacity.
    pub opacity: Option<f32>,
    pub offset: u64,
    /// Matrices in it: the whole buffer (`MAX_BONES` for an entity's own).
    pub count: u32,
}

/// A motion hold's kept matrix for one entity (#405).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeldMatrix {
    pub entity: String,
    pub tick: i64,
    pub rate: f32,
    pub matrix: BoneMatrix,
}

/// The tick an entity's bones were last written on under a hold.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeldBones {
    pub entity: String,
    pub tick: i64,
    pub rate: f32,
}

/// What the renderer's motion hold kept (#405).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MotionHoldState {
    /// The post time stamped this frame, `None` when no host stamped one.
    pub time: Option<f32>,
    /// Who is held, at what rate.
    pub rates: BTreeMap<String, f32>,
    pub matrices: Vec<HeldMatrix>,
    pub bones: Vec<HeldBones>,
}

/// One style's settings as the frame drew them (`StyleSetting`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StyleState {
    pub enabled: bool,
    pub params: BTreeMap<String, f32>,
    pub options: BTreeMap<String, String>,
    pub order: Option<f32>,
    pub feeds_bloom: Option<bool>,
    pub blend: Option<f32>,
    pub follows_tear: bool,
    pub off_everywhere: bool,
    /// Its reveal region, as Rust prints it, when it has one (not yet in a
    /// form a replay reads back: `fresh_only` says so).
    pub reveal: Option<String>,
}

/// The renderer's share of `replay/inputs.json` (its `renderer` table).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplayRenderer {
    /// The drawn config's styles by name (`effective_post_process_config`,
    /// scripts' overrides and the quality caps laid). Its flat keys are the
    /// manifest's `frame.post_process`.
    pub styles: BTreeMap<String, StyleState>,
    /// The drawn config whole, as Rust prints it: every field, the
    /// renderer's own ones (ground fog, the boil) among them, for a reader
    /// that needs one the structured fields leave out.
    pub post_process_debug: String,
    /// Each quality setting as `--quality` writes it.
    pub quality: BTreeMap<String, String>,
    pub msaa: u32,
    pub render_scale: f32,
    pub debug_mode: String,
    /// The entity the selection outline draws round, by name.
    pub selected: Option<String>,
    /// Whether the floor grid was drawn (`set_show_grid`; a still `flint
    /// render` draws it unless `--no-grid`, #1211). A capture from before
    /// this field reads `false`, as its replay always drew.
    pub grid: bool,
    pub motion_hold: MotionHoldState,
    /// The palette index of `replay/skinning.bin` ([`PaletteEntry`]).
    pub skinning: Vec<PaletteEntry>,
    /// Each skinned asset's shared palette (what an entity without its own
    /// buffer draws with), by asset name: kept on the CPU, so copied, not
    /// read back. Empty unless a draw of the frame binds one.
    pub asset_bones: BTreeMap<String, Vec<BoneMatrix>>,
    /// Each entity's part tints (`set_entity_part_tints`).
    pub part_tints: BTreeMap<String, Vec<[f32; 3]>>,
    /// Each entity's part softness (#582).
    pub part_softness: BTreeMap<String, Vec<f32>>,
    /// The entities whose echoes may only add light.
    pub echo_lighten: Vec<String>,
    /// Each style's frame-to-frame history (#1136): its parts, their keys
    /// and the files under `replay/` that hold their textures
    /// ([`crate::stylize::history`]).
    pub style_history: Vec<crate::stylize::history::StyleHistoryState>,
}

/// A palette copied in the frame's encoder, not yet read.
struct Staged {
    entry: PaletteEntry,
    buffer: wgpu::Buffer,
}

/// The renderer's replay state from one frame: the CPU part at once, the
/// palettes once read back. See the module docs.
pub struct ReplayStateCapture {
    /// Everything but `skinning`, which [`read`](Self::read) fills.
    pub renderer: ReplayRenderer,
    staged: Vec<Staged>,
    /// The styles' history textures (#1136), copied beside the palettes.
    history: Vec<crate::stylize::history::StagedHistory>,
    mapped: Arc<AtomicUsize>,
    failed: Arc<AtomicBool>,
    mapping: bool,
}

/// The palettes read back: `skinning.bin`'s bytes and its index; and the
/// styles' history textures (#1136), each file under `replay/` with its
/// bytes (`history/ascii.tone.cells.bin`).
pub struct ReadPalettes {
    pub index: Vec<PaletteEntry>,
    pub bytes: Vec<u8>,
    pub history: Vec<(String, Vec<u8>)>,
}

impl ReplayStateCapture {
    /// Bytes held in staging until read.
    pub fn staging_bytes(&self) -> u64 {
        self.staged.iter().map(|s| s.buffer.size()).sum::<u64>()
            + self.history.iter().map(|s| s.buffer.size()).sum::<u64>()
    }

    /// Ask for every palette to be mapped. Call once the frame is submitted.
    pub fn map(&mut self) {
        if self.mapping {
            return;
        }
        self.mapping = true;
        let buffers = self
            .staged
            .iter()
            .map(|s| &s.buffer)
            .chain(self.history.iter().map(|s| &s.buffer));
        for buffer in buffers {
            let (mapped, failed) = (self.mapped.clone(), self.failed.clone());
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    if result.is_err() {
                        failed.store(true, Ordering::Relaxed);
                    }
                    mapped.fetch_add(1, Ordering::Release);
                });
        }
    }

    /// Whether every palette is mapped.
    pub fn ready(&self) -> bool {
        self.mapping
            && self.mapped.load(Ordering::Acquire) == self.staged.len() + self.history.len()
    }

    /// Run the device's callbacks without waiting; whether it's ready.
    pub fn poll(&mut self, device: &wgpu::Device) -> bool {
        self.map();
        if !self.ready() {
            let _ = device.poll(wgpu::PollType::Poll);
        }
        self.ready()
    }

    /// Wait for the palettes: for a host with no frame to keep.
    pub fn wait(&mut self, device: &wgpu::Device) {
        self.map();
        while !self.ready() {
            let _ = crate::wait_for_gpu(device);
        }
    }

    /// Copy the palettes out of mapped memory. `Err` while one isn't mapped
    /// or when one failed to map.
    pub fn read(self) -> Result<(ReplayRenderer, ReadPalettes), String> {
        if !self.ready() {
            return Err("the bone palettes aren't mapped yet: poll until ready".into());
        }
        if self.failed.load(Ordering::Relaxed) {
            return Err("a bone palette failed to map".into());
        }
        let mut bytes = Vec::with_capacity(self.staging_bytes() as usize);
        let mut index = Vec::with_capacity(self.staged.len());
        for s in self.staged {
            let data = s
                .buffer
                .slice(..)
                .get_mapped_range()
                .map_err(|e| format!("bone palette of {}: {e}", s.entry.entity))?;
            let mut entry = s.entry;
            entry.offset = bytes.len() as u64;
            entry.count = (data.len() / std::mem::size_of::<BoneMatrix>()) as u32;
            bytes.extend_from_slice(&data);
            drop(data);
            s.buffer.unmap();
            index.push(entry);
        }
        let mut history = Vec::with_capacity(self.history.len());
        for s in self.history {
            let data = s
                .buffer
                .slice(..)
                .get_mapped_range()
                .map_err(|e| format!("style history {}: {e}", s.file))?;
            history.push((s.file.clone(), s.unpadded(&data)));
            drop(data);
            s.buffer.unmap();
        }
        let mut renderer = self.renderer;
        renderer.skinning = index.clone();
        Ok((
            renderer,
            ReadPalettes {
                index,
                bytes,
                history,
            },
        ))
    }
}

impl ReplayStateCapture {
    /// A capture holding `renderer` and no palettes yet.
    pub(crate) fn new(renderer: ReplayRenderer) -> Self {
        Self {
            renderer,
            staged: Vec::new(),
            history: Vec::new(),
            mapped: Arc::new(AtomicUsize::new(0)),
            failed: Arc::new(AtomicBool::new(false)),
            mapping: false,
        }
    }

    /// Keep the styles' history (#1136): their states, written into
    /// `inputs.json`, and the copies of their textures staged in the
    /// frame's encoder.
    pub(crate) fn stage_history(
        &mut self,
        states: Vec<crate::stylize::history::StyleHistoryState>,
        staged: Vec<crate::stylize::history::StagedHistory>,
    ) {
        self.renderer.style_history = states;
        self.history = staged;
    }

    /// Copy `source` into a staging buffer of its own in `encoder`, as
    /// `entry`'s palette. A buffer made without `COPY_SRC` is skipped.
    pub(crate) fn stage(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        entry: PaletteEntry,
        source: &wgpu::Buffer,
    ) {
        if !source.usage().contains(wgpu::BufferUsages::COPY_SRC) {
            return;
        }
        let buffer = device.create_buffer_counted(&wgpu::BufferDescriptor {
            label: Some("Frame Capture Bones"),
            size: source.size(),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(source, 0, &buffer, 0, source.size());
        self.staged.push(Staged { entry, buffer });
    }
}

// ── The host's inputs, as data ──
//
// `FrameInputs` and `Camera` aren't serde, and ADR 0126 keeps them so: the
// capture has its own types, made from them with `of`.

/// One entity's shallow wooze drift ([`ShallowWoozeDraw`](crate::ShallowWoozeDraw)).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WoozeDrawState {
    /// Who drifts: `None` for woozy grass's comb.
    pub entity: Option<String>,
    pub amplitude: f32,
    pub wavelength: f32,
    pub index: u64,
    pub weight: f32,
}

impl WoozeDrawState {
    fn of(entity: Option<String>, d: &crate::ShallowWoozeDraw) -> Self {
        Self {
            entity,
            amplitude: d.amplitude,
            wavelength: d.wavelength,
            index: d.index,
            weight: d.weight,
        }
    }
}

/// A depth-tested script line (`flint_core::world_line::WorldLine`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldLineState {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub color: [f32; 4],
    pub thickness: f32,
}

/// A host's sea tint over the `ocean` component's (`OceanTint`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OceanTintState {
    pub amount: f32,
    pub deep: [f32; 3],
    pub shallow: [f32; 3],
    pub sss: [f32; 3],
    pub foam: [f32; 3],
    pub sky: [f32; 3],
    pub ramp_steps: f32,
    pub band_wobble: f32,
    pub pools: f32,
    pub keep_light: bool,
    pub below: bool,
}

/// Every field of the frame's [`FrameInputs`](crate::FrameInputs), as data:
/// `inputs.json`'s `frame` table.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FrameInputsState {
    pub time: f64,
    pub dt: f32,
    pub look_time: Option<f64>,
    pub grass_time: Option<f32>,
    pub grass_wooze: Option<WoozeDrawState>,
    pub shallow_wooze: Vec<WoozeDrawState>,
    /// The boil: `seed`, `amount`, `step`.
    pub boil_seed: u32,
    pub boil_amount: f32,
    pub boil_step: f32,
    pub pixels_per_point: f32,
    pub view_2d_offset: [f32; 2],
    pub view_2d_ortho_height: f32,
    pub view_2d_aspect_ratio: f32,
    /// Whether particles were drawn: they are `particles.bin` (see
    /// [`ParticlesState`]).
    pub particles: bool,
    pub world_lines: Vec<WorldLineState>,
    /// What bent the grass, each a position.
    pub grass_benders: Vec<[f32; 3]>,
    /// Each `ocean_contact` hull's velocity given this frame, by name.
    pub contact_velocities: BTreeMap<String, [f32; 3]>,
    pub ocean_tint: Option<OceanTintState>,
    /// The screen field the game filled (#859), each shape tagged by kind.
    pub style_field: Vec<flint_core::style_field::FieldShape>,
}

impl FrameInputsState {
    /// `inputs` as data, its entities named from `world`.
    pub fn of(inputs: &crate::FrameInputs<'_>, world: &FlintWorld) -> Self {
        let name = |id: flint_core::EntityId| entity_name(world, id);
        Self {
            time: inputs.time,
            dt: inputs.dt,
            look_time: inputs.look_time,
            grass_time: inputs.grass_time,
            grass_wooze: inputs
                .grass_wooze
                .as_ref()
                .map(|d| WoozeDrawState::of(None, d)),
            shallow_wooze: inputs
                .shallow_wooze
                .iter()
                .map(|(id, d)| WoozeDrawState::of(Some(name(*id)), d))
                .collect(),
            boil_seed: inputs.boil.seed,
            boil_amount: inputs.boil.amount,
            boil_step: inputs.boil.step,
            pixels_per_point: inputs.pixels_per_point,
            view_2d_offset: inputs.view_2d.camera_offset,
            view_2d_ortho_height: inputs.view_2d.ortho_height,
            view_2d_aspect_ratio: inputs.view_2d.aspect_ratio,
            particles: inputs.particles.is_some(),
            world_lines: inputs
                .world_lines
                .iter()
                .map(|l| WorldLineState {
                    a: l.a,
                    b: l.b,
                    color: l.color,
                    thickness: l.thickness,
                })
                .collect(),
            grass_benders: inputs.grass_benders.iter().map(|b| b.position).collect(),
            contact_velocities: inputs
                .contact_velocities
                .iter()
                .map(|(id, v)| (name(*id), *v))
                .collect(),
            ocean_tint: inputs.ocean_tint.map(|t| OceanTintState {
                amount: t.amount,
                deep: t.deep,
                shallow: t.shallow,
                sss: t.sss,
                foam: t.foam,
                sky: t.sky,
                ramp_steps: t.ramp_steps,
                band_wobble: t.band_wobble,
                pools: t.pools,
                keep_light: t.keep_light,
                below: t.below,
            }),
            style_field: inputs.style_field.to_vec(),
        }
    }
}

/// [`FrameInputsState`] made into what [`FrameInputs`](crate::FrameInputs)
/// borrows (#1089): its entities found by name in a replay's world. Take
/// the frame's inputs with [`inputs`](Self::inputs).
pub struct OwnedFrameInputs {
    pub state: FrameInputsState,
    pub shallow_wooze: Vec<(flint_core::EntityId, crate::ShallowWoozeDraw)>,
    pub world_lines: Vec<flint_core::world_line::WorldLine>,
    pub grass_benders: Vec<crate::GrassEntityPosition>,
    pub contact_velocities: Vec<(flint_core::EntityId, [f32; 3])>,
    /// The names the replay's world has no entity of.
    pub unknown: Vec<String>,
}

impl OwnedFrameInputs {
    /// The frame's inputs, particles none (a replay uploads its own).
    pub fn inputs(&self) -> crate::FrameInputs<'_> {
        let s = &self.state;
        crate::FrameInputs {
            time: s.time,
            dt: s.dt,
            look_time: s.look_time,
            grass_time: s.grass_time,
            grass_wooze: s.grass_wooze.as_ref().map(WoozeDrawState::draw),
            shallow_wooze: &self.shallow_wooze,
            boil: flint_core::field_frame::BoilFrame {
                seed: s.boil_seed,
                amount: s.boil_amount,
                step: s.boil_step,
            },
            pixels_per_point: s.pixels_per_point,
            view_2d: crate::View2d {
                camera_offset: s.view_2d_offset,
                ortho_height: s.view_2d_ortho_height,
                aspect_ratio: s.view_2d_aspect_ratio,
            },
            particles: None,
            world_lines: &self.world_lines,
            grass_benders: &self.grass_benders,
            contact_velocities: &self.contact_velocities,
            ocean_tint: s
                .ocean_tint
                .as_ref()
                .map(|t| crate::ocean_pipeline::OceanTint {
                    amount: t.amount,
                    deep: t.deep,
                    shallow: t.shallow,
                    sss: t.sss,
                    foam: t.foam,
                    sky: t.sky,
                    ramp_steps: t.ramp_steps,
                    band_wobble: t.band_wobble,
                    pools: t.pools,
                    keep_light: t.keep_light,
                    below: t.below,
                }),
            draw_controls: None,
            style_field: &s.style_field,
        }
    }
}

impl WoozeDrawState {
    fn draw(&self) -> crate::ShallowWoozeDraw {
        crate::ShallowWoozeDraw {
            amplitude: self.amplitude,
            wavelength: self.wavelength,
            index: self.index,
            weight: self.weight,
        }
    }
}

impl FrameInputsState {
    /// The inputs again, for `world` (#1089): the reverse of
    /// [`of`](Self::of), entities by name.
    pub fn owned(&self, world: &FlintWorld) -> OwnedFrameInputs {
        let mut unknown = Vec::new();
        let mut id = |name: &str| {
            let id = world.get_id(name);
            if id.is_none() {
                unknown.push(name.to_string());
            }
            id
        };
        let shallow_wooze = self
            .shallow_wooze
            .iter()
            .filter_map(|d| Some((id(d.entity.as_deref()?)?, d.draw())))
            .collect();
        let contact_velocities = self
            .contact_velocities
            .iter()
            .filter_map(|(name, v)| Some((id(name)?, *v)))
            .collect();
        OwnedFrameInputs {
            state: self.clone(),
            shallow_wooze,
            world_lines: self
                .world_lines
                .iter()
                .map(|l| flint_core::world_line::WorldLine {
                    a: l.a,
                    b: l.b,
                    color: l.color,
                    thickness: l.thickness,
                })
                .collect(),
            grass_benders: self
                .grass_benders
                .iter()
                .map(|&position| crate::GrassEntityPosition {
                    position,
                    _pad: 0.0,
                })
                .collect(),
            contact_velocities,
            unknown,
        }
    }
}

/// Every [`Camera`](crate::Camera) field: `inputs.json`'s `camera` table.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraState {
    pub position: [f32; 3],
    pub target: [f32; 3],
    pub up: [f32; 3],
    pub fov: f32,
    pub near: f32,
    pub far: f32,
    pub aspect: f32,
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
    /// `orbit` or `first_person`.
    pub mode: String,
    pub orthographic: bool,
    pub ortho_height: f32,
    pub ndc_offset: [f32; 2],
}

impl CameraState {
    pub fn of(c: &crate::Camera) -> Self {
        let v = |v: flint_core::Vec3| [v.x, v.y, v.z];
        Self {
            position: v(c.position),
            target: v(c.target),
            up: v(c.up),
            fov: c.fov,
            near: c.near,
            far: c.far,
            aspect: c.aspect,
            distance: c.distance,
            yaw: c.yaw,
            pitch: c.pitch,
            mode: match c.mode {
                crate::CameraMode::Orbit => "orbit",
                crate::CameraMode::FirstPerson => "first_person",
            }
            .to_string(),
            orthographic: c.orthographic,
            ortho_height: c.ortho_height,
            ndc_offset: c.ndc_offset,
        }
    }
}

impl CameraState {
    /// The camera again (#1089).
    pub fn camera(&self) -> crate::Camera {
        let v = |a: [f32; 3]| flint_core::Vec3::new(a[0], a[1], a[2]);
        crate::Camera {
            position: v(self.position),
            target: v(self.target),
            up: v(self.up),
            fov: self.fov,
            near: self.near,
            far: self.far,
            aspect: self.aspect,
            distance: self.distance,
            yaw: self.yaw,
            pitch: self.pitch,
            mode: match self.mode.as_str() {
                "first_person" => crate::CameraMode::FirstPerson,
                _ => crate::CameraMode::Orbit,
            },
            orthographic: self.orthographic,
            ortho_height: self.ortho_height,
            ndc_offset: self.ndc_offset,
        }
    }
}

/// `particles.bin` read back (#1089): its instances, and each emitter's
/// draw as the renderer uploads it.
pub struct ReplayParticles {
    pub instances: Vec<flint_particles::ParticleInstance>,
    pub emitters: Vec<EmitterDraw>,
}

impl ReplayParticles {
    /// The stream `bytes` (as `particles.bin` holds it) and the draws
    /// `emitters` index into it. An emitter past the stream's end is left
    /// out.
    pub fn read(bytes: &[u8], emitters: &[EmitterDraw]) -> Self {
        let size = std::mem::size_of::<flint_particles::ParticleInstance>();
        let whole = bytes.len() / size * size;
        Self {
            instances: bytemuck::pod_collect_to_vec(&bytes[..whole]),
            emitters: emitters.to_vec(),
        }
    }

    /// What [`SceneRenderer::update_particles`](crate::SceneRenderer::update_particles) takes.
    pub fn draws(&self) -> Vec<crate::ParticleDrawData<'_>> {
        use flint_particles::ParticleBlendMode as B;
        self.emitters
            .iter()
            .filter_map(|e| {
                let (first, count) = (e.first as usize, e.count as usize);
                Some(crate::ParticleDrawData {
                    instances: self.instances.get(first..first + count)?,
                    texture: &e.texture,
                    blend: match e.blend.as_str() {
                        "Additive" => B::Additive,
                        "Premultiplied" => B::Premultiplied,
                        "Multiply" => B::Multiply,
                        _ => B::Alpha,
                    },
                    sort_key: e.sort_key,
                })
            })
            .collect()
    }
}

/// One emitter's draw in `replay/particles.bin`: `count` packed
/// `ParticleInstance`s (16 little-endian f32 each: `pos_size`, `color`,
/// `rotation_frame`, `vel_stretch`) from instance `first`, with the draw
/// data the renderer took with them. The file is the frame's whole packed
/// stream, camera-sorted as drawn.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmitterDraw {
    /// The emitting entity by name; `None` for a detached burst.
    pub entity: Option<String>,
    pub emitter: String,
    pub emitter_index: usize,
    pub first: u32,
    pub count: u32,
    /// The blend, as Rust prints it (`Alpha`, `Additive`...).
    pub blend: String,
    pub texture: String,
    pub frames: [u32; 2],
    pub sort_key: f32,
    pub soft_distance: f32,
    pub fade_near: f32,
    pub fade_far: f32,
    pub lighting: f32,
    pub fog: bool,
    pub stretch: f32,
}

/// The frame's particles: the packed stream and each emitter's draw.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParticlesState {
    pub emitters: Vec<EmitterDraw>,
    /// `particles.bin`'s bytes.
    pub bytes: Vec<u8>,
}

impl ParticlesState {
    /// What `sync` packed for this frame, named from `world`.
    pub fn of(sync: &flint_particles::ParticleSync, world: &FlintWorld) -> Self {
        let mut out = Self::default();
        for d in sync.draw_data() {
            let first =
                (out.bytes.len() / std::mem::size_of::<flint_particles::ParticleInstance>()) as u32;
            out.bytes
                .extend_from_slice(bytemuck::cast_slice(d.instances));
            out.emitters.push(EmitterDraw {
                entity: d.entity_id.map(|id| entity_name(world, id)),
                emitter: d.emitter_name.to_string(),
                emitter_index: d.emitter_index,
                first,
                count: d.instances.len() as u32,
                blend: format!("{:?}", d.blend_mode),
                texture: d.texture.to_string(),
                frames: [d.frames_x, d.frames_y],
                sort_key: d.sort_key,
                soft_distance: d.soft_distance,
                fade_near: d.fade_near,
                fade_far: d.fade_far,
                lighting: d.lighting,
                fog: d.fog,
                stretch: d.stretch,
            });
        }
        out
    }
}

/// `replay/inputs.json`: what the host gave the frame, and what the
/// renderer held.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplayInputs {
    /// [`FrameInputsState`]'s version: 1.
    pub version: u32,
    pub frame: FrameInputsState,
    /// The camera, in a capture written before #1176. Since, it is the
    /// manifest's `camera` (`FrameDescription::camera`, what the frame drew
    /// with) and left out here, so the two can't disagree:
    /// [`camera_of`](Self::camera_of) reads either.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<CameraState>,
    pub renderer: ReplayRenderer,
    /// The particle draws of `particles.bin`.
    pub particles: Vec<EmitterDraw>,
}

impl ReplayInputs {
    /// The camera a replay of the frame `description` describes draws with:
    /// the manifest's, else (a capture from before #1176) these inputs'.
    pub fn camera_of<'a>(
        &'a self,
        description: &'a crate::frame_capture::FrameDescription,
    ) -> Option<&'a CameraState> {
        description.camera.as_ref().or(self.camera.as_ref())
    }
}

/// What a replay of this frame draws fresh rather than as captured
/// (ADR 0126 decision 4's list), a line each: the manifest's
/// `replay.fresh_only`, and the reason a replay names when its image
/// differs.
pub fn fresh_only(renderer: &crate::SceneRenderer, world: &FlintWorld) -> Vec<String> {
    let mut out = Vec::new();
    let config = renderer.effective_post_process_config();
    for (name, style) in config.styles.iter() {
        if !style.enabled {
            continue;
        }
        // A style's frame-to-frame history (ASCII's, a user style's
        // targets) is captured since #1136: `renderer.style_history`
        if !style.reveal.is_empty() {
            out.push(format!("{name}: its reveal region (recorded as text only)"));
        }
    }
    let runtime: Vec<String> = world
        .entities_with_component(flint_core::components::MODEL)
        .iter()
        .copied()
        .filter(|&id| {
            world
                .view(id, flint_core::components::MODEL)
                .opt_bool("runtime")
                .unwrap_or(false)
        })
        .map(|id| entity_name(world, id))
        .collect();
    if !runtime.is_empty() {
        out.push(format!("runtime meshes: {}", runtime.join(", ")));
    }
    if renderer.grass_config().is_some() {
        out.push("grass: mask repaints since load (update_grass_mask)".into());
    }
    if !world
        .entities_with_component(flint_core::components::TERRAIN)
        .is_empty()
    {
        out.push("terrain: runtime edits since load".into());
    }
    out
}

/// Each model and texture `world` names, `(kind, name)`, once each, sorted:
/// what a capture hashes. A model's split part (`bench/bench`) is its file's.
pub fn assets_named(world: &FlintWorld) -> Vec<(&'static str, String)> {
    use flint_core::components as comp;
    let mut out: Vec<(&'static str, String)> = Vec::new();
    for info in world.all_entities() {
        if let Some(asset) = world.view(info.id, comp::MODEL).opt_str("asset") {
            let file = asset.split('/').next().unwrap_or(asset).to_string();
            if !out.iter().any(|(k, n)| *k == "model" && *n == file) {
                out.push(("model", file));
            }
        }
        if let Some(texture) = world.view(info.id, comp::MATERIAL).opt_str("texture") {
            if !out.iter().any(|(k, n)| *k == "texture" && n == texture) {
                out.push(("texture", texture.to_string()));
            }
        }
    }
    out.sort();
    out
}

/// Each of `names` found by `config`'s search, read and hashed: the
/// manifest's `replay.assets`. One the search doesn't find is left out (the
/// load drew it as missing too).
pub fn hash_assets(
    names: Vec<(&'static str, String)>,
    config: &crate::model_loader::ModelLoadConfig,
) -> Vec<crate::frame_capture::AssetDescription> {
    let mut found = Vec::new();
    for (kind, name) in names {
        let Ok(path) = resolve_asset(config, kind, &name) else {
            continue;
        };
        let (sha256, bytes) = hash_file(&path).map_or((None, 0), |(h, n)| (Some(h), n));
        found.push(crate::frame_capture::AssetDescription {
            kind: kind.to_string(),
            name,
            path: flint_core::project::shown(&path),
            sha256,
            bytes,
        });
    }
    found
}

/// Where `config`'s search finds a `kind` (`model` or `texture`) named `name`.
pub fn resolve_asset(
    config: &crate::model_loader::ModelLoadConfig,
    kind: &str,
    name: &str,
) -> Result<std::path::PathBuf, flint_core::project::NotFound> {
    match kind {
        "model" => crate::model_loader::resolve_model_path(config, name),
        _ => crate::model_loader::resolve_texture_path(config, name),
    }
}

/// A file's SHA-256 (hex) and length, when it reads.
pub fn hash_file(path: &std::path::Path) -> Option<(String, u64)> {
    flint_core::vfs::read(path).ok().map(|b| {
        (
            flint_core::ContentHash::from_bytes(&b).to_hex(),
            b.len() as u64,
        )
    })
}

/// The styles' history a capture holds (#1136), read from its `replay/`
/// folder for [`SceneRenderer::import_style_history`](crate::SceneRenderer::import_style_history):
/// each of `states` with its textures' bytes. A file that doesn't read is
/// left out and named, a line each (that part then draws fresh, and says
/// so).
pub fn read_style_history(
    replay_dir: &std::path::Path,
    states: &[crate::stylize::history::StyleHistoryState],
) -> (Vec<crate::stylize::history::ImportedHistory>, Vec<String>) {
    let mut said = Vec::new();
    let histories = states
        .iter()
        .map(|state| {
            let mut data = BTreeMap::new();
            for t in state.parts.iter().flat_map(|p| &p.textures) {
                match flint_core::vfs::read(replay_dir.join(&t.file)) {
                    Ok(bytes) => {
                        data.insert(t.file.clone(), Arc::new(bytes));
                    }
                    Err(e) => said.push(format!("replay/{}: {e}", t.file)),
                }
            }
            crate::stylize::history::ImportedHistory::new(state.clone(), data)
        })
        .collect();
    (histories, said)
}

/// An entity's name, or `#<id>` for one without (a capture names by name).
pub fn entity_name(world: &FlintWorld, id: flint_core::EntityId) -> String {
    world
        .get_name(id)
        .map(str::to_string)
        .unwrap_or_else(|| format!("#{}", id.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_renderers_share_reads_back_as_written() {
        let state = ReplayRenderer {
            msaa: 4,
            skinning: vec![PaletteEntry {
                entity: "tree".into(),
                kind: PaletteKind::Echo,
                echo: Some(1),
                opacity: Some(0.5),
                offset: 8192,
                count: 128,
            }],
            ..Default::default()
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains(r#""kind":"echo""#), "{json}");
        let back: ReplayRenderer = serde_json::from_str(&json).unwrap();
        assert_eq!(back, state);
        // A field a later engine adds is no reason to refuse it
        let old: ReplayRenderer = serde_json::from_str(r#"{"msaa":1,"later":true}"#).unwrap();
        assert_eq!(old.msaa, 1);
        // A capture from before the grid was recorded replays as it always
        // did, without one (#1211)
        assert!(!old.grid);
    }
}
