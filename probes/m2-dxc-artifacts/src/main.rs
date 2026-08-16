use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

const VERSION_JSON: &str = concat!(
    r#"{"schema":"fn64.wgpu-shader-validator.v2","wgpu_major":30,"wgpu_version":"30.0.0","naga_version":"30.0.0","backend":"noop","validation":"wgpu-30-closed-profile-naga-validation-plus-checked-api","profiles":[{"name":"baseline","required_features":[],"required_limits":{"max_immediate_size":0}},{"name":"immediates-4","required_features":["IMMEDIATES"],"required_limits":{"max_immediate_size":4}},{"name":"immediates-8","required_features":["IMMEDIATES"],"required_limits":{"max_immediate_size":8}},{"name":"immediates-16","required_features":["IMMEDIATES"],"required_limits":{"max_immediate_size":16}},{"name":"immediates-20","required_features":["IMMEDIATES"],"required_limits":{"max_immediate_size":20}},{"name":"immediates-24","required_features":["IMMEDIATES"],"required_limits":{"max_immediate_size":24}},{"name":"immediates-32","required_features":["IMMEDIATES"],"required_limits":{"max_immediate_size":32}},{"name":"immediates-40","required_features":["IMMEDIATES"],"required_limits":{"max_immediate_size":40}},{"name":"immediates-56","required_features":["IMMEDIATES"],"required_limits":{"max_immediate_size":56}}]}"#,
    "\n"
);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ValidationProfile {
    Baseline,
    Immediates4,
    Immediates8,
    Immediates16,
    Immediates20,
    Immediates24,
    Immediates32,
    Immediates40,
    Immediates56,
}

impl ValidationProfile {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "baseline" => Ok(Self::Baseline),
            "immediates-4" => Ok(Self::Immediates4),
            "immediates-8" => Ok(Self::Immediates8),
            "immediates-16" => Ok(Self::Immediates16),
            "immediates-20" => Ok(Self::Immediates20),
            "immediates-24" => Ok(Self::Immediates24),
            "immediates-32" => Ok(Self::Immediates32),
            "immediates-40" => Ok(Self::Immediates40),
            "immediates-56" => Ok(Self::Immediates56),
            _ => Err(format!("unsupported validation profile {value:?}")),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Immediates4 => "immediates-4",
            Self::Immediates8 => "immediates-8",
            Self::Immediates16 => "immediates-16",
            Self::Immediates20 => "immediates-20",
            Self::Immediates24 => "immediates-24",
            Self::Immediates32 => "immediates-32",
            Self::Immediates40 => "immediates-40",
            Self::Immediates56 => "immediates-56",
        }
    }

    fn max_immediate_size(self) -> u32 {
        match self {
            Self::Baseline => 0,
            Self::Immediates4 => 4,
            Self::Immediates8 => 8,
            Self::Immediates16 => 16,
            Self::Immediates20 => 20,
            Self::Immediates24 => 24,
            Self::Immediates32 => 32,
            Self::Immediates40 => 40,
            Self::Immediates56 => 56,
        }
    }

    fn required_features(self) -> wgpu::Features {
        if self == Self::Baseline {
            wgpu::Features::empty()
        } else {
            wgpu::Features::IMMEDIATES
        }
    }

    fn for_immediate_size(size: u32) -> Result<Self, String> {
        match size {
            0 => Ok(Self::Baseline),
            4 => Ok(Self::Immediates4),
            8 => Ok(Self::Immediates8),
            16 => Ok(Self::Immediates16),
            20 => Ok(Self::Immediates20),
            24 => Ok(Self::Immediates24),
            32 => Ok(Self::Immediates32),
            40 => Ok(Self::Immediates40),
            56 => Ok(Self::Immediates56),
            _ => Err(format!("SPIR-V requires unreviewed immediate size {size}")),
        }
    }

    fn contract_json(self) -> String {
        let features = if self == Self::Baseline {
            "[]"
        } else {
            r#"["IMMEDIATES"]"#
        };
        format!(
            r#"{{"name":"{}","required_features":{},"required_limits":{{"max_immediate_size":{}}}}}"#,
            self.name(),
            features,
            self.max_immediate_size()
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Stage {
    Vertex,
    Fragment,
    Compute,
}

impl Stage {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "vertex" => Ok(Self::Vertex),
            "fragment" => Ok(Self::Fragment),
            "compute" => Ok(Self::Compute),
            _ => Err(format!("unsupported stage {value:?}")),
        }
    }

    fn execution_model(self) -> u32 {
        match self {
            Self::Vertex => 0,
            Self::Fragment => 4,
            Self::Compute => 5,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Vertex => "vertex",
            Self::Fragment => "fragment",
            Self::Compute => "compute",
        }
    }

    fn naga(self) -> naga::ShaderStage {
        match self {
            Self::Vertex => naga::ShaderStage::Vertex,
            Self::Fragment => naga::ShaderStage::Fragment,
            Self::Compute => naga::ShaderStage::Compute,
        }
    }
}

