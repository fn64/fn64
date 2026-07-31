//! Canonical identity of the fn64-owned RT64 adapter, its shared render seam,
//! and build shape.

use sha2::{Digest, Sha256};
use std::{fs, io, path::Path};

const ROOT_FILES: &[&str] = &["Cargo.toml", "adapter_source_identity.rs", "build.rs"];
const FFI_FILES: &[&str] = &[
    "ffi/CMakeLists.txt",
    "ffi/fn64_rt64_shim.cpp",
    "ffi/fn64_rt64_shim.h",
    "ffi/fn64_rt64_raster_ps_overlay.hlsli",
    "ffi/fn64_rt64_video_interface.h",
    "ffi/fn64_rt64_video_interface_ps.hlsl",
];
const SHARED_RENDER_ROOT: &str = "../fn64-render";

pub fn adapter_source_sha256(
    manifest_dir: &Path,
    target: &str,
    enabled_features: &[String],
) -> io::Result<[u8; 32]> {
    let paths = adapter_source_paths(manifest_dir)?;

    let mut features = enabled_features.to_vec();
    features.sort();
    features.dedup();

    let inputs = paths
        .into_iter()
        .map(|relative| {
            let bytes = fs::read(manifest_dir.join(&relative))?;
            Ok((relative, bytes))
        })
        .collect::<io::Result<Vec<_>>>()?;
    Ok(hash_inputs(target, &features, &inputs))
}

fn hash_inputs(target: &str, features: &[String], inputs: &[(String, Vec<u8>)]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"fn64.rt64-adapter-source.v1\0");
    push_bytes(&mut digest, target.as_bytes());
    digest.update((features.len() as u64).to_be_bytes());
    for feature in features {
        push_bytes(&mut digest, feature.as_bytes());
    }
    digest.update((inputs.len() as u64).to_be_bytes());
    for (relative, bytes) in inputs {
        push_bytes(&mut digest, relative.as_bytes());
        push_bytes(&mut digest, bytes);
    }
    digest.finalize().into()
}

pub fn adapter_source_paths(manifest_dir: &Path) -> io::Result<Vec<String>> {
    let mut paths = ROOT_FILES
        .iter()
        .chain(FFI_FILES)
        .map(|path| (*path).to_owned())
        .collect::<Vec<_>>();
    collect_rs_files(
        manifest_dir,
        manifest_dir.join("src"),
        Path::new(""),
        &mut paths,
    )?;

    let shared_render = manifest_dir.join(SHARED_RENDER_ROOT);
    paths.push(format!("{SHARED_RENDER_ROOT}/Cargo.toml"));
    collect_rs_files(
        &shared_render,
        shared_render.join("src"),
        Path::new(SHARED_RENDER_ROOT),
        &mut paths,
    )?;
    paths.sort();
    assert!(
        paths.windows(2).all(|pair| pair[0] != pair[1]),
        "adapter source identity contains duplicate paths"
    );
    Ok(paths)
}

