use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

const VERSION_JSON: &str = concat!(
    r#"{"schema":"fn64.wgpu-shader-validator.v1","wgpu_major":30,"wgpu_version":"30.0.0","naga_version":"30.0.0","backend":"noop","validation":"wgpu-30-baseline-naga-validation-plus-checked-api"}"#,
    "\n"
);

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
    if first != "--shader" {
        return Err(format!("unexpected argument {first:?}"));
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
    let limits = wgpu::Limits::default();
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
        wgpu::Features::empty(),
        wgpu::DownlevelCapabilities::default().flags,
        naga::valid::ValidationFlags::all(),
    );
    validator
        .validate(&module)
        .map_err(|error| format!("wgpu 30 naga validation failed: {error}"))?;

    let (device, _queue) = wgpu::Device::noop(&wgpu::DeviceDescriptor::default());
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
                r#"{{"schema":"fn64.wgpu-shader-validation.v1","status":"passed","wgpu_major":30,"stage":"{}","entry":"{}","module_bytes":{}}}"#,
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
    use super::{Request, Stage, decode_name, has_entry, validate, words};

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
        let result = validate(&Request {
            shader: path.clone(),
            stage: Stage::Compute,
            entry: "CSMain".to_owned(),
        });
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
        let result = validate(&Request {
            shader: path.clone(),
            stage: Stage::Compute,
            entry: "CSMain".to_owned(),
        });
        std::fs::remove_file(path).expect("remove valid SPIR-V fixture");
        assert!(result.is_ok(), "{result:?}");
    }
}
