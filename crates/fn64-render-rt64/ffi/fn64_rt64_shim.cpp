#include "fn64_rt64_shim.h"

#include <algorithm>
#include <array>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <exception>
#include <initializer_list>
#include <limits>
#include <stdexcept>
#include <memory>
#include <mutex>
#include <new>
#include <string>
#include <thread>
#include <vector>

#include "fn64_rt64_video_interface.h"

#include <SDL.h>
#include "contrib/plume/plume_vulkan.h"
#include "rhi/rt64_render_hooks.h"
#if defined(__APPLE__)
#include <SDL_syswm.h>
#include <Metal/Metal.hpp>
#include <objc/runtime.h>
#include <pthread.h>
#include "contrib/plume/plume_metal.h"
#elif defined(_WIN32)
#include "contrib/plume/plume_d3d12.h"
#endif

#include "hle/rt64_application.h"
#include "hle/rt64_interpreter.h"
#include "hle/rt64_present_queue.h"
#include "hle/rt64_vi.h"
#include "hle/rt64_workload_queue.h"
#if defined(FN64_RT64_SYNTHETIC_F3DEX2_EVIDENCE)
#include "gbi/rt64_gbi_f3dex2.h"
#include "gbi/rt64_gbi_rdp.h"
#endif
#if defined(FN64_RT64_SYNTHETIC_S2DEX_EVIDENCE)
#include "gbi/rt64_gbi_rdp.h"
#include "gbi/rt64_gbi_s2dex.h"
#include "gbi/rt64_gbi_s2dex2.h"
#endif
#include "common/rt64_filesystem_directory.h"
#include "common/rt64_filesystem_zip.h"
#include "common/rt64_tmem_hasher.h"
#include "render/rt64_render_target.h"
#include "render/rt64_shader_library.h"
#include "render/rt64_texture_cache.h"

namespace {
constexpr size_t N64_RDRAM_SIZE = 8U * 1024U * 1024U;
constexpr uint32_t VI_STATUS_16_BIT = 2U;
static_assert(sizeof(Fn64Rt64UcodeGeneration) == 120U);
static_assert(sizeof(Fn64Rt64TaskResult) == 88U);
static_assert(sizeof(Fn64Rt64ViState) == 72U);
static_assert(sizeof(interop::VideoInterfaceCB) == 64U);
static_assert(sizeof(Fn64Rt64ExtendedPresentCapture) == 72U);
static_assert(sizeof(Fn64Rt64PresentCapture) == 48U);
#if defined(FN64_RT64_HFR_EVIDENCE)
static_assert(sizeof(Fn64Rt64HfrPacingSample) == 48U);
static_assert(sizeof(Fn64Rt64HfrPacingEvidence) == 3080U);
#endif

void set_error(char *error, size_t capacity, const std::string &message);

#if defined(__APPLE__)
using MetalNewRenderPipelineState = void *(*)(void *, SEL, void *, void **);

struct MetalPipelineConstructionProbe {
    std::mutex control_mutex;
    Class device_class = nullptr;
    SEL selector = nullptr;
    IMP original_implementation = nullptr;
    std::thread::id caller_thread;
    std::thread::id workload_thread;
    std::thread::id present_thread;
    std::atomic<bool> active{false};
    std::atomic<bool> caller_scope{false};
    std::atomic<uint64_t> caller_events{0};
    std::atomic<uint64_t> workload_events{0};
    std::atomic<uint64_t> present_events{0};
    std::atomic<uint64_t> background_events{0};
};

MetalPipelineConstructionProbe metal_pipeline_probe;

void *metal_new_render_pipeline_state_probe(
    void *receiver,
    SEL selector,
    void *descriptor,
    void **error) {
    MetalPipelineConstructionProbe &probe = metal_pipeline_probe;
    if (probe.active.load(std::memory_order_acquire)) {
        const std::thread::id current = std::this_thread::get_id();
        if (probe.caller_scope.load(std::memory_order_relaxed) &&
            (current == probe.caller_thread)) {
            probe.caller_events.fetch_add(1U, std::memory_order_relaxed);
        }
        else if (current == probe.workload_thread) {
            probe.workload_events.fetch_add(1U, std::memory_order_relaxed);
        }
        else if (current == probe.present_thread) {
            probe.present_events.fetch_add(1U, std::memory_order_relaxed);
        }
        else {
            probe.background_events.fetch_add(1U, std::memory_order_relaxed);
        }
    }

    auto original = reinterpret_cast<MetalNewRenderPipelineState>(
        probe.original_implementation);
    return original(receiver, selector, descriptor, error);
}

bool install_metal_pipeline_probe(
    plume::MetalDevice *device,
    char *error,
    size_t error_capacity) {
    if ((device == nullptr) || (device->mtl == nullptr)) {
        set_error(error, error_capacity, "RT64 ubershader evidence requires a Metal device");
        return false;
    }

    MetalPipelineConstructionProbe &probe = metal_pipeline_probe;
    Class device_class = object_getClass(reinterpret_cast<id>(device->mtl));
    SEL selector = sel_registerName("newRenderPipelineStateWithDescriptor:error:");
    Method method = class_getInstanceMethod(device_class, selector);
    if (method == nullptr) {
        set_error(error, error_capacity, "Metal device does not expose synchronous render-pipeline construction");
        return false;
    }

    const IMP current = method_getImplementation(method);
    const IMP hook = reinterpret_cast<IMP>(metal_new_render_pipeline_state_probe);
    if (probe.original_implementation == nullptr) {
        probe.device_class = device_class;
        probe.selector = selector;
        probe.original_implementation = current;
        class_replaceMethod(device_class, selector, hook, method_getTypeEncoding(method));
    }
    else if ((probe.device_class != device_class) ||
             (probe.selector != selector) ||
             (current != hook)) {
        set_error(error, error_capacity, "Metal render-pipeline construction hook ownership changed");
        return false;
    }

    return true;
}

struct MetalPipelineCriticalScope {
    bool active = false;

    explicit MetalPipelineCriticalScope(bool enabled) : active(enabled) {
        if (active && metal_pipeline_probe.caller_scope.exchange(
                          true,
                          std::memory_order_acq_rel)) {
            std::fputs("nested RT64 Metal pipeline critical scope\n", stderr);
            std::terminate();
        }
    }

