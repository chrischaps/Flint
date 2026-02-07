//! Shared helpers for registering generated/imported assets into Flint's catalog.

use crate::provider::{AssetKind, GenerateRequest, GenerateResult};
use flint_asset::{AssetMeta, AssetType, ContentStore};
use flint_core::{FlintError, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const DEFAULT_STORE_ROOT: &str = ".flint/assets";
const DEFAULT_ASSETS_ROOT: &str = "assets";

#[derive(Debug, Clone)]
pub struct RegisteredAsset {
    pub hash: String,
    pub sidecar_path: PathBuf,
    pub meta: AssetMeta,
}

/// Register a generated asset using default project paths:
/// - content store: `.flint/assets`
/// - sidecars: `assets/<kind>/*.asset.toml`
pub fn register_generated_asset(
    request: &GenerateRequest,
    result: &GenerateResult,
) -> Result<RegisteredAsset> {
    register_generated_asset_with_roots(
        request,
        result,
        Path::new(DEFAULT_STORE_ROOT),
        Path::new(DEFAULT_ASSETS_ROOT),
    )
}

/// Register a generated asset into a specific content store and sidecar root.
pub fn register_generated_asset_with_roots(
    request: &GenerateRequest,
    result: &GenerateResult,
    store_root: &Path,
    assets_root: &Path,
) -> Result<RegisteredAsset> {
    let output_path = Path::new(&result.output_path);
    if !output_path.exists() {
        return Err(FlintError::GenerationError(format!(
            "Generated file not found: {}",
            output_path.display()
        )));
    }

    let store = ContentStore::new(store_root);
    let hash = store.store(output_path)?;
    let hash_str = hash.to_prefixed_hex();

    let meta = AssetMeta {
        name: request.name.clone(),
        asset_type: asset_type_for_kind(request.kind),
        hash: hash_str.clone(),
        source_path: Some(result.output_path.clone()),
        format: output_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_string()),
        properties: build_generation_properties(result),
        tags: request.tags.clone(),
    };

    let sidecar_path = write_asset_sidecar_with_root(&meta, assets_root)?;

    Ok(RegisteredAsset {
        hash: hash_str,
        sidecar_path,
        meta,
    })
}

/// Write a sidecar file using default `assets/` root.
pub fn write_asset_sidecar(meta: &AssetMeta) -> Result<PathBuf> {
    write_asset_sidecar_with_root(meta, Path::new(DEFAULT_ASSETS_ROOT))
}

/// Write a sidecar file under a specific assets root directory.
pub fn write_asset_sidecar_with_root(meta: &AssetMeta, assets_root: &Path) -> Result<PathBuf> {
    #[derive(Serialize)]
    struct Sidecar<'a> {
        asset: &'a AssetMeta,
    }

    let subdir = asset_subdirectory(meta.asset_type);
    let sidecar_dir = assets_root.join(subdir);
    std::fs::create_dir_all(&sidecar_dir)?;

    let sidecar_path = sidecar_dir.join(format!("{}.asset.toml", meta.name));
    let toml_str = toml::to_string_pretty(&Sidecar { asset: meta })?;
    std::fs::write(&sidecar_path, toml_str)?;
    Ok(sidecar_path)
}

fn build_generation_properties(result: &GenerateResult) -> HashMap<String, toml::Value> {
    let mut props = HashMap::new();
    props.insert(
        "prompt".to_string(),
        toml::Value::String(result.prompt_used.clone()),
    );
    props.insert(
        "provider".to_string(),
        toml::Value::String(result.provider.clone()),
    );
    for (k, v) in &result.metadata {
        props.insert(k.clone(), toml::Value::String(v.clone()));
    }
    props
}

fn asset_type_for_kind(kind: AssetKind) -> AssetType {
    match kind {
        AssetKind::Texture => AssetType::Texture,
        AssetKind::Model => AssetType::Mesh,
        AssetKind::Audio => AssetType::Audio,
    }
}

fn asset_subdirectory(asset_type: AssetType) -> &'static str {
    match asset_type {
        AssetType::Mesh => "meshes",
        AssetType::Texture => "textures",
        AssetType::Material => "materials",
        AssetType::Audio => "audio",
        AssetType::Script => "scripts",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{AudioParams, ModelParams, TextureParams};
    use std::io::Write;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "flint_registration_test_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_request(kind: AssetKind, name: &str) -> GenerateRequest {
        GenerateRequest {
            name: name.to_string(),
            description: "test".to_string(),
            kind,
            texture_params: if kind == AssetKind::Texture {
                Some(TextureParams::default())
            } else {
                None
            },
            model_params: if kind == AssetKind::Model {
                Some(ModelParams::default())
            } else {
                None
            },
            audio_params: if kind == AssetKind::Audio {
                Some(AudioParams::default())
            } else {
                None
            },
            tags: vec!["tag_a".to_string(), "tag_b".to_string()],
        }
    }

    #[test]
    fn test_register_generated_asset_with_roots() {
        let dir = temp_dir();
        let store_root = dir.join("store");
        let assets_root = dir.join("assets");
        let out_dir = dir.join("out");
        std::fs::create_dir_all(&out_dir).unwrap();

        let output_path = out_dir.join("brick wall.png");
        let mut f = std::fs::File::create(&output_path).unwrap();
        f.write_all(b"fake-image").unwrap();

        let request = sample_request(AssetKind::Texture, "brick_wall");
        let result = GenerateResult {
            output_path: output_path.to_string_lossy().to_string(),
            prompt_used: "weathered \"brick\" wall".to_string(),
            provider: "mock".to_string(),
            duration_secs: 0.1,
            content_hash: None,
            metadata: HashMap::from([("seed".to_string(), "42".to_string())]),
        };

        let registered = register_generated_asset_with_roots(
            &request,
            &result,
            &store_root,
            &assets_root,
        )
        .unwrap();

        assert!(registered.hash.starts_with("sha256:"));
        assert!(registered.sidecar_path.exists());
        assert_eq!(registered.meta.name, "brick_wall");
        assert_eq!(registered.meta.asset_type, AssetType::Texture);

        let sidecar = std::fs::read_to_string(&registered.sidecar_path).unwrap();
        let parsed: toml::Value = toml::from_str(&sidecar).unwrap();
        assert_eq!(
            parsed
                .get("asset")
                .and_then(|a| a.get("name"))
                .and_then(|v| v.as_str()),
            Some("brick_wall")
        );
        assert_eq!(
            parsed
                .get("asset")
                .and_then(|a| a.get("properties"))
                .and_then(|p| p.get("prompt"))
                .and_then(|v| v.as_str()),
            Some("weathered \"brick\" wall")
        );

        std::fs::remove_dir_all(dir).ok();
    }
}