struct Request {
    profile: ValidationProfile,
    shader: PathBuf,
    stage: Stage,
    entry: String,
}

fn parse_request(mut arguments: impl Iterator<Item = String>) -> Result<Option<Request>, String> {
    let first = arguments
        .next()
        .ok_or_else(|| "missing command".to_owned())?;
    if first == "--fn64-version" {
        if arguments.next().is_some() {
            return Err("--fn64-version takes no other arguments".to_owned());
        }
        return Ok(None);
    }
    if first != "--profile" {
        return Err(format!("unexpected argument {first:?}"));
    }
    let profile = ValidationProfile::parse(
        &arguments
            .next()
            .ok_or_else(|| "--profile requires a value".to_owned())?,
    )?;
    if arguments.next().as_deref() != Some("--shader") {
        return Err("expected --shader after validation profile".to_owned());
    }
    let shader = PathBuf::from(
        arguments
            .next()
            .ok_or_else(|| "--shader requires a path".to_owned())?,
    );
    if arguments.next().as_deref() != Some("--stage") {
        return Err("expected --stage after shader path".to_owned());
    }
    let stage = Stage::parse(
        &arguments
            .next()
            .ok_or_else(|| "--stage requires a value".to_owned())?,
    )?;
    if arguments.next().as_deref() != Some("--entry") {
        return Err("expected --entry after stage".to_owned());
    }
    let entry = arguments
        .next()
        .ok_or_else(|| "--entry requires a value".to_owned())?;
    if entry.is_empty()
        || !entry
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
        || arguments.next().is_some()
    {
        return Err(
            "entry must be a non-empty ASCII identifier and terminate the command".to_owned(),
        );
    }
    Ok(Some(Request {
        profile,
        shader,
        stage,
        entry,
    }))
}

fn words(bytes: &[u8]) -> Result<Vec<u32>, String> {
    if bytes.len() < 20 || bytes.len() % 4 != 0 {
        return Err("SPIR-V must contain a complete five-word header and whole words".to_owned());
    }
    let result: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes(chunk.try_into().expect("four-byte chunk")))
        .collect();
    if result[0] != 0x0723_0203 {
        return Err("SPIR-V magic mismatch".to_owned());
    }
    Ok(result)
}

fn decode_name(operands: &[u32]) -> Result<String, String> {
    let mut bytes = Vec::new();
    for word in operands {
        let word_bytes = word.to_le_bytes();
        if let Some(terminator) = word_bytes.iter().position(|byte| *byte == 0) {
            if word_bytes[terminator + 1..].iter().any(|byte| *byte != 0) {
                return Err("SPIR-V entry point name padding is not zero".to_owned());
            }
            bytes.extend(&word_bytes[..terminator]);
            return String::from_utf8(bytes)
                .map_err(|_| "SPIR-V entry point name is not UTF-8".to_owned());
        }
        bytes.extend(word_bytes);
    }
    Err("SPIR-V entry point name is not terminated".to_owned())
}

