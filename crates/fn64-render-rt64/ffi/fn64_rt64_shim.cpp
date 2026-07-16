#include "fn64_rt64_shim.h"

#include <algorithm>
#include <array>
#include <cstdio>
#include <cstring>
#include <exception>
#include <memory>
#include <new>
#include <string>

#include <SDL.h>

#include "hle/rt64_application.h"
#include "hle/rt64_present_queue.h"
#include "hle/rt64_workload_queue.h"

namespace {
constexpr size_t N64_RDRAM_SIZE = 8U * 1024U * 1024U;
constexpr uint32_t VI_STATUS_16_BIT = 2U;

void ignore_interrupts() {}

void set_error(char *error, size_t capacity, const std::string &message) {
    if ((error == nullptr) || (capacity == 0)) {
        return;
    }

    const size_t count = std::min(capacity - 1, message.size());
    std::memcpy(error, message.data(), count);
    error[count] = '\0';
}

const char *setup_result_name(RT64::Application::SetupResult result) {
    switch (result) {
    case RT64::Application::SetupResult::Success:
        return "success";
    case RT64::Application::SetupResult::DynamicLibrariesNotFound:
        return "required graphics dynamic libraries were not found";
    case RT64::Application::SetupResult::InvalidGraphicsAPI:
        return "the configured graphics API is invalid on this platform";
    case RT64::Application::SetupResult::GraphicsAPINotFound:
        return "no supported graphics API could be initialized";
    case RT64::Application::SetupResult::GraphicsDeviceNotFound:
        return "no compatible graphics device was found";
    }

    return "unknown setup result";
}
} // namespace

struct Fn64Rt64Context {
    std::array<uint8_t, 64> header{};
    std::array<uint8_t, 4096> dmem{};
    std::array<uint8_t, 4096> imem{};
    std::unique_ptr<uint8_t[]> placeholder_rdram;
    std::array<uint32_t, 24> registers{};
    std::unique_ptr<RT64::Application> application;
    uint8_t *active_rdram = nullptr;
    size_t active_rdram_len = 0;
    uint32_t width = 320;
    uint32_t height = 240;
    uint32_t output_addr = 0;
    bool setup_complete = false;

    Fn64Rt64Context(uint32_t width, uint32_t height)
        : placeholder_rdram(std::make_unique<uint8_t[]>(N64_RDRAM_SIZE)),
          width(width),
          height(height) {
        std::memset(placeholder_rdram.get(), 0, N64_RDRAM_SIZE);
    }

    ~Fn64Rt64Context() {
        if (setup_complete && application) {
            application->end();
        }
    }

    RT64::Application::Core make_core() {
        RT64::Application::Core core{};
        core.HEADER = header.data();
        core.RDRAM = placeholder_rdram.get();
        core.DMEM = dmem.data();
        core.IMEM = imem.data();
        core.MI_INTR_REG = &registers[0];
        core.DPC_START_REG = &registers[1];
        core.DPC_END_REG = &registers[2];
        core.DPC_CURRENT_REG = &registers[3];
        core.DPC_STATUS_REG = &registers[4];
        core.DPC_CLOCK_REG = &registers[5];
        core.DPC_BUFBUSY_REG = &registers[6];
        core.DPC_PIPEBUSY_REG = &registers[7];
        core.DPC_TMEM_REG = &registers[8];
        core.VI_STATUS_REG = &registers[9];
        core.VI_ORIGIN_REG = &registers[10];
        core.VI_WIDTH_REG = &registers[11];
        core.VI_INTR_REG = &registers[12];
        core.VI_V_CURRENT_LINE_REG = &registers[13];
        core.VI_TIMING_REG = &registers[14];
        core.VI_V_SYNC_REG = &registers[15];
        core.VI_H_SYNC_REG = &registers[16];
        core.VI_LEAP_REG = &registers[17];
        core.VI_H_START_REG = &registers[18];
        core.VI_V_START_REG = &registers[19];
        core.VI_V_BURST_REG = &registers[20];
        core.VI_X_SCALE_REG = &registers[21];
        core.VI_Y_SCALE_REG = &registers[22];
        core.checkInterrupts = ignore_interrupts;
        return core;
    }

    void update_vi() {
        registers[9] = VI_STATUS_16_BIT;
        // RT64 compensates for the VI origin convention by subtracting one
        // scanline. Supplying the following line makes decodeVI().fbAddress()
        // equal fn64's exact physical output_addr.
        registers[10] = output_addr + width * 2U;
        registers[11] = width;
        registers[15] = 525;
        registers[16] = 3093;
        registers[18] = (108U << 16U) | 748U;
        registers[19] = (34U << 16U) | (34U + height * 2U);
        registers[21] = 0x400;
        registers[22] = 0x400;
    }
};

