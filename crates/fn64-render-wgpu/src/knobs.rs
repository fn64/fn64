//! Typed launch-time configuration for this crate, supplied by the host.
//!
//! Before task 2.2b, `WgpuBackend::try_new` read seven environment variables
//! directly through two ad-hoc helpers (`env_exact_one`/`env_default_one`) and
//! stored the results in seven loose `bool` fields. That made the backend's
//! policy invisible to its own callers: nothing in the type said which knobs
//! existed, what they defaulted to, or that a test could not set one without
//! mutating the process environment out from under every other test.
//!
//! [`WgpuKnobs`] is the whole of that surface, as one value. `Default` is the
//! documented default of every knob, byte-for-byte what the env helpers
//! produced with nothing set, so `try_new()` is exactly the old behavior with
//! an empty environment. [`ProbePolicy`] is what the backend actually holds:
//! the resolved booleans, decided ONCE at construction rather than re-read.
//!
//! `fn64-shell` builds a `WgpuKnobs` from its own `Knobs` (the process-wide
//! typed config surface) and passes it to [`WgpuBackend::try_new_with_knobs`].
//! Nothing in this crate reads the environment for these values any more.
//!
//! [`WgpuBackend::try_new_with_knobs`]: crate::WgpuBackend::try_new_with_knobs

/// Host-supplied launch-time configuration for [`crate::WgpuBackend`].
///
/// Every field's `Default` is the value the corresponding environment
/// variable produced when unset. See [`ProbePolicy::from_knobs`] for how the
/// two `gpu_triangle_draw`/`diagnostic_tmem_projection` knobs interact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuKnobs {
    /// `FN64_GPU_TRIANGLE_DRAW`: run the diagnostic GPU triangle pipeline.
    ///
    /// Default `false`. Note this is the KNOB's default, not the resolved
    /// policy's: under `cfg(test)` the resolved value is forced on, because
    /// the suites assert on the pipeline's output. On a play run it is ~65%
    /// of frame time and never reaches the screen.
    pub gpu_triangle_draw: bool,
    /// `FN64_DIAGNOSTIC_TMEM_PROJECTION`: build TMEM projections and complete
    /// GPU triangle fixtures WITHOUT submitting to the GPU.
    ///
    /// Default `false`. Exists so a performance measurement can restore the
    /// CPU preparation work alone as an exact in-process control.
    pub diagnostic_tmem_projection: bool,
    /// `FN64_COMPUTE_RASTER_PROBE`: explicit game-derived CPU/compute
    /// differential over the closed hottest-state batch. Default `false`.
    pub compute_raster_probe: bool,
    /// `FN64_COMPUTE_RASTER_CHAIN_PROBE`: the same CPU oracle, executing all
    /// typed batches as one ordered on-device target chain. Default `false`.
    pub compute_raster_chain_probe: bool,
    /// `FN64_COMPUTE_RASTER_REPLACE`: opt-in production A/B in which eligible
    /// all-triangle packets use the ordered compute chain as their color
    /// executor. Default `false`.
    pub compute_raster_replace: bool,
    /// `FN64_RAW_DPC_TASK_COMPUTE`: execute task batches on the compute
    /// rasterizer. Default `false` -- the compute shader still evaluates
    /// continuous attribute planes and cannot reproduce the RDP's masked
    /// scanline latch, so it stays an explicit diagnostic until that
    /// arithmetic and hidden-coverage publication are exact.
    pub raw_dpc_task_compute: bool,
    /// `FN64_RAW_DPC_TASK_CPU_COLOR_BATCH`: task-local CPU target
    /// accumulation with sparse per-packet publication.
    ///
    /// Default **`true`** -- the one knob here whose env helper was
    /// `env_default_one` rather than `env_exact_one`. Setting the variable to
    /// `0` was, and remains, the way to turn it off.
    pub raw_dpc_task_cpu_color_batch: bool,
}

impl Default for WgpuKnobs {
    fn default() -> Self {
        Self {
            gpu_triangle_draw: false,
            diagnostic_tmem_projection: false,
            compute_raster_probe: false,
            compute_raster_chain_probe: false,
            compute_raster_replace: false,
            raw_dpc_task_compute: false,
            raw_dpc_task_cpu_color_batch: true,
        }
    }
}

/// The resolved probe/diagnostic policy one [`crate::WgpuBackend`] holds for
/// its whole life. Constructed once, from a [`WgpuKnobs`], at `try_new`.
///
/// Separate from `WgpuKnobs` because two of these values are not simply the
/// knob: `gpu_triangle_draw` is forced on under `cfg(test)`, and
/// `project_gpu_tmem` is the OR of two knobs. Collapsing them would either
/// lose the distinction between "what the host asked for" and "what this
/// backend does", or push `cfg(test)` into the host's config type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbePolicy {
    /// Whether to run the diagnostic GPU triangle pipeline.
    pub(crate) gpu_triangle_draw_enabled: bool,
    /// Whether to build TMEM projections and complete GPU triangle fixtures.
    /// Normally identical to `gpu_triangle_draw_enabled`; the separate value
    /// exists so the projection work can be restored without also submitting.
    pub(crate) project_gpu_tmem: bool,
    pub(crate) compute_raster_probe_enabled: bool,
    pub(crate) compute_raster_chain_probe_enabled: bool,
    pub(crate) compute_raster_replace_enabled: bool,
    pub(crate) task_compute_raster_enabled: bool,
    pub(crate) task_cpu_color_batch_enabled: bool,
}

