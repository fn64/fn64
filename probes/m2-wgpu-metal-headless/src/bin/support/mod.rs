use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use serde::Serialize;
use sha2::{Digest, Sha256};

pub const EXIT_PASS: i32 = 0;
pub const EXIT_SEMANTIC_FAILURE: i32 = 2;
pub const EXIT_NO_ADAPTER: i32 = 69;
pub const EXIT_UNSUPPORTED: i32 = 78;

const POLL_TIMEOUT: Duration = Duration::from_secs(10);
const MAP_CALLBACK_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    SemanticOrValidationMismatch,
    NoMetalAdapter,
    ExplicitlyUnsupportedNativeSubtest,
}

impl Status {
    pub const fn exit_code(self) -> i32 {
        match self {
            Self::Pass => EXIT_PASS,
            Self::SemanticOrValidationMismatch => EXIT_SEMANTIC_FAILURE,
            Self::NoMetalAdapter => EXIT_NO_ADAPTER,
            Self::ExplicitlyUnsupportedNativeSubtest => EXIT_UNSUPPORTED,
        }
    }
}

pub struct ProbeOutput {
    pub json: String,
    pub exit_code: i32,
}

#[derive(Serialize)]
struct ReceiptBody<'a, T: Serialize> {
    schema: &'static str,
    probe: &'static str,
    command: [&'static str; 1],
    crate_identity: CrateIdentity,
    execution_identity: ExecutionIdentity,
    source_sha256: String,
    binary_sha256: Option<String>,
    status: Status,
    evidence: &'a T,
}

#[derive(Serialize)]
struct SignedReceipt<'a, T: Serialize> {
    #[serde(flatten)]
    body: ReceiptBody<'a, T>,
    canonical_sha256: String,
}

#[derive(Serialize)]
struct CrateIdentity {
    name: &'static str,
    version: &'static str,
    wgpu_version: &'static str,
    wgpu_backend: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct ExecutionIdentity {
    adapter: Option<AdapterIdentity>,
    target: &'static str,
    rustc_release: &'static str,
    rustc_commit_hash: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct AdapterIdentity {
    name: String,
    vendor: u32,
    device: u32,
    device_type: String,
    driver: String,
    driver_info: String,
    backend: String,
    subgroup_min_size: u32,
    subgroup_max_size: u32,
    device_pci_bus_id: String,
    transient_saves_memory: Option<bool>,
    limit_bucket: Option<String>,
}

pub fn finish<T: Serialize>(
    schema: &'static str,
    probe: &'static str,
    adapter: Option<&wgpu::Adapter>,
    status: Status,
    evidence: &T,
    source_parts: &[&str],
) -> ProbeOutput {
    let execution_identity = ExecutionIdentity {
        adapter: adapter.map(|adapter| adapter_identity(adapter.get_info())),
        target: env!("FN64_PROBE_TARGET"),
        rustc_release: env!("FN64_PROBE_RUSTC_RELEASE"),
        rustc_commit_hash: env!("FN64_PROBE_RUSTC_COMMIT_HASH"),
    };
    finish_with_identity(
        schema,
        probe,
        execution_identity,
        status,
        evidence,
        source_parts,
    )
}

fn finish_with_identity<T: Serialize>(
    schema: &'static str,
    probe: &'static str,
    execution_identity: ExecutionIdentity,
    status: Status,
    evidence: &T,
    source_parts: &[&str],
) -> ProbeOutput {
    let body = ReceiptBody {
        schema,
        probe,
        command: [probe],
        crate_identity: CrateIdentity {
            name: env!("CARGO_PKG_NAME"),
            version: env!("CARGO_PKG_VERSION"),
            wgpu_version: "30.0.0",
            wgpu_backend: "metal",
        },
        execution_identity,
        source_sha256: digest_chunks(source_parts.iter().map(|part| part.as_bytes())),
        binary_sha256: executable_digest(),
        status,
        evidence,
    };
    let canonical_value =
        serde_json::to_value(&body).expect("construct canonical probe receipt body");
    let canonical =
        serde_json::to_vec(&canonical_value).expect("serialize canonical probe receipt body");
    let receipt = SignedReceipt {
        body,
        canonical_sha256: digest_chunks([canonical.as_slice()]),
    };
    let canonical_receipt =
        serde_json::to_value(&receipt).expect("construct canonical signed probe receipt");
    ProbeOutput {
        json: serde_json::to_string(&canonical_receipt)
            .expect("serialize canonical signed probe receipt"),
        exit_code: status.exit_code(),
    }
}

fn adapter_identity(info: wgpu::AdapterInfo) -> AdapterIdentity {
    AdapterIdentity {
        name: info.name,
        vendor: info.vendor,
        device: info.device,
        device_type: format!("{:?}", info.device_type),
        driver: info.driver,
        driver_info: info.driver_info,
        backend: format!("{:?}", info.backend),
        subgroup_min_size: info.subgroup_min_size,
        subgroup_max_size: info.subgroup_max_size,
        device_pci_bus_id: info.device_pci_bus_id,
        transient_saves_memory: info.transient_saves_memory,
        limit_bucket: info.limit_bucket.map(|bucket| bucket.name.into_owned()),
    }
}

pub enum MetalAdapter {
    Available(wgpu::Instance, wgpu::Adapter),
    Unavailable,
}

pub fn request_metal_adapter() -> MetalAdapter {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::METAL,
        flags: wgpu::InstanceFlags::VALIDATION,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }));
    match adapter {
        Ok(adapter) if adapter.get_info().backend == wgpu::Backend::Metal => {
            MetalAdapter::Available(instance, adapter)
        }
        Ok(_) | Err(_) => MetalAdapter::Unavailable,
    }
}

