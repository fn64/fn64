#[path = "../src/rt64_aa_experiment.rs"]
mod rt64_aa_experiment;

use fn64_render::{
    DownsampleMultiplier, RenderAntialiasing, RenderResolution, ResolutionMultiplier,
};
use rt64_aa_experiment::{load, settings_sha256_hex, Rt64AaPreset};

#[test]
fn presets_isolate_high_resolution_downsample_and_msaa_axes() {
    let native = Rt64AaPreset::Native.settings();
    let high = Rt64AaPreset::HighResolution2x.settings();
    let supersample = Rt64AaPreset::Supersample2x.settings();
    let msaa = Rt64AaPreset::Msaa4x.settings();

    assert_eq!(
        Rt64AaPreset::ALL.map(Rt64AaPreset::label),
        [
            "native-1x",
            "high-resolution-2x",
            "supersample-2x-box",
            "native-1x-msaa4x",
        ]
    );
    assert_eq!(native, fn64_render::RenderRuntimeSettings::default());
    assert_eq!(high.resolution, RenderResolution::Manual);
    assert_eq!(
        high.resolution_multiplier,
        ResolutionMultiplier::new(2.0).unwrap()
    );
    assert_eq!(
        high.downsample_multiplier,
        DownsampleMultiplier::new(1).unwrap()
    );
    assert_eq!(supersample.resolution, RenderResolution::Manual);
    assert_eq!(
        supersample.resolution_multiplier,
        ResolutionMultiplier::new(2.0).unwrap()
    );
    assert_eq!(
        supersample.downsample_multiplier,
        DownsampleMultiplier::new(2).unwrap()
    );
    assert_eq!(msaa.resolution, RenderResolution::Original);
    assert_eq!(msaa.antialiasing, RenderAntialiasing::Msaa4x);
    assert_eq!(settings_sha256_hex(&native).len(), 64);
}

#[test]
fn config_requires_an_exact_typed_image_and_rejects_unknown_fields() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("rt64-aa.toml");
    std::fs::write(
        &path,
        "resolution = \"manual\"\nresolution_multiplier = 2.0\ndownsample_multiplier = 2\nantialiasing = \"msaa2x\"\n",
    )
    .unwrap();
    let settings = load(&path).unwrap();
    assert_eq!(settings.resolution, RenderResolution::Manual);
    assert_eq!(
        settings.resolution_multiplier,
        ResolutionMultiplier::new(2.0).unwrap()
    );
    assert_eq!(
        settings.downsample_multiplier,
        DownsampleMultiplier::new(2).unwrap()
    );
    assert_eq!(settings.antialiasing, RenderAntialiasing::Msaa2x);

    std::fs::write(
        &path,
        "resolution = \"manual\"\nresolution_multiplier = 2.0\ndownsample_multiplier = 2\nantialiasing = \"none\"\n typo = true\n",
    )
    .unwrap();
    assert!(load(&path)
        .unwrap_err()
        .to_string()
        .contains("unknown field"));
}

#[test]
fn config_rejects_out_of_range_typed_values() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("rt64-aa.toml");
    std::fs::write(
        &path,
        "resolution = \"manual\"\nresolution_multiplier = 2.0\ndownsample_multiplier = 0\nantialiasing = \"none\"\n",
    )
    .unwrap();
    assert!(load(&path)
        .unwrap_err()
        .to_string()
        .contains("downsample_multiplier=0"));
}
