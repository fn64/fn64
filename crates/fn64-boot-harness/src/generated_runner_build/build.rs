#![allow(clippy::module_inception)]
use super::*;

impl BuildEnvironmentV3 {
    fn new(cargo: &Path, scratch: &Path) -> Result<Self, GeneratedRunnerBuildError> {
        let toolchain = cargo
            .parent()
            .ok_or_else(|| error("verified Cargo has no parent directory"))?;
        let rustc = toolchain.join(if cfg!(windows) { "rustc.exe" } else { "rustc" });
        let rustc_sha256 = sha256_file(&rustc, "verified Cargo sibling rustc")?;
        let home = scratch.join("build-home");
        let temp = scratch.join("build-temp");
        fs::create_dir(&home).map_err(|source| error(format!("create build HOME: {source}")))?;
        fs::create_dir(&temp).map_err(|source| error(format!("create build TMPDIR: {source}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&home, fs::Permissions::from_mode(0o700))
                .map_err(|source| error(format!("restrict build HOME: {source}")))?;
            fs::set_permissions(&temp, fs::Permissions::from_mode(0o700))
                .map_err(|source| error(format!("restrict build TMPDIR: {source}")))?;
        }
        let cargo_home = std::env::var_os("CARGO_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
            .ok_or_else(|| error("verified frozen build requires an explicit Cargo cache home"))?
            .canonicalize()
            .map_err(|source| error(format!("resolve Cargo cache home: {source}")))?;
        let path = std::env::join_paths([
            toolchain.to_path_buf(),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
            PathBuf::from("/usr/sbin"),
            PathBuf::from("/sbin"),
        ])
        .map_err(|source| error(format!("construct verified build PATH: {source}")))?;
        let cargo_config_sha256 = cargo_config_sha256_v3(&cargo_home, scratch)?;
        let mut digest = Sha256::new();
        digest.update(b"fn64.generated-runner-build-environment.v1\0");
        for (name, value) in [
            ("PATH", path.as_encoded_bytes()),
            ("HOME", home.as_os_str().as_encoded_bytes()),
            ("CARGO_HOME", cargo_home.as_os_str().as_encoded_bytes()),
            ("TMPDIR", temp.as_os_str().as_encoded_bytes()),
            ("RUSTC", rustc.as_os_str().as_encoded_bytes()),
            ("RUSTFLAGS", b""),
        ] {
            push_bytes(&mut digest, name.as_bytes());
            push_bytes(&mut digest, value);
        }
        digest.update(decode_sha256(&rustc_sha256)?);
        digest.update(decode_sha256(&cargo_config_sha256)?);
        Ok(Self {
            path,
            home,
            cargo_home,
            temp,
            rustc,
            identity_sha256: hex(&digest.finalize()),
            rustc_sha256,
            cargo_config_sha256,
        })
    }

    fn apply(&self, command: &mut Command) {
        command
            .env_clear()
            .env("PATH", &self.path)
            .env("HOME", &self.home)
            .env("CARGO_HOME", &self.cargo_home)
            .env("TMPDIR", &self.temp)
            .env("RUSTC", &self.rustc)
            .env("RUSTFLAGS", "")
            .env("CARGO_ENCODED_RUSTFLAGS", "");
    }

    fn revalidate(&self) -> Result<(), GeneratedRunnerBuildError> {
        if sha256_file(&self.rustc, "verified Cargo sibling rustc revalidation")?
            != self.rustc_sha256
            || cargo_config_sha256_v3(
                &self.cargo_home,
                self.home.parent().expect("build HOME has scratch parent"),
            )? != self.cargo_config_sha256
        {
            return Err(error("verified build toolchain environment changed"));
        }
        Ok(())
    }
}

pub(super) fn cargo_config_sha256_v3(
    cargo_home: &Path,
    cargo_current_dir: &Path,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.generated-runner-cargo-config.v1\0");
    let mut candidates = ["config", "config.toml"]
        .map(|name| cargo_home.join(name))
        .to_vec();
    for ancestor in cargo_current_dir.ancestors() {
        for name in ["config", "config.toml"] {
            candidates.push(ancestor.join(".cargo").join(name));
        }
    }
    for path in candidates {
        push_bytes(&mut digest, path.as_os_str().as_encoded_bytes());
        if path.exists() {
            digest.update([1]);
            let source = crate::private_fs::read_regular_stable(&path, "Cargo home config")
                .map_err(error)?;
            push_bytes(&mut digest, &source.contents);
        } else {
            digest.update([0]);
        }
    }
    Ok(hex(&digest.finalize()))
}

/// Build and select the exact repository-owned WM generated runner.
///
/// Cargo itself is the build authority already pinned by
/// `platform_certification`: this reuses that executable identity, invokes a
/// frozen standalone build in a fresh target directory, accepts exactly one
/// matching compiler artifact, and launches only its fixed identity mode.
pub fn build_generated_runner_v1(
    inputs: GeneratedRunnerBuildInputsV1,
) -> Result<VerifiedGeneratedRunnerBuildV1, GeneratedRunnerBuildError> {
    validate_inputs(&inputs)?;
    let workspace = repository_workspace()?;
    let package_root = game_package_root()?.join("wm2000-block-boot");
    let manifest = package_root.join("Cargo.toml");
    let lock = package_root.join("Cargo.lock");
    let manifest_sha256 = sha256_file(&manifest, "WM generated-runner manifest")?;
    let lock_sha256 = sha256_file(&lock, "WM generated-runner lockfile")?;
    let prepared_source_mode = prepared_source_mode_v3(&package_root)?;
    let expected_root_adapter_source_sha256 = root_adapter_source_sha256(&package_root)?;
    let expected_shard_source_sha256 =
        shard_cargo_source_sha256(&package_root, prepared_source_mode)?;
    let expected_emitter_source_sha256 = emitter_source_sha256(&workspace)?;
    let expected_runtime_source_sha256 =
        hex(&fn64_recomp_rs::generated_runner_runtime_source_receipt_v1().source_sha256());
    let prepared_claims = prepared_source_claims_v3(&workspace)?;
    let memory_guard = workspace.join("scripts/memory-guard.zsh");
    let memory_guard_sha256 = validate_memory_guard(&memory_guard)?;
    let cargo = crate::platform_certification::verified_build_cargo()
        .map_err(|source| error(format!("verify Cargo build owner: {source}")))?;
    let builder_cargo_sha256 = env!("FN64_BUILD_CARGO_SHA256").to_owned();
    let mut nonce = [0u8; 32];
    getrandom::fill(&mut nonce)
        .map_err(|source| error(format!("obtain generated-runner build nonce: {source}")))?;
    let scratch = ScratchDirectory::create(&nonce)?;
    let build_environment = BuildEnvironmentV3::new(&cargo, scratch.path())?;
    let staged_inputs = stage_private_inputs(&inputs, scratch.path())?;

    let private_build_inputs_sha256 = private_inputs_sha256(&staged_inputs)?;
    let expected_normalized_rom_sha256 = normalized_rom_sha256(&staged_inputs.rom)?;
    let producer = build_prepared_producer_v3(
        &memory_guard,
        &cargo,
        &build_environment,
        &workspace,
        scratch.path(),
        staged_inputs.max_build_seconds,
    )?;
    build_environment.revalidate()?;
    if prepared_source_claims_v3(&workspace)? != prepared_claims {
        return Err(error(
            "prepared source claims changed during producer build",
        ));
    }
    let prepared = invoke_prepared_producer_v3(
        &memory_guard,
        &producer,
        &build_environment,
        &staged_inputs.rom,
        &prepared_claims,
        &expected_normalized_rom_sha256,
        scratch.path(),
        staged_inputs.max_build_seconds,
    )?;
    build_environment.revalidate()?;
    if prepared_source_claims_v3(&workspace)? != prepared_claims {
        return Err(error("prepared source claims changed during publication"));
    }
    let metadata = run_cargo_metadata(&cargo, &build_environment, &manifest, scratch.path())?;
    let cargo_graph_sha256 = hex(&Sha256::digest(&metadata));
    let cargo_source_sha256 = cargo_metadata_source_sha256(&metadata)?;
    if measure_prepared_tree_v3(
        &prepared.root,
        &expected_normalized_rom_sha256,
        &prepared_claims,
    )? != prepared
    {
        return Err(error("prepared tree changed before the owned Cargo build"));
    }
    let selected = build_selected_binary(
        &memory_guard,
        &cargo,
        &manifest,
        &staged_inputs,
        &prepared,
        &producer,
        prepared_source_mode,
        &build_environment,
        scratch.path(),
    )?;
    build_environment.revalidate()?;
    if prepared_source_claims_v3(&workspace)? != prepared_claims
        || prepared_source_mode_v3(&package_root)? != prepared_source_mode
    {
        return Err(error(
            "prepared source authority changed during Cargo build",
        ));
    }
    crate::platform_certification::verified_build_cargo()
        .map_err(|source| error(format!("reverify Cargo build owner: {source}")))?;
    if measure_prepared_tree_v3(
        &prepared.root,
        &expected_normalized_rom_sha256,
        &prepared_claims,
    )? != prepared
    {
        return Err(error("prepared tree changed during the owned Cargo build"));
    }
    if private_inputs_sha256(&staged_inputs)? != private_build_inputs_sha256 {
        return Err(error(
            "private generated-runner build inputs changed during Cargo build",
        ));
    }
    let selected_binary_sha256 = sha256_file(&selected, "built generated runner")?;
    let staged = stage_selected_binary(&selected, scratch.path(), &selected_binary_sha256)?;
    let identity = launch_identity_child(&staged, scratch.path())?;
    build_environment.revalidate()?;
    validate_identity(&identity, &manifest_sha256, &lock_sha256)?;
    if identity.root_adapter_source_sha256 != expected_root_adapter_source_sha256
        || identity.shard_cargo_source_tree_sha256 != expected_shard_source_sha256
        || identity.emitter_source_sha256 != expected_emitter_source_sha256
        || identity.runtime_source_sha256 != expected_runtime_source_sha256
    {
        return Err(error(
            "generated-runner child source attestation does not match verifier-measured source domains",
        ));
    }
    validate_prepared_identity_v3(&identity, &prepared, &producer, prepared_source_mode)?;
    if prepared_source_claims_v3(&workspace)? != prepared_claims
        || prepared_source_mode_v3(&package_root)? != prepared_source_mode
    {
        return Err(error(
            "prepared source authority changed during identity child",
        ));
    }
    revalidate_prepared_producer_v3(
        &producer,
        &cargo,
        &build_environment,
        &workspace,
        scratch.path(),
    )?;
    if measure_prepared_tree_v3(
        &prepared.root,
        &expected_normalized_rom_sha256,
        &prepared_claims,
    )? != prepared
    {
        return Err(error(
            "prepared tree changed during Cargo or identity child",
        ));
    }
    if sha256_file(&staged, "staged generated runner after identity launch")?
        != selected_binary_sha256
    {
        return Err(error(
            "selected generated runner changed during identity launch",
        ));
    }
    if private_inputs_sha256(&staged_inputs)? != private_build_inputs_sha256 {
        return Err(error(
            "private generated-runner build inputs changed during identity launch",
        ));
    }
    if sha256_file(&manifest, "WM generated-runner manifest after build")? != manifest_sha256
        || sha256_file(&lock, "WM generated-runner lockfile after build")? != lock_sha256
    {
        return Err(error(
            "generated-runner manifest or lockfile changed during the owned build",
        ));
    }
    let metadata_after = run_cargo_metadata(&cargo, &build_environment, &manifest, scratch.path())?;
    if hex(&Sha256::digest(&metadata_after)) != cargo_graph_sha256
        || cargo_metadata_source_sha256(&metadata_after)? != cargo_source_sha256
    {
        return Err(error(
            "generated-runner Cargo graph or package sources changed during the owned build",
        ));
    }
    crate::platform_certification::verified_build_cargo().map_err(|source| {
        error(format!(
            "reverify Cargo owner after identity launch: {source}"
        ))
    })?;
    if validate_memory_guard(&memory_guard)? != memory_guard_sha256 {
        return Err(error(
            "generated-runner process-group memory guard changed during the owned build",
        ));
    }
    build_environment.revalidate()?;
    fs::remove_dir_all(scratch.path().join("build-target")).map_err(|source| {
        error(format!(
            "remove completed generated-runner Cargo target from verifier scratch: {source}"
        ))
    })?;
    fs::remove_dir_all(scratch.path().join("producer-target")).map_err(|source| {
        error(format!(
            "remove completed prepared-producer Cargo target from verifier scratch: {source}"
        ))
    })?;

    let mut evidence = GeneratedRunnerBuildEvidenceV1 {
        schema: VERIFIED_GENERATED_RUNNER_BUILD_SCHEMA_V5,
        builder_cargo_sha256,
        cargo_graph_sha256,
        cargo_source_sha256,
        build_environment_sha256: build_environment.identity_sha256,
        builder_rustc_sha256: build_environment.rustc_sha256,
        cargo_config_sha256: build_environment.cargo_config_sha256,
        memory_guard_sha256,
        selected_build_cargo_jobs: SELECTED_BUILD_CARGO_JOBS_V5,
        build_max_rss_mib: BUILD_MAX_RSS_MIB,
        build_min_free_percent: BUILD_MIN_FREE_PERCENT,
        max_build_seconds: staged_inputs.max_build_seconds,
        selected_binary_sha256,
        private_build_inputs_sha256,
        prepared_tree_descriptor_sha256: prepared.descriptor_sha256.clone(),
        prepared_tree_sha256: prepared.tree_sha256.clone(),
        prepared_source_mode: prepared_source_mode.to_owned(),
        producer_manifest_sha256: producer.manifest_sha256.clone(),
        producer_lock_sha256: producer.lock_sha256.clone(),
        producer_cargo_graph_sha256: producer.cargo_graph_sha256.clone(),
        producer_cargo_source_sha256: producer.cargo_source_sha256.clone(),
        producer_binary_sha256: producer.binary_sha256.clone(),
        identity,
        authority_sha256: String::new(),
    };
    evidence.authority_sha256 = evidence.recompute_authority_sha256();
    evidence.verify_integrity()?;
    Ok(VerifiedGeneratedRunnerBuildV1 {
        evidence,
        selected_binary: staged,
        private_inputs: staged_inputs,
        prepared,
        producer,
        _scratch: scratch,
    })
}

impl GeneratedRunnerBuildEvidenceV1 {
    pub(super) fn verify_integrity(&self) -> Result<(), GeneratedRunnerBuildError> {
        if self.schema != VERIFIED_GENERATED_RUNNER_BUILD_SCHEMA_V5 {
            return Err(error("unsupported verified generated-runner build schema"));
        }
        for (field, digest) in [
            ("builder_cargo_sha256", &self.builder_cargo_sha256),
            ("cargo_graph_sha256", &self.cargo_graph_sha256),
            ("cargo_source_sha256", &self.cargo_source_sha256),
            ("build_environment_sha256", &self.build_environment_sha256),
            ("builder_rustc_sha256", &self.builder_rustc_sha256),
            ("cargo_config_sha256", &self.cargo_config_sha256),
            ("memory_guard_sha256", &self.memory_guard_sha256),
            ("selected_binary_sha256", &self.selected_binary_sha256),
            (
                "private_build_inputs_sha256",
                &self.private_build_inputs_sha256,
            ),
            (
                "prepared_tree_descriptor_sha256",
                &self.prepared_tree_descriptor_sha256,
            ),
            ("prepared_tree_sha256", &self.prepared_tree_sha256),
            ("producer_manifest_sha256", &self.producer_manifest_sha256),
            ("producer_lock_sha256", &self.producer_lock_sha256),
            (
                "producer_cargo_graph_sha256",
                &self.producer_cargo_graph_sha256,
            ),
            (
                "producer_cargo_source_sha256",
                &self.producer_cargo_source_sha256,
            ),
            ("producer_binary_sha256", &self.producer_binary_sha256),
            ("authority_sha256", &self.authority_sha256),
        ] {
            require_sha256(digest, field)?;
        }
        if self.selected_build_cargo_jobs != SELECTED_BUILD_CARGO_JOBS_V5 {
            return Err(error(format!(
                "generated-runner build evidence requires exactly {SELECTED_BUILD_CARGO_JOBS_V5} selected-build Cargo jobs"
            )));
        }
        if self.build_max_rss_mib != BUILD_MAX_RSS_MIB
            || self.build_min_free_percent != BUILD_MIN_FREE_PERCENT
            || !(MIN_BUILD_TIMEOUT_SECONDS..=MAX_BUILD_TIMEOUT_SECONDS)
                .contains(&self.max_build_seconds)
        {
            return Err(error(
                "generated-runner build evidence has a noncanonical safety envelope",
            ));
        }
        if !matches!(
            self.prepared_source_mode.as_str(),
            PREPARED_SOURCE_MODE_INACTIVE_V1 | PREPARED_SOURCE_MODE_CONSUMED_V1
        ) || self.prepared_source_mode != self.identity.prepared_source_mode
        {
            return Err(error(
                "generated-runner build has an invalid prepared source mode",
            ));
        }
        validate_identity(
            &self.identity,
            &self.identity.manifest_sha256,
            &self.identity.lock_sha256,
        )?;
        if self.prepared_tree_sha256 != self.identity.prepared_tree_sha256
            || self.producer_manifest_sha256 != self.identity.producer_manifest_sha256
            || self.producer_lock_sha256 != self.identity.producer_lock_sha256
            || self.producer_cargo_graph_sha256 != self.identity.producer_cargo_graph_sha256
            || self.producer_cargo_source_sha256 != self.identity.producer_cargo_source_sha256
            || self.producer_binary_sha256 != self.identity.producer_binary_sha256
        {
            return Err(error(
                "generated-runner build evidence differs from its child prepared authority",
            ));
        }
        let recomputed = self.recompute_authority_sha256();
        if recomputed != self.authority_sha256 {
            return Err(error(format!(
                "generated-runner build authority digest mismatch: stored={}, recomputed={recomputed}",
                self.authority_sha256
            )));
        }
        Ok(())
    }

    pub(super) fn recompute_authority_sha256(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"fn64.verified-generated-runner-build.v5\0");
        for bytes in [
            self.schema.as_bytes(),
            self.builder_cargo_sha256.as_bytes(),
            self.cargo_graph_sha256.as_bytes(),
            self.cargo_source_sha256.as_bytes(),
            self.build_environment_sha256.as_bytes(),
            self.builder_rustc_sha256.as_bytes(),
            self.cargo_config_sha256.as_bytes(),
            self.memory_guard_sha256.as_bytes(),
            self.selected_binary_sha256.as_bytes(),
            self.private_build_inputs_sha256.as_bytes(),
            self.prepared_tree_descriptor_sha256.as_bytes(),
            self.prepared_tree_sha256.as_bytes(),
            self.producer_manifest_sha256.as_bytes(),
            self.producer_lock_sha256.as_bytes(),
            self.producer_cargo_graph_sha256.as_bytes(),
            self.producer_cargo_source_sha256.as_bytes(),
            self.producer_binary_sha256.as_bytes(),
            self.prepared_source_mode.as_bytes(),
        ] {
            push_bytes(&mut digest, bytes);
        }
        digest.update(self.selected_build_cargo_jobs.to_be_bytes());
        digest.update(self.build_max_rss_mib.to_be_bytes());
        digest.update([self.build_min_free_percent]);
        digest.update(self.max_build_seconds.to_be_bytes());
        let identity = serde_json::to_vec(&self.identity)
            .expect("generated-runner build identity serialization is infallible");
        push_bytes(&mut digest, &identity);
        hex(&digest.finalize())
    }
}

pub(super) fn validate_identity(
    identity: &GeneratedRunnerBuildIdentityV1,
    expected_manifest_sha256: &str,
    expected_lock_sha256: &str,
) -> Result<(), GeneratedRunnerBuildError> {
    if identity.schema != GENERATED_RUNNER_BUILD_IDENTITY_SCHEMA_V3
        || identity.package != PACKAGE
        || identity.source_attestation_schema
            != fn64_recomp_rs::GENERATED_RUNNER_SOURCE_ATTESTATION_SCHEMA_V2
    {
        return Err(error(
            "generated-runner child reported an unsupported identity envelope",
        ));
    }
    if identity.manifest_sha256 != expected_manifest_sha256
        || identity.lock_sha256 != expected_lock_sha256
    {
        return Err(error(
            "generated-runner child manifest/lock identity does not match the verifier-owned build",
        ));
    }
    if !identity.cargo_source_fields_validated {
        return Err(error(
            "generated-runner child did not validate its Cargo source fields",
        ));
    }
    if !matches!(
        identity.prepared_source_mode.as_str(),
        PREPARED_SOURCE_MODE_INACTIVE_V1 | PREPARED_SOURCE_MODE_CONSUMED_V1
    ) {
        return Err(error(
            "generated-runner child has an invalid prepared source mode",
        ));
    }
    for (field, digest) in [
        ("manifest_sha256", &identity.manifest_sha256),
        ("lock_sha256", &identity.lock_sha256),
        ("program_identity_sha256", &identity.program_identity_sha256),
        (
            "root_adapter_source_sha256",
            &identity.root_adapter_source_sha256,
        ),
        (
            "shard_cargo_source_tree_sha256",
            &identity.shard_cargo_source_tree_sha256,
        ),
        ("emitter_source_sha256", &identity.emitter_source_sha256),
        ("runtime_source_sha256", &identity.runtime_source_sha256),
        ("normalized_rom_sha256", &identity.normalized_rom_sha256),
        (
            "prepared_manifest_sha256",
            &identity.prepared_manifest_sha256,
        ),
        ("prepared_tree_sha256", &identity.prepared_tree_sha256),
        (
            "prepared_generator_source_sha256",
            &identity.prepared_generator_source_sha256,
        ),
        (
            "prepared_discovery_source_sha256",
            &identity.prepared_discovery_source_sha256,
        ),
        (
            "prepared_emitter_source_sha256",
            &identity.prepared_emitter_source_sha256,
        ),
        (
            "prepared_runtime_source_sha256",
            &identity.prepared_runtime_source_sha256,
        ),
        (
            "prepared_materializer_source_sha256",
            &identity.prepared_materializer_source_sha256,
        ),
        (
            "producer_manifest_sha256",
            &identity.producer_manifest_sha256,
        ),
        ("producer_lock_sha256", &identity.producer_lock_sha256),
        (
            "producer_cargo_graph_sha256",
            &identity.producer_cargo_graph_sha256,
        ),
        (
            "producer_cargo_source_sha256",
            &identity.producer_cargo_source_sha256,
        ),
        ("producer_binary_sha256", &identity.producer_binary_sha256),
        ("binding_sha256", &identity.binding_sha256),
    ] {
        require_sha256(digest, field)?;
    }
    if identity.build_receipt_schema != 1
        || !identity.aot_runtime
        || !identity.production_aot
        || identity.dev_interpreter
    {
        return Err(error(
            "selected generated runner is not the production-AOT feature artifact",
        ));
    }
    if identity.runners.is_empty() {
        return Err(error("generated-runner child reported no linked runners"));
    }
    let mut prior = None;
    for runner in &identity.runners {
        if prior.is_some_and(|bank| bank >= runner.bank) {
            return Err(error(
                "generated-runner identities are not in strictly increasing bank order",
            ));
        }
        prior = Some(runner.bank);
        require_sha256(
            &runner.generated_runner_source_sha256,
            "runners[].generated_runner_source_sha256",
        )?;
        require_sha256(&runner.code_words_sha256, "runners[].code_words_sha256")?;
        if runner.vram_start & 3 != 0
            || runner.vram_end & 3 != 0
            || runner.vram_start >= runner.vram_end
            || runner.composite_subrunner_count == 0
        {
            return Err(error(
                "generated-runner child reported invalid code geometry",
            ));
        }
    }
    let recomputed = recompute_binding_sha256(identity)?;
    if recomputed != identity.binding_sha256 {
        return Err(error(format!(
            "generated-runner binding digest mismatch: child={}, recomputed={recomputed}",
            identity.binding_sha256
        )));
    }
    Ok(())
}

pub(super) fn validate_prepared_identity_v3(
    identity: &GeneratedRunnerBuildIdentityV1,
    prepared: &PreparedTreeMeasurementV3,
    producer: &ProducerBuildMeasurementV3,
    prepared_source_mode: &str,
) -> Result<(), GeneratedRunnerBuildError> {
    if identity.prepared_source_mode != prepared_source_mode {
        return Err(error(
            "generated-runner child source mode differs from the exact shard manifests",
        ));
    }
    let pairs = [
        (
            &identity.normalized_rom_sha256,
            &prepared.normalized_rom_sha256,
        ),
        (
            &identity.prepared_manifest_sha256,
            &prepared.manifest_sha256,
        ),
        (&identity.prepared_tree_sha256, &prepared.tree_sha256),
        (
            &identity.prepared_generator_source_sha256,
            &prepared.claims.generator_source_sha256,
        ),
        (
            &identity.prepared_discovery_source_sha256,
            &prepared.claims.discovery_source_sha256,
        ),
        (
            &identity.prepared_emitter_source_sha256,
            &prepared.claims.emitter_source_sha256,
        ),
        (
            &identity.prepared_runtime_source_sha256,
            &prepared.claims.runtime_source_sha256,
        ),
        (
            &identity.prepared_materializer_source_sha256,
            &prepared.claims.materializer_source_sha256,
        ),
        (
            &identity.producer_manifest_sha256,
            &producer.manifest_sha256,
        ),
        (&identity.producer_lock_sha256, &producer.lock_sha256),
        (
            &identity.producer_cargo_graph_sha256,
            &producer.cargo_graph_sha256,
        ),
        (
            &identity.producer_cargo_source_sha256,
            &producer.cargo_source_sha256,
        ),
        (&identity.producer_binary_sha256, &producer.binary_sha256),
    ];
    if pairs
        .iter()
        .any(|(observed, expected)| observed != expected)
    {
        return Err(error(
            "generated-runner child prepared identity differs from verifier measurements",
        ));
    }
    Ok(())
}

pub(super) fn recompute_binding_sha256(
    identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(fn64_recomp_rs::GENERATED_RUNNER_SOURCE_BINDING_DOMAIN_V2);
    for value in [
        &identity.program_identity_sha256,
        &identity.root_adapter_source_sha256,
        &identity.shard_cargo_source_tree_sha256,
        &identity.emitter_source_sha256,
        &identity.runtime_source_sha256,
    ] {
        digest.update(decode_sha256(value)?);
    }
    for runner in &identity.runners {
        digest.update(runner.bank.to_be_bytes());
        digest.update(decode_sha256(&runner.generated_runner_source_sha256)?);
        digest.update(decode_sha256(&runner.code_words_sha256)?);
        digest.update(runner.vram_start.to_be_bytes());
        digest.update(runner.vram_end.to_be_bytes());
        digest.update(runner.composite_subrunner_count.to_be_bytes());
        digest.update([runner.adapter_role.tag()]);
    }
    digest.update(identity.build_receipt_schema.to_be_bytes());
    digest.update([
        u8::from(identity.aot_runtime),
        u8::from(identity.production_aot),
        u8::from(identity.dev_interpreter),
    ]);
    Ok(hex(&digest.finalize()))
}

pub(super) fn run_cargo_metadata(
    cargo: &Path,
    environment: &BuildEnvironmentV3,
    manifest: &Path,
    scratch: &Path,
) -> Result<Vec<u8>, GeneratedRunnerBuildError> {
    let mut command = Command::new(cargo);
    environment.apply(&mut command);
    let output = command
        .arg("metadata")
        .arg("--frozen")
        .arg("--format-version=1")
        .arg("--manifest-path")
        .arg(manifest)
        .current_dir(scratch)
        .output()
        .map_err(|source| error(format!("run frozen Cargo metadata: {source}")))?;
    fs::write(scratch.join("cargo-metadata.stderr.log"), &output.stderr)
        .map_err(|source| error(format!("write Cargo metadata log: {source}")))?;
    if !output.status.success() {
        return Err(error(format!(
            "frozen Cargo metadata failed {}; stderr: {}",
            output.status,
            bounded_diagnostic(&output.stderr),
        )));
    }
    Ok(output.stdout)
}

pub(super) fn bounded_diagnostic(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    if text.is_empty() {
        "<empty>".to_owned()
    } else {
        let mut tail = text.chars().rev().take(4096).collect::<Vec<_>>();
        let truncated = tail.len() < text.chars().count();
        tail.reverse();
        let diagnostic: String = tail.into_iter().collect();
        if truncated {
            format!("<earlier output truncated>\n{diagnostic}")
        } else {
            diagnostic
        }
    }
}

pub(super) fn bounded_diagnostic_file(path: &Path) -> String {
    match fs::read(path) {
        Ok(bytes) => bounded_diagnostic(&bytes),
        Err(source) => format!("<cannot read diagnostic: {source}>"),
    }
}

pub(super) fn cargo_build_progress(bytes: &[u8]) -> String {
    let Ok(source) = std::str::from_utf8(bytes) else {
        return "compiler_artifacts=unreadable".to_owned();
    };
    let expected = PREPARED_PACKAGES
        .iter()
        .map(|package| package.replace('-', "_"))
        .collect::<BTreeSet<_>>();
    let mut completed_shards = BTreeSet::new();
    let mut compiler_artifacts = 0usize;
    let mut root_binary = false;
    for line in source.lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if message["reason"] != "compiler-artifact" {
            continue;
        }
        compiler_artifacts += 1;
        let Some(name) = message["target"]["name"].as_str() else {
            continue;
        };
        let kinds = message["target"]["kind"].as_array();
        if expected.contains(name)
            && kinds.is_some_and(|kinds| kinds.iter().any(|kind| kind == "lib"))
        {
            completed_shards.insert(name.to_owned());
        }
        if name == PACKAGE && kinds.is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin")) {
            root_binary = true;
        }
    }
    format!(
        "compiler_artifacts={compiler_artifacts} completed_shards={}/{} root_binary={}",
        completed_shards.len(),
        PREPARED_PACKAGES.len(),
        u8::from(root_binary),
    )
}

