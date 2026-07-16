cmake_minimum_required(VERSION 3.20)
project(fn64_rt64_shim LANGUAGES C CXX)

if(NOT DEFINED FN64_RT64_SOURCE_DIR)
    message(FATAL_ERROR "FN64_RT64_SOURCE_DIR must name the sibling MIT RT64 checkout")
endif()

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(RT64_STATIC ON CACHE BOOL "Build only RT64's static render/HLE library" FORCE)
set(BUILD_SHARED_LIBS OFF CACHE BOOL "Keep RT64 and its dependencies static" FORCE)
set(NFD_BUILD_TESTS OFF CACHE BOOL "Do not build nativefiledialog tests" FORCE)
set(NFD_INSTALL OFF CACHE BOOL "Do not generate nativefiledialog install target" FORCE)
set(ZSTD_BUILD_PROGRAMS OFF CACHE BOOL "Do not build zstd tools" FORCE)
set(ZSTD_BUILD_TESTS OFF CACHE BOOL "Do not build zstd tests" FORCE)
set(PLUME_BUILD_EXAMPLES OFF CACHE BOOL "Do not build plume examples" FORCE)

# EXCLUDE_FROM_ALL plus building only fn64_rt64_shim means CMake traverses
# RT64's `rt64` static target and its render dependencies, not its standalone
# texture tools. This source tree defines no mupen plugin target; its GPL
# mupen64plus subtree is therefore neither compiled nor linked.
add_subdirectory("${FN64_RT64_SOURCE_DIR}" rt64-build EXCLUDE_FROM_ALL)