pub struct DeviceContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    uncaptured: Arc<Mutex<Vec<String>>>,
}

impl DeviceContext {
    pub fn uncaptured_error_count(&self) -> usize {
        self.uncaptured.lock().unwrap().len()
    }
}

pub fn request_device(
    adapter: &wgpu::Adapter,
    required_features: wgpu::Features,
    required_limits: wgpu::Limits,
    label: &'static str,
) -> Result<DeviceContext, &'static str> {
    let request = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some(label),
        required_features,
        required_limits,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::MemoryUsage,
        trace: wgpu::Trace::Off,
    }));
    let (device, queue) = request.map_err(|_| "request_device")?;
    let uncaptured = Arc::new(Mutex::new(Vec::new()));
    let errors = Arc::clone(&uncaptured);
    device.on_uncaptured_error(Arc::new(move |error| {
        errors.lock().unwrap().push(error.to_string());
    }));
    Ok(DeviceContext {
        device,
        queue,
        uncaptured,
    })
}

pub fn submit_and_wait(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: wgpu::CommandEncoder,
) -> Result<(), &'static str> {
    let submission = queue.submit([encoder.finish()]);
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(POLL_TIMEOUT),
        })
        .map_err(|_| "submission_poll")?;
    Ok(())
}

pub fn map_buffer(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Result<Vec<u8>, &'static str> {
    let (sender, receiver) = mpsc::sync_channel(1);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(POLL_TIMEOUT),
        })
        .map_err(|_| "map_poll")?;
    match receiver.recv_timeout(MAP_CALLBACK_TIMEOUT) {
        Ok(Ok(())) => {}
        Ok(Err(_)) => return Err("map_result"),
        Err(_) => return Err("map_callback_timeout"),
    }
    let mapped = buffer
        .slice(..)
        .get_mapped_range()
        .map_err(|_| "mapped_range")?;
    let bytes = mapped.to_vec();
    drop(mapped);
    buffer.unmap();
    Ok(bytes)
}

pub fn pop_validation_scope(scope: wgpu::ErrorScopeGuard) -> bool {
    if let Some(error) = block_on(scope.pop()) {
        eprintln!("wgpu validation scope rejected probe operation: {error}");
        true
    } else {
        false
    }
}

