//! Flash-cartridge (`osPfsIsPlug`-adjacent) sector-erase/read-array/init
//! recognizers, and the programmed-IO discovery entry point.

use super::core::{
    collapse_overlapping_runs, imm, is_addiu, is_jr_ra, jal_field, op, rs, HostBinding,
    HostBindingSymbol,
};
use super::io::{epi_io_wrapper_targets, is_raw_epi_device_io};

/// Recognize the public `osFlashSectorErase(u32 page_num) -> s32`.
///
/// Public N64 FlashRAM Programming Manual: the sector is selected by writing
/// command `0x4B` carrying the page number, the erase is launched with command
/// `0x78`, and the caller then polls status until the busy bit clears. Both
/// commands are delivered through the title's own resolved `osEPiWriteIo` and
/// the poll reads back through its resolved `osEPiReadIo`.
///
/// Parameterising on the resolved wrapper addresses is what makes this
/// discriminating: the two command constants alone co-occur in fourteen corpus
/// titles, but only a routine that issues them through *this* title's
/// programmed-IO seam is that title's sector erase.
fn is_flash_sector_erase(words: &[u32], epi_write: u32, epi_read: u32) -> bool {
    if !(is_addiu(words[0], 29, 29, imm(words[0])) && imm(words[0]) < 0) {
        return false;
    }
    let Some(body) = flash_body_to_return(words) else {
        return false;
    };
    if !body.iter().any(|word| is_lui_immediate(*word, 0x4b00))
        || !body.iter().any(|word| is_lui_immediate(*word, 0x7800))
    {
        return false;
    }
    if flash_calls_to(body, epi_write).len() < 2 {
        return false;
    }
    let Some(read_at) = flash_calls_to(body, epi_read).first().copied() else {
        return false;
    };
    // The documented busy poll: a backward branch after reading status.
    body[read_at..]
        .iter()
        .any(|word| matches!(op(*word), 4 | 5) && imm(*word) < 0)
}

/// Recognize the public `osFlashReadArray(OSIoMesg *, s32 pri, u32 page,
/// void *dram, u32 nPages, OSMesgQueue *mq) -> s32`.
///
/// Public manual: the device is placed in read-array mode with command `0xF0`
/// before any transfer, and pages are moved in a loop. The fifth and sixth o32
/// arguments arrive on the caller's frame, above this routine's own.
fn is_flash_read_array(words: &[u32], epi_write: u32, epi_read: u32) -> bool {
    if !(is_addiu(words[0], 29, 29, imm(words[0])) && imm(words[0]) < 0) {
        return false;
    }
    let frame = -imm(words[0]);
    let Some(body) = flash_body_to_return(words) else {
        return false;
    };
    if !body.iter().any(|word| is_lui_immediate(*word, 0xf000)) {
        return false;
    }
    if flash_calls_to(body, epi_write).is_empty() || flash_calls_to(body, epi_read).is_empty() {
        return false;
    }
    // o32 arguments five and six live above this routine's own frame.
    let stack_arguments = body
        .iter()
        .filter(|word| op(**word) == 0x23 && rs(**word) == 29 && imm(**word) >= frame)
        .count();
    if stack_arguments < 2 {
        return false;
    }
    body.iter()
        .any(|word| matches!(op(*word), 4 | 5) && imm(*word) < 0)
}

