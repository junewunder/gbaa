---
title: Cross-building for Windows and running under Wine in podman
hook: SDL3 needs CMake's MinGW toolchain file; Wine without wine-common stalls 5 min at first boot (2026-10-24)
---

- SDL3's `build-from-source-static` configures for the host (and fails
  wanting X11) unless `CMAKE_TOOLCHAIN_FILE_x86_64_pc_windows_gnu` points at
  Fedora's `/usr/share/mingw/toolchain-mingw64.cmake`. Setting it later
  reruns SDL's build from an empty cache (vendor/README.md); before
  2026-10-25 the failed configure stuck and `sdl3-sys-*` had to be deleted.
- Fedora's `wine-core` alone lacks `/usr/share/wine/winmd`; wineboot then
  asks setupapi for the "media" and retries for exactly 5 minutes, on every
  fresh prefix (the first `wine` run too). Install `wine-common` and boot the
  prefix in the image (`scripts/windows.Containerfile`): runs start in ~1 s.
- `wineboot` isn't on PATH in that image: `wine wineboot -i`.
- Rust's windows-gnu .exe imports only Windows DLLs (libgcc, winpthreads
  static); host `objdump -p` reads PE imports, no MinGW needed on the host.
- Emerald Rogue's .exe is 125 MB stripped; its MinGW build ran ~15 min on
  quack with the heavy-slot queue.
