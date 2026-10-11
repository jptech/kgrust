#!/bin/zsh
# Build the reference's C++ backend, `libkanagawa-backend`, from the
# `third_party/kanagawa` submodule — CIRCT and LLVM with it, the Haskell
# frontend not at all (`backend/CMakeLists.txt` says how).
#
#   scripts/build-backend.sh                   # → target/backend/dist/bin/libkanagawa-backend.{dylib,so}
#   scripts/build-backend.sh --channel local   # with upstream's consistency checks, as a developer build
#   scripts/build-backend.sh --reference       # also the reference's own `kanagawa` (needs ghcup), for the parity gates
#
# Options:
#   --build-dir DIR      where to build (default target/backend)
#   --channel C          release (default: as upstream ships) or local
#   --build-type T       CMake build type (default RelWithDebInfo, upstream's)
#   --jobs N             parallel jobs (default: every core)
#   --reference          then scripts/build-reference.sh: the reference's own
#                        compiler against this library, in target/reference,
#                        for the parity gates (needs ghcup)
#
# Needs CMake ≥ 3.30, Ninja, a C++17 compiler and Boost ≥ 1.88 (headers
# only; `brew install cmake ninja boost`, or your distribution's packages).
# The first build compiles LLVM, MLIR and CIRCT: about an hour on 16 cores,
# ~2.5 GB of sources fetched (shallow) and ~15 GB of build tree. Later builds
# are incremental. Nothing is written inside the submodule but its own
# nested submodules' checkouts.
#
# Then use it: `export KG_BACKEND_DIR=$PWD/target/backend/dist/bin`, or
# `cargo xtask dist`, which packages it beside `kg`.
set -eu
cd "$(dirname "$0")/.." || exit 1
root=$PWD

build=target/backend channel=release type=RelWithDebInfo reference=0
jobs=$( (sysctl -n hw.ncpu || nproc) 2>/dev/null )
while (( $# )); do
  case $1 in
    --build-dir) build=$2; shift 2 ;;
    --channel) channel=$2; shift 2 ;;
    --build-type) type=$2; shift 2 ;;
    --jobs|-j) jobs=$2; shift 2 ;;
    --reference) reference=1; shift ;;
    -h|--help) sed -n '2,/^set -eu/p' $0 | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) print -u2 "build-backend: unknown option $1 (try --help)"; exit 2 ;;
  esac
done
[[ $channel == release || $channel == local ]] || { print -u2 "build-backend: --channel is release or local"; exit 2 }
[[ $build == /* ]] || build=$root/$build
src=$root/third_party/kanagawa

need() { command -v $1 >/dev/null || { print -u2 "build-backend: $1 not found — $2"; exit 1 } }
need cmake "install CMake 3.30 or later"
need ninja "install Ninja"
need git "install git"

# The sources: the submodule, then CIRCT and LLVM inside it, shallow — the
# commits upstream pins, fetched by hash.
if [[ ! -f $src/compiler/cpp/CMakeLists.txt ]]; then
  print "build-backend: fetching third_party/kanagawa"
  git submodule update --init third_party/kanagawa
fi
if [[ ! -f $src/thirdparty/circt/CMakeLists.txt ]]; then
  print "build-backend: fetching CIRCT (shallow)"
  git -C $src submodule update --init --depth 1 thirdparty/circt
fi
if [[ ! -f $src/thirdparty/circt/llvm/llvm/CMakeLists.txt ]]; then
  print "build-backend: fetching CIRCT's LLVM (shallow, ~2.5 GB)"
  git -C $src/thirdparty/circt submodule update --init --depth 1 llvm
fi

# One heavy run at a time on this machine: LLVM's links and the gates both
# take gigabytes (scripts/lib.sh).
source scripts/lib.sh
kg_heavy_lock "the backend build"
trap kg_heavy_unlock EXIT

print "build-backend: configuring $build ($type, channel $channel)"
cmake -S backend -B $build -G Ninja \
  -DCMAKE_BUILD_TYPE=$type \
  -DBUILD_CHANNEL=$channel \
  -DKANAGAWA_SOURCE_DIR=$src \
  ${CMAKE_PREFIX_PATH:+-DCMAKE_PREFIX_PATH=$CMAKE_PREFIX_PATH} \
  > $build.configure.log 2>&1 || { tail -40 $build.configure.log; print -u2 "build-backend: configure failed — $build.configure.log"; exit 1 }

print "build-backend: building kanagawa_lib with $jobs jobs (log: $build.build.log)"
ninja -C $build -j $jobs kanagawa_lib > $build.build.log 2>&1 || { tail -40 $build.build.log; print -u2 "build-backend: build failed — $build.build.log"; exit 1 }

lib=$(print -r -- $build/dist/bin/libkanagawa-backend.(dylib|so)(N[1]))
[[ -n $lib ]] || { print -u2 "build-backend: no library in $build/dist/bin"; exit 1 }
print "build-backend: $lib"

if (( reference )); then
  scripts/build-reference.sh --backend-dir $build/dist/bin
fi
print "build-backend: done. export KG_BACKEND_DIR=$build/dist/bin"
