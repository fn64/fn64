//! OoT NTSC 1.0 host-first lookup shared by every rs-lane harness
//! (headless `oot-boot` and windowed `fn64-shell` include this file by
//! `#[path]`; it is game-profile data, so it lives beside the OoT harness,
//! not in the game-agnostic crates).

#[cfg(not(fn64_recomp_rs_block_program))]
use oot_recompiled as recompiled;

/// OoT NTSC 1.0's libultra vrams, from its decomp-derived symbol dump
/// (`games/OOTU/syms/dump.toml`). Each target is an ordinary safe typed
/// adapter; the raw C ABI exists only inside `fn64-abi::recompiled`.
fn exact_host_lookup(vram: u32) -> Option<fn64_recomp_rs::RecompFunc> {
    use fn64_abi::recompiled as r;
    let host = match vram {
        0x8000_1DB0 => r::os_pi_get_access,
        0x8000_1DF4 => r::os_pi_rel_access,
        0x8000_1E20 => r::os_send_mesg,
        0x8000_1F70 => r::os_stop_thread,
        0x8000_2030 => r::os_recv_mesg,
        0x8000_21D8 => r::ull_div,
        0x8000_227C => r::ll_div,
        0x8000_22D8 => r::ll_mul,
        0x8000_2D70 => r::os_destroy_thread,
        0x8000_2F20 => r::os_create_thread,
        0x8000_3070 => r::os_initialize,
        0x8000_3420 => r::os_set_sr,
        0x8000_3430 => r::os_get_sr,
        0x8000_3440 => r::os_writeback_dcache,
        0x8000_34C0 => r::os_vi_get_next_framebuffer,
        0x8000_3500 => r::os_create_pi_manager,
        0x8000_3B60 => r::os_virtual_to_physical,
        0x8000_3BE0 => r::os_vi_black,
        0x8000_3CA0 => r::os_get_thread_id,
        0x8000_3CC0 => r::os_set_int_mask,
        0x8000_3D60 => r::os_vi_set_mode,
        0x8000_3E90 => r::os_get_mem_size,
        0x8000_3FB0 => r::os_set_event_mesg,
        0x8000_40C0 => r::os_epi_start_dma,
        0x8000_41A0 => r::os_inval_icache,
        0x8000_4220 => r::os_create_mesg_queue,
        0x8000_4250 => r::os_inval_dcache,
        0x8000_4330 => r::os_jam_mesg,
        0x8000_4480 => r::os_set_thread_pri,
        0x8000_4560 => r::os_get_thread_pri,
        0x8000_48C0 => r::os_get_time,
        0x8000_4D50 => r::os_get_count,
        0x8000_5130 => r::os_disable_int,
        0x8000_51A0 => r::os_restore_int,
        0x8000_5630 => r::os_epi_read_io,
        0x8000_5680 => r::os_cart_rom_init,
        0x8000_5800 => r::os_epi_write_io,
        0x8000_5900 => r::os_get_cause,
        0x8000_5A70 => r::os_set_timer,
        0x8000_5BA0 => r::os_create_vi_manager,
        0x8000_5EC0 => r::os_start_thread,
        0x800C_FE20 => r::os_cont_init,
        0x800C_F7BC => r::os_sp_task_load,
        0x800C_F94C => r::os_sp_task_start_go,
        0x800C_F370 => r::os_get_int_mask,
        0x800D_0160 => r::os_cont_start_read_data,
        0x800D_01E4 => r::os_cont_get_read_data,
        0x800D_0660 => r::os_si_raw_start_dma,
        0x800D_0710 => r::os_sp_task_yield,
        0x800D_0CD0 => r::os_stop_timer,
        0x800D_0DF0 => r::os_cont_start_query,
        0x800D_0E74 => r::os_cont_get_query,
        0x800D_2420 => r::os_vi_swap_buffer,
        0x800D_2690 => r::os_sp_task_yielded,
        0x800D_2AF0 => r::os_dp_get_status,
        0x800D_2B00 => r::os_dp_set_status,
        0x800D_2E40 => r::os_vi_set_special_features,
        0x800D_3000 => r::os_vi_set_event,
        0x800D_3270 => r::os_cont_set_ch,
        0x800D_32E0 => r::os_ai_get_length,
        0x800D_5A80 => r::os_sp_get_status,
        0x800D_5A90 => r::os_sp_set_status,
        0x800D_5AA0 => r::os_writeback_dcache_all,
        0x800D_5CF0 => r::os_vi_set_y_scale,
        0x800D_5D50 => r::os_vi_get_current_framebuffer,
        0x800D_5D90 => r::os_sp_set_pc,
        0x800B_BE80 => r::os_ai_set_next_buffer,
        0x800D_2900 => r::os_ai_set_frequency,
        _ => return None,
    };
    Some(host)
}

/// Resolve only OoT's named host ABI adapters. The arbitrary-PC block lane
/// uses this narrower table so an omitted guest bank cannot silently escape
/// into the whole-function generated crate.
pub fn host_only_lookup(vram: u32) -> Option<fn64_recomp_rs::RecompFunc> {
    exact_host_lookup(vram).or_else(|| {
        let canonical = fn64_abi::recompiled::canonical_vram(vram)?;
        exact_host_lookup(canonical)
    })
}

/// Whole-function lane lookup: host adapters take priority, then the emitted
/// function dispatcher owns every remaining canonical guest destination.
#[cfg(not(fn64_recomp_rs_block_program))]
pub fn recompiled_or_host_lookup(vram: u32) -> Option<fn64_recomp_rs::RecompFunc> {
    if let Some(host) = host_only_lookup(vram) {
        return Some(host);
    }
    let canonical = fn64_abi::recompiled::canonical_vram(vram)?;
    Some(recompiled::lookup(canonical))
}
