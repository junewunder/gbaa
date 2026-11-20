# c-heap: what it draws

`just c-tests` builds this into `$GBAA_DATA/c/c-heap-agbcc.gba` and
`c-heap-gcc.gba`; both draw the same frames. The lifter's check runs both
(`just lift-check`, tables `c-heap-agbcc` and `c-heap-gcc`, on
`units/c-heap-<cc>-600.toml`).

Everything on screen lives on a heap: `heap_alloc` is a first-fit allocator
over `gHeap` (2 KB; EWRAM in GCC's build, `.bss` in agbcc's, which has no
section attribute), each block a 12-byte header and its bytes;
`heap_alloc_zeroed` calls it and clears the bytes; `heap_free` marks a
block free and joins it with a free one after it. Three call sites
allocate, each its own kind of object, with function pointers beside
numbers:

- `new_sprite` (through `heap_alloc_zeroed`): three 10x10 boxes, `update`
  at +0. Two bounce (red, green; `sprite_bounce`), one stays put and blinks
  blue and white every 16 frames (`sprite_blink`);
- `new_menu`: one menu, `draw` at +0 and `on_wrap` at +4. Every 32 frames
  its cursor, a yellow 16x8 bar at the bottom, moves 24 pixels right; after
  eight places it wraps, and `on_wrap` recolours an 8x8 square in the top
  right corner;
- `new_effect`: every 16 frames, into the next of three slots if it is
  empty, an effect that lives 20 frames and is then freed, so its memory is
  taken again: a `life` count at +0 (where a sprite keeps `update`) and
  `step` at +4, by turns a green bar that grows (`effect_grow`) and a
  magenta dot that falls (`effect_fall`).

Mode 3, BG2 on, black. Frame 600: hash `95692f5f4d510086`, the same in mGBA
0.10.5 and our interpreter (`just mgba-diff`, 2026-11-19).

The allocators are named in `holes/c-heap-<cc>.toml` (`allocators`), which
the function flow reads with `GBAA_FNREF_HEAP` set (native/fnref-heap-sites).