pub fn digest_chunks<'a>(chunks: impl IntoIterator<Item = &'a [u8]>) -> String {
    let mut digest = Sha256::new();
    for chunk in chunks {
        digest.update((chunk.len() as u64).to_be_bytes());
        digest.update(chunk);
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn executable_digest() -> Option<String> {
    let executable = std::env::current_exe().ok()?;
    digest_file(&executable).ok()
}

fn digest_file(path: &Path) -> std::io::Result<String> {
    let bytes = fs::read(path)?;
    Ok(digest_chunks([bytes.as_slice()]))
}

pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::future::Future;
    use std::pin::pin;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Evidence {
        passed: bool,
    }

    #[test]
    fn status_exit_codes_are_stable() {
        assert_eq!(Status::Pass.exit_code(), 0);
        assert_eq!(Status::SemanticOrValidationMismatch.exit_code(), 2);
        assert_eq!(Status::NoMetalAdapter.exit_code(), 69);
        assert_eq!(Status::ExplicitlyUnsupportedNativeSubtest.exit_code(), 78);
    }

    #[test]
    fn receipt_is_compact_self_hashed_and_path_free() {
        let output = finish(
            "fn64.test.v1",
            "test_probe",
            None,
            Status::Pass,
            &Evidence { passed: true },
            &["source"],
        );
        assert_eq!(output.exit_code, 0);
        assert!(!output.json.contains('\n'));
        assert!(!output.json.contains("/Users/"));
        assert!(!output.json.contains("/private/"));
        let mut value: serde_json::Value = serde_json::from_str(&output.json).unwrap();
        assert_eq!(
            value["execution_identity"]["adapter"],
            serde_json::Value::Null
        );
        assert!(value["execution_identity"]["target"].as_str().is_some());
        assert!(
            value["execution_identity"]["rustc_release"]
                .as_str()
                .is_some()
        );
        assert!(
            value["execution_identity"]["rustc_commit_hash"]
                .as_str()
                .is_some()
        );
        let recorded = value
            .as_object_mut()
            .unwrap()
            .remove("canonical_sha256")
            .unwrap();
        assert_eq!(recorded.as_str().unwrap().len(), 64);
        assert_eq!(
            recorded.as_str().unwrap(),
            digest_chunks([serde_json::to_vec(&value).unwrap().as_slice()])
        );
    }

    #[test]
    fn canonical_hash_binds_adapter_device_driver_and_toolchain_identity() {
        let adapter = AdapterIdentity {
            name: "adapter-a".into(),
            vendor: 1,
            device: 2,
            device_type: "IntegratedGpu".into(),
            driver: "driver-a".into(),
            driver_info: "driver-info-a".into(),
            backend: "Metal".into(),
            subgroup_min_size: 4,
            subgroup_max_size: 64,
            device_pci_bus_id: String::new(),
            transient_saves_memory: Some(true),
            limit_bucket: None,
        };
        let identity = ExecutionIdentity {
            adapter: Some(adapter.clone()),
            target: "aarch64-apple-darwin",
            rustc_release: "1.test",
            rustc_commit_hash: "commit-a",
        };
        let first = finish_with_identity(
            "fn64.test.v1",
            "test_probe",
            identity.clone(),
            Status::Pass,
            &Evidence { passed: true },
            &["source"],
        );
        let first_value: serde_json::Value = serde_json::from_str(&first.json).unwrap();
        assert_eq!(
            first_value["execution_identity"]["adapter"]["name"],
            "adapter-a"
        );
        assert_eq!(first_value["execution_identity"]["adapter"]["device"], 2);
        assert_eq!(
            first_value["execution_identity"]["adapter"]["driver"],
            "driver-a"
        );
        assert_eq!(
            first_value["execution_identity"]["target"],
            "aarch64-apple-darwin"
        );
        assert_eq!(first_value["execution_identity"]["rustc_release"], "1.test");
        assert_eq!(
            first_value["execution_identity"]["rustc_commit_hash"],
            "commit-a"
        );

        let mut adapter_name = identity.clone();
        adapter_name.adapter.as_mut().unwrap().name = "adapter-b".into();
        let mut device = identity.clone();
        device.adapter.as_mut().unwrap().device = 3;
        let mut driver = identity.clone();
        driver.adapter.as_mut().unwrap().driver = "driver-b".into();
        let target = ExecutionIdentity {
            target: "x86_64-apple-darwin",
            ..identity.clone()
        };
        let release = ExecutionIdentity {
            rustc_release: "2.test",
            ..identity.clone()
        };
        let commit = ExecutionIdentity {
            rustc_commit_hash: "commit-b",
            ..identity
        };
        for mutated_identity in [adapter_name, device, driver, target, release, commit] {
            let mutated = finish_with_identity(
                "fn64.test.v1",
                "test_probe",
                mutated_identity,
                Status::Pass,
                &Evidence { passed: true },
                &["source"],
            );
            let mutated_value: serde_json::Value = serde_json::from_str(&mutated.json).unwrap();
            assert_ne!(
                first_value["canonical_sha256"],
                mutated_value["canonical_sha256"]
            );
        }
    }
}
