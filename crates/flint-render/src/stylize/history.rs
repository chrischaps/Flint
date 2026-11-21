//! A style's frame-to-frame history, handed out and taken back (#1136,
//! ADR 0126 decision 4): what a frame capture keeps so a fresh renderer
//! draws the captured frame from the same past.
//!
//! A style that keeps history (ASCII's tone cells and edge cells, a user
//! style's `history` targets) reads last frame's textures and never writes
//! them, so once the captured frame is drawn they still hold its past. A
//! capture asks each built style for them after the frame
//! ([`StylePass::export_history`]): the style names each part of its
//! history with a key (what it must share with a frame to be that frame's
//! past: the grid, the font, the size, but not the frame number or the
//! [`history_epoch`](StyleFrame::history_epoch), which are the capturing
//! renderer's), says whether the frame drew with it, and stages each
//! texture's copy in the capture's encoder.
//!
//! A replay hands them back before its first frame
//! ([`SceneRenderer::import_style_history`](crate::SceneRenderer::import_style_history)).
//! The style keeps them until it next draws and trusts them only when that
//! frame's key is the captured one: then it writes the texels into the
//! textures it reads as last frame's and draws as if that frame had come
//! just before. A key that differs, a part the frame drew without, or bytes
//! that don't fit are no past: the frame draws fresh and says why
//! ([`SceneRenderer::take_style_history_notes`](crate::SceneRenderer::take_style_history_notes)).
//!
//! Nothing here runs on a frame nobody captures or replays.

use crate::gpu_counts::CountedDevice;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// One style's history in a capture: `inputs.json`'s
/// `renderer.style_history`, one entry a style that keeps any.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StyleHistoryState {
    pub style: String,
    pub parts: Vec<HistoryPart>,
}

/// One part of a style's history (ASCII's `tone`, `edges`; a user style's
/// `targets`), with the key it belongs to.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryPart {
    pub name: String,
    /// What a frame must share with the captured one for this to be its
    /// past, as the style writes it.
    pub key: serde_json::Value,
    /// Whether the captured frame drew with it. When it didn't, it drew
    /// fresh, and so does a replay: the textures aren't kept.
    pub used: bool,
    /// Numbers the part keeps beside its textures (ASCII's last
    /// view-projection, #344).
    pub values: BTreeMap<String, Vec<f32>>,
    pub textures: Vec<HistoryTexture>,
}

/// One texture of a part: `file` under `replay/` holds its texels, rows
/// tightly packed, as the GPU stores `format` (little-endian).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryTexture {
    pub name: String,
    /// The format as Rust prints it (`Rgba16Uint`).
    pub format: String,
    pub size: [u32; 2],
    /// `history/<style>.<part>.<name>.bin`.
    pub file: String,
    pub bytes: u64,
}

/// A texture copied in the capture's encoder, not yet read.
pub(crate) struct StagedHistory {
    pub file: String,
    pub buffer: wgpu::Buffer,
    pub row: u32,
    pub padded_row: u32,
    pub height: u32,
}

impl StagedHistory {
    /// The texels out of the mapped buffer, rows unpadded.
    pub(crate) fn unpadded(&self, data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.row as usize * self.height as usize);
        for y in 0..self.height as usize {
            let start = y * self.padded_row as usize;
            bytes.extend_from_slice(&data[start..start + self.row as usize]);
        }
        bytes
    }
}

/// What [`StylePass::export_history`](super::StylePass::export_history)
/// writes into: the parts, and each texture's copy staged in the capture's
/// encoder.
pub struct HistoryExport<'a> {
    pub(crate) device: &'a wgpu::Device,
    pub(crate) encoder: &'a mut wgpu::CommandEncoder,
    /// The style asked.
    pub(crate) style: String,
    /// The number of the frame captured ([`StyleFrame::frame`](super::StyleFrame::frame)):
    /// a part whose last frame is another wasn't drawn by it (the style or
    /// its edges were off), and is left out.
    pub frame: u64,
    pub(crate) parts: Vec<HistoryPart>,
    pub(crate) staged: Vec<StagedHistory>,
}

