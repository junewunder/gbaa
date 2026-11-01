# Patched dependencies

Crates from crates.io with a change of ours, used in their place through
`[patch.crates-io]` in `.cargo/config.toml` (so the generated crates in
`out/` get them too, built from this checkout). Each keeps its licence
files; what we changed is listed here.

## cmake 0.1.58

SDL3 is built from source by `sdl3-sys`'s build script, which prints no
`rerun-if` lines. Cargo reruns such a script only when its own package's
files change, and they never do, being in the registry. So a change to what
SDL's build reads from outside its sources (`crates/desktop/sdl-build/`'s
toolchain file and headers, the machine's X11 headers) went unseen, and when
the script did rerun, CMake kept every result in its old cache. A debug
build configured before the toolchain file looked up `libXext.so.6` kept
`XEXT_LIB-NOTFOUND`, so its X11 driver couldn't load and it opened no window
under gamescope (stretch/sdl-debug-rebuild).

The change: `src/inputs.rs`, called from `Config::build` after upstream's own
build-directory check. It reads the list file `CMAKE_INPUTS` names
(`crates/desktop/sdl-build/inputs.txt`), prints `rerun-if-changed` for each
path in it, and clears the build directory when the paths' contents hash
differs from the last run's stamp (`cmake-inputs.stamp` beside it), or when
there is no stamp yet. The environment variables the crate reads
(`CMAKE_TOOLCHAIN_FILE` and the rest) go into the hash too, each with a
`rerun-if-env-changed` (`inputs::env_read`, from `getenv_os`). Without
`CMAKE_INPUTS` the crate behaves as upstream.
`rust-version` is 1.74, for `OsStr::as_encoded_bytes`, and `#![allow(deprecated)]`
keeps upstream's use of `cc::Tool::env` from warning in every build.

To move to a newer cmake: copy its `src/`, `Cargo.toml.orig` (as
`Cargo.toml`), README and licences here, and put back `mod inputs;` and its
three calls (`track`, `env_read`, `clear_if_changed`) and the `allow`.
