//! One place where every `host-gpu-tests` adapter request decides which
//! backends it will accept, so a CI runner without a physical GPU can run
//! the same tests against Mesa's Lavapipe (`llvmpipe`) software Vulkan
//! device.
//!
//! ## Why this module exists
//!
//! The crate's 33 `#[cfg(feature = "host-gpu-tests")]` tests were absent
//! from a default `cargo nextest run` entirely -- not skipped, not
//! reported, simply not compiled. A lane consequently described
//! `wgpu_backend_draws_a_real_texture_rectangle_at_its_own_wire_position`
//! as "pre-existing red" when a default run never contained it. Making the
//! gated set runnable on a GPU-less runner is what removes that hiding
//! place.
//!
//! ## What software-green does and does not prove
//!
//! Lavapipe is a **fourth rasterizer** alongside the N64 RDP, fn64's CPU
//! oracles, and the host's Metal/DX12/Vulkan driver. It has its own
//! rounding, its own interpolation precision, and its own tie-breaking at
//! pixel edges.
//!
//! - A **plumbing** test -- one whose assertions are CPU-side (uniform
//!   serialization, descriptor construction, plan/execute admission,
//!   pipeline caching) -- is genuinely covered by a software run. The GPU
//!   is present only so the code path can execute at all.
//! - A **pixel** test -- one asserting rasterized output values -- passing
//!   under Lavapipe is evidence about Lavapipe, **not** about any hardware
//!   driver. Software-green must never be read as hardware-green.
//!
//! `docs/RT64-GPU-TEST-MATRIX.md` carries the per-test classification.
//!
//! ## Selection
//!
//! `FN64_WGPU_SOFTWARE_ADAPTER=1` restricts every request in this crate to
//! `Backends::VULKAN` and requires the adapter that comes back to report
//! `DeviceType::Cpu`. Unset (the default) the behavior is byte-for-byte the
//! prior behavior: the full native backend mask and no device-type
//! constraint, so a developer on this Metal host sees no change.
//!
//! The env var is deliberately not a cargo feature. A feature would change
//! which `wgpu` features the crate compiles for *every* consumer of the
//! dependency graph, and `vulkan-portability` (the Apple-side enabler --
//! see below) is a `wgpu-core-deps-apple` unification knob whose effect is
//! link-time, not per-test. An env var lets one built test binary serve
//! both a hardware run and a software run, which is what makes the
//! side-by-side comparison in the matrix doc possible at all.
//!
//! ## The Apple gotcha, measured
//!
//! On Apple targets `wgpu`'s `vulkan` feature is a **no-op**:
//! `wgpu-core/Cargo.toml:96` routes it to
//! `wgpu-core-deps-windows-linux-android/vulkan`, and only
//! `vulkan-portability` (`:97`) reaches `wgpu-core-deps-apple`. With
//! `vulkan` alone this crate enumerated zero Vulkan adapters on a host
//! whose `vulkaninfo` listed `llvmpipe` correctly. Adding
//! `vulkan-portability` made the same host enumerate
//! `Vulkan | llvmpipe (LLVM 22.1.8, 128 bits) | Cpu`.
//!
//! `ash` also `dlopen`s a bare `libvulkan.dylib`, which is not on the
//! default macOS search path and which SIP strips `DYLD_LIBRARY_PATH` for;
//! `DYLD_FALLBACK_LIBRARY_PATH` is the variable that survives. A Linux CI
//! runner needs neither -- `libvulkan.so.1` resolves normally there.

/// Whether this process was asked to run against a software adapter.
///
/// Read from the environment on every call rather than cached: a test
/// binary is a single process, and caching would make the first test to
/// touch the flag decide for the rest, which is exactly the kind of
/// order-dependence this crate's tests are written to avoid.
pub(crate) fn software_adapter_requested() -> bool {
    matches!(
        std::env::var("FN64_WGPU_SOFTWARE_ADAPTER").as_deref(),
        Ok("1")
    )
}

/// The backend mask an adapter request should use, given the mask it would
/// have used on hardware.
///
/// Under the software flag this is `VULKAN` regardless of `native`, because
/// Lavapipe is a Vulkan ICD and neither Metal nor DX12 has a software
/// implementation this crate can reach. Otherwise `native` is returned
/// unchanged, so the default path is unmodified.
pub(crate) fn backends_for_request(native: wgpu::Backends) -> wgpu::Backends {
    if software_adapter_requested() {
        wgpu::Backends::VULKAN
    } else {
        native
    }
}

/// Panic unless the adapter that came back is the kind that was asked for.
///
/// This is the guard against the failure mode that let 33 tests hide: a run
/// that quietly succeeds while proving nothing. Under the software flag an
/// adapter reporting anything other than `DeviceType::Cpu` means the
/// intended software device was not the one selected -- a hardware Vulkan
/// driver answered instead -- and a "pass" from it would be mislabelled
/// evidence in the matrix doc. Off the flag this is a no-op, so hardware
/// runs are unaffected.
pub(crate) fn assert_expected_adapter(adapter: &wgpu::Adapter) {
    if !software_adapter_requested() {
        return;
    }
    let info = adapter.get_info();
    assert_eq!(
        info.device_type,
        wgpu::DeviceType::Cpu,
        "FN64_WGPU_SOFTWARE_ADAPTER=1 requires a software (Cpu) adapter, but the request \
         returned {:?} \"{}\" on the {:?} backend; a hardware adapter answering here would \
         label hardware results as software results in docs/RT64-GPU-TEST-MATRIX.md",
        info.device_type,
        info.name,
        info.backend,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mask rewrite is the whole of the software path's backend
    /// selection, so both directions are pinned. Asserted against the
    /// env-independent helper shape rather than by setting the variable:
    /// nextest runs tests in one process per binary but many threads, and
    /// mutating the environment mid-run would make neighbouring tests
    /// order-dependent.
    #[test]
    fn hardware_masks_pass_through_unchanged_when_the_flag_is_absent() {
        if software_adapter_requested() {
            return;
        }
        let native = wgpu::Backends::METAL | wgpu::Backends::VULKAN | wgpu::Backends::DX12;
        assert_eq!(backends_for_request(native), native);
        assert_eq!(
            backends_for_request(wgpu::Backends::METAL),
            wgpu::Backends::METAL
        );
    }

    /// Under the flag every request collapses to Vulkan, including one that
    /// natively asked for Metal only -- Lavapipe is a Vulkan ICD and there
    /// is no software Metal to fall back to.
    #[test]
    fn every_mask_collapses_to_vulkan_under_the_software_flag() {
        if !software_adapter_requested() {
            return;
        }
        for native in [
            wgpu::Backends::METAL | wgpu::Backends::VULKAN | wgpu::Backends::DX12,
            wgpu::Backends::METAL,
            wgpu::Backends::DX12,
            wgpu::Backends::VULKAN,
        ] {
            assert_eq!(backends_for_request(native), wgpu::Backends::VULKAN);
        }
    }

    /// `"1"` is the only accepted spelling; anything else -- including
    /// `"0"`, `"true"` and the empty string -- leaves the hardware path
    /// alone, so a stray export cannot silently reroute a hardware run.
    #[test]
    fn the_flag_reads_exactly_one_and_nothing_else() {
        let observed = std::env::var("FN64_WGPU_SOFTWARE_ADAPTER");
        assert_eq!(
            software_adapter_requested(),
            observed.as_deref() == Ok("1"),
            "software_adapter_requested must agree with an exact \"1\" match on the raw variable"
        );
    }
}
