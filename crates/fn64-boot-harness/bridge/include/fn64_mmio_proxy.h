#ifndef FN64_MMIO_PROXY_H
#define FN64_MMIO_PROXY_H

// Clean-room adapter for N64Recomp-generated C. The generated MEM_W macro is
// an lvalue, so plain C cannot observe the value of an assignment. Building
// generated translation units as C++ lets this narrow proxy preserve lvalue
// syntax while routing only KSEG1 RCP words into fn64's live DeviceFabric.
// Ordinary RDRAM words retain recomp.h's direct native-word access.

#include "recomp.h"

#ifndef __cplusplus
#error "fn64_mmio_proxy.h requires generated translation units to compile as C++"
#endif

extern "C" int32_t fn64_c_mmio_read_w(uint64_t vaddr);
extern "C" void fn64_c_mmio_write_w(uint64_t vaddr, uint32_t value);
extern "C" void fn64_c_mmio_bad_width(uint64_t vaddr, uint32_t width,
                                      uint32_t is_write);
extern "C" void fn64_c_rdram_write(uint64_t vaddr, uint32_t width);
extern "C" void fn64_c_mem_unaligned(uint64_t vaddr, uint32_t width,
                                     uint32_t is_write);

static inline bool fn64_is_rcp_mmio_word(gpr address) {
    const uint32_t low = static_cast<uint32_t>(address);
    return low >= 0xA4000000U && low < 0xA4900000U;
}

// KSEG1 uncached-mirror folding. On hardware, KSEG0 (0x80000000+) and KSEG1
// (0xA0000000+) map the SAME physical DRAM -- an uncached pointer built by
// `or $v0, $v0, 0xA0000000` (WM2000's own task loader does exactly this at
// 0x80031E28 to read `ucode_data + 0xBFC`, and its raw-read helper at
// 0x800373B8 does the same) must observe the bytes the cached KSEG0 view
// holds. recomp.h's generated address math (`addr - 0xFFFFFFFF80000000`)
// instead lands KSEG1 RAM at rdram offset 0x20000000 + phys: a disjoint,
// permanently-zero region of the oversized mapping, so uncached reads
// returned deterministic zeros and uncached writes vanished from the cached
// view. Fold non-RCP KSEG1 RAM (0xA0000000..0xA4000000, i.e. phys < 64 MiB,
// below the modeled RCP register window) down onto the KSEG0/physical bytes.
// The RCP raw-register window (0xA4000000..0xA9000000) keeps its dedicated
// backing range (0x24000000+) -- `sync_mmio_into_rdram` owns those bytes.
static inline gpr fn64_fold_kseg1_mirror(gpr address) {
    const uint32_t low = static_cast<uint32_t>(address);
    if (low - 0xA0000000U < 0x04000000U) {
        return address - UINT64_C(0x20000000);
    }
    return address;
}

template <typename T, gpr LaneXor>
class fn64_mem_ref {
public:
    fn64_mem_ref(uint8_t* rdram_, gpr address_)
        : rdram(rdram_), address(address_) {}

    operator T() const {
        // MIPS lw/lh REQUIRE natural alignment -- hardware raises an
        // address-error exception (AdEL) on violation. recomp.h's raw
        // pointer cast instead reads a byte-lane-swizzled CHIMERA of two
        // adjacent native words, feeding the guest deterministic garbage
        // that surfaces as a wild pointer far downstream (the WM2000
        // demo-scene 0x27813FE8 crash). Trap loudly at the first violation.
        if constexpr (sizeof(T) > 1) {
            if ((address & (sizeof(T) - 1)) != 0) {
                fn64_c_mem_unaligned(address, sizeof(T), 0);
            }
        }
        if (fn64_is_rcp_mmio_word(address)) {
            if constexpr (sizeof(T) != sizeof(uint32_t)) {
                fn64_c_mmio_bad_width(address, sizeof(T), 0);
                return T{};
            } else {
                return static_cast<T>(fn64_c_mmio_read_w(address));
            }
        }
        return *reinterpret_cast<T*>(
            rdram + ((fn64_fold_kseg1_mirror(address) ^ LaneXor) -
                     UINT64_C(0xFFFFFFFF80000000)));
    }

    template <typename U>
    fn64_mem_ref& operator=(U value) {
        store(static_cast<T>(value));
        return *this;
    }

    fn64_mem_ref& operator=(const fn64_mem_ref& other) {
        store(static_cast<T>(other));
        return *this;
    }

private:
    void store(T value) {
        // See the load path: MIPS sw/sh trap AdES on unaligned addresses.
        if constexpr (sizeof(T) > 1) {
            if ((address & (sizeof(T) - 1)) != 0) {
                fn64_c_mem_unaligned(address, sizeof(T), 1);
            }
        }
        if (fn64_is_rcp_mmio_word(address)) {
            if constexpr (sizeof(T) != sizeof(uint32_t)) {
                fn64_c_mmio_bad_width(address, sizeof(T), 1);
                return;
            } else {
                fn64_c_mmio_write_w(address, static_cast<uint32_t>(value));
                return;
            }
        }
        const gpr folded = fn64_fold_kseg1_mirror(address);
        *reinterpret_cast<T*>(
            rdram + ((folded ^ LaneXor) - UINT64_C(0xFFFFFFFF80000000))) = value;
        fn64_c_rdram_write(folded, sizeof(T));
    }

    uint8_t* rdram;
    gpr address;
};

#undef MEM_W
#define MEM_W(offset, reg) \
    fn64_mem_ref<int32_t, 0>(rdram, \
        static_cast<gpr>(reg) + static_cast<gpr>(offset))
#undef MEM_H
#define MEM_H(offset, reg) \
    fn64_mem_ref<int16_t, 2>(rdram, \
        static_cast<gpr>(reg) + static_cast<gpr>(offset))
#undef MEM_B
#define MEM_B(offset, reg) \
    fn64_mem_ref<int8_t, 3>(rdram, \
        static_cast<gpr>(reg) + static_cast<gpr>(offset))
#undef MEM_HU
#define MEM_HU(offset, reg) \
    fn64_mem_ref<uint16_t, 2>(rdram, \
        static_cast<gpr>(reg) + static_cast<gpr>(offset))
#undef MEM_BU
#define MEM_BU(offset, reg) \
    fn64_mem_ref<uint8_t, 3>(rdram, \
        static_cast<gpr>(reg) + static_cast<gpr>(offset))

// Generated functions must keep the unmangled ABI that fn64-abi and the
// section tables link against even though these translation units now use the
// C++ compiler for the lvalue proxy.
#undef RECOMP_FUNC
#if defined(_MSC_VER) && !defined(__clang__) && !defined(__INTEL_COMPILER)
#define RECOMP_FUNC extern "C" __declspec(noinline)
#elif defined(__clang__)
// No `inline`: a C++ inline function that is never called in its own
// translation unit emits no symbol at all, which empties every generated
// object file. `weak` alone provides the cross-TU duplicate tolerance.
#define RECOMP_FUNC extern "C" __attribute__((weak, noinline))
#elif defined(__GNUC__) && !defined(__INTEL_COMPILER)
#define RECOMP_FUNC \
    extern "C" __attribute__((noipa, optimize("rounding-math")))
#else
#error "No fn64 RECOMP_FUNC definition for this C++ compiler"
#endif

#endif
