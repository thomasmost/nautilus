#!/usr/bin/env bash
# Write the update manifest (`latest.toml`) the browser's update check
# reads: the build's version, number and commit, and one entry per
# platform asset with its download URL, SHA-256 and size. The format is
# osmena-core's `update::Manifest`; a test there parses a sample.
#
#   ci/update-manifest.sh <version> <build> <commit> <release-url> <out> <platform>=<file>...
#
# <release-url> is the GitHub release page (`…/releases/tag/<tag>`); the
# asset URLs are derived from it (`…/releases/download/<tag>/<name>`).
# A <platform> is `<os>-<arch>` as Rust names them: windows-x86_64,
# macos-aarch64, macos-x86_64.
set -euo pipefail

if [ "$#" -lt 6 ]; then
  echo "usage: $0 <version> <build> <commit> <release-url> <out> <platform>=<file>..." >&2
  exit 2
fi
version=$1
build=$2
commit=$3
release_url=$4
out=$5
shift 5

download_base=${release_url/\/releases\/tag\//\/releases\/download\/}
published=$(date -u +%Y-%m-%dT%H:%M:%SZ)

{
  echo "version = \"$version\""
  echo "build = $build"
  echo "commit = \"$commit\""
  echo "published = \"$published\""
  echo "notes_url = \"$release_url\""
  for pair in "$@"; do
    platform=${pair%%=*}
    file=${pair#*=}
    if [ ! -f "$file" ]; then
      echo "$0: no such file: $file" >&2
      exit 1
    fi
    name=$(basename "$file")
    sha256=$(sha256sum "$file" | cut -d' ' -f1)
    size=$(stat -c %s "$file" 2>/dev/null || stat -f %z "$file")
    echo
    echo "[assets.$platform]"
    echo "name = \"$name\""
    echo "url = \"$download_base/$name\""
    echo "sha256 = \"$sha256\""
    echo "size = $size"
  done
} > "$out"

echo "wrote $out:"
cat "$out"
