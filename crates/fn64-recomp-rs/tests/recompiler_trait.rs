//! Integration test for the [`Recompiler`] trait impl: a `RecompConfig` in,
//! a generated typed-Rust module out. Proves `RsRecompiler` is a drop-in
//! alternative to the N64Recomp adapter (same trait, same `RecompOutput`
//! shape) and that stub/ignore lists and the ABI-version handshake are honored.

use fn64_recomp::{AbiVersion, Function, Patches, RecompConfig, Recompiler, Section};
use fn64_recomp_rs::RsRecompiler;

/// Assemble a tiny two-function ROM image and recompile it through the trait.
#[test]
fn recompile_produces_typed_rust_module() {
    // Two trivial functions back-to-back in a fake "code" section starting at
    // rom 0x0, vram 0x80000000.
    //   f_ret:   jr $ra ; nop                 (a bare return)
    //   f_addu:  addu $v0,$a0,$a1 ; jr $ra ; nop
    let words: [u32; 5] = [
        0x03E00008, // jr $ra
        0x00000000, // nop
        0x00851021, // addu $v0,$a0,$a1
        0x03E00008, // jr $ra
        0x00000000, // nop
    ];
    let mut rom = Vec::new();
    for w in words {
        rom.extend_from_slice(&w.to_be_bytes());
    }

    let dir = std::env::temp_dir().join(format!("fn64_recomp_rs_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rom_path = dir.join("tiny.z64");
    std::fs::write(&rom_path, &rom).unwrap();

    let cfg = RecompConfig {
        entrypoint: 0x8000_0000,
        rom_file_path: rom_path.clone(),
        bss_section_suffix: "_bss".to_string(),
        output_func_path: "RecompiledFuncs".into(),
        trace_mode: false,
        sections: vec![Section {
            name: "code".to_string(),
            rom: 0,
            vram: 0x8000_0000,
            size: rom.len() as u32,
            functions: vec![
                Function {
                    name: "f_ret".to_string(),
                    vram: 0x8000_0000,
                    size: 0x8,
                },
                Function {
                    name: "f_addu".to_string(),
                    vram: 0x8000_0008,
                    size: 0xC,
                },
            ],
        }],
        patches: Patches::default(),
    };

    let recomp = RsRecompiler::new(AbiVersion::new(1, 0));
    assert_eq!(recomp.abi_version(), AbiVersion::new(1, 0));

    let out = recomp.recompile(&cfg).expect("recompile should succeed");

    // Both functions recompiled, in order.
    assert_eq!(out.recompiled_functions, vec!["f_ret", "f_addu"]);
    assert_eq!(out.generated_files.len(), 1);

    let (path, src) = &out.generated_files[0];
    assert_eq!(path, &std::path::PathBuf::from("RecompiledFuncs/funcs.rs"));

    // The generated module must be typed Rust with no unsafe / no pointer casts.
    assert!(src.contains("pub fn f_ret(ctx: &mut RecompContext, mem: &mut Rdram)"));
    assert!(src.contains("pub fn f_addu(ctx: &mut RecompContext, mem: &mut Rdram)"));
    assert!(src.contains("pub const FN64_FUNCTION_ENTRY_OBSERVATION_SCHEMA"));
    assert!(src.contains("TranslatedFunctionIdentity::new(0x80000000, \"f_ret\")"));
    assert!(src.contains("TranslatedFunctionIdentity::new(0x80000008, \"f_addu\")"));
    assert!(src.contains("ctx.set_r32(2, (ctx.r_s32(4)).wrapping_add(ctx.r_s32(5)));"));
    // No `unsafe` blocks/fns and no pointer casts in the generated code. (The
    // banner comment says "no unsafe", so match the keyword usage, not the
    // word.)
    assert!(
        !src.contains("unsafe {"),
        "emitted code must never open an `unsafe` block"
    );
    assert!(
        !src.contains("unsafe fn"),
        "emitted code must never define an `unsafe fn`"
    );
    assert!(
        !src.contains("as *"),
        "emitted code must never cast a pointer"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A stubbed function name is skipped (not recompiled), matching the adapter's
/// `[patches].stubs` semantics.
#[test]
fn stubbed_function_is_skipped() {
    let words: [u32; 2] = [0x03E00008, 0x00000000]; // jr $ra ; nop
    let mut rom = Vec::new();
    for w in words {
        rom.extend_from_slice(&w.to_be_bytes());
    }
    let dir = std::env::temp_dir().join(format!("fn64_recomp_rs_stub_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rom_path = dir.join("tiny.z64");
    std::fs::write(&rom_path, &rom).unwrap();

    let mut patches = Patches::default();
    patches.stubs.push("stubbed".to_string());

    let cfg = RecompConfig {
        entrypoint: 0x8000_0000,
        rom_file_path: rom_path,
        bss_section_suffix: "_bss".to_string(),
        output_func_path: "RecompiledFuncs".into(),
        trace_mode: false,
        sections: vec![Section {
            name: "code".to_string(),
            rom: 0,
            vram: 0x8000_0000,
            size: rom.len() as u32,
            functions: vec![Function {
                name: "stubbed".to_string(),
                vram: 0x8000_0000,
                size: 0x8,
            }],
        }],
        patches,
    };

    let out = RsRecompiler::default().recompile(&cfg).unwrap();
    assert!(out.recompiled_functions.is_empty());
    assert!(!out.generated_files[0].1.contains("pub fn stubbed"));
    let _ = std::fs::remove_dir_all(
        std::env::temp_dir().join(format!("fn64_recomp_rs_stub_{}", std::process::id())),
    );
}

/// RSP recompile is out of scope for this CPU recompiler and must decline
/// loudly, not silently emit a wrong stub.
#[test]
fn rsp_recompile_declines_loudly() {
    use fn64_recomp::{RecompError, RspConfig};
    let cfg = RspConfig::new(0, 0, 0, "unused.z64", "some_ucode");
    let err = RsRecompiler::default().recompile_rsp(&cfg).unwrap_err();
    assert!(matches!(err, RecompError::InvalidConfig(_)));
}
