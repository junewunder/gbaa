# The native port: a game as a plain Rust program

Unit `native/design`, 2026-10-31 on quack. june's decision of the same day
(`d/modded-builds-are-plain-rust-programs-no-save`, decisions.md 2): a
modded build is literally a Rust program. No registers, no GBA memory
block, no CPU, nothing kept for save states, rewind or rollback; as regular
and pretty as we can make it. Decision 2 left "how a game gets from the
emulated machine to that program" undesigned; this is that design.

It starts from what exists: the analysis (`crates/analysis`), the
structured codegen (`docs/structured-codegen-design.md`), the mod view and
its readable form (`docs/mod-view-design.md`), the registers and types
review (`docs/research/registers-and-types.md`) and the ROM-addresses
research (`docs/research/code-without-rom-addresses.md`). Nothing here is
built yet. The units it proposes are at the end.

## Summary

- **A native port is a lift, not a translation of the machine.** The
  play build translates instructions and keeps the machine (`Machine`:
  `cpu`, `bus`, `ppu`, `sched`; `crates/runtime/src/machine.rs:28`). The
  native port takes the same analysis (functions, CFGs, value sets, jump
  tables, copies) and the same structuring plan (`codegen::plan`), and
  prints functions over typed state that call a hardware library. Every
  fact a ROM address carried is used up while lifting; none is left in the
  program.
- **State is one `Game` struct passed as `&mut Game`**: its `ram`, a
  plain-data `Ram` struct with one field per global, typed from DWARF
  where a decomp has it (`codegen::types` already reads pret's types as
  far as layout), `#[repr(C)]` so the game's own byte-wise code
  (checksums, `memcpy`, save blocks) still works, and its `hw`. Where no
  types are known yet, a region is a byte array field. Pointers are typed
  handles (`Ref<T>`, offsets into `Ram`), not addresses.
- **The hardware is a library, `Hw`, a field of `Game`**: the runtime's
  own PPU, sound, timers and BIOS routines, refactored to read a `Video`
  and an `Audio` struct instead of the `Bus`
  (`crates/runtime/src/ppu.rs:143`, `render_line(&mut self, bus: &Bus,
  y)`), so one renderer serves the play build and the native port. Lifted
  code writes `g.hw.ppu.bg[0].scroll_x = x`, not a store to `0x0400_0010`.
  This is what the SM64 and OoT PC ports did with the N64's display lists:
  keep the game's interface to the hardware, replace what consumes it.
- **Time moves only where the code waits or looks at it.** Code takes no
  time. `hw.wait_vblank()` draws the frame, runs the V-blank handler,
  presents and polls the keys; a busy loop with no stores gets
  `hw.idle()` on its back edge; reading `VCOUNT` or a timer advances the
  beam; the library's BIOS copies, fills, decompressions and DMA advance
  it by their cost (section 7). Interrupt handlers are callbacks the
  library runs at those points.
- **Function pointers are `FnRef` IDs**: a `u32` newtype indexing a
  generated table, so equality, storage in task data and Emerald's
  split-into-halves all keep working. Tables of functions in ROM are
  `static [fn(&mut Game); N]`. ROM addresses are analysis-time facts only:
  WarioWare's `sub lr, lr, #24` becomes a jump to a fixed block at lift.
- **Anything not resolved statically is a build error or a listed hole.**
  There is no interpreter to fall back on. The hole list
  (`holes.toml` per game) names each one and what stands in for it (a
  hand-written Rust function, a stub); `native/hole-report` counts them.
