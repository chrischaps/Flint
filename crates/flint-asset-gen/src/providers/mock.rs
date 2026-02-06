//! Mock provider for testing
//!
//! Generates solid-color PNGs (textures), minimal GLB (models),
//! and silence WAV (audio) without any network calls.

use crate::job::GenerationJob;
use crate::provider::*;
use crate::style::StyleGuide;
use flint_core::{FlintError, Result};
use std::collections::HashMap;
use std::path::Path;

/// A mock provider that generates placeholder assets locally
#[derive(Default)]
pub struct MockProvider;

impl MockProvider {
    pub fn new() -> Self {
        Self
    }
}

impl GenerationProvider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    fn supported_kinds(&self) -> Vec<AssetKind> {
        vec![AssetKind::Texture, AssetKind::Model, AssetKind::Audio]
    }

    fn health_check(&self) -> Result<ProviderStatus> {
        Ok(ProviderStatus::Available)
    }

    fn generate(
        &self,
        request: &GenerateRequest,
        style: Option<&StyleGuide>,
        output_dir: &Path,
    ) -> Result<GenerateResult> {
        let start = std::time::Instant::now();
        let prompt = self.build_prompt(request, style);

        std::fs::create_dir_all(output_dir)?;

        let output_path = match request.kind {
            AssetKind::Texture => {
                let params = request
                    .texture_params
                    .as_ref()
                    .cloned()
                    .unwrap_or_default();
                generate_solid_png(output_dir, &request.name, params.width, params.height)?
            }
            AssetKind::Model => generate_minimal_glb(output_dir, &request.name)?,
            AssetKind::Audio => {
                let params = request
                    .audio_params
                    .as_ref()
                    .cloned()
                    .unwrap_or_default();
                generate_silence_wav(output_dir, &request.name, params.duration)?
            }
        };

        let duration = start.elapsed().as_secs_f64();

        // Compute content hash
        let hash = flint_core::ContentHash::from_file(&output_path)
            .map(|h| h.to_prefixed_hex())
            .ok();

        Ok(GenerateResult {
            output_path: output_path.to_string_lossy().to_string(),
            prompt_used: prompt,
            provider: "mock".to_string(),
            duration_secs: duration,
            content_hash: hash,
            metadata: HashMap::new(),
        })
    }

    fn submit_job(
        &self,
        request: &GenerateRequest,
        _style: Option<&StyleGuide>,
    ) -> Result<GenerationJob> {
        let mut job = GenerationJob::new("mock", &request.name);
        job.prompt = Some(request.description.clone());
        Ok(job)
    }

    fn poll_job(&self, _job: &GenerationJob) -> Result<JobPollResult> {
        // Mock jobs complete instantly
        Ok(JobPollResult::Complete)
    }

    fn download_result(&self, job: &GenerationJob, output_dir: &Path) -> Result<GenerateResult> {
        // For mock, just generate inline
        let request = GenerateRequest {
            name: job.asset_name.clone(),
            description: job.prompt.clone().unwrap_or_default(),
            kind: AssetKind::Texture,
            texture_params: None,
            model_params: None,
            audio_params: None,
            tags: vec![],
        };
        self.generate(&request, None, output_dir)
    }

    fn build_prompt(&self, request: &GenerateRequest, style: Option<&StyleGuide>) -> String {
        match style {
            Some(s) => s.enrich_prompt(&request.description),
            None => request.description.clone(),
        }
    }
}

/// Generate a solid-color PNG texture
fn generate_solid_png(
    output_dir: &Path,
    name: &str,
    width: u32,
    height: u32,
) -> Result<std::path::PathBuf> {
    // Generate a warm, earthy color from the name hash for visual interest
    let hash_val = name.bytes().fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    let r = ((hash_val >> 16) & 0xFF) as u8;
    let g = ((hash_val >> 8) & 0xFF) as u8;
    let b = (hash_val & 0xFF) as u8;

    let mut img_data = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..(width * height) {
        img_data.extend_from_slice(&[r, g, b, 255]);
    }

    let path = output_dir.join(format!("{}.png", name));
    let img = image::RgbaImage::from_raw(width, height, img_data).ok_or_else(|| {
        FlintError::GenerationError("Failed to create image buffer".to_string())
    })?;
    img.save(&path).map_err(|e| {
        FlintError::GenerationError(format!("Failed to save PNG: {}", e))
    })?;

    Ok(path)
}

