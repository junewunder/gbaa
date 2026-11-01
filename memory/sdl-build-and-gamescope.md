---
title: SDL3's static build, and testing a window under gamescope
hook: spotting SDL's X11 driver; gamescope in a lane needs WAYLAND_DISPLAY unset; cu type can overtake cu key
---

- A binary whose SDL3 lacks a working X11 driver says "starting SDL's video:
  No available video device" under gamescope (Xwayland only). Quick test
  without running it: `strings target/debug/gbaa-desktop | grep -x libXext.so.6`
  (empty means SDL was configured with `XEXT_LIB-NOTFOUND`; see
  `target/<profile>/build/sdl3-sys-*/out/build/CMakeCache.txt`).
- Since stretch/sdl-debug-rebuild, SDL's build reruns from an empty CMake
  cache whenever `crates/desktop/sdl-build/inputs.txt`'s paths or the cmake
  env vars change (vendor/README.md); no more deleting `sdl3-sys-*` by hand.
  A path in the list that doesn't exist is only noticed with the next change
  to one that does: after installing a -devel package elsewhere,
  `cargo clean -p sdl3-sys`.
- A lane's shell has `WAYLAND_DISPLAY=/dev/null/no-display`; gamescope then
  fails with "Failed to connect to wayland socket". Run
  `env -u WAYLAND_DISPLAY -u DISPLAY gamescope --backend headless -W 1280 -H 800 -- BIN ROM --pad-script '4000 start+select'`;
  `timeout 15` ends it (exit 124 is the window having run).
- Under `gamescope --backend headless`, `--fullscreen` opens at 1280x800
  but SDL reports the window's fullscreen state as Off, so the window
  widens for script sidebars/the console and gamescope scales it down.
  Since scripting/gamescope-fullscreen (2026-10-29) a window filling its
  display counts as fullscreen (`window::is_fullscreen`): the log says
  "fullscreen on" there and the console overlays the 5x frame.
  For fullscreen screenshots use a `cu session new --size 1280x800`
  (KWin confirms fullscreen) and `cu -s NAME key f9` / `shot` (2026-10-26).
- Two netplay windows in a `cu session` (multiplayer/menu-netplay,
  2026-10-29): start `gbaa-server` inside the session (`cu session run S
  -- sh -c '…'`), not with `just long` (it queues behind any heavy job).
  `cu type` right after a run of `cu key backspace` can land before the
  keys: wait a second between them, and screenshot a field before using
  it. Use `cu session new --fresh-profile` so the menu's saved settings
  don't touch june's config.