impl ProbePolicy {
    /// Resolve one backend's policy from the host's knobs.
    ///
    /// The two derivations this applies, both preserved exactly from the
    /// pre-2.2b `try_new`:
    ///
    /// - `gpu_triangle_draw_enabled` is `cfg!(test) || knob`. The test suites
    ///   assert on the pipeline's output, so a test build runs it whatever
    ///   the host asked for.
    /// - `project_gpu_tmem` is `gpu_triangle_draw_enabled || knob`. Drawing
    ///   implies projecting; the separate knob adds projection alone.
    pub fn from_knobs(knobs: &WgpuKnobs) -> Self {
        let gpu_triangle_draw_enabled = cfg!(test) || knobs.gpu_triangle_draw;
        Self {
            gpu_triangle_draw_enabled,
            project_gpu_tmem: gpu_triangle_draw_enabled || knobs.diagnostic_tmem_projection,
            compute_raster_probe_enabled: knobs.compute_raster_probe,
            compute_raster_chain_probe_enabled: knobs.compute_raster_chain_probe,
            compute_raster_replace_enabled: knobs.compute_raster_replace,
            task_compute_raster_enabled: knobs.raw_dpc_task_compute,
            task_cpu_color_batch_enabled: knobs.raw_dpc_task_cpu_color_batch,
        }
    }
}

impl Default for ProbePolicy {
    fn default() -> Self {
        Self::from_knobs(&WgpuKnobs::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The documented default of every knob, one assertion each.
    ///
    /// These are the values the deleted `env_exact_one`/`env_default_one`
    /// helpers produced with the variable unset. A change here is a change to
    /// what an unconfigured `fn64` does, which is exactly the class of silent
    /// drift task 2.2 exists to close.
    #[test]
    fn every_knob_default_is_the_documented_env_default() {
        let knobs = WgpuKnobs::default();
        // env_exact_one: absent => false.
        assert!(!knobs.gpu_triangle_draw, "FN64_GPU_TRIANGLE_DRAW");
        assert!(
            !knobs.diagnostic_tmem_projection,
            "FN64_DIAGNOSTIC_TMEM_PROJECTION"
        );
        assert!(!knobs.compute_raster_probe, "FN64_COMPUTE_RASTER_PROBE");
        assert!(
            !knobs.compute_raster_chain_probe,
            "FN64_COMPUTE_RASTER_CHAIN_PROBE"
        );
        assert!(!knobs.compute_raster_replace, "FN64_COMPUTE_RASTER_REPLACE");
        assert!(!knobs.raw_dpc_task_compute, "FN64_RAW_DPC_TASK_COMPUTE");
        // env_default_one: absent => true. The only one.
        assert!(
            knobs.raw_dpc_task_cpu_color_batch,
            "FN64_RAW_DPC_TASK_CPU_COLOR_BATCH defaults ON"
        );
    }

    /// `project_gpu_tmem` is the OR, and the triangle-draw knob alone
    /// implies it.
    #[test]
    fn projection_follows_the_draw_or_its_own_knob() {
        let draw_only = ProbePolicy::from_knobs(&WgpuKnobs {
            gpu_triangle_draw: true,
            ..WgpuKnobs::default()
        });
        assert!(draw_only.gpu_triangle_draw_enabled);
        assert!(draw_only.project_gpu_tmem, "drawing implies projecting");

        let projection_only = ProbePolicy::from_knobs(&WgpuKnobs {
            diagnostic_tmem_projection: true,
            ..WgpuKnobs::default()
        });
        assert!(projection_only.project_gpu_tmem);
        // Under cfg(test) the draw is forced on regardless, so the useful
        // assertion is the one below, not `!gpu_triangle_draw_enabled`.
    }

    /// A test build runs the diagnostic draw whatever the host asked for.
    #[test]
    fn a_test_build_forces_the_diagnostic_draw_on() {
        let policy = ProbePolicy::from_knobs(&WgpuKnobs::default());
        assert!(
            policy.gpu_triangle_draw_enabled,
            "cfg(test) forces the diagnostic triangle draw on"
        );
        assert!(policy.project_gpu_tmem, "and therefore the projection too");
    }

    /// The default policy is the default knobs' policy, with no separate
    /// hand-written copy of the values to drift.
    #[test]
    fn the_default_policy_is_the_default_knobs_policy() {
        assert_eq!(
            ProbePolicy::default(),
            ProbePolicy::from_knobs(&WgpuKnobs::default())
        );
        let policy = ProbePolicy::default();
        assert!(!policy.compute_raster_probe_enabled);
        assert!(!policy.compute_raster_chain_probe_enabled);
        assert!(!policy.compute_raster_replace_enabled);
        assert!(!policy.task_compute_raster_enabled);
        assert!(
            policy.task_cpu_color_batch_enabled,
            "the one default-ON knob survives the resolve"
        );
    }
}
