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

add_subdirectory(${BHPC_DYNARMIC_DIR} dynarmic EXCLUDE_FROM_ALL)

add_library(bhpc_shim STATIC ${BHPC_SHIM_DIR}/bhpc_shim.cpp)
target_include_directories(bhpc_shim PRIVATE ${BHPC_SHIM_DIR} ${BHPC_DYNARMIC_DIR}/src ${BHPC_DYNARMIC_DIR}/externals/mcl/include ${BHPC_DYNARMIC_DIR}/externals/fmt/include ${BHPC_DYNARMIC_DIR}/externals/robin-map/include)
target_link_libraries(bhpc_shim PRIVATE dynarmic)
install(TARGETS bhpc_shim ARCHIVE DESTINATION lib)