/// Generate a minimal valid GLB file (single triangle)
fn generate_minimal_glb(output_dir: &Path, name: &str) -> Result<std::path::PathBuf> {
    // Minimal valid glTF 2.0 binary with a single triangle
    // This is the smallest possible valid GLB that importers will accept
    let json = serde_json::json!({
        "asset": { "version": "2.0", "generator": "flint-mock" },
        "scene": 0,
        "scenes": [{ "nodes": [0] }],
        "nodes": [{ "mesh": 0 }],
        "meshes": [{
            "primitives": [{
                "attributes": { "POSITION": 0 },
                "indices": 1
            }]
        }],
        "accessors": [
            {
                "bufferView": 0,
                "componentType": 5126,
                "count": 3,
                "type": "VEC3",
                "max": [1.0, 1.0, 0.0],
                "min": [-1.0, 0.0, 0.0]
            },
            {
                "bufferView": 1,
                "componentType": 5123,
                "count": 3,
                "type": "SCALAR",
                "max": [2],
                "min": [0]
            }
        ],
        "bufferViews": [
            { "buffer": 0, "byteOffset": 0, "byteLength": 36, "target": 34962 },
            { "buffer": 0, "byteOffset": 36, "byteLength": 6, "target": 34963 }
        ],
        "buffers": [{ "byteLength": 44 }]
    });

    let json_str = serde_json::to_string(&json).map_err(|e| {
        FlintError::GenerationError(format!("Failed to serialize GLB JSON: {}", e))
    })?;

    // Pad JSON to 4-byte alignment
    let json_bytes = json_str.as_bytes();
    let json_padded_len = (json_bytes.len() + 3) & !3;
    let mut json_padded = json_bytes.to_vec();
    json_padded.resize(json_padded_len, b' ');

    // Binary buffer: 3 vertices (3 * 12 bytes) + 3 indices (3 * 2 bytes) + 2 padding
    #[allow(clippy::approx_constant)]
    let vertices: [f32; 9] = [
        -1.0, 0.0, 0.0, // v0
        1.0, 0.0, 0.0, // v1
        0.0, 1.0, 0.0, // v2
    ];
    let indices: [u16; 3] = [0, 1, 2];

    let mut bin_data = Vec::new();
    for v in &vertices {
        bin_data.extend_from_slice(&v.to_le_bytes());
    }
    for i in &indices {
        bin_data.extend_from_slice(&i.to_le_bytes());
    }
    // Pad to 4-byte alignment
    let bin_padded_len = (bin_data.len() + 3) & !3;
    bin_data.resize(bin_padded_len, 0);

    // GLB header
    let total_len =
        12 + 8 + json_padded.len() as u32 + 8 + bin_data.len() as u32;

    let path = output_dir.join(format!("{}.glb", name));
    let mut file = std::fs::File::create(&path)?;
    use std::io::Write;

    // Header
    file.write_all(b"glTF")?; // magic
    file.write_all(&2u32.to_le_bytes())?; // version
    file.write_all(&total_len.to_le_bytes())?; // length

    // JSON chunk
    file.write_all(&(json_padded.len() as u32).to_le_bytes())?;
    file.write_all(&0x4E4F534Au32.to_le_bytes())?; // "JSON"
    file.write_all(&json_padded)?;

    // BIN chunk
    file.write_all(&(bin_data.len() as u32).to_le_bytes())?;
    file.write_all(&0x004E4942u32.to_le_bytes())?; // "BIN\0"
    file.write_all(&bin_data)?;

    Ok(path)
}