pub(super) fn cargo_metadata_source_sha256(metadata: &[u8]) -> Result<String, GeneratedRunnerBuildError> {
    let document: serde_json::Value = serde_json::from_slice(metadata)
        .map_err(|source| error(format!("parse Cargo metadata: {source}")))?;
    let packages = document["packages"]
        .as_array()
        .ok_or_else(|| error("Cargo metadata has no packages array"))?;
    let mut roots = packages
        .iter()
        .map(|package| {
            let id = package["id"]
                .as_str()
                .ok_or_else(|| error("Cargo metadata package has no id"))?;
            let manifest = package["manifest_path"]
                .as_str()
                .ok_or_else(|| error("Cargo metadata package has no manifest_path"))?;
            let root = PathBuf::from(manifest)
                .parent()
                .ok_or_else(|| error("Cargo package manifest has no parent"))?
                .to_path_buf();
            Ok((id.to_owned(), root))
        })
        .collect::<Result<Vec<_>, GeneratedRunnerBuildError>>()?;
    roots.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    digest.update(b"fn64.generated-runner-cargo-source-graph.v1\0");
    for (id, root) in roots {
        push_bytes(&mut digest, id.as_bytes());
        let mut files = Vec::new();
        collect_package_files(&root, &root, &mut files)?;
        files.sort();
        for file in files {
            let relative = file
                .strip_prefix(&root)
                .expect("collected package file remains under root");
            push_bytes(
                &mut digest,
                relative.to_string_lossy().replace('\\', "/").as_bytes(),
            );
            let source = crate::private_fs::read_regular_stable(&file, "Cargo package source")
                .map_err(error)?;
            push_bytes(&mut digest, &source.contents);
        }
    }
    Ok(hex(&digest.finalize()))
}

