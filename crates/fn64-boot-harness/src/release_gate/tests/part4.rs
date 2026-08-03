use super::*;

#[test]
fn live_capture_without_arm_fails_before_sampling_ambient_state() {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("fn64-unarmed-release-{}.json", std::process::id()));
    let result = LiveReleaseGate::new(0).capture_and_write_observed(
        crate::CommittedViBoundary::synthetic_for_test(0),
        "unarmed",
        b"input",
        None,
        LiveObservedArtifacts {
            framebuffer_artifact_bytes: b"fb",
            framebuffer_payload_bytes: 2,
            observations: observations(),
        },
        path,
    );
    assert!(matches!(result, Err(GateError::LiveGateNotArmed)));
}

#[test]
fn missing_boundary_memory_has_its_own_error_before_geometry_validation() {
    assert!(matches!(
        require_boundary_physical_rdram(None),
        Err(GateError::BoundaryPhysicalRdramUnavailable)
    ));
}
