//! The renderer's side of runtime quality settings (FlintKart#55): masking
//! a scene's post-process config by a tier's caps, and describing the GPU.
//! `SceneRenderer::set_quality` applies a whole [`QualitySettings`].

pub use flint_core::quality::{GpuInfo, QualityError, QualitySettings, QualityValue};

use crate::postprocess::PostProcessConfig;

/// `authored` with `quality`'s caps applied: an effect the caps turn off is
/// off, one they leave on stays as the scene authored it.
pub fn mask_post_process(
    authored: &PostProcessConfig,
    quality: &QualitySettings,
) -> PostProcessConfig {
    let mut c = authored.clone();
    c.enabled &= quality.post_process;
    c.bloom_enabled &= quality.bloom;
    c.ssao_enabled &= quality.ssao;
    c.ssao_samples = c.ssao_samples.min(quality.ssao_samples);
    c.fog_enabled &= quality.fog;
    c.fog_height_enabled &= quality.fog;
    if !quality.fog {
        c.ground_fog_density = 0.0;
    }
    c.volumetric_enabled &= quality.volumetric;
    c
}

/// The vendor's name from its PCI id.
fn vendor_name(id: u32) -> &'static str {
    match id {
        0x10de => "nvidia",
        0x1002 | 0x1022 => "amd",
        0x8086 => "intel",
        0x13b5 => "arm",
        0x5143 => "qualcomm",
        0x106b => "apple",
        0x1010 => "imagination",
        0x1414 => "microsoft",
        _ => "unknown",
    }
}

/// The platform this build runs on.
pub fn platform() -> &'static str {
    if cfg!(target_arch = "wasm32") {
        "web"
    } else if cfg!(target_os = "android") {
        "android"
    } else if cfg!(target_os = "ios") {
        "ios"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// The class a game picks a starting tier from; see [`GpuInfo::class`].
pub fn gpu_class(device_type: wgpu::DeviceType, vendor: &str, platform: &str) -> &'static str {
    use wgpu::DeviceType as D;
    let mobile_vendor = matches!(vendor, "arm" | "qualcomm" | "imagination");
    match device_type {
        D::Cpu => "software",
        _ if platform == "android" || platform == "ios" || mobile_vendor => "mobile",
        D::DiscreteGpu => "desktop",
        D::IntegratedGpu => "integrated",
        // A browser reports what the page may know, often nothing.
        D::VirtualGpu | D::Other => "unknown",
    }
}

/// What `adapter` is; see [`GpuInfo`].
pub fn gpu_info(adapter: &wgpu::Adapter) -> GpuInfo {
    let info = adapter.get_info();
    let vendor = vendor_name(info.vendor).to_string();
    let platform = platform();
    let device_type = match info.device_type {
        wgpu::DeviceType::DiscreteGpu => "discrete",
        wgpu::DeviceType::IntegratedGpu => "integrated",
        wgpu::DeviceType::VirtualGpu => "virtual",
        wgpu::DeviceType::Cpu => "cpu",
        wgpu::DeviceType::Other => "other",
    };
    let backend = match info.backend {
        wgpu::Backend::Vulkan => "vulkan",
        wgpu::Backend::Dx12 => "dx12",
        wgpu::Backend::Metal => "metal",
        wgpu::Backend::Gl => "gl",
        wgpu::Backend::BrowserWebGpu => "webgpu",
        wgpu::Backend::Empty => "none",
    };
    let driver = [info.driver.as_str(), info.driver_info.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    GpuInfo {
        name: info.name.clone(),
        class: gpu_class(info.device_type, &vendor, platform).to_string(),
        vendor,
        device_type: device_type.to_string(),
        backend: backend.to_string(),
        driver,
        platform: platform.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_turn_effects_off_but_never_on() {
        let authored = PostProcessConfig {
            bloom_enabled: true,
            ssao_enabled: false,
            ssao_samples: 32,
            fog_enabled: true,
            ground_fog_density: 0.3,
            ..PostProcessConfig::default()
        };

        let all_on = QualitySettings::default();
        let same = mask_post_process(&authored, &all_on);
        assert!(same.bloom_enabled);
        assert!(
            !same.ssao_enabled,
            "a cap can't add SSAO the scene left out"
        );
        assert_eq!(same.ssao_samples, 32);
        assert_eq!(same.ground_fog_density, 0.3);

        let low = QualitySettings {
            bloom: false,
            ssao: true,
            ssao_samples: 16,
            fog: false,
            ..QualitySettings::default()
        };
        let masked = mask_post_process(&authored, &low);
        assert!(!masked.bloom_enabled);
        assert!(!masked.fog_enabled);
        assert_eq!(masked.ground_fog_density, 0.0);
        assert_eq!(masked.ssao_samples, 16);
        // The authored config is untouched, so lifting the caps restores it.
        assert!(authored.bloom_enabled);
    }

    #[test]
    fn phones_are_mobile_whatever_the_driver_says() {
        assert_eq!(
            gpu_class(wgpu::DeviceType::IntegratedGpu, "arm", "android"),
            "mobile"
        );
        assert_eq!(
            gpu_class(wgpu::DeviceType::Other, "unknown", "android"),
            "mobile"
        );
        assert_eq!(
            gpu_class(wgpu::DeviceType::IntegratedGpu, "qualcomm", "windows"),
            "mobile"
        );
        assert_eq!(
            gpu_class(wgpu::DeviceType::DiscreteGpu, "nvidia", "windows"),
            "desktop"
        );
        assert_eq!(
            gpu_class(wgpu::DeviceType::IntegratedGpu, "intel", "windows"),
            "integrated"
        );
        assert_eq!(
            gpu_class(wgpu::DeviceType::Cpu, "microsoft", "windows"),
            "software"
        );
        assert_eq!(
            gpu_class(wgpu::DeviceType::Other, "unknown", "web"),
            "unknown"
        );
    }
}
