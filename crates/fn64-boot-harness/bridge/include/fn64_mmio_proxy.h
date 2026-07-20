#ifndef FN64_MMIO_PROXY_H
#define FN64_MMIO_PROXY_H

// Clean-room adapter for N64Recomp-generated C. The generated MEM_W macro is
// an lvalue, so plain C cannot observe the value of an assignment. Building
// generated translation units as C++ lets this narrow proxy preserve lvalue
// syntax while routing only KSEG1 RCP words into fn64's live DeviceFabric.
// Ordinary RDRAM words retain recomp.h's direct native-word access.

// The vendor header defines these helpers while its raw MEM_* macros are
// active. Rename those definitions so generated call sites resolve to the
// proxy-backed versions below; otherwise LD/SD and every unaligned family
// bypass alias canonicalization, MMIO routing, and post-store events.
#define load_doubleword fn64_vendor_load_doubleword
#define do_lwl fn64_vendor_do_lwl
#define do_lwr fn64_vendor_do_lwr
#define do_swl fn64_vendor_do_swl
#define do_swr fn64_vendor_do_swr
#define do_ldl fn64_vendor_do_ldl
#define do_ldr fn64_vendor_do_ldr
#define do_sdl fn64_vendor_do_sdl
#define do_sdr fn64_vendor_do_sdr
#include "recomp.h"
#undef load_doubleword
#undef do_lwl
#undef do_lwr
#undef do_swl
#undef do_swr
#undef do_ldl
#undef do_ldr
#undef do_sdl
#undef do_sdr

#ifndef __cplusplus
#error "fn64_mmio_proxy.h requires generated translation units to compile as C++"
#endif

extern "C" int32_t fn64_c_mmio_read_w(uint64_t vaddr);
extern "C" void fn64_c_mmio_write_w(uint64_t vaddr, uint32_t value);
extern "C" void fn64_c_mmio_bad_width(uint64_t vaddr, uint32_t width,
                                      uint32_t is_write);
extern "C" void fn64_c_bad_direct_address(uint64_t vaddr, uint32_t width,
                                           uint32_t is_write);
extern "C" void fn64_c_rdram_write(uint64_t vaddr, uint32_t width,
                                    uint64_t value);
extern "C" void fn64_c_recompiled_function_enter(recomp_func_t* function);

static inline bool fn64_is_rcp_mmio_word(gpr address) {
    const uint64_t upper = address >> 32;
    const uint32_t low = static_cast<uint32_t>(address);
    const bool canonical_32 = upper == 0 || upper == UINT32_MAX;
    if (canonical_32 && low >= 0xA4000000U && low < 0xA4900000U) {
        return true;
    }
    // PIF RAM (physical 0x1FC007C0..0x1FC00800, KSEG0 or KSEG1 view):
    // direct uncached CPU access is real hardware behavior (AKI-era joybus
    // code polls it raw); the Rust seam backs it with the device fabric's
    // one PIF RAM, shared with SI DMA.
    const uint32_t physical = low & 0x1FFFFFFFU;
    const bool direct_segment = low >= 0x80000000U && low < 0xC0000000U;
    return canonical_32 && direct_segment &&
        physical >= 0x1FC007C0U && physical < 0x1FC00800U;
}

static inline bool fn64_is_rdram_direct_alias(gpr address) {
    const uint64_t upper = address >> 32;
    const uint32_t low = static_cast<uint32_t>(address);
    const uint32_t physical = low & 0x1FFFFFFFU;
    const bool canonical_32 = upper == 0 || upper == UINT32_MAX;
    const bool direct_segment = low >= 0x80000000U && low < 0xC0000000U;
    return canonical_32 && direct_segment && physical < 0x00800000U;
}

static inline bool fn64_is_unsupported_rdram_alias(gpr address) {
    const uint32_t physical = static_cast<uint32_t>(address) & 0x1FFFFFFFU;
    return physical < 0x00800000U && !fn64_is_rdram_direct_alias(address);
}

static inline bool fn64_is_unsupported_pif_alias(gpr address) {
    const uint32_t physical = static_cast<uint32_t>(address) & 0x1FFFFFFFU;
    return physical >= 0x1FC007C0U && physical < 0x1FC00800U &&
        !fn64_is_rcp_mmio_word(address);
}

static inline bool fn64_is_invalid_rdram_access(gpr address, uint32_t width) {
    if (!fn64_is_rdram_direct_alias(address)) {
        return false;
    }
    const uint32_t physical = static_cast<uint32_t>(address) & 0x1FFFFFFFU;
    return (physical & (width - 1)) != 0 ||
        width > 0x00800000U - physical;
}

