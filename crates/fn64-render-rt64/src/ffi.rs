use std::ffi::{c_char, c_int, CStr};
use std::ptr::NonNull;

use fn64_render::OsTask;

const ERROR_CAPACITY: usize = 1024;

#[repr(C)]
struct RawContext {
    _private: [u8; 0],
}

#[repr(C)]
struct RawTask {
    task_type: u32,
    flags: u32,
    ucode_boot: u32,
    ucode_boot_size: u32,
    ucode: u32,
    ucode_size: u32,
    ucode_data: u32,
    ucode_data_size: u32,
    dram_stack: u32,
    dram_stack_size: u32,
    output_buff: u32,
    output_buff_size: u32,
    data_ptr: u32,
    data_size: u32,
}

const _: [(); 14 * std::mem::size_of::<u32>()] = [(); std::mem::size_of::<RawTask>()];

impl From<&OsTask> for RawTask {
    fn from(task: &OsTask) -> Self {
        Self {
            task_type: task.task_type,
            flags: task.flags,
            ucode_boot: task.ucode_boot,
            ucode_boot_size: task.ucode_boot_size,
            ucode: task.ucode,
            ucode_size: task.ucode_size,
            ucode_data: task.ucode_data,
            ucode_data_size: task.ucode_data_size,
            dram_stack: task.dram_stack,
            dram_stack_size: task.dram_stack_size,
            output_buff: task.output_buff,
            output_buff_size: task.output_buff_size,
            data_ptr: task.data_ptr,
            data_size: task.data_size,
        }
    }
}

unsafe extern "C" {
    fn fn64_rt64_create(
        width: u32,
        height: u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> *mut RawContext;
    fn fn64_rt64_process_task(
        context: *mut RawContext,
        rdram: *mut u8,
        rdram_len: usize,
        task: *const RawTask,
        output_addr: u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_present(
        context: *mut RawContext,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_resize(context: *mut RawContext, width: u32, height: u32);
    fn fn64_rt64_destroy(context: *mut RawContext);
}

fn error_string(buffer: &[c_char; ERROR_CAPACITY], fallback: &str) -> String {
    // SAFETY: every C ABI operation receives the full buffer capacity and
    // the shim always writes a trailing NUL when it reports an error. The
    // zero-initialized Rust buffer also guarantees a NUL if no text arrived.
    let message = unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    if message.is_empty() {
        fallback.to_string()
    } else {
        message
    }
}

pub(crate) struct Context(NonNull<RawContext>);

impl Context {
    pub(crate) fn create(width: u32, height: u32) -> Result<Self, String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: `error` is writable for the advertised capacity; the C++
        // shim returns either a uniquely-owned opaque context or null.
        let raw = unsafe { fn64_rt64_create(width, height, error.as_mut_ptr(), error.len()) };
        NonNull::new(raw)
            .map(Self)
            .ok_or_else(|| error_string(&error, "RT64 create failed without a diagnostic"))
    }

    pub(crate) fn process_task(
        &mut self,
        rdram: &mut [u8],
        task: &OsTask,
        output_addr: u32,
    ) -> Result<(), String> {
        let raw_task = RawTask::from(task);
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the opaque context is alive and uniquely borrowed; both
        // slice pointer/length and the repr(C) task remain valid for the
        // synchronous call. The shim waits for RT64's render-to-RAM worker
        // before returning, so no foreign thread retains the Rust borrow.
        let ok = unsafe {
            fn64_rt64_process_task(
                self.0.as_ptr(),
                rdram.as_mut_ptr(),
                rdram.len(),
                &raw_task,
                output_addr,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 task processing failed without a diagnostic",
            ))
        }
    }

    pub(crate) fn present(&mut self) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the opaque context is alive and uniquely borrowed for the
        // synchronous presentation call.
        let ok = unsafe { fn64_rt64_present(self.0.as_ptr(), error.as_mut_ptr(), error.len()) };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 present failed without a diagnostic",
            ))
        }
    }

    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        // SAFETY: the opaque context is alive and uniquely borrowed.
        unsafe { fn64_rt64_resize(self.0.as_ptr(), width, height) };
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: Context is the unique owner of the pointer returned by
        // fn64_rt64_create and calls destroy exactly once.
        unsafe { fn64_rt64_destroy(self.0.as_ptr()) };
    }
}