impl<'a> HistoryExport<'a> {
    pub(crate) fn new(
        device: &'a wgpu::Device,
        encoder: &'a mut wgpu::CommandEncoder,
        style: &str,
        frame: u64,
    ) -> Self {
        Self {
            device,
            encoder,
            style: style.to_string(),
            frame,
            parts: Vec::new(),
            staged: Vec::new(),
        }
    }

    /// Add part `name` with its `key`, whether the frame drew with it,
    /// its `values`, and each `(name, texture)` it read as last frame's.
    /// The textures are copied only when `used`. A texture that can't be
    /// copied (no `COPY_SRC`, a format of blocks) makes the part unused.
    pub fn part(
        &mut self,
        name: &str,
        key: serde_json::Value,
        used: bool,
        values: BTreeMap<String, Vec<f32>>,
        textures: &[(&str, &wgpu::Texture)],
    ) {
        let mut part = HistoryPart {
            name: name.to_string(),
            key,
            used,
            values,
            textures: Vec::new(),
        };
        let copyable = textures.iter().all(|(_, t)| {
            texel_bytes(t.format()).is_some() && t.usage().contains(wgpu::TextureUsages::COPY_SRC)
        });
        if !copyable {
            part.used = false;
        }
        if part.used {
            for (tex, texture) in textures {
                let texel = texel_bytes(texture.format()).expect("checked above");
                let (width, height) = (texture.width(), texture.height());
                let row = width * texel;
                let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
                let padded_row = row.div_ceil(align) * align;
                let buffer = self.device.create_buffer_counted(&wgpu::BufferDescriptor {
                    label: Some("Frame Capture Style History"),
                    size: u64::from(padded_row) * u64::from(height),
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                self.encoder.copy_texture_to_buffer(
                    texture.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(padded_row),
                            rows_per_image: Some(height),
                        },
                    },
                    wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                );
                let file = format!("history/{}.{name}.{tex}.bin", self.style);
                part.textures.push(HistoryTexture {
                    name: tex.to_string(),
                    format: format!("{:?}", texture.format()),
                    size: [width, height],
                    file: file.clone(),
                    bytes: u64::from(row) * u64::from(height),
                });
                self.staged.push(StagedHistory {
                    file,
                    buffer,
                    row,
                    padded_row,
                    height,
                });
            }
        }
        self.parts.push(part);
    }

    /// What was added: the style's state (`None` with no part) and the
    /// copies to read.
    pub(crate) fn finish(self) -> (Option<StyleHistoryState>, Vec<StagedHistory>) {
        let state = if self.parts.is_empty() {
            None
        } else {
            Some(StyleHistoryState {
                style: self.style,
                parts: self.parts,
            })
        };
        (state, self.staged)
    }
}

/// Bytes a texel of `format`, for a format of single texels.
fn texel_bytes(format: wgpu::TextureFormat) -> Option<u32> {
    (format.block_dimensions() == (1, 1) && !format.is_depth_stencil_format())
        .then(|| format.block_copy_size(None))
        .flatten()
}

/// One style's history taken back from a capture: its state and each
/// texture's texels by file. Handed to the style by
/// [`SceneRenderer::import_style_history`](crate::SceneRenderer::import_style_history).
#[derive(Clone, Default)]
pub struct ImportedHistory {
    pub state: StyleHistoryState,
    pub data: BTreeMap<String, Arc<Vec<u8>>>,
    /// Where the style says what it did with it.
    pub(crate) notes: Notes,
}

/// What styles said of the history handed to them, a line each.
pub(crate) type Notes = Arc<Mutex<Vec<String>>>;

impl ImportedHistory {
    /// `state` with its textures' texels by file.
    pub fn new(state: StyleHistoryState, data: BTreeMap<String, Arc<Vec<u8>>>) -> Self {
        Self {
            state,
            data,
            notes: Notes::default(),
        }
    }

    /// Part `name`, when the captured frame drew with it.
    pub fn used_part(&self, name: &str) -> Option<&HistoryPart> {
        self.state.parts.iter().find(|p| p.name == name && p.used)
    }

    /// Say `line` about this style's history.
    pub fn say(&self, line: impl Into<String>) {
        self.notes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(line.into());
    }

