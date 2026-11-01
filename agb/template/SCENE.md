# template: expected frame

`just agb-tests` builds this into `$GBAA_DATA/agb/template.gba`. The scene
is set up once before the first VBlank and never changes, so every frame
from the first is the same; check it at frame 10 or later:

    just first-frame "$GBAA_DATA/agb/template.gba" 10
    just mgba-diff "$GBAA_DATA/agb/template.gba" 10

Mode 0, BG0 and OBJ on, 1D OBJ mapping. Colours are BGR555.

| Pixel (x, y)   | Colour   | Why                                         |
|----------------|----------|---------------------------------------------|
| (0, 0)         | `0x001F` | BG0 tile 1 (red), map entries (0..3, 0..3)  |
| (31, 31)       | `0x001F` | last pixel of BG0's red 32x32 square        |
| (32, 0)        | `0x4210` | BG0 tile 0 is transparent: backdrop (grey)  |
| (0, 32)        | `0x4210` | backdrop                                    |
| (16, 16)       | `0x03E0` | sprite 0, 8x8 green, over the red square    |
| (23, 23)       | `0x03E0` | sprite 0's last pixel                       |
| (24, 24)       | `0x001F` | just past sprite 0: BG0 again               |
| (100, 60)      | `0x03E0` | sprite 1, 16x16 green, over the backdrop    |
| (115, 75)      | `0x03E0` | sprite 1's last pixel                       |
| (116, 76)      | `0x4210` | just past sprite 1: backdrop                |
| (239, 159)     | `0x4210` | backdrop                                    |

Every other pixel is one of those three colours: red inside (0..31, 0..31),
green inside sprite 0's and sprite 1's boxes, grey elsewhere.

## Copying it

A unit testing a screen feature copies `agb/template/` to `agb/<name>/`,
renames the package in `Cargo.toml` and `Cargo.lock` (`name = "<name>"`),
rewrites `scene()` in `src/main.rs` and this file's table, and keeps VRAM,
palette and OAM writes to halfwords (`store16`, `fill16`, `tile4`). The
runtime's unit tests have the same rule and the same helpers in
`crates/runtime/src/test_util.rs` (`test_util::ppu`).
