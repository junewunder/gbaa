# Tools on the factory machines

What's installed for jobs a unit reaches for, and the project's way to do
each. Recipes and their flags are in `docs/tools/commands.md`.

- **Looking at an image:** `just view-image <path>`, then Read the copy it
  prints. Never Read a full-size screenshot or frame dump.
- **Comparing or measuring images:** ImageMagick (`magick`, `compare`).
  For Python, Pillow isn't installed system-wide; run it through uv:
  `uv run --no-project --with pillow python3 script.py`. A check that
  stays in the repo goes in Rust (`aw-check` and `route-check` read PNGs).
- **Video and audio:** `ffmpeg`, `sox`.
- **Windows on a screen:** a private headless desktop (`cu session`) or
  `gamescope --backend headless`, never june's screen (`docs/factory.md`,
  Resources, GUI). Containers: `podman`.
- **A dependency's source:** the cargo registry has two index folders, so
  a `cd` into a glob is refused. Resolve the path first, then use it:
  `d=$(ls -d ~/.cargo/registry/src/index.crates.io-*/sdl3-0.20* | tail -1); grep -n … "$d"/src`.
- **Profiling:** `perf` on quack (`perf record -g -- <cmd>`, then
  `perf report --stdio`). It profiles your own processes without root
  (`kernel.perf_event_paranoid` is 2), so no kernel symbols. Run it on a
  release build under `just capped`.
