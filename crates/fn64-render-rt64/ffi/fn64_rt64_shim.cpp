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
#if defined(__APPLE__)
#include <SDL_syswm.h>
#include <Metal/Metal.hpp>
#include <pthread.h>
#endif

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
#if defined(__APPLE__)
    SDL_Window *host_window = nullptr;
    SDL_MetalView metal_view = nullptr;
    plume::RenderWindow render_window{};
#endif
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
        application.reset();
#if defined(__APPLE__)
        if (metal_view != nullptr) {
            SDL_Metal_DestroyView(metal_view);
        }
        if (host_window != nullptr) {
            SDL_DestroyWindow(host_window);
        }
#endif
    }

    RT64::Application::Core make_core() {
        RT64::Application::Core core{};
#if defined(__APPLE__)
        core.window = render_window;
#endif
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

#if defined(__APPLE__)
    bool create_hidden_metal_surface(char *error, size_t error_capacity) {
        // AppKit objects must be created and messaged on the process's main
        // thread. Returning before SDL/RT64 touches Cocoa makes a worker-thread
        // embedder a recoverable backend error instead of an Objective-C crash.
        if (pthread_main_np() == 0) {
            set_error(error, error_capacity, "RT64 Metal initialization must run on the macOS main thread");
            return false;
        }

        if (SDL_VideoInit(nullptr) != 0) {
            set_error(error, error_capacity, std::string("SDL video initialization failed: ") + SDL_GetError());
            return false;
        }

        // plume's MetalDevice constructor dereferences the system device
        // before its isValid() check. Probe it here so a headless/no-GPU host
        // returns through the C ABI rather than messaging a null MTL::Device.
        MTL::Device *device = MTL::CreateSystemDefaultDevice();
        if (device == nullptr) {
            set_error(error, error_capacity, "no Metal system-default device is available");
            return false;
        }
        device->release();

        host_window = SDL_CreateWindow(
            "fn64 RT64 hidden render surface",
            SDL_WINDOWPOS_UNDEFINED,
            SDL_WINDOWPOS_UNDEFINED,
            static_cast<int>(width),
            static_cast<int>(height),
            SDL_WINDOW_HIDDEN | SDL_WINDOW_METAL);
        if (host_window == nullptr) {
            set_error(error, error_capacity, std::string("hidden Metal surface creation failed: ") + SDL_GetError());
            return false;
        }

        SDL_SysWMinfo wm_info{};
        SDL_VERSION(&wm_info.version);
        if (SDL_GetWindowWMInfo(host_window, &wm_info) != SDL_TRUE) {
            set_error(error, error_capacity, std::string("Cocoa window lookup failed: ") + SDL_GetError());
            return false;
        }
        if (wm_info.info.cocoa.window == nullptr) {
            set_error(error, error_capacity, "SDL returned a null Cocoa NSWindow for the hidden Metal surface");
            return false;
        }

        metal_view = SDL_Metal_CreateView(host_window);
        if (metal_view == nullptr) {
            set_error(error, error_capacity, std::string("CAMetalLayer view creation failed: ") + SDL_GetError());
            return false;
        }
        void *metal_layer = SDL_Metal_GetLayer(metal_view);
        if (metal_layer == nullptr) {
            set_error(error, error_capacity, "SDL returned a null CAMetalLayer for the hidden Metal surface");
            return false;
        }

        // RT64's own macOS window path stores SDL_Window* in this field, but
        // plume::CocoaWindow bridges it to NSWindow* and sends contentView,
        // which is the exact objc_msgSend crash this shim must bypass. Supply
        // SDL's native NSWindow and CAMetalLayer handles instead.
        render_window.window = wm_info.info.cocoa.window;
        render_window.view = metal_layer;
        return true;
    }
#endif

    void update_vi(
        bool blanked = false,
        bool fade_enabled = false,
        uint16_t fade_factor = 0,
        bool repeat_line = false) {
        registers[9] = blanked ? 0U : VI_STATUS_16_BIT;
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
        // VI_Y_SCALE is `offset << 16 | scale`. A zero scale repeats one
        // sampled row; its 10-bit offset chooses the interpolation between
        // source rows zero and one. These are the hardware mechanisms behind
        // the public osViRepeatLine and osViFade scanout operations.
        if (fade_enabled) {
            registers[22] = static_cast<uint32_t>(fade_factor & 0x03FFU) << 16U;
        } else if (repeat_line) {
            registers[22] = 0U;
        } else {
            registers[22] = 0x400U;
        }
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

        auto context = std::make_unique<Fn64Rt64Context>(width, height);
#if defined(__APPLE__)
        if (!context->create_hidden_metal_surface(error, error_capacity)) {
            return nullptr;
        }
#else
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
#endif
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
    uint8_t *dmem,
    size_t dmem_len,
    uint8_t *imem,
    size_t imem_len,
    const Fn64Rt64Task *task,
    uint32_t output_addr,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if ((rdram == nullptr) || (dmem == nullptr) || (imem == nullptr) || (task == nullptr)) {
            set_error(error, error_capacity, "null RDRAM, RSP memory, or OSTask pointer");
            return 0;
        }
        if (rdram_len < N64_RDRAM_SIZE) {
            set_error(error, error_capacity, "RDRAM slice is smaller than the 8 MiB RT64 address space");
            return 0;
        }
        if ((dmem_len != context->dmem.size()) || (imem_len != context->imem.size())) {
            set_error(error, error_capacity, "RSP memory banks are not exactly 4 KiB");
            return 0;
        }

        std::memcpy(context->dmem.data(), dmem, context->dmem.size());
        std::memcpy(context->imem.data(), imem, context->imem.size());

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

        std::memcpy(dmem, context->dmem.data(), context->dmem.size());
        std::memcpy(imem, context->imem.data(), context->imem.size());

        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 task processing threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 task processing failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_process_rdp_commands(
    Fn64Rt64Context *context,
    uint8_t *rdram,
    size_t rdram_len,
    uint32_t start,
    uint32_t end,
    uint32_t output_addr,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if (rdram == nullptr) {
            set_error(error, error_capacity, "null RDRAM pointer");
            return 0;
        }
        if (rdram_len < N64_RDRAM_SIZE) {
            set_error(error, error_capacity, "RDRAM slice is smaller than the 8 MiB RT64 address space");
            return 0;
        }
        if ((start >= end) || ((start & 7U) != 0U) || ((end & 7U) != 0U) ||
            (end > rdram_len)) {
            set_error(error, error_capacity, "raw RDP range must be nonempty, 8-byte aligned, and inside RDRAM");
            return 0;
        }

        context->active_rdram = rdram;
        context->active_rdram_len = rdram_len;
        context->output_addr = output_addr & 0x00FFFFFFU;
        context->update_vi();
        context->registers[1] = start;
        context->registers[2] = end;
        context->registers[3] = start;

        // RT64's public embedding entry accepts an explicit bounded LLE RDP
        // range when isHLE is false. Keep both public RDRAM aliases coherent
        // exactly as the task path does before the interpreter observes it.
        context->application->core.RDRAM = rdram;
        context->application->state->RDRAM = rdram;
        const uint64_t previous_workload = context->application->state->workloadId;
        context->application->processDisplayLists(rdram, start, end, false);
        context->registers[3] = end;
        const uint64_t submitted_workload = context->application->state->workloadId;
        if (submitted_workload > previous_workload) {
            context->application->workloadQueue->waitForWorkloadId(submitted_workload);
        }

        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 raw RDP processing threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 raw RDP processing failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_present(
    Fn64Rt64Context *context,
    uint8_t blanked,
    uint8_t fade_enabled,
    uint16_t fade_factor,
    uint8_t repeat_line,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }

        if ((fade_enabled != 0U) && (repeat_line != 0U)) {
            set_error(error, error_capacity, "VI fade and repeat-line cannot be enabled together");
            return 0;
        }
        if (fade_factor > 0x03FFU) {
            set_error(error, error_capacity, "VI fade factor exceeds 10 bits");
            return 0;
        }

        context->update_vi(
            blanked != 0U,
            fade_enabled != 0U,
            fade_factor,
            repeat_line != 0U);
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