/// Generate a WAV file with silence
fn generate_silence_wav(
    output_dir: &Path,
    name: &str,
    duration_secs: f64,
) -> Result<std::path::PathBuf> {
    let sample_rate: u32 = 44100;
    let num_channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let num_samples = (sample_rate as f64 * duration_secs) as u32;
    let data_size = num_samples * (bits_per_sample / 8) as u32 * num_channels as u32;

    let path = output_dir.join(format!("{}.wav", name));
    let mut file = std::fs::File::create(&path)?;
    use std::io::Write;

    // RIFF header
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + data_size).to_le_bytes())?;
    file.write_all(b"WAVE")?;

    // fmt chunk
    file.write_all(b"fmt ")?;
    file.write_all(&16u32.to_le_bytes())?; // chunk size
    file.write_all(&1u16.to_le_bytes())?; // PCM format
    file.write_all(&num_channels.to_le_bytes())?;
    file.write_all(&sample_rate.to_le_bytes())?;
    let byte_rate = sample_rate * num_channels as u32 * (bits_per_sample / 8) as u32;
    file.write_all(&byte_rate.to_le_bytes())?;
    let block_align = num_channels * (bits_per_sample / 8);
    file.write_all(&block_align.to_le_bytes())?;
    file.write_all(&bits_per_sample.to_le_bytes())?;

    // data chunk
    file.write_all(b"data")?;
    file.write_all(&data_size.to_le_bytes())?;

    // Write silence (zeros)
    let silence = vec![0u8; data_size as usize];
    file.write_all(&silence)?;

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("flint_mock_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_mock_provider_health() {
        let provider = MockProvider::new();
        assert_eq!(provider.health_check().unwrap(), ProviderStatus::Available);
    }

    #[test]
    fn test_mock_provider_supported_kinds() {
        let provider = MockProvider::new();
        let kinds = provider.supported_kinds();
        assert!(kinds.contains(&AssetKind::Texture));
        assert!(kinds.contains(&AssetKind::Model));
        assert!(kinds.contains(&AssetKind::Audio));
    }

    #[test]
    fn test_mock_generate_texture() {
        let dir = temp_dir();
        let provider = MockProvider::new();

        let request = GenerateRequest {
            name: "test_brick".to_string(),
            description: "red brick wall".to_string(),
            kind: AssetKind::Texture,
            texture_params: Some(TextureParams {
                width: 64,
                height: 64,
                seed: None,
                seamless: false,
            }),
            model_params: None,
            audio_params: None,
            tags: vec![],
        };

        let result = provider.generate(&request, None, &dir).unwrap();
        assert!(Path::new(&result.output_path).exists());
        assert_eq!(result.provider, "mock");
        assert!(result.content_hash.is_some());

        // Verify it's a valid PNG
        let img = image::open(&result.output_path).unwrap();
        assert_eq!(img.width(), 64);
        assert_eq!(img.height(), 64);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_mock_generate_model() {
        let dir = temp_dir();
        let provider = MockProvider::new();

        let request = GenerateRequest {
            name: "test_chair".to_string(),
            description: "wooden chair".to_string(),
            kind: AssetKind::Model,
            texture_params: None,
            model_params: None,
            audio_params: None,
            tags: vec![],
        };

        let result = provider.generate(&request, None, &dir).unwrap();
        let path = Path::new(&result.output_path);
        assert!(path.exists());
        assert!(path.extension().unwrap() == "glb");

        // Verify it starts with glTF magic
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(&bytes[..4], b"glTF");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_mock_generate_audio() {
        let dir = temp_dir();
        let provider = MockProvider::new();

        let request = GenerateRequest {
            name: "test_sound".to_string(),
            description: "door creak".to_string(),
            kind: AssetKind::Audio,
            texture_params: None,
            model_params: None,
            audio_params: Some(AudioParams {
                duration: 1.0,
                seed: None,
            }),
            tags: vec![],
        };

        let result = provider.generate(&request, None, &dir).unwrap();
        let path = Path::new(&result.output_path);
        assert!(path.exists());

        // Verify it starts with RIFF header
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_mock_generate_with_style() {
        let dir = temp_dir();
        let provider = MockProvider::new();

        let style = StyleGuide {
            name: "test_style".to_string(),
            description: None,
            prompt_prefix: Some("Fantasy medieval".to_string()),
            prompt_suffix: None,
            negative_prompt: None,
            palette: vec!["#8B4513".to_string()],
            materials: crate::style::MaterialConstraints::default(),
            geometry: crate::style::GeometryConstraints::default(),
        };

        let request = GenerateRequest {
            name: "styled_tex".to_string(),
            description: "brick wall".to_string(),
            kind: AssetKind::Texture,
            texture_params: Some(TextureParams {
                width: 32,
                height: 32,
                ..Default::default()
            }),
            model_params: None,
            audio_params: None,
            tags: vec![],
        };

        let result = provider.generate(&request, Some(&style), &dir).unwrap();
        assert!(result.prompt_used.contains("Fantasy medieval"));
        assert!(result.prompt_used.contains("brick wall"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_mock_build_prompt_no_style() {
        let provider = MockProvider::new();
        let request = GenerateRequest {
            name: "test".to_string(),
            description: "hello world".to_string(),
            kind: AssetKind::Texture,
            texture_params: None,
            model_params: None,
            audio_params: None,
            tags: vec![],
        };
        assert_eq!(provider.build_prompt(&request, None), "hello world");
    }
}
