# UI spikes

Throwaway spikes for docs/research/ui-library.md; not in the workspace and
not built by `just check`. Each crate has its own `[workspace]` and lock file.

- `overlay/`: winit + softbuffer, a menu drawn by hand into the 240×160 frame.
- `egui/`: winit + glutin + egui_glow, the frame as a texture and an egui menu.
- `sizes/*/`: hello worlds, built only to measure clean build time and size.

Build one outside the repo's target, inside the build lock:

    cd spikes/ui/egui && env CARGO_BUILD_JOBS=6 CARGO_TARGET_DIR=/tmp/ui-spike-egui \
        flock /tmp/gbaa-build.lock cargo build --release

Run it unattended in Game Mode's setting, scripting the menu with
`SPIKE_NAV` (see `common.rs`) and taking screenshots with `gamescopectl
screenshot <path>` from inside the session:

    SPIKE_NAV="4000 menu down right ok 1500 quit" \
        gamescope --backend headless -W 1280 -H 800 -- /tmp/ui-spike-egui/release/ui-egui

Pass `--fullscreen` to start fullscreen; `SPIKE_VSYNC=1` makes the egui
spike wait for vblank. Each prints present-gap and draw-time statistics on exit.
