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

static inline bool fn64_is_rcp_mmio_word(gpr address) {
    const uint32_t low = static_cast<uint32_t>(address);
    return low >= 0xA4000000U && low < 0xA4900000U;
}

template <typename T, gpr LaneXor>
class fn64_mem_ref {
public:
    fn64_mem_ref(uint8_t* rdram_, gpr address_)
        : rdram(rdram_), address(address_) {}

    operator T() const {
        if (fn64_is_rcp_mmio_word(address)) {
            if constexpr (sizeof(T) != sizeof(uint32_t)) {
                fn64_c_mmio_bad_width(address, sizeof(T), 0);
                return T{};
            } else {
                return static_cast<T>(fn64_c_mmio_read_w(address));
            }
        }
        return *reinterpret_cast<T*>(
            rdram + ((address ^ LaneXor) - UINT64_C(0xFFFFFFFF80000000)));
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
        if (fn64_is_rcp_mmio_word(address)) {
            if constexpr (sizeof(T) != sizeof(uint32_t)) {
                fn64_c_mmio_bad_width(address, sizeof(T), 1);
                return;
            } else {
                fn64_c_mmio_write_w(address, static_cast<uint32_t>(value));
                return;
            }
        }
        *reinterpret_cast<T*>(
            rdram + ((address ^ LaneXor) - UINT64_C(0xFFFFFFFF80000000))) = value;
        fn64_c_rdram_write(address, sizeof(T));
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
