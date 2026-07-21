#include <cstdio>
#include <cstring>
#include "fn64_mmio_proxy.h"

RECOMP_FUNC void fn64_proxy_generated_shape(uint8_t* rdram, recomp_context*) {
    const gpr vi_status = UINT64_C(0xFFFFFFFFA4400000);
    const gpr vi_origin = UINT64_C(0xFFFFFFFFA4400004);
    const gpr vi_v_sync = UINT64_C(0xFFFFFFFFA4400018);

    MEM_W(0, vi_status) = UINT32_C(0xFFFFFFFF);
    MEM_W(0, vi_origin) = UINT32_C(0xFFFFFFFF);
    MEM_W(0, vi_v_sync) = UINT32_C(0xFFFFFFFF);
}

int main(int argc, char** argv) {
    uint8_t storage[128] = {};
    uint8_t* rdram = storage;

    // Ordinary RDRAM remains recomp.h-compatible direct native-word storage.
    const gpr kseg0 = UINT64_C(0xFFFFFFFF80000000);
    const gpr kseg1 = UINT64_C(0xFFFFFFFFA0000000);
    const gpr ddrom_kseg1 = UINT64_C(0xFFFFFFFFA6000000);
    if (fn64_mem_storage_offset(kseg0 + 8, 2) != 10 ||
        fn64_mem_storage_offset(kseg1 + 8, 2) != 10 ||
        !fn64_is_sparse_direct_backing(ddrom_kseg1) ||
        fn64_is_rcp_mmio_word(ddrom_kseg1) ||
        fn64_is_rdram_direct_alias(ddrom_kseg1) ||
        fn64_mem_storage_offset(ddrom_kseg1, 0) != UINT64_C(0x26000000) ||
        fn64_mem_storage_offset(UINT64_C(0x00000000A6000000), 0) !=
            UINT64_C(0x26000000) ||
        fn64_mem_storage_offset(UINT64_C(8), 2) == 10 ||
        fn64_mem_storage_offset(UINT64_C(0xFFFFFFFFC0000008), 2) == 10) {
        return 11;
    }
    MEM_W(0, kseg0) = UINT32_C(0x12345678);
    if (static_cast<uint32_t>(MEM_W(0, kseg0)) != UINT32_C(0x12345678)) {
        return 1;
    }
    MEM_H(4, kseg0) = UINT16_C(0x6ABC);
    // The second assignment is deliberately identical: the generated-C
    // proxy must still emit the post-commit non-RDP 16-bit write event.
    MEM_H(4, kseg0) = UINT16_C(0x6ABC);
    MEM_B(6, kseg0) = UINT8_C(0x5D);
    if (static_cast<uint16_t>(MEM_HU(4, kseg0)) != UINT16_C(0x6ABC) ||
        static_cast<uint8_t>(MEM_BU(6, kseg0)) != UINT8_C(0x5D)) {
        return 5;
    }

    // Cached and uncached RDRAM are aliases of one physical device, not two
    // independent regions in the host's sparse KSEG backing allocation.
    MEM_H(8, kseg1) = UINT16_C(0x8123);
    if (static_cast<uint16_t>(MEM_HU(8, kseg0)) != UINT16_C(0x8123)) {
        return 9;
    }
    MEM_H(10, kseg0) = UINT16_C(0xFEDC);
    if (static_cast<uint16_t>(MEM_HU(10, kseg1)) != UINT16_C(0xFEDC)) {
        return 10;
    }
    MEM_W(12, UINT64_C(0x00000000A0000000)) = UINT32_C(0x76543210);
    if (static_cast<uint32_t>(MEM_W(12, UINT64_C(0x0000000080000000))) !=
            UINT32_C(0x76543210) ||
        static_cast<uint32_t>(MEM_W(12, kseg0)) != UINT32_C(0x76543210)) {
        return 26;
    }

    // The vendor header defines LD/SD and unaligned helpers while its raw
    // macros are active. Every family must resolve to the proxy-backed
    // replacements and therefore see the same KSEG0/KSEG1 bytes.
    SD(UINT64_C(0x0123456789ABCDEF), 16, kseg1);
    if (LD(16, kseg0) != UINT64_C(0x0123456789ABCDEF)) {
        return 14;
    }
    MEM_W(24, kseg0) = UINT32_C(0x89ABCDEF);
    for (gpr misalignment = 0; misalignment < 4; ++misalignment) {
        if (do_lwl(rdram, UINT64_C(0x11223344), 24 + misalignment, kseg0) !=
                do_lwl(rdram, UINT64_C(0x11223344), 24 + misalignment, kseg1) ||
            do_lwr(rdram, UINT64_C(0x55667788), 24 + misalignment, kseg0) !=
                do_lwr(rdram, UINT64_C(0x55667788), 24 + misalignment, kseg1)) {
            return 15;
        }
        MEM_W(28, kseg0) = UINT32_C(0x11223344);
        MEM_W(32, kseg0) = UINT32_C(0x11223344);
        do_swl(rdram, 28 + misalignment, kseg0, UINT32_C(0xA1B2C3D4));
        do_swl(rdram, 32 + misalignment, kseg1, UINT32_C(0xA1B2C3D4));
        if (static_cast<uint32_t>(MEM_W(28, kseg0)) !=
            static_cast<uint32_t>(MEM_W(32, kseg0))) {
            return 16;
        }
        MEM_W(36, kseg0) = UINT32_C(0x55667788);
        MEM_W(40, kseg0) = UINT32_C(0x55667788);
        do_swr(rdram, 36 + misalignment, kseg0, UINT32_C(0xD4C3B2A1));
        do_swr(rdram, 40 + misalignment, kseg1, UINT32_C(0xD4C3B2A1));
        if (static_cast<uint32_t>(MEM_W(36, kseg0)) !=
            static_cast<uint32_t>(MEM_W(40, kseg0))) {
            return 17;
        }
    }
    SD(UINT64_C(0x1020304050607080), 48, kseg0);
    for (gpr misalignment = 0; misalignment < 8; ++misalignment) {
        if (do_ldl(rdram, UINT64_C(0xFFEEDDCCBBAA9988), 48 + misalignment, kseg0) !=
                do_ldl(rdram, UINT64_C(0xFFEEDDCCBBAA9988), 48 + misalignment, kseg1) ||
            do_ldr(rdram, UINT64_C(0x8877665544332211), 48 + misalignment, kseg0) !=
                do_ldr(rdram, UINT64_C(0x8877665544332211), 48 + misalignment, kseg1)) {
            return 18;
        }
        SD(UINT64_C(0x1111222233334444), 64, kseg0);
        SD(UINT64_C(0x1111222233334444), 72, kseg0);
        do_sdl(rdram, 64 + misalignment, kseg0, UINT64_C(0xA1A2A3A4A5A6A7A8));
        do_sdl(rdram, 72 + misalignment, kseg1, UINT64_C(0xA1A2A3A4A5A6A7A8));
        if (LD(64, kseg0) != LD(72, kseg0)) {
            return 19;
        }
        SD(UINT64_C(0x5555666677778888), 80, kseg0);
        SD(UINT64_C(0x5555666677778888), 88, kseg0);
        do_sdr(rdram, 80 + misalignment, kseg0, UINT64_C(0xB1B2B3B4B5B6B7B8));
        do_sdr(rdram, 88 + misalignment, kseg1, UINT64_C(0xB1B2B3B4B5B6B7B8));
        if (LD(80, kseg0) != LD(88, kseg0)) {
            return 20;
        }
    }

    MEM_W(0, UINT64_C(0xFFFFFFFFBFC007C8)) = UINT32_C(0xCAFEBABE);
    if (static_cast<uint32_t>(MEM_W(0, UINT64_C(0xFFFFFFFF9FC007C8))) !=
        UINT32_C(0xCAFEBABE)) {
        return 21;
    }

    if (argc > 1 && std::strcmp(argv[1], "--bad-width") == 0) {
        MEM_H(0, UINT64_C(0xFFFFFFFFA4400000)) = UINT16_C(1);
        return 6;
    }
    if (argc > 1 && std::strcmp(argv[1], "--bad-kuseg") == 0) {
        (void)static_cast<uint32_t>(MEM_W(0, UINT64_C(0x0000000000000000)));
        return 12;
    }
    if (argc > 1 && std::strcmp(argv[1], "--bad-kseg2") == 0) {
        MEM_W(0, UINT64_C(0xFFFFFFFFC0000000)) = UINT32_C(1);
        return 13;
    }
    if (argc > 1 && std::strcmp(argv[1], "--bad-noncanonical-sparse") == 0) {
        (void)static_cast<uint32_t>(MEM_W(0, UINT64_C(0x00000001A6000000)));
        return 29;
    }
    if (argc > 1 && std::strcmp(argv[1], "--bad-pif-kuseg") == 0) {
        (void)static_cast<uint32_t>(MEM_W(0, UINT64_C(0x000000001FC007C0)));
        return 22;
    }
    if (argc > 1 && std::strcmp(argv[1], "--bad-pif-kseg2") == 0) {
        MEM_W(0, UINT64_C(0xFFFFFFFFDFC007C0)) = UINT32_C(1);
        return 23;
    }
    if (argc > 1 && std::strcmp(argv[1], "--bad-dword-read") == 0) {
        (void)LD(0, UINT64_C(0xFFFFFFFFA4000000));
        return 24;
    }
    if (argc > 1 && std::strcmp(argv[1], "--bad-dword-write") == 0) {
        SD(UINT64_C(1), 0, UINT64_C(0xFFFFFFFFBFC007C0));
        return 25;
    }
    if (argc > 1 && std::strcmp(argv[1], "--bad-swl") == 0) {
        do_swl(rdram, 1, UINT64_C(0xFFFFFFFFA4400000), UINT32_C(1));
        return 27;
    }
    if (argc > 1 && std::strcmp(argv[1], "--bad-swr") == 0) {
        do_swr(rdram, 2, UINT64_C(0xFFFFFFFFA4400000), UINT32_C(1));
        return 28;
    }

    fn64_proxy_generated_shape(rdram, nullptr);
    if (static_cast<uint32_t>(MEM_W(0, UINT64_C(0xFFFFFFFFA4400000))) !=
        UINT32_C(0x0001FFFF)) {
        return 2;
    }
    if (static_cast<uint32_t>(MEM_W(0, UINT64_C(0xFFFFFFFFA4400004))) !=
        UINT32_C(0x00FFFFFF)) {
        return 3;
    }
    if (static_cast<uint32_t>(MEM_W(0, UINT64_C(0xFFFFFFFFA4400018))) !=
        UINT32_C(0x000003FF)) {
        return 4;
    }

    // A generated-C raw AI start reaches the timed FIFO, not the old shadow
    // register: BUSY is live immediately after the length command.
    MEM_W(0, UINT64_C(0xFFFFFFFFA4500010)) = UINT32_C(151);
    MEM_W(0, UINT64_C(0xFFFFFFFFA4500000)) = UINT32_C(0x1000);
    MEM_W(0, UINT64_C(0xFFFFFFFFA4500004)) = UINT32_C(0x80);
    if ((static_cast<uint32_t>(
             MEM_W(0, UINT64_C(0xFFFFFFFFA450000C))) &
         UINT32_C(0x40000000)) == 0) {
        return 7;
    }

    // SP memory and the real PC address use the same proxy and persistent
    // DeviceFabric state. SP_WR_LEN at A404000C is deliberately not confused
    // with PC (A4080000).
    MEM_W(0, UINT64_C(0xFFFFFFFFA4000000)) = UINT32_C(0xDEADBEEF);
    MEM_W(0, UINT64_C(0xFFFFFFFFA4001000)) = UINT32_C(0x12345678);
    MEM_W(0, UINT64_C(0xFFFFFFFFA4080000)) = UINT32_C(0x000001A8);
    if (static_cast<uint32_t>(MEM_W(0, UINT64_C(0xFFFFFFFFA4000000))) !=
            UINT32_C(0xDEADBEEF) ||
        static_cast<uint32_t>(MEM_W(0, UINT64_C(0xFFFFFFFFA4001000))) !=
            UINT32_C(0x12345678) ||
        static_cast<uint32_t>(MEM_W(0, UINT64_C(0xFFFFFFFFA4080000))) !=
            UINT32_C(0x000001A8)) {
        return 8;
    }

    std::puts("fn64 generated-C MMIO proxy: live DeviceFabric round-trip OK");
    return 0;
}