pub(super) fn root_adapter_source_sha256(package_root: &Path) -> Result<String, GeneratedRunnerBuildError> {
    source_tree_sha256(
        package_root,
        b"fn64:wm2000-root-adapter-source:v1:",
        &["Cargo.toml", "Cargo.lock", "build.rs", "src/main.rs"],
    )
}

pub(super) fn shard_root(package_root: &Path) -> Result<PathBuf, GeneratedRunnerBuildError> {
    Ok(package_root
        .parent()
        .ok_or_else(|| error("WM root package has no examples parent"))?
        .join(env!("FN64_WM_SHARD_DIR")))
}

pub(super) fn shard_cargo_source_sha256(
    package_root: &Path,
    prepared_source_mode: &str,
) -> Result<String, GeneratedRunnerBuildError> {
    let shard_root = shard_root(package_root)?;
    let mut files = vec![(
        format!("../{}/lib.rs", env!("FN64_WM_SHARD_DIR")),
        shard_root.join("lib.rs"),
    )];
    match prepared_source_mode {
        PREPARED_SOURCE_MODE_INACTIVE_V1 => {
            files.push((
                format!("../{}/build.rs", env!("FN64_WM_SHARD_DIR")),
                shard_root.join("build.rs"),
            ));
        }
        PREPARED_SOURCE_MODE_CONSUMED_V1 => {
            files.push((
                format!("../{}/prepared_build.rs", env!("FN64_WM_SHARD_DIR")),
                shard_root.join("prepared_build.rs"),
            ));
            files.push((
                format!("../{}/materializer.rs", env!("FN64_WM_SHARD_DIR")),
                shard_root.join("materializer.rs"),
            ));
        }
        _ => return Err(error("unsupported prepared source mode")),
    }
    let manifests = exact_shard_manifests(&shard_root)?;
    for manifest in manifests {
        let relative = manifest.strip_prefix(&shard_root).map_err(|_| {
            error(format!(
                "WM shard manifest escaped shard source graph: {}",
                manifest.display()
            ))
        })?;
        files.push((
            format!(
                "../{}/{}",
                env!("FN64_WM_SHARD_DIR"),
                relative.to_string_lossy().replace('\\', "/")
            ),
            manifest,
        ));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    digest.update(b"fn64:wm2000-shard-cargo-source-tree:v1:");
    for (label, path) in files {
        push_bytes(&mut digest, label.as_bytes());
        let source = crate::private_fs::read_regular_stable(&path, "WM shard Cargo source")
            .map_err(error)?;
        push_bytes(&mut digest, &source.contents);
    }
    Ok(hex(&digest.finalize()))
}

pub(super) fn prepared_source_mode_v3(
    package_root: &Path,
) -> Result<&'static str, GeneratedRunnerBuildError> {
    let shard_root = shard_root(package_root)?;
    let manifests = exact_shard_manifests(&shard_root)?;
    let mut legacy = 0usize;
    let mut prepared = 0usize;
    for manifest in manifests {
        let source = crate::private_fs::read_regular_stable(&manifest, "WM shard manifest")
            .map_err(error)?;
        let text = std::str::from_utf8(&source.contents)
            .map_err(|source| error(format!("WM shard manifest is not UTF-8: {source}")))?;
        legacy += usize::from(
            text.lines()
                .filter(|line| line.trim() == "build = \"../build.rs\"")
                .count()
                == 1,
        );
        prepared += usize::from(
            text.lines()
                .filter(|line| line.trim() == "build = \"../prepared_build.rs\"")
                .count()
                == 1,
        );
    }
    match (legacy, prepared) {
        (count, 0) if count == SHARD_COUNT => Ok(PREPARED_SOURCE_MODE_INACTIVE_V1),
        (0, count) if count == SHARD_COUNT => Ok(PREPARED_SOURCE_MODE_CONSUMED_V1),
        _ => Err(error(
            "WM shard manifests mix or omit legacy/prepared source modes",
        )),
    }
}