// N64Recomp's raw pointer formula gives KSEG1 a sparse host offset 512 MiB
// above KSEG0. Real RDRAM is nevertheless one physical device: its cached
// and uncached direct-segment aliases must address the same bytes. Keep the
// sparse formula for non-RDRAM windows and unsupported mapped addresses; a
// low-29-bit mask alone would silently invent TLB behavior for KUSEG.
static inline uint64_t fn64_mem_storage_offset(gpr address, gpr lane_xor) {
    const uint32_t physical = static_cast<uint32_t>(address) & 0x1FFFFFFFU;
    if (fn64_is_rdram_direct_alias(address)) {
        return static_cast<uint64_t>(physical) ^ lane_xor;
    }
    return (address ^ lane_xor) - UINT64_C(0xFFFFFFFF80000000);
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
        if (fn64_is_unsupported_rdram_alias(address) ||
            fn64_is_unsupported_pif_alias(address)) {
            fn64_c_bad_direct_address(address, sizeof(T), 0);
            return T{};
        }
        if (fn64_is_invalid_rdram_access(address, sizeof(T))) {
            fn64_c_bad_direct_address(address, sizeof(T), 0);
            return T{};
        }
        return *reinterpret_cast<T*>(
            rdram + fn64_mem_storage_offset(address, LaneXor));
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
        if (fn64_is_unsupported_rdram_alias(address) ||
            fn64_is_unsupported_pif_alias(address)) {
            fn64_c_bad_direct_address(address, sizeof(T), 1);
            return;
        }
        if (fn64_is_invalid_rdram_access(address, sizeof(T))) {
            fn64_c_bad_direct_address(address, sizeof(T), 1);
            return;
        }
        *reinterpret_cast<T*>(
            rdram + fn64_mem_storage_offset(address, LaneXor)) = value;
        if (fn64_is_rdram_direct_alias(address)) {
            fn64_c_rdram_write(address, sizeof(T), static_cast<uint64_t>(value));
        }
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

static inline void fn64_store_doubleword(uint8_t* rdram, gpr address, uint64_t value) {
    if (fn64_is_rcp_mmio_word(address) || fn64_is_rcp_mmio_word(address + 4)) {
        fn64_c_mmio_bad_width(address, 8, 1);
        return;
    }
    if (fn64_is_unsupported_rdram_alias(address) ||
        fn64_is_unsupported_pif_alias(address) ||
        fn64_is_invalid_rdram_access(address, 8)) {
        fn64_c_bad_direct_address(address, 8, 1);
        return;
    }
    if (fn64_is_rdram_direct_alias(address) &&
        fn64_is_rdram_direct_alias(address + 4)) {
        // Preserve N64Recomp's low-word then high-word commit order, but
        // publish one post-commit range for the architectural SD.
        *reinterpret_cast<uint32_t*>(rdram + fn64_mem_storage_offset(address + 4, 0)) =
            static_cast<uint32_t>(value);
        *reinterpret_cast<uint32_t*>(rdram + fn64_mem_storage_offset(address, 0)) =
            static_cast<uint32_t>(value >> 32);
        fn64_c_rdram_write(address, 8, value);
        return;
    }
    MEM_W(4, address) = static_cast<uint32_t>(value);
    MEM_W(0, address) = static_cast<uint32_t>(value >> 32);
}

#undef SD
#define SD(val, offset, reg) \
    fn64_store_doubleword(rdram, static_cast<gpr>(reg) + static_cast<gpr>(offset), \
                          static_cast<uint64_t>(val))

static inline uint64_t load_doubleword(uint8_t* rdram, gpr reg, gpr offset) {
    const gpr address = reg + offset;
    if (fn64_is_rcp_mmio_word(address) || fn64_is_rcp_mmio_word(address + 4)) {
        fn64_c_mmio_bad_width(address, 8, 0);
        return 0;
    }
    if (fn64_is_unsupported_rdram_alias(address) ||
        fn64_is_unsupported_pif_alias(address) ||
        fn64_is_invalid_rdram_access(address, 8)) {
        fn64_c_bad_direct_address(address, 8, 0);
        return 0;
    }
    const uint64_t lo = static_cast<uint32_t>(MEM_W(reg, offset + 4));
    const uint64_t hi = static_cast<uint32_t>(MEM_W(reg, offset + 0));
    return lo | (hi << 32);
}

static inline gpr do_lwl(uint8_t* rdram, gpr initial_value, gpr offset, gpr reg) {
    const gpr address = offset + reg;
    const gpr word_address = address & ~UINT64_C(3);
    uint32_t loaded_value = static_cast<uint32_t>(MEM_W(0, word_address));
    const gpr misalignment = address & 3;
    const gpr masked_value = initial_value &
        static_cast<gpr>(static_cast<uint32_t>(~(UINT32_MAX << (misalignment * 8))));
    loaded_value <<= misalignment * 8;
    return static_cast<gpr>(static_cast<int32_t>(masked_value | loaded_value));
}

static inline gpr do_lwr(uint8_t* rdram, gpr initial_value, gpr offset, gpr reg) {
    const gpr address = offset + reg;
    const gpr word_address = address & ~UINT64_C(3);
    uint32_t loaded_value = static_cast<uint32_t>(MEM_W(0, word_address));
    const gpr misalignment = address & 3;
    const gpr masked_value = initial_value & static_cast<gpr>(static_cast<uint32_t>(
        ~(UINT32_MAX >> (24 - misalignment * 8))));
    loaded_value >>= 24 - misalignment * 8;
    return static_cast<gpr>(static_cast<int32_t>(masked_value | loaded_value));
}

static inline void do_swl(uint8_t* rdram, gpr offset, gpr reg, gpr val) {
    const gpr address = offset + reg;
    const gpr word_address = address & ~UINT64_C(3);
    const gpr misalignment = address & 3;
    if (fn64_is_rcp_mmio_word(word_address)) {
        if (misalignment != 0) {
            fn64_c_mmio_bad_width(address, 4 - misalignment, 1);
            return;
        }
        MEM_W(0, word_address) = static_cast<uint32_t>(val);
        return;
    }
    const uint32_t initial_value = static_cast<uint32_t>(MEM_W(0, word_address));
    const uint32_t masked = initial_value & ~(UINT32_MAX >> (misalignment * 8));
    MEM_W(0, word_address) = masked | (static_cast<uint32_t>(val) >> (misalignment * 8));
}

static inline void do_swr(uint8_t* rdram, gpr offset, gpr reg, gpr val) {
    const gpr address = offset + reg;
    const gpr word_address = address & ~UINT64_C(3);
    const gpr misalignment = address & 3;
    if (fn64_is_rcp_mmio_word(word_address)) {
        if (misalignment != 3) {
            fn64_c_mmio_bad_width(address, misalignment + 1, 1);
            return;
        }
        MEM_W(0, word_address) = static_cast<uint32_t>(val);
        return;
    }
    const uint32_t initial_value = static_cast<uint32_t>(MEM_W(0, word_address));
    const uint32_t masked = initial_value & ~(UINT32_MAX << (24 - misalignment * 8));
    MEM_W(0, word_address) = masked | (static_cast<uint32_t>(val) << (24 - misalignment * 8));
}

static inline gpr do_ldl(uint8_t* rdram, gpr initial_value, gpr offset, gpr reg) {
    const gpr address = offset + reg;
    const gpr dword_address = address & ~UINT64_C(7);
    uint64_t loaded_value = load_doubleword(rdram, 0, dword_address);
    const gpr misalignment = address & 7;
    const gpr masked = initial_value & ~(UINT64_MAX << (misalignment * 8));
    loaded_value <<= misalignment * 8;
    return masked | loaded_value;
}

static inline gpr do_ldr(uint8_t* rdram, gpr initial_value, gpr offset, gpr reg) {
    const gpr address = offset + reg;
    const gpr dword_address = address & ~UINT64_C(7);
    uint64_t loaded_value = load_doubleword(rdram, 0, dword_address);
    const gpr misalignment = address & 7;
    const gpr masked = initial_value & ~(UINT64_MAX >> (56 - misalignment * 8));
    loaded_value >>= 56 - misalignment * 8;
    return masked | loaded_value;
}

static inline void do_sdl(uint8_t* rdram, gpr offset, gpr reg, gpr val) {
    const gpr address = offset + reg;
    const gpr dword_address = address & ~UINT64_C(7);
    const uint64_t initial = load_doubleword(rdram, 0, dword_address);
    const gpr misalignment = address & 7;
    const uint64_t result = (initial & ~(UINT64_MAX >> (misalignment * 8))) |
        (val >> (misalignment * 8));
    fn64_store_doubleword(rdram, dword_address, result);
}

static inline void do_sdr(uint8_t* rdram, gpr offset, gpr reg, gpr val) {
    const gpr address = offset + reg;
    const gpr dword_address = address & ~UINT64_C(7);
    const uint64_t initial = load_doubleword(rdram, 0, dword_address);
    const gpr misalignment = address & 7;
    const uint64_t result = (initial & ~(UINT64_MAX << (56 - misalignment * 8))) |
        (val << (56 - misalignment * 8));
    fn64_store_doubleword(rdram, dword_address, result);
}

// Generated functions must keep the unmangled ABI that fn64-abi and the
// section tables link against even though these translation units now use the
// C++ compiler for the lvalue proxy.
#undef RECOMP_FUNC
#if defined(_MSC_VER) && !defined(__clang__) && !defined(__INTEL_COMPILER)
#define RECOMP_FUNC extern "C" __declspec(noinline)
#elif defined(__clang__)
// N64Recomp's C spelling uses `extern inline` to force an emitted external
// definition. C++ gives that spelling different linkage semantics and may
// emit no body at all, so retain weak interposition and no-inlining without
// the C-only `inline` mechanism.
#define RECOMP_FUNC extern "C" __attribute__((weak, noinline))
#elif defined(__GNUC__) && !defined(__INTEL_COMPILER)
#define RECOMP_FUNC \
    extern "C" __attribute__((noipa, optimize("rounding-math")))
#else
#error "No fn64 RECOMP_FUNC definition for this C++ compiler"
#endif

#endif
