#!/usr/bin/env bash
# Build the Direct3D bytecode translator used by avatar exports.
set -euo pipefail

prefix=${1:?usage: install-avatar-shader-tools.sh PREFIX}
mkdir -p "$prefix/bin"
prefix=$(realpath "$prefix")
compiler="$prefix/bin/vkd3d-compiler"

if [[ ! -x "$compiler" ]]; then
  build_dir=$(mktemp -d "${TMPDIR:-/tmp}/mtgoxml-shader-tools.XXXXXX")
  trap 'rm -rf "$build_dir"' EXIT

  source_archive="$build_dir/vkd3d-2.1.tar.xz"
  curl --fail --location --retry 3 --output "$source_archive" \
    https://dl.winehq.org/vkd3d/source/vkd3d-2.1.tar.xz
  printf '%s  %s\n' \
    7510146aff2adfb4ae07ab890701a607e5ff7c66e57100cfc9f630ec92eeda6a \
    "$source_archive" | sha256sum --check --status

  tar --extract --xz --file "$source_archive" --directory "$build_dir"

  if ! (
    cd "$build_dir/vkd3d-2.1" || exit
    ./configure --disable-demos --disable-tests --without-spirv-tools \
      --without-opengl --without-xcb --without-ncurses --disable-doxygen-doc \
      --disable-shared --disable-maintainer-mode || exit
    make include/private/vkd3d_version.h || exit
    make -j4 vkd3d-compiler
  ) > "$build_dir/build.log" 2>&1; then
    cat "$build_dir/build.log" >&2
    exit 1
  fi

  install -m 0755 "$build_dir/vkd3d-2.1/vkd3d-compiler" "$compiler.tmp"
  mv "$compiler.tmp" "$compiler"
fi

"$compiler" --version
if [[ -n ${GITHUB_PATH:-} ]]; then
  printf '%s\n' "$prefix/bin" >> "$GITHUB_PATH"
fi
