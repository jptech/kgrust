#!/bin/zsh
# Build the reference compiler — upstream's own `kanagawa`, its Haskell
# frontend — against the backend library `build-backend.sh` built, in a
# tree the parity gates can grade against. Users of `kg` never need it; the
# gates do: they record what the reference does (`xtask ptrace`, `output`)
# and run upstream's own CLI tests (`xtask cli`), which only upstream's CMake
# configures.
#
#   scripts/build-reference.sh [--backend-dir target/backend/dist/bin] [--build-dir target/reference]
#
# What it makes, in --build-dir (the gates' `KANAGAWA_BUILD`):
#   - upstream's CMake configured there — configured only, nothing built by
#     it — for the test definitions under test/;
#   - dist/bin/libkanagawa-backend.*, copied from --backend-dir, so the
#     reference and `kg` run the same backend;
#   - dist/bin/kanagawa, built with cabal from a copy of upstream's
#     compiler/ (nothing builds with its working directory inside the
#     reference tree), its rpath finding the library beside it.
#
# Needs ghc and cabal (ghcup; GHC 9.6.7, upstream's, used when present),
# CMake, Ninja and Boost.
set -eu
cd "$(dirname "$0")/.." || exit 1
root=$PWD
backend=$root/target/backend/dist/bin
build=$root/target/reference
while (( $# )); do
  case $1 in
    --backend-dir) backend=$2; shift 2 ;;
    --build-dir) build=$2; shift 2 ;;
    -h|--help) sed -n '2,/^set -eu/p' $0 | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) print -u2 "build-reference: unknown option $1"; exit 2 ;;
  esac
done
[[ $backend == /* ]] || backend=$root/$backend
[[ $build == /* ]] || build=$root/$build
lib=$(print -r -- $backend/libkanagawa-backend.(dylib|so)(N[1]))
[[ -n $lib ]] || { print -u2 "build-reference: no backend library in $backend — run scripts/build-backend.sh first"; exit 1 }

export PATH="$HOME/.ghcup/bin:$PATH"
ghc=$(command -v ghc-9.6.7 || command -v ghc || true)
[[ -n $ghc ]] || { print -u2 "build-reference: ghc not found — install it with ghcup"; exit 1 }
command -v cabal >/dev/null || { print -u2 "build-reference: cabal not found — install it with ghcup"; exit 1 }
src=$root/third_party/kanagawa

# The channel the library was built with, which the frontend is told too
# (compiler/hs/CMakeLists.txt).
channel=$(sed -n 's/^BUILD_CHANNEL:STRING=//p' ${backend:h:h}/CMakeCache.txt 2>/dev/null || true)
channel=${channel:-release}
case $channel in nightly) channel_num=1 ;; release) channel_num=2 ;; *) channel_num=0 ;; esac
IFS=. read -r major minor patch < $src/VERSION || true  # VERSION has no final newline

mkdir -p $build/dist/bin
if [[ ! -f $build/CMakeCache.txt ]]; then
  print "build-reference: configuring upstream's CMake in $build (configure only)"
  cmake -S $src -B $build -G Ninja -DCMAKE_BUILD_TYPE=RelWithDebInfo -DBUILD_CHANNEL=$channel \
    -DGHC_EXE=$ghc -DCABAL_EXE=$(command -v cabal) \
    ${CMAKE_PREFIX_PATH:+-DCMAKE_PREFIX_PATH=$CMAKE_PREFIX_PATH} \
    > $build/configure.log 2>&1 || { tail -30 $build/configure.log; print -u2 "build-reference: configure failed — $build/configure.log"; exit 1 }
fi
cp -p $lib $build/dist/bin/

work=$build/cabal
rm -rf $work/src && mkdir -p $work/src/cpp
cp -R $src/compiler/hs $src/compiler/kanagawa.cabal $src/compiler/LICENSE $work/src/
cp $src/compiler/cpp/*.h $work/src/cpp/
boost=$( (brew --prefix boost 2>/dev/null) || true )

# cabal's hsc2hs runs helper programs linked against the library, which
# find it by the executable's rpath (`@loader_path/../..`) from inside the
# build tree — so upstream copies it there first (compiler/hs/CMakeLists.txt,
# "On macOS, copy the shared library into the cabal build directory").
case $(uname -s)-$(uname -m) in
  Darwin-arm64) arch=aarch64-osx ;;
  Darwin-x86_64) arch=x86_64-osx ;;
  Linux-aarch64) arch=aarch64-linux ;;
  *) arch=x86_64-linux ;;
esac
exe_dir=$work/dist/build/$arch/ghc-$($ghc --numeric-version)/kanagawa-1.0.0/x/kanagawa/build/kanagawa
mkdir -p $exe_dir && cp -p $lib $exe_dir/

print "build-reference: building kanagawa with GHC $($ghc --numeric-version) (log: $work/build.log)"
(
  cd $work/src
  cabal v2-build exe:kanagawa \
    --with-compiler=$ghc \
    --extra-lib-dirs=$build/dist/bin \
    --builddir=$work/dist \
    --ghc-options=-DRELEASE_VERSION_MAJOR=$major \
    --ghc-options=-DRELEASE_VERSION_MINOR=$minor \
    --ghc-options=-DRELEASE_VERSION_PATCH=$patch \
    --ghc-options=-DBUILD_CHANNEL=$channel_num \
    ${boost:+--extra-include-dirs=$boost/include}
) > $work/build.log 2>&1 || { tail -30 $work/build.log; print -u2 "build-reference: failed — $work/build.log"; exit 1 }

exe=$(find $work/dist -path '*/x/kanagawa/build/kanagawa/kanagawa' -type f -perm +111 | head -1)
[[ -n $exe ]] || { print -u2 "build-reference: no executable under $work/dist"; exit 1 }
cp $exe $build/dist/bin/kanagawa
print "build-reference: $build/dist/bin/kanagawa (the gates' KANAGAWA_BUILD is $build)"
