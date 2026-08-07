//! Attributed host access to guest memory, for code outside the guest.
//!
//! # Why this exists
//!
//! Every guest store already declares itself to the canonical mutation
//! journal: `Rdram::store_w` and friends call `notify_cpu_instruction_store`,
//! PI DMA calls `notify_pi_dma_write`, the renderer calls
//! `notify_rdp_renderer_write`. Host-side code had no such path. The only way
//! to write guest memory from outside a recompiled function was
//! `Rdram::as_mut_slice`, which declares nothing -- so bytes written that way
//! are invisible to the journal until the next dispatch trips
//! "unjournaled executable mutation" naming an address and no writer.
//!
//! That made the honest answer to "can a mod read or write guest memory?"
//! *"only by punching a hole in the firewall"*. This is the hole-free answer:
//! writes go through [`WriterChannel::HostAbi`], the same channel the ABI's own
//! host adapters use, inside a child transaction that reconciles the watched
//! ranges on commit.
//!
//! This ADDS no authority. It routes host writes onto the declared path that
//! already existed, which is why it needs no new `WriterChannel` variant --
//! the eight-variant denominator is deliberately fixed.

use super::execution::CatalogNestedWriterTransactionV1;
use crate::with_host;
use fn64_runtime::RdramAddr;

/// Read `len` bytes of guest physical RDRAM in guest byte order.
///
/// Reads need no attribution -- the journal only cares about mutation -- so
/// this is a plain checked copy. Returns `None` when no recompiled program is
/// live or the interval leaves registered RDRAM.
pub fn read_guest_physical(physical_start: u32, len: u32) -> Option<Vec<u8>> {
    let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
    if rdram.is_null() {
        return None;
    }
    let physical_end = physical_start.checked_add(len)?;
    if physical_end as usize > rdram_len {
        return None;
    }
    // SAFETY: the process RDRAM allocation is stable for the run, and this
    // borrow ends before returning. The bounds are checked immediately above.
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let mut bytes = vec![0u8; len as usize];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = unsafe { storage.read_u8(RdramAddr::from_offset(physical_start + index as u32)) };
    }
    Some(bytes)
}

/// Write `bytes` to guest physical RDRAM, declared as [`WriterChannel::HostAbi`].
///
/// The write is bracketed by a child writer transaction, so the canonical
/// mutation journal sees a declaration covering exactly the bytes written. If
/// they land in a watched executable range the journal records the mutation
/// with its writer attributed, instead of discovering an anonymous change at
/// the next dispatch.
///
/// Returns `false` when no recompiled program is live or the interval leaves
/// registered RDRAM; in neither case is anything written.
///
/// [`WriterChannel::HostAbi`]: fn64_recomp_rs::WriterChannel::HostAbi
pub fn write_guest_physical(physical_start: u32, bytes: &[u8]) -> bool {
    let len = match u32::try_from(bytes.len()) {
        Ok(len) => len,
        Err(_) => return false,
    };
    let Some(physical_end) = physical_start.checked_add(len) else {
        return false;
    };
    let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
    if rdram.is_null() {
        return false;
    }
    if physical_end as usize > rdram_len {
        return false;
    }
    if len == 0 {
        return true;
    }

    let live = with_host(|host| host.canonical_recompiled_program.clone());

    // SAFETY: as in `read_guest_physical`; bounds checked above, and the
    // borrow does not outlive this call.
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };

    // Without a live canonical program there is no journal to declare to, and
    // the write is just a memory poke. With one, the transaction is what makes
    // the bytes attributable.
    let Some(live) = live else {
        for (index, &byte) in bytes.iter().enumerate() {
            unsafe { storage.write_u8(RdramAddr::from_offset(physical_start + index as u32), byte) };
        }
        return true;
    };

    let transaction = match live.mutation_state.as_ref() {
        Some(state) => {
            let transaction_id = state.borrow_mut().begin_child_transaction();
            CatalogNestedWriterTransactionV1::for_host_memory_api(live.clone(), transaction_id)
        }
        None => CatalogNestedWriterTransactionV1::inert(),
    };
    for (index, &byte) in bytes.iter().enumerate() {
        unsafe { storage.write_u8(RdramAddr::from_offset(physical_start + index as u32), byte) };
    }
    fn64_recomp_rs::notify_host_abi_write(physical_start, len);
    // Commit against the view, not just the byte reader: the view lets the
    // changed-byte scan run one `memcmp` per watched range instead of
    // rebuilding the whole watched region a byte at a time. See
    // `commit_with_optional_view`.
    //
    // SAFETY: as for `storage` above -- `rdram` is non-null and `rdram_len` is
    // the registered length of that one allocation, both checked at the top of
    // this function, and neither the slice nor the view outlives this call.
    let view = fn64_runtime::RdramView::from_storage(unsafe {
        std::slice::from_raw_parts(rdram as *const u8, rdram_len)
    });
    transaction.commit_with_optional_view(
        |physical| unsafe { storage.read_u8(RdramAddr::from_offset(physical)) },
        Some(&view),
    );
    true
}