fn collect_rs_files(
    source_root: &Path,
    directory: impl AsRef<Path>,
    identity_root: &Path,
    paths: &mut Vec<String>,
) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_rs_files(source_root, &path, identity_root, paths)?;
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            paths.push(
                identity_root
                    .join(
                        path.strip_prefix(source_root)
                            .expect("source traversal stays beneath its declared root"),
                    )
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

fn push_bytes(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_binds_source_paths_bytes_target_and_features() {
        let inputs = vec![
            (
                "../fn64-render/src/microcode.rs".to_owned(),
                b"shared-seam-v1".to_vec(),
            ),
            ("ffi/shim.cpp".to_owned(), b"shim-v1".to_vec()),
            ("src/lib.rs".to_owned(), b"adapter-v1".to_vec()),
        ];
        let features = vec!["RT64".to_owned()];
        let baseline = hash_inputs("aarch64-apple-darwin", &features, &inputs);

        for changed in [
            vec![
                (
                    "../fn64-render/src/microcode.rs".to_owned(),
                    b"shared-seam-v1".to_vec(),
                ),
                ("ffi/shim.cpp".to_owned(), b"shim-v2".to_vec()),
                ("src/lib.rs".to_owned(), b"adapter-v1".to_vec()),
            ],
            vec![
                (
                    "../fn64-render/src/microcode.rs".to_owned(),
                    b"shared-seam-v1".to_vec(),
                ),
                ("ffi/renamed.cpp".to_owned(), b"shim-v1".to_vec()),
                ("src/lib.rs".to_owned(), b"adapter-v1".to_vec()),
            ],
            vec![
                (
                    "../fn64-render/src/microcode.rs".to_owned(),
                    b"shared-seam-v2".to_vec(),
                ),
                ("ffi/shim.cpp".to_owned(), b"shim-v1".to_vec()),
                ("src/lib.rs".to_owned(), b"adapter-v1".to_vec()),
            ],
        ] {
            assert_ne!(
                hash_inputs("aarch64-apple-darwin", &features, &changed),
                baseline
            );
        }
        assert_ne!(
            hash_inputs("x86_64-unknown-linux-gnu", &features, &inputs),
            baseline
        );
        assert_ne!(
            hash_inputs(
                "aarch64-apple-darwin",
                &["HFR_EVIDENCE".to_owned(), "RT64".to_owned()],
                &inputs,
            ),
            baseline
        );
    }

    #[test]
    fn crate_identity_covers_rust_cpp_manifest_and_build_inputs() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = if manifest_dir.join("src/ffi.rs").is_file() {
            manifest_dir.to_path_buf()
        } else {
            manifest_dir.join("../fn64-render-rt64")
        };
        let paths = adapter_source_paths(&root).unwrap();
        for required in [
            "Cargo.toml",
            "adapter_source_identity.rs",
            "build.rs",
            "src/lib.rs",
            "src/ffi.rs",
            "ffi/CMakeLists.txt",
            "ffi/fn64_rt64_shim.cpp",
            "ffi/fn64_rt64_shim.h",
            "ffi/fn64_rt64_raster_ps_overlay.hlsli",
            "ffi/fn64_rt64_video_interface.h",
            "ffi/fn64_rt64_video_interface_ps.hlsl",
            "../fn64-render/Cargo.toml",
            "../fn64-render/src/lib.rs",
            "../fn64-render/src/microcode.rs",
            "../fn64-render/src/rdp_completion.rs",
            "../fn64-render/src/settings.rs",
        ] {
            assert!(
                paths.iter().any(|path| path == required),
                "missing {required}"
            );
        }
        assert_ne!(
            adapter_source_sha256(&root, "test-target", &["RT64".to_owned()]).unwrap(),
            [0; 32]
        );
    }

    #[test]
    fn rdp_dither_overlay_is_exact_guarded_and_shares_one_typed_noise_sample() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = if manifest_dir.join("src/ffi.rs").is_file() {
            manifest_dir.to_path_buf()
        } else {
            manifest_dir.join("../fn64-render-rt64")
        };
        let cmake = fs::read_to_string(root.join("ffi/CMakeLists.txt")).unwrap();
        let shader =
            fs::read_to_string(root.join("ffi/fn64_rt64_raster_ps_overlay.hlsli")).unwrap();

        for guard in [
            "957b283411550457573421015580f84ca750c68a050981dbc1a4fe2634507820",
            "5e5de4bbd8a22192e857c277bae5af033b8606a6464aa73863d2efe0fd0a0d4d",
        ] {
            assert!(
                cmake.contains(guard),
                "missing exact RT64 source guard {guard}"
            );
        }

        for variant in [
            "Fn64RdpDitherDynamic",
            "Fn64RdpDitherDynamicMS",
            "Fn64RdpDitherSpecConstant",
            "Fn64RdpDitherSpecConstantMS",
            "Fn64RdpDitherSpecConstantFlat",
            "Fn64RdpDitherSpecConstantFlatMS",
            "Fn64RdpDitherLibrary",
            "Fn64RdpDitherLibraryMS",
        ] {
            assert!(
                cmake.contains(variant),
                "missing replacement variant {variant}"
            );
        }
        for mechanism in [
            "FN64_RT64_ORIGINAL_RASTER_PS_SHADER_OBJECTS",
            "HEADER_FILE_ONLY TRUE",
            "FN64_RT64_ORIGINAL_RASTER_PS_BLOBS",
            "original raster PS blob remains selected",
            "fn64_rt64_raster_shader.cpp",
            "${CMAKE_CURRENT_BINARY_DIR}/fn64_rt64_raster_ps.hlsl",
            "build_shader_dxil_impl(",
            "build_shader_msl_impl(",
            "build_shader_spirv_impl(",
        ] {
            assert!(
                cmake.contains(mechanism),
                "missing structural guard {mechanism}"
            );
        }
        assert!(!cmake.contains("${CMAKE_CURRENT_SOURCE_DIR}/fn64_rt64_raster_ps.hlsl"));

        assert!(cmake
            .contains("Alpha compare and coverage intentionally observe the original combiner"));
        assert!(cmake.contains("Only the combiner input to blending receives this bounded policy"));
        assert!(cmake.contains("Fn64RdpTakeFragmentNoiseSample(randomSeed);"));
        assert_eq!(
            cmake
                .matches("Fn64RdpFragmentNoiseUnitFloat(fragmentNoise)")
                .count(),
            2,
            "combiner NOISE and G_AC_DITHER must consume the same sample"
        );
        assert!(
            cmake.contains("otherMode, combinerColor.a, floor(vertexPosition.xy), fragmentNoise);")
        );
        assert!(cmake.contains("RT64 raster PS retains an unshared fragment-noise draw"));

        for mechanism in [
            "struct Fn64RdpFragmentNoiseSample",
            "Fn64RdpTakeFragmentNoiseSample(",
            "Fn64RdpFragmentNoiseUnitFloat(",
            "Fn64RdpFragmentNoiseLowThreeBits(",
            "otherMode.alphaDither() == G_AD_DISABLE",
            "AlphaDitherValue(",
            "Fn64RdpFragmentNoiseLowThreeBits(fragmentNoise));",
            "round(clamp(combinerAlpha, 0.0f, 1.0f) * 255.0f)",
            "(alpha8 & 7U) > threshold",
            "(rounded5 << 3U) | (rounded5 >> 2U)",
        ] {
            assert!(shader.contains(mechanism), "alpha policy lost {mechanism}");
        }
        assert_eq!(
            shader.matches("nextRandUint(").count(),
            1,
            "typed fragment sample must advance the generator exactly once"
        );
        assert!(!shader.contains("nextRand("));
        assert!(!shader.contains("shadeColor"));
        assert!(!shader.contains("fogColor"));
    }

    #[test]
    fn s2dex_object_rectangle_overlay_is_exact_guarded_and_bounded() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = if manifest_dir.join("src/ffi.rs").is_file() {
            manifest_dir.to_path_buf()
        } else {
            manifest_dir.join("../fn64-render-rt64")
        };
        let cmake = fs::read_to_string(root.join("ffi/CMakeLists.txt")).unwrap();
        for mechanism in [
            "7c8e779092eb7e2ddc8794694c0c04176a3e29958dc288952d577750673acc3f",
            "struct Fn64ObjSprite",
            "static_assert(sizeof(Fn64ObjSprite) == 24)",
            "static_assert(std::is_trivially_copyable_v<uObjTxtr>)",
            "G_OBJ_LDTX_RECT unsupported by bounded fn64 slice",
            "active microcode is not S2DEX2",
            "texture command is not exact public G_OBJLT_TXTRBLOCK",
            "objRenderMode is not zero",
            "RDP state is not point-filtered one-cycle mode",
            "RDP sampler state is not tile LOD, clamp detail, no TLUT, and no perspective",
            "scale is not exact 1:1 u5.10",
            "image is not non-paletted RGBA16",
            "image flip flags are nonzero",
            "block tsize, TMEM origin, and sprite stride/extent disagree",
            "block tline does not match the exact sprite stride",
            "block source range escapes physical RDRAM",
            "block source is not public 8-byte aligned",
            "block source uses non-public segment bits",
            "compound source range escapes physical RDRAM",
            "compound source is not public 8-byte aligned",
            "compound source uses non-public segment bits",
            "DMA length low24 is not exact public 0x2f",
            "std::memcpy(",
            "static_assert(offsetof(Fn64ObjSprite, imageFmt) == 23)",
            "(addressClass == 0x80U) || (addressClass == 0xA0U)",
            "uint64_t(state->rsp->segments[addressClass]) + uint64_t(offset)",
            "std::memcpy(&texture, objectBytes, sizeof(texture));",
            "void doFn64ObjLoadTxtr(",
            "doFn64ObjLoadTxtr(state, texture, imageAddress);",
            "doFn64ObjLoadTxRect(state, state->rsp->S2D.struct_buffer.data());",
            "fn64_rt64_gbi_s2dex.cpp",
            "TARGET_DIRECTORY rt64",
            "HEADER_FILE_ONLY TRUE",
        ] {
            assert!(cmake.contains(mechanism), "S2DEX overlay lost {mechanism}");
        }
        let validation = cmake
            .find("if (texture.type != 0x00001033U)")
            .expect("bounded validation remains present");
        let dma_validation = cmake
            .find("if (((*dl)->w0 & 0x00FFFFFFU) != 0x2FU)")
            .expect("exact compound DMA validation remains present");
        let object_validation = cmake
            .find("const uint32_t objectAddress = resolveFn64ObjRdramSpan(")
            .expect("compound pointer validation remains present");
        let sprite_read = cmake
            .find("state->fromRDRAM(objectAddress),")
            .expect("exact compound DMA read remains present");
        let mutation = cmake
            .find("doFn64ObjLoadTxtr(state, texture, imageAddress);")
            .expect("texture load remains present");
        assert!(
            validation < mutation,
            "unsupported shapes must reject before mutation"
        );
        assert!(
            dma_validation < object_validation && object_validation < sprite_read,
            "length and pointer validation must precede the compound read"
        );
        assert!(!cmake.contains("readS2DStruct(state, (*dl)->w1, 0x30U);"));
        assert!(!cmake.contains("doLoadTxtr(state, &texture);"));
        assert_eq!(
            cmake
                .matches("(uObjTxSprite*)state->rsp->S2D.struct_buffer.data()")
                .count(),
            1,
            "the typed cast may appear only in the upstream source guard, never the patch"
        );
        assert!(!cmake.contains("const Fn64ObjSprite *sprite = reinterpret_cast"));
    }
}
