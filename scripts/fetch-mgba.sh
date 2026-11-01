#!/usr/bin/env bash
# Clone mGBA at a pinned tag into $GBAA_DATA/tools/mgba/ (default
# ~/.local/share/gbaa) and build a static libmgba with no optional
# dependencies, for crates/mgba-dump to link (#55). Idempotent: a matching
# checkout with a built library is left alone. It takes the build lock
# around the compile itself: don't run it under the lock.
#
# 0.10.5 is the system flatpak's version. CMake 4 rejects mGBA's
# cmake_minimum_required, hence CMAKE_POLICY_VERSION_MINIMUM. LTO is off
# because GCC's LTO objects can't be read by the Rust linker (rust-lld),
# and PIC on because Rust links position-independent executables.
# mGBA master (fd9dc5c, 2026-10-18) was tried as a sharper oracle: it passes
# jsmolka's bios test 001 but still fails thumb 230 and now arm 522 (stmfd
# with the base in rlist), so 0.10.5 stays.
#
# `fetch-mgba.sh tas` builds BizHawk 2.9's mGBA instead (TASEmulators' fork
# at 645282a, 0.11-dev) into $GBAA_DATA/tools/mgba-tas/, for a TAS-only
# mgba-dump that replays movies made on it (june's OK, 2026-10-30). The
# oracle stays on 0.10.5.
set -euo pipefail

tag=0.10.5
repo=https://github.com/mgba-emu/mgba
lib=mgba
if [[ "${1:-}" == tas ]]; then
  tag=645282a39
  repo=https://github.com/TASEmulators/mgba
  lib=mgba-tas
fi
data="${GBAA_DATA:-$HOME/.local/share/gbaa}"
if [[ "$data" != /* ]]; then
  echo "GBAA_DATA must be an absolute path, got: $data" >&2
  exit 1
fi
dest="$data/tools/$lib"
src="$dest/src"
build="$dest/build"

flags=(-G Ninja -DCMAKE_BUILD_TYPE=Release -DBUILD_LTO=OFF -DCMAKE_POSITION_INDEPENDENT_CODE=ON
  -DCMAKE_POLICY_VERSION_MINIMUM=3.5 -DDISABLE_DEPS=ON -DBUILD_QT=OFF -DBUILD_SDL=OFF
  -DBUILD_GL=OFF -DBUILD_GLES2=OFF -DBUILD_GLES3=OFF -DENABLE_SCRIPTING=OFF
  -DUSE_DEBUGGERS=OFF -DM_CORE_GB=OFF -DBUILD_SHARED=OFF -DBUILD_STATIC=ON
  -DUSE_EDITLINE=OFF -DUSE_FFMPEG=OFF -DUSE_ZLIB=OFF -DUSE_MINIZIP=OFF -DUSE_PNG=OFF
  -DUSE_LIBZIP=OFF -DUSE_SQLITE3=OFF -DUSE_ELF=OFF -DUSE_LZMA=OFF -DUSE_DISCORD_RPC=OFF
  -DUSE_JSON_C=OFF -DUSE_LUA=OFF -DUSE_EPOXY=OFF -DUSE_FREETYPE=OFF
  -DBUILD_PERF=OFF -DBUILD_TEST=OFF -DBUILD_SUITE=OFF -DBUILD_CINEMA=OFF -DBUILD_EXAMPLE=OFF)

# The macros libmgba's sources were compiled with, one per line, for
# crates/mgba-dump/build.rs to compile its shim with: mCore's layout depends
# on some that build/include/mgba/flags.h leaves out (mGBA master's
# ENABLE_DEBUGGERS and ENABLE_DIRECTORIES), and a shim compiled without them
# calls through the wrong fields.
defines() {
  awk '/^build CMakeFiles\/mgba.dir\/src\/gba\/core.c.o/ { f = 1 } f && /^  DEFINES = / { print; exit }' \
    "$build/build.ninja" | sed 's/^  DEFINES = //' | tr ' ' '\n' | sed -n 's/^-D//p' > "$build/gbaa-defines"
  [[ -s "$build/gbaa-defines" ]] || { echo "mgba: no compile defines in $build/build.ninja" >&2; exit 1; }
}

# mGBA master's canvas demos (res/scripts/demos at fd9dc5c), for the
# canvas check (crates/desktop/tests/script_canvas.rs), into
# $data/tools/mgba-demos/, each checked against its hash.
demos="$data/tools/mgba-demos"
demo_rev=fd9dc5c2cbf234365068855851107e762fc6211b
mkdir -p "$demos"
while read -r sum name; do
  if ! echo "$sum  $demos/$name" | sha256sum -c --quiet > /dev/null 2>&1; then
    curl -sfL -o "$demos/$name" \
      "https://raw.githubusercontent.com/mgba-emu/mgba/$demo_rev/res/scripts/demos/$name"
    echo "$sum  $demos/$name" | sha256sum -c --quiet
  fi
done <<'SUMS'
e80be40104be7f0dec5abbf88ba756a97b4e5fde92e270e5605f7adee04261e8 text-demo.lua
ffc9ab245a79fc2b2dbf2a270d91688bd6c3c96f7d885654e1c9fa929a130934 logo-bounce.lua
670cfb8a12d33e5e30e4168e0272f120d77cb1fb99688b325b60d0c896a0b617 logo-fg.png
3012a89c4d8e6f43cd25c1519f5f14dade56335e7958c0dfa13d695ce5e9b5be logo-bg.png
08df266400933d3178d081a45f94a08814c3e55b4b7dd2e0ff69cb1329f13ab6 SourceSans3-Regular.otf
SUMS

# The checkout's tag, or for the fork (no tags) its short commit.
at() {
  if [[ "$lib" == mgba ]]; then
    git -C "$src" describe --tags --exact-match 2>/dev/null
  else
    git -C "$src" rev-parse --short=9 HEAD 2>/dev/null
  fi
}

# The flags the library was built with, so changing them here rebuilds it.
stamp="$build/gbaa-flags"
if [[ -d "$src/.git" ]] && [[ "$(at)" == "$tag" ]] \
  && [[ -f "$build/libmgba.a" ]] && [[ "$(cat "$stamp" 2>/dev/null)" == "${flags[*]}" ]]; then
  [[ -f "$build/gbaa-defines" ]] || defines
  echo "$lib: up to date ($tag, $build/libmgba.a)"
  exit 0
fi

if ! [[ -d "$src/.git" ]] || [[ "$(at)" != "$tag" ]]; then
  echo "mgba: fetching $tag"
  rm -rf "$src" "$build"
  mkdir -p "$dest"
  if [[ "$lib" == mgba ]]; then
    git clone -q --depth 1 --branch "$tag" "$repo" "$src"
  else
    git clone -q "$repo" "$src"
    git -C "$src" checkout -q "$tag"
  fi
fi

rm -rf "$build"
cmake -S "$src" -B "$build" "${flags[@]}"
# Under the build lock (justfile: lock), like every compile.
flock /tmp/gbaa-build.lock cmake --build "$build" -j 6 --target mgba
defines
echo "${flags[*]}" > "$stamp"
echo "$lib: built $build/libmgba.a"