fn has_entry(words: &[u32], stage: Stage, expected: &str) -> Result<bool, String> {
    let mut offset = 5;
    while offset < words.len() {
        let instruction = words[offset];
        let word_count = (instruction >> 16) as usize;
        let opcode = instruction & 0xffff;
        if word_count == 0 || offset + word_count > words.len() {
            return Err("SPIR-V instruction extent is invalid".to_owned());
        }
        if opcode == 15 {
            if word_count < 4 {
                return Err("SPIR-V OpEntryPoint is truncated".to_owned());
            }
            let name = decode_name(&words[offset + 3..offset + word_count])?;
            if words[offset + 1] == stage.execution_model() && name == expected {
                return Ok(true);
            }
        }
        offset += word_count;
    }
    Ok(false)
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::future::Future;
    use std::pin::pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    struct ThreadWake(std::thread::Thread);
    impl Wake for ThreadWake {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match Future::poll(future.as_mut(), &mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::park(),
        }
    }
}

fn validate(request: &Request) -> Result<usize, String> {
    let bytes =
        fs::read(&request.shader).map_err(|error| format!("cannot read shader: {error}"))?;
    let module_words = words(&bytes)?;
    if !has_entry(&module_words, request.stage, &request.entry)? {
        return Err("requested entry point and execution model are absent".to_owned());
    }

    let options = naga::front::spv::Options {
        adjust_coordinate_space: false,
        strict_capabilities: true,
        block_ctx_dump_prefix: None,
    };
    let module = naga::front::spv::Frontend::new(module_words.iter().copied(), &options)
        .parse()
        .map_err(|error| format!("wgpu 30 SPIR-V parse failed: {error}"))?;
    if !module
        .entry_points
        .iter()
        .any(|entry| entry.stage == request.stage.naga() && entry.name == request.entry)
    {
        return Err("wgpu 30 parser did not retain the requested entry point".to_owned());
    }
    let immediate_count = module
        .global_variables
        .iter()
        .filter(|(_, variable)| variable.space == naga::AddressSpace::Immediate)
        .count();
    if immediate_count > 1 {
        return Err("SPIR-V declares more than one immediate global".to_owned());
    }
    let required_profile = ValidationProfile::for_immediate_size(
        naga::valid::ImmediateSlots::size_for_module(&module),
    )?;
    if request.profile != required_profile {
        return Err(format!(
            "validation profile {:?} does not equal derived minimum {:?}",
            request.profile.name(),
            required_profile.name()
        ));
    }
    let mut limits = wgpu::Limits::default();
    limits.max_immediate_size = request.profile.max_immediate_size();
    for (_, variable) in module.global_variables.iter() {
        if let Some(binding) = variable.binding {
            if binding.group >= limits.max_bind_groups {
                return Err(format!(
                    "wgpu 30 bind group {} exceeds default limit {}",
                    binding.group, limits.max_bind_groups
                ));
            }
        }
    }
    let mut validator = wgpu_naga_bridge::create_validator(
        request.profile.required_features(),
        wgpu::DownlevelCapabilities::default().flags,
        naga::valid::ValidationFlags::all(),
    );
    validator
        .validate(&module)
        .map_err(|error| format!("wgpu 30 naga validation failed: {error}"))?;

    let device_descriptor = wgpu::DeviceDescriptor {
        required_features: request.profile.required_features(),
        required_limits: limits,
        ..Default::default()
    };
    let (device, _queue) = wgpu::Device::noop(&device_descriptor);
    if device.features() != request.profile.required_features()
        || device.limits().max_immediate_size != request.profile.max_immediate_size()
    {
        return Err("noop device did not retain the exact validation profile".to_owned());
    }
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let _module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("fn64-m2-dxc-artifact"),
        source: wgpu::ShaderSource::SpirV(Cow::Owned(module_words)),
    });
    if let Some(error) = block_on(scope.pop()) {
        return Err(format!("wgpu shader-module validation failed: {error}"));
    }
    Ok(bytes.len())
}