pub(super) fn exact_shard_manifests(shard_root: &Path) -> Result<Vec<PathBuf>, GeneratedRunnerBuildError> {
    let expected = SHARD_MANIFEST_DIRS
        .iter()
        .map(|directory| shard_root.join(directory).join("Cargo.toml"))
        .collect::<Vec<_>>();
    if expected.iter().any(|path| !path.is_file()) {
        return Err(error(
            "WM shard manifest inventory is missing an expected package",
        ));
    }
    let mut observed = fs::read_dir(shard_root)
        .map_err(|source| error(format!("enumerate WM shard manifests: {source}")))?
        .map(|entry| {
            entry
                .map(|entry| entry.path().join("Cargo.toml"))
                .map_err(|source| error(format!("enumerate WM shard manifest: {source}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    observed.retain(|path| path.is_file());
    observed.sort();
    let mut expected_sorted = expected.clone();
    expected_sorted.sort();
    if observed != expected_sorted {
        return Err(error("WM shard manifest inventory has an extra package"));
    }
    for (path, package) in expected.iter().zip(PREPARED_PACKAGES) {
        let source =
            crate::private_fs::read_regular_stable(path, "WM shard manifest").map_err(error)?;
        let expected_name = format!("name = \"{package}\"");
        if std::str::from_utf8(&source.contents)
            .map_err(|source| error(format!("WM shard manifest is not UTF-8: {source}")))?
            .lines()
            .filter(|line| line.trim() == expected_name)
            .count()
            != 1
        {
            return Err(error(
                "WM shard manifest path/package mapping is noncanonical",
            ));
        }
    }
    Ok(expected)
}

pub(super) fn source_tree_sha256(
    root: &Path,
    domain: &[u8],
    labels: &[&str],
) -> Result<String, GeneratedRunnerBuildError> {
    let mut labels = labels.to_vec();
    labels.sort_unstable();
    let mut digest = Sha256::new();
    digest.update(domain);
    for label in labels {
        let path = root.join(label);
        let metadata = fs::symlink_metadata(&path).map_err(|source| {
            error(format!(
                "inspect generated-runner source {}: {source}",
                path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(error(format!(
                "generated-runner source must be a regular non-symlink file: {}",
                path.display()
            )));
        }
        let bytes = crate::private_fs::read_regular_stable(&path, "generated-runner source")
            .map_err(error)?
            .contents;
        push_bytes(&mut digest, label.as_bytes());
        push_bytes(&mut digest, &bytes);
    }
    Ok(hex(&digest.finalize()))
}

pub(super) fn collect_package_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), GeneratedRunnerBuildError> {
    for entry in fs::read_dir(directory).map_err(|source| {
        error(format!(
            "enumerate Cargo source {}: {source}",
            directory.display()
        ))
    })? {
        let path = entry
            .map_err(|source| {
                error(format!(
                    "enumerate Cargo source {}: {source}",
                    directory.display()
                ))
            })?
            .path();
        let relative = path
            .strip_prefix(root)
            .expect("package entry remains under root");
        if relative
            .components()
            .any(|component| matches!(component.as_os_str().to_str(), Some("target" | ".git")))
        {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).map_err(|source| {
            error(format!("inspect Cargo source {}: {source}", path.display()))
        })?;
        if metadata.file_type().is_symlink() {
            return Err(error(format!(
                "Cargo package source contains symlink {}",
                path.display()
            )));
        }
        if metadata.is_dir() {
            collect_package_files(root, &path, files)?;
        } else if metadata.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

pub(super) fn prepared_source_claims_v3(
    workspace: &Path,
) -> Result<PreparedSourceClaimsV3, GeneratedRunnerBuildError> {
    let shard_root = game_package_root()?.join(env!("FN64_WM_SHARD_DIR"));
    Ok(PreparedSourceClaimsV3 {
        generator_source_sha256: source_tree_sha256(
            &shard_root,
            b"fn64.wm-prepared-generator-source.v1\0",
            &[
                "build.rs",
                "materializer.rs",
                "prepared_build.rs",
                "prepared_tree.rs",
                "producer.rs",
            ],
        )?,
        discovery_source_sha256: package_source_tree_sha256(
            &workspace.join("crates/fn64-discover"),
            b"fn64.wm-prepared-discovery-source.v1\0",
        )?,
        emitter_source_sha256: emitter_source_sha256(workspace)?,
        runtime_source_sha256: hex(
            &fn64_recomp_rs::generated_runner_runtime_source_receipt_v1().source_sha256(),
        ),
        materializer_source_sha256: sha256_file(
            &shard_root.join("materializer.rs"),
            "WM prepared materializer source",
        )?,
    })
}

pub(super) fn emitter_source_sha256(workspace: &Path) -> Result<String, GeneratedRunnerBuildError> {
    let root = workspace.join("crates/fn64-recomp-rs-codegen");
    let mut digest = Sha256::new();
    digest.update(b"fn64:generated-runner-emitter-source:v2:");
    // This order is part of GeneratedRunnerEmitterSourceReceiptV2's wire.
    // The generic source-tree helper sorts labels and therefore cannot measure
    // this receipt independently without changing its digest.
    for label in ["Cargo.toml", "src/lib.rs", "src/emit.rs"] {
        push_bytes(&mut digest, label.as_bytes());
        let source = crate::private_fs::read_regular_stable(
            &root.join(label),
            "generated-runner emitter source",
        )
        .map_err(error)?;
        push_bytes(&mut digest, &source.contents);
    }
    Ok(hex(&digest.finalize()))
}

pub(super) fn package_source_tree_sha256(
    root: &Path,
    domain: &[u8],
) -> Result<String, GeneratedRunnerBuildError> {
    let mut files = Vec::new();
    collect_package_files(root, root, &mut files)?;
    files.sort();
    let mut digest = Sha256::new();
    digest.update(domain);
    for file in files {
        let relative = file
            .strip_prefix(root)
            .expect("collected package source remains below root");
        push_bytes(
            &mut digest,
            relative.to_string_lossy().replace('\\', "/").as_bytes(),
        );
        let source =
            crate::private_fs::read_regular_stable(&file, "package source").map_err(error)?;
        push_bytes(&mut digest, &source.contents);
    }
    Ok(hex(&digest.finalize()))
}

pub(super) fn producer_cargo_source_sha256_v3(
    metadata_source_sha256: &str,
    workspace: &Path,
) -> Result<String, GeneratedRunnerBuildError> {
    let external_sources = source_tree_sha256(
        &game_package_root()?.join(env!("FN64_WM_SHARD_DIR")),
        b"fn64.wm-prepared-producer-external-sources.v1\0",
        &["build.rs", "prepared_tree.rs", "producer.rs"],
    )?;
    let mut digest = Sha256::new();
    digest.update(b"fn64.wm-prepared-producer-cargo-source-graph.v1\0");
    digest.update(decode_sha256(metadata_source_sha256)?);
    digest.update(decode_sha256(&external_sources)?);
    Ok(hex(&digest.finalize()))
}

pub(super) fn normalized_rom_sha256(path: &Path) -> Result<String, GeneratedRunnerBuildError> {
    let source = crate::private_fs::read_regular_stable(path, "staged WM ROM").map_err(error)?;
    let bytes = normalize_n64_rom_bytes(&source.contents)?;
    Ok(hex(&Sha256::digest(bytes)))
}

pub(super) fn normalize_n64_rom_bytes(source: &[u8]) -> Result<Vec<u8>, GeneratedRunnerBuildError> {
    if source.len() < 0x40 || source.len() % 4 != 0 {
        return Err(error("staged WM ROM is too small or not word aligned"));
    }
    let magic = u32::from_be_bytes(source[..4].try_into().expect("ROM header is four bytes"));
    match magic {
        0x8037_1240 => Ok(source.to_vec()),
        0x4012_3780 => Ok(source
            .chunks_exact(4)
            .flat_map(|word| [word[3], word[2], word[1], word[0]])
            .collect()),
        0x3780_4012 => Ok(source
            .chunks_exact(2)
            .flat_map(|pair| [pair[1], pair[0]])
            .collect()),
        _ => Err(error("staged WM ROM has an unknown byte-order magic")),
    }
}

pub(super) fn measure_prepared_tree_v3(
    root: &Path,
    expected_rom: &str,
    expected_claims: &PreparedSourceClaimsV3,
) -> Result<PreparedTreeMeasurementV3, GeneratedRunnerBuildError> {
    validate_input_path(root, "prepared shard root")?;
    require_private_entry(root, true, "prepared shard root")?;
    let expected_root = BTreeSet::from_iter(
        std::iter::once(PREPARED_MANIFEST_NAME.to_owned()).chain(
            PREPARED_PACKAGES
                .iter()
                .map(|package| (*package).to_owned()),
        ),
    );
    let root_entries = exact_directory_entries(root, "prepared shard root")?;
    if root_entries != expected_root || root_entries.contains(PREPARED_UPDATE_MARKER_NAME) {
        return Err(error(format!(
            "prepared shard root does not contain exactly manifest.v2 and {SHARD_COUNT} package directories",
        )));
    }

    let manifest_path = root.join(PREPARED_MANIFEST_NAME);
    require_private_entry(&manifest_path, false, "prepared root manifest")?;
    let manifest = crate::private_fs::read_regular_stable(&manifest_path, "prepared root manifest")
        .map_err(error)?;
    let manifest_text = std::str::from_utf8(&manifest.contents)
        .map_err(|source| error(format!("prepared root manifest is not UTF-8: {source}")))?;
    if !manifest_text.ends_with('\n') || manifest_text.contains("\r") {
        return Err(error("prepared root manifest is not canonical LF text"));
    }
    let lines = manifest_text.lines().collect::<Vec<_>>();
    if lines.len() != 7 + PREPARED_PACKAGES.len()
        || lines[0] != "schema fn64.wm-prepared-shard-tree.v2"
        || lines[6] != format!("artifact_count {SHARD_COUNT}")
    {
        return Err(error("prepared root manifest has a noncanonical shape"));
    }
    let normalized_rom_sha256 = parse_manifest_digest(lines[1], "normalized_rom_sha256")?;
    let claims = PreparedSourceClaimsV3 {
        generator_source_sha256: parse_manifest_digest(lines[2], "generator_source_sha256")?,
        discovery_source_sha256: parse_manifest_digest(lines[3], "discovery_source_sha256")?,
        emitter_source_sha256: parse_manifest_digest(lines[4], "emitter_source_sha256")?,
        runtime_source_sha256: parse_manifest_digest(lines[5], "runtime_source_sha256")?,
        materializer_source_sha256: expected_claims.materializer_source_sha256.clone(),
    };
    if normalized_rom_sha256 != expected_rom || &claims != expected_claims {
        return Err(error(
            "prepared root ROM or source claims differ from verifier measurements",
        ));
    }

    let mut tree = Sha256::new();
    tree.update(b"fn64.wm-prepared-shard-complete-tree.v1\0");
    let mut descriptors = Sha256::new();
    descriptors.update(b"fn64.wm-prepared-shard-descriptors.v1\0");
    hash_directory_descriptor(&mut descriptors, root, ".")?;
    hash_stable_measurement(
        &mut tree,
        &mut descriptors,
        PREPARED_MANIFEST_NAME,
        &manifest.measurement,
    )?;
    for (index, package) in PREPARED_PACKAGES.iter().enumerate() {
        let package_root = root.join(package);
        require_private_entry(&package_root, true, "prepared package directory")?;
        hash_directory_descriptor(&mut descriptors, &package_root, package)?;
        if exact_directory_entries(&package_root, "prepared package directory")?
            != BTreeSet::from([
                "identity.v1".to_owned(),
                "metadata.rs".to_owned(),
                "runner.rs".to_owned(),
            ])
        {
            return Err(error("prepared package has noncanonical topology"));
        }
        let mut measured = Vec::new();
        for name in ["identity.v1", "runner.rs", "metadata.rs"] {
            let path = package_root.join(name);
            require_private_entry(&path, false, "prepared package artifact")?;
            let measurement =
                crate::private_fs::read_regular_stable(&path, "prepared package artifact")
                    .map_err(error)?;
            let label = format!("{package}/{name}");
            hash_stable_measurement(
                &mut tree,
                &mut descriptors,
                &label,
                &measurement.measurement,
            )?;
            measured.push((name, measurement));
        }
        let identity = &measured[0].1;
        let runner = &measured[1].1;
        let metadata = &measured[2].1;
        validate_prepared_sidecar(
            &identity.contents,
            package,
            &runner.measurement.sha256,
            &metadata.measurement.sha256,
        )?;
        let expected_line = format!(
            "artifact {package} {} {} {}",
            identity.measurement.sha256, runner.measurement.sha256, metadata.measurement.sha256,
        );
        if lines[7 + index] != expected_line {
            return Err(error(
                "prepared root manifest artifact line differs from measured package",
            ));
        }
    }
    Ok(PreparedTreeMeasurementV3 {
        root: root.to_path_buf(),
        normalized_rom_sha256,
        manifest_sha256: manifest.measurement.sha256,
        tree_sha256: hex(&tree.finalize()),
        descriptor_sha256: hex(&descriptors.finalize()),
        claims,
    })
}

pub(super) fn parse_manifest_digest(line: &str, field: &str) -> Result<String, GeneratedRunnerBuildError> {
    let value = line
        .strip_prefix(field)
        .and_then(|rest| rest.strip_prefix(' '))
        .ok_or_else(|| error(format!("prepared manifest is missing canonical {field}")))?;
    require_sha256(value, field)?;
    if value == "0".repeat(64) {
        return Err(error(format!("prepared manifest {field} is zero")));
    }
    Ok(value.to_owned())
}

pub(super) fn validate_prepared_sidecar(
    bytes: &[u8],
    package: &str,
    runner_sha256: &str,
    metadata_sha256: &str,
) -> Result<(), GeneratedRunnerBuildError> {
    let expected = format!(
        "schema fn64.wm-prepared-shard-artifact.v1\npackage {package}\nrunner_sha256 {runner_sha256}\nmetadata_sha256 {metadata_sha256}\n"
    );
    if bytes != expected.as_bytes() {
        return Err(error("prepared package identity sidecar is noncanonical"));
    }
    Ok(())
}

pub(super) fn exact_directory_entries(
    path: &Path,
    label: &str,
) -> Result<BTreeSet<String>, GeneratedRunnerBuildError> {
    fs::read_dir(path)
        .map_err(|source| error(format!("enumerate {label}: {source}")))?
        .map(|entry| {
            entry
                .map_err(|source| error(format!("enumerate {label}: {source}")))?
                .file_name()
                .into_string()
                .map_err(|_| error(format!("{label} contains a non-UTF-8 name")))
        })
        .collect()
}

pub(super) fn require_private_entry(
    path: &Path,
    directory: bool,
    label: &str,
) -> Result<(), GeneratedRunnerBuildError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|source| error(format!("inspect {label}: {source}")))?;
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(error(format!("{label} has the wrong filesystem type")));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let expected = if directory { 0o700 } else { 0o600 };
        if metadata.permissions().mode() & 0o777 != expected {
            return Err(error(format!("{label} must have mode {expected:o}")));
        }
    }
    Ok(())
}

pub(super) fn hash_stable_measurement(
    tree: &mut Sha256,
    descriptors: &mut Sha256,
    label: &str,
    measurement: &crate::private_fs::StableFileMeasurement,
) -> Result<(), GeneratedRunnerBuildError> {
    push_bytes(tree, label.as_bytes());
    tree.update(measurement.bytes.to_be_bytes());
    tree.update(decode_sha256(&measurement.sha256)?);
    push_bytes(descriptors, label.as_bytes());
    descriptors.update(measurement.bytes.to_be_bytes());
    descriptors.update(measurement.unix_mode.unwrap_or(0).to_be_bytes());
    match &measurement.object_id {
        #[cfg(unix)]
        crate::private_fs::StableObjectId::Unix { device, inode } => {
            descriptors.update([1]);
            descriptors.update(device.to_be_bytes());
            descriptors.update(inode.to_be_bytes());
        }
        #[cfg(windows)]
        crate::private_fs::StableObjectId::Windows {
            volume_serial_number,
            file_id,
        } => {
            descriptors.update([2]);
            descriptors.update(volume_serial_number.to_be_bytes());
            descriptors.update(file_id);
        }
    }
    Ok(())
}

pub(super) fn hash_directory_descriptor(
    descriptors: &mut Sha256,
    path: &Path,
    label: &str,
) -> Result<(), GeneratedRunnerBuildError> {
    let before =
        crate::private_fs::check_directory_nofollow(path, "prepared directory").map_err(error)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| error(format!("inspect prepared directory: {source}")))?;
    let after =
        crate::private_fs::check_directory_nofollow(path, "prepared directory").map_err(error)?;
    if before.object_id != after.object_id {
        return Err(error(
            "prepared directory changed during descriptor measurement",
        ));
    }
    push_bytes(descriptors, label.as_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        descriptors.update(metadata.permissions().mode().to_be_bytes());
    }
    #[cfg(windows)]
    descriptors.update(0u32.to_be_bytes());
    match before.object_id {
        #[cfg(unix)]
        crate::private_fs::StableObjectId::Unix { device, inode } => {
            descriptors.update([3]);
            descriptors.update(device.to_be_bytes());
            descriptors.update(inode.to_be_bytes());
        }
        #[cfg(windows)]
        crate::private_fs::StableObjectId::Windows {
            volume_serial_number,
            file_id,
        } => {
            descriptors.update([4]);
            descriptors.update(volume_serial_number.to_be_bytes());
            descriptors.update(file_id);
        }
    }
    Ok(())
}

pub(super) fn build_prepared_producer_v3(
    memory_guard: &Path,
    cargo: &Path,
    environment: &BuildEnvironmentV3,
    workspace: &Path,
    scratch: &Path,
    max_build_seconds: u64,
) -> Result<ProducerBuildMeasurementV3, GeneratedRunnerBuildError> {
    let package_root = game_package_root()?.join("wm2000-prepared-shard-producer");
    let manifest = package_root.join("Cargo.toml");
    let lock = package_root.join("Cargo.lock");
    let manifest_sha256 = sha256_file(&manifest, "prepared producer manifest")?;
    let lock_sha256 = sha256_file(&lock, "prepared producer lockfile")?;
    let metadata = run_cargo_metadata(cargo, environment, &manifest, scratch)?;
    let cargo_graph_sha256 = hex(&Sha256::digest(&metadata));
    let cargo_source_sha256 =
        producer_cargo_source_sha256_v3(&cargo_metadata_source_sha256(&metadata)?, workspace)?;
    let stdout_path = scratch.join("producer-build.stdout.jsonl");
    let stderr_path = scratch.join("producer-build.stderr.log");
    let mut command = Command::new(memory_guard);
    environment.apply(&mut command);
    command
        .arg(cargo)
        .arg("build")
        .arg("-j1")
        .arg("--frozen")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--target-dir")
        .arg(scratch.join("producer-target"))
        .arg("--package")
        .arg(PRODUCER_PACKAGE)
        .arg("--bin")
        .arg(PRODUCER_PACKAGE)
        .arg("--message-format=json-render-diagnostics")
        .current_dir(scratch)
        .env("CARGO_BUILD_JOBS", "1")
        .env("FN64_GUARD_MAX_RSS_MIB", BUILD_MAX_RSS_MIB.to_string())
        .env(
            "FN64_GUARD_MIN_FREE_PERCENT",
            BUILD_MIN_FREE_PERCENT.to_string(),
        )
        .env("FN64_GUARD_MAX_SECONDS", max_build_seconds.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(create_new(&stdout_path)?))
        .stderr(Stdio::from(create_new(&stderr_path)?));
    let status = command
        .status()
        .map_err(|source| error(format!("run frozen prepared producer build: {source}")))?;
    if !status.success() {
        return Err(error(format!(
            "prepared producer build exited {status}; stderr: {}",
            bounded_diagnostic_file(&stderr_path),
        )));
    }
    let selected = select_named_compiler_artifact(
        &fs::read(&stdout_path)
            .map_err(|source| error(format!("read producer artifact stream: {source}")))?,
        PRODUCER_PACKAGE,
    )?;
    let binary_sha256 = sha256_file(&selected, "selected prepared producer")?;
    let binary = stage_executable(
        &selected,
        &scratch.join("selected-prepared-producer"),
        &binary_sha256,
        "prepared producer",
    )?;
    let metadata_after = run_cargo_metadata(cargo, environment, &manifest, scratch)?;
    if sha256_file(&manifest, "prepared producer manifest after build")? != manifest_sha256
        || sha256_file(&lock, "prepared producer lockfile after build")? != lock_sha256
        || hex(&Sha256::digest(&metadata_after)) != cargo_graph_sha256
        || producer_cargo_source_sha256_v3(
            &cargo_metadata_source_sha256(&metadata_after)?,
            workspace,
        )? != cargo_source_sha256
    {
        return Err(error(
            "prepared producer manifest, lock, or frozen source graph changed during build",
        ));
    }
    Ok(ProducerBuildMeasurementV3 {
        manifest_sha256,
        lock_sha256,
        cargo_graph_sha256,
        cargo_source_sha256,
        binary_sha256,
        binary,
    })
}

pub(super) fn invoke_prepared_producer_v3(
    memory_guard: &Path,
    producer: &ProducerBuildMeasurementV3,
    environment: &BuildEnvironmentV3,
    rom: &Path,
    claims: &PreparedSourceClaimsV3,
    expected_rom: &str,
    scratch: &Path,
    max_build_seconds: u64,
) -> Result<PreparedTreeMeasurementV3, GeneratedRunnerBuildError> {
    let root = scratch.join("prepared-shards");
    let stdout_path = scratch.join("producer.stdout.log");
    let stderr_path = scratch.join("producer.stderr.log");
    let mut command = Command::new(memory_guard);
    environment.apply(&mut command);
    command
        .arg(&producer.binary)
        .arg("--rom")
        .arg(rom)
        .arg("--output")
        .arg(&root)
        .arg("--generator-source-sha256")
        .arg(&claims.generator_source_sha256)
        .arg("--discovery-source-sha256")
        .arg(&claims.discovery_source_sha256)
        .arg("--emitter-source-sha256")
        .arg(&claims.emitter_source_sha256)
        .arg("--runtime-source-sha256")
        .arg(&claims.runtime_source_sha256)
        .env("FN64_GUARD_MAX_RSS_MIB", BUILD_MAX_RSS_MIB.to_string())
        .env(
            "FN64_GUARD_MIN_FREE_PERCENT",
            BUILD_MIN_FREE_PERCENT.to_string(),
        )
        .env("FN64_GUARD_MAX_SECONDS", max_build_seconds.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(create_new(&stdout_path)?))
        .stderr(Stdio::from(create_new(&stderr_path)?));
    let status = command
        .status()
        .map_err(|source| error(format!("run prepared producer: {source}")))?;
    if !status.success() {
        return Err(error(format!(
            "prepared producer exited {status}; stderr: {}",
            bounded_diagnostic_file(&stderr_path),
        )));
    }
    let measurement = measure_prepared_tree_v3(&root, expected_rom, claims)?;
    let expected_stdout = format!(
        "schema=fn64.wm-prepared-shard-tree.v2 normalized_rom_sha256={} prepared_manifest_sha256={}\n",
        measurement.normalized_rom_sha256, measurement.manifest_sha256
    );
    if fs::read(&stdout_path).map_err(|source| error(format!("read producer stdout: {source}")))?
        != expected_stdout.as_bytes()
    {
        return Err(error("prepared producer stdout is not canonical"));
    }
    Ok(measurement)
}

pub(super) fn revalidate_prepared_producer_v3(
    expected: &ProducerBuildMeasurementV3,
    cargo: &Path,
    environment: &BuildEnvironmentV3,
    workspace: &Path,
    scratch: &Path,
) -> Result<(), GeneratedRunnerBuildError> {
    let package_root = game_package_root()?.join("wm2000-prepared-shard-producer");
    let manifest = package_root.join("Cargo.toml");
    let lock = package_root.join("Cargo.lock");
    let metadata = run_cargo_metadata(cargo, environment, &manifest, scratch)?;
    let metadata_source = cargo_metadata_source_sha256(&metadata)?;
    if sha256_file(&manifest, "prepared producer manifest revalidation")?
        != expected.manifest_sha256
        || sha256_file(&lock, "prepared producer lockfile revalidation")? != expected.lock_sha256
        || hex(&Sha256::digest(&metadata)) != expected.cargo_graph_sha256
        || producer_cargo_source_sha256_v3(&metadata_source, workspace)?
            != expected.cargo_source_sha256
        || sha256_file(&expected.binary, "staged prepared producer revalidation")?
            != expected.binary_sha256
    {
        return Err(error(
            "prepared producer authority changed after publication",
        ));
    }
    Ok(())
}

pub(super) fn build_selected_binary(
    memory_guard: &Path,
    cargo: &Path,
    manifest: &Path,
    inputs: &GeneratedRunnerBuildInputsV1,
    prepared: &PreparedTreeMeasurementV3,
    producer: &ProducerBuildMeasurementV3,
    prepared_source_mode: &str,
    environment: &BuildEnvironmentV3,
    scratch: &Path,
) -> Result<PathBuf, GeneratedRunnerBuildError> {
    let stdout_path = scratch.join("cargo-build.stdout.jsonl");
    let stderr_path = scratch.join("cargo-build.stderr.log");
    let mut command = guarded_build_command(
        memory_guard,
        cargo,
        manifest,
        inputs,
        prepared,
        producer,
        prepared_source_mode,
        environment,
        scratch,
    )?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(create_new(&stdout_path)?))
        .stderr(Stdio::from(create_new(&stderr_path)?));
    let mut child = command
        .spawn()
        .map_err(|source| error(format!("spawn frozen generated-runner build: {source}")))?;
    // The exact repository guard establishes a new session/process group
    // before Cargo begins. Its memory and wall-time failures terminate that
    // whole group, including rustc descendants orphaned by Cargo.
    let status = child
        .wait()
        .map_err(|source| error(format!("wait for guarded generated-runner build: {source}")))?;
    if !status.success() {
        let progress = fs::read(&stdout_path)
            .map(|bytes| cargo_build_progress(&bytes))
            .unwrap_or_else(|source| format!("compiler_artifacts=unreadable({source})"));
        return Err(error(format!(
            "generated-runner Cargo build exited {status}; {progress}; stderr: {}",
            bounded_diagnostic_file(&stderr_path),
        )));
    }
    select_compiler_artifact(
        &fs::read(&stdout_path)
            .map_err(|source| error(format!("read Cargo compiler-artifact stream: {source}")))?,
    )
}

pub(super) fn guarded_build_command(
    memory_guard: &Path,
    cargo: &Path,
    manifest: &Path,
    inputs: &GeneratedRunnerBuildInputsV1,
    prepared: &PreparedTreeMeasurementV3,
    producer: &ProducerBuildMeasurementV3,
    prepared_source_mode: &str,
    environment: &BuildEnvironmentV3,
    scratch: &Path,
) -> Result<Command, GeneratedRunnerBuildError> {
    validate_memory_guard(memory_guard)?;
    let mut command = Command::new(memory_guard);
    environment.apply(&mut command);
    command
        .arg(cargo)
        .arg("build")
        .arg(format!("-j{SELECTED_BUILD_CARGO_JOBS_V5}"))
        .arg("--frozen")
        .arg("--manifest-path")
        .arg(manifest)
        .arg("--target-dir")
        .arg(scratch.join("build-target"))
        .arg("--package")
        .arg(PACKAGE)
        .arg("--bin")
        .arg(PACKAGE)
        .arg("--message-format=json-render-diagnostics")
        .current_dir(scratch)
        .env("ROM", &inputs.rom)
        .env("FN64_BOOT_CONTEXT", &inputs.boot_context)
        .env(PREPARED_ROOT_ENV, &prepared.root)
        .env("FN64_WM_PREPARED_SOURCE_MODE", prepared_source_mode)
        .env(
            "FN64_WM_PREPARED_TREE_DESCRIPTOR_SHA256",
            &prepared.descriptor_sha256,
        )
        .env(
            "FN64_WM_PREPARED_MATERIALIZER_SOURCE_SHA256",
            &prepared.claims.materializer_source_sha256,
        )
        .env(
            "FN64_WM_PREPARED_PRODUCER_MANIFEST_SHA256",
            &producer.manifest_sha256,
        )
        .env(
            "FN64_WM_PREPARED_PRODUCER_LOCK_SHA256",
            &producer.lock_sha256,
        )
        .env(
            "FN64_WM_PREPARED_PRODUCER_CARGO_GRAPH_SHA256",
            &producer.cargo_graph_sha256,
        )
        .env(
            "FN64_WM_PREPARED_PRODUCER_CARGO_SOURCE_SHA256",
            &producer.cargo_source_sha256,
        )
        .env(
            "FN64_WM_PREPARED_PRODUCER_BINARY_SHA256",
            &producer.binary_sha256,
        )
        .env(
            "FN64_EXECUTABLE_IMAGE_GROUPS",
            inputs
                .executable_image_groups
                .iter()
                .map(|group| group.environment_name.as_str())
                .collect::<Vec<_>>()
                .join(","),
        )
        .env("CARGO_BUILD_JOBS", SELECTED_BUILD_CARGO_JOBS_V5.to_string())
        .env("FN64_GUARD_MAX_RSS_MIB", BUILD_MAX_RSS_MIB.to_string())
        .env(
            "FN64_GUARD_MIN_FREE_PERCENT",
            BUILD_MIN_FREE_PERCENT.to_string(),
        )
        .env(
            "FN64_GUARD_MAX_SECONDS",
            inputs.max_build_seconds.to_string(),
        );
    for group in &inputs.executable_image_groups {
        let joined = std::env::join_paths(&group.captures).map_err(|source| {
            error(format!(
                "join capture group {}: {source}",
                group.environment_name
            ))
        })?;
        command.env(&group.environment_name, joined);
    }
    Ok(command)
}

pub(super) fn validate_memory_guard(path: &Path) -> Result<String, GeneratedRunnerBuildError> {
    let bytes = fs::read(path)
        .map_err(|source| error(format!("read generated-runner memory guard: {source}")))?;
    validate_memory_guard_source(&bytes)?;
    if bytes != MEMORY_GUARD_SOURCE {
        return Err(error(
            "repository memory guard differs from the implementation compiled into the verifier",
        ));
    }
    Ok(hex(&Sha256::digest(bytes)))
}

pub(super) fn validate_memory_guard_source(bytes: &[u8]) -> Result<(), GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes)
        .map_err(|source| error(format!("memory guard source is not UTF-8: {source}")))?;
    for required in [
        "setsid",
        "collect_group",
        "terminate_group",
        "signal_group KILL",
        "FN64_GUARD_MAX_RSS_MIB",
        "FN64_GUARD_MIN_FREE_PERCENT",
        "FN64_GUARD_MAX_SECONDS",
    ] {
        if !source.contains(required) {
            return Err(error(format!(
                "memory guard source is missing required process-group policy {required}"
            )));
        }
    }
    Ok(())
}

pub(super) fn select_compiler_artifact(bytes: &[u8]) -> Result<PathBuf, GeneratedRunnerBuildError> {
    select_named_compiler_artifact(bytes, PACKAGE)
}

pub(super) fn select_named_compiler_artifact(
    bytes: &[u8],
    package: &str,
) -> Result<PathBuf, GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes).map_err(|source| {
        error(format!(
            "Cargo compiler-artifact stream is not UTF-8: {source}"
        ))
    })?;
    let mut selected = None;
    for line in source.lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let is_selected = message["reason"] == "compiler-artifact"
            && message["target"]["name"] == package
            && message["target"]["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"));
        if !is_selected {
            continue;
        }
        let executable = message["executable"]
            .as_str()
            .ok_or_else(|| error("selected Cargo artifact has no executable"))?;
        if selected.replace(PathBuf::from(executable)).is_some() {
            return Err(error(
                "Cargo emitted multiple selected generated-runner executables",
            ));
        }
    }
    let selected =
        selected.ok_or_else(|| error("Cargo emitted no selected generated-runner executable"))?;
    let canonical = selected.canonicalize().map_err(|source| {
        error(format!(
            "resolve selected generated runner {}: {source}",
            selected.display()
        ))
    })?;
    if !canonical.is_file() {
        return Err(error(
            "selected generated-runner executable is not a regular file",
        ));
    }
    Ok(canonical)
}