/// Recognize the public `osFlashInit(void) -> OSPiHandle *`.
///
/// Public `<PR/os_flash.h>` fixes the handle this routine builds, and building
/// it is the routine's whole contract: device type 8, latency 5, pageSize
/// `0x0F`, relDuration 2, pulse `0x0C`, domain 1, and the uncached device base
/// `0xA800_0000`. It is idempotent, comparing the stored base against that
/// constant and returning the existing handle when already initialised.
///
/// This one is not parameterised on the programmed-IO seam, because a title may
/// call `osFlashInit` before anything reaches the device. It does not need to
/// be: the seven published constants together are specific enough that the
/// whole 287-ROM corpus yields exactly the titles that genuinely link the
/// routine, each verified by disassembly.
fn is_flash_init(words: &[u32]) -> bool {
    if !(is_addiu(words[0], 29, 29, imm(words[0])) && imm(words[0]) < 0) {
        return false;
    }
    let Some(body) = flash_body_to_return(words) else {
        return false;
    };
    // The published uncached FlashRAM device base.
    if !body.iter().any(|word| is_lui_immediate(*word, 0xa800)) {
        return false;
    }
    // The six published byte-width handle fields.
    let has_immediate = |value: u16| {
        body.iter()
            .any(|word| op(*word) == 9 && rs(*word) == 0 && (*word as u16) == value)
    };
    if ![8u16, 5, 0x0c, 0x0f, 2, 1].into_iter().all(has_immediate) {
        return false;
    }
    // Those fields are stored as bytes into the handle.
    if body.iter().filter(|word| op(**word) == 0x28).count() < 6 {
        return false;
    }
    // The idempotence guard.
    body.iter().any(|word| op(*word) == 4)
}

/// The instruction range from a routine's frame setup to its first `jr $ra`.
fn flash_body_to_return(words: &[u32]) -> Option<&[u32]> {
    (4..words.len())
        .find(|index| is_jr_ra(words[*index]))
        .map(|end| &words[..end])
}

/// Indices of direct calls to `target` within a routine body.
fn flash_calls_to(body: &[u32], target: u32) -> Vec<usize> {
    let field = (target & 0x0fff_ffff) >> 2;
    body.iter()
        .enumerate()
        .filter_map(|(index, word)| (jal_field(*word) == Some(field)).then_some(index))
        .collect()
}

fn is_lui_immediate(word: u32, immediate: u16) -> bool {
    op(word) == 0x0f && rs(word) == 0 && (word as u16) == immediate
}

/// Discover the FlashRAM API bindings, [`FLASH_HOST_SYMBOLS`].
///
/// Never fails, for the same reason as the programmed-IO roles: a title with no
/// FlashRAM links none of these, and that is the correct answer rather than an
/// error. The two command-issuing roles are resolved against the title's own
/// programmed-IO wrappers, so this takes them as inputs and returns nothing for
/// them when the wrappers are absent.
pub fn discover_flash_host_bindings(
    words: &[u32],
    va_start: u32,
    programmed_io: &[HostBinding],
) -> Vec<HostBinding> {
    const SECTOR_ERASE_WINDOW: usize = 88;
    const READ_ARRAY_WINDOW: usize = 110;
    const INIT_WINDOW: usize = 70;

    if !va_start.is_multiple_of(4) {
        return Vec::new();
    }
    let address_of = |symbol| {
        programmed_io
            .iter()
            .find(|binding| binding.symbol == symbol)
            .map(|binding| binding.vram)
    };

    let mut bindings = Vec::new();
    let mut install = |symbol, width: usize, predicate: &dyn Fn(&[u32]) -> bool| {
        if words.len() < width {
            return;
        }
        let mut candidates = words
            .windows(width)
            .enumerate()
            .filter_map(|(index, window)| {
                predicate(window)
                    .then(|| va_start.checked_add(u32::try_from(index).ok()?.checked_mul(4)?))?
            })
            .collect::<Vec<_>>();
        candidates.sort_unstable();
        candidates.dedup();
        if let [vram] = collapse_overlapping_runs(&candidates)[..] {
            bindings.push(HostBinding { symbol, vram });
        }
    };

    install(HostBindingSymbol::OsFlashInit, INIT_WINDOW, &is_flash_init);
    if let (Some(epi_write), Some(epi_read)) = (
        address_of(HostBindingSymbol::OsEPiWriteIo),
        address_of(HostBindingSymbol::OsEPiReadIo),
    ) {
        install(
            HostBindingSymbol::OsFlashSectorErase,
            SECTOR_ERASE_WINDOW,
            &|window: &[u32]| is_flash_sector_erase(window, epi_write, epi_read),
        );
        install(
            HostBindingSymbol::OsFlashReadArray,
            READ_ARRAY_WINDOW,
            &|window: &[u32]| is_flash_read_array(window, epi_write, epi_read),
        );
    }
    bindings.sort_by_key(|binding| binding.symbol);
    bindings
}

