# Building for Windows: cross-compiled from Linux, checked under Wine

stretch/windows, 2026-10-24. Unlike macOS (docs/research/macos-build.md),
Windows cross-compiles from Linux with nothing licensed to fetch:
`just windows-exe ROM` builds `out/windows/<name>/gbaa-<name>.exe` on any
factory machine, and `just windows-check NAME` runs it under Wine against
the Linux build (docs/tools/commands.md). Playing it on real Windows, with
a real pad, is june's.

## The toolchain

- **Target `x86_64-pc-windows-gnu`** (Rust tier 1), linked by MinGW-w64's
  gcc. Not `-msvc`: that needs Microsoft's CRT and SDK (`cargo-xwin`
  downloads them, under Microsoft's licence) and clang-cl, for no gain
  here. The GNU target links libgcc and winpthreads statically, so the
  .exe imports only Windows' own DLLs (`scripts/windows-package.sh`
  checks the list: kernel32, user32, ole32, setupapi, winmm, mmdevapi...).
- **SDL3** builds from source with cmake as on Linux, but the `cmake`
  crate doesn't tell CMake it is cross-compiling: without
  `CMAKE_TOOLCHAIN_FILE_x86_64_pc_windows_gnu` (Fedora's
  `/usr/share/mingw/toolchain-mingw64.cmake`) SDL configures for Linux
  and fails wanting X11. A failed configure stays in CMake's cache under
  `target/windows/.../build/sdl3-sys-*`: delete that before retrying.
  Built so, SDL3 has its Windows video driver with Direct3D 11/12,
  Vulkan and OpenGL renderers; WASAPI and DirectSound audio; and every
  Windows pad API: XInput (Xbox pads), Windows.Gaming.Input, RawInput,
  HIDAPI (PlayStation, Switch Pro and the like) and DirectInput. It loads
  XInput and the rest with LoadLibrary, so they aren't imports.
- **cpal** plays through WASAPI. **libc's `localtime_r`** (save backups'
  names) is POSIX only: `sav_file::stamp_now` calls `localtime_s` there.
- All of it lives in a podman image, `scripts/windows.Containerfile`
  (Fedora 44: `mingw64-gcc`, cmake, rustup's stable with the Windows
  target, Wine 11), so no machine needs MinGW or Wine installed; quack
  has neither. The image is about 4 GB; `just windows-image` builds it
  once (a couple of minutes) and Podman's cache keeps it.

## Running it under Wine

- Wine's first boot of a prefix needs `wine-common` (its winmd files):
  with only `wine-core` installed, wineboot asks setupapi for the missing
  media and retries for exactly five minutes before going on. The image
  installs it and boots the prefix at build time (Mono and Gecko off), so
  a run starts in about a second.
- Headless runs need no display: the check never opens a window, and the
  container sees no X or Wayland socket, so it can't open one on a desk.
- Paths: the checkout and the ROM's folder are mounted at the same paths
  as on the host and Wine's Z: is the container's root, so the Linux
  paths the route gives work unchanged (Rust reads `/home/...` as the
  current drive's root, which is Z:).

## Platform differences in the build

- Config files (`gamepad.conf`, `settings.conf`) are in
  `%APPDATA%\gbaa\`, not `~/.config/gbaa/`.
- Double-clicking passes no arguments, so a game's binary started with no
  ROM runs the ROM built into it (deck/embed-rom), as if it were
  `NAME.gba` beside it (`gbaa-NAME[.exe]`; `desktop::embedded_path`), so
  its `.sav` lands there. (Until 2026-10-30 the package carried the ROM
  beside the .exe and the binary looked for it there.)
- A console window opens with the game (the binary is a console program,
  so headless runs print to the terminal and errors show somewhere).
  Hiding it means `windows_subsystem = "windows"` in the generated
  `main.rs` and a message box for errors: not done.
- Saves are written beside the ROM, so the folder must be writable
  (not Program Files).

## What the check covers, and what it can't

`windows-check` compares, to the route's first checkpoint: every frame's
hash, the checkpoint PNG, the last registers, and the `.sav` both builds
write (when the game writes one), and the checkpoint's hash against the
route's. That proves the .exe loads, runs the recompiled code and the
interpreter fallback identically and reads and writes files. It can't
prove the window, sound or pads on Windows: Wine's drivers aren't
Windows', and the factory has no Windows machine or pad. Those are
june's to try on real Windows.