pub(super) fn launch_identity_child(
    child: &Path,
    scratch: &Path,
) -> Result<GeneratedRunnerBuildIdentityV1, GeneratedRunnerBuildError> {
    let stdout_path = scratch.join("identity.stdout.log");
    let stderr_path = scratch.join("identity.stderr.log");
    let mut command = Command::new(child);
    command
        .arg(GENERATED_RUNNER_BUILD_IDENTITY_ARGUMENT_V1)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::from(create_new(&stdout_path)?))
        .stderr(Stdio::from(create_new(&stderr_path)?));
    let mut process = command
        .spawn()
        .map_err(|source| error(format!("launch generated-runner identity child: {source}")))?;
    wait_with_watchdog(
        &mut process,
        IDENTITY_WATCHDOG,
        "generated-runner identity child",
    )?;
    let status = process
        .try_wait()
        .map_err(|source| error(format!("read identity child status: {source}")))?
        .expect("watchdog returned only after child exit");
    if !status.success() {
        return Err(error(format!(
            "generated-runner identity child exited {status}; stderr: {}",
            bounded_diagnostic_file(&stderr_path),
        )));
    }
    parse_identity_output(
        &fs::read(&stdout_path)
            .map_err(|source| error(format!("read identity child output: {source}")))?,
    )
}

pub(super) fn parse_identity_output(
    bytes: &[u8],
) -> Result<GeneratedRunnerBuildIdentityV1, GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes)
        .map_err(|source| error(format!("identity child output is not UTF-8: {source}")))?;
    let mut identity = None;
    for line in source.lines() {
        let Some(json) = line.strip_prefix(GENERATED_RUNNER_BUILD_IDENTITY_PREFIX_V1) else {
            continue;
        };
        let parsed = serde_json::from_str(json)
            .map_err(|source| error(format!("parse generated-runner child identity: {source}")))?;
        if identity.replace(parsed).is_some() {
            return Err(error(
                "generated-runner child emitted multiple identity envelopes",
            ));
        }
    }
    identity.ok_or_else(|| error("generated-runner child emitted no identity envelope"))
}