/// Discover the optional programmed-IO host bindings,
/// [`PROGRAMMED_IO_HOST_SYMBOLS`].
///
/// Unlike [`discover_wm_block_runtime_host_bindings`] this never fails: a title
/// that does not link these routines returns an empty map, which is the correct
/// answer for an SRAM title rather than an error. Titles that do link them get
/// both, or -- for a build that links only one half -- just that one.
///
/// `osEPiWriteIo` and `osEPiReadIo` share one shape and are separated by the
/// raw routine each calls: the write half's callee ends in a store through the
/// device pointer, the read half's in a load followed by a store to the
/// caller's out-parameter. Both callees are validated as genuine PI device IO
/// by [`is_raw_epi_device_io`] before either address is reported, so a
/// same-shaped routine that forwards to something else cannot be installed.
///
/// Ambiguity is dropped rather than guessed. If either role matches more than
/// one address the role is omitted, because binding the wrong address would
/// redirect a guest call into an unrelated host shim.
pub fn discover_programmed_io_host_bindings(words: &[u32], va_start: u32) -> Vec<HostBinding> {
    /// Widest window the wrapper needs: prologue, three calls with their
    /// argument routing, and the epilogue.
    const WINDOW: usize = 40;
    /// The raw callee's `lw handle+12` sits well past its PI-status poll.
    const CALLEE_WINDOW: usize = 96;

    if !va_start.is_multiple_of(4) || words.len() < WINDOW {
        return Vec::new();
    }

    let callee_body = |target: u32| -> Option<&[u32]> {
        // `jal_field` yields the raw 26-bit instruction field, which is a WORD
        // address. Scale it to bytes before forming the segment-relative vram.
        let vram = 0x8000_0000u32 | (target << 2);
        let index = usize::try_from(vram.checked_sub(va_start)?).ok()? / 4;
        (index < words.len()).then(|| &words[index..words.len().min(index + CALLEE_WINDOW)])
    };

    let mut writes = Vec::new();
    let mut reads = Vec::new();
    for (index, window) in words.windows(WINDOW).enumerate() {
        let Some((_, raw, _)) = epi_io_wrapper_targets(window) else {
            continue;
        };
        let Some(body) = callee_body(raw) else {
            continue;
        };
        if !is_raw_epi_device_io(body) {
            continue;
        }
        let Some(vram) = u32::try_from(index)
            .ok()
            .and_then(|index| index.checked_mul(4))
            .and_then(|offset| va_start.checked_add(offset))
        else {
            continue;
        };
        // The write half's raw routine stores the caller's data through the
        // device pointer; the read half's loads from it. That is the ABI
        // difference between the two, and the only thing distinguishing them.
        if raw_epi_device_io_is_write(body) {
            writes.push(vram);
        } else {
            reads.push(vram);
        }
    }

    let mut bindings = Vec::new();
    for (symbol, mut candidates) in [
        (HostBindingSymbol::OsEPiReadIo, reads),
        (HostBindingSymbol::OsEPiWriteIo, writes),
    ] {
        candidates.sort_unstable();
        candidates.dedup();
        if let [vram] = collapse_overlapping_runs(&candidates)[..] {
            bindings.push(HostBinding { symbol, vram });
        }
    }
    bindings.sort_by_key(|binding| binding.symbol);
    bindings
}

/// Whether a validated raw EPI device routine is the WRITE half.
///
/// The two halves differ only in the direction of the single device access the
/// public routine performs: the write half stores the caller's `data` through
/// the uncached device pointer, the read half loads from it. Everything before
/// that access -- the PI-status poll, the domain-register publication, the
/// handle decode -- is identical in both.
fn raw_epi_device_io_is_write(words: &[u32]) -> bool {
    words
        .iter()
        .enumerate()
        .find_map(|(index, word)| {
            (op(*word) == 0x23 && imm(*word) == 12 && rs(*word) == 4).then_some(index)
        })
        .and_then(|index| {
            words[index + 1..words.len().min(index + 6)]
                .iter()
                .find(|word| op(**word) == 0x23 || op(**word) == 0x2b)
                .map(|word| op(*word) == 0x2b)
        })
        .unwrap_or(false)
}
