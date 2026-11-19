#!/usr/bin/env bash
# Delete every CMake build tree in the restored build cache whose
# toolchain is no longer on the machine.
#
# cef-dll-sys builds libcef_dll_wrapper with CMake in its build script's
# output directory, which the Rust cache keeps. A CMake tree records the
# full path of the compiler it was configured with, and of the linker
# and archiver found beside it. On Windows that path names the MSVC
# toolset's version, so when a runner image brings a newer toolset the
# tree points at tools that are gone: CMake takes the new compiler from
# the command line, keeps the old linker from the tree, and fails its
# first test program with "no such file or directory". A tree in that
# state is of no use (its objects came from another compiler), so it is
# removed and the build script configures a new one.
#
#   ci/drop-stale-cmake-trees.sh [<directory>...]
#
# Each <directory> is searched for CMake build trees; the default is the
# output directory of every build script under `target/`.
set -euo pipefail

if [ "$#" -eq 0 ]; then
  shopt -s nullglob
  set -- target/*/build/*/out
fi

# The tools a tree was configured with: absolute paths only (a bare
# name is looked up again on every run and cannot go stale).
tools='^CMAKE_(C_COMPILER|CXX_COMPILER|LINKER|AR):[A-Z]+=(/|[A-Za-z]:[/\\])'

dropped=0
for root in "$@"; do
  [ -d "$root" ] || continue
  # CMake's own scratch projects under CMakeFiles/ are not trees to judge.
  while IFS= read -r cache; do
    tree=$(dirname "$cache")
    # A tree deleted earlier in this loop may have held this one.
    [ -f "$cache" ] || continue
    while IFS= read -r line; do
      tool=${line#*=}
      tool=${tool%$'\r'}
      if [ ! -e "$tool" ]; then
        echo "dropping $tree: ${line%%:*} was $tool, which is gone"
        rm -rf "$tree"
        dropped=$((dropped + 1))
        break
      fi
    done < <(grep -E "$tools" "$cache" || true)
  done < <(find "$root" -name CMakeCache.txt -not -path '*/CMakeFiles/*')
done
echo "$dropped stale CMake build tree(s) dropped"
