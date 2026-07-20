#include <cstdio>
#include "fn64_mmio_proxy.h"

RECOMP_FUNC void fn64_proxy_generated_shape(uint8_t* rdram, recomp_context*) {
    const gpr vi_status = UINT64_C(0xFFFFFFFFA4400000);
    const gpr vi_origin = UINT64_C(0xFFFFFFFFA4400004);
    const gpr vi_v_sync = UINT64_C(0xFFFFFFFFA4400018);

    MEM_W(0, vi_status) = UINT32_C(0xFFFFFFFF);
    MEM_W(0, vi_origin) = UINT32_C(0xFFFFFFFF);
    MEM_W(0, vi_v_sync) = UINT32_C(0xFFFFFFFF);
}

int main(int argc, char**) {
    uint8_t storage[16] = {};
    uint8_t* rdram = storage;

    // Ordinary RDRAM remains recomp.h-compatible direct native-word storage.
    const gpr kseg0 = UINT64_C(0xFFFFFFFF80000000);
    MEM_W(0, kseg0) = UINT32_C(0x12345678);
    if (static_cast<uint32_t>(MEM_W(0, kseg0)) != UINT32_C(0x12345678)) {
        return 1;
    }
    MEM_H(4, kseg0) = UINT16_C(0x6ABC);
    MEM_B(6, kseg0) = UINT8_C(0x5D);
    if (static_cast<uint16_t>(MEM_HU(4, kseg0)) != UINT16_C(0x6ABC) ||
        static_cast<uint8_t>(MEM_BU(6, kseg0)) != UINT8_C(0x5D)) {
        return 5;
    }

    if (argc > 1) {
        MEM_H(0, UINT64_C(0xFFFFFFFFA4400000)) = UINT16_C(1);
        return 6;
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
