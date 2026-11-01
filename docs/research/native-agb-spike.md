# Spike: an agb program as a native Rust program

Unit `native/agb-spike`, 2026-10-31/11-01 on quack. The first native port,
by hand, at the smallest scale: `agb/native-spike` (a program of ours
written for it) lifted to `crates/native-spike`, a Rust program with no
`Machine`, no registers and no GBA addresses, per
docs/native-port-design.md (sections 1-3 and 7). Its hardware is a
throwaway adapter; the real library is `native/hw-library`.

## What was built

- **`agb/native-spike`** (agb 0.25, about 330 lines, `SCENE.md`): a 16x16
  sprite the D-pad moves (B doubles the speed, A steps its palette bank,
  it flips when it last moved left) over a 32x32-tile background that a
  dead-zone camera scrolls, in a 1024-pixel world with clamped edges. A
  V-blank handler (agb's `add_interrupt_handler`) counts frames in a
  global, and the count blinks the sprite's centre. Set-up writes VRAM,
  palette and OAM by halfword stores to fixed addresses, then the loop is
  wait for V-blank, read `KEYINPUT`, update, commit (scroll, OAM entry 0,
  one palette colour, `DISPCNT`).
- **`crates/native-spike`**:
  - `src/game.rs`, the lift (about 270 lines with the ROM's tables):
    `Game { ram: Ram, hw: Hw }`; `Ram` holds the one global
    (`frames`); `main(g) -> Result<Infallible, Exit>` keeps the
    program's own loop with `hw::wait_vblank(g)?` as its wait; the
    handler is `fn vblank(g: &mut Game)`, installed with
    `g.hw.irq.on_vblank(vblank)`; stores are fields
    (`v.bg[0].scroll_x = cx`, `v.oam[0].attr1 = ..`,
    `v.obj_palette[i] = ..`) and the set-up loops are slice copies
    (`v.screen_block(31).copy_from_slice(&MAP)`).
  - `src/hw.rs`, the adapter: `Hw { video, keys, irq }` with
    `Video { dispcnt, bg: [Bg; 4], bg_palette, obj_palette, vram, oam:
    [Obj; 128] }`. At each `wait_vblank` it copies `Video` into a
    `runtime::bus::Bus` (one, kept) at its addresses, draws the 160 lines with the
    runtime's own `Ppu::render_line`, hands the frame to a `Host`,
    latches the keys, then runs the V-blank handler. `Host::present`
    returning `false` makes the wait return `Err(Exit)`, which `?`
    carries out of `main`: the design's `Reset` shape, used for stopping.
  - `src/main.rs`, the program: gbaa-desktop's headless flags
    (`--headless --frames N --input PRESSES --frame-hashes F`, plus
    `--dump`), parsed and hashed with the desktop's own `Script` and
    `frame_hash`, and the keys indexed as the desktop's
    (`script.keys(shown - 1)` after each present).
  - `src/bin/check.rs` (`native-spike-check`) and `check.toml`: the check.
- **`just native-spike`**: recompiles the ROM at `--timing exact`, builds
  it, builds the native program plain and with `--features plant` (one
  wrong store: OAM entry 0's x one pixel off while facing left), and runs
  all three on `check.toml`'s 600-frame script.

## The check, measured (quack, 2026-11-01)

```
native = play: 600 frames identical (510 distinct pictures), hashes file 5539f4ec2af9bb48, last frame 600 hash e40446740c70e154
planted wrong store caught: play "frame 212 hash f6dee7bb5c6ee4b7", planted "frame 212 hash f60d58b8ffc489bf"
```

The script holds a D-pad key in every 60-frame window (the checker
checks), walks out of the camera's dead zone right and down, into the world's
left and top edges (the sprite's and the camera's clamps both act on
each), presses B and A. The right and bottom edges, 800 pixels away,
aren't reached. Frame 212 is the first frame
drawn after the first Left (pressed from frame 210, latched at frame 211's
V-blank). Before the full check, the native program also matched the
interpreter's hashes over 200 frames of a shorter script.

Both take about 0.2 s for 600 frames headless; drawing dominates both
(the play build's game code ran 854,624 cycles in all, 1.4 k a frame).

## What was hard

- **Time, at once.** The first agb version computed its tiles and map at
  run time (`% 3`, `% 5` per pixel: calls into the division routine).
  Set-up then took longer than the first frame, so in the play build the
  first wait returned a frame late and the display came on at frame 3;
  a native program, where code takes no time, shows it at frame 2. That
  is section 7's lag problem at the smallest scale, met before the first
  frame. The spike side-stepped it in the agb source (the tables became
  `const`, so set-up is a few thousand cycles) and kept set-up from
  showing anything (DISPCNT 0, backdrop colour unwritten until the first
  commit), so frame 1 is black whatever set-up costs. A real game's
  boot will cross V-blanks (Emerald's does), and its frame count from
  power-on depends on how long its code took: the exact play build
  cannot be the oracle there. `--timing native`
  (`q/may-native-port-s-check`) would make the play build show it at
  frame 2 too, since its code takes no time either. Evidence for that
  question.
- **The frame boundary.** Matching hashes needed the native program's
  frame N to be exactly the play build's: frame N is what the beam drew
  before the Nth V-blank, the keys latched after it are the script's
  frame N - 1 (desktop's `shown - 1`), and the handler runs after the
  present. Drawing all 160 lines at the wait instead of line by line is
  only right because the program changes the display in V-blank alone;
  the real `wait_vblank` must draw from the beam's line, as the design
  says.
- **Little else.** With the same renderer on both sides the adapter
  matched on the first run. Nothing in the program needed a hole.

## Done by hand that the lifter must do

The ROM has 107 functions (discovery 104); the program's own code is
two of them: `main` (set-up, update and commit inlined into it by
rustc) and the handler's closure. Everything else is agb, `core` and
`compiler_builtins`. What the hand lift did, as rules for the printer:

1. **Recognise agb's library and replace it, not lift it.** The entry
   and crt0 (`.data`/`.iwram` copies), `add_interrupt_handler` (a
   heap-allocated, pinned linked list of boxed closures walked by agb's
   IWRAM master handler, under `critical_section`), `VBlank::get` and
   `wait_for_vblank` (`IE`, `DISPSTAT`, `swi VBlankIntrWait`), and the
   allocator behind them. Lifted literally they are a heap, a list of
   function pointers and an IRQ dispatcher in IWRAM; replaced, they are
   `g.hw.irq.on_vblank(f)` and `hw::wait_vblank(g)?`. That is the
   `library-matching` research applied to agb: a recognised library maps
   to `Hw` calls, an unrecognised one is lifted and its IRQ list is a hole
   (an indirect call through heap data).
2. **Constant-address stores become fields**: `0x0400_0010` to
   `bg[0].scroll_x`, `0x0700_0002` to `oam[0].attr1`, `0x0500_0204 +
   bank * 32` to `obj_palette[bank * 16 + 2]`. Needs each store's address
   as a value set (codegen's), then the I/O map; an address computed in
   a loop indexes the array behind it.
3. **Copy loops become slice copies**: a counted loop of halfword loads
   from ROM and stores to VRAM is `copy_from_slice`. Without that idiom
   it is still correct as `vram[base + i] = ROM_TABLE[i]` in a loop.
4. **ROM data becomes typed constants**, by the width the code reads it
   at (here `[u16; N]`). The hand lift reproduced the tables with the agb
   source's `const fn`s; a lifter prints the bytes.
5. **`KEYINPUT` inverted is `keys.held()`**: `!load16(KEYINPUT) & 0x3FF`
   recognised, or printed as is through `hw.io`.
6. **Globals to `Ram`, locals to locals.** The frame counter is a
   `.bss` global; the game state (`State`) lives in `main`'s stack frame,
   and stays a local only if stack slots are recovered as locals; in
   stage A (byte RAM) it is bytes of a stack array.
7. **The frame loop keeps its shape.** `loop { wait; read; update;
   commit }` printed as is, the wait a call returning `Result` so the
   host can stop it, needed nothing special.

## What the hardware library should take from the adapter

- `wait_vblank` as a free function over `&mut Game`, since the handler
  needs all of `g`: right, as the design said.
- A `Host` (the design's `Platform`) whose `present` can stop the program
  is enough for headless runs; `Exit` through `?` works and costs the
  lifted code nothing but a `?` at each wait.
- VRAM as halfwords with views (`tiles4(cb, t)`, `screen_block(n)`) read
  well for 4bpp text backgrounds; OAM as `[Obj; 128]` with the affine
  halfword kept in each entry, palettes as two `[u16; 256]`.
- Copying `Video` into a `Bus` every frame (96 KB of VRAM) is fine for a
  spike and is what `native/hw-library`'s split removes.

## What the next units should know

- **`native/hw-library`**: the adapter's API is a starting point, not a
  design. A line-by-line `wait_vblank` from the beam's position, H-blank
  and V-count handlers, and `io.write16` by offset are untested here.
- **`native/timing-native`** and the timing question
  (`q/may-native-port-s-check`): the spike's first version is a
  worked example of why exact timing fails as the native oracle (a boot
  that takes over a frame), worth putting to june with the question.
- **The lifter** (`native/lift-stage-a`): its first target, this ROM,
  is 98% agb library by function count; without recognising agb, the
  output is mostly allocator and critical sections. Library recognition
  for agb comes before or with stage A for agb programs.
- No window yet: `crates/native-spike` is headless only. A window wants
  the desktop's `Platform` behind `Host`, which the real library gives.
