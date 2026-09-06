//! The typed settings/policy staging surface.
//!
//! Split out of the former monolithic `RenderBackend` (see the parent
//! module). All three methods are defaulted to the same named
//! `RenderError::Backend` refusal they carried before the split, so a backend
//! with no typed settings writes `impl SettingsSink for MyBackend {}`.
//!
//! These stayed three separate methods rather than collapsing into one
//! `apply(SettingsScope)` because they do not share a return type:
//! `apply_runtime_settings` yields [`RenderSettingsApply`] (whose
//! `RestartRequired` variant carries the exact changed fields), while the
//! other two yield [`RenderPolicyApply`]. Every call site consumes that
//! value, so a single `Result<(), RenderError>` method would have to discard
//! information the callers match on.

use super::super::*;

/// Stage or live-apply typed renderer settings and pinned RT64 policy.
pub trait SettingsSink {
    /// Stage settings before `create`, or apply live-safe fields after it.
    /// Backends must return a named error for unsupported settings rather than
    /// retain the request while rendering with a different configuration.
    fn apply_runtime_settings(
        &mut self,
        _settings: &RenderRuntimeSettings,
    ) -> Result<RenderSettingsApply, RenderError> {
        Err(RenderError::Backend {
            backend: "render-runtime-settings",
            reason: "registered backend does not implement typed runtime settings".to_string(),
        })
    }

    /// Stage or live-apply the complete pinned RT64 enhancement policy.
    fn apply_enhancement_settings(
        &mut self,
        _settings: &RenderEnhancementSettings,
    ) -> Result<RenderPolicyApply, RenderError> {
        Err(RenderError::Backend {
            backend: "render-enhancement-settings",
            reason: "registered backend does not implement typed enhancement settings".to_string(),
        })
    }

    /// Stage or live-apply the complete pinned RT64 emulator/device policy.
    fn apply_emulator_settings(
        &mut self,
        _settings: &RenderEmulatorSettings,
    ) -> Result<RenderPolicyApply, RenderError> {
        Err(RenderError::Backend {
            backend: "render-emulator-settings",
            reason: "registered backend does not implement typed emulator settings".to_string(),
        })
    }
}