# plume acquires several autoreleased convenience-factory results, then manually
# releases them from commit/end/destructor paths. When RT64's workload thread
# exits, its implicit autorelease pool releases the already-deallocated AGX blit
# context and crashes in objc_release. Retain at acquisition so plume's existing
# releases balance. Cover both persistent encoder variants because they share
# the same ownership shape even though framebuffer readback currently exercises
# the blit encoder.
# Keep this exact-source check loud: a changed upstream ownership path must be
# reviewed rather than receiving a stale textual patch silently.
if(APPLE)
    set(FN64_PLUME_METAL_SOURCE
        "${FN64_RT64_SOURCE_DIR}/src/contrib/plume/plume_metal.cpp")
    file(READ "${FN64_PLUME_METAL_SOURCE}" FN64_PLUME_METAL_CONTENTS)
    set(FN64_PLUME_COMMAND_BUFFER_ACQUIRE
        "        mtl = queue->mtl->commandBufferWithUnretainedReferences();\n        mtl->setLabel(MTLSTR(\"RT64 Command List\"));")
    set(FN64_PLUME_COMMAND_BUFFER_RETAIN
        "        mtl = queue->mtl->commandBufferWithUnretainedReferences();\n        mtl->retain();\n        mtl->setLabel(MTLSTR(\"RT64 Command List\"));")
    string(FIND
        "${FN64_PLUME_METAL_CONTENTS}"
        "${FN64_PLUME_COMMAND_BUFFER_ACQUIRE}"
        FN64_PLUME_PATCH_OFFSET)
    if(FN64_PLUME_PATCH_OFFSET EQUAL -1)
        message(FATAL_ERROR
            "plume Metal command-buffer ownership source changed; review the fn64 retain patch")
    endif()
    string(REPLACE
        "${FN64_PLUME_COMMAND_BUFFER_ACQUIRE}"
        "${FN64_PLUME_COMMAND_BUFFER_RETAIN}"
        FN64_PLUME_METAL_PATCHED
        "${FN64_PLUME_METAL_CONTENTS}")
    set(FN64_PLUME_BLIT_ENCODER_ACQUIRE
        "            activeBlitEncoder = mtl->blitCommandEncoder(device->sharedBlitDescriptor);\n            activeBlitEncoder->setLabel(MTLSTR(\"Copy Blit Encoder\"));")
    set(FN64_PLUME_BLIT_ENCODER_RETAIN
        "            activeBlitEncoder = mtl->blitCommandEncoder(device->sharedBlitDescriptor);\n            activeBlitEncoder->retain();\n            activeBlitEncoder->setLabel(MTLSTR(\"Copy Blit Encoder\"));")
    string(FIND
        "${FN64_PLUME_METAL_PATCHED}"
        "${FN64_PLUME_BLIT_ENCODER_ACQUIRE}"
        FN64_PLUME_BLIT_PATCH_OFFSET)
    if(FN64_PLUME_BLIT_PATCH_OFFSET EQUAL -1)
        message(FATAL_ERROR
            "plume Metal blit-encoder ownership source changed; review the fn64 retain patch")
    endif()
    string(REPLACE
        "${FN64_PLUME_BLIT_ENCODER_ACQUIRE}"
        "${FN64_PLUME_BLIT_ENCODER_RETAIN}"
        FN64_PLUME_METAL_PATCHED
        "${FN64_PLUME_METAL_PATCHED}")
    set(FN64_PLUME_RESOLVE_ENCODER_ACQUIRE
        "            activeResolveComputeEncoder = mtl->computeCommandEncoder(MTL::DispatchTypeConcurrent);\n            activeResolveComputeEncoder->setLabel(MTLSTR(\"Resolve Texture Encoder\"));")
    set(FN64_PLUME_RESOLVE_ENCODER_RETAIN
        "            activeResolveComputeEncoder = mtl->computeCommandEncoder(MTL::DispatchTypeConcurrent);\n            activeResolveComputeEncoder->retain();\n            activeResolveComputeEncoder->setLabel(MTLSTR(\"Resolve Texture Encoder\"));")
    string(FIND
        "${FN64_PLUME_METAL_PATCHED}"
        "${FN64_PLUME_RESOLVE_ENCODER_ACQUIRE}"
        FN64_PLUME_RESOLVE_PATCH_OFFSET)
    if(FN64_PLUME_RESOLVE_PATCH_OFFSET EQUAL -1)
        message(FATAL_ERROR
            "plume Metal resolve-encoder ownership source changed; review the fn64 retain patch")
    endif()
    string(REPLACE
        "${FN64_PLUME_RESOLVE_ENCODER_ACQUIRE}"
        "${FN64_PLUME_RESOLVE_ENCODER_RETAIN}"
        FN64_PLUME_METAL_PATCHED
        "${FN64_PLUME_METAL_PATCHED}")
    set(FN64_PLUME_TEXTURE_DESCRIPTOR_ACQUIRE
        "        MTL::TextureDescriptor *descriptor = MTL::TextureDescriptor::textureBufferDescriptor(pixelFormat, width, options, usage);\n        this->texture = buffer->mtl->newTexture(descriptor, 0, bytesPerRow);")
    set(FN64_PLUME_TEXTURE_DESCRIPTOR_RETAIN
        "        MTL::TextureDescriptor *descriptor = MTL::TextureDescriptor::textureBufferDescriptor(pixelFormat, width, options, usage);\n        descriptor->retain();\n        this->texture = buffer->mtl->newTexture(descriptor, 0, bytesPerRow);")
    string(FIND
        "${FN64_PLUME_METAL_PATCHED}"
        "${FN64_PLUME_TEXTURE_DESCRIPTOR_ACQUIRE}"
        FN64_PLUME_TEXTURE_DESCRIPTOR_PATCH_OFFSET)
    if(FN64_PLUME_TEXTURE_DESCRIPTOR_PATCH_OFFSET EQUAL -1)
        message(FATAL_ERROR
            "plume Metal texture-descriptor ownership source changed; review the fn64 retain patch")
    endif()
    string(REPLACE
        "${FN64_PLUME_TEXTURE_DESCRIPTOR_ACQUIRE}"
        "${FN64_PLUME_TEXTURE_DESCRIPTOR_RETAIN}"
        FN64_PLUME_METAL_PATCHED
        "${FN64_PLUME_METAL_PATCHED}")
    set(FN64_PLUME_FUNCTION_NAME_ACQUIRE
        "        this->functionName = (entryPointName != nullptr) ? NS::String::string(entryPointName, NS::UTF8StringEncoding) : MTLSTR(\"\");")
    set(FN64_PLUME_FUNCTION_NAME_RETAIN
        "        this->functionName = (entryPointName != nullptr) ? NS::String::string(entryPointName, NS::UTF8StringEncoding) : MTLSTR(\"\");\n        this->functionName->retain();")
    string(FIND
        "${FN64_PLUME_METAL_PATCHED}"
        "${FN64_PLUME_FUNCTION_NAME_ACQUIRE}"
        FN64_PLUME_FUNCTION_NAME_PATCH_OFFSET)
    if(FN64_PLUME_FUNCTION_NAME_PATCH_OFFSET EQUAL -1)
        message(FATAL_ERROR
            "plume Metal shader-name ownership source changed; review the fn64 retain patch")
    endif()
    string(REPLACE
        "${FN64_PLUME_FUNCTION_NAME_ACQUIRE}"
        "${FN64_PLUME_FUNCTION_NAME_RETAIN}"
        FN64_PLUME_METAL_PATCHED
        "${FN64_PLUME_METAL_PATCHED}")
    set(FN64_PLUME_DEBUG_NAME_ACQUIRE
        "        debugName = NS::String::string(name.c_str(), NS::UTF8StringEncoding);\n        library->setLabel(debugName);")
    set(FN64_PLUME_DEBUG_NAME_RETAIN
        "        debugName = NS::String::string(name.c_str(), NS::UTF8StringEncoding);\n        debugName->retain();\n        library->setLabel(debugName);")
    string(FIND
        "${FN64_PLUME_METAL_PATCHED}"
        "${FN64_PLUME_DEBUG_NAME_ACQUIRE}"
        FN64_PLUME_DEBUG_NAME_PATCH_OFFSET)
    if(FN64_PLUME_DEBUG_NAME_PATCH_OFFSET EQUAL -1)
        message(FATAL_ERROR
            "plume Metal shader-debug-name ownership source changed; review the fn64 retain patch")
    endif()
    string(REPLACE
        "${FN64_PLUME_DEBUG_NAME_ACQUIRE}"
        "${FN64_PLUME_DEBUG_NAME_RETAIN}"
        FN64_PLUME_METAL_PATCHED
        "${FN64_PLUME_METAL_PATCHED}")
    set(FN64_PLUME_METAL_PATCHED_SOURCE
        "${CMAKE_CURRENT_BINARY_DIR}/fn64_plume_metal.cpp")
    file(WRITE
        "${FN64_PLUME_METAL_PATCHED_SOURCE}"
        "${FN64_PLUME_METAL_PATCHED}")

    # plume declared the source relative to its own source directory. Mark that
    # entry non-compiling in plume's directory scope before adding the patched
    # translation unit; mutating the target's SOURCES list leaves both object
    # files in Xcode/Makefile generators.
    set_source_files_properties(
        "${FN64_PLUME_METAL_SOURCE}"
        TARGET_DIRECTORY plume
        PROPERTIES HEADER_FILE_ONLY TRUE)
    target_sources(plume PRIVATE "${FN64_PLUME_METAL_PATCHED_SOURCE}")
