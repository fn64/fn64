#[cfg(any(feature = "rt64", test))]
use fn64_render::{OsTask, RenderError, M_GFXTASK};

#[cfg(any(feature = "rt64", test))]
const PHYSICAL_ADDRESS_MASK: u32 = 0x00ff_ffff;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveSurfaceSize {
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[cfg(any(feature = "rt64", test))]
fn invalid(reason: impl Into<String>) -> RenderError {
    RenderError::Backend {
        backend: "rt64-native-ingress",
        reason: reason.into(),
    }
}

#[cfg(any(feature = "rt64", test))]
fn physical_address(value: u32, field: &str) -> Result<u32, RenderError> {
    if value & !PHYSICAL_ADDRESS_MASK != 0 {
        return Err(invalid(format!(
            "{field} {value:#010x} is not a physical RDRAM offset"
        )));
    }
    Ok(value)
}

#[cfg(any(feature = "rt64", test))]
fn bounded_range(
    offset: u32,
    len: usize,
    rdram_len: usize,
    field: &str,
) -> Result<(), RenderError> {
    let start = usize::try_from(offset).expect("u32 RDRAM offset fits usize");
    let Some(end) = start.checked_add(len) else {
        return Err(invalid(format!("{field} range overflows host usize")));
    };
    if end > rdram_len {
        return Err(RenderError::InvalidTaskBounds {
            offset,
            len: u32::try_from(len).unwrap_or(u32::MAX),
            rdram_len,
        });
    }
    Ok(())
}

#[cfg(any(feature = "rt64", test))]
pub(crate) fn validate_output_target(
    rdram_len: usize,
    output_addr: u32,
    surface: Option<ActiveSurfaceSize>,
) -> Result<(), RenderError> {
    if output_addr == 0 {
        return Ok(());
    }
    let output_addr = physical_address(output_addr, "output address")?;
    if !output_addr.is_multiple_of(8) {
        return Err(invalid(format!(
            "output address {output_addr:#010x} is not 64-bit aligned"
        )));
    }
    let surface = surface.ok_or(RenderError::NotReady(
        "RT64 output validation requires an active created surface",
    ))?;
    let pixels = usize::try_from(surface.width)
        .expect("u32 width fits usize")
        .checked_mul(usize::try_from(surface.height).expect("u32 height fits usize"))
        .ok_or_else(|| invalid("active RT64 surface pixel count overflows host usize"))?;
    let bytes = pixels
        .checked_mul(2)
        .ok_or_else(|| invalid("active RT64 RGBA16 output size overflows host usize"))?;
    if bytes == 0 {
        return Err(invalid("active RT64 surface has an empty RGBA16 output"));
    }
    bounded_range(output_addr, bytes, rdram_len, "RGBA16 output")
}

#[cfg(any(feature = "rt64", test))]
pub(crate) fn validate_task_ingress(
    rdram_len: usize,
    task: &OsTask,
    output_addr: u32,
    surface: Option<ActiveSurfaceSize>,
) -> Result<(), RenderError> {
    if task.task_type != M_GFXTASK {
        return Err(invalid(format!(
            "native graphics ingress received task type {}",
            task.task_type
        )));
    }
    let data_ptr = physical_address(task.data_ptr, "display-list address")?;
    if !data_ptr.is_multiple_of(8) {
        return Err(invalid(format!(
            "display-list address {data_ptr:#010x} is not 64-bit aligned"
        )));
    }
    let data_size = usize::try_from(task.data_size).expect("u32 task data size fits usize");
    if data_size == 0 || !data_size.is_multiple_of(8) {
        return Err(invalid(format!(
            "display-list size {data_size} must be a nonzero 64-bit multiple"
        )));
    }
    bounded_range(data_ptr, data_size, rdram_len, "display list")?;
    validate_output_target(rdram_len, output_addr, surface)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RDRAM_LEN: usize = fn64_runtime::rdram::DEFAULT_RDRAM_SIZE;
    const SURFACE: ActiveSurfaceSize = ActiveSurfaceSize {
        width: 320,
        height: 240,
    };

    fn task() -> OsTask {
        OsTask {
            task_type: M_GFXTASK,
            data_ptr: 0x1000,
            data_size: 8,
            ..OsTask::default()
        }
    }

    #[test]
    fn task_ingress_requires_a_complete_aligned_declared_display_list() {
        validate_task_ingress(RDRAM_LEN, &task(), 0, Some(SURFACE)).unwrap();
        for invalid_task in [
            OsTask {
                task_type: 0,
                ..task()
            },
            OsTask {
                data_ptr: 0x1001,
                ..task()
            },
            OsTask {
                data_ptr: 0x8000_1000,
                ..task()
            },
            OsTask {
                data_size: 0,
                ..task()
            },
            OsTask {
                data_size: 7,
                ..task()
            },
            OsTask {
                data_ptr: (RDRAM_LEN - 8) as u32,
                data_size: 16,
                ..task()
            },
        ] {
            assert!(validate_task_ingress(RDRAM_LEN, &invalid_task, 0, Some(SURFACE)).is_err());
        }
    }

    #[test]
    fn output_target_is_physical_aligned_and_covers_the_active_rgba16_surface() {
        validate_output_target(RDRAM_LEN, 0, None).unwrap();
        validate_output_target(RDRAM_LEN, 0x400000, Some(SURFACE)).unwrap();
        for output in [0x400001, 0x8040_0000, (RDRAM_LEN - 8) as u32] {
            assert!(validate_output_target(RDRAM_LEN, output, Some(SURFACE)).is_err());
        }
        assert!(validate_output_target(RDRAM_LEN, 0x400000, None).is_err());
        assert!(validate_output_target(
            RDRAM_LEN,
            0x400000,
            Some(ActiveSurfaceSize {
                width: 0,
                height: 240,
            }),
        )
        .is_err());
    }
}
