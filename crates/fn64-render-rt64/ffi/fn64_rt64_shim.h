#ifndef FN64_RT64_SHIM_H
#define FN64_RT64_SHIM_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct Fn64Rt64Context Fn64Rt64Context;

typedef struct Fn64Rt64Task {
    uint32_t task_type;
    uint32_t flags;
    uint32_t ucode_boot;
    uint32_t ucode_boot_size;
    uint32_t ucode;
    uint32_t ucode_size;
    uint32_t ucode_data;
    uint32_t ucode_data_size;
    uint32_t dram_stack;
    uint32_t dram_stack_size;
    uint32_t output_buff;
    uint32_t output_buff_size;
    uint32_t data_ptr;
    uint32_t data_size;
} Fn64Rt64Task;

#ifdef __cplusplus
static_assert(sizeof(Fn64Rt64Task) == 14 * sizeof(uint32_t));
#endif

Fn64Rt64Context *fn64_rt64_create(
    uint32_t width,
    uint32_t height,
    char *error,
    size_t error_capacity);

int fn64_rt64_process_task(
    Fn64Rt64Context *context,
    uint8_t *rdram,
    size_t rdram_len,
    const Fn64Rt64Task *task,
    uint32_t output_addr,
    char *error,
    size_t error_capacity);

int fn64_rt64_present(
    Fn64Rt64Context *context,
    char *error,
    size_t error_capacity);

void fn64_rt64_resize(Fn64Rt64Context *context, uint32_t width, uint32_t height);
void fn64_rt64_destroy(Fn64Rt64Context *context);

#ifdef __cplusplus
}
#endif

#endif