fn main() -> ExitCode {
    let request = match parse_request(std::env::args().skip(1)) {
        Ok(Some(request)) => request,
        Ok(None) => {
            print!("{VERSION_JSON}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("fn64-wgpu-shader-validator: {error}");
            return ExitCode::from(2);
        }
    };
    match validate(&request) {
        Ok(module_bytes) => {
            println!(
                r#"{{"schema":"fn64.wgpu-shader-validation.v2","status":"passed","wgpu_major":30,"profile":{},"stage":"{}","entry":"{}","module_bytes":{}}}"#,
                request.profile.contract_json(),
                request.stage.name(),
                request.entry,
                module_bytes
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("fn64-wgpu-shader-validator: {error}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Request, Stage, ValidationProfile, decode_name, has_entry, parse_request, validate, words,
    };

    fn request(profile: ValidationProfile, shader: std::path::PathBuf) -> Request {
        Request {
            profile,
            shader,
            stage: Stage::Compute,
            entry: "CSMain".to_owned(),
        }
    }

    fn push_constant_span(member_types: &[u32], offsets: &[u32]) -> u32 {
        assert_eq!(member_types.len(), offsets.len());
        fn instruction(words: &mut Vec<u32>, opcode: u32, operands: &[u32]) {
            words.push(((operands.len() as u32 + 1) << 16) | opcode);
            words.extend_from_slice(operands);
        }
        let mut module_words = vec![0x0723_0203, 0x0001_0000, 0, 14, 0];
        instruction(&mut module_words, 17, &[1]);
        instruction(&mut module_words, 14, &[0, 1]);
        instruction(
            &mut module_words,
            15,
            &[
                5,
                3,
                u32::from_le_bytes(*b"CSMa"),
                u32::from_le_bytes(*b"in\0\0"),
            ],
        );
        instruction(&mut module_words, 16, &[3, 17, 1, 1, 1]);
        instruction(&mut module_words, 71, &[11, 2]);
        for (index, offset) in offsets.iter().copied().enumerate() {
            instruction(&mut module_words, 72, &[11, index as u32, 35, offset]);
        }
        instruction(&mut module_words, 19, &[1]);
        instruction(&mut module_words, 33, &[2, 1]);
        instruction(&mut module_words, 22, &[5, 32]);
        instruction(&mut module_words, 21, &[6, 32, 0]);
        instruction(&mut module_words, 23, &[7, 5, 2]);
        instruction(&mut module_words, 23, &[8, 6, 2]);
        instruction(&mut module_words, 23, &[9, 5, 3]);
        instruction(&mut module_words, 23, &[10, 5, 4]);
        let mut struct_operands = vec![11];
        struct_operands.extend_from_slice(member_types);
        instruction(&mut module_words, 30, &struct_operands);
        instruction(&mut module_words, 32, &[12, 9, 11]);
        instruction(&mut module_words, 59, &[12, 13, 9]);
        instruction(&mut module_words, 54, &[1, 3, 0, 2]);
        instruction(&mut module_words, 248, &[4]);
        instruction(&mut module_words, 253, &[]);
        instruction(&mut module_words, 56, &[]);
        let options = naga::front::spv::Options {
            adjust_coordinate_space: false,
            strict_capabilities: true,
            block_ctx_dump_prefix: None,
        };
        let module = naga::front::spv::Frontend::new(module_words.into_iter(), &options)
            .parse()
            .expect("mixed-alignment SPIR-V fixture parses");
        naga::valid::ImmediateSlots::size_for_module(&module)
    }

    #[test]
    fn parses_only_closed_ordered_profile_commands() {
        for (token, expected) in [
            ("baseline", ValidationProfile::Baseline),
            ("immediates-4", ValidationProfile::Immediates4),
            ("immediates-8", ValidationProfile::Immediates8),
            ("immediates-16", ValidationProfile::Immediates16),
            ("immediates-20", ValidationProfile::Immediates20),
            ("immediates-24", ValidationProfile::Immediates24),
            ("immediates-32", ValidationProfile::Immediates32),
            ("immediates-40", ValidationProfile::Immediates40),
            ("immediates-56", ValidationProfile::Immediates56),
        ] {
            let parsed = parse_request(
                [
                    "--profile",
                    token,
                    "--shader",
                    "input.spv",
                    "--stage",
                    "compute",
                    "--entry",
                    "CSMain",
                ]
                .into_iter()
                .map(str::to_owned),
            )
            .expect("closed profile command")
            .expect("validation request");
            assert_eq!(parsed.profile, expected);
        }
        for arguments in [
            vec![
                "--shader",
                "input.spv",
                "--stage",
                "compute",
                "--entry",
                "CSMain",
            ],
            vec![
                "--profile",
                "immediates-12",
                "--shader",
                "input.spv",
                "--stage",
                "compute",
                "--entry",
                "CSMain",
            ],
            vec![
                "--profile",
                "baseline",
                "--feature",
                "IMMEDIATES",
                "--shader",
                "input.spv",
                "--stage",
                "compute",
                "--entry",
                "CSMain",
            ],
            vec![
                "--profile",
                "baseline",
                "--shader",
                "input.spv",
                "--entry",
                "CSMain",
                "--stage",
                "compute",
            ],
            vec![
                "--profile",
                "baseline",
                "--profile",
                "immediates-8",
                "--shader",
                "input.spv",
                "--stage",
                "compute",
                "--entry",
                "CSMain",
            ],
            vec![
                "--profile",
                "baseline",
                "--shader",
                "input.spv",
                "--stage",
                "compute",
                "--entry",
                "CSMain",
                "--limit",
                "8",
            ],
            vec!["--fn64-version", "--profile", "baseline"],
        ] {
            assert!(parse_request(arguments.into_iter().map(str::to_owned)).is_err());
        }
    }

    #[test]
    fn profile_contracts_are_exact() {
        assert_eq!(
            ValidationProfile::Baseline.required_features(),
            wgpu::Features::empty()
        );
        assert_eq!(ValidationProfile::Baseline.max_immediate_size(), 0);
        assert_eq!(
            ValidationProfile::Immediates56.required_features(),
            wgpu::Features::IMMEDIATES
        );
        assert_eq!(ValidationProfile::Immediates56.max_immediate_size(), 56);
        assert_eq!(
            ValidationProfile::Immediates8.contract_json(),
            r#"{"name":"immediates-8","required_features":["IMMEDIATES"],"required_limits":{"max_immediate_size":8}}"#
        );
        for (size, expected) in [
            (0, ValidationProfile::Baseline),
            (4, ValidationProfile::Immediates4),
            (8, ValidationProfile::Immediates8),
            (16, ValidationProfile::Immediates16),
            (20, ValidationProfile::Immediates20),
            (24, ValidationProfile::Immediates24),
            (32, ValidationProfile::Immediates32),
            (40, ValidationProfile::Immediates40),
            (56, ValidationProfile::Immediates56),
        ] {
            assert_eq!(ValidationProfile::for_immediate_size(size), Ok(expected));
        }
        assert!(ValidationProfile::for_immediate_size(12).is_err());
        assert!(ValidationProfile::for_immediate_size(36).is_err());
        assert!(ValidationProfile::for_immediate_size(52).is_err());
    }

    #[test]
    fn naga_mixed_alignment_spans_select_only_closed_profiles() {
        let video_interface = push_constant_span(&[7, 7, 5], &[0, 8, 16]);
        assert_eq!(video_interface, 24);
        assert_eq!(
            ValidationProfile::for_immediate_size(video_interface),
            Ok(ValidationProfile::Immediates24)
        );

        let fb_common =
            push_constant_span(&[8, 6, 6, 6, 6, 6, 6, 6], &[0, 8, 12, 16, 20, 24, 28, 32]);
        assert_eq!(fb_common, 40);
        assert_eq!(
            ValidationProfile::for_immediate_size(fb_common),
            Ok(ValidationProfile::Immediates40)
        );

        assert_eq!(push_constant_span(&[9, 5, 5], &[0, 12, 16]), 32);
        assert_eq!(push_constant_span(&[10, 5], &[0, 16]), 32);
        let unreviewed = push_constant_span(
            &[8, 6, 6, 6, 6, 6, 6, 6, 6, 6],
            &[0, 8, 12, 16, 20, 24, 28, 32, 36, 40],
        );
        assert_eq!(unreviewed, 48);
        assert!(ValidationProfile::for_immediate_size(unreviewed).is_err());
    }

    #[test]
    fn rejects_bad_magic() {
        assert!(words(&[0; 20]).is_err());
    }

    #[test]
    fn finds_exact_stage_and_entry() {
        let mut module = vec![0x0723_0203, 0x0001_0000, 0, 2, 0];
        module.extend([
            0x0005_000f,
            5,
            1,
            u32::from_le_bytes(*b"CSMa"),
            u32::from_le_bytes(*b"in\0\0"),
        ]);
        assert_eq!(has_entry(&module, Stage::Compute, "CSMain"), Ok(true));
        assert_eq!(has_entry(&module, Stage::Fragment, "CSMain"), Ok(false));
    }

    #[test]
    fn rejects_nonzero_bytes_after_name_terminator() {
        assert!(decode_name(&[u32::from_le_bytes(*b"A\0B\0")]).is_err());
    }

    #[test]
    fn wgpu_rejects_entry_only_fabrication() {
        let path = std::env::temp_dir().join(format!(
            "fn64-wgpu-validator-invalid-{}.spv",
            std::process::id()
        ));
        let mut words = vec![0x0723_0203, 0x0001_0000, 0, 2, 0];
        words.extend([
            0x0005_000f,
            5,
            1,
            u32::from_le_bytes(*b"CSMa"),
            u32::from_le_bytes(*b"in\0\0"),
        ]);
        let bytes: Vec<u8> = words.into_iter().flat_map(u32::to_le_bytes).collect();
        std::fs::write(&path, bytes).expect("write invalid SPIR-V fixture");
        let result = validate(&request(ValidationProfile::Baseline, path.clone()));
        std::fs::remove_file(path).expect("remove invalid SPIR-V fixture");
        assert!(result.is_err());
    }

    #[test]
    fn wgpu_accepts_minimal_valid_compute_module() {
        let path = std::env::temp_dir().join(format!(
            "fn64-wgpu-validator-valid-{}.spv",
            std::process::id()
        ));
        let words = [
            0x0723_0203,
            0x0001_0000,
            0,
            5,
            0,
            0x0002_0011,
            1,
            0x0003_000e,
            0,
            1,
            0x0005_000f,
            5,
            3,
            u32::from_le_bytes(*b"CSMa"),
            u32::from_le_bytes(*b"in\0\0"),
            0x0006_0010,
            3,
            17,
            1,
            1,
            1,
            0x0002_0013,
            1,
            0x0003_0021,
            2,
            1,
            0x0005_0036,
            1,
            3,
            0,
            2,
            0x0002_00f8,
            4,
            0x0001_00fd,
            0x0001_0038,
        ];
        let bytes: Vec<u8> = words.into_iter().flat_map(u32::to_le_bytes).collect();
        std::fs::write(&path, bytes).expect("write valid SPIR-V fixture");
        let result = validate(&request(ValidationProfile::Baseline, path.clone()));
        std::fs::remove_file(path).expect("remove valid SPIR-V fixture");
        assert!(result.is_ok(), "{result:?}");
    }
}
