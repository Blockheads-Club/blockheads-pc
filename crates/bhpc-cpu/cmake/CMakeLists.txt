# SPDX-License-Identifier: GPL-3.0-only
# Wrapper project: builds vendored dynarmic (A32 only) plus our C ABI shim as static libraries.
cmake_minimum_required(VERSION 3.16)
project(bhpc_cpu_native CXX C)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)
set(BUILD_TESTING OFF CACHE BOOL "" FORCE)
set(DYNARMIC_FRONTENDS "A32" CACHE STRING "" FORCE)
set(DYNARMIC_USE_BUNDLED_EXTERNALS ON CACHE BOOL "" FORCE)
set(DYNARMIC_TESTS OFF CACHE BOOL "" FORCE)
set(DYNARMIC_WARNINGS_AS_ERRORS OFF CACHE BOOL "" FORCE)
set(DYNARMIC_USE_PRECOMPILED_HEADERS OFF CACHE BOOL "" FORCE)

# CMake >= 3.30 drops the FindBoost module (CMP0167) and wants a Boost CMake package; the header-only
# Boost on GitHub's Windows runners and in many installs only has the module-style layout.
if (POLICY CMP0167)
    cmake_policy(SET CMP0167 OLD)
endif()

if (APPLE)
    # dynarmic's macOS fastmem handler runs `mig` and writes generated files into the (vendored,
    # read-only) source tree. Force the generic handler; bhpc-cpu then defaults fastmem off on macOS.
    set(MACH_EXC_DEFS_DIR "MACH_EXC_DEFS_DIR-NOTFOUND" CACHE PATH "" FORCE)
endif()

if (MSVC)
    # Keep the shim's flags in line with the dynarmic ones (conformance, big objects, no min/max macros).
    add_compile_options(/permissive- /Zc:__cplusplus /bigobj /DNOMINMAX)
endif()

# dynarmic's headers include Boost (variant, icl) and dynarmic links Boost::boost only PRIVATE, so the shim needs it
# too. On Linux the headers sit in /usr/include and this went unnoticed; Homebrew and Windows keep them elsewhere.
find_package(Boost 1.57 REQUIRED)

add_subdirectory(${BHPC_DYNARMIC_DIR} dynarmic EXCLUDE_FROM_ALL)

add_library(bhpc_shim STATIC ${BHPC_SHIM_DIR}/bhpc_shim.cpp)
target_include_directories(bhpc_shim PRIVATE ${BHPC_SHIM_DIR} ${BHPC_DYNARMIC_DIR}/src ${BHPC_DYNARMIC_DIR}/externals/mcl/include ${BHPC_DYNARMIC_DIR}/externals/fmt/include ${BHPC_DYNARMIC_DIR}/externals/robin-map/include)
target_link_libraries(bhpc_shim PRIVATE dynarmic Boost::boost)
install(TARGETS bhpc_shim ARCHIVE DESTINATION lib)