    /// Part `part`'s texture `name` written into `texture`, when the
    /// capture holds texels of its size and format; why not otherwise.
    pub fn write(
        &self,
        queue: &wgpu::Queue,
        part: &HistoryPart,
        name: &str,
        texture: &wgpu::Texture,
    ) -> Result<(), String> {
        use crate::gpu_counts::CountedQueue;
        let t = part
            .textures
            .iter()
            .find(|t| t.name == name)
            .ok_or_else(|| format!("the capture has no {name} texture"))?;
        let format = format!("{:?}", texture.format());
        let size = [texture.width(), texture.height()];
        if t.format != format || t.size != size {
            return Err(format!(
                "its {name} texture is {}x{} {}, this frame's {}x{} {format}",
                t.size[0], t.size[1], t.format, size[0], size[1]
            ));
        }
        let texel = texel_bytes(texture.format())
            .ok_or_else(|| format!("its {name} texture's format can't be written"))?;
        let bytes = self
            .data
            .get(&t.file)
            .ok_or_else(|| format!("{} isn't in the capture", t.file))?;
        let row = size[0] * texel;
        if bytes.len() as u64 != u64::from(row) * u64::from(size[1]) {
            return Err(format!(
                "{} holds {} bytes, not the {} a {}x{} {format} texture is",
                t.file,
                bytes.len(),
                u64::from(row) * u64::from(size[1]),
                size[0],
                size[1]
            ));
        }
        queue.write_texture_counted(
            texture.as_image_copy(),
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(size[1]),
            },
            wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }
}

/// What differs between two keys, field by field (`grid [80, 45] vs
/// [40, 23]`), or `None` when they're the same.
pub fn key_difference(captured: &serde_json::Value, now: &serde_json::Value) -> Option<String> {
    if captured == now {
        return None;
    }
    let (Some(a), Some(b)) = (captured.as_object(), now.as_object()) else {
        return Some(format!("{captured} vs {now}"));
    };
    let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
    keys.sort();
    keys.dedup();
    let null = serde_json::Value::Null;
    let differ: Vec<String> = keys
        .into_iter()
        .filter_map(|k| {
            let (x, y) = (a.get(k).unwrap_or(&null), b.get(k).unwrap_or(&null));
            (x != y).then(|| format!("{k} {x} vs {y}"))
        })
        .collect();
    Some(differ.join(", "))
}

/// The history a style holds until it next draws, taken by its encode.
#[derive(Default)]
pub struct PendingHistory(Mutex<Option<ImportedHistory>>);

impl PendingHistory {
    pub fn put(&self, history: ImportedHistory) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(history);
    }

    /// The history handed over, once; `None` on every frame after, and on
    /// every frame of a renderer nobody handed any (one lock).
    pub fn take(&self) -> Option<ImportedHistory> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_difference_names_each_field_that_differs() {
        let a = serde_json::json!({"grid": [80, 45], "font": "builtin", "size": [640, 360]});
        let b = serde_json::json!({"grid": [40, 23], "font": "builtin", "size": [320, 180]});
        assert_eq!(key_difference(&a, &a), None);
        assert_eq!(
            key_difference(&a, &b).as_deref(),
            Some("grid [80,45] vs [40,23], size [640,360] vs [320,180]")
        );
    }

    #[test]
    fn a_state_reads_back_as_written_and_takes_what_a_later_engine_adds() {
        let state = StyleHistoryState {
            style: "ascii".into(),
            parts: vec![HistoryPart {
                name: "tone".into(),
                key: serde_json::json!({"grid": [3, 2]}),
                used: true,
                values: [("view_proj".to_string(), vec![1.0; 16])].into(),
                textures: vec![HistoryTexture {
                    name: "cells".into(),
                    format: "Rgba16Uint".into(),
                    size: [3, 2],
                    file: "history/ascii.tone.cells.bin".into(),
                    bytes: 48,
                }],
            }],
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: StyleHistoryState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, state);
        let later: StyleHistoryState =
            serde_json::from_str(r#"{"style":"ascii","later":1}"#).unwrap();
        assert_eq!(later.style, "ascii");
    }
}
