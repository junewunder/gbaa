# Headers for building SDL3

SDL3 is built from source (`sdl3`'s `build-from-source-static`), and its
X11 video driver, which the Steam Deck's Game Mode needs (gamescope gives
games Xwayland only), is left out unless `X11/extensions/Xext.h` is found.
That header comes in libXext's development package, which a machine may
lack (quack does). This copy, from libXext 1.3.7 (MIT, its notice in the
file), is found through `CPATH` and `CMAKE_INCLUDE_PATH` in
`.cargo/config.toml`. SDL loads libXext.so.6 itself at run time.

A machine's own copy, where it has one, does as well; the rest of what
SDL's X11 and Wayland drivers need to build is listed in
docs/tools/commands.md (gbaa-desktop).

`inputs.txt` lists what SDL's build reads from outside its sources (this
directory, the machine's X11 and Wayland headers): a change to any of them
reruns it from an empty CMake cache (vendor/README.md says how).