    ~MetalPipelineCriticalScope() {
        if (active) {
            metal_pipeline_probe.caller_scope.store(false, std::memory_order_release);
        }
    }
};
#endif

struct PresentDiagnosticSnapshot {
    uint64_t workload_before = 0;
    uint64_t workload_after = 0;
    uint64_t present_before = 0;
    uint64_t present_after = 0;
    uint64_t capture_before = 0;
    uint64_t capture_after = 0;
};

bool present_diagnostics_enabled() {
    static const bool enabled = []() {
        const char *value = std::getenv("FN64_RT64_PRESENT_DIAGNOSTICS");
        return (value != nullptr) && (std::strcmp(value, "1") == 0);
    }();
    return enabled;
}

void print_present_diagnostics(const PresentDiagnosticSnapshot &snapshot) {
    std::fprintf(
        stderr,
        "fn64 RT64 present diagnostic: workload=%llu->%llu present=%llu->%llu capture=%llu->%llu\n",
        static_cast<unsigned long long>(snapshot.workload_before),
        static_cast<unsigned long long>(snapshot.workload_after),
        static_cast<unsigned long long>(snapshot.present_before),
        static_cast<unsigned long long>(snapshot.present_after),
        static_cast<unsigned long long>(snapshot.capture_before),
        static_cast<unsigned long long>(snapshot.capture_after));
}

void ignore_interrupts() {}

void set_error(char *error, size_t capacity, const std::string &message) {
    if ((error == nullptr) || (capacity == 0)) {
        return;
    }

    const size_t count = std::min(capacity - 1, message.size());
    std::memcpy(error, message.data(), count);
    error[count] = '\0';
}

template <typename Enum>
bool decode_enum(uint32_t raw, uint32_t count, const char *name, Enum &decoded,
                 char *error, size_t error_capacity) {
    if (raw >= count) {
        set_error(error, error_capacity, std::string("invalid RT64 user-config ") + name + " tag " + std::to_string(raw));
        return false;
    }
    decoded = static_cast<Enum>(raw);
    return true;
}

bool decode_bool(uint32_t raw, const char *name, bool &decoded,
                 char *error, size_t error_capacity) {
    if (raw > 1U) {
        set_error(error, error_capacity, std::string("invalid RT64 user-config ") + name + " boolean " + std::to_string(raw));
        return false;
    }
    decoded = raw != 0U;
    return true;
}

bool bounded_double(double value, double minimum, double maximum, const char *name,
                    char *error, size_t error_capacity) {
    if (!std::isfinite(value)) {
        set_error(error, error_capacity, std::string("RT64 user-config ") + name + " must be finite");
        return false;
    }
    if ((value < minimum) || (value > maximum)) {
        set_error(error, error_capacity, std::string("RT64 user-config ") + name + " is out of range");
        return false;
    }
    return true;
}

bool decode_user_config(const Fn64Rt64UserConfig *raw, RT64::UserConfiguration &config,
                        char *error, size_t error_capacity) {
    if (raw == nullptr) {
        set_error(error, error_capacity, "null RT64 user-config pointer");
        return false;
    }
    using User = RT64::UserConfiguration;
    if (!decode_enum(raw->graphics_api, static_cast<uint32_t>(User::GraphicsAPI::OptionCount), "graphics_api", config.graphicsAPI, error, error_capacity) ||
        !decode_enum(raw->resolution, static_cast<uint32_t>(User::Resolution::OptionCount), "resolution", config.resolution, error, error_capacity) ||
        !decode_enum(raw->display_buffering, static_cast<uint32_t>(User::DisplayBuffering::OptionCount), "display_buffering", config.displayBuffering, error, error_capacity) ||
        !decode_enum(raw->antialiasing, static_cast<uint32_t>(User::Antialiasing::OptionCount), "antialiasing", config.antialiasing, error, error_capacity) ||
        !decode_enum(raw->filtering, static_cast<uint32_t>(User::Filtering::OptionCount), "filtering", config.filtering, error, error_capacity) ||
        !decode_enum(raw->aspect_ratio, static_cast<uint32_t>(User::AspectRatio::OptionCount), "aspect_ratio", config.aspectRatio, error, error_capacity) ||
        !decode_enum(raw->extended_aspect_ratio, static_cast<uint32_t>(User::AspectRatio::OptionCount), "extended_aspect_ratio", config.extAspectRatio, error, error_capacity) ||
        !decode_enum(raw->upscale_2d, static_cast<uint32_t>(User::Upscale2D::OptionCount), "upscale_2d", config.upscale2D, error, error_capacity) ||
        !decode_enum(raw->refresh_rate, static_cast<uint32_t>(User::RefreshRate::OptionCount), "refresh_rate", config.refreshRate, error, error_capacity) ||
        !decode_enum(raw->internal_color_format, static_cast<uint32_t>(User::InternalColorFormat::OptionCount), "internal_color_format", config.internalColorFormat, error, error_capacity) ||
        !decode_enum(raw->hardware_resolve, static_cast<uint32_t>(User::HardwareResolve::OptionCount), "hardware_resolve", config.hardwareResolve, error, error_capacity) ||
        !decode_bool(raw->three_point_filtering, "three_point_filtering", config.threePointFiltering, error, error_capacity) ||
        !decode_bool(raw->idle_work_active, "idle_work_active", config.idleWorkActive, error, error_capacity) ||
        !decode_bool(raw->developer_mode, "developer_mode", config.developerMode, error, error_capacity)) {
        return false;
    }
    if (!bounded_double(raw->resolution_multiplier, 0.0, 32.0, "resolution_multiplier", error, error_capacity) ||
        !bounded_double(raw->aspect_target, 0.1, 100.0, "aspect_target", error, error_capacity) ||
        !bounded_double(raw->extended_aspect_target, 0.1, 100.0, "extended_aspect_target", error, error_capacity) ||
        (raw->downsample_multiplier < 1U) || (raw->downsample_multiplier > 32U) ||
        (raw->refresh_rate_target < 10U) || (raw->refresh_rate_target > 1000U)) {
        if ((raw->downsample_multiplier < 1U) || (raw->downsample_multiplier > 32U)) {
            set_error(error, error_capacity, "RT64 user-config downsample_multiplier is out of range");
        }
        else if ((raw->refresh_rate_target < 10U) || (raw->refresh_rate_target > 1000U)) {
            set_error(error, error_capacity, "RT64 user-config refresh_rate_target is out of range");
        }
        return false;
    }
    config.resolutionMultiplier = raw->resolution_multiplier == 0.0 ? 0.0 : raw->resolution_multiplier;
    config.downsampleMultiplier = static_cast<int>(raw->downsample_multiplier);
    config.aspectTarget = raw->aspect_target;
    config.extAspectTarget = raw->extended_aspect_target;
    config.refreshRateTarget = static_cast<int>(raw->refresh_rate_target);
    return true;
}

Fn64Rt64UserConfig encode_user_config(const RT64::UserConfiguration &config) {
    return Fn64Rt64UserConfig{
        static_cast<uint32_t>(config.graphicsAPI),
        static_cast<uint32_t>(config.resolution),
        static_cast<uint32_t>(config.displayBuffering),
        static_cast<uint32_t>(config.antialiasing),
        config.resolutionMultiplier,
        static_cast<uint32_t>(config.downsampleMultiplier),
        static_cast<uint32_t>(config.filtering),
        static_cast<uint32_t>(config.aspectRatio),
        config.aspectTarget,
        static_cast<uint32_t>(config.extAspectRatio),
        config.extAspectTarget,
        static_cast<uint32_t>(config.upscale2D),
        static_cast<uint32_t>(config.threePointFiltering),
        static_cast<uint32_t>(config.refreshRate),
        static_cast<uint32_t>(config.refreshRateTarget),
        static_cast<uint32_t>(config.internalColorFormat),
        static_cast<uint32_t>(config.hardwareResolve),
        static_cast<uint32_t>(config.idleWorkActive),
        static_cast<uint32_t>(config.developerMode)};
}

bool decode_policy_bool(uint32_t raw, const char *family, const char *name, bool &decoded,
                        char *error, size_t error_capacity) {
    if (raw > 1U) {
        set_error(error, error_capacity, std::string("invalid RT64 ") + family + " " + name + " boolean " + std::to_string(raw));
        return false;
    }
    decoded = raw != 0U;
    return true;
}

bool decode_enhancement_config(const Fn64Rt64EnhancementConfig *raw,
                               RT64::EnhancementConfiguration &config,
                               char *error, size_t error_capacity) {
    if (raw == nullptr) {
        set_error(error, error_capacity, "null RT64 enhancement-config pointer");
        return false;
    }
    if (raw->presentation_mode > 2U) {
        set_error(error, error_capacity, std::string("invalid RT64 enhancement-config presentation_mode tag ") + std::to_string(raw->presentation_mode));
        return false;
    }
    config.presentation.mode = static_cast<RT64::EnhancementConfiguration::Presentation::Mode>(raw->presentation_mode);
    return decode_policy_bool(raw->framebuffer_reinterpret_fix_uls, "enhancement-config", "framebuffer_reinterpret_fix_uls", config.framebuffer.reinterpretFixULS, error, error_capacity) &&
           decode_policy_bool(raw->remove_black_borders, "enhancement-config", "remove_black_borders", config.presentation.removeBlackBorders, error, error_capacity) &&
           decode_policy_bool(raw->rect_fix_lower_right, "enhancement-config", "rect_fix_lower_right", config.rect.fixRectLR, error, error_capacity) &&
           decode_policy_bool(raw->f3dex_force_branch, "enhancement-config", "f3dex_force_branch", config.f3dex.forceBranch, error, error_capacity) &&
           decode_policy_bool(raw->s2dex_fix_bilerp_mismatch, "enhancement-config", "s2dex_fix_bilerp_mismatch", config.s2dex.fixBilerpMismatch, error, error_capacity) &&
           decode_policy_bool(raw->s2dex_framebuffer_fast_path, "enhancement-config", "s2dex_framebuffer_fast_path", config.s2dex.framebufferFastPath, error, error_capacity) &&
           decode_policy_bool(raw->texture_lod_scale, "enhancement-config", "texture_lod_scale", config.textureLOD.scale, error, error_capacity);
}

Fn64Rt64EnhancementConfig encode_enhancement_config(const RT64::EnhancementConfiguration &config) {
    return Fn64Rt64EnhancementConfig{
        static_cast<uint32_t>(config.framebuffer.reinterpretFixULS),
        static_cast<uint32_t>(config.presentation.mode),
        static_cast<uint32_t>(config.presentation.removeBlackBorders),
        static_cast<uint32_t>(config.rect.fixRectLR),
        static_cast<uint32_t>(config.f3dex.forceBranch),
        static_cast<uint32_t>(config.s2dex.fixBilerpMismatch),
        static_cast<uint32_t>(config.s2dex.framebufferFastPath),
        static_cast<uint32_t>(config.textureLOD.scale)};
}

bool decode_emulator_config(const Fn64Rt64EmulatorConfig *raw,
                            RT64::EmulatorConfiguration &config,
                            char *error, size_t error_capacity) {
    if (raw == nullptr) {
        set_error(error, error_capacity, "null RT64 emulator-config pointer");
        return false;
    }
    return decode_policy_bool(raw->post_blend_noise, "emulator-config", "post_blend_noise", config.dither.postBlendNoise, error, error_capacity) &&
           decode_policy_bool(raw->post_blend_noise_negative, "emulator-config", "post_blend_noise_negative", config.dither.postBlendNoiseNegative, error, error_capacity) &&
           decode_policy_bool(raw->framebuffer_render_to_ram, "emulator-config", "framebuffer_render_to_ram", config.framebuffer.renderToRAM, error, error_capacity) &&
           decode_policy_bool(raw->framebuffer_copy_with_gpu, "emulator-config", "framebuffer_copy_with_gpu", config.framebuffer.copyWithGPU, error, error_capacity);
}

Fn64Rt64EmulatorConfig encode_emulator_config(const RT64::EmulatorConfiguration &config) {
    return Fn64Rt64EmulatorConfig{
        static_cast<uint32_t>(config.dither.postBlendNoise),
        static_cast<uint32_t>(config.dither.postBlendNoiseNegative),
        static_cast<uint32_t>(config.framebuffer.renderToRAM),
        static_cast<uint32_t>(config.framebuffer.copyWithGPU)};
}

Fn64Rt64ReplacementDatabaseConfig encode_replacement_config(
    const RT64::ReplacementConfiguration &config) {
    return Fn64Rt64ReplacementDatabaseConfig{
        static_cast<uint32_t>(config.autoPath),
        static_cast<uint32_t>(config.defaultOperation),
        static_cast<uint32_t>(config.defaultShift),
        config.configurationVersion,
        config.hashVersion};
}

bool replacement_configs_equal(const Fn64Rt64ReplacementDatabaseConfig &left,
                               const Fn64Rt64ReplacementDatabaseConfig &right) {
    return (left.auto_path == right.auto_path) &&
           (left.default_operation == right.default_operation) &&
           (left.default_shift == right.default_shift) &&
           (left.configuration_version == right.configuration_version) &&
           (left.hash_version == right.hash_version);
}

bool inspect_replacement_pack(const char *path_utf8,
                              Fn64Rt64ReplacementDatabaseConfig &config,
                              std::vector<uint8_t> &database_bytes,
                              char *error, size_t error_capacity) {
    if ((path_utf8 == nullptr) || (path_utf8[0] == '\0')) {
        set_error(error, error_capacity, "replacement-pack path is null or empty");
        return false;
    }
    const std::filesystem::path path = std::filesystem::u8path(path_utf8);
    std::error_code ec;
    const std::filesystem::file_status status = std::filesystem::symlink_status(path, ec);
    if (ec) {
        set_error(error, error_capacity, std::string("replacement-pack status failed: ") + ec.message());
        return false;
    }
    if (std::filesystem::is_symlink(status)) {
        set_error(error, error_capacity, "replacement-pack root may not be a symbolic link");
        return false;
    }

    std::unique_ptr<RT64::FileSystem> file_system;
    if (std::filesystem::is_directory(status)) {
        file_system = RT64::FileSystemDirectory::create(path);
    }
    else if (std::filesystem::is_regular_file(status)) {
        if (RT64::ReplacementDatabase::toLower(path.extension().u8string()) != RT64::ReplacementPackExtension) {
            set_error(error, error_capacity, "replacement-pack file must have the .rtz extension");
            return false;
        }
        file_system = RT64::FileSystemZip::create(path, "");
    }
    else {
        set_error(error, error_capacity, "replacement-pack path is neither one directory nor one regular .rtz file");
        return false;
    }
    if (file_system == nullptr) {
        set_error(error, error_capacity, "replacement-pack filesystem could not be opened");
        return false;
    }
    if (!file_system->load(RT64::ReplacementDatabaseFilename, database_bytes)) {
        set_error(error, error_capacity, "replacement pack has no readable non-empty root rt64.json");
        return false;
    }

    try {
        const nlohmann::json root = nlohmann::json::parse(
            database_bytes.begin(), database_bytes.end(), nullptr, true);
        if (root.contains("configuration")) {
            const nlohmann::json &raw_config = root.at("configuration");
            if (!raw_config.is_object()) {
                set_error(error, error_capacity, "replacement database configuration must be an object");
                return false;
            }
            auto reject_unknown_string = [&](const char *field,
                                             std::initializer_list<const char *> allowed) {
                if (!raw_config.contains(field)) {
                    return false;
                }
                if (!raw_config.at(field).is_string()) {
                    set_error(error, error_capacity, std::string("replacement database ") + field + " must be a string");
                    return true;
                }
                const std::string value = raw_config.at(field).get<std::string>();
                for (const char *candidate : allowed) {
                    if (value == candidate) {
                        return false;
                    }
                }
                set_error(error, error_capacity, std::string("replacement database has unknown ") + field + " value " + value);
                return true;
            };
            if (reject_unknown_string("autoPath", {"rt64", "rice"}) ||
                reject_unknown_string("defaultOperation", {"preload", "stream", "stall"}) ||
                reject_unknown_string("defaultShift", {"none", "half"})) {
                return false;
            }
        }
        RT64::ReplacementDatabase database = root;
        if (database.config.hashVersion > RT64::TMEMHasher::CurrentHashVersion) {
            set_error(error, error_capacity, "replacement database hashVersion is newer than pinned RT64");
            return false;
        }
        if (database.config.defaultOperation == RT64::ReplacementOperation::Auto) {
            set_error(error, error_capacity, "replacement database defaultOperation may not be auto");
            return false;
        }
        if (database.config.defaultShift == RT64::ReplacementShift::Auto) {
            set_error(error, error_capacity, "replacement database defaultShift may not be auto");
            return false;
        }
        config = encode_replacement_config(database.config);
        return true;
    }
    catch (const nlohmann::detail::exception &exception) {
        set_error(error, error_capacity, std::string("replacement database JSON is invalid: ") + exception.what());
        return false;
    }
}

bool decode_replacement_packs(const Fn64Rt64ReplacementPack *packs, size_t pack_count,
                              std::vector<RT64::ReplacementDirectory> &directories,
                              char *error, size_t error_capacity) {
    if ((pack_count > 0) && (packs == nullptr)) {
        set_error(error, error_capacity, "non-zero replacement-pack count has a null array");
        return false;
    }
    directories.reserve(pack_count);
    for (size_t i = 0; i < pack_count; i++) {
        Fn64Rt64ReplacementDatabaseConfig actual{};
        std::vector<uint8_t> ignored_database_bytes;
        if (!inspect_replacement_pack(packs[i].path_utf8, actual, ignored_database_bytes,
                                      error, error_capacity)) {
            return false;
        }
        if (!replacement_configs_equal(actual, packs[i].expected_database)) {
            set_error(error, error_capacity, "replacement database changed after host inspection");
            return false;
        }
        directories.emplace_back(std::filesystem::u8path(packs[i].path_utf8));
    }
    return true;
}

bool restart_fields_equal(const RT64::UserConfiguration &left, const RT64::UserConfiguration &right) {
    return (left.graphicsAPI == right.graphicsAPI) &&
           (left.displayBuffering == right.displayBuffering) &&
           (left.internalColorFormat == right.internalColorFormat);
}

bool resolution_change_discards_framebuffers(const RT64::UserConfiguration &next,
                                             const RT64::UserConfiguration &active) {
    using Resolution = RT64::UserConfiguration::Resolution;
    using Aspect = RT64::UserConfiguration::AspectRatio;
    return (next.resolution != active.resolution) ||
           ((next.resolution == Resolution::Manual) &&
            (next.resolutionMultiplier != active.resolutionMultiplier)) ||
           (next.aspectRatio != active.aspectRatio) ||
           ((next.aspectRatio == Aspect::Manual) &&
            (next.aspectTarget != active.aspectTarget));
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

void write_vi_registers(
    std::array<uint32_t, 24> &registers,
    uint32_t output_addr,
    uint32_t width,
    uint32_t height,
    const Fn64Rt64ViState &vi) {
    if ((vi.registers_present > 1U) || (vi.blanked > 1U) ||
        (vi.fade_enabled > 1U) || (vi.repeat_line > 1U) ||
        (vi.aa_mode_specified > 1U) ||
        (vi.reserved != 0U)) {
        throw std::runtime_error("VI state contains invalid boolean or reserved fields");
    }
    if ((vi.registers_present != 0U) && (vi.aa_mode_specified == 0U)) {
        throw std::runtime_error("VI register image requires an explicit AA selector marker");
    }
    if ((vi.fade_enabled != 0U) && (vi.repeat_line != 0U)) {
        throw std::runtime_error("VI fade and repeat-line cannot be enabled together");
    }
    if (vi.fade_factor > 0x03FFU) {
        throw std::runtime_error("VI fade factor exceeds 10 bits");
    }

    std::array<uint32_t, 14> guest{};
    if (vi.registers_present != 0U) {
        std::copy(std::begin(vi.registers), std::end(vi.registers), guest.begin());
        const uint32_t h_start = (guest[9] >> 16U) & 0x03FFU;
        const uint32_t h_end = guest[9] & 0x03FFU;
        const uint32_t v_start = (guest[10] >> 16U) & 0x03FFU;
        const uint32_t v_end = guest[10] & 0x03FFU;
        // A window is active only when BOTH axes are programmed, matching
        // `ViActiveWindow::try_from_registers` (crates/fn64-render/src/lib.rs),
        // which returns None -- "nothing to scan out yet" -- unless H_VIDEO and
        // V_VIDEO are each nonzero. This predicate used OR where that Rust
        // contract uses AND, and the two disagree on exactly the state a real
        // guest passes through during boot: WM2000's first VI retrace has
        // V_VIDEO programmed (v=[37,511]) and H_VIDEO still zero, which OR
        // called "active" and then rejected for h_end <= h_start. That aborted
        // the process on a frame the Rust side correctly treats as not-yet-
        // programmed and skips.
        //
        // This only narrows what counts as active; it does not weaken any
        // malformed-window check. A window with both axes written still gets
        // the full width/ordering/parity validation below, which is what
        // `cpp_vi_ingress_rejects_an_odd_half_line_extent` pins.
        const bool active_window =
            (((h_start | h_end) != 0U) && ((v_start | v_end) != 0U));
        if (active_window &&
            (((guest[2] & 0x0FFFU) == 0U) || (h_end <= h_start) ||
             (v_end <= v_start) || (((v_end - v_start) & 1U) != 0U))) {
            throw std::runtime_error("VI register image has invalid width or active window");
        }
    } else {
        const uint64_t vertical_end = 34ULL + static_cast<uint64_t>(height) * 2ULL;
        if ((width == 0U) || (height == 0U) || (vertical_end > 0x03FFU)) {
            throw std::runtime_error("compatibility VI geometry exceeds public register fields");
        }
        guest[0] = vi.registers[0];
        guest[1] = output_addr & 0x00FFFFFFU;
        guest[2] = width;
        guest[6] = 525U;
        guest[7] = 3093U;
        guest[9] = (108U << 16U) | 748U;
        guest[10] = (34U << 16U) | static_cast<uint32_t>(vertical_end);
        guest[12] = 0x400U;
        guest[13] = 0x400U;
    }

    for (size_t index = 0; index < guest.size(); ++index) {
        registers[9 + index] = guest[index];
    }
    registers[9] = (vi.blanked != 0U) ? 0U : guest[0];
    // RT64 compensates for the VI origin convention by subtracting one row,
    // or two for an odd serrated field. Supplying the same row count makes
    // decodeVI().fbAddress() equal the guest's exact physical VI_ORIGIN. The
    // source stride and pixel width come from the same retained register image.
    const uint32_t bytes_per_pixel = ((guest[0] & 3U) == 3U) ? 4U : 2U;
    const uint32_t effective_width = guest[2] & 0x0FFFU;
    const uint32_t origin_rows =
        (((guest[0] & (1U << 6U)) != 0U) && ((guest[4] & 1U) != 0U)) ? 2U : 1U;
    const uint64_t adjusted_origin =
        static_cast<uint64_t>(guest[1] & 0x00FFFFFFU) +
        static_cast<uint64_t>(effective_width) * bytes_per_pixel * origin_rows;
    if (adjusted_origin > UINT32_MAX) {
        throw std::runtime_error("VI origin compensation overflows u32");
    }
    registers[10] = static_cast<uint32_t>(adjusted_origin);
    // VI_Y_SCALE is `offset << 16 | scale`. A zero scale repeats one
    // sampled row; its 10-bit offset chooses the interpolation between
    // source rows zero and one. These are the hardware mechanisms behind
    // the public osViRepeatLine and osViFade scanout operations.
    if (vi.fade_enabled != 0U) {
        registers[22] = static_cast<uint32_t>(vi.fade_factor & 0x03FFU) << 16U;
    } else if (vi.repeat_line != 0U) {
        registers[22] = 0U;
    }
}

void digest_u64(uint64_t &digest, uint64_t value) {
    constexpr uint64_t FNV_PRIME = 1099511628211ULL;
    for (int shift = 56; shift >= 0; shift -= 8) {
        digest ^= (value >> shift) & 0xFFU;
        digest *= FNV_PRIME;
    }
}

struct ExtendedDispatchProbe {
    Fn64Rt64ExtendedGbiEvidence evidence{};
    RT64::GBIFunction original_hook = nullptr;
    RT64::GBIFunction original_extended = nullptr;
    uint32_t rect_alignment_count = 0;
    uint32_t rect_alignment_call_index = 0;
    int32_t left_offset = 0;
    int32_t top_offset = 0;
    int32_t right_offset = 0;
    int32_t bottom_offset = 0;
    std::array<uint32_t, FN64_RT64_EXTENDED_MAX_VERTEX_Z_MARKERS> vertex_command_indices{};
    uint32_t vertex_command_count = 0;
    std::string invalid;
};

thread_local ExtendedDispatchProbe *active_extended_probe = nullptr;

void invalidate_extended_probe(ExtendedDispatchProbe &probe, const char *message) {
    if (probe.invalid.empty()) {
        probe.invalid = message;
    }
}

void observed_extended_hook(RT64::State *state, RT64::DisplayList **dl) {
    ExtendedDispatchProbe *probe = active_extended_probe;
    if ((probe == nullptr) || (probe->original_hook == nullptr)) {
        std::terminate();
    }
    const uint32_t magic = (*dl)->p0(0, 24);
    const uint32_t operation = (*dl)->p1(28, 4);
    if ((magic == 0x525464U) && (operation == 1U)) {
        if (probe->evidence.hook_enable_count == std::numeric_limits<uint32_t>::max()) {
            invalidate_extended_probe(*probe, "RT64 Extended-GBI hook-enable count overflowed");
        }
        else {
            probe->evidence.hook_enable_count++;
        }
        probe->evidence.enabled_opcode = static_cast<uint8_t>((*dl)->p1(0, 8));
    }
    probe->original_hook(state, dl);
}

void observed_extended_command(RT64::State *state, RT64::DisplayList **dl) {
    ExtendedDispatchProbe *probe = active_extended_probe;
    if ((probe == nullptr) || (probe->original_extended == nullptr)) {
        std::terminate();
    }
    const uint32_t command = (*dl)->p0(0, 24);
    if (command >= FN64_RT64_EXTENDED_COMMAND_COUNT) {
        invalidate_extended_probe(*probe, "RT64 dispatched an out-of-range Extended-GBI command");
    }
    else if (probe->evidence.command_counts[command] == std::numeric_limits<uint32_t>::max()) {
        invalidate_extended_probe(*probe, "RT64 Extended-GBI command count overflowed");
    }
    else {
        probe->evidence.command_counts[command]++;
    }

    switch (command) {
    case 0x06: {
        if (probe->rect_alignment_count == std::numeric_limits<uint32_t>::max()) {
            invalidate_extended_probe(*probe,
                                      "RT64 rectangle-alignment evidence count overflowed");
        }
        else {
            probe->rect_alignment_count++;
        }
        const uint32_t workload_slot = state->ext.workloadQueue->writeCursor;
        probe->rect_alignment_call_index =
            state->ext.workloadQueue->workloads[workload_slot].gameCallCount;
        RT64::DisplayList *parameters = *dl + 1;
        probe->left_offset = static_cast<int16_t>(parameters->p0(16, 16));
        probe->top_offset = static_cast<int16_t>(parameters->p0(0, 16));
        probe->right_offset = static_cast<int16_t>(parameters->p1(16, 16));
        probe->bottom_offset = static_cast<int16_t>(parameters->p1(0, 16));
        break;
    }
    case 0x09:
        probe->evidence.has_refresh_rate = 1;
        probe->evidence.refresh_rate = static_cast<uint16_t>((*dl)->p1(0, 16));
        break;
    case 0x0A:
        if (probe->vertex_command_count >= probe->vertex_command_indices.size()) {
            invalidate_extended_probe(*probe, "RT64 Extended-GBI vertex-Z commands exceed evidence capacity");
        }
        else {
            probe->vertex_command_indices[probe->vertex_command_count++] = (*dl)->p1(0, 8);
        }
        break;
    case 0x0C: {
        const uint32_t index = probe->evidence.group_count;
        if (index >= FN64_RT64_EXTENDED_MAX_GROUPS) {
            invalidate_extended_probe(*probe, "RT64 Extended-GBI matrix groups exceed evidence capacity");
            break;
        }
        RT64::DisplayList *selectors = *dl + 1;
        Fn64Rt64TransformGroupEvidence &group = probe->evidence.groups[index];
        group.group_id = (*dl)->w1;
        group.push = static_cast<uint8_t>(selectors->p0(0, 1));
        group.projection = static_cast<uint8_t>(selectors->p0(1, 1));
        group.decompose = static_cast<uint8_t>(selectors->p0(2, 1));
        group.position_selector = static_cast<uint8_t>(selectors->p0(3, 2));
        group.rotation_selector = static_cast<uint8_t>(selectors->p0(5, 2));
        group.scale_selector = static_cast<uint8_t>(selectors->p0(7, 2));
        group.skew_selector = static_cast<uint8_t>(selectors->p0(9, 2));
        group.perspective_selector = static_cast<uint8_t>(selectors->p0(11, 2));
        group.vertex_selector = static_cast<uint8_t>(selectors->p0(13, 2));
        group.tile_selector = static_cast<uint8_t>(selectors->p0(15, 2));
        group.ordering = static_cast<uint8_t>(selectors->p0(17, 2));
        group.editable = static_cast<uint8_t>(selectors->p0(19, 1));
        group.aspect_mode = static_cast<uint8_t>(selectors->p0(20, 2));
        group.texcoord_selector = static_cast<uint8_t>(selectors->p0(22, 2));
        group.look_at_selector = static_cast<uint8_t>(selectors->p0(24, 2));
        probe->evidence.group_count++;
        break;
    }
    default:
        break;
    }
    probe->original_extended(state, dl);
}

struct ExtendedDispatchScope {
    RT64::Interpreter *interpreter;
    RT64::GBI *initial_gbi;
    ExtendedDispatchProbe &probe;

    ExtendedDispatchScope(RT64::Interpreter *interpreter, ExtendedDispatchProbe &probe)
        : interpreter(interpreter), initial_gbi(interpreter->hleGBI), probe(probe) {
        probe.original_hook = initial_gbi->map[0xE0];
        probe.original_extended = interpreter->extendedFunction;
        if ((active_extended_probe != nullptr) || (probe.original_hook == nullptr) ||
            (probe.original_extended == nullptr)) {
            std::terminate();
        }
        active_extended_probe = &probe;
        initial_gbi->map[0xE0] = observed_extended_hook;
        interpreter->extendedFunction = observed_extended_command;
    }

    ~ExtendedDispatchScope() {
        if ((active_extended_probe != &probe) ||
            (initial_gbi->map[0xE0] != observed_extended_hook) ||
            (interpreter->extendedFunction != observed_extended_command)) {
            std::terminate();
        }
        initial_gbi->map[0xE0] = probe.original_hook;
        interpreter->extendedFunction = probe.original_extended;
        active_extended_probe = nullptr;
    }
};

bool deferred_workload_snapshot(
    const RT64::Workload &workload,
    Fn64Rt64DeferredWorkloadSnapshot &snapshot,
    char *error,
    size_t error_capacity) {
    if ((workload.fbPairCount > workload.fbPairs.size()) ||
        (workload.fbPairCount > FN64_RT64_DEFERRED_MAX_FRAMEBUFFER_PAIRS)) {
        set_error(error, error_capacity, "RT64 deferred workload exceeds framebuffer-pair evidence capacity");
        return false;
    }
    if (workload.gameCallCount > FN64_RT64_DEFERRED_MAX_DRAW_CALLS) {
        set_error(error, error_capacity, "RT64 deferred workload exceeds draw-call evidence capacity");
        return false;
    }

    const auto checked_count = [&](size_t value, const char *name, uint32_t &output) {
        if (value > std::numeric_limits<uint32_t>::max()) {
            set_error(error, error_capacity, std::string("RT64 deferred workload ") + name + " count exceeds u32");
            return false;
        }
        output = static_cast<uint32_t>(value);
        return true;
    };

    snapshot = {};
    snapshot.workload_id = workload.workloadId;
    snapshot.present_id = workload.presentId;
    snapshot.submission_frame = workload.submissionFrame;
    snapshot.framebuffer_pair_count = workload.fbPairCount;
    snapshot.game_call_count = workload.gameCallCount;
    snapshot.selected_framebuffer_index = workload.debuggerRenderer.framebufferIndex;
    snapshot.selected_draw_call_index = workload.debuggerRenderer.globalDrawCallIndex;
    snapshot.selected_framebuffer_address = workload.debuggerRenderer.framebufferAddress;
    snapshot.paused = static_cast<uint32_t>(workload.paused);
    if (!checked_count(workload.drawData.worldIndices.size(), "vertex", snapshot.vertex_count) ||
        !checked_count(workload.drawData.faceIndices.size(), "face-index", snapshot.face_index_count) ||
        !checked_count(workload.drawData.rdpParams.size(), "RDP-parameter", snapshot.rdp_param_count) ||
        !checked_count(workload.drawData.loadOperations.size(), "load-operation", snapshot.load_operation_count)) {
        return false;
    }

    uint64_t content_digest = 1469598103934665603ULL;
    digest_u64(content_digest, workload.submissionFrame);
    digest_u64(content_digest, workload.fbPairCount);
    digest_u64(content_digest, workload.gameCallCount);
    for (uint32_t count : {
             snapshot.vertex_count,
             snapshot.face_index_count,
             snapshot.rdp_param_count,
             snapshot.load_operation_count}) {
        digest_u64(content_digest, count);
    }

    uint32_t global_call = 0;
    for (uint32_t f = 0; f < workload.fbPairCount; f++) {
        const RT64::FramebufferPair &pair = workload.fbPairs[f];
        if (pair.projectionCount > pair.projections.size()) {
            set_error(error, error_capacity, "RT64 deferred framebuffer pair has an invalid projection count");
            return false;
        }
        snapshot.pair_color_addresses[f] = pair.colorImage.address;
        snapshot.pair_game_call_counts[f] = pair.gameCallCount;
        snapshot.pair_projection_counts[f] = pair.projectionCount;
        digest_u64(content_digest, f);
        for (uint64_t value : {
                 uint64_t(pair.colorImage.address),
                 uint64_t(pair.colorImage.fmt),
                 uint64_t(pair.colorImage.siz),
                 uint64_t(pair.colorImage.width),
                 uint64_t(pair.depthImage.address),
                 uint64_t(pair.gameCallCount),
                 uint64_t(pair.projectionCount)}) {
            digest_u64(content_digest, value);
        }

        uint32_t pair_call_count = 0;
        for (uint32_t p = 0; p < pair.projectionCount; p++) {
            const RT64::Projection &projection = pair.projections[p];
            if (projection.gameCallCount > projection.gameCalls.size()) {
                set_error(error, error_capacity, "RT64 deferred projection has an invalid draw-call count");
                return false;
            }
            snapshot.projection_count++;
            digest_u64(content_digest, p);
            digest_u64(content_digest, static_cast<uint32_t>(projection.type));
            digest_u64(content_digest, projection.gameCallCount);
            for (uint32_t d = 0; d < projection.gameCallCount; d++) {
                if (global_call >= FN64_RT64_DEFERRED_MAX_DRAW_CALLS) {
                    set_error(error, error_capacity, "RT64 deferred draw-call ordering exceeds evidence capacity");
                    return false;
                }
                const RT64::DrawCall &call = projection.gameCalls[d].callDesc;
                snapshot.call_uids[global_call] = call.uid;
                snapshot.call_fill_colors[global_call] = call.fillColor;
                snapshot.call_triangle_counts[global_call] = call.triangleCount;
                snapshot.triangle_count += call.triangleCount;
                for (uint64_t value : {
                         uint64_t(global_call),
                         uint64_t(call.uid),
                         uint64_t(call.callIndex),
                         uint64_t(call.triangleCount),
                         uint64_t(call.fillColor),
                         uint64_t(static_cast<uint32_t>(call.rect.ulx)),
                         uint64_t(static_cast<uint32_t>(call.rect.uly)),
                         uint64_t(static_cast<uint32_t>(call.rect.lrx)),
                         uint64_t(static_cast<uint32_t>(call.rect.lry)),
                         uint64_t(call.tileIndex),
                         uint64_t(call.tileCount),
                         uint64_t(call.loadIndex),
                         uint64_t(call.loadCount)}) {
                    digest_u64(content_digest, value);
                }
                global_call++;
                pair_call_count++;
            }
        }
        if (pair_call_count != pair.gameCallCount) {
            set_error(error, error_capacity, "RT64 deferred framebuffer-pair draw-call total is inconsistent");
            return false;
        }
    }
    if (global_call != workload.gameCallCount) {
        set_error(error, error_capacity, "RT64 deferred workload draw-call total is inconsistent");
        return false;
    }

    snapshot.content_digest = content_digest;
    snapshot.identity_digest = content_digest;
    digest_u64(snapshot.identity_digest, workload.workloadId);
    digest_u64(snapshot.identity_digest, workload.presentId);
    return true;
}

bool extended_workload_snapshot(
    const RT64::Workload &workload,
    uint32_t rect_alignment_count,
    uint32_t rect_alignment_call_index,
    int32_t left_offset,
    int32_t top_offset,
    int32_t right_offset,
    int32_t bottom_offset,
    const std::array<uint32_t, FN64_RT64_EXTENDED_MAX_VERTEX_Z_MARKERS>
        &vertex_command_indices,
    uint32_t vertex_command_count,
    Fn64Rt64ExtendedGbiEvidence &evidence,
    char *error,
    size_t error_capacity) {
    if (rect_alignment_count > 1) {
        set_error(error, error_capacity,
                  "multiple rectangle-alignment commands make per-call Extended evidence ambiguous");
        return false;
    }
    if ((rect_alignment_count != 0U) &&
        ((evidence.command_counts[0x02] != 0U) ||
         (evidence.command_counts[0x03] != 0U))) {
        set_error(error, error_capacity,
                  "global and per-command rectangle alignment evidence is ambiguous");
        return false;
    }

    uint32_t vertex_command_cursor = 0;
    int32_t active_vertex_marker = -1;
    uint32_t active_face_end = 0;
    uint32_t global_call_index = 0;
    for (uint32_t f = 0; f < workload.fbPairCount; f++) {
        const RT64::FramebufferPair &pair = workload.fbPairs[f];
        if (pair.projectionCount > pair.projections.size()) {
            set_error(error, error_capacity,
                      "RT64 Extended evidence found an invalid projection count");
            return false;
        }
        for (uint32_t p = 0; p < pair.projectionCount; p++) {
            const RT64::Projection &projection = pair.projections[p];
            if (projection.gameCallCount > projection.gameCalls.size()) {
                set_error(error, error_capacity,
                          "RT64 Extended evidence found an invalid draw-call count");
                return false;
            }
            for (uint32_t d = 0; d < projection.gameCallCount; d++) {
                const RT64::GameCall &game_call = projection.gameCalls[d];
                const RT64::DrawCall &call = game_call.callDesc;
                if (projection.type == RT64::Projection::Type::Rectangle) {
                    const uint32_t index = evidence.rect_count;
                    if (index >= FN64_RT64_EXTENDED_MAX_RECTS) {
                        set_error(error, error_capacity,
                                  "RT64 Extended rectangles exceed evidence capacity");
                        return false;
                    }
                    Fn64Rt64ExtendedRectEvidence &rect = evidence.rects[index];
                    rect.draw_call_uid = call.uid;
                    rect.left_origin = call.rectLeftOrigin;
                    rect.right_origin = call.rectRightOrigin;
                    const bool alignment_applies =
                        (rect_alignment_count != 0U) &&
                        (global_call_index >= rect_alignment_call_index);
                    rect.left_offset = alignment_applies ? left_offset : 0;
                    rect.top_offset = alignment_applies ? top_offset : 0;
                    rect.right_offset = alignment_applies ? right_offset : 0;
                    rect.bottom_offset = alignment_applies ? bottom_offset : 0;
                    rect.upper_left_x = call.rect.ulx;
                    rect.upper_left_y = call.rect.uly;
                    rect.lower_right_x = call.rect.lrx;
                    rect.lower_right_y = call.rect.lry;
                    rect.aspect_mode = call.rectAspect;
                    evidence.rect_count++;
                }

                if (call.extendedType == RT64::DrawExtendedType::VertexTestZ) {
                    if ((active_vertex_marker >= 0) ||
                        (vertex_command_cursor >= vertex_command_count) ||
                        (evidence.vertex_z_count >=
                         FN64_RT64_EXTENDED_MAX_VERTEX_Z_MARKERS)) {
                        set_error(error, error_capacity,
                                  "RT64 Extended vertex-Z begin markers are ambiguous or exceed evidence capacity");
                        return false;
                    }
                    const uint32_t marker_index = evidence.vertex_z_count++;
                    Fn64Rt64VertexZEvidence &marker = evidence.vertex_z[marker_index];
                    marker.marker_kind = FN64_RT64_VERTEX_Z_BEGIN;
                    marker.command_vertex_index =
                        vertex_command_indices[vertex_command_cursor++];
                    marker.resolved_source_index =
                        call.extendedData.vertexTestZ.vertexIndex;
                    const uint64_t affected_start =
                        uint64_t(game_call.meshDesc.faceIndicesStart) +
                        uint64_t(call.triangleCount) * 3U;
                    if (affected_start > std::numeric_limits<uint32_t>::max()) {
                        set_error(error, error_capacity,
                                  "RT64 Extended vertex-Z face-index start overflowed");
                        return false;
                    }
                    marker.affected_face_index_start =
                        static_cast<uint32_t>(affected_start);
                    marker.affected_face_index_count = 0;
                    active_vertex_marker = static_cast<int32_t>(marker_index);
                    active_face_end = marker.affected_face_index_start;
                }
                else if (call.extendedType == RT64::DrawExtendedType::EndVertexTestZ) {
                    if ((active_vertex_marker < 0) ||
                        (evidence.vertex_z_count >=
                         FN64_RT64_EXTENDED_MAX_VERTEX_Z_MARKERS) ||
                        (game_call.meshDesc.faceIndicesStart != active_face_end)) {
                        set_error(error, error_capacity,
                                  "RT64 Extended vertex-Z end marker has no exact affected face-index range");
                        return false;
                    }
                    Fn64Rt64VertexZEvidence &begin =
                        evidence.vertex_z[static_cast<uint32_t>(active_vertex_marker)];
                    begin.affected_face_index_count =
                        active_face_end - begin.affected_face_index_start;
                    Fn64Rt64VertexZEvidence &end =
                        evidence.vertex_z[evidence.vertex_z_count++];
                    end.marker_kind = FN64_RT64_VERTEX_Z_END;
                    end.command_vertex_index = FN64_RT64_VERTEX_Z_NO_COMMAND_INDEX;
                    end.resolved_source_index =
                        call.extendedData.vertexTestZ.vertexIndex;
                    end.affected_face_index_start = begin.affected_face_index_start;
                    end.affected_face_index_count = begin.affected_face_index_count;
                    active_vertex_marker = -1;
                }
                else if (active_vertex_marker >= 0) {
                    const bool uses_viewport =
                        (projection.type == RT64::Projection::Type::Perspective) ||
                        (projection.type == RT64::Projection::Type::Orthographic);
                    if (!uses_viewport ||
                        (game_call.meshDesc.faceIndicesStart != active_face_end)) {
                        set_error(error, error_capacity,
                                  "RT64 Extended vertex-Z affected face indices are non-contiguous");
                        return false;
                    }
                    const uint64_t next = uint64_t(active_face_end) +
                                          uint64_t(call.triangleCount) * 3U;
                    if (next > std::numeric_limits<uint32_t>::max()) {
                        set_error(error, error_capacity,
                                  "RT64 Extended vertex-Z face-index range overflowed");
                        return false;
                    }
                    active_face_end = static_cast<uint32_t>(next);
                }
                if (global_call_index == std::numeric_limits<uint32_t>::max()) {
                    set_error(error, error_capacity,
                              "RT64 Extended draw-call index overflowed");
                    return false;
                }
                global_call_index++;
            }
        }
    }
    if ((active_vertex_marker >= 0) ||
        (vertex_command_cursor != vertex_command_count)) {
        set_error(error, error_capacity,
                  "RT64 Extended vertex-Z command and marker counts disagree");
        return false;
    }
    return true;
}
} // namespace

namespace {
std::mutex present_capture_registry_mutex;
std::vector<Fn64Rt64Context *> present_capture_contexts;

// One caller-provided draw callback layered on top of present-capture's own
// readback, e.g. fn64-rmlui's UI overlay. `RT64::SetRenderHooks`'s draw slot
// is a single process-wide raw-function-pointer -- it cannot hold more than
// one callback and cannot capture state -- so `draw_hook_dispatch` below is
// the one function actually installed there, and this registry is how a
// second (third, ...) caller gets to draw into the same already-open
// command list without contending for that one slot. Plain C function
// pointer + user_data, not std::function: this crosses the same extern "C"
// boundary every other shim export does, and a non-C++ caller (fn64-rmlui's
// Rust side) cannot construct a std::function anyway.
struct DrawHookRegistrant {
    Fn64Rt64Context *context = nullptr;
    void (*callback)(void *command_list, void *framebuffer, void *user_data) = nullptr;
    void *user_data = nullptr;
};
std::mutex draw_hook_registry_mutex;
std::vector<DrawHookRegistrant> draw_hook_registrants;

std::mutex vi_filter_registry_mutex;
std::vector<Fn64Rt64Context *> vi_filter_contexts;
struct ViHistoryRateEntry {
    const void *history = nullptr;
    uint32_t nominal_refresh_rate = 0;
};
std::mutex vi_history_rate_registry_mutex;
std::vector<ViHistoryRateEntry> vi_history_rate_entries;
#if defined(FN64_RT64_HFR_EVIDENCE)
std::mutex hfr_pacing_registry_mutex;
std::vector<Fn64Rt64Context *> hfr_pacing_contexts;
#endif

struct ExtendedPresentCaptureSlot {
    std::unique_ptr<plume::RenderBuffer> buffer;
    uint64_t buffer_size = 0;
    uint64_t capture_generation = 0;
    uint64_t workload_id = 0;
    uint64_t present_id = 0;
    uint32_t generated_ordinal = FN64_RT64_EXTENDED_NO_GENERATED_ORDINAL;
    uint32_t interpolation_numerator = 0;
    uint32_t interpolation_denominator = 0;
    uint32_t width = 0;
    uint32_t height = 0;
    uint32_t row_pitch = 0;
    uint32_t format = 0;
};

void capture_present_draw(
    plume::RenderCommandList *command_list,
    plume::RenderFramebuffer *framebuffer) noexcept;
void unregister_present_capture(Fn64Rt64Context *context);
// The function actually installed via `RT64::SetRenderHooks`'s draw slot.
// Runs `capture_present_draw` first (unchanged, so present-capture keeps
// reading a UI-free frame), then every external registrant in
// `draw_hook_registrants`, in registration order.
void draw_hook_dispatch(
    plume::RenderCommandList *command_list,
    plume::RenderFramebuffer *framebuffer) noexcept;
void unregister_overlay_draw(Fn64Rt64Context *context);
void register_vi_filter_context(Fn64Rt64Context *context);
void unregister_vi_filter_context(Fn64Rt64Context *context);
void register_vi_history_rate(const void *history, uint32_t nominal_refresh_rate);
void unregister_vi_history_rate(const void *history);
#if defined(FN64_RT64_HFR_EVIDENCE)
void unregister_hfr_pacing(Fn64Rt64Context *context);
#endif
} // namespace

extern "C" uint32_t fn64_rt64_nominal_full_rate(const void *history) {
    std::scoped_lock lock(vi_history_rate_registry_mutex);
    const auto entry = std::find_if(
        vi_history_rate_entries.begin(),
        vi_history_rate_entries.end(),
        [history](const ViHistoryRateEntry &candidate) {
            return candidate.history == history;
        });
    if (entry == vi_history_rate_entries.end()) {
        throw std::runtime_error(
            "RT64 VI history reached logical-rate inference without a registered TV standard");
    }
    return entry->nominal_refresh_rate;
}

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
    bool ubershader_evidence_active = false;
#endif
    std::unique_ptr<RT64::Application> application;
    uint32_t width = 320;
    uint32_t height = 240;
    uint32_t nominal_refresh_rate = 60;
    uint32_t output_addr = 0;
    Fn64Rt64ViState vi_state{};
    bool setup_complete = false;
    bool ucode_admission_poisoned = false;
    bool vi_history_rate_registered = false;
    bool vi_filter_registered = false;
    bool presentation_refresh_pending = false;
    bool replacement_observed_resolved_not_installed = false;
    uint32_t replacement_stream_worker_count = 0;
    bool replacement_stream_workers_paused = false;
    bool deferred_capture_armed = false;
    bool deferred_capture_valid = false;
    uint32_t deferred_capture_slot = 0;
    Fn64Rt64DeferredWorkloadSnapshot deferred_pre_submission{};
    bool extended_capture_armed = false;
    bool extended_capture_valid = false;
    uint32_t extended_capture_slot = 0;
    uint64_t extended_capture_workload_id = 0;
    Fn64Rt64ExtendedGbiEvidence extended_evidence{};
    uint32_t extended_rect_alignment_count = 0;
    uint32_t extended_rect_alignment_call_index = 0;
    int32_t extended_left_offset = 0;
    int32_t extended_top_offset = 0;
    int32_t extended_right_offset = 0;
    int32_t extended_bottom_offset = 0;
    std::array<uint32_t, FN64_RT64_EXTENDED_MAX_VERTEX_Z_MARKERS>
        extended_vertex_command_indices{};
    uint32_t extended_vertex_command_count = 0;
    std::mutex present_capture_mutex;
    std::unique_ptr<plume::RenderBuffer> present_capture_buffer;
    uint64_t present_capture_buffer_size = 0;
    uint64_t present_capture_generation = 0;
    uint64_t present_capture_id = 0;
    uint64_t present_capture_workload_id = 0;
    uint32_t present_capture_width = 0;
    uint32_t present_capture_height = 0;
    uint32_t present_capture_row_pitch = 0;
    uint32_t present_capture_format = 0;
    uint32_t present_capture_graphics_api = 0;
    std::string present_capture_error;
    bool present_capture_enabled = false;
    std::array<ExtendedPresentCaptureSlot, FN64_RT64_EXTENDED_MAX_GENERATED_PRESENTS>
        extended_present_captures{};
    uint32_t extended_present_capture_count = 0;
    bool extended_present_capture_recording = false;
    bool extended_present_capture_finalized = false;
#if defined(FN64_RT64_HFR_EVIDENCE)
    bool hfr_capture_armed = false;
    bool hfr_capture_valid = false;
    uint32_t hfr_capture_slot = 0;
    uint64_t hfr_capture_workload_id = 0;
    std::array<ExtendedPresentCaptureSlot, FN64_RT64_EXTENDED_MAX_GENERATED_PRESENTS>
        hfr_present_captures{};
    uint32_t hfr_present_capture_count = 0;
    bool hfr_present_capture_recording = false;
    bool hfr_present_capture_finalized = false;
    std::array<Fn64Rt64HfrPacingSample, FN64_RT64_HFR_MAX_PACING_SAMPLES>
        hfr_pacing_samples{};
    uint32_t hfr_pacing_sample_count = 0;
    bool hfr_pacing_recording = false;
    bool hfr_pacing_pending = false;
    std::string hfr_pacing_error;
#endif

    Fn64Rt64Context(uint32_t width, uint32_t height, uint32_t nominal_refresh_rate)
        : placeholder_rdram(std::make_unique<uint8_t[]>(N64_RDRAM_SIZE)),
          width(width),
          height(height),
          nominal_refresh_rate(nominal_refresh_rate) {
        std::memset(placeholder_rdram.get(), 0, N64_RDRAM_SIZE);
        vi_state.registers[0] = VI_STATUS_16_BIT;
    }