endif()

# AppleClang 21 no longer makes the C `labs` declaration transitively visible
# to RT64's bundled hlslpp scalar backend. Inject the standard declaration at
# compile time without modifying the sibling checkout. The warning suppressions
# mirror RT64's own Clang >=20 branch, which CMake does not select when the
# compiler id is spelled AppleClang.
if(APPLE AND CMAKE_CXX_COMPILER_ID STREQUAL "AppleClang")
    target_compile_options(rt64 PRIVATE
        "$<$<COMPILE_LANGUAGE:CXX>:-include>"
        "$<$<COMPILE_LANGUAGE:CXX>:stdlib.h>"
        "$<$<COMPILE_LANGUAGE:CXX>:-Wno-nontrivial-memcall>"
        "$<$<COMPILE_LANGUAGE:CXX>:-Wno-deprecated-literal-operator>"
    )
endif()

add_library(fn64_rt64_shim STATIC fn64_rt64_shim.cpp)
target_compile_definitions(fn64_rt64_shim PRIVATE
    HLSL_CPU
    FFX_GCC
    IMGUI_IMPL_VULKAN_NO_PROTOTYPES
)
target_include_directories(fn64_rt64_shim PRIVATE
    "${FN64_RT64_SOURCE_DIR}/src"
    "${FN64_RT64_SOURCE_DIR}/src/contrib"
    "${FN64_RT64_SOURCE_DIR}/src/contrib/plume"
    "${FN64_RT64_SOURCE_DIR}/src/contrib/plume/contrib/metal-cpp"
    "${FN64_RT64_SOURCE_DIR}/src/contrib/hlslpp/include"
    "${FN64_RT64_SOURCE_DIR}/src/contrib/imgui"
    "${FN64_RT64_SOURCE_DIR}/src/contrib/nativefiledialog-extended/src/include"
    "${CMAKE_CURRENT_BINARY_DIR}/rt64-build/src"
)
target_link_libraries(fn64_rt64_shim PRIVATE rt64)
