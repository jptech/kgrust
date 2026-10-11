# The reference's C++ backend, `libkanagawa-backend`, built from the
# `third_party/kanagawa` submodule — and nothing else of it.
#
# Upstream's own CMake builds the whole toolchain from its root: the
# Haskell frontend, the tests, the tools; its configure step fails without
# ghc and cabal (`build/cmake/deps.cmake`). `kg` needs only the backend
# library, so this project does what upstream's root and `thirdparty/` do
# for that one target — the same CIRCT and LLVM settings, upstream's own
# `compiler/cpp/CMakeLists.txt` unchanged — and builds `kanagawa_lib` alone.
#
# Driven by `scripts/build-backend.sh`; see it for the options. The library
# lands in `<build>/dist/bin`, the layout upstream's build uses and `kg`
# looks for (`KG_BACKEND_DIR`, or beside the executable).

cmake_minimum_required(VERSION 3.30)
project(kgrust_backend LANGUAGES C CXX)

set(KANAGAWA_SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../third_party/kanagawa"
    CACHE PATH "The upstream kanagawa checkout to build the backend from")
get_filename_component(KANAGAWA_SOURCE_DIR "${KANAGAWA_SOURCE_DIR}" ABSOLUTE)
if(NOT EXISTS "${KANAGAWA_SOURCE_DIR}/compiler/cpp/CMakeLists.txt")
  message(FATAL_ERROR "No kanagawa checkout at ${KANAGAWA_SOURCE_DIR}. Run `git submodule update --init third_party/kanagawa`.")
endif()
if(NOT EXISTS "${KANAGAWA_SOURCE_DIR}/thirdparty/circt/llvm/llvm/CMakeLists.txt")
  message(FATAL_ERROR "CIRCT and its LLVM are not checked out under ${KANAGAWA_SOURCE_DIR}/thirdparty. `scripts/build-backend.sh` fetches them.")
endif()

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED True)

# `release` is what upstream's published builds use: it defines
# KANAGAWA_SKIP_CONSISTENCY_CHECKS (compiler/cpp/CMakeLists.txt), which drops
# a print-and-reparse of every design's MLIR, a location check after every
# global optimisation, and the "Running internal consistency checks." line
# every run prints otherwise. `local` keeps them, as a developer build does.
set(BUILD_CHANNEL "release" CACHE STRING "release (as upstream ships) or local (with the consistency checks)")

# What upstream's root CMakeLists.txt defines and compiler/cpp reads.
set(BUILD_OUTPUT_DIR "${CMAKE_BINARY_DIR}/dist")
set(BUILD_OUTPUT_BIN_DIR "${BUILD_OUTPUT_DIR}/bin")
set(KANAGAWA_SHARED_LIBRARY_NAME "kanagawa-backend")

# What upstream's build/cmake/deps.cmake finds for the backend: Boost's
# headers and threads. Not Verilator, ghc or cabal.
cmake_policy(SET CMP0167 NEW)
find_package(Boost 1.88 CONFIG REQUIRED HINTS ${Boost_DIR} ${Boost_DIR}/lib/cmake)
find_package(Threads REQUIRED)

# CIRCT and LLVM, as upstream's thirdparty/CMakeLists.txt configures them.
if(UNIX)
  set(CMAKE_POSITION_INDEPENDENT_CODE ON)
endif()
set(LLVM_ENABLE_PROJECTS "mlir" CACHE STRING "Enable MLIR project")
set(BUILD_SHARED_LIBS "OFF" CACHE STRING "Kanagawa doesn't use shared libs")
set(LLVM_TARGETS_TO_BUILD "X86" CACHE STRING "Part of CIRCT needs the X86 target")
set(STANDALONE_INSTALL "OFF" CACHE STRING "Kanagawa only uses libs from CIRCT")
set(LLVM_EXTERNAL_PROJECTS "circt" CACHE STRING "Enable the CIRCT project")
set(LLVM_EXTERNAL_CIRCT_SOURCE_DIR "${KANAGAWA_SOURCE_DIR}/thirdparty/circt" CACHE STRING "Location of CIRCT")
set(LLVM_ENABLE_TERMINFO "OFF" CACHE STRING "Don't use terminfo library")
set(CLANG_TIDY_DISABLE "ON")
set(SYSTEMC_DISABLE "ON")
set(YOSYS_DISABLE "ON")
set(IVERILOG_DISABLE "ON")
set(CAPNP_DISABLE "ON")
set(OR_TOOLS_DISABLE "ON")
set(CIRCT_LLHD_SIM_ENABLED "OFF")
set(CIRCT_LEC_DISABLE "ON")
set(CIRCT_BINDINGS_PYTHON_ENABLED "OFF")
set(CIRCT_BINDINGS_TCL_ENABLED "OFF")

# Ours, beyond upstream's: nothing but the libraries the backend links is
# configured or built, and the library depends on nothing a user's machine
# may lack (no zlib, zstd or libxml2), so it can be shipped as it is.
set(LLVM_INCLUDE_TESTS "OFF" CACHE BOOL "")
set(LLVM_INCLUDE_BENCHMARKS "OFF" CACHE BOOL "")
set(LLVM_INCLUDE_EXAMPLES "OFF" CACHE BOOL "")
set(LLVM_INCLUDE_DOCS "OFF" CACHE BOOL "")
set(MLIR_INCLUDE_TESTS "OFF" CACHE BOOL "")
set(MLIR_INCLUDE_INTEGRATION_TESTS "OFF" CACHE BOOL "")
set(CIRCT_INCLUDE_TESTS "OFF" CACHE BOOL "")
set(CIRCT_INCLUDE_DOCS "OFF" CACHE BOOL "")
set(LLVM_ENABLE_ZLIB "OFF" CACHE STRING "")
set(LLVM_ENABLE_ZSTD "OFF" CACHE STRING "")
set(LLVM_ENABLE_LIBXML2 "OFF" CACHE STRING "")
set(LLVM_ENABLE_BINDINGS "OFF" CACHE BOOL "")
# Linking LLVM's tools takes gigabytes each; two at a time keeps a 16-core
# build inside memory.
set(LLVM_PARALLEL_LINK_JOBS "2" CACHE STRING "")

add_subdirectory("${KANAGAWA_SOURCE_DIR}/thirdparty/circt/llvm/llvm" "${CMAKE_BINARY_DIR}/llvm")

# Upstream's backend, its CMakeLists.txt as it is. It includes
# ${CMAKE_SOURCE_DIR}/build/cmake/msvc.cmake, which `build/cmake/msvc.cmake`
# here forwards to upstream's.
add_subdirectory("${KANAGAWA_SOURCE_DIR}/compiler/cpp" "${CMAKE_BINARY_DIR}/kanagawa-backend")