    ~Fn64Rt64Context() {
        if (setup_complete && application) {
            if (present_diagnostics_enabled()) {
                std::fprintf(
                    stderr,
                    "fn64 RT64 shutdown diagnostic: begin state_workload=%llu queue_workload=%llu state_present=%llu queue_present=%llu\n",
                    static_cast<unsigned long long>(application->state->workloadId),
                    static_cast<unsigned long long>(application->workloadQueue->workloadId),
                    static_cast<unsigned long long>(application->state->presentId),
                    static_cast<unsigned long long>(application->presentQueue->presentId));
            }
            application->workloadQueue->waitForWorkloadId(application->state->workloadId);
            if (present_diagnostics_enabled()) {
                std::fprintf(stderr, "fn64 RT64 shutdown diagnostic: workload-id complete\n");
            }
            application->presentQueue->waitForPresentId(application->state->presentId);
            if (present_diagnostics_enabled()) {
                std::fprintf(stderr, "fn64 RT64 shutdown diagnostic: present-id complete\n");
            }

            // PresentQueue publishes presentId before its swapchain-present
            // tail finishes. Destruction could therefore reset State and
            // WorkloadQueue while that tail still used their shared resources.
            // Waiting on both queue thread mutexes closes that exact shutdown
            // interleaving after their published IDs have caught up.
            application->workloadQueue->waitForIdle();
            if (present_diagnostics_enabled()) {
                std::fprintf(stderr, "fn64 RT64 shutdown diagnostic: workload-idle complete\n");
            }
            application->presentQueue->waitForIdle();
            if (present_diagnostics_enabled()) {
                std::fprintf(stderr, "fn64 RT64 shutdown diagnostic: present-idle complete\n");
            }
#if defined(__APPLE__)
            if (ubershader_evidence_active) {
                std::scoped_lock probe_lock(metal_pipeline_probe.control_mutex);
                metal_pipeline_probe.active.store(false, std::memory_order_release);
                metal_pipeline_probe.caller_scope.store(false, std::memory_order_release);
                application->workloadQueue->ubershadersOnly.store(false);
                ubershader_evidence_active = false;
            }
#endif
            // Rust owns Context uniquely, so destruction cannot overlap the
            // display-list caller that performs logical-rate lookup. Drain the
            // RT64 workers, then remove the entry while State still owns the
            // exact VIHistory address.
            if (vi_history_rate_registered) {
                unregister_vi_history_rate(&application->state->viHistory);
                vi_history_rate_registered = false;
            }
            if (vi_filter_registered) {
                unregister_vi_filter_context(this);
                vi_filter_registered = false;
            }
            if (present_diagnostics_enabled()) {
                std::fprintf(stderr, "fn64 RT64 shutdown diagnostic: application-end begin\n");
            }
            application->end();
            if (present_diagnostics_enabled()) {
                std::fprintf(stderr, "fn64 RT64 shutdown diagnostic: application-end complete\n");
            }
        }
        if (present_diagnostics_enabled()) {
            std::fprintf(stderr, "fn64 RT64 shutdown diagnostic: capture-unregister begin\n");
        }
        unregister_present_capture(this);
        unregister_overlay_draw(this);
#if defined(FN64_RT64_HFR_EVIDENCE)
        unregister_hfr_pacing(this);
#endif
        if (present_diagnostics_enabled()) {
            std::fprintf(stderr, "fn64 RT64 shutdown diagnostic: capture-unregister complete\n");
        }
        application.reset();
        if (present_diagnostics_enabled()) {
            std::fprintf(stderr, "fn64 RT64 shutdown diagnostic: application-reset complete\n");
        }
#if defined(__APPLE__)
        if (metal_view != nullptr) {
            SDL_Metal_DestroyView(metal_view);
        }
        if (host_window != nullptr) {
            SDL_DestroyWindow(host_window);
        }
        if (present_diagnostics_enabled()) {
            std::fprintf(stderr, "fn64 RT64 shutdown diagnostic: native-surface teardown complete\n");
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

    void update_vi() {
        std::array<uint32_t, 24> next_registers = registers;
        write_vi_registers(
            next_registers,
            output_addr,
            width,
            height,
            vi_state);
        registers = next_registers;
    }

    void update_vi(const Fn64Rt64ViState &next_vi_state) {
        std::array<uint32_t, 24> next_registers = registers;
        write_vi_registers(
            next_registers,
            output_addr,
            width,
            height,
            next_vi_state);
        vi_state = next_vi_state;
        registers = next_registers;
    }
};

namespace {
void register_vi_filter_context(Fn64Rt64Context *context) {
    std::scoped_lock lock(vi_filter_registry_mutex);
    const auto duplicate = std::find_if(
        vi_filter_contexts.begin(),
        vi_filter_contexts.end(),
        [context](const Fn64Rt64Context *candidate) {
            return (candidate == context) ||
                   (candidate->application->device.get() ==
                    context->application->device.get());
        });
    if (duplicate != vi_filter_contexts.end()) {
        throw std::runtime_error("RT64 VI filter device was registered twice");
    }
    vi_filter_contexts.push_back(context);
}

void unregister_vi_filter_context(Fn64Rt64Context *context) {
    std::scoped_lock lock(vi_filter_registry_mutex);
    const size_t prior_size = vi_filter_contexts.size();
    vi_filter_contexts.erase(
        std::remove(vi_filter_contexts.begin(), vi_filter_contexts.end(), context),
        vi_filter_contexts.end());
    if (vi_filter_contexts.size() + 1U != prior_size) {
        std::terminate();
    }
}

uint32_t vi_filter_flags_for_context(const Fn64Rt64Context &context) {
    const uint32_t status = context.registers[9];
    const bool rgba16 = (status & 3U) == 2U;
    const bool silhouette_aa =
        (context.vi_state.aa_mode_specified != 0U) &&
        (((status >> 8U) & 3U) <= 1U);
    return
        (((status & (1U << 16U)) != 0U) && rgba16
             ? interop::ViFilterDitherRestoration
             : 0U) |
        (silhouette_aa ? interop::ViFilterSilhouetteAa : 0U) |
        (rgba16 ? interop::ViFilterRgba16 : 0U) |
        (((status & (1U << 6U)) != 0U)
             ? interop::ViFilterSerratedRows
             : 0U);
}

Fn64Rt64Context *vi_filter_context_for_device(void *device) {
    const auto entry = std::find_if(
        vi_filter_contexts.begin(),
        vi_filter_contexts.end(),
        [device](const Fn64Rt64Context *context) {
            return context->application &&
                   (context->application->device.get() == device);
        });
    return (entry == vi_filter_contexts.end()) ? nullptr : *entry;
}
} // namespace

extern "C" void fn64_rt64_vi_filter_constants(
    void *device,
    interop::VideoInterfaceCB *constants) {
    if (constants == nullptr) {
        std::terminate();
    }
    std::scoped_lock lock(vi_filter_registry_mutex);
    Fn64Rt64Context *context = vi_filter_context_for_device(device);
    if (context == nullptr) {
        // Pinned setup constructs VIRenderer but invokes this hook only from
        // render, after the completed context publishes its stable device.
        // A miss is therefore stale/wrong live ownership; silently clearing
        // filters would publish an image under false policy.
        std::terminate();
    }
    const uint32_t status = context->registers[9];
    constants->gammaDither = (status & (1U << 2U)) != 0U ? 1U : 0U;
    constants->divot = (status & (1U << 4U)) != 0U ? 1U : 0U;
    constants->viFilterFlags = vi_filter_flags_for_context(*context);
    constants->noiseSeedLow = uint32_t(context->vi_state.noise_seed);
    constants->noiseSeedHigh = uint32_t(context->vi_state.noise_seed >> 32U);
    constants->policyVersion = 1U;
}

extern "C" uint32_t fn64_rt64_vi_retrace_event(
    void *device,
    uint32_t from_early_present) {
    if (from_early_present != 0U) {
        return 0U;
    }
    std::scoped_lock lock(vi_filter_registry_mutex);
    // A registered call is one authoritative fn64 VI event. Setup-owned
    // updateScreen calls precede registration and keep pinned RT64 behavior.
    return vi_filter_context_for_device(device) != nullptr ? 1U : 0U;
}

/// Bind fn64's one physical allocation only for a synchronous renderer call.
/// Every caller waits its exact workload or present worker before this scope
/// ends, so neither RT64 alias can retain a Rust-owned pointer while guest
/// execution resumes or another direct embedder supplies its allocation.
class ScopedRdramBinding {
public:
    ScopedRdramBinding(Fn64Rt64Context *context, uint8_t *rdram)
        : context(context) {
        context->application->core.RDRAM = rdram;
        context->application->state->RDRAM = rdram;
    }

    ScopedRdramBinding(const ScopedRdramBinding &) = delete;
    ScopedRdramBinding &operator=(const ScopedRdramBinding &) = delete;

    ~ScopedRdramBinding() {
        // Interleaving closed here, including exception exits: a workload or
        // present worker can publish its ID before its queue tail releases
        // Core/State. Restoring the placeholder at that publication point
        // would let the tail dereference either a replaced allocation or the
        // placeholder while Rust believes its call-scoped capability ended.
        // Both queues must be idle before either foreign alias changes.
        context->application->workloadQueue->waitForIdle();
        context->application->presentQueue->waitForIdle();
        uint8_t *placeholder = context->placeholder_rdram.get();
        context->application->core.RDRAM = placeholder;
        context->application->state->RDRAM = placeholder;
    }

private:
    Fn64Rt64Context *context;
};

namespace {
// FIPS 180-4 SHA-256 keeps the plan identity self-contained at the C++ ABI;
// pointer values never participate in the canonical encoding.
class Sha256 {
public:
    Sha256()
        : state{0x6A09E667U, 0xBB67AE85U, 0x3C6EF372U, 0xA54FF53AU,
                0x510E527FU, 0x9B05688CU, 0x1F83D9ABU, 0x5BE0CD19U} {}

    void update(const uint8_t *bytes, size_t length) {
        if ((bytes == nullptr) && (length != 0U)) {
            throw std::runtime_error("SHA-256 input is null");
        }
        for (size_t index = 0; index < length; index++) {
            block[block_length++] = bytes[index];
            if (block_length == block.size()) {
                transform();
                bit_length += 512U;
                block_length = 0;
            }
        }
    }

    std::array<uint8_t, 32> finish() {
        const uint64_t final_bit_length = bit_length + uint64_t(block_length) * 8U;
        block[block_length++] = 0x80U;
        if (block_length > 56U) {
            std::fill(block.begin() + block_length, block.end(), 0U);
            transform();
            block_length = 0;
        }
        std::fill(block.begin() + block_length, block.begin() + 56U, 0U);
        for (uint32_t index = 0; index < 8U; index++) {
            block[63U - index] =
                static_cast<uint8_t>(final_bit_length >> (index * 8U));
        }
        transform();

        std::array<uint8_t, 32> digest{};
        for (uint32_t word = 0; word < state.size(); word++) {
            for (uint32_t byte = 0; byte < 4U; byte++) {
                digest[word * 4U + byte] = static_cast<uint8_t>(
                    state[word] >> (24U - byte * 8U));
            }
        }
        return digest;
    }

private:
    static uint32_t rotate_right(uint32_t value, uint32_t amount) {
        return (value >> amount) | (value << (32U - amount));
    }

    void transform() {
        static constexpr std::array<uint32_t, 64> RoundConstants = {
            0x428A2F98U, 0x71374491U, 0xB5C0FBCFU, 0xE9B5DBA5U,
            0x3956C25BU, 0x59F111F1U, 0x923F82A4U, 0xAB1C5ED5U,
            0xD807AA98U, 0x12835B01U, 0x243185BEU, 0x550C7DC3U,
            0x72BE5D74U, 0x80DEB1FEU, 0x9BDC06A7U, 0xC19BF174U,
            0xE49B69C1U, 0xEFBE4786U, 0x0FC19DC6U, 0x240CA1CCU,
            0x2DE92C6FU, 0x4A7484AAU, 0x5CB0A9DCU, 0x76F988DAU,
            0x983E5152U, 0xA831C66DU, 0xB00327C8U, 0xBF597FC7U,
            0xC6E00BF3U, 0xD5A79147U, 0x06CA6351U, 0x14292967U,
            0x27B70A85U, 0x2E1B2138U, 0x4D2C6DFCU, 0x53380D13U,
            0x650A7354U, 0x766A0ABBU, 0x81C2C92EU, 0x92722C85U,
            0xA2BFE8A1U, 0xA81A664BU, 0xC24B8B70U, 0xC76C51A3U,
            0xD192E819U, 0xD6990624U, 0xF40E3585U, 0x106AA070U,
            0x19A4C116U, 0x1E376C08U, 0x2748774CU, 0x34B0BCB5U,
            0x391C0CB3U, 0x4ED8AA4AU, 0x5B9CCA4FU, 0x682E6FF3U,
            0x748F82EEU, 0x78A5636FU, 0x84C87814U, 0x8CC70208U,
            0x90BEFFFAU, 0xA4506CEBU, 0xBEF9A3F7U, 0xC67178F2U};

        std::array<uint32_t, 64> schedule{};
        for (uint32_t index = 0; index < 16U; index++) {
            const uint32_t offset = index * 4U;
            schedule[index] = (uint32_t(block[offset]) << 24U) |
                              (uint32_t(block[offset + 1U]) << 16U) |
                              (uint32_t(block[offset + 2U]) << 8U) |
                              uint32_t(block[offset + 3U]);
        }
        for (uint32_t index = 16U; index < schedule.size(); index++) {
            const uint32_t s0 = rotate_right(schedule[index - 15U], 7U) ^
                                rotate_right(schedule[index - 15U], 18U) ^
                                (schedule[index - 15U] >> 3U);
            const uint32_t s1 = rotate_right(schedule[index - 2U], 17U) ^
                                rotate_right(schedule[index - 2U], 19U) ^
                                (schedule[index - 2U] >> 10U);
            schedule[index] = schedule[index - 16U] + s0 +
                              schedule[index - 7U] + s1;
        }

        uint32_t a = state[0];
        uint32_t b = state[1];
        uint32_t c = state[2];
        uint32_t d = state[3];
        uint32_t e = state[4];
        uint32_t f = state[5];
        uint32_t g = state[6];
        uint32_t h = state[7];
        for (uint32_t index = 0; index < schedule.size(); index++) {
            const uint32_t s1 = rotate_right(e, 6U) ^ rotate_right(e, 11U) ^
                                rotate_right(e, 25U);
            const uint32_t choose = (e & f) ^ ((~e) & g);
            const uint32_t temporary1 = h + s1 + choose +
                                        RoundConstants[index] + schedule[index];
            const uint32_t s0 = rotate_right(a, 2U) ^ rotate_right(a, 13U) ^
                                rotate_right(a, 22U);
            const uint32_t majority = (a & b) ^ (a & c) ^ (b & c);
            const uint32_t temporary2 = s0 + majority;
            h = g;
            g = f;
            f = e;
            e = d + temporary1;
            d = c;
            c = b;
            b = a;
            a = temporary1 + temporary2;
        }
        state[0] += a;
        state[1] += b;
        state[2] += c;
        state[3] += d;
        state[4] += e;
        state[5] += f;
        state[6] += g;
        state[7] += h;
    }

    std::array<uint32_t, 8> state;
    std::array<uint8_t, 64> block{};
    uint64_t bit_length = 0;
    size_t block_length = 0;
};

void sha256_u32_le(Sha256 &sha, uint32_t value) {
    const std::array<uint8_t, 4> encoded = {
        static_cast<uint8_t>(value),
        static_cast<uint8_t>(value >> 8U),
        static_cast<uint8_t>(value >> 16U),
        static_cast<uint8_t>(value >> 24U)};
    sha.update(encoded.data(), encoded.size());
}

void sha256_u64_le(Sha256 &sha, uint64_t value) {
    std::array<uint8_t, 8> encoded{};
    for (uint32_t index = 0; index < encoded.size(); index++) {
        encoded[index] = static_cast<uint8_t>(value >> (index * 8U));
    }
    sha.update(encoded.data(), encoded.size());
}

struct NativeUcodeIdentity {
    RT64::GBIUCode ucode = RT64::GBIUCode::Unknown;
    bool low_p = false;
    bool no_n = false;
    bool rej = false;
    bool compute_mvp = false;
    bool point_lighting = false;
};

NativeUcodeIdentity native_ucode_identity(const RT64::GBI &gbi) {
    return NativeUcodeIdentity{
        gbi.ucode,
        gbi.flags.LowP,
        gbi.flags.NoN,
        gbi.flags.ReJ,
        gbi.flags.computeMVP,
        gbi.flags.pointLighting};
}

void apply_native_ucode_flags(RT64::GBI &gbi,
                              const NativeUcodeIdentity &identity) {
    gbi.flags.LowP = identity.low_p;
    gbi.flags.NoN = identity.no_n;
    gbi.flags.ReJ = identity.rej;
    gbi.flags.computeMVP = identity.compute_mvp;
    gbi.flags.pointLighting = identity.point_lighting;
}

bool same_native_ucode_identity(const NativeUcodeIdentity &left,
                                const NativeUcodeIdentity &right) {
    return (left.ucode == right.ucode) &&
           (left.low_p == right.low_p) &&
           (left.no_n == right.no_n) &&
           (left.rej == right.rej) &&
           (left.compute_mvp == right.compute_mvp) &&
           (left.point_lighting == right.point_lighting);
}

bool expected_family_matches(uint32_t family,
                             uint32_t detail,
                             const NativeUcodeIdentity &identity) {
    using UCode = RT64::GBIUCode;
    switch (family) {
    case 1U:
        return (identity.ucode == UCode::F3D) && !identity.low_p &&
               !identity.no_n && !identity.rej;
    case 2U:
        return (identity.ucode == UCode::F3DEX) && !identity.low_p &&
               !identity.no_n && !identity.rej;
    case 3U:
        return (identity.ucode == UCode::F3DEX) && identity.low_p &&
               !identity.no_n && !identity.rej;
    case 4U:
        return (identity.ucode == UCode::F3DEX) && identity.low_p &&
               !identity.no_n && identity.rej;
    case 5U:
        return (identity.ucode == UCode::F3DEX2) && !identity.low_p &&
               !identity.no_n && !identity.rej;
    case 6U:
        return (identity.ucode == UCode::F3DEX2) && !identity.low_p &&
               identity.no_n && !identity.rej;
    case 7U:
        return (identity.ucode == UCode::F3DEX2) && !identity.low_p &&
               !identity.no_n && identity.rej;
    case 8U:
        return (identity.ucode == UCode::F3DEX2) && identity.low_p &&
               !identity.no_n && identity.rej;
    case 9U:
        // RT64 exposes the I/J distinction only through the admitted raw
        // identity; their native behavior flags are intentionally identical.
        return (identity.ucode == UCode::F3DZEX2) && !identity.low_p &&
               identity.no_n && !identity.rej &&
               (identity.point_lighting == (detail != 1U));
    case 10U:
        return (identity.ucode == UCode::S2DEX) && !identity.low_p &&
               !identity.no_n && !identity.rej;
    case 11U:
        return (identity.ucode == UCode::S2DEX2) && !identity.low_p &&
               !identity.no_n && !identity.rej;
    case 13U:
        return (identity.ucode == UCode::L3DEX2) && !identity.low_p &&
               !identity.no_n && !identity.rej;
    default:
        return false;
    }
}

struct ImmutableUcodePlan {
    std::vector<Fn64Rt64UcodeGeneration> entries;
    std::vector<uint8_t> raw;
    std::array<uint8_t, 32> sha256{};
    std::vector<NativeUcodeIdentity> native_identities;
};

std::array<uint8_t, 32> canonical_plan_sha256(
    const Fn64Rt64UcodePlan &plan,
    const std::vector<Fn64Rt64UcodeGeneration> &entries,
    const std::vector<uint8_t> &raw) {
    static constexpr uint8_t Domain[] = "fn64-rt64-ucode-plan-v2";
    Sha256 sha;
    sha.update(Domain, sizeof(Domain) - 1U);
    sha256_u32_le(sha, plan.schema);
    sha256_u32_le(sha, plan.count);
    for (const Fn64Rt64UcodeGeneration &generation : entries) {
        const uint32_t scalars[] = {
            generation.source,
            generation.text_address,
            generation.data_address,
            generation.expected_family,
            generation.data_bytes,
            generation.raw_text_offset,
            generation.raw_text_len,
            generation.raw_data_offset,
            generation.raw_data_len,
            generation.expected_detail};
        for (const uint32_t scalar : scalars) {
            sha256_u32_le(sha, scalar);
        }
        sha.update(generation.logical_text_sha256,
                   sizeof(generation.logical_text_sha256));
        sha.update(generation.logical_data_sha256,
                   sizeof(generation.logical_data_sha256));
        for (const uint32_t reserved : generation.reserved) {
            sha256_u32_le(sha, reserved);
        }
    }
    sha256_u64_le(sha, plan.raw_len);
    for (const uint32_t reserved : plan.reserved) {
        sha256_u32_le(sha, reserved);
    }
    sha.update(raw.data(), raw.size());
    return sha.finish();
}

enum class UcodePlanPreflight {
    Ready,
    NeedsLle,
    Invalid
};

UcodePlanPreflight preflight_ucode_plan(
    const Fn64Rt64UcodePlan *source,
    const Fn64Rt64Task &task,
    ImmutableUcodePlan &plan,
    uint32_t &rejected_generation,
    std::string &diagnostic) {
    rejected_generation = FN64_RT64_UCODE_NO_REJECTED_GENERATION;
    if (source == nullptr) {
        diagnostic = "null RT64 ordered microcode-plan pointer";
        return UcodePlanPreflight::Invalid;
    }
    if (source->schema != FN64_RT64_UCODE_PLAN_SCHEMA) {
        diagnostic = "unsupported RT64 ordered microcode-plan schema";
        return UcodePlanPreflight::Invalid;
    }
    if ((source->count == 0U) || (source->entries == nullptr)) {
        diagnostic = "RT64 ordered microcode plan has no task-entry generation";
        return UcodePlanPreflight::Invalid;
    }
    if ((source->raw_len > uint64_t(std::numeric_limits<size_t>::max())) ||
        ((source->raw_len != 0U) && (source->raw_pool == nullptr))) {
        diagnostic = "RT64 ordered microcode-plan raw pool is invalid";
        return UcodePlanPreflight::Invalid;
    }
    if (std::any_of(std::begin(source->reserved), std::end(source->reserved),
                    [](uint32_t value) { return value != 0U; })) {
        diagnostic = "RT64 ordered microcode-plan reserved fields are nonzero";
        return UcodePlanPreflight::Invalid;
    }

    plan.entries.assign(source->entries, source->entries + source->count);
    if (source->raw_len != 0U) {
        plan.raw.assign(source->raw_pool,
                        source->raw_pool + static_cast<size_t>(source->raw_len));
    }
    std::copy(std::begin(source->plan_sha256), std::end(source->plan_sha256),
              plan.sha256.begin());
    const std::array<uint8_t, 32> computed =
        canonical_plan_sha256(*source, plan.entries, plan.raw);
    if (computed != plan.sha256) {
        diagnostic = "RT64 ordered microcode-plan SHA-256 mismatch";
        return UcodePlanPreflight::Invalid;
    }

    const uint32_t task_text = task.ucode & 0x00FFFFF8U;
    const uint32_t task_data = task.ucode_data & 0x00FFFFF8U;
    RT64::GBIManager scratch_manager;
    std::vector<uint8_t> scratch(N64_RDRAM_SIZE, 0U);
    plan.native_identities.reserve(plan.entries.size());
    for (uint32_t index = 0; index < plan.entries.size(); index++) {
        const Fn64Rt64UcodeGeneration &generation = plan.entries[index];
        const bool entry = index == 0U;
        const uint32_t expected_source = entry
            ? FN64_RT64_UCODE_SOURCE_TASK_ENTRY
            : FN64_RT64_UCODE_SOURCE_SELF_LOAD;
        if (generation.source != expected_source) {
            diagnostic = "RT64 ordered microcode-plan source order is invalid at generation " +
                         std::to_string(index);
            return UcodePlanPreflight::Invalid;
        }
        if ((generation.text_address & 0xFF000007U) != 0U ||
            (generation.data_address & 0xFF000007U) != 0U) {
            diagnostic = "RT64 ordered microcode-plan address is not a masked aligned physical address at generation " +
                         std::to_string(index);
            return UcodePlanPreflight::Invalid;
        }
        if (entry && ((generation.text_address != task_text) ||
                      (generation.data_address != task_data))) {
            diagnostic = "RT64 ordered microcode-plan entry addresses disagree with OSTask";
            return UcodePlanPreflight::Invalid;
        }
        const bool valid_expected_detail =
            (generation.expected_family == 0U) ||
            ((generation.expected_family == 9U) &&
             (generation.expected_detail >= 1U) &&
             (generation.expected_detail <= 3U)) ||
            ((generation.expected_family != 9U) &&
             (generation.expected_detail == 0U));
        if ((generation.data_bytes == 0U) ||
            (generation.data_bytes > 4096U) ||
            ((generation.data_bytes & 7U) != 0U) ||
            (generation.raw_text_len !=
             FN64_RT64_UCODE_TEXT_RECOGNITION_BYTES) ||
            (generation.raw_data_len !=
             FN64_RT64_UCODE_DATA_RECOGNITION_BYTES) ||
            !valid_expected_detail ||
            std::any_of(std::begin(generation.reserved),
                        std::end(generation.reserved),
                        [](uint32_t value) { return value != 0U; })) {
            diagnostic = "RT64 ordered microcode-plan generation shape or expected detail is invalid at generation " +
                         std::to_string(index);
            return UcodePlanPreflight::Invalid;
        }
        const uint64_t text_end = uint64_t(generation.raw_text_offset) +
                                  generation.raw_text_len;
        const uint64_t data_end = uint64_t(generation.raw_data_offset) +
                                  generation.raw_data_len;
        const uint64_t rdram_text_end = uint64_t(generation.text_address) +
                                        generation.raw_text_len;
        const uint64_t rdram_data_end = uint64_t(generation.data_address) +
                                        generation.raw_data_len;
        if ((text_end > plan.raw.size()) || (data_end > plan.raw.size()) ||
            (rdram_text_end > N64_RDRAM_SIZE) ||
            (rdram_data_end > N64_RDRAM_SIZE)) {
            diagnostic = "RT64 ordered microcode-plan recognition window is out of bounds at generation " +
                         std::to_string(index);
            return UcodePlanPreflight::Invalid;
        }

        const uint8_t *text = plan.raw.data() + generation.raw_text_offset;
        const uint8_t *data = plan.raw.data() + generation.raw_data_offset;
        std::memcpy(scratch.data() + generation.text_address,
                    text, generation.raw_text_len);
        std::memcpy(scratch.data() + generation.data_address,
                    data, generation.raw_data_len);
        if ((std::memcmp(scratch.data() + generation.text_address,
                         text, generation.raw_text_len) != 0) ||
            (std::memcmp(scratch.data() + generation.data_address,
                         data, generation.raw_data_len) != 0)) {
            diagnostic = "RT64 ordered microcode-plan overlapping recognition windows conflict at generation " +
                         std::to_string(index);
            return UcodePlanPreflight::Invalid;
        }

        // Family 12 is the logical L3DEX family, which pinned RT64 does not
        // expose as an HLE GBI. Reject it before calling the manager because
        // its database's historical Unknown tag is not an executable GBI.
        if ((generation.expected_family == 0U) ||
            (generation.expected_family == 12U) ||
            (generation.expected_family > 13U)) {
            rejected_generation = index;
            diagnostic = "RT64 ordered microcode-plan family requires LLE at generation " +
                         std::to_string(index);
            return UcodePlanPreflight::NeedsLle;
        }
        RT64::GBI *recognized = scratch_manager.getGBIForUCode(
            scratch.data(), generation.text_address, generation.data_address);
        if (recognized == nullptr) {
            rejected_generation = index;
            diagnostic = "RT64 did not recognize ordered microcode generation " +
                         std::to_string(index);
            return UcodePlanPreflight::NeedsLle;
        }
        const NativeUcodeIdentity identity = native_ucode_identity(*recognized);
        if (!expected_family_matches(generation.expected_family,
                                     generation.expected_detail,
                                     identity)) {
            rejected_generation = index;
            diagnostic = "RT64 native microcode family or expected detail disagrees with ordered preflight at generation " +
                         std::to_string(index);
            return UcodePlanPreflight::NeedsLle;
        }
        plan.native_identities.push_back(identity);
    }
    return UcodePlanPreflight::Ready;
}

struct ActiveUcodePlan {
    Fn64Rt64Context *context = nullptr;
    RT64::Interpreter *interpreter = nullptr;
    const ImmutableUcodePlan *plan = nullptr;
    Fn64Rt64TaskResult *result = nullptr;
    uint32_t cursor = 0;
    bool execution_started = false;
    RT64::GBI *pending_gbi = nullptr;
    NativeUcodeIdentity pending_identity{};
    bool pending = false;
};

thread_local ActiveUcodePlan *active_ucode_plan = nullptr;

class ScopedUcodePlan {
public:
    ScopedUcodePlan(Fn64Rt64Context *context,
                    RT64::Interpreter *interpreter,
                    const ImmutableUcodePlan &plan,
                    Fn64Rt64TaskResult *result)
        : active{context, interpreter, &plan, result, 0U, false,
                 nullptr, {}, false} {
        if (active_ucode_plan != nullptr) {
            throw std::runtime_error("nested RT64 ordered microcode-plan scope");
        }
        active_ucode_plan = &active;
    }

    ScopedUcodePlan(const ScopedUcodePlan &) = delete;
    ScopedUcodePlan &operator=(const ScopedUcodePlan &) = delete;

    ~ScopedUcodePlan() {
        if (active_ucode_plan == &active) {
            active_ucode_plan = nullptr;
        }
        if (active.execution_started &&
            (active.cursor != active.plan->entries.size())) {
            active.context->ucode_admission_poisoned = true;
            active.result->rejected_generation = active.cursor;
        }
    }

    bool exhausted() const {
        return active.cursor == active.plan->entries.size();
    }

    uint32_t cursor() const {
        return active.cursor;
    }

private:
    ActiveUcodePlan active;
};

[[noreturn]] void poison_ucode_plan(ActiveUcodePlan &active,
                                    uint32_t generation,
                                    const std::string &diagnostic) {
    active.context->ucode_admission_poisoned = true;
    active.result->rejected_generation = generation;
    throw std::runtime_error(diagnostic);
}
} // namespace

extern "C" void *fn64_rt64_ucode_generation_observe(
    void *raw_interpreter,
    uint32_t text_address,
    uint32_t data_address,
    uint32_t reset_from_task) {
    ActiveUcodePlan *active = active_ucode_plan;
    if (active == nullptr) {
        throw std::runtime_error(
            "RT64 microcode load occurred without an ordered admission plan");
    }
    active->execution_started = true;
    const uint32_t index = active->cursor;
    if (active->pending) {
        poison_ucode_plan(*active, index,
                          "RT64 began a microcode generation before applying the previous admission");
    }
    if (raw_interpreter != active->interpreter) {
        poison_ucode_plan(*active, index,
                          "RT64 ordered microcode-plan interpreter changed");
    }
    if (index >= active->plan->entries.size()) {
        poison_ucode_plan(*active, index,
                          "RT64 observed an extra microcode generation");
    }
    const Fn64Rt64UcodeGeneration &generation = active->plan->entries[index];
    const uint32_t source = reset_from_task != 0U
        ? FN64_RT64_UCODE_SOURCE_TASK_ENTRY
        : FN64_RT64_UCODE_SOURCE_SELF_LOAD;
    const uint32_t masked_text = text_address & 0x00FFFFF8U;
    const uint32_t masked_data = data_address & 0x00FFFFF8U;
    if ((source != generation.source) ||
        (masked_text != generation.text_address) ||
        (masked_data != generation.data_address)) {
        poison_ucode_plan(*active, index,
                          "RT64 microcode generation source or address order disagrees with admission plan");
    }

    RT64::Interpreter *interpreter = active->interpreter;
    uint8_t *rdram = interpreter->state->RDRAM;
    const uint8_t *expected_text =
        active->plan->raw.data() + generation.raw_text_offset;
    const uint8_t *expected_data =
        active->plan->raw.data() + generation.raw_data_offset;
    if ((std::memcmp(rdram + masked_text, expected_text,
                     generation.raw_text_len) != 0) ||
        (std::memcmp(rdram + masked_data, expected_data,
                     generation.raw_data_len) != 0)) {
        poison_ucode_plan(*active, index,
                          "RT64 live microcode recognition bytes disagree with immutable admission plan");
    }

    const bool recognized_was_active = interpreter->hleGBI != nullptr;
    const NativeUcodeIdentity previous_active = recognized_was_active
        ? native_ucode_identity(*interpreter->hleGBI)
        : NativeUcodeIdentity{};
    RT64::GBI *recognized = interpreter->gbiManager.getGBIForUCode(
        rdram, masked_text, masked_data);
    if (recognized == nullptr) {
        poison_ucode_plan(*active, index,
                          "RT64 failed to recognize a preflighted live microcode generation");
    }
    const NativeUcodeIdentity observed = native_ucode_identity(*recognized);
    if (!same_native_ucode_identity(
            observed, active->plan->native_identities[index])) {
        poison_ucode_plan(*active, index,
                          "RT64 live microcode identity or flags disagree with preflight");
    }

    // getGBIForUCode stores per-instance flags in its broad-family cache.
    // Preserve the old active instance until pinned RT64 has flushed it; the
    // paired apply hook publishes the preflighted identity immediately after.
    if (recognized_was_active && (recognized == interpreter->hleGBI)) {
        apply_native_ucode_flags(*recognized, previous_active);
    }
    active->pending_gbi = recognized;
    active->pending_identity = observed;
    active->pending = true;
    return recognized;
}

extern "C" void fn64_rt64_ucode_generation_apply(
    void *raw_interpreter,
    void *raw_gbi) {
    ActiveUcodePlan *active = active_ucode_plan;
    if (active == nullptr) {
        throw std::runtime_error(
            "RT64 microcode apply occurred without an ordered admission plan");
    }
    const uint32_t index = active->cursor;
    if ((raw_interpreter != active->interpreter) || !active->pending ||
        (raw_gbi != active->pending_gbi)) {
        poison_ucode_plan(*active, index,
                          "RT64 microcode apply disagrees with the observed admission generation");
    }
    RT64::GBI *recognized = active->pending_gbi;
    apply_native_ucode_flags(*recognized, active->pending_identity);
    active->interpreter->hleGBI = recognized;
    active->interpreter->state->rsp->setGBI(recognized);
    active->pending_gbi = nullptr;
    active->pending = false;
    active->cursor++;
    active->result->observed_count = active->cursor;
    if (index == 0U) {
        active->result->entry_gbi_available = 1U;
    }
}

#if defined(FN64_RT64_HFR_EVIDENCE)
extern "C" void fn64_rt64_hfr_present_call_observe(
    void *device,
    uint64_t present_id,
    uint32_t burst_ordinal,
    uint32_t burst_count,
    uint32_t original_refresh_rate,
    uint32_t target_refresh_rate,
    uint32_t phase,
    uint32_t swapchain_valid) noexcept {
    try {
        std::scoped_lock registry_lock(hfr_pacing_registry_mutex);
        const auto entry = std::find_if(
            hfr_pacing_contexts.begin(),
            hfr_pacing_contexts.end(),
            [device](const Fn64Rt64Context *context) {
                return context->application &&
                       (context->application->device.get() == device);
            });
        if (entry == hfr_pacing_contexts.end()) {
            return;
        }

        Fn64Rt64Context *context = *entry;
        std::scoped_lock capture_lock(context->present_capture_mutex);
        if (!context->hfr_pacing_recording) {
            return;
        }
        const uint64_t now_ns = static_cast<uint64_t>(
            std::chrono::duration_cast<std::chrono::nanoseconds>(
                std::chrono::steady_clock::now().time_since_epoch())
                .count());
        auto fail = [context](const char *message) {
            context->hfr_pacing_error = message;
            context->hfr_pacing_recording = false;
            context->hfr_pacing_pending = false;
        };

        if (phase == 0U) {
            if (context->hfr_pacing_pending) {
                fail("RT64 HFR pacing observer received nested present-call start");
                return;
            }
            if (context->hfr_pacing_sample_count >=
                context->hfr_pacing_samples.size()) {
                fail("RT64 HFR pacing observation exceeded bounded capacity");
                return;
            }
            if ((present_id == 0U) || (burst_count == 0U) ||
                (burst_ordinal >= burst_count) ||
                (original_refresh_rate == 0U) ||
                (target_refresh_rate == 0U)) {
                fail("RT64 HFR pacing observer received invalid present metadata");
                return;
            }
            Fn64Rt64HfrPacingSample &sample =
                context->hfr_pacing_samples[context->hfr_pacing_sample_count];
            sample = Fn64Rt64HfrPacingSample{};
            sample.call_start_monotonic_ns = now_ns;
            sample.present_id = present_id;
            sample.burst_ordinal = burst_ordinal;
            sample.burst_count = burst_count;
            sample.original_refresh_rate = original_refresh_rate;
            sample.target_refresh_rate = target_refresh_rate;
            context->hfr_pacing_pending = true;
            return;
        }
        if (phase != 1U) {
            fail("RT64 HFR pacing observer received an unknown phase");
            return;
        }
        if (!context->hfr_pacing_pending ||
            (context->hfr_pacing_sample_count >=
             context->hfr_pacing_samples.size())) {
            fail("RT64 HFR pacing observer received an unpaired present-call return");
            return;
        }
        Fn64Rt64HfrPacingSample &sample =
            context->hfr_pacing_samples[context->hfr_pacing_sample_count];
        if ((sample.present_id != present_id) ||
            (sample.burst_ordinal != burst_ordinal) ||
            (sample.burst_count != burst_count) ||
            (sample.original_refresh_rate != original_refresh_rate) ||
            (sample.target_refresh_rate != target_refresh_rate)) {
            fail("RT64 HFR pacing present-call start/return metadata changed");
            return;
        }
        sample.call_return_monotonic_ns = now_ns;
        sample.swapchain_valid = swapchain_valid;
        context->hfr_pacing_sample_count++;
        context->hfr_pacing_pending = false;
    } catch (...) {
        // The observer crosses RT64's present loop and must never unwind into
        // it. A lost sample is rejected by the exact count in the reader.
    }
}
#endif

namespace {
void capture_present_draw(
    plume::RenderCommandList *command_list,
    plume::RenderFramebuffer *framebuffer) noexcept {
    enum class CaptureBackend { Vulkan, Metal, D3D12 };
    plume::RenderDevice *device = nullptr;
    const plume::RenderTexture *source_texture = nullptr;
    plume::RenderFormat source_format = plume::RenderFormat::UNKNOWN;
    uint32_t source_width = 0;
    uint32_t source_height = 0;
    CaptureBackend backend = CaptureBackend::Vulkan;
#if defined(__APPLE__)
    MTL::Texture *metal_source_native = nullptr;
#endif

    if (auto *vulkan_framebuffer =
            dynamic_cast<plume::VulkanFramebuffer *>(framebuffer)) {
        auto *vulkan_list =
            dynamic_cast<plume::VulkanCommandList *>(command_list);
        if ((vulkan_list == nullptr) ||
            (vulkan_framebuffer->colorAttachments.size() != 1U) ||
            (vulkan_framebuffer->colorAttachments[0] == nullptr)) {
            return;
        }
        const plume::VulkanTexture *texture =
            vulkan_framebuffer->colorAttachments[0];
        device = vulkan_framebuffer->device;
        source_texture = texture;
        source_format = texture->desc.format;
        source_width = vulkan_framebuffer->width;
        source_height = vulkan_framebuffer->height;
        backend = CaptureBackend::Vulkan;
    }
#if defined(__APPLE__)
    else if (auto *metal_framebuffer =
                 dynamic_cast<plume::MetalFramebuffer *>(framebuffer)) {
        auto *metal_list = dynamic_cast<plume::MetalCommandList *>(command_list);
        if ((metal_list == nullptr) ||
            (metal_framebuffer->colorAttachments.size() != 1U) ||
            (metal_framebuffer->colorAttachments[0].texture == nullptr)) {
            return;
        }
        const plume::MetalAttachment &attachment =
            metal_framebuffer->colorAttachments[0];
        device = metal_list->device;
        source_texture = attachment.texture;
        source_format = attachment.format;
        source_width = attachment.width;
        source_height = attachment.height;
        metal_source_native = attachment.getTexture();
        backend = CaptureBackend::Metal;
    }
#elif defined(_WIN32)
    else if (auto *d3d_framebuffer =
                 dynamic_cast<plume::D3D12Framebuffer *>(framebuffer)) {
        auto *d3d_list = dynamic_cast<plume::D3D12CommandList *>(command_list);
        if ((d3d_list == nullptr) ||
            (d3d_framebuffer->colorTargets.size() != 1U) ||
            (d3d_framebuffer->colorTargets[0] == nullptr)) {
            return;
        }
        const plume::D3D12Texture *texture = d3d_framebuffer->colorTargets[0];
        device = d3d_framebuffer->device;
        source_texture = texture;
        source_format = texture->desc.format;
        source_width = d3d_framebuffer->width;
        source_height = d3d_framebuffer->height;
        backend = CaptureBackend::D3D12;
    }
#endif
    else {
        return;
    }

    uint32_t capture_graphics_api = 0;
    switch (backend) {
    case CaptureBackend::D3D12:
        capture_graphics_api = FN64_RT64_PRESENT_GRAPHICS_API_D3D12;
        break;
    case CaptureBackend::Vulkan:
        capture_graphics_api = FN64_RT64_PRESENT_GRAPHICS_API_VULKAN;
        break;
    case CaptureBackend::Metal:
        capture_graphics_api = FN64_RT64_PRESENT_GRAPHICS_API_METAL;
        break;
    }

    std::scoped_lock registry_lock(present_capture_registry_mutex);
    const auto entry = std::find_if(
        present_capture_contexts.begin(),
        present_capture_contexts.end(),
        [device](const Fn64Rt64Context *context) {
            return context->application &&
                   (context->application->device.get() == device);
        });
    if (entry == present_capture_contexts.end()) {
        return;
    }

    Fn64Rt64Context *context = *entry;
    const char *capture_stage = "capture-lock";
    try {
        std::scoped_lock capture_lock(context->present_capture_mutex);
        capture_stage = "attachment-validation";
        context->present_capture_error.clear();
        if ((source_texture == nullptr) || (device == nullptr)) {
            context->present_capture_error =
                "RT64 present framebuffer has no readable color attachment";
            return;
        }
        uint32_t capture_format = 0;
        if (source_format == plume::RenderFormat::B8G8R8A8_UNORM) {
            capture_format = FN64_RT64_PRESENT_FORMAT_BGRA8_UNORM;
        }
        else if (source_format == plume::RenderFormat::R8G8B8A8_UNORM) {
            capture_format = FN64_RT64_PRESENT_FORMAT_RGBA8_UNORM;
        }
        else {
            context->present_capture_error =
                "RT64 present framebuffer is neither BGRA8 nor RGBA8 UNORM";
            return;
        }
        if ((source_width == 0U) || (source_height == 0U)) {
            context->present_capture_error =
                "RT64 present framebuffer has zero dimensions";
            return;
        }
        const uint64_t tight_row_bytes =
            static_cast<uint64_t>(source_width) * 4U;
        uint64_t alignment = 4U;
#if defined(__APPLE__)
        if (backend == CaptureBackend::Metal) {
            if (metal_source_native == nullptr) {
                context->present_capture_error =
                    "RT64 present framebuffer has no Metal texture";
                return;
            }
            const auto *metal_device = static_cast<plume::MetalDevice *>(device);
            alignment = metal_device->mtl
                ->minimumLinearTextureAlignmentForPixelFormat(
                    metal_source_native->pixelFormat());
        }
#endif
#if defined(_WIN32)
        if (backend == CaptureBackend::D3D12) {
            alignment = D3D12_TEXTURE_DATA_PITCH_ALIGNMENT;
        }
#endif
        if (backend == CaptureBackend::Vulkan) {
            const auto *vulkan_device = static_cast<plume::VulkanDevice *>(device);
            alignment = std::max<uint64_t>(
                4U,
                vulkan_device->physicalDeviceProperties.limits
                    .optimalBufferCopyRowPitchAlignment);
        }
        if (alignment == 0U) {
            context->present_capture_error =
                "RT64 present framebuffer returned zero row-pitch alignment";
            return;
        }
        const uint64_t row_pitch = ((tight_row_bytes + alignment - 1U) / alignment) * alignment;
        if ((row_pitch > std::numeric_limits<uint32_t>::max()) ||
            (source_height > (std::numeric_limits<uint64_t>::max() / row_pitch))) {
            context->present_capture_error = "RT64 present framebuffer readback size overflows";
            return;
        }
        const uint64_t buffer_size = row_pitch * source_height;
        capture_stage = "readback-allocation";
        if ((context->present_capture_buffer == nullptr) ||
            (context->present_capture_buffer_size != buffer_size)) {
            context->present_capture_buffer = device->createBuffer(
                plume::RenderBufferDesc::ReadbackBuffer(buffer_size));
            context->present_capture_buffer_size = buffer_size;
        }
        if (context->present_capture_buffer == nullptr) {
            context->present_capture_error =
                "RT64 present readback-buffer allocation failed";
            return;
        }

        ExtendedPresentCaptureSlot *history_slot = nullptr;
        uint32_t *history_count = nullptr;
        bool *history_recording = nullptr;
        const char *history_name = nullptr;
        if (context->extended_present_capture_recording) {
            history_count = &context->extended_present_capture_count;
            history_recording = &context->extended_present_capture_recording;
            history_name = "Extended";
        }
#if defined(FN64_RT64_HFR_EVIDENCE)
        else if (context->hfr_present_capture_recording) {
            history_count = &context->hfr_present_capture_count;
            history_recording = &context->hfr_present_capture_recording;
            history_name = "HFR";
        }
#endif
        if (history_count != nullptr) {
            if ((*history_count) >= FN64_RT64_EXTENDED_MAX_GENERATED_PRESENTS) {
                context->present_capture_error = std::string("RT64 ") + history_name +
                    " present-capture history exceeds evidence capacity";
                *history_recording = false;
                return;
            }
            if (history_name[0] == 'E') {
                history_slot = &context->extended_present_captures[*history_count];
            }
#if defined(FN64_RT64_HFR_EVIDENCE)
            else {
                history_slot = &context->hfr_present_captures[*history_count];
            }
#endif
            history_slot->buffer = device->createBuffer(
                plume::RenderBufferDesc::ReadbackBuffer(buffer_size));
            history_slot->buffer_size = buffer_size;
            if (history_slot->buffer == nullptr) {
                context->present_capture_error = std::string("RT64 ") + history_name +
                    " present-history buffer allocation failed";
                *history_recording = false;
                return;
            }
        }

        std::array<plume::RenderBuffer *, 2> destinations = {
            context->present_capture_buffer.get(),
            (history_slot != nullptr) ? history_slot->buffer.get() : nullptr,
        };
#if defined(__APPLE__)
        if (backend == CaptureBackend::Metal) {
            capture_stage = "metal-encoder-setup";
            auto *metal_list = static_cast<plume::MetalCommandList *>(command_list);
            metal_list->endOtherEncoders(plume::EncoderType::Blit);
            metal_list->checkActiveBlitEncoder();
            metal_list->activeType = plume::EncoderType::Blit;
            for (plume::RenderBuffer *buffer : destinations) {
                if (buffer == nullptr) {
                    continue;
                }
                auto *destination = static_cast<plume::MetalBuffer *>(buffer);
                capture_stage = "metal-copy-encoding";
                metal_list->activeBlitEncoder->copyFromTexture(
                    metal_source_native,
                    0,
                    0,
                    MTL::Origin(0, 0, 0),
                    MTL::Size(source_width, source_height, 1),
                    destination->mtl,
                    0,
                    row_pitch,
                    buffer_size);
            }
        }
        else
#endif
        if (backend == CaptureBackend::Vulkan) {
            auto *vulkan_list = static_cast<plume::VulkanCommandList *>(command_list);
            auto *vulkan_source =
                const_cast<plume::VulkanTexture *>(
                    static_cast<const plume::VulkanTexture *>(source_texture));
            const plume::RenderTextureLayout previous_layout =
                vulkan_source->textureLayout;
            static_cast<plume::RenderCommandList *>(vulkan_list)->barriers(
                plume::RenderBarrierStage::COPY,
                plume::RenderTextureBarrier(
                    vulkan_source,
                    plume::RenderTextureLayout::COPY_SOURCE));
            for (plume::RenderBuffer *buffer : destinations) {
                if (buffer == nullptr) {
                    continue;
                }
                auto *destination = static_cast<plume::VulkanBuffer *>(buffer);
                VkBufferImageCopy region{};
                region.bufferOffset = 0;
                region.bufferRowLength = static_cast<uint32_t>(row_pitch / 4U);
                region.bufferImageHeight = source_height;
                region.imageSubresource.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
                region.imageSubresource.layerCount = 1;
                region.imageExtent = {source_width, source_height, 1};
                vkCmdCopyImageToBuffer(
                    vulkan_list->vk,
                    vulkan_source->vk,
                    VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
                    destination->vk,
                    1,
                    &region);
            }
            static_cast<plume::RenderCommandList *>(vulkan_list)->barriers(
                plume::RenderBarrierStage::COPY,
                plume::RenderTextureBarrier(vulkan_source, previous_layout));
        }
#if defined(_WIN32)
        else if (backend == CaptureBackend::D3D12) {
            auto *d3d_list = static_cast<plume::D3D12CommandList *>(command_list);
            auto *d3d_source = const_cast<plume::D3D12Texture *>(
                static_cast<const plume::D3D12Texture *>(source_texture));
            const plume::RenderTextureLayout previous_layout = d3d_source->layout;
            static_cast<plume::RenderCommandList *>(d3d_list)->barriers(
                plume::RenderBarrierStage::COPY,
                plume::RenderTextureBarrier(
                    d3d_source,
                    plume::RenderTextureLayout::COPY_SOURCE));
            for (plume::RenderBuffer *buffer : destinations) {
                if (buffer == nullptr) {
                    continue;
                }
                auto *destination = static_cast<plume::D3D12Buffer *>(buffer);
                D3D12_TEXTURE_COPY_LOCATION destination_location{};
                destination_location.pResource = destination->d3d;
                destination_location.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
                destination_location.PlacedFootprint.Offset = 0;
                destination_location.PlacedFootprint.Footprint.Format =
                    (source_format == plume::RenderFormat::B8G8R8A8_UNORM)
                        ? DXGI_FORMAT_B8G8R8A8_UNORM
                        : DXGI_FORMAT_R8G8B8A8_UNORM;
                destination_location.PlacedFootprint.Footprint.Width = source_width;
                destination_location.PlacedFootprint.Footprint.Height = source_height;
                destination_location.PlacedFootprint.Footprint.Depth = 1;
                destination_location.PlacedFootprint.Footprint.RowPitch =
                    static_cast<UINT>(row_pitch);
                D3D12_TEXTURE_COPY_LOCATION source_location{};
                source_location.pResource = d3d_source->d3d;
                source_location.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
                source_location.SubresourceIndex = 0;
                d3d_list->d3d->CopyTextureRegion(
                    &destination_location,
                    0,
                    0,
                    0,
                    &source_location,
                    nullptr);
            }
            static_cast<plume::RenderCommandList *>(d3d_list)->barriers(
                plume::RenderBarrierStage::COPY,
                plume::RenderTextureBarrier(d3d_source, previous_layout));
        }
#endif

        if (history_slot != nullptr) {
            // Every generated draw needs a distinct destination. Reusing the
            // ordinary last-present buffer lets draw N+1 enqueue an overwrite
            // before Rust maps draw N after the present-queue idle boundary.
            // This per-draw allocation closes that exact publication
            // interleaving; the evidence reader maps only after queue idle.
            history_slot->capture_generation =
                context->present_capture_generation + 1U;
            history_slot->width = source_width;
            history_slot->height = source_height;
            history_slot->row_pitch = static_cast<uint32_t>(row_pitch);
            history_slot->format = capture_format;
            (*history_count)++;
        }

        context->present_capture_width = source_width;
        context->present_capture_height = source_height;
        context->present_capture_row_pitch = static_cast<uint32_t>(row_pitch);
        context->present_capture_format = capture_format;
        context->present_capture_graphics_api = capture_graphics_api;
        capture_stage = "capture-publication";
        context->present_capture_generation++;
    } catch (const std::exception &exception) {
        std::scoped_lock capture_lock(context->present_capture_mutex);
        context->present_capture_error = std::string("RT64 present capture threw: ") + exception.what();
    } catch (...) {
        std::scoped_lock capture_lock(context->present_capture_mutex);
        context->present_capture_error =
            std::string("RT64 present capture failed with an unknown C++ exception during ") +
            capture_stage;
    }
}

// The function RT64::SetRenderHooks actually installs. Stage 1 is
// present-capture's own readback, unchanged, so it keeps seeing a UI-free
// frame. Stage 2 runs every external registrant (e.g. fn64-rmlui's overlay)
// in registration order, strictly after stage 1's copy commands are
// recorded into the same command list.
//
// The registrants are snapshot-copied out from under `draw_hook_registry_mutex`
// before any of them run, rather than holding that mutex across the calls.
// Each registrant is foreign code (potentially reentering into Rust) that
// this function does not control the duration of, and `draw_hook_registry_mutex`
// is the same mutex a caller's register/unregister call takes from a
// different thread -- holding it across an unbounded foreign call invites
// contention or deadlock for no benefit. `capture_present_draw` above holds
// its own registry mutex for its whole (short, internal, bounded) body; that
// shape does not extend safely to callbacks this function does not own.
void draw_hook_dispatch(
    plume::RenderCommandList *command_list,
    plume::RenderFramebuffer *framebuffer) noexcept {
    capture_present_draw(command_list, framebuffer);

    std::vector<DrawHookRegistrant> registrants;
    {
        std::scoped_lock registry_lock(draw_hook_registry_mutex);
        registrants = draw_hook_registrants;
    }
    for (const DrawHookRegistrant &registrant : registrants) {
        try {
            registrant.callback(command_list, framebuffer, registrant.user_data);
        } catch (...) {
            // Crosses RT64's present loop, same discipline as
            // capture_present_draw above: never unwind into it. An overlay
            // registrant has no per-context error slot to report into the
            // way present-capture does, since it owns none of this file's
            // state; a registrant that needs error reporting is responsible
            // for capturing its own failures inside its callback.
        }
    }
}

void register_vi_history_rate(const void *history, uint32_t nominal_refresh_rate) {
    std::scoped_lock lock(vi_history_rate_registry_mutex);
    if ((nominal_refresh_rate != 50U) && (nominal_refresh_rate != 60U)) {
        throw std::runtime_error("RT64 VI history requires a 50 or 60 Hz TV standard");
    }
    const auto existing = std::find_if(
        vi_history_rate_entries.begin(),
        vi_history_rate_entries.end(),
        [history](const ViHistoryRateEntry &entry) {
            return entry.history == history;
        });
    if (existing != vi_history_rate_entries.end()) {
        throw std::runtime_error("RT64 VI history TV standard was registered twice");
    }
    vi_history_rate_entries.push_back({history, nominal_refresh_rate});
}

void unregister_vi_history_rate(const void *history) {
    std::scoped_lock lock(vi_history_rate_registry_mutex);
    const size_t prior_size = vi_history_rate_entries.size();
    vi_history_rate_entries.erase(
        std::remove_if(
            vi_history_rate_entries.begin(),
            vi_history_rate_entries.end(),
            [history](const ViHistoryRateEntry &entry) {
                return entry.history == history;
            }),
        vi_history_rate_entries.end());
    if (vi_history_rate_entries.size() + 1U != prior_size) {
        std::terminate();
    }
}

// present_capture_contexts and draw_hook_registrants share one underlying
// RT64 hook slot (RT64::SetRenderHooks has exactly one draw callback, which
// is draw_hook_dispatch, dispatching to both). The slot may only be torn
// down once BOTH registries are empty -- this is the one place the two
// cannot stay fully independent. Each unregister path (here and
// unregister_overlay_draw below) releases its OWN registry's mutex before
// checking the other registry, so no caller ever holds both
// present_capture_registry_mutex and draw_hook_registry_mutex at the same
// time -- there is exactly one mutex held at any instant on this path, so
// no lock-ordering cycle is possible between the two.
bool draw_hook_slot_still_needed() {
    std::scoped_lock draw_hook_lock(draw_hook_registry_mutex);
    return !draw_hook_registrants.empty();
}

void unregister_present_capture(Fn64Rt64Context *context) {
    bool present_capture_now_empty = false;
    {
        std::scoped_lock registry_lock(present_capture_registry_mutex);
        if (!context->present_capture_enabled) {
            return;
        }
        present_capture_contexts.erase(
            std::remove(
                present_capture_contexts.begin(),
                present_capture_contexts.end(),
                context),
            present_capture_contexts.end());
        context->present_capture_enabled = false;
        present_capture_now_empty = present_capture_contexts.empty();
    }
    if (present_capture_now_empty &&
        !draw_hook_slot_still_needed() &&
        (RT64::GetRenderHookInit() == nullptr) &&
        (RT64::GetRenderHookDraw() == draw_hook_dispatch) &&
        (RT64::GetRenderHookDeinit() == nullptr)) {
        RT64::SetRenderHooks(nullptr, nullptr, nullptr);
    }
}

void unregister_overlay_draw(Fn64Rt64Context *context) {
    bool draw_hook_now_empty = false;
    {
        std::scoped_lock registry_lock(draw_hook_registry_mutex);
        const size_t prior_size = draw_hook_registrants.size();
        draw_hook_registrants.erase(
            std::remove_if(
                draw_hook_registrants.begin(),
                draw_hook_registrants.end(),
                [context](const DrawHookRegistrant &registrant) {
                    return registrant.context == context;
                }),
            draw_hook_registrants.end());
        if (draw_hook_registrants.size() == prior_size) {
            // Not registered; matches present-capture's idempotent-unregister
            // shape (an early return above for the already-disabled case).
            return;
        }
        draw_hook_now_empty = draw_hook_registrants.empty();
    }
    if (draw_hook_now_empty) {
        std::scoped_lock present_capture_lock(present_capture_registry_mutex);
        if (present_capture_contexts.empty() &&
            (RT64::GetRenderHookInit() == nullptr) &&
            (RT64::GetRenderHookDraw() == draw_hook_dispatch) &&
            (RT64::GetRenderHookDeinit() == nullptr)) {
            RT64::SetRenderHooks(nullptr, nullptr, nullptr);
        }
    }
}

#if defined(FN64_RT64_HFR_EVIDENCE)
void unregister_hfr_pacing(Fn64Rt64Context *context) {
    std::scoped_lock registry_lock(hfr_pacing_registry_mutex);
    hfr_pacing_contexts.erase(
        std::remove(
            hfr_pacing_contexts.begin(),
            hfr_pacing_contexts.end(),
            context),
        hfr_pacing_contexts.end());
    context->hfr_pacing_recording = false;
    context->hfr_pacing_pending = false;
}
#endif
} // namespace

extern "C" int fn64_rt64_capture_adapter_inputs(
    const Fn64Rt64Task *task,
    uint32_t output_addr,
    uint32_t width,
    uint32_t height,
    const Fn64Rt64ViState *vi,
    Fn64Rt64AdapterCapture *capture,
    char *error,
    size_t error_capacity) {
    try {
        if ((task == nullptr) || (vi == nullptr) || (capture == nullptr)) {
            set_error(error, error_capacity, "null OSTask, VI-state, or adapter-capture pointer");
            return 0;
        }
        if ((width == 0U) || (height == 0U)) {
            set_error(error, error_capacity, "capture dimensions must be non-zero");
            return 0;
        }

        *capture = Fn64Rt64AdapterCapture{};
        capture->task = *task;
        capture->output_addr = output_addr & 0x00FFFFFFU;
        capture->width = width;
        capture->height = height;
        // This scalar-only capture context never creates an RT64 State or
        // performs logical-rate inference; the nominal value is intentionally
        // inert but remains explicit so production contexts cannot default it.
        Fn64Rt64Context context(width, height, 60U);
        context.output_addr = capture->output_addr;
        context.update_vi(*vi);
        capture->aa_mode_specified = context.vi_state.aa_mode_specified;
        capture->vi_filter_flags = vi_filter_flags_for_context(context);
        capture->noise_seed_low = uint32_t(context.vi_state.noise_seed);
        capture->noise_seed_high = uint32_t(context.vi_state.noise_seed >> 32U);
        std::copy(context.registers.begin(), context.registers.end(), capture->registers);
        // Every HLE/raw submission and resize takes this no-argument path.
        // It must refresh address aliases without replacing the last complete
        // presentation-owned VI register image.
        context.update_vi();
        std::copy(
            context.registers.begin(),
            context.registers.end(),
            capture->registers_after_submission);
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 adapter capture threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 adapter capture failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_probe_logical_rate(
    uint32_t nominal_refresh_rate,
    uint32_t factor,
    uint32_t *logical_rate,
    char *error,
    size_t error_capacity) {
    try {
        if (logical_rate == nullptr) {
            set_error(error, error_capacity, "null logical-rate output pointer");
            return 0;
        }
        if ((nominal_refresh_rate != 50U) && (nominal_refresh_rate != 60U)) {
            set_error(error, error_capacity, "nominal TV refresh rate must be 50 or 60 Hz");
            return 0;
        }
        if (factor == 0U) {
            set_error(error, error_capacity, "VI presentation factor must be non-zero");
            return 0;
        }

        RT64::VIHistory history;
        register_vi_history_rate(&history, nominal_refresh_rate);
        struct ViHistoryRateRegistration {
            const void *history;
            ~ViHistoryRateRegistration() {
                unregister_vi_history_rate(history);
            }
        } registration{&history};

        history.pushFactor(factor);
        history.pushFactor(factor);
        history.pushFactor(factor);
        *logical_rate = history.logicalRateFromFactors();
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 logical-rate probe threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 logical-rate probe failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_roundtrip_user_config(
    const Fn64Rt64UserConfig *input,
    Fn64Rt64UserConfig *output,
    char *error,
    size_t error_capacity) {
    try {
        if (output == nullptr) {
            set_error(error, error_capacity, "null RT64 user-config output pointer");
            return 0;
        }
        RT64::UserConfiguration decoded;
        if (!decode_user_config(input, decoded, error, error_capacity)) {
            return 0;
        }
        *output = encode_user_config(decoded);
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 user-config roundtrip threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 user-config roundtrip failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_roundtrip_enhancement_config(
    const Fn64Rt64EnhancementConfig *input,
    Fn64Rt64EnhancementConfig *output,
    char *error,
    size_t error_capacity) {
    try {
        if (output == nullptr) {
            set_error(error, error_capacity, "null RT64 enhancement-config output pointer");
            return 0;
        }
        RT64::EnhancementConfiguration decoded;
        if (!decode_enhancement_config(input, decoded, error, error_capacity)) {
            return 0;
        }
        *output = encode_enhancement_config(decoded);
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 enhancement-config roundtrip threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 enhancement-config roundtrip failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_roundtrip_emulator_config(
    const Fn64Rt64EmulatorConfig *input,
    Fn64Rt64EmulatorConfig *output,
    char *error,
    size_t error_capacity) {
    try {
        if (output == nullptr) {
            set_error(error, error_capacity, "null RT64 emulator-config output pointer");
            return 0;
        }
        RT64::EmulatorConfiguration decoded;
        if (!decode_emulator_config(input, decoded, error, error_capacity)) {
            return 0;
        }
        *output = encode_emulator_config(decoded);
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 emulator-config roundtrip threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 emulator-config roundtrip failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_inspect_replacement_pack(
    const char *path_utf8,
    Fn64Rt64ReplacementDatabaseConfig *config,
    uint8_t *database_bytes,
    size_t database_capacity,
    size_t *database_size,
    char *error,
    size_t error_capacity) {
    try {
        if ((config == nullptr) || (database_size == nullptr) ||
            ((database_bytes == nullptr) != (database_capacity == 0))) {
            set_error(error, error_capacity, "invalid replacement-pack inspection output pointers");
            return 0;
        }
        std::vector<uint8_t> inspected_bytes;
        if (!inspect_replacement_pack(path_utf8, *config, inspected_bytes, error, error_capacity)) {
            return 0;
        }
        *database_size = inspected_bytes.size();
        if (database_capacity == 0) {
            return 1;
        }
        if (database_capacity < inspected_bytes.size()) {
            set_error(error, error_capacity, "replacement database output buffer is too small");
            return 0;
        }
        std::memcpy(database_bytes, inspected_bytes.data(), inspected_bytes.size());
        return 1;
    }
    catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("replacement-pack inspection threw: ") + exception.what());
        return 0;
    }
    catch (...) {
        set_error(error, error_capacity, "replacement-pack inspection failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" Fn64Rt64Context *fn64_rt64_create(
    uint32_t width,
    uint32_t height,
    uint32_t nominal_refresh_rate,
    const Fn64Rt64UserConfig *user_config,
    const Fn64Rt64EnhancementConfig *enhancement_config,
    const Fn64Rt64EmulatorConfig *emulator_config,
    char *error,
    size_t error_capacity) {
    try {
        if ((width == 0) || (height == 0)) {
            set_error(error, error_capacity, "render dimensions must be non-zero");
            return nullptr;
        }
        if ((nominal_refresh_rate != 50U) && (nominal_refresh_rate != 60U)) {
            set_error(error, error_capacity, "nominal TV refresh rate must be 50 or 60 Hz");
            return nullptr;
        }

        RT64::UserConfiguration requested_user_config;
        if (!decode_user_config(user_config, requested_user_config, error, error_capacity)) {
            return nullptr;
        }
        RT64::EnhancementConfiguration requested_enhancement_config;
        if (!decode_enhancement_config(enhancement_config, requested_enhancement_config, error, error_capacity)) {
            return nullptr;
        }
        RT64::EmulatorConfiguration requested_emulator_config;
        if (!decode_emulator_config(emulator_config, requested_emulator_config, error, error_capacity)) {
            return nullptr;
        }

        auto context = std::make_unique<Fn64Rt64Context>(
            width,
            height,
            nominal_refresh_rate);
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
        context->application->userConfig = requested_user_config;
        context->application->enhancementConfig = requested_enhancement_config;
        context->application->emulatorConfig = requested_emulator_config;

        const RT64::Application::SetupResult result = context->application->setup(0);
        if (result != RT64::Application::SetupResult::Success) {
            set_error(
                error,
                error_capacity,
                std::string("RT64 setup failed: ") + setup_result_name(result));
            return nullptr;
        }

        context->setup_complete = true;
        register_vi_history_rate(
            &context->application->state->viHistory,
            context->nominal_refresh_rate);
        context->vi_history_rate_registered = true;
        // Setup has finished assigning Application::device. Publish only this
        // stable pointer so concurrent context creation cannot race a registry
        // scan against unique_ptr mutation or collide on two null devices.
        register_vi_filter_context(context.get());
        context->vi_filter_registered = true;
        if (context->application->userConfig.antialiasing != requested_user_config.antialiasing) {
            set_error(error, error_capacity, "RT64 device silently rejected the requested antialiasing sample count");
            return nullptr;
        }
        return context.release();
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 create threw: ") + exception.what());
        return nullptr;
    } catch (...) {
        set_error(error, error_capacity, "RT64 create failed with an unknown C++ exception");
        return nullptr;
    }
}

extern "C" int fn64_rt64_apply_user_config(
    Fn64Rt64Context *context,
    const Fn64Rt64UserConfig *user_config,
    uint8_t *framebuffers_discarded,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || (framebuffers_discarded == nullptr)) {
            set_error(error, error_capacity, "null RT64 context or settings-result pointer");
            return 0;
        }
        if (!context->setup_complete || !context->application) {
            set_error(error, error_capacity, "RT64 settings apply requires a completed setup");
            return 0;
        }

        RT64::UserConfiguration requested;
        if (!decode_user_config(user_config, requested, error, error_capacity)) {
            return 0;
        }
        const RT64::UserConfiguration active = context->application->userConfig;
        if (!restart_fields_equal(requested, active)) {
            set_error(error, error_capacity, "RT64 graphics API, display buffering, or internal color format requires backend recreation");
            return 0;
        }

        const bool multisampling_changed = requested.antialiasing != active.antialiasing;
        if (multisampling_changed) {
            const RenderSampleCounts supported =
                context->application->device->getSampleCountsSupported(
                    RT64::RenderTarget::colorBufferFormat(context->application->shaderLibrary->usesHDR)) &
                context->application->device->getSampleCountsSupported(
                    RT64::RenderTarget::depthBufferFormat());
            if ((supported & requested.msaaSampleCount()) == 0U) {
                set_error(error, error_capacity, "RT64 device does not support the requested antialiasing sample count");
                return 0;
            }
        }

        const bool discard_for_resolution = resolution_change_discards_framebuffers(requested, active);
        const bool hardware_resolve_changed = requested.hardwareResolve != active.hardwareResolve;
        context->application->userConfig = requested;
        if (multisampling_changed) {
            context->application->updateMultisampling();
        }
        if (hardware_resolve_changed) {
            context->application->shaderLibrary->usesHardwareResolve =
                requested.hardwareResolve != RT64::UserConfiguration::HardwareResolve::Disabled;
        }
        context->application->updateUserConfig(discard_for_resolution);
        context->presentation_refresh_pending = true;
        *framebuffers_discarded = static_cast<uint8_t>(discard_for_resolution || multisampling_changed);
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 settings apply threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 settings apply failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_apply_enhancement_config(
    Fn64Rt64Context *context,
    const Fn64Rt64EnhancementConfig *enhancement_config,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete || !context->application) {
            set_error(error, error_capacity, "RT64 enhancement apply requires a completed setup");
            return 0;
        }
        RT64::EnhancementConfiguration requested;
        if (!decode_enhancement_config(enhancement_config, requested, error, error_capacity)) {
            return 0;
        }
        context->application->enhancementConfig = requested;
        context->application->updateEnhancementConfig();
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 enhancement apply threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 enhancement apply failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_apply_emulator_config(
    Fn64Rt64Context *context,
    const Fn64Rt64EmulatorConfig *emulator_config,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete || !context->application) {
            set_error(error, error_capacity, "RT64 emulator apply requires a completed setup");
            return 0;
        }
        RT64::EmulatorConfiguration requested;
        if (!decode_emulator_config(emulator_config, requested, error, error_capacity)) {
            return 0;
        }
        context->application->emulatorConfig = requested;
        context->application->updateEmulatorConfig();
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 emulator apply threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 emulator apply failed with an unknown C++ exception");
        return 0;
    }
}

namespace {
int apply_replacement_packs(Fn64Rt64Context *context,
                            const Fn64Rt64ReplacementPack *packs,
                            size_t pack_count,
                            uint32_t enabled,
                            char *error,
                            size_t error_capacity) {
    if ((context == nullptr) || !context->setup_complete || !context->application ||
        !context->application->textureCache) {
        set_error(error, error_capacity, "RT64 replacement load requires a completed setup");
        return 0;
    }
    bool decoded_enabled = false;
    if (!decode_policy_bool(enabled, "replacement-config", "enabled", decoded_enabled,
                            error, error_capacity)) {
        return 0;
    }
    std::vector<RT64::ReplacementDirectory> directories;
    if (!decode_replacement_packs(packs, pack_count, directories, error, error_capacity)) {
        return 0;
    }
    RT64::TextureCache *texture_cache = context->application->textureCache.get();
    context->replacement_observed_resolved_not_installed = false;
    if (directories.empty()) {
        texture_cache->clearReplacementDirectories();
    }
    else if (!texture_cache->loadReplacementDirectories(directories)) {
        set_error(error, error_capacity, "RT64 rejected a preflighted replacement-pack set");
        return 0;
    }
    {
        std::unique_lock lock(texture_cache->textureMapMutex);
        for (const auto &hash_entry : texture_cache->textureMap.hashMap) {
            bool resolved = false;
            for (const auto &resolved_paths :
                 texture_cache->textureMap.replacementMap.fileSystemResolvedPaths) {
                if (resolved_paths.find(hash_entry.first) != resolved_paths.end()) {
                    resolved = true;
                    break;
                }
            }
            const bool installed =
                texture_cache->textureMap.textureReplacements[hash_entry.second] != nullptr;
            if (resolved && !installed) {
                context->replacement_observed_resolved_not_installed = true;
                break;
            }
        }
    }
    texture_cache->textureMap.replacementMapEnabled = decoded_enabled;
    return 1;
}
}

extern "C" int fn64_rt64_load_replacement_packs(
    Fn64Rt64Context *context,
    const Fn64Rt64ReplacementPack *packs,
    size_t pack_count,
    uint32_t enabled,
    char *error,
    size_t error_capacity) {
    try {
        return apply_replacement_packs(context, packs, pack_count, enabled, error, error_capacity);
    }
    catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("replacement-pack load threw: ") + exception.what());
        return 0;
    }
    catch (...) {
        set_error(error, error_capacity, "replacement-pack load failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_reload_replacement_packs(
    Fn64Rt64Context *context,
    const Fn64Rt64ReplacementPack *packs,
    size_t pack_count,
    uint32_t enabled,
    char *error,
    size_t error_capacity) {
    return fn64_rt64_load_replacement_packs(
        context, packs, pack_count, enabled, error, error_capacity);
}

extern "C" int fn64_rt64_set_replacement_enabled(
    Fn64Rt64Context *context,
    uint32_t enabled,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete || !context->application ||
            !context->application->textureCache) {
            set_error(error, error_capacity, "RT64 replacement enable requires a completed setup");
            return 0;
        }
        bool decoded_enabled = false;
        if (!decode_policy_bool(enabled, "replacement-config", "enabled", decoded_enabled,
                                error, error_capacity)) {
            return 0;
        }
        context->application->textureCache->textureMap.replacementMapEnabled = decoded_enabled;
        return 1;
    }
    catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("replacement enable apply threw: ") + exception.what());
        return 0;
    }
    catch (...) {
        set_error(error, error_capacity, "replacement enable apply failed with an unknown C++ exception");
        return 0;
    }
}

namespace {
bool read_texture_replacement_state(
    Fn64Rt64Context *context,
    uint64_t requested_hash,
    Fn64Rt64TextureReplacementState &state,
    char *error,
    size_t error_capacity) {
    if ((context == nullptr) || !context->setup_complete ||
        !context->application || !context->application->textureCache) {
        set_error(error, error_capacity,
                  "RT64 texture-replacement evidence requires a completed setup");
        return false;
    }

    RT64::TextureCache *cache = context->application->textureCache.get();
    state = {};
    {
        std::unique_lock lock(cache->textureMapMutex);
        state.texture_count = static_cast<uint32_t>(cache->textureMap.hashMap.size());
        if ((requested_hash == 0) && (state.texture_count > 1)) {
            set_error(error, error_capacity,
                      "RT64 texture-replacement evidence expected one live TMEM texture");
            return false;
        }

        const uint64_t hash = (requested_hash != 0)
            ? requested_hash
            : ((state.texture_count == 1) ? cache->textureMap.hashMap.begin()->first : 0);
        state.texture_hash = hash;
        const auto texture_it = cache->textureMap.hashMap.find(hash);
        state.texture_known = (hash != 0) && (texture_it != cache->textureMap.hashMap.end());
        state.replacements_enabled = cache->textureMap.replacementMapEnabled;

        for (const auto &resolved_paths : cache->textureMap.replacementMap.fileSystemResolvedPaths) {
            if (resolved_paths.find(hash) != resolved_paths.end()) {
                state.replacement_resolved = 1;
                break;
            }
        }

        if (state.texture_known) {
            const uint32_t texture_index = texture_it->second;
            RT64::Texture *replacement = cache->textureMap.textureReplacements[texture_index];
            if (replacement != nullptr) {
                state.replacement_installed = 1;
                state.replacement_mip_levels = replacement->mipmaps;
            }
        }
    }
    {
        std::unique_lock lock(cache->streamDescStackMutex);
        state.stream_queued = static_cast<uint32_t>(cache->streamDescStack.size());
        state.stream_active = static_cast<uint32_t>(std::max(cache->streamDescStackActiveCount, 0));
    }
    {
        std::unique_lock lock(cache->uploadQueueMutex);
        state.stream_results_pending = static_cast<uint32_t>(cache->streamResultQueue.size());
        state.uploads_pending = static_cast<uint32_t>(cache->uploadQueue.size());
        state.resolved_paths_pending = static_cast<uint32_t>(cache->resolvedPathQueue.size());
    }
    {
        std::unique_lock lock(cache->streamPerformanceMutex);
        state.stream_load_count = cache->streamLoadCount;
    }
    state.stream_workers_paused = context->replacement_stream_workers_paused ? 1U : 0U;
    state.stream_worker_count = context->replacement_stream_workers_paused
        ? context->replacement_stream_worker_count
        : static_cast<uint32_t>(cache->streamThreads.size());
    return true;
}
} // namespace

extern "C" int fn64_rt64_set_stream_workers_paused(
    Fn64Rt64Context *context,
    uint32_t paused,
    char *error,
    size_t error_capacity) {
    try {
        bool decoded_paused = false;
        if (!decode_policy_bool(paused, "replacement-evidence", "stream_workers_paused",
                                decoded_paused, error, error_capacity)) {
            return 0;
        }
        if ((context == nullptr) || !context->setup_complete ||
            !context->application || !context->application->textureCache) {
            set_error(error, error_capacity,
                      "RT64 stream-worker evidence control requires a completed setup");
            return 0;
        }

        RT64::TextureCache *cache = context->application->textureCache.get();
        if (decoded_paused == context->replacement_stream_workers_paused) {
            set_error(error, error_capacity,
                      decoded_paused
                          ? "RT64 stream workers are already paused for evidence"
                          : "RT64 stream workers are not paused for evidence");
            return 0;
        }

        if (decoded_paused) {
            {
                std::unique_lock upload_lock(cache->uploadQueueMutex);
                std::unique_lock stream_lock(cache->streamDescStackMutex);
                if (!cache->uploadQueue.empty() || !cache->resolvedPathQueue.empty() ||
                    !cache->streamResultQueue.empty() || !cache->streamDescStack.empty() ||
                    (cache->streamDescStackActiveCount != 0)) {
                    set_error(error, error_capacity,
                              "RT64 stream workers may pause only while upload and stream queues are quiescent");
                    return 0;
                }
            }

            context->replacement_stream_worker_count =
                static_cast<uint32_t>(cache->streamThreads.size());
            if (context->replacement_stream_worker_count == 0) {
                set_error(error, error_capacity,
                          "RT64 texture cache has no stream workers to pause");
                return 0;
            }
            cache->streamThreads.clear();
            {
                std::unique_lock stream_lock(cache->streamDescStackMutex);
                cache->streamDescStackActiveCount = 0;
            }
            context->replacement_stream_workers_paused = true;
        }
        else {
            const uint32_t worker_count = context->replacement_stream_worker_count;
            if ((worker_count == 0) || !cache->streamThreads.empty()) {
                set_error(error, error_capacity,
                          "RT64 paused stream-worker evidence state is inconsistent");
                return 0;
            }
            {
                std::unique_lock stream_lock(cache->streamDescStackMutex);
                if (cache->streamDescStackActiveCount != 0) {
                    set_error(error, error_capacity,
                              "RT64 paused stream-worker active count is not zero");
                    return 0;
                }
                cache->streamDescStackActiveCount = static_cast<int32_t>(worker_count);
            }
            for (uint32_t index = 0; index < worker_count; index++) {
                cache->streamThreads.push_back(
                    std::make_unique<RT64::TextureCache::StreamThread>(cache));
            }
            context->replacement_stream_workers_paused = false;
            cache->streamDescStackChanged.notify_all();
        }
        return 1;
    }
    catch (const std::exception &exception) {
        set_error(error, error_capacity,
                  std::string("stream-worker evidence control threw: ") + exception.what());
        return 0;
    }
    catch (...) {
        set_error(error, error_capacity,
                  "stream-worker evidence control failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_wait_stream_fallback_state(
    Fn64Rt64Context *context,
    uint64_t texture_hash,
    Fn64Rt64TextureReplacementState *state,
    char *error,
    size_t error_capacity) {
    try {
        if (state == nullptr) {
            set_error(error, error_capacity,
                      "RT64 stream-fallback evidence output is null");
            return 0;
        }

        uint64_t previous_progress = std::numeric_limits<uint64_t>::max();
        uint32_t unchanged_iterations = 0;
        constexpr uint32_t MaxUnchangedIterations = 10'000'000;
        for (;;) {
            Fn64Rt64TextureReplacementState current{};
            if (!read_texture_replacement_state(context, texture_hash, current,
                                                error, error_capacity)) {
                return 0;
            }
            if (!current.stream_workers_paused) {
                set_error(error, error_capacity,
                          "RT64 stream-fallback evidence requires paused stream workers");
                return 0;
            }
            if (current.texture_known && current.replacement_resolved &&
                !current.replacement_installed && (current.stream_queued > 0) &&
                (current.stream_active == 0) && (current.stream_load_count == 0)) {
                current.observed_resolved_not_installed = 1;
                *state = current;
                return 1;
            }

            const uint64_t progress =
                (static_cast<uint64_t>(current.stream_queued) << 32U) ^
                (static_cast<uint64_t>(current.resolved_paths_pending) << 16U) ^
                (static_cast<uint64_t>(current.texture_known) << 63U) ^
                (static_cast<uint64_t>(current.replacement_resolved) << 62U);
            if (progress == previous_progress) {
                unchanged_iterations++;
                if (unchanged_iterations >= MaxUnchangedIterations) {
                    set_error(error, error_capacity,
                              "RT64 stream-fallback evidence exceeded the deterministic no-progress iteration cap");
                    return 0;
                }
            }
            else {
                previous_progress = progress;
                unchanged_iterations = 0;
            }
            std::this_thread::yield();
        }
    }
    catch (const std::exception &exception) {
        set_error(error, error_capacity,
                  std::string("stream-fallback evidence threw: ") + exception.what());
        return 0;
    }
    catch (...) {
        set_error(error, error_capacity,
                  "stream-fallback evidence failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_wait_texture_replacement_state(
    Fn64Rt64Context *context,
    uint64_t texture_hash,
    uint32_t require_replacement,
    Fn64Rt64TextureReplacementState *state,
    char *error,
    size_t error_capacity) {
    try {
        bool decoded_require_replacement = false;
        if ((state == nullptr) ||
            !decode_policy_bool(require_replacement, "replacement-evidence",
                                "require_replacement", decoded_require_replacement,
                                error, error_capacity)) {
            if (state == nullptr) {
                set_error(error, error_capacity,
                          "RT64 texture-replacement evidence output is null");
            }
            return 0;
        }

        bool observed_resolved_not_installed =
            context->replacement_observed_resolved_not_installed;
        uint64_t previous_progress = std::numeric_limits<uint64_t>::max();
        uint32_t unchanged_iterations = 0;
        constexpr uint32_t MaxUnchangedIterations = 10'000'000;
        for (;;) {
            Fn64Rt64TextureReplacementState current{};
            if (!read_texture_replacement_state(context, texture_hash, current,
                                                error, error_capacity)) {
                return 0;
            }
            observed_resolved_not_installed |=
                current.texture_known && current.replacement_resolved &&
                !current.replacement_installed;
            current.observed_resolved_not_installed =
                observed_resolved_not_installed ? 1U : 0U;
            if (current.texture_known &&
                (!decoded_require_replacement || current.replacement_installed)) {
                *state = current;
                return 1;
            }

            const uint64_t progress =
                (current.stream_load_count << 32U) ^
                (static_cast<uint64_t>(current.stream_queued) << 24U) ^
                (static_cast<uint64_t>(current.stream_active) << 16U) ^
                (static_cast<uint64_t>(current.stream_results_pending) << 8U) ^
                static_cast<uint64_t>(current.resolved_paths_pending) ^
                (static_cast<uint64_t>(current.texture_known) << 63U) ^
                (static_cast<uint64_t>(current.replacement_resolved) << 62U);
            if (progress == previous_progress) {
                unchanged_iterations++;
                if (unchanged_iterations >= MaxUnchangedIterations) {
                    set_error(error, error_capacity,
                              "RT64 texture-replacement evidence exceeded the deterministic no-progress iteration cap");
                    return 0;
                }
            }
            else {
                previous_progress = progress;
                unchanged_iterations = 0;
            }

            // The predicate is RT64's actual map/queue transition. Yielding
            // only lets its upload/stream workers run; no wall-clock delay or
            // timing threshold participates in the evidence result.
            std::this_thread::yield();
        }
    }
    catch (const std::exception &exception) {
        set_error(error, error_capacity,
                  std::string("texture-replacement evidence threw: ") + exception.what());
        return 0;
    }
    catch (...) {
        set_error(error, error_capacity,
                  "texture-replacement evidence failed with an unknown C++ exception");
        return 0;
    }
}

static bool require_display_list_processing_enabled(
    Fn64Rt64Context *context,
    const char *operation,
    char *error,
    size_t error_capacity) {
    // Pinned RT64's paused debugger path bypasses display-list parsing and
    // fabricates a DP interrupt. It therefore cannot provide native command
    // execution or FullSync-count evidence for either HLE or raw lists.
    if (context->application->state->debuggerInspector.paused) {
        set_error(error, error_capacity,
                  std::string("RT64 ") + operation +
                      " is unavailable while the debugger is paused");
        return false;
    }
    return true;
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
    const Fn64Rt64UcodePlan *ucode_plan,
    Fn64Rt64TaskResult *result,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if (context->ucode_admission_poisoned) {
            set_error(error, error_capacity,
                      "RT64 context is poisoned by an ordered microcode-plan divergence");
            return 0;
        }
        if (!require_display_list_processing_enabled(
                context, "native task processing", error, error_capacity)) {
            return 0;
        }
        if ((rdram == nullptr) || (dmem == nullptr) || (imem == nullptr) ||
            (task == nullptr) || (result == nullptr)) {
            set_error(error, error_capacity,
                      "null RDRAM, RSP memory, OSTask, or task-result pointer");
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

        *result = {};
        result->schema = FN64_RT64_TASK_RESULT_SCHEMA;
        result->rejected_generation = FN64_RT64_UCODE_NO_REJECTED_GENERATION;

        ImmutableUcodePlan immutable_plan;
        uint32_t rejected_generation = FN64_RT64_UCODE_NO_REJECTED_GENERATION;
        std::string preflight_diagnostic;
        const UcodePlanPreflight preflight = preflight_ucode_plan(
            ucode_plan, *task, immutable_plan, rejected_generation,
            preflight_diagnostic);
        if (ucode_plan != nullptr) {
            result->planned_count = ucode_plan->count;
            std::copy(std::begin(ucode_plan->plan_sha256),
                      std::end(ucode_plan->plan_sha256),
                      std::begin(result->plan_sha256));
        }
        if (preflight == UcodePlanPreflight::Invalid) {
            set_error(error, error_capacity, preflight_diagnostic);
            return 0;
        }
        if (preflight == UcodePlanPreflight::NeedsLle) {
            result->disposition = FN64_RT64_TASK_DISPOSITION_NEEDS_LLE;
            result->rejected_generation = rejected_generation;
            set_error(error, error_capacity, preflight_diagnostic);
            return 1;
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

        context->output_addr = output_addr & 0x00FFFFFFU;
        context->update_vi();

        // Application::Core and State are RT64's public embedding state.
        // Update both aliases together before any interpreter or worker can
        // observe the fn64-owned allocation.
        ScopedRdramBinding rdram_binding(context, rdram);
        ScopedUcodePlan admission_scope(
            context,
            context->application->interpreter.get(),
            immutable_plan,
            result);
        context->application->interpreter->loadUCodeGBI(
            ucode_address,
            ucode_data_address,
            true);
        const bool capture_extended = context->extended_capture_armed;
        context->extended_capture_armed = false;
        context->extended_capture_valid = false;
        if (context->application->interpreter->hleGBI == nullptr) {
            set_error(error, error_capacity, "RT64 did not recognize the task's graphics microcode");
            return 0;
        }
        result->entry_gbi_available = 1;
        result->initial_ucode_text_address =
            context->application->interpreter->UCode.textAddress;
        result->initial_ucode_data_address =
            context->application->interpreter->UCode.dataAddress;

        const uint64_t previous_workload = context->application->state->workloadId;
        result->workload_id_before = previous_workload;
        ExtendedDispatchProbe extended_probe{};
        RT64::GBI *initial_gbi = context->application->interpreter->hleGBI;
        std::unique_ptr<ExtendedDispatchScope> extended_scope;
        if (capture_extended) {
            if (initial_gbi->ucode != RT64::GBIUCode::F3DEX2) {
                set_error(error, error_capacity,
                          "Extended-GBI evidence requires recognized F3DEX2 microcode");
                return 0;
            }
            extended_scope = std::make_unique<ExtendedDispatchScope>(
                context->application->interpreter.get(), extended_probe);
        }
#if defined(__APPLE__)
        MetalPipelineCriticalScope pipeline_scope(context->ubershader_evidence_active);
#endif
        context->application->processDisplayLists(rdram, dl_address, 0, true);
        if (!admission_scope.exhausted()) {
            context->ucode_admission_poisoned = true;
            result->rejected_generation = admission_scope.cursor();
            set_error(error, error_capacity,
                      "RT64 task completed without exhausting its ordered microcode plan");
            return 0;
        }
        extended_scope.reset();
        const uint64_t submitted_workload = context->application->state->workloadId;
        if (submitted_workload < previous_workload) {
            set_error(error, error_capacity,
                      "RT64 task workload ID moved backwards during processing");
            return 0;
        }
        result->workload_id_after = submitted_workload;
        result->final_ucode_text_address =
            context->application->interpreter->UCode.textAddress;
        result->final_ucode_data_address =
            context->application->interpreter->UCode.dataAddress;
        if (submitted_workload > previous_workload) {
            // renderToRAM is enabled above. Waiting for this exact workload
            // closes the GPU/CPU interleaving before Rust or the VI capture
            // reads the fn64-owned RGBA5551 framebuffer.
            context->application->workloadQueue->waitForWorkloadId(submitted_workload);
        }

        if (capture_extended) {
            if (context->application->interpreter->hleGBI != initial_gbi) {
                set_error(error, error_capacity,
                          "RT64 changed microcode during Extended-GBI evidence capture");
                return 0;
            }
            if (!extended_probe.invalid.empty()) {
                set_error(error, error_capacity, extended_probe.invalid);
                return 0;
            }
            if (submitted_workload <= previous_workload) {
                set_error(error, error_capacity,
                          "armed Extended-GBI evidence observed no completed workload");
                return 0;
            }
            const uint32_t slot =
                context->application->workloadQueue->previousWriteCursor();
            const RT64::Workload &workload =
                context->application->workloadQueue->workloads[slot];
            if (workload.workloadId != submitted_workload) {
                set_error(error, error_capacity,
                          "Extended-GBI workload slot identity is inconsistent");
                return 0;
            }
            context->extended_capture_slot = slot;
            context->extended_capture_workload_id = submitted_workload;
            context->extended_evidence = extended_probe.evidence;
            context->extended_evidence.workload_id = submitted_workload;
            context->extended_evidence.present_id = workload.presentId;
            context->extended_rect_alignment_count =
                extended_probe.rect_alignment_count;
            context->extended_rect_alignment_call_index =
                extended_probe.rect_alignment_call_index;
            context->extended_left_offset = extended_probe.left_offset;
            context->extended_top_offset = extended_probe.top_offset;
            context->extended_right_offset = extended_probe.right_offset;
            context->extended_bottom_offset = extended_probe.bottom_offset;
            context->extended_vertex_command_indices =
                extended_probe.vertex_command_indices;
            context->extended_vertex_command_count =
                extended_probe.vertex_command_count;
            context->extended_capture_valid = true;
        }

        std::memcpy(dmem, context->dmem.data(), context->dmem.size());
        std::memcpy(imem, context->imem.data(), context->imem.size());

        result->disposition = FN64_RT64_TASK_DISPOSITION_COMPLETE;

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
    int wait_for_completion,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if (!require_display_list_processing_enabled(
                context, "raw RDP processing", error, error_capacity)) {
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

        uint64_t previous_present = 0;
        uint64_t previous_capture_generation = 0;
        if (context->present_capture_enabled) {
            previous_present = context->application->state->presentId;
            std::scoped_lock capture_lock(context->present_capture_mutex);
            previous_capture_generation = context->present_capture_generation;
            context->present_capture_error.clear();
        }

        context->output_addr = output_addr & 0x00FFFFFFU;
        context->update_vi();
        context->registers[1] = start;
        context->registers[2] = end;
        context->registers[3] = start;

        // RT64's public embedding entry accepts an explicit bounded LLE RDP
        // range when isHLE is false. Keep both public RDRAM aliases coherent
        // exactly as the task path does before the interpreter observes it.
        ScopedRdramBinding rdram_binding(context, rdram);
        const uint64_t previous_workload = context->application->state->workloadId;

        const bool capture_deferred = context->deferred_capture_armed;
        std::unique_lock<std::mutex> deferred_worker_lock(
            context->application->workloadQueue->threadMutex,
            std::defer_lock);
        if (capture_deferred) {
            {
                std::scoped_lock configuration_lock(
                    context->application->sharedQueueResources->configurationMutex);
                if (context->application->sharedQueueResources->enhancementConfig.presentation.mode ==
                    RT64::EnhancementConfiguration::Presentation::Mode::PresentEarly) {
                    context->deferred_capture_armed = false;
                    set_error(error, error_capacity, "pre-submit workload capture is incompatible with PresentEarly");
                    return 0;
                }
            }

            // FullSync publishes the queue cursor before the workload worker
            // takes threadMutex. Holding that existing mutex across enqueue
            // and snapshot closes the exact worker-consumes-or-mutates-slot
            // before evidence reads it interleaving. The lock is released
            // before the ordinary completion wait below.
            deferred_worker_lock.lock();
            context->deferred_capture_armed = false;
            context->deferred_capture_valid = false;
        }
#if defined(__APPLE__)
        MetalPipelineCriticalScope pipeline_scope(context->ubershader_evidence_active);
#endif
        context->application->processDisplayLists(rdram, start, end, false);
        context->registers[3] = end;
        const uint64_t submitted_workload = context->application->state->workloadId;
        bool deferred_snapshot_ok = true;
        if (capture_deferred) {
            if (submitted_workload <= previous_workload) {
                set_error(error, error_capacity, "armed pre-submit capture observed no completed RT64 workload");
                deferred_snapshot_ok = false;
            }
            else {
                const uint32_t slot = context->application->workloadQueue->previousWriteCursor();
                deferred_snapshot_ok = deferred_workload_snapshot(
                    context->application->workloadQueue->workloads[slot],
                    context->deferred_pre_submission,
                    error,
                    error_capacity);
                if (deferred_snapshot_ok) {
                    context->deferred_capture_slot = slot;
                    context->deferred_capture_valid = true;
                }
            }
            deferred_worker_lock.unlock();
        }
        // Deferring the wait is only safe when nothing below this point reads
        // completed-workload state before the CALLER'S own eventual wait (the
        // next call in the same field passing wait_for_completion=1, or a
        // present). `deferred_snapshot_ok`'s branch reads `submitted_workload`
        // for THIS call immediately below, so `capture_deferred` still forces
        // the wait unconditionally.
        //
        // `present_capture_enabled` is different, and forcing the wait for it
        // here was overbroad: the block it guards below only does anything
        // when `submitted_present > previous_present`, and `presentId` is
        // advanced by an actual Present, never by a display-list submission
        // (rt64_state.cpp's present path, not processDisplayLists). On this
        // call -- a raw-RDP command submission -- that comparison is false in
        // the ordinary case, so the block was dead here and the wait was pure
        // cost paid for a read that could not fire. The real hazard
        // `present_capture_enabled` exists for is closed at present time
        // (fn64_rt64_present / Rt64Backend::present, which flushes any
        // outstanding workload before it reads anything -- see
        // fn64_rt64_flush_pending_workload). Measured 2026-08-12 with a real
        // sampling profiler (macOS `sample`): this was the ONLY
        // synchronization wait appearing anywhere in a 5-second capture of
        // the windowed shell's render-heavy phase.
        const bool must_wait_for_capture = capture_deferred;
        if (submitted_workload > previous_workload && (wait_for_completion != 0 || must_wait_for_capture)) {
            context->application->workloadQueue->waitForWorkloadId(submitted_workload);
        }
        if (!deferred_snapshot_ok) {
            return 0;
        }

        if (context->present_capture_enabled) {
            const uint64_t submitted_present = context->application->state->presentId;
            if (submitted_present > previous_present) {
                // PresentEarly publishes its present ID from FullSync before
                // the present worker waits for this workload and records the
                // backend capture. The workload wait above can therefore finish
                // while the hook still owns the old generation. Waiting for
                // that exact present ID closes the cross-thread publication
                // interleaving before the process-time capture is exposed.
                context->application->presentQueue->waitForPresentId(submitted_present);

                std::scoped_lock capture_lock(context->present_capture_mutex);
                if (!context->present_capture_error.empty()) {
                    set_error(error, error_capacity, context->present_capture_error);
                    return 0;
                }
                if (context->present_capture_generation <= previous_capture_generation) {
                    set_error(error, error_capacity, "RT64 PresentEarly completed without a swapchain capture");
                    return 0;
                }
                const RT64::Present &completed =
                    context->application->presentQueue->presents[
                        context->application->presentQueue->previousWriteCursor()];
                if ((completed.presentId != submitted_present) ||
                    (completed.workloadId == 0U)) {
                    set_error(error, error_capacity,
                              "RT64 PresentEarly capture provenance is inconsistent");
                    return 0;
                }
                context->present_capture_id = submitted_present;
                context->present_capture_workload_id = completed.workloadId;
            }
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

// Wait for whatever workload is currently outstanding, regardless of which
// call submitted it. `application->state->workloadId` is the same monotonic
// counter `waitForWorkloadId` compares against (rt64_workload_queue.cpp:93,
// `waitId <= workloadId`), and every submission path sets it -- reading it
// fresh here rather than threading a caller-supplied id means this correctly
// flushes the true latest submission even if several were made without
// waiting. Mirrors the shutdown-flush idiom already used in
// ~Fn64Rt64Context.
extern "C" int fn64_rt64_flush_pending_workload(
    Fn64Rt64Context *context,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        context->application->workloadQueue->waitForWorkloadId(context->application->state->workloadId);
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 workload flush threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 workload flush failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_present(
    Fn64Rt64Context *context,
    uint8_t *rdram,
    size_t rdram_len,
    const Fn64Rt64ViState *vi,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || (vi == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if (rdram == nullptr) {
            set_error(error, error_capacity, "null RDRAM pointer at VI presentation");
            return 0;
        }
        if (rdram_len < N64_RDRAM_SIZE) {
            set_error(error, error_capacity, "RDRAM presentation view is smaller than the 8 MiB RT64 address space");
            return 0;
        }

        // A process-time PresentEarly may still be reading the preceding VI
        // policy after its producer returns. Drain that worker before replacing
        // `vi_state`; the current call then remains joined through the existing
        // end-of-function idle wait, so no present can observe the next
        // retrace's seed with the prior retrace's register image.
        context->application->presentQueue->waitForIdle();
        ScopedRdramBinding rdram_binding(context, rdram);

        PresentDiagnosticSnapshot diagnostic{};
        diagnostic.workload_before = context->application->state->workloadId;

        uint64_t previous_capture_generation = 0;
        if (context->present_capture_enabled) {
            std::scoped_lock capture_lock(context->present_capture_mutex);
            previous_capture_generation = context->present_capture_generation;
            diagnostic.capture_before = previous_capture_generation;
            context->present_capture_error.clear();
        }

        context->update_vi(*vi);
        const uint64_t previous_present = context->application->state->presentId;
        diagnostic.present_before = previous_present;
#if defined(__APPLE__)
        MetalPipelineCriticalScope pipeline_scope(context->ubershader_evidence_active);
#endif
        context->application->updateScreen();
        uint64_t submitted_present = context->application->state->presentId;
        if (context->presentation_refresh_pending && (submitted_present == previous_present)) {
            // The workload thread can finish a resolution/downsample rebuild
            // before this call while the VI and native RDRAM bytes remain
            // unchanged. Normal updateScreen then suppresses the only present
            // that can publish the rebuilt high-resolution target. A forced
            // current-VI event closes that exact interleaving without changing
            // lastScreenVI or fabricating a hardware VI transition.
            context->application->state->updateScreen(
                context->application->core.decodeVI(),
                true);
            submitted_present = context->application->state->presentId;
        }
        context->presentation_refresh_pending = false;
        diagnostic.workload_after = context->application->state->workloadId;
        diagnostic.present_after = submitted_present;
        if (submitted_present > previous_present) {
            context->application->presentQueue->waitForPresentId(submitted_present);
        }
        // Interleaving closed here: updateScreen publishes presentId before
        // the worker's post-publication tail has necessarily released State
        // and Core. Rust's call-scoped RDRAM capability cannot end while that
        // tail can still dereference the process allocation, so wait for the
        // queue itself to become idle before the binding restores the
        // placeholder and guest execution resumes.
        context->application->presentQueue->waitForIdle();
        if (context->present_capture_enabled) {
            std::string capture_error;
            {
                std::scoped_lock capture_lock(context->present_capture_mutex);
                capture_error = context->present_capture_error;
                diagnostic.capture_after = context->present_capture_generation;
            }
            if (!capture_error.empty()) {
                set_error(error, error_capacity, capture_error);
                return 0;
            }

            if (diagnostic.capture_after <= previous_capture_generation) {
                // A swapchain can fail its first acquire after RT64 has already
                // published the present ID. That leaves swapChainValid false,
                // but RT64 performs its resize/reacquire recovery only when a
                // subsequent event enters the present loop. Enqueue exactly
                // one current-VI event so that recovery runs; a second miss
                // remains the same loud capture failure below.
                const uint64_t retry_previous_present = submitted_present;
                context->application->state->updateScreen(
                    context->application->core.decodeVI(),
                    true);
                submitted_present = context->application->state->presentId;
                diagnostic.present_after = submitted_present;
                if (submitted_present > retry_previous_present) {
                    context->application->presentQueue->waitForPresentId(submitted_present);
                }

                std::scoped_lock capture_lock(context->present_capture_mutex);
                capture_error = context->present_capture_error;
                diagnostic.capture_after = context->present_capture_generation;
            }
            if (present_diagnostics_enabled()) {
                print_present_diagnostics(diagnostic);
            }
            if (!capture_error.empty()) {
                set_error(error, error_capacity, capture_error);
                return 0;
            }
            if (diagnostic.capture_after <= previous_capture_generation) {
                set_error(error, error_capacity, "RT64 present completed without a swapchain capture");
                return 0;
            }
            {
                // The present thread records the backend copy before the same
                // command fence as VI rendering, then publishes presentId.
                // Waiting for that ID before taking this mutex closes the
                // CPU-maps-readback-before-Vulkan/D3D12/Metal-copy-completes
                // interleaving while the hook may replace its allocation.
                std::scoped_lock capture_lock(context->present_capture_mutex);
                const RT64::Present &completed =
                    context->application->presentQueue->presents[
                        context->application->presentQueue->previousWriteCursor()];
                if (completed.presentId != submitted_present) {
                    set_error(error, error_capacity,
                              "RT64 present capture provenance is inconsistent");
                    return 0;
                }
                if (completed.workloadId == 0U) {
                    // Interleaving closed here: a game's VI thread can present
                    // before its graphics thread publishes the first workload.
                    // Those pixels have no workload provenance, so keep them
                    // unavailable as release evidence. Zero after either side
                    // has observed real work would instead erase provenance.
                    if ((diagnostic.workload_before != 0U) ||
                        (diagnostic.workload_after != 0U) ||
                        (context->present_capture_id != 0U) ||
                        (context->present_capture_workload_id != 0U)) {
                        set_error(error, error_capacity,
                                  "RT64 present capture lost workload provenance");
                        return 0;
                    }
                }
                else {
                    if (completed.workloadId > diagnostic.workload_after) {
                        set_error(error, error_capacity,
                                  "RT64 present capture observed a future workload");
                        return 0;
                    }
                    context->present_capture_id = submitted_present;
                    context->present_capture_workload_id = completed.workloadId;
                }
            }
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

// Installs draw_hook_dispatch as RT64's single draw hook if the slot is
// currently empty, or verifies this shim already owns it if not. Shared by
// both registration entry points (present-capture and the overlay-draw
// registry) since either may be the first caller to need the slot. Caller
// must hold the mutex for whichever registry it is registering into; this
// function does not take any lock itself, it only inspects/installs the
// global RT64 hook state.
bool ensure_draw_hook_dispatch_installed(char *error, size_t error_capacity) {
    if ((RT64::GetRenderHookInit() == nullptr) &&
        (RT64::GetRenderHookDraw() == nullptr) &&
        (RT64::GetRenderHookDeinit() == nullptr)) {
        RT64::SetRenderHooks(nullptr, draw_hook_dispatch, nullptr);
        return true;
    }
    if ((RT64::GetRenderHookInit() != nullptr) ||
        (RT64::GetRenderHookDraw() != draw_hook_dispatch) ||
        (RT64::GetRenderHookDeinit() != nullptr)) {
        set_error(error, error_capacity, "RT64 render hooks are already owned by another embedder");
        return false;
    }
    return true;
}

extern "C" int fn64_rt64_enable_present_capture(
    Fn64Rt64Context *context,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        std::scoped_lock registry_lock(present_capture_registry_mutex);
        if (context->present_capture_enabled) {
            return 1;
        }
        if (!ensure_draw_hook_dispatch_installed(error, error_capacity)) {
            return 0;
        }
        present_capture_contexts.push_back(context);
        context->present_capture_enabled = true;
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 present-capture enable threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 present-capture enable failed with an unknown C++ exception");
        return 0;
    }
}

// fn64-rmlui's own registration entry point (and any future second overlay
// draw-hook caller). context->setup_complete gates this the same way as
// present-capture, since ensure_draw_hook_dispatch_installed touches the
// same process-global RT64 hook slot either function may need to install.
extern "C" int fn64_rt64_register_overlay_draw(
    Fn64Rt64Context *context,
    void (*callback)(void *command_list, void *framebuffer, void *user_data),
    void *user_data,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if (callback == nullptr) {
            set_error(error, error_capacity, "null overlay draw callback");
            return 0;
        }
        std::scoped_lock registry_lock(draw_hook_registry_mutex);
        if (!ensure_draw_hook_dispatch_installed(error, error_capacity)) {
            return 0;
        }
        const auto existing = std::find_if(
            draw_hook_registrants.begin(),
            draw_hook_registrants.end(),
            [context](const DrawHookRegistrant &registrant) {
                return registrant.context == context;
            });
        if (existing != draw_hook_registrants.end()) {
            // Replace, not reject: a hot-reloaded UI document set
            // re-registering is expected to just update the callback/
            // user_data in place, mirroring fn64_rt64_enable_present_capture
            // treating a second enable call as a no-op success rather than
            // an error.
            existing->callback = callback;
            existing->user_data = user_data;
            return 1;
        }
        draw_hook_registrants.push_back(DrawHookRegistrant{context, callback, user_data});
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 overlay-draw register threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 overlay-draw register failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_unregister_overlay_draw(
    Fn64Rt64Context *context,
    char *error,
    size_t error_capacity) {
    try {
        if (context == nullptr) {
            set_error(error, error_capacity, "null RT64 context");
            return 0;
        }
        unregister_overlay_draw(context);
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 overlay-draw unregister threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 overlay-draw unregister failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" void *fn64_rt64_get_render_device(
    Fn64Rt64Context *context,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete || !context->application) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return nullptr;
        }
        return context->application->device.get();
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 render-device query threw: ") + exception.what());
        return nullptr;
    } catch (...) {
        set_error(error, error_capacity, "RT64 render-device query failed with an unknown C++ exception");
        return nullptr;
    }
}

extern "C" int fn64_rt64_read_present_capture(
    Fn64Rt64Context *context,
    Fn64Rt64PresentCapture *capture,
    uint8_t *bytes,
    size_t bytes_capacity,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if (capture == nullptr) {
            set_error(error, error_capacity, "null present-capture metadata pointer");
            return 0;
        }
        std::scoped_lock capture_lock(context->present_capture_mutex);
        if (!context->present_capture_enabled) {
            set_error(error, error_capacity, "RT64 present capture was not enabled");
            return 0;
        }
        if (!context->present_capture_error.empty()) {
            set_error(error, error_capacity, context->present_capture_error);
            return 0;
        }
        if ((context->present_capture_id == 0U) ||
            (context->present_capture_buffer == nullptr)) {
            set_error(error, error_capacity,
                      "RT64 has no completed post-workload present capture");
            return 0;
        }

        const uint64_t tight_row_bytes =
            static_cast<uint64_t>(context->present_capture_width) * 4U;
        const uint64_t byte_len = tight_row_bytes * context->present_capture_height;
        capture->width = context->present_capture_width;
        capture->height = context->present_capture_height;
        capture->row_bytes = static_cast<uint32_t>(tight_row_bytes);
        capture->format = context->present_capture_format;
        capture->graphics_api = context->present_capture_graphics_api;
        capture->reserved = 0U;
        capture->byte_len = byte_len;
        capture->present_id = context->present_capture_id;
        capture->workload_id = context->present_capture_workload_id;

        if (bytes == nullptr) {
            if (bytes_capacity != 0U) {
                set_error(error, error_capacity, "null present-capture byte pointer has nonzero capacity");
                return 0;
            }
            return 1;
        }
        if (bytes_capacity < byte_len) {
            set_error(error, error_capacity, "present-capture byte buffer is too small");
            return 0;
        }

        plume::RenderRange read_range(0, context->present_capture_buffer_size);
        const auto *mapped = static_cast<const uint8_t *>(
            context->present_capture_buffer->map(0, &read_range));
        if (mapped == nullptr) {
            set_error(error, error_capacity, "RT64 present readback buffer could not be mapped");
            return 0;
        }
        for (uint32_t row = 0; row < context->present_capture_height; row++) {
            std::memcpy(
                bytes + static_cast<uint64_t>(row) * tight_row_bytes,
                mapped + static_cast<uint64_t>(row) * context->present_capture_row_pitch,
                tight_row_bytes);
        }
        context->present_capture_buffer->unmap(0, nullptr);
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 present-capture read threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 present-capture read failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_read_present_selection(
    Fn64Rt64Context *context,
    Fn64Rt64PresentSelection *selection,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if (selection == nullptr) {
            set_error(error, error_capacity, "null RT64 present-selection output");
            return 0;
        }
#if defined(__APPLE__)
        // The workload thread owns render-target-map mutations and the present
        // thread owns the VI descriptor binding. The unique Rust borrow admits
        // no next submission; these two idle barriers therefore close the
        // exact read-versus-worker-write interleaving for the snapshot below.
        context->application->workloadQueue->waitForIdle();
        context->application->presentQueue->waitForIdle();

        RT64::PresentQueue *present_queue = context->application->presentQueue.get();
        if ((present_queue->viRenderer == nullptr) ||
            (present_queue->viRenderer->descriptorSet == nullptr)) {
            set_error(error, error_capacity, "RT64 VI renderer has no completed source binding");
            return 0;
        }

        auto *descriptor_set = static_cast<plume::MetalDescriptorSet *>(
            present_queue->viRenderer->descriptorSet->get());
        const uint32_t source_index = present_queue->viRenderer->descriptorSet->gInput;
        if ((descriptor_set == nullptr) ||
            (source_index >= descriptor_set->resourceEntries.size())) {
            set_error(error, error_capacity, "RT64 VI source descriptor is unavailable");
            return 0;
        }
        MTL::Resource *source = descriptor_set->resourceEntries[source_index].resource;
        if (source == nullptr) {
            set_error(error, error_capacity, "RT64 VI source descriptor has no texture resource");
            return 0;
        }

        RT64::RenderTarget *matched_target = nullptr;
        for (const auto &entry : context->application->sharedQueueResources->renderTargetManager.targetMap) {
            RT64::RenderTarget *candidate = entry.second.get();
            if ((candidate == nullptr) || (candidate->type != RT64::Framebuffer::Type::Color)) {
                continue;
            }

            const auto matches = [source](const plume::RenderTexture *texture) {
                return (texture != nullptr) &&
                       (static_cast<const plume::MetalTexture *>(texture)->mtl == source);
            };
            if (!matches(candidate->getResolvedTexture()) &&
                !matches(candidate->downsampledTexture.get())) {
                continue;
            }
            if ((matched_target != nullptr) && (matched_target != candidate)) {
                set_error(error, error_capacity, "RT64 VI source texture matches multiple render targets");
                return 0;
            }
            matched_target = candidate;
        }
        if (matched_target == nullptr) {
            set_error(error, error_capacity, "RT64 VI source texture matches no managed render target");
            return 0;
        }

        RT64::Framebuffer *framebuffer =
            context->application->sharedQueueResources->framebufferManager.find(
                matched_target->addressForName);
        if (framebuffer == nullptr) {
            set_error(error, error_capacity, "RT64 selected render target has no framebuffer identity");
            return 0;
        }

        uint64_t completed_present = 0;
        {
            std::scoped_lock present_id_lock(present_queue->presentIdMutex);
            completed_present = present_queue->presentId;
        }
        if (completed_present == 0U) {
            set_error(error, error_capacity, "RT64 has no completed present selection");
            return 0;
        }

        selection->present_id = completed_present;
        selection->source_texture_identity =
            static_cast<uint64_t>(reinterpret_cast<uintptr_t>(source));
        selection->target_address = framebuffer->addressStart;
        selection->target_width = framebuffer->width;
        selection->target_height = framebuffer->height;
        selection->target_size = framebuffer->siz;
        return 1;
#else
        set_error(error, error_capacity, "RT64 present-selection evidence requires the Metal backend");
        return 0;
#endif
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 present-selection query threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 present-selection query failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_enable_deferred_workload_capture(
    Fn64Rt64Context *context,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if (context->deferred_capture_armed) {
            set_error(error, error_capacity, "RT64 deferred workload capture is already armed");
            return 0;
        }
        context->application->workloadQueue->waitForIdle();
        context->application->presentQueue->waitForIdle();
        context->deferred_capture_valid = false;
        context->deferred_capture_armed = true;
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 deferred workload capture arm threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 deferred workload capture arm failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_read_deferred_workload_evidence(
    Fn64Rt64Context *context,
    Fn64Rt64DeferredWorkloadEvidence *evidence,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if (evidence == nullptr) {
            set_error(error, error_capacity, "null RT64 deferred-workload evidence output");
            return 0;
        }
        if (!context->deferred_capture_valid) {
            set_error(error, error_capacity, "RT64 has no captured pre-submit workload");
            return 0;
        }

        // The same queue-idle boundary used by present-selection evidence
        // keeps both the deferred vectors and debugger fields immutable for
        // this read. The unique Rust borrow prevents a new producer event.
        context->application->workloadQueue->waitForIdle();
        context->application->presentQueue->waitForIdle();
        const uint32_t current_slot =
            context->application->workloadQueue->previousWriteCursor();
        if (current_slot != context->deferred_capture_slot) {
            set_error(error, error_capacity, "captured RT64 deferred workload is no longer the current queue slot");
            return 0;
        }

        Fn64Rt64DeferredWorkloadSnapshot current{};
        if (!deferred_workload_snapshot(
                context->application->workloadQueue->workloads[current_slot],
                current,
                error,
                error_capacity)) {
            return 0;
        }
        if ((current.submission_frame != context->deferred_pre_submission.submission_frame) ||
            (current.content_digest != context->deferred_pre_submission.content_digest)) {
            set_error(error, error_capacity, "RT64 paused replay did not preserve captured deferred workload content");
            return 0;
        }

        evidence->pre_submission = context->deferred_pre_submission;
        evidence->current = current;
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 deferred-workload evidence query threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 deferred-workload evidence query failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_read_framebuffer_copy_path_evidence(
    Fn64Rt64Context *context,
    Fn64Rt64FramebufferCopyPathEvidence *evidence,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if (evidence == nullptr) {
            set_error(error, error_capacity,
                      "null RT64 framebuffer-copy path evidence output");
            return 0;
        }
        if (!context->deferred_capture_valid) {
            set_error(error, error_capacity,
                      "RT64 framebuffer-copy path evidence requires a captured deferred workload");
            return 0;
        }

        // This reuses the existing read-only queue-idle boundary. The unique
        // Rust borrow excludes a new producer while the completed workload's
        // path flags and operation vectors are counted.
        context->application->workloadQueue->waitForIdle();
        context->application->presentQueue->waitForIdle();
        const uint32_t current_slot =
            context->application->workloadQueue->previousWriteCursor();
        if (current_slot != context->deferred_capture_slot) {
            set_error(error, error_capacity,
                      "captured RT64 framebuffer-copy workload is no longer the current queue slot");
            return 0;
        }

        const RT64::Workload &workload =
            context->application->workloadQueue->workloads[current_slot];
        Fn64Rt64FramebufferCopyPathEvidence snapshot{};
        uint32_t valid_tile_count = 0;
        uint32_t unresolved_ordinary_tile_count = 0;
        snapshot.workload_id = workload.workloadId;
        for (uint32_t pair_index = 0; pair_index < workload.fbPairCount;
             pair_index++) {
            const RT64::FramebufferPair &fb_pair = workload.fbPairs[pair_index];
            if (fb_pair.syncRequired) {
                snapshot.sync_framebuffer_pair_count++;
            }
            for (const RT64::FramebufferOperation &operation :
                 fb_pair.startFbOperations) {
                if (operation.type ==
                    RT64::FramebufferOperation::Type::CreateTileCopy) {
                    snapshot.gpu_create_tile_copy_operation_count++;
                }
            }
        }
        for (const RT64::DrawCallTile &tile : workload.drawData.callTiles) {
            if (!tile.valid) {
                continue;
            }
            valid_tile_count++;
            if (tile.rawTMEM) {
                snapshot.raw_tmem_tile_count++;
            }
            if (tile.tileCopyUsed) {
                snapshot.gpu_tile_dispatch_count++;
            }
            else if (!tile.rawTMEM) {
                snapshot.cpu_rdram_tmem_upload_count++;
                if (tile.tmemHashOrID == 0U) {
                    unresolved_ordinary_tile_count++;
                }
            }
        }

        const uint32_t load_operation_count =
            static_cast<uint32_t>(workload.drawData.loadOperations.size());
        const bool gpu_path =
            (snapshot.gpu_create_tile_copy_operation_count == 1U) &&
            (snapshot.gpu_tile_dispatch_count == 1U) &&
            (snapshot.cpu_rdram_tmem_upload_count == 0U) &&
            (snapshot.raw_tmem_tile_count == 0U) &&
            (snapshot.sync_framebuffer_pair_count == 1U) &&
            (workload.fbPairCount == 1U) &&
            (valid_tile_count == 1U) && (load_operation_count == 1U) &&
            (unresolved_ordinary_tile_count == 0U);
        const bool cpu_path =
            (snapshot.gpu_create_tile_copy_operation_count == 0U) &&
            (snapshot.gpu_tile_dispatch_count == 0U) &&
            (snapshot.cpu_rdram_tmem_upload_count == 1U) &&
            (snapshot.raw_tmem_tile_count == 0U) &&
            (snapshot.sync_framebuffer_pair_count == 0U) &&
            (workload.fbPairCount == 1U) &&
            (valid_tile_count == 1U) && (load_operation_count == 1U) &&
            (unresolved_ordinary_tile_count == 0U);
        if (gpu_path == cpu_path) {
            set_error(
                error,
                error_capacity,
                "RT64 framebuffer-copy path is zero, mixed, or multiple: gpu_operations=" +
                    std::to_string(snapshot.gpu_create_tile_copy_operation_count) +
                    ", gpu_tiles=" +
                    std::to_string(snapshot.gpu_tile_dispatch_count) +
                    ", cpu_rdram_tmem_uploads=" +
                    std::to_string(snapshot.cpu_rdram_tmem_upload_count) +
                    ", raw_tmem_tiles=" +
                    std::to_string(snapshot.raw_tmem_tile_count) +
                    ", sync_pairs=" +
                    std::to_string(snapshot.sync_framebuffer_pair_count) +
                    ", framebuffer_pairs=" +
                    std::to_string(workload.fbPairCount) +
                    ", valid_tiles=" + std::to_string(valid_tile_count) +
                    ", unresolved_ordinary_tiles=" +
                    std::to_string(unresolved_ordinary_tile_count) +
                    ", load_operations=" + std::to_string(load_operation_count));
            return 0;
        }

        const RT64::LoadTexture &source_texture =
            workload.drawData.loadOperations[0].texture;
        const uint32_t source_address = source_texture.address;
        for (uint32_t pair_index = 0; pair_index < workload.fbPairCount;
             pair_index++) {
            if (workload.fbPairs[pair_index].colorImage.address ==
                source_address) {
                set_error(error, error_capacity,
                          "RT64 framebuffer-copy source belongs to the current workload instead of a prior managed framebuffer");
                return 0;
            }
        }
        RT64::Framebuffer *source_framebuffer =
            context->application->state->framebufferManager.find(source_address);
        if ((source_framebuffer == nullptr) ||
            (source_framebuffer->addressStart != source_address) ||
            (source_framebuffer->width != source_texture.width) ||
            (source_framebuffer->siz != source_texture.siz) ||
            (source_framebuffer->lastWriteTimestamp == 0U)) {
            set_error(error, error_capacity,
                      "RT64 framebuffer-copy source does not exactly match a prior managed framebuffer");
            return 0;
        }
        RT64::Framebuffer *sample_framebuffer =
            context->application->state->framebufferManager.find(
                workload.fbPairs[0].colorImage.address);
        if ((sample_framebuffer == nullptr) ||
            (source_framebuffer->lastWriteTimestamp >=
             sample_framebuffer->lastWriteTimestamp)) {
            set_error(error, error_capacity,
                      "RT64 framebuffer-copy source was not written strictly before the captured sample workload");
            return 0;
        }
        snapshot.source_framebuffer_address = source_address;
        snapshot.source_framebuffer_identity = reinterpret_cast<uint64_t>(
            source_framebuffer);
        if (snapshot.source_framebuffer_identity == 0U) {
            set_error(error, error_capacity,
                      "RT64 prior managed framebuffer identity is null");
            return 0;
        }
        snapshot.path = gpu_path ? FN64_RT64_FRAMEBUFFER_COPY_PATH_GPU
                                 : FN64_RT64_FRAMEBUFFER_COPY_PATH_CPU;
        *evidence = snapshot;
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity,
                  std::string("RT64 framebuffer-copy path evidence query threw: ") +
                      exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity,
                  "RT64 framebuffer-copy path evidence query failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_read_s2dex_fast_path_evidence(
    Fn64Rt64Context *context,
    Fn64Rt64S2dexFastPathEvidence *evidence,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if (evidence == nullptr) {
            set_error(error, error_capacity,
                      "null RT64 S2DEX fast-path evidence output");
            return 0;
        }
        if (!context->deferred_capture_valid) {
            set_error(error, error_capacity,
                      "RT64 S2DEX fast-path evidence requires a captured deferred workload");
            return 0;
        }
        context->application->workloadQueue->waitForIdle();
        context->application->presentQueue->waitForIdle();
        const uint32_t slot =
            context->application->workloadQueue->previousWriteCursor();
        if (slot != context->deferred_capture_slot) {
            set_error(error, error_capacity,
                      "captured RT64 S2DEX workload is no longer the current queue slot");
            return 0;
        }

        const RT64::Workload &workload =
            context->application->workloadQueue->workloads[slot];
        Fn64Rt64S2dexFastPathEvidence snapshot{};
        snapshot.workload_id = workload.workloadId;
        snapshot.framebuffer_pair_count = workload.fbPairCount;
        for (uint32_t pair_index = 0; pair_index < workload.fbPairCount;
             pair_index++) {
            const RT64::FramebufferPair &fb_pair = workload.fbPairs[pair_index];
            if (fb_pair.syncRequired) {
                snapshot.sync_framebuffer_pair_count++;
            }
            for (const RT64::FramebufferOperation &operation :
                 fb_pair.startFbOperations) {
                if (operation.type ==
                    RT64::FramebufferOperation::Type::CreateTileCopy) {
                    snapshot.gpu_create_tile_copy_operation_count++;
                }
            }
        }
        for (const RT64::DrawCallTile &tile : workload.drawData.callTiles) {
            if (!tile.valid) {
                continue;
            }
            snapshot.valid_tile_count++;
            if (tile.rawTMEM) {
                snapshot.raw_tmem_tile_count++;
            }
            if (tile.tileCopyUsed) {
                snapshot.gpu_tile_dispatch_count++;
            }
            else if (!tile.rawTMEM) {
                snapshot.cpu_rdram_tmem_upload_count++;
            }
        }
        snapshot.load_operation_count = static_cast<uint32_t>(
            workload.drawData.loadOperations.size());
        if (snapshot.load_operation_count == 0U) {
            set_error(error, error_capacity,
                      "captured RT64 S2DEX workload contains no texture load");
            return 0;
        }
        const uint32_t first_source_address =
            workload.drawData.loadOperations[0].texture.address;
        snapshot.source_address = first_source_address;
        RT64::Framebuffer *source_framebuffer =
            context->application->state->framebufferManager.find(
                first_source_address);
        if (source_framebuffer != nullptr) {
            if ((source_framebuffer->addressStart != first_source_address) ||
                (source_framebuffer->lastWriteTimestamp == 0U)) {
                set_error(error, error_capacity,
                          "captured RT64 S2DEX source is not an exact written framebuffer base");
                return 0;
            }
            if (workload.fbPairCount != 1U) {
                set_error(error, error_capacity,
                          "captured RT64 S2DEX workload does not have exactly one target framebuffer");
                return 0;
            }
            RT64::Framebuffer *target_framebuffer =
                context->application->state->framebufferManager.find(
                    workload.fbPairs[0].colorImage.address);
            if ((target_framebuffer == nullptr) ||
                (target_framebuffer == source_framebuffer) ||
                (source_framebuffer->lastWriteTimestamp >=
                 target_framebuffer->lastWriteTimestamp)) {
                set_error(error, error_capacity,
                          "captured RT64 S2DEX source does not strictly precede its distinct target");
                return 0;
            }
            snapshot.source_is_managed_framebuffer = 1U;
            snapshot.source_address = source_framebuffer->addressStart;
            snapshot.source_width = source_framebuffer->width;
            snapshot.source_height = source_framebuffer->height;
            snapshot.source_size = source_framebuffer->siz;
            snapshot.source_framebuffer_identity = reinterpret_cast<uint64_t>(
                source_framebuffer);
        }
        uint64_t load_digest = 1469598103934665603ULL;
        std::vector<uint32_t> distinct_addresses;
        snapshot.minimum_source_address = UINT32_MAX;
        for (uint32_t index = 0; index < snapshot.load_operation_count; index++) {
            const auto &load = workload.drawData.loadOperations[index];
            digest_u64(load_digest, index);
            digest_u64(load_digest, load.texture.address);
            digest_u64(load_digest, load.texture.fmt);
            digest_u64(load_digest, load.texture.siz);
            digest_u64(load_digest, load.texture.width);
            digest_u64(load_digest, static_cast<uint32_t>(load.type));
            digest_u64(load_digest, load.tile.fmt);
            digest_u64(load_digest, load.tile.siz);
            digest_u64(load_digest, load.tile.line);
            digest_u64(load_digest, load.tile.tmem);
            digest_u64(load_digest, load.tile.palette);
            digest_u64(load_digest, load.tile.cms);
            digest_u64(load_digest, load.tile.cmt);
            digest_u64(load_digest, load.tile.masks);
            digest_u64(load_digest, load.tile.maskt);
            digest_u64(load_digest, load.tile.shifts);
            digest_u64(load_digest, load.tile.shiftt);
            digest_u64(load_digest, load.tile.uls);
            digest_u64(load_digest, load.tile.ult);
            digest_u64(load_digest, load.tile.lrs);
            digest_u64(load_digest, load.tile.lrt);
            switch (load.type) {
            case RT64::LoadOperation::Type::Tile:
                digest_u64(load_digest, load.operationTile.tile);
                digest_u64(load_digest, load.operationTile.uls);
                digest_u64(load_digest, load.operationTile.ult);
                digest_u64(load_digest, load.operationTile.lrs);
                digest_u64(load_digest, load.operationTile.lrt);
                break;
            case RT64::LoadOperation::Type::Block:
                digest_u64(load_digest, load.operationBlock.tile);
                digest_u64(load_digest, load.operationBlock.uls);
                digest_u64(load_digest, load.operationBlock.ult);
                digest_u64(load_digest, load.operationBlock.lrs);
                digest_u64(load_digest, load.operationBlock.dxt);
                break;
            case RT64::LoadOperation::Type::TLUT:
                digest_u64(load_digest, load.operationTLUT.tile);
                digest_u64(load_digest, load.operationTLUT.uls);
                digest_u64(load_digest, load.operationTLUT.ult);
                digest_u64(load_digest, load.operationTLUT.lrs);
                digest_u64(load_digest, load.operationTLUT.lrt);
                break;
            }
            snapshot.minimum_source_address = std::min(
                snapshot.minimum_source_address, load.texture.address);
            snapshot.maximum_source_address = std::max(
                snapshot.maximum_source_address, load.texture.address);
            if (std::find(distinct_addresses.begin(), distinct_addresses.end(),
                          load.texture.address) == distinct_addresses.end()) {
                distinct_addresses.push_back(load.texture.address);
            }
            if (load.texture.address == first_source_address) {
                snapshot.base_source_load_count++;
            }
            else {
                snapshot.offset_source_load_count++;
            }
            if (source_framebuffer != nullptr) {
                RT64::Framebuffer *containing =
                    context->application->state->framebufferManager.findMostRecentContaining(
                        load.texture.address, load.texture.address + 1U);
                if ((containing != source_framebuffer) ||
                    (load.texture.address < source_framebuffer->addressStart) ||
                    (load.texture.address >= source_framebuffer->addressEnd)) {
                    set_error(error, error_capacity,
                              "captured RT64 S2DEX load escapes the exact prior source framebuffer");
                    return 0;
                }
            }
            else if (context->application->state->framebufferManager.findMostRecentContaining(
                         load.texture.address, load.texture.address + 1U) != nullptr) {
                set_error(error, error_capacity,
                          "captured RT64 ordinary S2DEX source overlaps a managed framebuffer");
                return 0;
            }
        }
        snapshot.load_operation_digest = load_digest;
        snapshot.distinct_source_address_count =
            static_cast<uint32_t>(distinct_addresses.size());
        *evidence = snapshot;
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity,
                  std::string("RT64 S2DEX fast-path evidence query threw: ") +
                      exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity,
                  "RT64 S2DEX fast-path evidence query failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_enable_extended_gbi_evidence(
    Fn64Rt64Context *context,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        context->application->workloadQueue->waitForIdle();
        context->application->presentQueue->waitForIdle();
        std::scoped_lock capture_lock(context->present_capture_mutex);
        // Previously HFR could arm after Extended's unlocked precheck but
        // before Extended published its flags. Both histories then recorded,
        // and the present hook's Extended-first branch starved the HFR
        // history. Queue joins precede this lock so a present worker cannot
        // wait on us; checking and publishing both histories atomically closes
        // that exact interleaving.
        bool another_history_armed = context->extended_capture_armed ||
                                     context->extended_present_capture_recording;
#if defined(FN64_RT64_HFR_EVIDENCE)
        another_history_armed = another_history_armed ||
                                context->hfr_capture_armed ||
                                context->hfr_present_capture_recording;
#endif
        if (another_history_armed) {
            set_error(error, error_capacity,
                      "RT64 Extended-GBI evidence cannot overlap another armed presentation history");
            return 0;
        }
        if (context->present_capture_enabled) {
            for (ExtendedPresentCaptureSlot &slot :
                 context->extended_present_captures) {
                slot = ExtendedPresentCaptureSlot{};
            }
            context->extended_present_capture_count = 0;
            context->extended_present_capture_finalized = false;
            context->extended_present_capture_recording = true;
        }
        context->extended_capture_valid = false;
        context->extended_capture_armed = true;
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity,
                  std::string("RT64 Extended-GBI evidence arm threw: ") +
                      exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity,
                  "RT64 Extended-GBI evidence arm failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_read_extended_gbi_evidence(
    Fn64Rt64Context *context,
    Fn64Rt64ExtendedGbiEvidence *evidence,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if (evidence == nullptr) {
            set_error(error, error_capacity,
                      "null RT64 Extended-GBI evidence output");
            return 0;
        }
        if (!context->extended_capture_valid) {
            set_error(error, error_capacity,
                      "RT64 has no captured Extended-GBI task");
            return 0;
        }

        // The unique Rust borrow prevents a new producer while both existing
        // RT64 queues reach the same immutable boundary used by the other
        // workload evidence readers.
        context->application->workloadQueue->waitForIdle();
        context->application->presentQueue->waitForIdle();
        const uint32_t current_slot =
            context->application->workloadQueue->previousWriteCursor();
        if (current_slot != context->extended_capture_slot) {
            set_error(error, error_capacity,
                      "captured RT64 Extended-GBI workload is no longer current");
            return 0;
        }
        const RT64::Workload &workload =
            context->application->workloadQueue->workloads[current_slot];
        if (workload.workloadId != context->extended_capture_workload_id) {
            set_error(error, error_capacity,
                      "captured RT64 Extended-GBI workload identity changed");
            return 0;
        }

        Fn64Rt64ExtendedGbiEvidence snapshot = context->extended_evidence;
        snapshot.rect_count = 0;
        snapshot.vertex_z_count = 0;
        snapshot.generated_present_count = 0;
        std::fill(std::begin(snapshot.rects), std::end(snapshot.rects),
                  Fn64Rt64ExtendedRectEvidence{});
        std::fill(std::begin(snapshot.vertex_z), std::end(snapshot.vertex_z),
                  Fn64Rt64VertexZEvidence{});
        std::fill(std::begin(snapshot.generated_presents),
                  std::end(snapshot.generated_presents),
                  Fn64Rt64GeneratedPresentEvidence{});
        if (!extended_workload_snapshot(
                workload,
                context->extended_rect_alignment_count,
                context->extended_rect_alignment_call_index,
                context->extended_left_offset,
                context->extended_top_offset,
                context->extended_right_offset,
                context->extended_bottom_offset,
                context->extended_vertex_command_indices,
                context->extended_vertex_command_count,
                snapshot,
                error,
                error_capacity)) {
            return 0;
        }
        if (snapshot.has_refresh_rate != 0U) {
            if ((snapshot.refresh_rate == 0U) ||
                (workload.viOriginalRate != snapshot.refresh_rate)) {
                set_error(error, error_capacity,
                          "RT64 Extended refresh command and workload rate disagree");
                return 0;
            }
        }

        RT64::InterpolatedFrameCounters frame_counters;
        {
            std::scoped_lock interpolated_lock(
                context->application->sharedQueueResources->interpolatedMutex);
            const uint32_t index = context->application->sharedQueueResources
                                       ->interpolatedFramesIndex;
            frame_counters = context->application->sharedQueueResources
                                 ->interpolatedFrames[index];
        }
        uint32_t target_rate = 0;
        {
            std::scoped_lock configuration_lock(
                context->application->sharedQueueResources->configurationMutex);
            target_rate =
                context->application->sharedQueueResources->targetRate;
        }

        uint64_t present_id = snapshot.present_id;
        const RT64::Present &latest_present =
            context->application->presentQueue->presents[
                context->application->presentQueue->previousWriteCursor()];
        if (latest_present.workloadId == workload.workloadId) {
            present_id = latest_present.presentId;
        }
        snapshot.present_id = present_id;

        if (frame_counters.count > 1U) {
            if ((snapshot.has_refresh_rate == 0U) ||
                (present_id == 0U) ||
                (latest_present.workloadId != workload.workloadId) ||
                frame_counters.skipped ||
                // Pinned PresentQueue's `presented` counter covers only the
                // i>0 override targets, while the render hook captures every
                // draw including ordinal zero.
                (frame_counters.presented != frame_counters.count - 1U) ||
                ((frame_counters.available != frame_counters.count - 1U) &&
                 (frame_counters.available != frame_counters.count)) ||
                (target_rate == 0U) ||
                (target_rate % workload.viOriginalRate != 0U) ||
                (frame_counters.count !=
                 target_rate / workload.viOriginalRate) ||
                (frame_counters.count >
                 FN64_RT64_EXTENDED_MAX_GENERATED_PRESENTS)) {
                set_error(error, error_capacity,
                          "RT64 generated-presentation provenance is skipped, overflowed, or fractionally ambiguous: count=" +
                              std::to_string(frame_counters.count) +
                              " available=" + std::to_string(frame_counters.available) +
                              " presented=" + std::to_string(frame_counters.presented) +
                              " skipped=" + std::to_string(frame_counters.skipped) +
                              " target=" + std::to_string(target_rate) +
                              " original=" + std::to_string(workload.viOriginalRate) +
                              " present=" + std::to_string(present_id) +
                              " latest-workload=" + std::to_string(latest_present.workloadId) +
                              " workload=" + std::to_string(workload.workloadId));
                return 0;
            }
            RT64::WorkloadQueue &queue =
                *context->application->workloadQueue;
            const RT64::GameFrame &current_frame =
                queue.gameFrames[queue.curFrameIndex];
            const RT64::GameFrame &previous_frame =
                queue.gameFrames[queue.prevFrameIndex];
            if ((current_frame.workloads.size() != 1U) ||
                (previous_frame.workloads.size() != 1U) ||
                (current_frame.workloads[0] != current_slot)) {
                set_error(error, error_capacity,
                          "RT64 generated-presentation source workload pair is ambiguous");
                return 0;
            }
            const uint32_t previous_slot = previous_frame.workloads[0];
            if (previous_slot >= queue.workloads.size()) {
                set_error(error, error_capacity,
                          "RT64 generated-presentation previous workload slot is invalid");
                return 0;
            }
            const uint64_t previous_workload_id =
                queue.workloads[previous_slot].workloadId;
            if ((previous_workload_id == 0U) ||
                (previous_workload_id == workload.workloadId)) {
                set_error(error, error_capacity,
                          "RT64 generated-presentation source workloads are not distinct");
                return 0;
            }
            snapshot.generated_present_count = frame_counters.count;
            for (uint32_t i = 0; i < frame_counters.count; i++) {
                Fn64Rt64GeneratedPresentEvidence &generated =
                    snapshot.generated_presents[i];
                generated.previous_workload_id = previous_workload_id;
                generated.current_workload_id = workload.workloadId;
                generated.present_id = present_id;
                generated.presentation_ordinal = i;
                generated.interpolation_numerator = i + 1U;
                generated.interpolation_denominator = frame_counters.count;
                generated.original_refresh_rate = workload.viOriginalRate;
                generated.target_refresh_rate = target_rate;
            }
        }

        if (context->present_capture_enabled &&
            (context->extended_present_capture_recording ||
             (context->extended_present_capture_count != 0U))) {
            std::scoped_lock capture_lock(context->present_capture_mutex);
            const uint32_t expected_capture_count =
                (snapshot.generated_present_count == 0U)
                    ? 1U
                    : snapshot.generated_present_count;
            if (context->extended_present_capture_count !=
                expected_capture_count) {
                set_error(error, error_capacity,
                          "RT64 Extended generated-presentation provenance and capture counts disagree");
                context->extended_present_capture_recording = false;
                return 0;
            }
            for (uint32_t i = 0; i < expected_capture_count; i++) {
                ExtendedPresentCaptureSlot &slot =
                    context->extended_present_captures[i];
                slot.workload_id = snapshot.workload_id;
                slot.present_id = snapshot.present_id;
                if (snapshot.generated_present_count != 0U) {
                    const Fn64Rt64GeneratedPresentEvidence &generated =
                        snapshot.generated_presents[i];
                    if ((generated.presentation_ordinal != i) ||
                        (generated.current_workload_id != snapshot.workload_id) ||
                        (generated.present_id != snapshot.present_id)) {
                        set_error(error, error_capacity,
                                  "RT64 Extended generated-presentation order and capture provenance disagree");
                        context->extended_present_capture_recording = false;
                        return 0;
                    }
                    slot.generated_ordinal = generated.presentation_ordinal;
                    slot.interpolation_numerator =
                        generated.interpolation_numerator;
                    slot.interpolation_denominator =
                        generated.interpolation_denominator;
                }
                else {
                    slot.generated_ordinal =
                        FN64_RT64_EXTENDED_NO_GENERATED_ORDINAL;
                    slot.interpolation_numerator = 1U;
                    slot.interpolation_denominator = 1U;
                }
            }
            context->extended_present_capture_recording = false;
            context->extended_present_capture_finalized = true;
        }

        *evidence = snapshot;
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity,
                  std::string("RT64 Extended-GBI evidence query threw: ") +
                      exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity,
                  "RT64 Extended-GBI evidence query failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_read_extended_present_capture(
    Fn64Rt64Context *context,
    uint32_t capture_index,
    Fn64Rt64ExtendedPresentCapture *capture,
    uint8_t *bytes,
    size_t bytes_capacity,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if (capture == nullptr) {
            set_error(error, error_capacity,
                      "null Extended present-capture metadata pointer");
            return 0;
        }
        std::scoped_lock capture_lock(context->present_capture_mutex);
        if (!context->extended_present_capture_finalized) {
            set_error(error, error_capacity,
                      "RT64 Extended present-capture history is not finalized");
            return 0;
        }
        if (capture_index >= context->extended_present_capture_count) {
            set_error(error, error_capacity,
                      "RT64 Extended present-capture index is out of range");
            return 0;
        }
        ExtendedPresentCaptureSlot &slot =
            context->extended_present_captures[capture_index];
        if ((slot.buffer == nullptr) || (slot.width == 0U) ||
            (slot.height == 0U) || (slot.row_pitch == 0U) ||
            (slot.workload_id == 0U) || (slot.present_id == 0U)) {
            set_error(error, error_capacity,
                      "RT64 Extended present-capture slot is incomplete");
            return 0;
        }
        const uint64_t tight_row_bytes =
            static_cast<uint64_t>(slot.width) * 4U;
        const uint64_t byte_len = tight_row_bytes * slot.height;
        capture->capture_generation = slot.capture_generation;
        capture->workload_id = slot.workload_id;
        capture->present_id = slot.present_id;
        capture->capture_ordinal = capture_index;
        capture->capture_count = context->extended_present_capture_count;
        capture->generated_ordinal = slot.generated_ordinal;
        capture->interpolation_numerator = slot.interpolation_numerator;
        capture->interpolation_denominator = slot.interpolation_denominator;
        capture->width = slot.width;
        capture->height = slot.height;
        capture->row_bytes = static_cast<uint32_t>(tight_row_bytes);
        capture->format = slot.format;
        capture->byte_len = byte_len;

        if (bytes == nullptr) {
            if (bytes_capacity != 0U) {
                set_error(error, error_capacity,
                          "null Extended present-capture byte pointer has nonzero capacity");
                return 0;
            }
            return 1;
        }
        if (bytes_capacity < byte_len) {
            set_error(error, error_capacity,
                      "Extended present-capture byte buffer is too small");
            return 0;
        }
        plume::RenderRange read_range(0, slot.buffer_size);
        const auto *mapped = static_cast<const uint8_t *>(
            slot.buffer->map(0, &read_range));
        if (mapped == nullptr) {
            set_error(error, error_capacity,
                      "RT64 Extended present-capture buffer could not be mapped");
            return 0;
        }
        for (uint32_t row = 0; row < slot.height; row++) {
            std::memcpy(
                bytes + static_cast<uint64_t>(row) * tight_row_bytes,
                mapped + static_cast<uint64_t>(row) * slot.row_pitch,
                tight_row_bytes);
        }
        slot.buffer->unmap(0, nullptr);
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity,
                  std::string("RT64 Extended present-capture read threw: ") +
                      exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity,
                  "RT64 Extended present-capture read failed with an unknown C++ exception");
        return 0;
    }
}

#if defined(FN64_RT64_HFR_EVIDENCE)
extern "C" int fn64_rt64_enable_hfr_evidence(
    Fn64Rt64Context *context,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if (!context->present_capture_enabled) {
            set_error(error, error_capacity,
                      "RT64 HFR evidence requires present capture to be enabled");
            return 0;
        }
        context->application->workloadQueue->waitForIdle();
        context->application->presentQueue->waitForIdle();
        std::scoped_lock capture_lock(context->present_capture_mutex);
        // The present hook can write either recording flag false under this
        // mutex on overflow/error. Joining before locking avoids a worker
        // deadlock; reading both flags and arming under the same lock closes
        // enable's prior unsynchronized read racing that hook-owned write.
        if (context->hfr_capture_armed ||
            context->hfr_present_capture_recording ||
            context->extended_capture_armed ||
            context->extended_present_capture_recording) {
            set_error(error, error_capacity,
                      "RT64 HFR evidence cannot overlap another armed presentation history");
            return 0;
        }
        for (ExtendedPresentCaptureSlot &slot : context->hfr_present_captures) {
            slot = ExtendedPresentCaptureSlot{};
        }
        context->hfr_present_capture_count = 0;
        context->hfr_present_capture_finalized = false;
        context->hfr_present_capture_recording = true;
        context->hfr_capture_valid = false;
        context->hfr_capture_armed = true;
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity,
                  std::string("RT64 HFR evidence arm threw: ") +
                      exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity,
                  "RT64 HFR evidence arm failed with an unknown C++ exception");
        return 0;
    }
}

#endif

#if defined(FN64_RT64_SYNTHETIC_F3DEX2_EVIDENCE)
extern "C" int fn64_rt64_process_synthetic_hfr_f3dex2(
    Fn64Rt64Context *context,
    uint8_t *rdram,
    size_t rdram_len,
    uint32_t display_list,
    uint32_t output_addr,
    uint16_t original_refresh_rate,
    Fn64Rt64RegionRateEvidence *region_rate_evidence,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if ((rdram == nullptr) || (rdram_len < N64_RDRAM_SIZE)) {
            set_error(error, error_capacity,
                      "synthetic RT64 HFR RDRAM is null or smaller than 8 MiB");
            return 0;
        }
        const uint64_t target_byte_len =
            static_cast<uint64_t>(context->width) * context->height * 2U;
        const bool target_out_of_bounds =
            (target_byte_len == 0U) || (target_byte_len > rdram_len) ||
            (static_cast<uint64_t>(output_addr) >
             static_cast<uint64_t>(rdram_len) - target_byte_len);
        const bool capture_region_rate = region_rate_evidence != nullptr;
        if (((display_list & 7U) != 0U) || (display_list >= rdram_len) ||
            ((output_addr & 7U) != 0U) ||
            ((output_addr & 0xFF000000U) != 0U) || target_out_of_bounds ||
            ((original_refresh_rate == 0U) != capture_region_rate)) {
            set_error(error, error_capacity,
                      "synthetic RT64 F3DEX2 addresses, RGBA16 target footprint, or refresh-rate evidence mode are invalid");
            return 0;
        }

        const bool capture_extended = context->extended_capture_armed;
        context->extended_capture_armed = false;
        context->extended_capture_valid = false;

        context->output_addr = output_addr & 0x00FFFFFFU;
        context->update_vi();
        ScopedRdramBinding rdram_binding(context, rdram);

        RT64::Interpreter *interpreter = context->application->interpreter.get();
        RT64::State *state = context->application->state.get();
        RT64::GBI &synthetic_gbi = interpreter->gbiManager.gbiCache[
            static_cast<uint32_t>(RT64::GBIUCode::F3DEX2)];
        if (synthetic_gbi.ucode == RT64::GBIUCode::Unknown) {
            synthetic_gbi.ucode = RT64::GBIUCode::F3DEX2;
            RT64::GBI_RDP::setup(&synthetic_gbi, true);
            RT64::GBI_F3DEX2::setup(&synthetic_gbi);
        }
        RT64::GBI *previous_gbi = interpreter->hleGBI;
        struct RestoreSyntheticGbi {
            RT64::Interpreter *interpreter;
            RT64::State *state;
            RT64::GBI *previous;
            uint32_t text_address;
            uint32_t data_address;
            bool no_n;
            uint32_t cull_both_mask;
            uint32_t cull_front_mask;
            uint32_t projection_mask;
            uint32_t load_mask;
            uint32_t push_mask;
            uint32_t shading_smooth_mask;
            ~RestoreSyntheticGbi() {
                interpreter->hleGBI = previous;
                interpreter->UCode.textAddress = text_address;
                interpreter->UCode.dataAddress = data_address;
                state->rsp->NoN = no_n;
                state->rsp->cullBothMask = cull_both_mask;
                state->rsp->cullFrontMask = cull_front_mask;
                state->rsp->projMask = projection_mask;
                state->rsp->loadMask = load_mask;
                state->rsp->pushMask = push_mask;
                state->rsp->shadingSmoothMask = shading_smooth_mask;
            }
        } restore{
            interpreter,
            state,
            previous_gbi,
            interpreter->UCode.textAddress,
            interpreter->UCode.dataAddress,
            state->rsp->NoN,
            state->rsp->cullBothMask,
            state->rsp->cullFrontMask,
            state->rsp->projMask,
            state->rsp->loadMask,
            state->rsp->pushMask,
            state->rsp->shadingSmoothMask};
        interpreter->hleGBI = &synthetic_gbi;
        state->rsp->setGBI(&synthetic_gbi);
        if (synthetic_gbi.resetFromTask != nullptr) {
            synthetic_gbi.resetFromTask(state);
        }
        if (capture_region_rate) {
            // This fixture isolates the production FullSync fallback. An
            // Extended GBI SetRefreshRate command would intentionally win
            // over VIHistory and make the region registry unobservable.
            if (state->extended.refreshRate != UINT16_MAX) {
                set_error(error, error_capacity,
                          "synthetic RT64 region-rate fixture inherited an Extended refresh override");
                return 0;
            }
        }
        else {
            state->setRefreshRate(original_refresh_rate);
        }

        const uint64_t previous_workload = state->workloadId;
        ExtendedDispatchProbe extended_probe{};
        std::unique_ptr<ExtendedDispatchScope> extended_scope;
        if (capture_extended) {
            extended_scope = std::make_unique<ExtendedDispatchScope>(
                interpreter, extended_probe);
        }
#if defined(__APPLE__)
        MetalPipelineCriticalScope pipeline_scope(context->ubershader_evidence_active);
#endif
        context->application->processDisplayLists(
            rdram,
            display_list,
            0,
            true);
        extended_scope.reset();
        const uint64_t submitted_workload = state->workloadId;
        if (submitted_workload <= previous_workload) {
            if (capture_extended) {
                context->extended_present_capture_recording = false;
            }
#if defined(FN64_RT64_HFR_EVIDENCE)
            if (context->hfr_capture_armed) {
                context->hfr_capture_armed = false;
                context->hfr_present_capture_recording = false;
            }
#endif
            set_error(error, error_capacity,
                      "synthetic RT64 HFR display list completed no workload");
            return 0;
        }
        context->application->workloadQueue->waitForWorkloadId(submitted_workload);

        if (capture_region_rate) {
            const uint32_t slot =
                context->application->workloadQueue->previousWriteCursor();
            const RT64::Workload &workload =
                context->application->workloadQueue->workloads[slot];
            if (workload.workloadId != submitted_workload) {
                set_error(error, error_capacity,
                          "synthetic RT64 region-rate workload slot identity is inconsistent");
                return 0;
            }
            const uint32_t registered_nominal_refresh_rate =
                fn64_rt64_nominal_full_rate(&state->viHistory);
            if (registered_nominal_refresh_rate !=
                context->nominal_refresh_rate) {
                set_error(error, error_capacity,
                          "synthetic RT64 region-rate registry disagrees with context creation");
                return 0;
            }
            Fn64Rt64RegionRateEvidence snapshot{};
            snapshot.workload_id = workload.workloadId;
            snapshot.configured_nominal_refresh_rate =
                context->nominal_refresh_rate;
            snapshot.registered_nominal_refresh_rate =
                registered_nominal_refresh_rate;
            snapshot.workload_original_refresh_rate = workload.viOriginalRate;
            snapshot.extended_refresh_override_absent = 1U;
            *region_rate_evidence = snapshot;
        }

        if (capture_extended) {
            if (interpreter->hleGBI != &synthetic_gbi) {
                context->extended_present_capture_recording = false;
                set_error(error, error_capacity,
                          "RT64 changed synthetic F3DEX2 during Extended-GBI evidence capture");
                return 0;
            }
            if (!extended_probe.invalid.empty()) {
                context->extended_present_capture_recording = false;
                set_error(error, error_capacity, extended_probe.invalid);
                return 0;
            }
            const uint32_t slot =
                context->application->workloadQueue->previousWriteCursor();
            const RT64::Workload &workload =
                context->application->workloadQueue->workloads[slot];
            if (workload.workloadId != submitted_workload) {
                context->extended_present_capture_recording = false;
                set_error(error, error_capacity,
                          "synthetic Extended-GBI workload slot identity is inconsistent");
                return 0;
            }
            context->extended_capture_slot = slot;
            context->extended_capture_workload_id = submitted_workload;
            context->extended_evidence = extended_probe.evidence;
            context->extended_evidence.workload_id = submitted_workload;
            context->extended_evidence.present_id = workload.presentId;
            context->extended_rect_alignment_count =
                extended_probe.rect_alignment_count;
            context->extended_rect_alignment_call_index =
                extended_probe.rect_alignment_call_index;
            context->extended_left_offset = extended_probe.left_offset;
            context->extended_top_offset = extended_probe.top_offset;
            context->extended_right_offset = extended_probe.right_offset;
            context->extended_bottom_offset = extended_probe.bottom_offset;
            context->extended_vertex_command_indices =
                extended_probe.vertex_command_indices;
            context->extended_vertex_command_count =
                extended_probe.vertex_command_count;
            context->extended_capture_valid = true;
        }

#if defined(FN64_RT64_HFR_EVIDENCE)
        if (context->hfr_capture_armed) {
            context->hfr_capture_armed = false;
            const uint32_t slot =
                context->application->workloadQueue->previousWriteCursor();
            const RT64::Workload &workload =
                context->application->workloadQueue->workloads[slot];
            if (workload.workloadId != submitted_workload) {
                context->hfr_present_capture_recording = false;
                set_error(error, error_capacity,
                          "synthetic RT64 HFR workload slot identity is inconsistent");
                return 0;
            }
            context->hfr_capture_slot = slot;
            context->hfr_capture_workload_id = submitted_workload;
            context->hfr_capture_valid = true;
        }
#endif
        return 1;
    } catch (const std::exception &exception) {
#if defined(FN64_RT64_HFR_EVIDENCE)
        if (context != nullptr) {
            context->hfr_capture_armed = false;
            context->hfr_present_capture_recording = false;
        }
#endif
        set_error(error, error_capacity,
                  std::string("synthetic RT64 HFR F3DEX2 processing threw: ") +
                      exception.what());
        return 0;
    } catch (...) {
#if defined(FN64_RT64_HFR_EVIDENCE)
        if (context != nullptr) {
            context->hfr_capture_armed = false;
            context->hfr_present_capture_recording = false;
        }
#endif
        set_error(error, error_capacity,
                  "synthetic RT64 HFR F3DEX2 processing failed with an unknown C++ exception");
        return 0;
    }
}

#endif

#if defined(FN64_RT64_SYNTHETIC_S2DEX_EVIDENCE)
extern "C" int fn64_rt64_process_synthetic_s2dex2(
    Fn64Rt64Context *context,
    uint8_t *rdram,
    size_t rdram_len,
    uint32_t display_list,
    uint32_t output_addr,
    uint32_t legacy_wire,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if ((rdram == nullptr) || (rdram_len < N64_RDRAM_SIZE)) {
            set_error(error, error_capacity,
                      "synthetic RT64 S2DEX2 RDRAM is null or smaller than 8 MiB");
            return 0;
        }
        const uint64_t target_byte_len =
            static_cast<uint64_t>(context->width) * context->height * 2U;
        const bool target_out_of_bounds =
            (target_byte_len == 0U) || (target_byte_len > rdram_len) ||
            (static_cast<uint64_t>(output_addr) >
             static_cast<uint64_t>(rdram_len) - target_byte_len);
        if (((display_list & 7U) != 0U) || (display_list >= rdram_len) ||
            ((output_addr & 7U) != 0U) ||
            ((output_addr & 0xFF000000U) != 0U) || target_out_of_bounds) {
            set_error(error, error_capacity,
                      "synthetic RT64 S2DEX2 addresses or RGBA16 target footprint are invalid");
            return 0;
        }

        context->output_addr = output_addr & 0x00FFFFFFU;
        context->update_vi();
        ScopedRdramBinding rdram_binding(context, rdram);

        RT64::Interpreter *interpreter = context->application->interpreter.get();
        RT64::State *state = context->application->state.get();
        if (legacy_wire > 1U) {
            set_error(error, error_capacity,
                      "synthetic RT64 S2DEX wire-family selector is not boolean");
            return 0;
        }
        const RT64::GBIUCode synthetic_ucode =
            (legacy_wire != 0U) ? RT64::GBIUCode::S2DEX : RT64::GBIUCode::S2DEX2;
        RT64::GBI &synthetic_gbi = interpreter->gbiManager.gbiCache[
            static_cast<uint32_t>(synthetic_ucode)];
        if (synthetic_gbi.ucode == RT64::GBIUCode::Unknown) {
            synthetic_gbi.ucode = synthetic_ucode;
            RT64::GBI_RDP::setup(&synthetic_gbi, true);
            if (legacy_wire != 0U) {
                RT64::GBI_S2DEX::setup(&synthetic_gbi);
            }
            else {
                RT64::GBI_S2DEX2::setup(&synthetic_gbi);
            }
        }
        RT64::GBI *previous_gbi = interpreter->hleGBI;
        struct RestoreSyntheticGbi {
            RT64::Interpreter *interpreter;
            RT64::State *state;
            RT64::GBI *previous;
            uint32_t text_address;
            uint32_t data_address;
            bool no_n;
            uint32_t cull_both_mask;
            uint32_t cull_front_mask;
            uint32_t projection_mask;
            uint32_t load_mask;
            uint32_t push_mask;
            uint32_t shading_smooth_mask;
            ~RestoreSyntheticGbi() {
                interpreter->hleGBI = previous;
                interpreter->UCode.textAddress = text_address;
                interpreter->UCode.dataAddress = data_address;
                state->rsp->NoN = no_n;
                state->rsp->cullBothMask = cull_both_mask;
                state->rsp->cullFrontMask = cull_front_mask;
                state->rsp->projMask = projection_mask;
                state->rsp->loadMask = load_mask;
                state->rsp->pushMask = push_mask;
                state->rsp->shadingSmoothMask = shading_smooth_mask;
            }
        } restore{
            interpreter,
            state,
            previous_gbi,
            interpreter->UCode.textAddress,
            interpreter->UCode.dataAddress,
            state->rsp->NoN,
            state->rsp->cullBothMask,
            state->rsp->cullFrontMask,
            state->rsp->projMask,
            state->rsp->loadMask,
            state->rsp->pushMask,
            state->rsp->shadingSmoothMask};
        interpreter->hleGBI = &synthetic_gbi;
        state->rsp->setGBI(&synthetic_gbi);
        if (synthetic_gbi.resetFromTask != nullptr) {
            synthetic_gbi.resetFromTask(state);
        }
        state->setRefreshRate(60);

        const uint64_t previous_workload = state->workloadId;
        const bool capture_deferred = context->deferred_capture_armed;
        std::unique_lock<std::mutex> deferred_worker_lock(
            context->application->workloadQueue->threadMutex,
            std::defer_lock);
        if (capture_deferred) {
            {
                std::scoped_lock configuration_lock(
                    context->application->sharedQueueResources->configurationMutex);
                if (context->application->sharedQueueResources->enhancementConfig.presentation.mode ==
                    RT64::EnhancementConfiguration::Presentation::Mode::PresentEarly) {
                    context->deferred_capture_armed = false;
                    set_error(error, error_capacity,
                              "S2DEX2 pre-submit capture is incompatible with PresentEarly");
                    return 0;
                }
            }
            // FullSync publishes the queue cursor before the worker takes this
            // mutex. Holding it through the snapshot closes worker mutation of
            // the captured slot before read-only mechanism evidence observes it.
            deferred_worker_lock.lock();
            context->deferred_capture_armed = false;
            context->deferred_capture_valid = false;
        }
        context->application->processDisplayLists(rdram, display_list, 0, true);
        const uint64_t submitted_workload = state->workloadId;
        bool deferred_snapshot_ok = true;
        if (capture_deferred) {
            if (submitted_workload <= previous_workload) {
                set_error(error, error_capacity,
                          "armed S2DEX2 pre-submit capture observed no completed workload");
                deferred_snapshot_ok = false;
            }
            else {
                const uint32_t slot =
                    context->application->workloadQueue->previousWriteCursor();
                deferred_snapshot_ok = deferred_workload_snapshot(
                    context->application->workloadQueue->workloads[slot],
                    context->deferred_pre_submission,
                    error,
                    error_capacity);
                if (deferred_snapshot_ok) {
                    context->deferred_capture_slot = slot;
                    context->deferred_capture_valid = true;
                }
            }
            deferred_worker_lock.unlock();
        }
        if (submitted_workload <= previous_workload) {
            set_error(error, error_capacity,
                      "synthetic RT64 S2DEX2 display list completed no workload");
            return 0;
        }
        context->application->workloadQueue->waitForWorkloadId(submitted_workload);
        if (!deferred_snapshot_ok) {
            return 0;
        }
        if (interpreter->hleGBI != &synthetic_gbi) {
            set_error(error, error_capacity,
                      "RT64 changed synthetic S2DEX2 during evidence capture");
            return 0;
        }
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity,
                  std::string("synthetic RT64 S2DEX2 processing threw: ") +
                      exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity,
                  "synthetic RT64 S2DEX2 processing failed with an unknown C++ exception");
        return 0;
    }
}
#endif

#if defined(FN64_RT64_HFR_EVIDENCE)

extern "C" int fn64_rt64_read_hfr_evidence(
    Fn64Rt64Context *context,
    Fn64Rt64HfrEvidence *evidence,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if (evidence == nullptr) {
            set_error(error, error_capacity, "null RT64 HFR evidence output");
            return 0;
        }
        if (!context->hfr_capture_valid || context->hfr_capture_armed) {
            set_error(error, error_capacity,
                      "RT64 has no completed synthetic HFR workload");
            return 0;
        }

        context->application->workloadQueue->waitForIdle();
        context->application->presentQueue->waitForIdle();
        RT64::WorkloadQueue &queue = *context->application->workloadQueue;
        const uint32_t current_slot = queue.previousWriteCursor();
        if (current_slot != context->hfr_capture_slot) {
            set_error(error, error_capacity,
                      "captured RT64 HFR workload is no longer current");
            return 0;
        }
        const RT64::Workload &workload = queue.workloads[current_slot];
        if (workload.workloadId != context->hfr_capture_workload_id) {
            set_error(error, error_capacity,
                      "captured RT64 HFR workload identity changed");
            return 0;
        }
        const RT64::GameFrame &current_frame =
            queue.gameFrames[queue.curFrameIndex];
        const RT64::GameFrame &previous_frame =
            queue.gameFrames[queue.prevFrameIndex];
        if ((current_frame.workloads.size() != 1U) ||
            (previous_frame.workloads.size() != 1U) ||
            (current_frame.workloads[0] != current_slot)) {
            set_error(error, error_capacity,
                      "RT64 HFR source workload history is ambiguous");
            return 0;
        }
        const uint32_t previous_slot = previous_frame.workloads[0];
        if (previous_slot >= queue.workloads.size()) {
            set_error(error, error_capacity,
                      "RT64 HFR previous workload slot is invalid");
            return 0;
        }
        const uint64_t previous_workload_id =
            queue.workloads[previous_slot].workloadId;
        if ((previous_workload_id == 0U) ||
            (previous_workload_id == workload.workloadId)) {
            set_error(error, error_capacity,
                      "RT64 HFR source workloads are not distinct");
            return 0;
        }

        RT64::InterpolatedFrameCounters frame_counters;
        {
            std::scoped_lock interpolated_lock(
                context->application->sharedQueueResources->interpolatedMutex);
            const uint32_t index = context->application->sharedQueueResources
                                       ->interpolatedFramesIndex;
            frame_counters = context->application->sharedQueueResources
                                 ->interpolatedFrames[index];
        }
        uint32_t target_rate = 0;
        {
            std::scoped_lock configuration_lock(
                context->application->sharedQueueResources->configurationMutex);
            target_rate =
                context->application->sharedQueueResources->targetRate;
        }
        // Pinned PresentQueue increments `presented` only for the i>0
        // override target in a non-MSAA burst (rt64_present_queue.cpp:269-297),
        // while the render hook observes both swapchain draws.
        const bool original_control =
            (target_rate == 0U) && (frame_counters.count == 1U) &&
            (frame_counters.available == 0U) &&
            (frame_counters.presented == 1U);
        const bool exact_double_rate =
            (target_rate == workload.viOriginalRate * 2U) &&
            (frame_counters.count == 2U) &&
            (frame_counters.available == 1U) &&
            (frame_counters.presented == 1U);
        if ((workload.viOriginalRate == 0U) ||
            frame_counters.skipped ||
            (!original_control && !exact_double_rate)) {
            set_error(
                error,
                error_capacity,
                std::string("RT64 HFR frame counters are skipped, overflowed, or fractionally ambiguous: original=") +
                    std::to_string(workload.viOriginalRate) +
                    " target=" + std::to_string(target_rate) +
                    " count=" + std::to_string(frame_counters.count) +
                    " available=" + std::to_string(frame_counters.available) +
                    " presented=" + std::to_string(frame_counters.presented) +
                    " skipped=" + (frame_counters.skipped ? "1" : "0"));
            return 0;
        }

        const RT64::Present &latest_present =
            context->application->presentQueue->presents[
                context->application->presentQueue->previousWriteCursor()];
        if ((latest_present.presentId == 0U) ||
            (latest_present.workloadId != workload.workloadId)) {
            set_error(error, error_capacity,
                      "RT64 HFR presentation is not associated with the captured workload");
            return 0;
        }
        RT64::Framebuffer *framebuffer =
            context->application->sharedQueueResources->framebufferManager.find(
                context->output_addr);
        if ((framebuffer == nullptr) || !framebuffer->interpolationEnabled ||
            (framebuffer->addressStart != context->output_addr)) {
            set_error(
                error,
                error_capacity,
                std::string("RT64 HFR target is not an interpolation-enabled managed framebuffer: found=") +
                    ((framebuffer != nullptr) ? "1" : "0") +
                    " enabled=" +
                    ((framebuffer != nullptr) && framebuffer->interpolationEnabled ? "1" : "0") +
                    " expected_address=" + std::to_string(context->output_addr) +
                    " actual_address=" +
                    std::to_string((framebuffer != nullptr) ? framebuffer->addressStart : 0U));
            return 0;
        }

        std::scoped_lock capture_lock(context->present_capture_mutex);
        if (!context->hfr_present_capture_recording ||
            (context->hfr_present_capture_count != frame_counters.count)) {
            context->hfr_present_capture_recording = false;
            set_error(error, error_capacity,
                      "RT64 HFR presentation count and captured images disagree");
            return 0;
        }
        Fn64Rt64HfrEvidence snapshot{};
        snapshot.previous_workload_id = previous_workload_id;
        snapshot.current_workload_id = workload.workloadId;
        snapshot.present_id = latest_present.presentId;
        snapshot.interpolation_framebuffer_identity =
            reinterpret_cast<uint64_t>(framebuffer);
        snapshot.interpolation_framebuffer_address = framebuffer->addressStart;
        snapshot.original_refresh_rate = workload.viOriginalRate;
        snapshot.target_refresh_rate = target_rate;
        snapshot.presentation_count = frame_counters.count;
        snapshot.available_interpolated_target_count = frame_counters.available;
        snapshot.presented_counter_value = frame_counters.presented;
        snapshot.skipped = frame_counters.skipped ? 1U : 0U;
        for (uint32_t i = 0; i < frame_counters.count; i++) {
            ExtendedPresentCaptureSlot &slot = context->hfr_present_captures[i];
            slot.workload_id = workload.workloadId;
            slot.present_id = latest_present.presentId;
            if (frame_counters.count > 1U) {
                Fn64Rt64GeneratedPresentEvidence &generated =
                    snapshot.generated_presents[i];
                generated.previous_workload_id = previous_workload_id;
                generated.current_workload_id = workload.workloadId;
                generated.present_id = latest_present.presentId;
                generated.presentation_ordinal = i;
                generated.interpolation_numerator = i + 1U;
                generated.interpolation_denominator = frame_counters.count;
                generated.original_refresh_rate = workload.viOriginalRate;
                generated.target_refresh_rate = target_rate;
                slot.generated_ordinal = i;
                slot.interpolation_numerator = i + 1U;
                slot.interpolation_denominator = frame_counters.count;
            }
            else {
                slot.generated_ordinal = FN64_RT64_EXTENDED_NO_GENERATED_ORDINAL;
                slot.interpolation_numerator = 1U;
                slot.interpolation_denominator = 1U;
            }
        }
        context->hfr_present_capture_recording = false;
        context->hfr_present_capture_finalized = true;
        *evidence = snapshot;
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity,
                  std::string("RT64 HFR evidence query threw: ") +
                      exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity,
                  "RT64 HFR evidence query failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_read_hfr_present_capture(
    Fn64Rt64Context *context,
    uint32_t capture_index,
    Fn64Rt64ExtendedPresentCapture *capture,
    uint8_t *bytes,
    size_t bytes_capacity,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if (capture == nullptr) {
            set_error(error, error_capacity,
                      "null HFR present-capture metadata pointer");
            return 0;
        }
        std::scoped_lock capture_lock(context->present_capture_mutex);
        if (!context->hfr_present_capture_finalized) {
            set_error(error, error_capacity,
                      "RT64 HFR present-capture history is not finalized");
            return 0;
        }
        if (capture_index >= context->hfr_present_capture_count) {
            set_error(error, error_capacity,
                      "RT64 HFR present-capture index is out of range");
            return 0;
        }
        ExtendedPresentCaptureSlot &slot =
            context->hfr_present_captures[capture_index];
        if ((slot.buffer == nullptr) || (slot.width == 0U) ||
            (slot.height == 0U) || (slot.row_pitch == 0U) ||
            (slot.workload_id == 0U) || (slot.present_id == 0U)) {
            set_error(error, error_capacity,
                      "RT64 HFR present-capture slot is incomplete");
            return 0;
        }
        const uint64_t tight_row_bytes =
            static_cast<uint64_t>(slot.width) * 4U;
        const uint64_t byte_len = tight_row_bytes * slot.height;
        capture->capture_generation = slot.capture_generation;
        capture->workload_id = slot.workload_id;
        capture->present_id = slot.present_id;
        capture->capture_ordinal = capture_index;
        capture->capture_count = context->hfr_present_capture_count;
        capture->generated_ordinal = slot.generated_ordinal;
        capture->interpolation_numerator = slot.interpolation_numerator;
        capture->interpolation_denominator = slot.interpolation_denominator;
        capture->width = slot.width;
        capture->height = slot.height;
        capture->row_bytes = static_cast<uint32_t>(tight_row_bytes);
        capture->format = slot.format;
        capture->byte_len = byte_len;
        if (bytes == nullptr) {
            if (bytes_capacity != 0U) {
                set_error(error, error_capacity,
                          "null HFR present-capture byte pointer has nonzero capacity");
                return 0;
            }
            return 1;
        }
        if (bytes_capacity < byte_len) {
            set_error(error, error_capacity,
                      "HFR present-capture byte buffer is too small");
            return 0;
        }
        plume::RenderRange read_range(0, slot.buffer_size);
        const auto *mapped = static_cast<const uint8_t *>(
            slot.buffer->map(0, &read_range));
        if (mapped == nullptr) {
            set_error(error, error_capacity,
                      "RT64 HFR present-capture buffer could not be mapped");
            return 0;
        }
        for (uint32_t row = 0; row < slot.height; row++) {
            std::memcpy(
                bytes + static_cast<uint64_t>(row) * tight_row_bytes,
                mapped + static_cast<uint64_t>(row) * slot.row_pitch,
                tight_row_bytes);
        }
        slot.buffer->unmap(0, nullptr);
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity,
                  std::string("RT64 HFR present-capture read threw: ") +
                      exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity,
                  "RT64 HFR present-capture read failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_enable_hfr_pacing_evidence(
    Fn64Rt64Context *context,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        context->application->workloadQueue->waitForIdle();
        context->application->presentQueue->waitForIdle();
        std::scoped_lock registry_lock(hfr_pacing_registry_mutex);
        std::scoped_lock capture_lock(context->present_capture_mutex);
        // The patched present thread publishes both call phases under this
        // mutex. Joining it before locking prevents a worker-lock inversion;
        // resetting and arming under the same mutex closes an old callback
        // publishing into the newly reset observation window.
        if (context->hfr_pacing_recording || context->hfr_pacing_pending) {
            set_error(error, error_capacity,
                      "RT64 HFR pacing evidence is already recording");
            return 0;
        }
        if (std::find(
                hfr_pacing_contexts.begin(),
                hfr_pacing_contexts.end(),
                context) != hfr_pacing_contexts.end()) {
            set_error(error, error_capacity,
                      "RT64 HFR pacing registry retained an inactive context");
            return 0;
        }
        context->hfr_pacing_samples = {};
        context->hfr_pacing_sample_count = 0;
        context->hfr_pacing_error.clear();
        context->hfr_pacing_recording = true;
        hfr_pacing_contexts.push_back(context);
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity,
                  std::string("RT64 HFR pacing evidence arm threw: ") +
                      exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity,
                  "RT64 HFR pacing evidence arm failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_read_hfr_pacing_evidence(
    Fn64Rt64Context *context,
    Fn64Rt64HfrPacingEvidence *evidence,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if (evidence == nullptr) {
            set_error(error, error_capacity,
                      "null RT64 HFR pacing evidence output");
            return 0;
        }
        context->application->workloadQueue->waitForIdle();
        context->application->presentQueue->waitForIdle();
        std::scoped_lock registry_lock(hfr_pacing_registry_mutex);
        std::scoped_lock capture_lock(context->present_capture_mutex);
        const auto registration = std::find(
            hfr_pacing_contexts.begin(),
            hfr_pacing_contexts.end(),
            context);
        if (registration == hfr_pacing_contexts.end()) {
            set_error(error, error_capacity,
                      "RT64 HFR pacing context is not registered");
            return 0;
        }
        auto stop_recording = [&]() {
            context->hfr_pacing_recording = false;
            context->hfr_pacing_pending = false;
            hfr_pacing_contexts.erase(registration);
        };
        if (!context->hfr_pacing_error.empty()) {
            set_error(error, error_capacity, context->hfr_pacing_error);
            stop_recording();
            return 0;
        }
        if (!context->hfr_pacing_recording || context->hfr_pacing_pending ||
            (context->hfr_pacing_sample_count == 0U)) {
            set_error(error, error_capacity,
                      "RT64 HFR pacing observation is empty, unpaired, or not recording");
            stop_recording();
            return 0;
        }
        Fn64Rt64HfrPacingEvidence snapshot{};
        snapshot.sample_count = context->hfr_pacing_sample_count;
        std::copy_n(
            context->hfr_pacing_samples.begin(),
            context->hfr_pacing_sample_count,
            std::begin(snapshot.samples));
        stop_recording();
        *evidence = snapshot;
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity,
                  std::string("RT64 HFR pacing evidence query threw: ") +
                      exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity,
                  "RT64 HFR pacing evidence query failed with an unknown C++ exception");
        return 0;
    }
}
#endif

extern "C" int fn64_rt64_set_debugger_inspection_for_evidence(
    Fn64Rt64Context *context,
    uint32_t paused,
    int32_t framebuffer_index,
    int32_t draw_call_index,
    uint32_t framebuffer_depth,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if ((paused > 1U) || (framebuffer_depth > 1U)) {
            set_error(error, error_capacity, "RT64 debugger evidence booleans must be zero or one");
            return 0;
        }
        if (!context->deferred_capture_valid) {
            set_error(error, error_capacity, "RT64 debugger evidence requires a captured deferred workload");
            return 0;
        }

        context->application->workloadQueue->waitForIdle();
        context->application->presentQueue->waitForIdle();
        const uint32_t slot = context->application->workloadQueue->previousWriteCursor();
        if (slot != context->deferred_capture_slot) {
            set_error(error, error_capacity, "RT64 debugger evidence workload is no longer current");
            return 0;
        }
        const RT64::Workload &workload =
            context->application->workloadQueue->workloads[slot];
        if ((framebuffer_index < -1) ||
            (framebuffer_index >= static_cast<int32_t>(workload.fbPairCount))) {
            set_error(error, error_capacity, "RT64 debugger framebuffer selection is out of range");
            return 0;
        }
        if ((draw_call_index < -1) ||
            (draw_call_index >= static_cast<int32_t>(workload.gameCallCount))) {
            set_error(error, error_capacity, "RT64 debugger draw-call selection is out of range");
            return 0;
        }
        if ((framebuffer_index < 0) && (framebuffer_depth != 0U)) {
            set_error(error, error_capacity, "RT64 debugger cannot select depth without a framebuffer");
            return 0;
        }

        uint32_t framebuffer_address = 0U;
        if (framebuffer_index >= 0) {
            const RT64::FramebufferPair &pair = workload.fbPairs[framebuffer_index];
            framebuffer_address =
                (framebuffer_depth != 0U) ? pair.depthImage.address : pair.colorImage.address;
            if (framebuffer_address == 0U) {
                set_error(error, error_capacity, "RT64 debugger selected a framebuffer with zero address");
                return 0;
            }
        }

        RT64::DebuggerInspector &debugger = context->application->state->debuggerInspector;
        debugger.paused = paused != 0U;
        debugger.renderer.framebufferIndex = framebuffer_index;
        debugger.renderer.globalDrawCallIndex = draw_call_index;
        debugger.renderer.framebufferDepth = framebuffer_depth != 0U;
        debugger.renderer.framebufferAddress = framebuffer_address;
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 debugger evidence control threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 debugger evidence control failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_enable_ubershader_evidence(
    Fn64Rt64Context *context,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
#if defined(__APPLE__)
        MetalPipelineConstructionProbe &probe = metal_pipeline_probe;
        std::scoped_lock probe_lock(probe.control_mutex);
        if (context->ubershader_evidence_active ||
            probe.active.load(std::memory_order_acquire)) {
            set_error(error, error_capacity, "RT64 ubershader evidence is already active");
            return 0;
        }

        context->application->workloadQueue->waitForIdle();
        context->application->presentQueue->waitForIdle();
        RT64::RasterShaderUber *shader_uber =
            context->application->rasterShaderCache->shaderUber.get();
        if (shader_uber == nullptr) {
            set_error(error, error_capacity, "RT64 raster ubershader is unavailable");
            return 0;
        }
        shader_uber->waitForPipelineCreation();
        uint32_t precreated_count = 0;
        for (const auto &pipeline : shader_uber->pipelines) {
            precreated_count += pipeline != nullptr ? 1U : 0U;
        }
        if (precreated_count != 8U) {
            set_error(error, error_capacity, "RT64 did not precreate exactly eight raster ubershader pipelines");
            return 0;
        }

        auto *metal_device = dynamic_cast<plume::MetalDevice *>(
            context->application->device.get());
        if (!install_metal_pipeline_probe(metal_device, error, error_capacity)) {
            return 0;
        }
        if ((context->application->workloadQueue->renderThread == nullptr) ||
            (context->application->presentQueue->presentThread == nullptr)) {
            set_error(error, error_capacity, "RT64 queue threads are unavailable for pipeline evidence");
            return 0;
        }

        probe.caller_thread = std::this_thread::get_id();
        probe.workload_thread = context->application->workloadQueue->renderThread->get_id();
        probe.present_thread = context->application->presentQueue->presentThread->get_id();
        probe.caller_events.store(0U, std::memory_order_relaxed);
        probe.workload_events.store(0U, std::memory_order_relaxed);
        probe.present_events.store(0U, std::memory_order_relaxed);
        probe.background_events.store(0U, std::memory_order_relaxed);
        probe.caller_scope.store(false, std::memory_order_relaxed);
        context->application->workloadQueue->ubershadersOnly.store(true);
        context->ubershader_evidence_active = true;
        probe.active.store(true, std::memory_order_release);
        return 1;
#else
        set_error(error, error_capacity, "RT64 ubershader pipeline evidence requires Metal");
        return 0;
#endif
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 ubershader evidence setup threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 ubershader evidence setup failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_read_ubershader_evidence(
    Fn64Rt64Context *context,
    Fn64Rt64UbershaderEvidence *evidence,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || !context->setup_complete) {
            set_error(error, error_capacity, "RT64 context is not initialized");
            return 0;
        }
        if (evidence == nullptr) {
            set_error(error, error_capacity, "null RT64 ubershader evidence output");
            return 0;
        }
#if defined(__APPLE__)
        if (!context->ubershader_evidence_active ||
            !metal_pipeline_probe.active.load(std::memory_order_acquire)) {
            set_error(error, error_capacity, "RT64 ubershader evidence is not active");
            return 0;
        }

        // The Metal hook publishes an event before forwarding the actual PSO
        // construction. Waiting both queue mutexes closes the exact hook-event
        // published but worker/present construction still in flight before
        // evidence reads the critical-thread counters interleaving.
        context->application->workloadQueue->waitForIdle();
        context->application->presentQueue->waitForIdle();
        if (metal_pipeline_probe.caller_scope.load(std::memory_order_acquire)) {
            set_error(error, error_capacity, "RT64 caller pipeline scope remained active at evidence read");
            return 0;
        }

        *evidence = {};
        evidence->workload_id = context->application->workloadQueue->workloadId;
        evidence->present_id = context->application->presentQueue->presentId;
        const uint64_t caller_events =
            metal_pipeline_probe.caller_events.load(std::memory_order_acquire);
        const uint64_t workload_events =
            metal_pipeline_probe.workload_events.load(std::memory_order_acquire);
        const uint64_t present_events =
            metal_pipeline_probe.present_events.load(std::memory_order_acquire);
        const uint64_t background_events =
            metal_pipeline_probe.background_events.load(std::memory_order_acquire);
        if ((caller_events > std::numeric_limits<uint32_t>::max()) ||
            (workload_events > std::numeric_limits<uint32_t>::max()) ||
            (present_events > std::numeric_limits<uint32_t>::max())) {
            set_error(error, error_capacity, "RT64 critical pipeline construction count exceeds u32");
            return 0;
        }
        evidence->caller_construction_events = static_cast<uint32_t>(caller_events);
        evidence->workload_construction_events = static_cast<uint32_t>(workload_events);
        evidence->present_construction_events = static_cast<uint32_t>(present_events);
        evidence->background_construction_events = background_events;
        evidence->graphics_pipeline_construction_events =
            caller_events + workload_events + present_events + background_events;
        evidence->ubershaders_only = static_cast<uint32_t>(
            context->application->workloadQueue->ubershadersOnly.load());
        evidence->specialized_shader_count =
            context->application->rasterShaderCache->shaderCount();

        const RT64::RasterShaderUber *shader_uber =
            context->application->rasterShaderCache->shaderUber.get();
        if (shader_uber == nullptr) {
            set_error(error, error_capacity, "RT64 raster ubershader disappeared during evidence read");
            return 0;
        }
        uint64_t pipeline_digest = 1469598103934665603ULL;
        for (uint32_t index = 0; index < 8U; index++) {
            const RenderPipeline *pipeline = shader_uber->pipelines[index].get();
            if (pipeline != nullptr) {
                evidence->precreated_pipeline_count++;
            }
            digest_u64(pipeline_digest, index);
            digest_u64(
                pipeline_digest,
                static_cast<uint64_t>(reinterpret_cast<uintptr_t>(pipeline)));
        }
        if (evidence->precreated_pipeline_count != 8U) {
            set_error(error, error_capacity, "RT64 no longer has exactly eight precreated raster ubershader pipelines");
            return 0;
        }

        uint64_t descriptor_digest = 1469598103934665603ULL;
        const RT64::FramebufferRenderer &renderer =
            *context->application->workloadQueue->framebufferRenderer;
        for (const RT64::InstanceDrawCall &draw : renderer.instanceDrawCallVector) {
            if ((draw.type != RT64::InstanceDrawCall::Type::IndexedTriangles) &&
                (draw.type != RT64::InstanceDrawCall::Type::RawTriangles) &&
                (draw.type != RT64::InstanceDrawCall::Type::RegularRect)) {
                continue;
            }
            const uint32_t call_index = evidence->raster_call_count;
            if (call_index >= FN64_RT64_UBERSHADER_MAX_RASTER_CALLS) {
                set_error(error, error_capacity, "RT64 raster calls exceed ubershader evidence capacity");
                return 0;
            }

            const RT64::ShaderDescription &description = draw.triangles.shaderDesc;
            const bool copy_mode = description.otherMode.cycleType() == G_CYC_COPY;
            const bool z_compare = !copy_mode && description.otherMode.zCmp() &&
                                   (description.otherMode.zMode() != ZMODE_DEC);
            const bool z_update = !copy_mode && description.otherMode.zUpd();
            const bool coverage_add =
                (description.otherMode.cvgDst() == CVG_DST_WRAP) ||
                (description.otherMode.cvgDst() == CVG_DST_SAVE);
            const uint32_t pipeline_index =
                (uint32_t(z_compare) << 0U) |
                (uint32_t(z_update) << 1U) |
                (uint32_t(coverage_add) << 2U);
            const RenderPipeline *selected_pipeline = draw.triangles.pipeline;
            const RenderPipeline *expected_pipeline =
                shader_uber->pipelines[pipeline_index].get();

            evidence->shader_hashes[call_index] = description.hash();
            evidence->pipeline_state_indices[call_index] = pipeline_index;
            evidence->pipeline_identities[call_index] =
                static_cast<uint64_t>(reinterpret_cast<uintptr_t>(selected_pipeline));
            if ((selected_pipeline != nullptr) &&
                (selected_pipeline == expected_pipeline)) {
                evidence->matched_ubershader_call_count++;
            }

            for (uint64_t value : {
                     uint64_t(call_index),
                     uint64_t(static_cast<uint32_t>(draw.type)),
                     description.hash(),
                     uint64_t(description.otherMode.L),
                     uint64_t(description.otherMode.H),
                     uint64_t(description.colorCombiner.L),
                     uint64_t(description.colorCombiner.H),
                     uint64_t(description.flags.value),
                     uint64_t(pipeline_index)}) {
                digest_u64(descriptor_digest, value);
            }
            digest_u64(pipeline_digest, pipeline_index);
            digest_u64(
                pipeline_digest,
                static_cast<uint64_t>(reinterpret_cast<uintptr_t>(selected_pipeline)));
            evidence->raster_call_count++;
        }

        evidence->descriptor_digest = descriptor_digest;
        evidence->pipeline_digest = pipeline_digest;
        return 1;
#else
        set_error(error, error_capacity, "RT64 ubershader pipeline evidence requires Metal");
        return 0;
#endif
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 ubershader evidence query threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 ubershader evidence query failed with an unknown C++ exception");
        return 0;
    }
}

extern "C" int fn64_rt64_resize(
    Fn64Rt64Context *context,
    uint32_t width,
    uint32_t height,
    char *error,
    size_t error_capacity) {
    try {
        if ((context == nullptr) || (width == 0) || (height == 0)) {
            set_error(error, error_capacity, "RT64 resize requires a live context and non-zero dimensions");
            return 0;
        }
        std::array<uint32_t, 24> next_registers = context->registers;
        write_vi_registers(
            next_registers,
            context->output_addr,
            width,
            height,
            context->vi_state);
#if defined(__APPLE__)
    // Plume derives the next Metal drawable size from the native window; VI
    // dimensions alone leave the swapchain at its creation geometry.
    SDL_SetWindowSize(context->host_window, static_cast<int>(width), static_cast<int>(height));
    // The present worker can read Plume's old cached Cocoa size, enqueue its
    // refresh onto the caller's blocked main thread, and keep publishing old
    // framebuffers. Draining that worker, then resizing from the main thread
    // closes the cache-refresh interleaving before the next present is queued.
    RT64::PresentQueue *present_queue = context->application->presentQueue.get();
    present_queue->waitForIdle();
    {
        std::scoped_lock thread_lock(present_queue->threadMutex);
        // This pin-sensitive seam mirrors the selected MetalSwapChain::resize
        // body without its worker-owned asynchronous Cocoa attribute cache.
        auto *metal_swap_chain = dynamic_cast<plume::MetalSwapChain *>(
            context->application->swapChain.get());
        if ((metal_swap_chain == nullptr) || (metal_swap_chain->layer == nullptr)) {
            std::fputs("fn64 RT64 resize expected the pinned Metal swapchain\n", stderr);
            std::terminate();
        }
        int native_width = 0;
        int native_height = 0;
        SDL_GetWindowSize(context->host_window, &native_width, &native_height);
        plume::CocoaWindowAttributes cocoa_attributes{};
        metal_swap_chain->windowWrapper->getWindowAttributes(&cocoa_attributes);
        if ((native_width != static_cast<int>(width)) ||
            (native_height != static_cast<int>(height)) ||
            (cocoa_attributes.width != static_cast<int>(width)) ||
            (cocoa_attributes.height != static_cast<int>(height))) {
            std::fputs("fn64 RT64 resize could not synchronize native Metal geometry\n", stderr);
            std::terminate();
        }
        metal_swap_chain->layer->setDrawableSize(CGSizeMake(width, height));
        metal_swap_chain->width = width;
        metal_swap_chain->height = height;
        for (plume::MetalDrawable &drawable : metal_swap_chain->drawables) {
            drawable.desc.width = width;
            drawable.desc.height = height;
        }
        present_queue->swapChainFramebuffers.clear();
        context->application->sharedQueueResources->setSwapChainSize(width, height);
    }
#endif
    context->width = width;
    context->height = height;
    context->registers = next_registers;
        return 1;
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("RT64 resize threw: ") + exception.what());
        return 0;
    } catch (...) {
        set_error(error, error_capacity, "RT64 resize failed with an unknown C++ exception");
        return 0;
    }
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
