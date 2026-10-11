#!/bin/zsh
# Builds a differential oracle from the reference's own frontend.
#
# The reference tree is READ-ONLY, so its sources are copied here at build time
# rather than vendored: the oracle then always tracks the reference instead of
# drifting from a snapshot nobody remembers taking. `lib/` is disposable and
# gitignored.
#
# Only the *library* is built. The reference's executable links a C++ backend
# (`kanagawa-backend`, see `compiler/kanagawa.cabal`); the frontend library has
# no such dependency, which is why this works at all and why "there is no GHC
# here" was true of the executable but never of the part we need.
set -e
HERE=${0:a:h}
REF=${KANAGAWA_ROOT:-$HERE/../third_party/kanagawa}
export PATH="$HOME/.ghcup/bin:$PATH"

[[ -d $REF/compiler/hs/lib ]] || { print -u2 "no reference at $REF"; exit 1 }
rm -rf $HERE/lib && cp -R $REF/compiler/hs/lib $HERE/lib
cd $HERE && cabal build kg-oracle "$@"
print "\noracle: $(find $HERE/dist-newstyle -name kg-oracle -type f -perm +111 | head -1)"