- **The oracle is the play build.** A check build of the native program
  lays its state out at the guest's addresses and numbers its functions
  by their ROM addresses (two build parameters, no change to the code), so
  its RAM compares byte for byte with the play build's at every game
  frame, beside the frames. That needs one new play-build timing,
  `--timing native` (time moves where the native port's moves), which is
  june's call (asked; see "The oracle").

## What we start from

- **Analysis** (`crates/analysis`). `discover` (`discovery.rs:145`)
  gives function starts with their evidence, every instruction reached,
  literal pools and tables as data, the BIOS copies into IWRAM
  (`Discovery::copies`) and their entries (`ram_entries`), and the
  jump-table recovery over every root (`Recovery`, `jumptable.rs:26`):
  each indirect jump `Resolved`, `Unresolved` or `Rejected`
  (`vsa/func.rs:110`) and the far-jump `bl`s; each function's analysis
  (`FuncResult`) also lists its calls that never return.
  A `bl` whose leaf callee returns somewhere computed is already an
  indirect jump at the `bl` (`vsa/func.rs:622`), which is how
  `__gnu_thumb1_case_*` resolves.
- **Structuring** (`codegen::cfg`, `codegen::plan`): Ramsey's
  translation per function with a recompile-time check
  (`plan::check`, `plan.rs:492`), irreducible regions collapsed into one
  `loop { match }` node. 99.9% of Emerald's functions are reducible.
- **The readable form** (`docs/mod-view-design.md`): names from the source
  map, typed globals as `Ptr<T>` constants with field accessors
  (`codegen::types`, `Ty` at `types.rs:30`), I/O registers by name
  (`codegen::io_regs`, pret's `REG_*` table), prototypes as parameters and
  return values (`Types::proto_of`), DWARF locals where liveness agrees
  (`codegen::dwarf`), bitfield writes as `set_bits`. Its accesses still go
  through the `Game` trait (`emit.rs:413`), whose `get`/`set` take a
  guest address and the instruction's address, and the machine still
  shows as sync blocks, pool loads and `tick!`: 64% of the spike's lines.
- **The runtime's hardware** (`crates/runtime`): the PPU per scanline
  with every mode, sprites, windows, blending and mosaic (`ppu.rs`), the
  sound FIFOs and PSG (`sound.rs`), timers, DMA in the bus store
  (`dma.rs`), the BIOS in Rust (`bios.rs:346`, `bios::swi`; `cpu_set` and
  `lz77` in `bios/decomp.rs:42,191`, `div` in `bios/math.rs:14`), saves
  (`save.rs`) and the RTC (`gpio.rs`). All of it reads and writes the
  `Bus` (`bus.rs:67`), the GBA's memory map as arrays.

What the native port needs that none of these give: a lift from IR values
to typed expressions without the guest registers (the readable form keeps
them in `m.cpu.r`), function signatures that are trusted (decision 10's
summaries, never built), points-to facts for pointers, and a hardware API
that isn't the memory map.

## 1. Where game state lives

**One struct, `Game`, passed to every function as `g: &mut Game`.**
Rust's `static mut` needs `unsafe` at every access (and the 2024
edition's `static_mut_refs` lint refuses references to them), and statics
make two instances in one process impossible (two consoles for link
play, tests that run a function from a chosen state). A struct passed by
`&mut` is ordinary Rust, safe, and testable function by function.

```rust
#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct Ram {
    pub gMain: Main,
    pub gTasks: [Task; 16],
    pub gSaveBlock2Ptr: Ref<SaveBlock2>,
    pub gHeap: [u8; 0x1c000],
    // … one field per RAM global, in link order
}

pub struct Game {
    pub ram: Ram, // plain data: what the game's own code can see as bytes
    pub hw: Hw,   // the library: renderer state, the Platform; not plain data
}
// Deref/DerefMut to Ram, so lifted code reads `g.gMain.callback2`; the
// printer writes `g.ram.` only where a borrow must be split from `g.hw`.
```

`Game` itself is never viewed as bytes: `Hw` owns the platform and the
renderer's state, which aren't plain data. Everything the game can
address is in `Ram`.

- **Names** are the source map's, through the mod view's identifier
  rules (`docs/mod-view-design.md`, "Names as Rust identifiers"), under
  `#![allow(non_snake_case)]`: `g.gMain.callback2` reads as pret's
  `gMain.callback2`. A game with no map gets `d_<offset>` per datum the
  analysis finds (as `f_<key>` names functions today; a name, not an
  address the program uses).
- **Types from DWARF where a decomp has them.** `codegen::types` already
  turns `[types]` into layouts (`u8`/`u16`/`u32` by size, `Ptr<T>`,
  arrays, struct and union markers). The native printer emits real
  structs instead of marker types: each field with its DWARF offset, a
  `const` assertion of every offset and the size, padding as explicit
  `_pad` fields, unions as `#[repr(C)] union` only where the code reads
  one member and writes another, else the member the code uses. Scalars
  get their signedness from DWARF (`i8`, `s16` as `i16`), which the
  readable form leaves to shifts.
- **`#[repr(C)]` and plain-old-data, on purpose.** Games treat their
  state as bytes: Emerald checksums and writes its save blocks a sector at
  a time, copies structs with `CpuSet`, clears them with `memset`. Every
  struct derives `zerocopy`'s `FromBytes`/`IntoBytes`, so `as_bytes()` is
  safe and those routines keep working (and a native Emerald reads real
  `.sav` files). That is ordinary Rust memory with a C layout, not the
  GBA's map: no 0x0200_0000, no I/O or VRAM in it, and globals in any
  order the lifter chooses.
- **A byte array only where types aren't known yet.** A game with no
  decomp starts with each RAM region's globals as one field,
  `ewram: [u8; 0x40000]` and `iwram: [u8; 0x7f00]`, laid out as the game
  has them, reached through `Ref<u8>`-style accessors
  (`g.ewram.u16(off)`). That is the first native form any game can reach
  (section 6's stage A); typing then carves fields out of it one datum at
  a time, each carve a check run, as pret's own history did with
  `gUnknown_` names.
- **The stack is gone.** Stack slots become Rust locals (the frame
  recovery of `registers-and-types.md`, "Stack slots": a slot whose
  address never escapes is a local; one whose address is passed to a
  callee is a local passed as `&mut`). A slot reached through computed
  offsets that types don't explain is a hole (section 6), or, in stage A,
  a local byte array.
- **The heap is the game's own.** Emerald's allocator over `gHeap` is game
  code; lifted, it allocates inside `g.gHeap`, and the pointers it gives
  are `Ref<T>` typed by where they're stored (`sBattleStructPtr:
  Ref<BattleStruct>`). Replacing it with `Box` is a later, optional step
  that a mod may take; the oracle then compares heap objects through their
  roots rather than the arena's bytes.

### Pointers into state

A C pointer becomes one of three things, chosen per variable or field
from the points-to facts the lifter gathers (value sets already track
constant addresses plus offsets; `vsa/domain.rs`):

1. **`&mut T` / `&T`, for a parameter that doesn't escape** (stored nowhere,
   not returned) and whose callee touches no other path to the same
   object. `GetBagItemQuantity(quantity: &u16)`. Needs a borrow proof
   per call site: the argument is a field path and the callee's effects
   (its summary's globals read and written) don't include that path's
   root. Where the proof fails, form 2.
2. **An index, where every value it can hold points into one array.**
   `struct Sprite *` that is always `&gSprites[i]` becomes `SpriteId(u8)`
   with `g.gSprites[id]`. The prettiest form, and the common one: task
   IDs and sprite IDs are already indices in pret's C.
3. **`Ref<T>`, otherwise**: a `#[repr(transparent)] struct Ref<T>(u32)`,
   the offset of a `T` inside `Ram` (or inside one of its byte regions),
   resolved by `g.at(r)` / `g.at_mut(r)` (a bounds- and alignment-checked
   `zerocopy` view of `g.ram`'s bytes). It keeps the pointer's 4-byte size, so structs keep
   their layout; `r.add(n)` is typed pointer arithmetic within an array;
   null is `Ref::NULL`. It is a typed handle, not an address: its value
   has no meaning outside this program's `Ram`.

Pointers to ROM data are separate: section 8.

## 2. The hardware as a library

**`Hw`: the GBA's devices behind typed fields and methods**, built from
the runtime's own code so that both builds draw with one renderer and mix
with one mixer (the oracle then sees only the game's differences, never
two PPUs').

```rust
g.hw.ppu.dispcnt = DispCnt::MODE_0 | DispCnt::BG0 | DispCnt::OBJ | DispCnt::OBJ_1D;
g.hw.ppu.bg[0].cnt = BgCnt::new().priority(1).char_base(0).screen_base(31);
g.hw.ppu.bg[0].scroll_x = g.gBattle_BG0_X;
g.hw.ppu.oam[i] = g.gMain.oamBuffer[i];        // an OamEntry, typed
g.hw.ppu.palette.bg[16 * p..16 * p + 16].copy_from_slice(&pal);
let keys = g.hw.keys.held();                    // KEYINPUT, pressed = 1
g.hw.sound.fifo_a.stream(Ref::of(&g.gSoundInfo.pcmBuffer));
```

- **What it is, concretely.** `runtime::ppu` reads `bus.io` (the
  display registers, `Ppu::render_line` at `ppu.rs:143`), `bus.vram`,
  `bus.palette` and `bus.oam` (in `ppu/bg_text.rs`, `bg_affine.rs`,
  `bitmap.rs` and `obj.rs`; `backdrop` at `ppu.rs:234`). Split those four out of
  `Bus` into a `Video` struct (registers decoded into fields, plus the
  three memories), have the PPU take `&Video`, and have `Bus` own a
  `Video` and route the 0x0400_0000-0x0400_0056, 0x0500, 0x0600 and 0x0700
  regions into it. The play build is unchanged in behaviour (its accessors
  still see bytes at addresses); the native port owns a `Video` directly.
  The same for sound (`Audio`: the FIFOs, PSG registers and
  `SOUNDCNT`), timers and keys. The library stays `no_std`, so the
  ESP32 stretch goal keeps working for native ports too.
- **Registers are fields, decoded once.** `DISPCNT`, `BGxCNT`, the
  windows, `BLDCNT`, mosaic: `bitflags`-style types with the PPU's
  existing constants (`ppu::dispcnt`, `ppu.rs:59`). Write-only registers
  that the game also tracks in RAM (scroll positions) are fields the game
  writes; reading one the hardware can't read back gives what the play
  build's bus gives (open bus, or the last write where `Io` keeps it).
- **The three memories stay memories**, because that is what they are on
  the hardware and in every game: VRAM `[u8; 0x18000]` with typed views
  (`vram.tiles4(char_base)`, `vram.screen(base)`), the palette as
  `[Rgb555; 256]` twice, OAM as `[OamEntry; 128]` plus the affine
  parameters it interleaves. Copies into them are library calls
  (section 8).
- **An escape hatch that isn't a hole:** `g.hw.io.write16(off, v)` and
  `read16(off)`, by offset into the I/O block, for code that addresses
  registers through a table or a loop (agb's register initialisation,
  a game's "write this list of registers" helper). The library decodes
  the offset into the field. It is static and checkable; the printer uses
  it only where the address isn't constant.
- **The CPU is gone, and the BIOS with it.** `swi`s become library
  calls with Rust signatures: `bios::div(n, d) -> (i32, i32, u32)`,
  `bios::sqrt`, `bios::arctan2`, `bios::lz77(src: &[u8], dst: &mut
  [u8])` (VRAM's 16-bit writes kept), `bios::rl`, `bios::huff`,
  `bios::bg_affine_set`, `bios::obj_affine_set` (from `bios/math.rs` and
  `bios/decomp.rs`, refactored from `(bus, regs)` to slices). `CpuSet`
  and `CpuFastSet` are copies (section 8). `SoftReset` returns to `main`,
  which builds a fresh `Game`. The waits are the frame loop (section 3).
- **Saves are the library's.** The play build emulates SRAM, flash and
  EEPROM chips (`runtime::save`), and games talk to them with the vendor's
  command sequences (writes to 0x0E00_5555). The native port replaces the
  vendor's save library (Nintendo's `AgbFlash`, `agb_flash`; recognised
  as a library by its code, the `library-matching` research) with
  `g.hw.save.read(sector, &mut buf)`/`write`, persisted as the same `.sav`
  file the play build writes, so the two read each other's saves. A game
  whose save code isn't recognised keeps its chip protocol, through
  `hw.save.cmd8(addr, v)`, the chip emulation's own entry. Built so far
  (native/flash-stack-standin): `Hw::save` is the chip emulation itself,
  and the three routines `AgbFlash` copies onto the stack and calls there
  (`ReadFlash1`, `ReadFlash_Core`, `VerifyFlashSector_Core`; recognised at
  the call by their bytes, `hw::flash::recognise`) are `hw::flash::{read_byte, read,
  verify}` over it. A lifted program's `call_fn` makes that call when a
  call into RAM finds no function there but one of the routines
  (native/lift-flash-routines; Metroid's SRAM library calls the same
  agbcc code).
- **Library code with its own convention is the runtime's.** libgcc's
  soft-float `__muldf3` and `__divdf3` (modern GCC's games: moonbeam,
  Emerald Rogue, Imperium) call their special-case paths with r4 and r5
  coming back changed, which the lift's calling convention can't carry.
  `analysis::softfloat` recognises them by their code, and every lift
  prints each as a call of `runtime::softfloat::{muldf3, divdf3}`,
  bit-exact on libgcc's results (native/libgcc-df-standins;
  `just softfloat-check ROM` pins them against the game's own copy).

**Prior art.** The PC ports of Super Mario 64 (`sm64-port`, `sm64ex`) and
Ocarina of Time (Ship of Harkinian, on `libultraship`) compile the
decomps' C for a PC and keep the game's interface to the hardware: the
game still builds N64 display lists with the `gSP*`/`gDP*` macros, and a
renderer (`gfx_pc.c`, "Fast3D") consumes those lists and draws them with
OpenGL or Direct3D; the audio microcode the RSP ran is reimplemented in C
(SM64's `mixer.c`). They didn't rewrite the game to the PC's model; they
replaced the consumer of the game's commands. For the GBA the "commands"
are register writes and the three memories, so our library keeps exactly
those, and its consumer is the PPU we already have. Higher-level
replacements (sprites as quads, widescreen, 60 fps tweens) are what those
ports grew afterwards, from that base, and are mods here too.

## 3. The frame loop, and interrupt handlers as callbacks

### Time

The play build counts cycles (decision 11; instant timing still counts one
per block exit). A native program has no instructions to count. Its rule:
**code takes no time; time moves only at the places the code waits for it
or looks at it**:

| In the game | Native | What the library does |
|---|---|---|
| `VBlankIntrWait`, `IntrWait`, `Halt` (BIOS waits) | `g.hw.wait_vblank(g)`, `wait_irq(mask)`, `halt()` | runs the beam to the next matching interrupt (below) |
| a loop with no stores and no calls (Emerald's `WaitForVBlank` spins on `gMain.intrCheck`, `structured-codegen-design.md`'s `f_080008ad`) | `g.hw.idle()` on its back edge | runs to the next event of any kind, then the loop re-tests |
| a read of `VCOUNT`, `DISPSTAT` or a timer's counter | `g.hw.ppu.vcount()`, `g.hw.timer[n].count()` | advances the beam one step (a line; for a timer, its smallest observable tick) before answering, so a poll loop finishes |

That one rule covers both a poll on a RAM flag the handler sets (the
idle back edge) and a poll on the beam itself (`while (REG_VCOUNT !=
100);` spins once a line until the beam reaches 100), without recognising either.
What the analysis must supply is the list of store-free, call-free loops,
from the CFGs and loops `codegen::cfg` already computes; a loop that
stores (a delay loop counting in RAM, `for (i = 0; i < 0x4000; i++) ;`
with `i` on the stack) is a local count with no observable effect and
needs nothing.

### The loop

```rust
fn main() {
    let mut platform = platform::Desktop::open("Pokémon Emerald");
    loop {
        let mut g = Game::new(&mut platform);  // RAM zeroed, as at power-on
        // AgbMain never returns; SoftReset returns Reset up the stack
        // (`-> Result<Infallible, Reset>`, `?` at each call that can reach it).
        let Err(Reset) = game::AgbMain(&mut g);
    }
}
```

**The default keeps the game's own control flow.** `AgbMain`'s loop
stays a `loop`, and its wait is a call: `g.hw.wait_vblank(g)` (in Rust,
a free function `hw::wait_vblank(g: &mut Game)`, since the library calls
handlers that need all of `g`). That call is the frame end: it finishes
the frame's lines, runs the V-blank handler, presents, hands the audio
over and polls the keys, then returns. It works wherever the game waits:
in `AgbMain`, or deep in a fade loop four calls down, which older games
do and which an inverted `fn frame()` couldn't express. It is also the
ordinary shape of a native game with vsync in `present`. A host that must
own the loop (a browser, an ESP32 task) runs `AgbMain` on a thread or a
stackful coroutine, as the desktop already runs the guest
(`crates/desktop/src/coro.rs`, `corosensei`).

**Inversion where it applies.** Where every wait is in the outermost loop
body of `AgbMain` (Emerald's: `ReadKeys`, the two callbacks, `WaitForVBlank`,
once a frame), the lifter can also print `impl Game { pub fn frame(&mut
self) }` with that body and a `main` that calls it, which is what the SM64
port did by hand (`game_loop_one_iteration`). Prettier, optional, and
checked the same way.

### Inside a wait

`wait_vblank` runs the rest of the frame from the beam's current line:

1. **Visible lines, up to 160.** For each line: draw it
   (`Ppu::render_line`, the same per-line drawing the play build does, so
   mid-frame register changes show as on hardware); then, if enabled, the
   H-blank handler; at the `DISPSTAT` V-count match, the V-count handler.
   H-blank DMA (section 8) runs here too.
2. **Line 160: V-blank.** The V-blank handler; V-blank DMA.
3. **Present, audio, keys.** `Platform::present` (`platform.rs`), the
   frame's samples to `Platform::audio`, then `Platform::keys` latched
   into `hw.keys` for the next frame. Same `Platform` trait as today, so
   every front end (desktop, headless, the PNG dumper) works unchanged.
4. **Lines 161-227.** Their H-blank and V-count handlers, if enabled;
   then return. Timers' overflows due in the frame are spread over the
   lines by their rate (280,896 cycles a frame), each running its handler
   and any FIFO refill it drives.

Whenever the beam crosses line 160, in whichever wait or idle it is,
steps 2 and 3 run: a frame is presented and the keys polled, so a game
spinning in a key-wait loop (an `idle` back edge) sees new input every
frame. `wait_irq(mask)` and `idle()` stop after the first event that
matches (any event, for `idle`), so a game that waits for V-count line 80 runs 80
lines and gets control back mid-frame, with lines 81 on drawn after
whatever it changes. The beam position and the rendered-so-far frame are
library state.

### Handlers

The game installs one handler at `0x0300_7FFC` (`IRQ_HANDLER_PTR`,
`sched.rs:53`) and dispatches from it: pret's `IntrMain` (hand-written ARM,
copied into IWRAM, reading `REG_IF`, acknowledging, switching to system
mode and calling `gIntrTable[n]`), agb's handler, or a game's own.

- **Lifted as code, recognised as a pattern.** The lifter recognises
  the store of a function to `IRQ_HANDLER_PTR` and prints
  `g.hw.irq.set_handler(IntrMain)`; `hw` calls it with `IF` set for each
  event. Its `REG_IF` read and acknowledge go through the library like
  any register. The mode switches and the IRQ stack in it (`msr cpsr`,
  the banked `sp`) are CPU state that no longer exists: the lifter
  recognises the standard prologue and epilogue (save SPSR, switch to
  system mode, re-enable IRQs, call, restore) and drops them, leaving
  the call. Unrecognised mode switches are holes.
- **No nesting.** Handlers run to completion at the points above (and
  when a library copy's cost crosses an event, section 7), never
  inside another handler or inside game code. A real GBA can take the
  V-blank IRQ while the main loop is still mid-frame (a lag frame) or
  nest a V-count IRQ inside an H-blank one; the native program can't.
  That is the one deliberate behavioural difference, and why the oracle
  needs a play build that times the same way (section 7).
- **Prettier on top:** where the master handler is pret's or agb's
  stock dispatch over a table, the lifter can drop it and print one
  `set_handler(Irq::VBlank, VBlankIntr)` per table entry the game
  stores. Same calls, same order (the stock handlers take the
  lowest-numbered pending source first).

## 4. Function pointers

The games use code addresses as values (the ROM-addresses research,
summary): Emerald compares `gMain.callback2 == CB2_Overworld` and finds
tasks by their function; it splits a function into halves in task data
(`SetTaskFuncWithFollowupFunc`: `data[14] = (u32)func >> 16; data[15] =
(u32)func`, joined back with `(data[14] << 16) | data[15]`); Kirby picks
behaviour by comparing state-function pointers. Rust `fn` values can be
called but not reliably compared (identical functions may be merged, and
one function may have several addresses across codegen units), and they
can't be split into halves.

**So a function pointer is an ID: `FnRef`**, a `#[repr(transparent)]`
`u32` newtype (the pointer's size, so layouts hold), indexing one
generated table:

```rust
pub static FNS: [Option<fn(&mut Game, u32, u32, u32, u32) -> u32>; N] = [None, Some(f_cb2_overworld), …];
impl FnRef { pub fn call(self, g: &mut Game, args…) -> u32 { (FNS[self.0 as usize].unwrap())(g, args…) } }
```

- **Typed where the analysis can type them.** A field or variable whose
  every stored value is a function of one signature (`TaskFunc` is
  `void (*)(u8)` in DWARF) gets `FnRef<fn(&mut Game, u8)>` and a typed
  table per signature, so `(g.gTasks[i].func).call(g, i)` reads like
  pret's `gTasks[i].func(i)`. Constants are named: `gMain.callback2 =
  fns::CB2_Overworld`, compared with `==`. Signatures come from DWARF
  prototypes or the summaries (section 5's "registers").
- **Untyped where they meet arithmetic.** The ID is a small integer, so
  the halves still work: `>> 16` gives 0, `& 0xffff` the ID, and the join
  gives the ID back. The lifter checks that the only arithmetic done on a
  value that holds an `FnRef` is that split and join (and `| 1`, the Thumb
  bit, folded at lift into the ID), via the value sets of everything
  flowing from a function constant; anything else (`<`, adding an offset,
  hashing) is a hole.
- **Tables of functions in ROM** (a battle script's command table,
  `sTurnActionsFuncsTable`, a state machine's handlers) are `static
  TABLE: [fn(&mut Game); N]` and called by index, the prettiest form,
  where the table is read only by index; one that is also read as words
  (copied into RAM, compared) is a `[FnRef; N]`.
- **An indirect call resolves statically** to "one of the functions in
  `FNS`": every address-taken function is an entry, so a call through a
  pointer is never a hole by itself. It is a hole when the value may be
  something that isn't a function entry: an address mid-function, in RAM,
  or computed (section 6).
- **IDs are a build parameter** (section 7): dense from 1 in a play
  build, the function's ROM address in the check build. The code is the
  same either way, since it only compares, stores, splits and calls them.

## 5. ROM addresses as analysis-time facts only

Every place an address shows in today's generated code, and what the
native port prints instead:

| Today (`codegen::emit`) | Native |
|---|---|
| block keys, `pc = K`, `match pc` | structured control flow (`codegen::plan`); labels for irreducible regions are local enums |
| `call!(m, pc, f -> ret, …)`, the return site in lr | a Rust call; the return is the call returning |
| `tick!(m, pc = …, cycles)` | nothing (section 3) |
| `m.get(at, p)`, the instruction's address for the stall | `g.x.y` / `g.at(r)` (no timing) |
| `ldr r0, =gTasks` (a pool word that is a RAM address) | `&mut g.gTasks`, or `Ref::of_gTasks()` |
| a pool word that is a function | `fns::Name` (an `FnRef`) |
| a pool word that is ROM data | `&ROM_DATA_NAME` (section 8) |
| `mov pc` through a jump table | `match` on the index, one arm per case (`analysis::jumptable`), `_` the default edge |
| `swi 0x05` | `hw::wait_vblank(g)` |

**Return addresses** vanish with calls: a call is a call. The guest's
push and pop of lr were stack traffic, and the stack is gone (section 1).
What remains is code that uses its return address as data, which the
lifter must turn into control flow at lift time:

- **WarioWare's helpers** (`code-without-rom-addresses.md`, "Do the games
  read their return addresses?"): a `bleq 0x3000468` whose helper does
  `sub lr, lr, #24` and returns there, 24 bytes back into the caller's
  loop. The value analysis sees lr enter the helper as a constant (the
  call site's return, `0x3000448`), so the `bx lr` target is a constant
  too (`0x3000430`): the helper "returns" to one fixed block. The lifter
  resolves it the way `vsa/func.rs:622` already resolves a leaf callee
  that returns somewhere computed: the `bl` is a jump into the helper's
  blocks, inlined into the caller's CFG, and the `bx lr` a jump to that
  fixed block. Structuring then sees an ordinary loop. Where the helper has
  several call sites, each gets its own inlined copy (a few
  instructions); where the computed return isn't one constant per site,
  it's a hole.
- **`__gnu_thumb1_case_uqi`/`_sqi`/`_uhi`/`_shi`/`_si`** (libgcc's switch
  helpers, which read a table at their return address and return past it;
  moonbeam): already an indirect jump at the `bl` with a resolved target
  set; the native port prints a `match`, and the helper itself isn't
  emitted.
- **Far-jump `bl`s** (decision 5a; `Recovery::far_jumps`): a `bl` used as
  a long branch prints as a jump within the function or a tail call, as
  the analysis classified it.
- **Calls that never return** (the per-function `FuncResult::no_returns`, `vsa/func.rs:130`): the call is the
  last statement of its path, `-> !` where the callee provably never
  returns.

**Registers** go the same way. Inside a function, guest registers and
NZCV are SSA values (the IR already is: `Sem<V = ValueId>`, decision 3),
printed as `let`s named from DWARF where the readable form's liveness rule
agrees (`codegen::dwarf`) and folded into expressions where used once.
Flags become the comparison they encode: `cmp r0, #5; bgt` is `if r0 as
i32 > 5`. Between functions, registers become parameters and return
values, and that needs what the play build never had to trust:
**per-function summaries** (decision 10, re-scoped; `registers-and-types.md`,
"Van Emmerik's preservation analysis"): which of r0-r3 a function reads
on entry (its parameters), which registers it defines on every return path
that a caller reads (its results: r0, sometimes r1, rarely others), and
proof that r4-r11 and sp come back unchanged. DWARF prototypes give the
answer for decomp functions and the summary checks it; elsewhere the
summary is the answer. A function whose callee-saves can't be proven, or
whose callers read a register it doesn't define, is a hole. Hand-written
assembly that keeps values in registers across calls in its own convention
(a mixer's inner loop calling a helper) becomes one function where the
helper is only called from it (inline), or a function returning a tuple.

**Measured** (`native/summaries`, 2026-11-01; `codegen::saves::summarize`,
`gbaa-recompile --summaries`). The summary is the callee-save proof's
abstract run with one more fact per r0–r3 and r12, where its value came
from: entry, the function's own write, a callee's return (by call site),
or a leftover nothing defines (a BIOS call's, an indirect call's). A read
of a maybe-entry value is a parameter; a call reads its callee's
parameters. Least fixed point over the call graph (every function starts
reading nothing and never returning; recursion by premises). An indirect
call is taken to read r0–r3 and define only r0; a BIOS call reads and
defines what GBATEK says. What the proof fails on is a hole in the
summary and the run goes on. *Agree* means the summary's parameters are
among the prototype's, what callers read after it is among what it
returns, and it defines what the prototype returns on every return path.
On Emerald's 15,158 functions (8,894 keep r4–r11 and sp):

| | count |
|---|---|
| agree | 13,359 |
| disagree | 1,664 |
| no prototype | 133 |
| prototype unreadable (a struct by value the map doesn't give) | 2 |
| disagree, trusting indirect calls to read nothing | 694 (14,329 agree) |

By category (a function can be in more than one), every one traced to a
root by hand, 10 per category (or all, where fewer):

| category | count | what it is |
|---|---|---|
| passes a register past its parameters to a callee | 505 | 504 are callers of `GetMonData`/`GetBoxMonData` with two arguments: pret's `GetBoxMonData2` is an alias of the three-parameter `GetBoxMonData3`, which reads r2 (`data`) for some fields, so a two-argument caller passes whatever r2 holds, and that is its own entry r2 where nothing wrote it. The binary really reads it; the prototype is the alias's. 1 is `TryWriteSector` through `ProgramFlashSectorAndVerify` (no prototype), which reads r2 after a loop the run doesn't know runs at least once (path-insensitive). |
| an indirect call reads r0–r3 (here, or through callees) | 656 | The sound assumption above: `CallCallbacks` calls `gMain.callback1()` with r1–r3 unset, so they become parameters. All 656 go away trusting indirect calls to read nothing, which is what C does but nothing proves. |
| a caller passes on (or returns unread) a register the prototype doesn't return | 619 | Derived from the two above: a caller passes a register a callee left (r2 after `Alloc`) to a callee whose summary reads it. 186 through the alias alone, 397 through indirect calls alone, 36 through both. |
| reads a register past its parameters itself | 1 | `ply_xwait` (m4a.c): agbcc builds a word from bytes by masking them into an uninitialised local, `ands r2, r0` on entry r2. The binary reads garbage; so does the C. |
| returns a value but never returns | 1 | `TryGetRandomWildMonIndexByType`: a variable-length array moves sp by a run-time amount, so the return address's slot is lost and its `bx r1` is a jump to an unknown place (a hole in the proof too). |
| a caller reads a register it doesn't return; a return leaves its result undefined; a BIOS call reads past the parameters | 0 each | |

So the summaries agree with every prototype except where the binary
really does something the prototype doesn't say (the alias, the
uninitialised read), where the analysis is path-insensitive (one loop) or
has a hole (one VLA), and where indirect calls are assumed to read
everything. For the stage-A lifter: use the prototype where there is one
and the summary elsewhere; `GetMonData2`/`GetBoxMonData2`'s callers need
r2 passed (a third argument, 0 or the leftover) to stay exact; an
indirect call's arguments need a decision (all four registers, or the
callee set's union of summaries where `analysis::jumptable` or the
function-pointer tables name them). The API: `Summaries::get(entry)`
gives `params`, `results` (defined on every return and read by a direct
caller, or returned unread by callers whose callers read it, `return
f(x);`; a function only called indirectly has none: 3,083 functions have
one), `defines()`, `keeps` and `holes` (a call to a callee whose run lost
its returns to a hole is one, and takes it to leave r0–r3 unknown);
`Summaries::root` follows a pass-on chain.

**With callee sets** (`native/indirect-call-args`, 2026-11-06):
`gbaa-recompile --summaries --callee-sets` takes an indirect call whose
callee set the function-address flow proves complete
(`codegen::native::fnref`) as calling exactly those
(`codegen::saves::summarize_sets`). Not measured on Emerald yet: of its
553 indirect calls, none has a set, because the flow didn't finish on
its 15,158 functions in 35 minutes (past 8 GB and growing). With
`native/fnref-flow-scale` landed (Metroid settles in 151 s) it still
didn't: killed at a 14 GB cap after 5 minutes (2026-11-07). Since
`native/fnref-emerald-speed` (2026-11-13) the run finishes, in 11 to 15
minutes at 2.4 GB, but the flow stops unsettled at its token bound (48M
held after 27,776 runs), so still no set is complete. Making it
settle on Emerald is `native/fnref-flow-emerald`; the re-measurement
of the 656, and what keeps sets incomplete (callbacks stored through RAM
globals such as `gMain.callback1`), is `native/indirect-call-args-emerald`.

## 6. Holes

There is no interpreter to run what the lift doesn't cover, so every gap
is found at build time. Each kind, how it is found and what can stand in:

| Hole | Found by | What can stand in |
|---|---|---|
| **Code not discovered**: run in the play build, not in `Discovery::code` | the coverage runs (`gbaa-fuzz`'s events, routes' compiled-share counts: the interpreter fallback ran it) | more discovery; a root added to the map |
| **An indirect jump or call not resolved** (`JumpRes::Unresolved` or `Rejected`, a call through a value that may not be a function entry) | `analysis::jumptable`'s `Recovery`; points-to on call targets | a hand-given target list in `holes.toml`, checked by the coverage runs |
| **Return-address arithmetic not resolved to one block per site** | the scrambled-return-address tool (`crates/interp/examples/lr_numbers.rs`) finds it dynamically; the value analysis statically | a hand-written replacement function |
| **Code that runs from RAM and changes** (Emerald Rogue's mixer stamping templates into its loop, `codegen::variants`; agb_flash's routine copied onto the stack; code built at run time) | `Discovery::copies`, `codegen::seen`'s copies seen running, the overlay guards' invalidations (`runtime/src/overlay.rs`) | a copy that never changes is just code (crt0's `.iwram`): no hole. Variants seen become one function each, and the store that patches the loop a field saying which runs; else a hand-written replacement |
| **Pointer arithmetic or casts the types don't explain** (an access through a `Ref<T>` at an offset that isn't a field of `T`, a cast between unrelated structs, a pointer into the middle of a global used as another type) | the typed lift: every access must resolve to a path | in stage A it isn't a hole (bytes work); in stage B a byte view of that object, listed |
| **CPU state the program doesn't have**: mode switches outside the recognised IRQ wrapper, `msr`/`mrs` used for anything else, stack switching (a game's own threads or coroutines), `setjmp`/`longjmp` | the lift: an instruction with no native meaning | a hand-written replacement (threads as Rust closures or a state machine) |
| **Hardware or BIOS outside the library**: an unimplemented BIOS call (decision 12's list), open-bus or BIOS-region reads, a register the library doesn't model | the lift | add it to the library |
| **Behaviour that depends on cycle timing** (a raster effect timed by instruction count, a timer read used as more than a seed) | not by the native oracle, which times the same way (section 7): by the play build at `--timing native` against `exact` on a route, where a difference is looked at by eye, as instant timing's were (Kirby's lag frames) | a cost the library charges (as for BIOS copies, section 7), or a hand change; most such games tolerate it already under instant timing |

**Two strengths of hole.** A *hard* hole means no program can be printed
for that function: the build fails and names it, unless `holes.toml` gives
a stand-in (a Rust file whose function replaces it, written by hand or by
an agent). A *soft* hole means the program is static and correct but less
pretty than the design wants: a region still bytes, a pointer still a
`Ref<u8>`, a ROM pointer read at run time (section 8), a function table
still `FnRef`s. Soft holes are counted, never fatal; they are the measure
of how far a port is from finished.

**Stages.** A: no hard holes; registers, CPU, stack and timing gone; RAM
as byte regions; I/O through `Hw`. Any game whose code is fully covered
can reach A without a decomp. B: state typed (from DWARF, or recovered),
pointers as indices or `Ref<T>`, ROM data typed. C: the pretty forms
(`&mut` parameters, `Box`, `fn` tables, inverted frame loop). Each stage
is checked against the same oracle.

`holes.toml` is per game, committed beside the game's other hand-made
files (like `names/<game>.toml`); a stand-in that translates the game's
code is the game's code, so it goes in `mods/<game>/`, which is
gitignored, never in `holes/`.

## 7. The oracle

**The native program draws the same frames as the play build from the
same inputs, and holds the same RAM.** The play build is itself checked
against mGBA (its exact checks), so a native port inherits mGBA as its
oracle through it.

### What "the same" can mean

Comparing by display frame fails as soon as the play build lags (its
main loop misses a V-blank and the game counts one frame where the
display shows two) or reads a timer, since the native program counts no
cycles. Two ways out:

1. **A play-build timing that times the way the native port does
   (recommended):** `--timing native` for `gbaa-recompile` and the
   interpreter. Blocks cost nothing; the clock moves only at halts (the
   BIOS waits), at the idle back edges of section 3 (the same list of
   loops, passed to both), and at reads of `VCOUNT`, `DISPSTAT` and the
   timer counters, by the same steps as the library's. IRQs are taken
   only at those points, so they never land mid-frame-work. The play
   build then behaves as the native program must, and comparisons are
   exact: every frame, and RAM at every frame end. It is checked like the
   other timings (compiled against interpreted, `just first-frame`-style),
   and it is the interpreter's timing model with one table added, not a
   new machine. It is a timing a check passes that isn't `exact`, which
   the timing decisions ("Play builds use --timing instant; checks pass
   --timing exact") don't foresee, so it is june's call: asked as "May the
   native port's check use a new timing, not exact?"
   (`q/may-native-port-s-check`), which units 3 and 7 below wait on.
2. **The play build at `--timing exact`, compared by game frame:** the
   Nth picture after the Nth return from a wait, and RAM compared only at
   frames where neither side lagged; timer reads replayed from the play
   build's record. Needs no new timing, but loses at the first lag frame
   or mid-frame interrupt, and Emerald's intro already depends on one
   (decision 12: its VRAM fill spans a V-blank).

**Work that must take time.** Under the native rule a BIOS copy or fill
takes no time either, so Emerald's 96 KB VRAM fill (decision 12) would
no longer span a V-blank, in the native port and under `--timing native`
alike: the oracle agrees with a program that may differ from the game.
So the library's BIOS copies, fills and decompressions (and DMA, section
8) advance the beam by their cost as mGBA's HLE counts it (`bios::call`'s
pricing, the same table in both builds), running any V-blank they cross
as a wait would. They are the only code that takes time; game code
still takes none. Anything else whose behaviour needs time to pass is the
cycle-timing hole of section 6.

The rest of this section holds for either.

### The check build

**The native program's state is byte-comparable with the play build's
because two things are build parameters, not code:**

- **`--layout guest`**: `Game`'s RAM fields are placed at their guest
  offsets (two `#[repr(C)]` region structs, EWRAM's and IWRAM's, with each
  global at its link address and padding between), and `Ref<T>` holds the
  guest address. The play layout (`--layout packed`) orders fields for
  Rust and `Ref<T>` holds an offset into `Game`.
- **`--fn-ids rom`**: each `FnRef` is the function's ROM address
  (Thumb bit included). The play build's IDs are dense.

The code doesn't change between them (it compares, stores and resolves
handles; it never interprets their values), so a check build's RAM
regions are the play build's EWRAM and IWRAM at every frame end, except
the stack (no native stack) and the IRQ stack. A native check runs both
with the same key presses, frame by frame:

- frames equal (frame hashes, PNGs of the first that differs, as
  `route-check` does today, `crates/route-check/src/main.rs`);
- EWRAM and IWRAM equal outside the guest stack, below the lowest `sp`
  the play build's run reached (it reports it), and outside the
  destinations of code copies (`Discovery::copies`, and the copies seen
  running that the lifter's holes file lists as `[[copy]]`, which
  `route-check --native` reads from it): the native program runs that code as Rust functions and
  doesn't copy it, so those bytes are the one other range it leaves
  empty. A copy that also carries data (agb's `.iwram` holds variables
  too) has its data's globals in `Ram` as usual, initialised from the
  copy's source, and they are compared;
- the I/O registers, VRAM, palette and OAM equal (the library's
  `Video` against the play build's `Bus`, which owns a `Video` after
  section 2's split, so it is a struct compare);
- audio samples equal per frame.

A second check proves the parameters don't matter: the check build and
the play-layout build draw the same frames over the same route. Routes
(`units/*.toml`) and their key presses are reused as they are; the native
binary takes the same `--frames` and `--press` arguments.

For a function at a time, before a whole game builds, fn-diff
(`codegen::fn_diff`, the readable form's check) extends to native
functions: from a recorded entry state, call the native function with
its parameters built from the registers its summary names, and compare
the RAM, I/O and return value with the play build's run of the same
function. That is how `native/emerald-ten` checks behaviour.

### The check as built (`native/oracle`, 2026-11-01)

`route-check --native NATIVE PLAY ROUTE` (`just native-check`,
`crates/route-check/src/native.rs`) on the spike's program and on
`agb/psg-square` lifted by hand (`crates/native-spike/src/psg_square.rs`),
both against their `--timing native` play builds. What building it
showed:

- **Records, not dumps.** Each build writes a record of every frame at
  its present (`--native-record`, `crates/native-record`): picture hash,
  EWRAM, IWRAM, the I/O block, palette, VRAM, OAM, audio, each region
  only when it changed. The library's side is `Hw::on_frame_end` (after
  V-blank DMA, before the present) and `Hw::raw16`. The check build's
  layout is a cargo feature (`guest-layout`): `Ram` a zerocopy
  `#[repr(C)]` struct of EWRAM then IWRAM with each global at its link
  address; the code is the same in both layouts. The spike has no
  `FnRef`s, so `--fn-ids rom` wasn't needed yet.
- **The lowest sp comes from the interpreter.** Compiled code doesn't
  keep it: a block's balanced push and pop never write the low value to
  r13, so noting r13's writes saw `0x03007e9c` where the stack reached
  `0x03007e94`. The check runs the play build a second time with
  `--interp` (`Machine::low_sp`, noted after each instruction), and its
  records must equal the compiled run's.
- **A replaced library leaves state the native program doesn't have.**
  agb's allocator, interrupt list, `NUM_VBLANKS` and heap in EWRAM, and
  its tile allocator's free list, which it keeps inside free VRAM.
  The native programs replaced agb (research rule 1), so those bytes
  are skipped by name (`crates/native-spike/check/*.replaced.toml`,
  printed on every run); the program's own global (`FRAMES`) is
  compared. A lift of the library itself (stage A without recognition)
  would hold them and need no list. The recogniser should emit the list.
- **Not compared:** `KEYINPUT` (the play build records after the
  present's key latch), and the DMA registers (the typed copies store
  none; the serial port's are compared). At first the sound wasn't
  either: the library had no mixer, so psg-square's samples were reported "NOT compared". Since
  native/hw-sound (below) every frame's samples are, with `SOUNDCNT_X`'s
  status, the FIFOs' last stores and the timers. Play's sound against a
  host that records none still fails unless the run gives `--no-audio
  WHY`.
- **Gaps the next programs will hit:** a code copy is skipped whole, so
  an agb `.iwram` static copied with the code isn't compared (the skip's
  line says so); a lowest sp outside IWRAM (r13 as a scratch register, a
  stack in EWRAM) fails the check rather than skip other RAM.
- **Library fixes it forced:** `IF` is requested as V-blank's line
  starts, before the present, and handled after (the record saw `IF` 0
  where the play build had 1); `hw::spend` moves the beam by cycles
  through its events (a sub-line `dot`), and `hw::bios::cpu_set_cycles`
  is the play build's own `CpuSet` price, so agb's crt0 copies take the
  play build's three lines before `main` (psg-square's first frame shows
  them: three black lines, then forced-blank white).

### Sound in the library as built (`native/hw-sound`, 2026-11-06)

The library mixes with the play build's own sound path, not a copy:

- **One path, two hosts.** Every function of `runtime::sound` (the timers'
  overflow walk, the FIFOs' pops and refills, the PSG, the mixer, a
  store's effects: `sound::store`) runs on a `sound::Audio`: the I/O
  block's sound and timer registers with the clock and `IF` (an `Io`),
  the `Sound` state, and whatever refills a FIFO. The play build's `Bus`
  is one (DMA1/2 refill, as before; its behaviour is unchanged, the
  functions only generic). `hw::Sound` is the other: an `Io` used for
  the sound registers (`0x60`-`0xA7`) and timers (`0x100`-`0x10F`) only,
  stored and read as the play build stores and reads them, the mixer,
  and `hw::FifoDma` for DMA1/2 aimed at a FIFO (`Hw::fifo_dma`), whose
  `read: fn(&mut G, u32) -> u32` takes words from the program's memory
  (ROM data, or the buffers a mixer fills). Timer and FIFO-DMA IRQs move
  from that `IF` into `Irq::flags`. The chunks go to `Host::audio`, and
  the native host records them as the play build's capture does.
- **The library keeps the play build's clock.** Samples are exact to the
  cycle, so the library's time is `Hw::now`, cycles since power-on, and
  its waits run the play build's `Machine::sync` step for step: events
  are the beam's half lines and the overflows of a timer something
  listens to; each sync brings the sound up to the clock before each beam
  event and after, hands the sound off when a buffer is half full, finds
  the next event, then runs the handlers. A V-blank DMA callback returns
  its price (`hw::dma_cycles`, the play build's), added after the
  channels run, which moves where the frame's samples end; a FIFO refill
  adds 10 cycles, as the play build's does. The waits start with a sync
  where they are (the play build's halt store's), so a timer just set up
  is an event. What changed for programs without sound: `halt` and
  `wait_irq` return at the event whose handler ran (an H-blank's, not the
  next line's start), and a `spend` across several events runs one
  handler pass after them, as the play build takes one IRQ after its sync.
  The lifter's check passed unchanged on all its programs.
- **A store never refills.** A store (`Hw::write16`) has no `&mut G`, so it
  can't read the program's memory; it doesn't need to, since every
  overflow up to the clock was walked when the clock last moved (in a
  sync, which can). One that found a FIFO due would panic.
- **The checks:** psg-square now plays FIFO A from `WAVE` (and on into
  `SCORE`, which follows it in ROM) through DMA1 on timer 0, and its
  V-blank copy writes words at the copy's price; `agb/direct-sound` (new)
  drives both FIFOs as a software mixer does, IWRAM double buffers
  swapped by a cascaded timer's IRQ, whose handler re-latches DMA1/2 and
  fills the buffer just played (its sound matches mGBA's for 299 frames).
  Both pass `route-check --native` with audio equal every frame, and a
  planted wrong sample in psg-square is caught. The card asked for "a
  native bios-sound"; agb/bios-sound only calls the BIOS sound driver's
  no-op stubs and plays nothing, so direct-sound is the direct-sound case.
- **Not yet:** the price of `Dma::hblank_to`'s H-blank DMA on the clock
  (no program needs it with sound yet).
- **DMA by its registers** (native/hw-dma-registers, 2026-11-12):
  `runtime::hw::dma_regs`. A lifted program that sets a channel up by
  address (m4a's `SoundInit`, agbcc games' copy and fill macros) gets the
  play build's channels: the registers stored in the sound's I/O block
  through its masks, latched on enable as `dma::control_written`, an
  immediate one run in the store through the program's memory
  (`Native::dma_load`/`dma_store`; the prelude's `io_write16` calls
  `hw::write_dma`) and priced as the play build's `io_write` (the clock
  moves, no sync), V-blank and H-blank ones repeated at the beam's events
  and priced there, a special DMA1/2 aimed at a FIFO becoming its
  `FifoDma`, the IRQ at the end. Checked by `agb/native-dma` in the lift
  check, each start set up by address, with a planted wrong `DMAxCNT`.
  Not yet: the EEPROM's DMA.
- **The serial port's registers** (native/hw-serial-registers): `0x120`-
  `0x15F` but `KEYINPUT`, kept in the sound's I/O block through the play
  build's masks and compared by the native check. Nothing answers: a
  started transfer keeps `SIOCNT`'s start bit and requests no IRQ, as in
  the play build (mGBA's unplugged port answers:
  native/serial-no-partner-as-mgba). Checked by `agb/native-serial`.

Once a mod edits the program, none of this applies: a mod is meant to
differ. The oracle is for the unmodded native port, and a mod is checked
the way mods are today (`route-check --mod-run`, frames side by side).

## 8. Data pointers, DMA and struct copies

### ROM data

The program carries the ROM's data (decision 2: a build carries its ROM
today, for data and for the interpreter; the native port needs it only
for data). Two forms:

- **Typed statics, where types are known.** A ROM object with a DWARF type
  (`gSpeciesInfo`, `gBattleMoves`, a sprite template table) is printed as
  a `static` initialised from the ROM's bytes at build time, with pointer
  fields as `&'static` references to the other statics they point at
  (Rust statics can refer to each other): `static gMonIconTable: [&[u8];
  N] = [&ICON_BULBASAUR, …]`. Strings are `&'static [u8]` in the game's
  encoding (the mod view's text tables print them). Being Rust statics,
  they stay out of `Game`: ROM is immutable and needs no handle.
- **Bytes, where they aren't.** The ROM's data stays one
  `include_bytes!` (as the play build's does, `crates/desktop/src/lib.rs`)
  and a pointer into it is a `RomRef<T>`, an offset with typed views. A
  pointer *loaded from* those bytes is a guest address at run time,
  turned into a `RomRef` by subtracting the ROM base: a soft hole
  (section 6), the one place a ROM address is a run-time value, counted
  by the hole report.
- **Pointers from RAM to ROM** (a script pointer stepping through
  bytecode, a palette pointer kept in a task) are `RomRef<T>` too, or
  `&'static [u8]` slices advanced by re-slicing where the field's points-to
  set is one ROM object. Under `--layout guest` a `RomRef` holds the guest
  address, so RAM still compares.

Graphics and music that the asset group extracts (`assets`,
`docs/asset-extraction-design.md`) can replace their bytes with files
later; the program sees the same `&'static [u8]`.

### DMA

A DMA is a library call whose form depends on where it copies, decoded
from the constant control word the lifter sees at the `DMAxCNT` store (or
the `DmaCopy16`-style helper it's written through):

| DMA | Native | Needs |
|---|---|---|
| immediate, RAM or ROM to RAM | a copy between typed objects, `g.a = g.b` or `copy_from_slice` | source and destination resolved to objects or slices of the right size; else bytes |
| immediate, fill (fixed source) | `.fill(v)` / `= Default::default()` | the destination resolved |
| immediate, to VRAM, palette or OAM | `g.hw.ppu.vram.copy_from(off, src)`, `palette.copy_from`, `oam.copy_from` | the destination's offset, the source as bytes |
| H-blank, repeating, to a register (a wave or window effect: a table of 160 values for a scroll or window register) | `g.hw.dma[n].hblank_to(Reg::BG1HOFS, src: Ref<[u16; 160]>)`; the library reads one entry per line while drawing (section 3) | the source a handle the library can read at each line, so it borrows `Game` then: the library reads it through the `Ref` during `wait_vblank` |
| V-blank, repeating (OAM from a shadow buffer every frame) | `g.hw.dma[n].vblank(...)`, run at step 2 of the wait | the same |
| sound FIFO (timer-triggered, repeat) | `g.hw.sound.fifo_a.stream(Ref<[i8; N]>)`; the library pulls samples from the game's buffer as the timer runs | the buffer's handle; the mixer that fills it is game code (m4a's, in the V-blank handler) |
| a control word or address computed at run time | the library's generic `g.hw.dma[n].start(src, dst, cnt)` taking `Ref`s | where the addresses are RAM or VRAM handles, not a hole; where a source or destination is a computed number, a hard hole |

The repeating kinds are why handles matter: the library reads game memory
later, at a line or a timer tick, so it holds a `Ref` and resolves it
against `Game` when it runs, inside `wait_vblank`, which has `&mut Game`.

### Struct copies

agbcc copies a struct with `ldmia`/`stmia` pairs, a `memcpy` call or
`CpuSet`; clears with `memset` or `CpuFastSet`. The lifter, once both
sides are typed:

- a copy whose extent is exactly one object of the same type on both
  sides is `*dst = *src` (or `g.a = g.b.clone()`, every type being
  plain-old-data: `Copy`);
- a fill whose extent is one object is `= Zeroable::zeroed()`;
- a copy or fill over part of an object, several adjacent globals, or
  between different types is a byte copy through `as_bytes()`
  (`zerocopy`), which is exact and stays a soft hole until someone names
  what it means (often it is a sub-struct DWARF does know).

`ldm`/`stm` that aren't copies (a register save, a multi-field load into
locals) disappear with the registers.

## Emerald's ten, scored (`native/emerald-ten`)

Done 2026-11-01 on quack: ten Emerald functions of different shapes, and
the callees they reach (22 functions in all), lifted to stage B by hand:
plain Rust over a typed `Ram` (DWARF types, the check build's guest
layout), `Hw` for the hardware, no registers or addresses. The code and
the side by sides are in `mods/emerald/native-ten/`, which is private:
it's Emerald translated, and `mods/` is gitignored. Here
are only the scores and what each gap asks of the lifter.

**Behaviour:** `just native-ten` (`crates/native-fn-diff`, a native form
of fn-diff). It records entry states in the interpreter, 600 frames with
no input; the seven functions that run never enters borrow other
functions' states. Each state runs 96 variants (parameters and the
globals read, drawn per function, after every global the ten name gets
random bytes, so a store to the wrong global shows; timer 1 runs at a
random count). The reference is the interpreter
from that state to the return, untimed: code takes no time, as the
native rule says (decision `d/native-fn-diff-s-reference-is-the-interpreter`).
The native function gets the parameters its DWARF prototype names.
Compared: EWRAM and IWRAM except the guest stack below the entry `sp`;
the display registers, palette, VRAM and OAM; `DISPSTAT` (but its
V-count match flag), `IE`, `IF`, `IME` and the timers' registers; the
beam's line and the timer counters as read (time doesn't move); that no
other I/O register changed; and the result. It prints `native 0 of 10 differ` (3,072 runs). A changed
byte in Random's literal pool is caught, and so are one-byte edits to the
native code: 19 small edits tried by hand (a byte, or one stray
statement) across all ten, each caught once the inputs reached its path
(GetBgAttribute's metric path, SetGpuReg's V-blank window edges, timer 1
running, and stray stores of zero needed inputs aimed at them).

| function | shape | pret lines | native lines | statements | left raw |
|---|---|---|---|---|---|
| GetGeneratedTrainerIdLower | leaf getter | 1 | 1 | same | none |
| ClearBattleMonForms | array loop | 3 | 1 | `for` + store → `fill(0)` | none |
| SeedRngAndSetTrainerId (+1) | call chain, timer | 4 + 2 | 4 + 2 | same | the timer is a stand-in (`Hw` has none) |
| GetPlayerFacingDirection | field through an index, bitfield | 1 | 1 | same | none |
| Random | u32 arithmetic | 3 | 3 | same | the macro's two constants |
| GetBagItemQuantity | pointer parameter, u16 | 1 | 1 | same, `u16 *` → `&u16` | none |
| EnableInterrupts (+5) | registers through a buffer | 4 + 39 | 4 + 42 | same order; one `for` → `while` | two `write16`s, one bounds guard |
| ResetTasks | struct-array loop, function pointer | 10 | 9 | same, `memset` → `= [0; 16]` | three macro constants |
| CalcCRC16 | nested loops over bytes | 10 | 6 | same loops, pointer + length → slice | none |
| GetBgAttribute (+6) | switch calling switches | 29 + 99 | 16 + 47 | `switch` → `match`, nested switches → tuple matches | one fall-out-of-switch as an `Option` |
| **all** | | **66 + 140** | **46 + 91** | | |

Against the decomp spike's exact readable form of the same eight
(173 lines, its play view 842), the native eight take 34. Every global,
field, parameter and enumerator has pret's name; what's lost is pret's
macros, which DWARF doesn't carry.

**What the lifter needs, gap by gap:**

1. **Fills.** A countdown pointer storing a constant over exactly one
   array (agbcc's form of a counted `for`) and a `memset` of one whole
   field both print as a fill. Needs the induction pointer's value set to
   match one object's extent.
2. **Timers in `Hw`**, with the stand-in's shape (`timer[n].count()`,
   `.control`, `.reload`). And the reads rule: a `VCOUNT`, `DISPSTAT` or
   timer read outside a poll loop moves no time (decision
   `d/a-vcount-dispstat-or-timer-read-outside-a-poll`). The table in
   section 3 had every read advance, but SetGpuReg reads `VCOUNT` on
   every call, so that would run frames of beam per game frame.
3. **Typed register stores must mask like the hardware.** `Irq::enable`
   is a plain field, so `hw.irq.enable = v` kept `IE` bits 14-15, which
   the play build drops. The check caught it on its first run; the store
   is `hw.write16(IE, …)` until `Hw` masks (a setter, or masking where it
   is read and compared).
4. **Bitfields** as their storage unit plus one accessor per field, from
   DWARF's bit offsets. agbcc reads a narrower unit than DWARF says
   (`facingDirection`'s `u16` overlaps `range`; the struct holds the bytes
   the code loads).
5. **Retyping from access widths:** a `u8[0x60]` read and written only as
   halfwords (`sGpuRegBuffer`) is `[u16; 0x30]`.
6. **Pointer parameters:** read-only and not escaping → `&T`, with `g` as
   `&Game` when the function only reads; a pointer with a length
   parameter bounding the loop over it → a slice.
7. **Index one past the end.** SetGpuReg's full waiting list stores onto
   the next global, which the next statement clears. The lifter must see
   the index's value set reach the length and either prove the store dead
   (here) or make it a hole. Likewise an index the game keeps in range
   but nothing proves (`sGpuBgConfigs2[bg]` for `BG_ATTR_BASETILE`) is a
   panic natively where the guest reads the neighbour: a soft hole to
   list.
8. **Switches:** jump tables to `match`; nested switches that only
   return, as one `match` on a tuple; a switch that falls out to a shared
   return, as an `Option` or a labelled block.
9. **Macros:** only names DWARF has survive (enumerators, globals,
   fields). `ISO_RANDOMIZE1`, `NUM_TASKS`, the sentinels, `FALSE`, the
   `DISPSTAT_*` and `INTR_FLAG_*` bits need a names file (hardware bits
   can come from the library's own constants) or stay literals.
10. **C's arithmetic:** implicit promotions and truncations become
    explicit `as` and `wrapping_*`, which the lifter gets from the
    instructions' widths.
11. **Summaries vs DWARF:** Random's and ResetTasks' summaries also
    return r2 (some caller reads r2 after them). The prototype was used, as
    decided, and the check compares r0 only, so it says nothing about
    those callers: what they read is the lifter's question at each call
    site, as with `GetMonData`'s two-argument callers (section 5).
12. **Function pointers** stored as constants are `FnRef`s
    (`fns::TaskDummy`, the ROM address under `--fn-ids rom`), as
    section 4 says.

## The stage-A lifter as built (`native/lift-stage-a`)

Built 2026-11-01 on quack: `codegen::native` (`crates/codegen/src/native.rs`,
its prelude `native/prelude.rs`), the `gbaa-lift` binary, `just lift` and
`just lift-check`. It prints a whole program from discovery, the IR and
the summaries as one `src/main.rs` (since `native/lift-modules`, a
`main.rs` and modules of about 4,000 lines; `docs/native-stage-b.md`,
"The first game"), with no `Machine`, CPU, cycles or
dispatcher, and it is checked against the `--timing native` play build
with the native check and no replaced-library list.

**What it prints.**

- Every function is `fn f_<key>(g: &mut Game, sp: u32, a: [u32; 4]) ->
  Result<[u32; 4], Exit>`. (Since `native/lift-params`, only those whose
  summary has a hole: the rest take their parameters and give their
  results, `docs/native-stage-b.md`, piece 3.) r0-r3 go in and come back; r4-r12, `lr` and
  NZCV are Rust locals; each block's IR values are `let`s. Because every
  function keeps r4-r11 in its own locals, a callee can't change its
  caller's, so the lift is exact for any callee that follows AAPCS,
  whatever the summary proof says. The summary is printed above each
  function (`/// Reads {r0, r1}; returns {r0}`), and a function whose
  summary breaks that (`analysis::holes::SUMMARY_HARD`: it returns sp
  changed, reads r4-r11 a callee changed, or changes them and reads one
  as it came, a register parameter) is a hard hole. One that only uses
  r4-r11 as scratch, where no caller reads them after, is lifted as is.
  The 47-50 functions per program whose proof fails for other reasons
  (an indirect call, a callee that never returns) are listed in the
  report as soft.
- Blocks are a `loop { match b }` over local labels, 0 the entry; ROM
  addresses survive only as the labels' comments, jump-table `match`
  arms and `FNS` keys. The structuring plan isn't used yet (stage C's
  prettiness); `Plan::Static` is a label or, to another function's entry,
  a tail call.
- A return is a jump to `RET`, the value `lr` holds on entry. Compiled
  code moves `lr` through the stack, so returns are told by value, not
  by shape: `pop {pc}`, `bx lr` and `ldr pc` all work, and anything else
  through a register is a resolved table's `match` or a tail call
  through `call_fn`.
- The stack stays where the game has it: `sp` is a parameter and the
  frames are IWRAM bytes. A frame recovered as Rust locals is stage B's
  (section 1). `Game::sp` is the stack pointer at the last wait, where a
  handler that switches to system mode puts its frames.
- Memory is `ld8/16/32` and `st8/16/32` over `Game::ram` (EWRAM then
  IWRAM: the check layout is the only layout a byte program has), the
  library's I/O (`Hw::read16`/`write16`; the I/O block's unused
  offsets read 0), its palette, VRAM and OAM (the play build's byte-store
  rules), and the ROM's bytes.
- Function pointers are their keys (`--fn-ids rom` only): the lift loads
  them from ROM bytes and RAM (agb's vtables and closures), so it can't
  renumber them. `call_fn` binary-searches `FNS`.
- BIOS: `Halt` is `hw::halt`; `IntrWait` and `VBlankIntrWait` are the play
  build's loop over `0x0300_7FF8` and `hw::halt`; `CpuSet` copies and
  spends `cpu_set_cycles`; `Div`. Idle edges get `hw::idle`, spin edges
  `hw::spin` (`ExitInfo::idle`, the play build's own marks). IRQs go
  through the word at `0x0300_7FFC`, as the BIOS's stub calls it.
  The prelude has every call `analysis::holes::NATIVE_BIOS` lists
  (native/lift-bios-calls, native/lift-bios-more). `SoftReset` clears
  IWRAM's top 0x200 bytes, sets `Game::reset` and returns `Err(Exit)`,
  which unwinds to `main`; `main` restarts the entry (or the EWRAM
  function, if the reset flag says so) on the same `Game`, as the
  hardware keeps EWRAM, VRAM and the registers, with sp, the CPSR and the
  tasks fresh. One in an IRQ handler panics: the handler's caller drops
  its `Err`. Code after a `SoftReset` or `HardReset` `swi` isn't the
  function's (`func::successors`), and sp set to a constant in a function
  that never returns (the SDK's reset) is no stack switch.
- **Holes.** `holes/<game>.toml` (a commercial game's in the private
  `mods/<game>/holes.toml`) gives stand-ins (`[[standin]]`, Rust
  for one function, with why), unresolved jumps' targets (`[[jump]]`)
  and code the game copies to RAM that only a run shows (`[[copy]]`, as
  `just holes` prints it; lifted at its RAM keys, native/lift-ram-copies).
  Every agb program needs four stand-ins: portable_atomic's
  `interrupt::disable` and `restore` (`mrs`/`msr cpsr_c`, a critical
  section: `Game::cpsr`), agb's `InterruptHandler` (the IRQ wrapper's
  mode switches; its IE, IF, IME and `0x0300_7FF8` effects kept), and
  a false function in agb's panic-screen rodata whose jump Recovery
  can't resolve.
- **Discovery's gaps are decoded at lift.** A jump into ROM where
  discovery has no code (it took a `bl`'s second half for a literal
  pool, or stopped a scanned fragment short) would fall to the
  interpreter in the play build; the lifter decodes from each such
  target until it meets known code, again until none are left, joins
  split `bl` halves, and lists the targets. The spike has 22,
  affine 5, mosaic-modes 143 (much of one large function).

**Measured** (`just lift-check`, 2 minutes on quack):

| program | functions (IWRAM) | blocks | stand-ins | gap targets | lines | native check, 600 frames |
|---|---|---|---|---|---|---|
| agb/native-spike | 111 (3) | 1,750 | 4 | 22 | 74,496 | pass, 0 differences |
| agb/affine | 110 (3) | 1,715 | 4 | 5 | 74,245 | pass, 0 differences |
| agb/mosaic-modes | 103 (3) | 1,706 | 4 | 143 | 74,415 | pass, 0 differences |

The check compares pictures, EWRAM and IWRAM (all but the stack from the
lowest sp and the code copies: agb's allocator, heap, interrupt list
and globals included), the I/O registers the library models, palette,
VRAM and OAM. A planted wrong store (`--plant 0x08000186`, the V-blank
handler's frame count) is caught at frame 2 (`EWRAM byte 0x020000c0:
play 0x01, native 0x02`); with the IRQ wrapper's stand-in dropped
(`--drop-standin 0x03000000`) the lift fails and the crate's build
stops at `error: hard hole in f_03000000 at 0x03000034: CPU state: mrs
(reads a PSR)`. Soft holes per program: about 3,050 loads and 1,800
stores at a computed address, 600-900 at a constant one, 61-65
indirect calls through `call_fn`, 8 jumps by value.

**What stage B should know.**

- The lift is exact because it is literal: each function is the
  machine's code over locals, so it ran its first time. Everything
  stage B changes (types, locals for stack slots, `&mut` parameters,
  fewer parameters from the summaries) is checked by the same command.
- agb dominates: of 111 functions, the program's own code is two
  (`main` and the V-blank closure). Without agb recognition the output is
  the allocator, critical sections, the interrupt list and `core`'s
  formatting and panics, 74,000 lines for a 330-line program. The four
  stand-ins are agb 0.25's and the same in every agb program but for
  their addresses: a recogniser by code bytes would write them.
- Stack frames are bytes, and agb's code passes addresses of stack
  slots everywhere (`&mut self` on stack values): recovering them as
  locals needs the escape analysis of section 1.
- Pointers are guest addresses, ROM data is read at run time, function
  pointers are ROM keys: three soft-hole families stage B turns into
  `Ref<T>`, typed statics and dense `FnRef`s.
- Two shortcuts that hold for these programs, not for every game: a
  fallthrough into another function's entry is printed as a tail call,
  which starts the continuation with r4-r11 and the flags zeroed (a call
  whose return site is another function's is taken never to return, and
  panics if it does); and `Game::cpsr`'s I bit doesn't hold an IRQ back
  at a wait, as the hardware would. (Both are fixed, `native-stage-b.md`,
  piece 1: an extended entry takes the caller's registers and flags, and
  the waits hold an IRQ while `Game::cpsr`'s I bit is set, until a
  critical section's end clears it.)
- Discovery's gaps are real: mosaic-modes' 143 targets (24 once the
  lifter stopped decoding past a `bl` into a literal pool,
  native/lift-fallthrough) are code the
  play build hands to its interpreter wherever it reaches them. Fixing discovery's `bl`-pair
  and scanned-fragment cases would make the lifter's decoding a no-op.

## The units that follow

All are filed in the group (`native`); the ones this design added are
marked new. In order:

1. **Spike: one of our agb programs as a native Rust program
   (`native/agb-spike`)**, filed. By hand, the lifter's output for a small
   program of ours. It tests sections 1-3 and 7 at the smallest scale and
   says what the printer must do. Its "same frames as its exact play
   build" works for agb/template (no lag); if it meets a timer or a lag
   frame, the timing question below decides. Its hardware library is a
   throwaway adapter (its own fields copied into today's `Bus` before
   each `render_line`), so that the real one is designed once, in unit 2,
   with what the spike learned. Done 2026-11-01 on a new `agb/native-spike`
   (`crates/native-spike`, `just native-spike`); what it found:
   `docs/research/native-agb-spike.md`.
2. **The hardware as a library, shared by both builds (new:
   `native/hw-library`).** Section 2's split: `Video` and `Audio` out of
   `Bus`, the PPU and mixer taking them, `Hw` with typed registers, the
   BIOS routines over slices, `wait_vblank`/`idle`/beam reads. The play
   build's checks (first-frame, routes' frame hashes, `same-states`) must
   be unchanged. Needed by the spike's second half and everything after.
   Built 2026-11-01 (`runtime::video`, `runtime::hw`); what it is:
   - `Video` holds the display registers as fields (`dispcnt: DispCnt`,
     `bg[n]: Bg { cnt: BgCnt, scroll_x, scroll_y }`, `affine[2]`,
     `win[2]`, blend and mosaic) and palette, VRAM and OAM as the bytes
     they are, with halfword and `OamEntry` accessors. The PPU draws from
     `&Video` only. The play build's `Bus` owns one for the three
     memories; its registers stay stored in `Io` (masks, save-state
     payload, every reader unchanged) and are decoded into the fields
     before each line (`Bus::video_for_draw`), so the snapshot layout
     didn't change and no version bump was needed.
   - `Hw<G: Native>`: `ppu: Video`, `stat` (DISPSTAT's enables and
     V-count line), `keys`, `irq` (a master handler, `set_handler`, or
     one per source, `on`), `dma` (H-blank DMA to a display register from
     a `fn(&G, i) -> u16`, V-blank DMA as a callback), and the host.
     Free functions over the whole game: `step`, `wait_vblank`,
     `wait_irq`, `idle`, `halt`, `vcount`, `dispstat`; every one returns
     `Result<_, Exit>`. The beam draws line by line; mid-frame changes
     from handlers show from the next line. `wait_vblank` returns at line
     160 after the V-blank handler (as `VBlankIntrWait` does: code after
     it reads `VCOUNT` 160), not after line 227 as step 4 of section 3
     says; lines 161-227 run at the start of the next wait. `hw.write16`/
     `read16` by offset cover the display registers, DISPSTAT, IE, IF and
     IME, and panic on anything else.
   - `hw::bios`: `div`, `sqrt`, `arctan2`, `bg_affine`, `obj_affine`
     (the play build's own arithmetic, refactored to values), `lz77` and
     `rl` over slices (separate decoders from the play build's, which
     count cycles through the bus; a test runs both on the same random
     streams).
   - Not yet: `Audio` (the mixer still reads `Io` and the bus), timers,
     serial, saves, Huffman and the other unpackers. `crates/native-spike`
     runs on `Hw` now.
3. **`--timing native` for the play build and the interpreter (new:
   `native/timing-native`)**, if june picks it: section 7's option 1, with
   the idle-loop list from analysis, checked compiled against interpreted.
4. **Function summaries: parameters, results and callee-saves (new:
   `native/summaries`).** Section 5's registers: Van Emmerik's analysis
   over the call graph, checked against DWARF prototypes on Emerald.
   Every lifted function needs it.
5. **Hole report (`native/hole-report`)**, filed: section 6's table as a
   tool, on the eight games. Its counts say which game goes native first.
6. **The stage-A lifter (new: `native/lift-stage-a`)**: a printer from
   IR, plan and summaries to plain Rust over byte RAM and `Hw`, for a
   whole program, with `holes.toml`; checked against the agb spike's hand
   version, then on that game end to end with the native check. Built
   2026-11-01: "The stage-A lifter as built" above.
7. **The native check (new: `native/oracle`)**: `--layout guest`,
   `--fn-ids rom`, and a `route-check` mode comparing a native binary's
   frames, RAM, I/O and audio with the play build's at every frame.
8. **Emerald: ten functions as native Rust (`native/emerald-ten`)**,
   filed: stage B by hand for ten functions, scored against pret, which
   tells the typed lifter (stage B) what to do. The typed lifter, the
   function-pointer IDs (section 4) and the ROM statics (section 8) are
   units to file from what the spike and these ten find, not before, so
   no two sessions design them twice. Done 2026-11-01: "Emerald's ten,
   scored" above.
9. **Stage B, filed 2026-11-01**: the typed lifter, function-pointer IDs
   and ROM statics, as seventeen units after the bars unit
   (`native/lift-bars`), plus three for the first game (Metroid: Zero Mission at
   stage A, june's to confirm). The plan, its order and each piece's
   check: [native-stage-b.md](native-stage-b.md).