extern "C" Fn64Rt64Context *fn64_rt64_create(
    uint32_t width,
    uint32_t height,
    char *error,
    size_t error_capacity) {
    try {
        if ((width == 0) || (height == 0)) {
            set_error(error, error_capacity, "render dimensions must be non-zero");
            return nullptr;
        }

        // RT64's internal window helper asserts after these failures. Probe
        // the same headless/display prerequisites first so an unavailable
        // display degrades to a Rust error instead of reaching that assert.
        if (SDL_VideoInit(nullptr) != 0) {
            set_error(error, error_capacity, std::string("SDL video initialization failed: ") + SDL_GetError());
            return nullptr;
        }
        SDL_DisplayMode display_mode{};
        if (SDL_GetDesktopDisplayMode(0, &display_mode) != 0) {
            set_error(error, error_capacity, std::string("no usable display is available: ") + SDL_GetError());
            return nullptr;
        }

        auto context = std::make_unique<Fn64Rt64Context>(width, height);
        context->update_vi();

        RT64::ApplicationConfiguration configuration;
        configuration.appId = "fn64";
        configuration.dataPath.clear();
        configuration.detectDataPath = false;
        configuration.useConfigurationFile = false;

        const RT64::Application::Core core = context->make_core();
        context->application = std::make_unique<RT64::Application>(core, configuration);
        context->application->userConfig.resolution = RT64::UserConfiguration::Resolution::Original;
        context->application->userConfig.resolutionMultiplier = 1.0;
        context->application->userConfig.developerMode = false;
        context->application->emulatorConfig.framebuffer.renderToRAM = true;

        const RT64::Application::SetupResult result = context->application->setup(0);
        if (result != RT64::Application::SetupResult::Success) {
            set_error(
                error,
                error_capacity,
                std::string("RT64 setup failed: ") + setup_result_name(result));
            return nullptr;
        }

        context->setup_complete = true;
        return context.release();
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 create threw: ") + exception.what());
        return nullptr;
    } catch (...) {
        set_error(error, error_capacity, "RT64 create failed with an unknown C++ exception");
        return nullptr;
    }
}

extern "C" int fn64_rt64_process_task(
    Fn64Rt64Context *context,
    uint8_t *rdram,
    size_t rdram_len,
    const Fn64Rt64Task *task,
    uint32_t output_addr,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if ((rdram == nullptr) || (task == nullptr)) {
            set_error(error, error_capacity, "null RDRAM or OSTask pointer");
            return 0;
        }
        if (rdram_len < N64_RDRAM_SIZE) {
            set_error(error, error_capacity, "RDRAM slice is smaller than the 8 MiB RT64 address space");
            return 0;
        }

        const uint32_t dl_address = task->data_ptr & 0x00FFFFFFU;
        const uint32_t ucode_address = task->ucode & 0x00FFFFFFU;
        const uint32_t ucode_data_address = task->ucode_data & 0x00FFFFFFU;
        if ((dl_address >= rdram_len) || (ucode_address >= rdram_len) ||
            (ucode_data_address >= rdram_len)) {
            set_error(error, error_capacity, "display-list or ucode address exceeds RDRAM");
            return 0;
        }

        context->active_rdram = rdram;
        context->active_rdram_len = rdram_len;
        context->output_addr = output_addr & 0x00FFFFFFU;
        context->update_vi();

        // Application::Core and State are RT64's public embedding state.
        // Update both aliases together before any interpreter or worker can
        // observe the fn64-owned allocation.
        context->application->core.RDRAM = rdram;
        context->application->state->RDRAM = rdram;
        context->application->interpreter->loadUCodeGBI(
            ucode_address,
            ucode_data_address,
            true);
        if (context->application->interpreter->hleGBI == nullptr) {
            set_error(error, error_capacity, "RT64 did not recognize the task's graphics microcode");
            return 0;
        }

        const uint64_t previous_workload = context->application->state->workloadId;
        context->application->processDisplayLists(rdram, dl_address, 0, true);
        const uint64_t submitted_workload = context->application->state->workloadId;
        if (submitted_workload > previous_workload) {
            // renderToRAM is enabled above. Waiting for this exact workload
            // closes the GPU/CPU interleaving before Rust or the VI capture
            // reads the fn64-owned RGBA5551 framebuffer.
            context->application->workloadQueue->waitForWorkloadId(submitted_workload);
        }

        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 task processing threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 task processing failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_present(
    Fn64Rt64Context *context,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }

        context->update_vi();
        const uint64_t previous_present = context->application->state->presentId;
        context->application->updateScreen();
        const uint64_t submitted_present = context->application->state->presentId;
        if (submitted_present > previous_present) {
            context->application->presentQueue->waitForPresentId(submitted_present);
        }
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 present threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 present failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" void fn64_rt64_resize(Fn64Rt64Context *context, uint32_t width, uint32_t height) {
    if ((context == nullptr) || (width == 0) || (height == 0)) {
        return;
    }
    context->width = width;
    context->height = height;
    context->update_vi();
}

extern "C" void fn64_rt64_destroy(Fn64Rt64Context *context) {
    try {
        delete context;
    } catch (...) {
        // C++ exceptions must never unwind across the C ABI. Destruction has
        // no recoverable Rust-side result channel and RT64's end path is
        // otherwise deterministic, so quarantine the exception here.
    }
}
