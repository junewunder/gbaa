# gbaa in depth

The [README](../README.md) says what gbaa is in a page. This is the detail
behind it: exactly what is checked and against what, how the recompiler
works, and where native ports stand. Numbers are as run on 2026-11-18.

Two names are used throughout. A game runs here in two ways:

- The **recompiled build** is the game's translated code compiled together
  with the runtime into one program.
- The **interpreter fallback** is an interpreter inside the runtime. It runs
  any code the translation missed, one instruction at a time. It can also run
  a whole game by itself.

The **compiled share** is the part of a game's CPU time spent in compiled
code; the rest runs in the interpreter fallback. In june's own sessions with
the two Emerald romhacks it was 98.6 to 98.7%. The build measured that
itself, and no check in the repo reproduces it.

## How a build is checked

A game is tested against mGBA 0.10.5, and mGBA's answer is taken as right. A
person never decides what the right picture is, because a person looking at a
game that mostly works will pass it.

A check on a game is a **route**: a ROM, a script of button presses, and a
list of **checkpoints**, which are named frames (a frame is one refresh of the
screen, about a sixtieth of a second). At each checkpoint three programs must
draw the same picture, compared by hash: mGBA, gbaa's interpreter, and the
recompiled build. All three are run on every check. The hash mGBA must draw is
kept in the route's file, and a person looked at that frame once, when the
route was written, to confirm it is the scene the route was meant to reach.
Part of the route that takes Pokémon Emerald, a commercial game, from reset to
its first frame of play:

```toml
[[press]]            # A every 20 frames, through the opening speech
button = "a"
from = 1100
to = 5380
every = 20

[[checkpoint]]       # the player in the moving truck
name = "truck"
frame = 5400
hash = "26c2f9d4009c5696"
```

That route has four checkpoints over 5,400 frames. The repo holds 46 routes
with 91 checkpoints, one TOML file per route in the `units/` folder. They
cover commercial games, ROMs written to test emulators, and small programs
written here to exercise one hardware feature each. Six of the routes, with
37 of the checkpoints, need a commercial ROM. The other 40 run on ROMs that
anyone can fetch or build from this repo.

How far that goes, exactly:

- **Against mGBA:** the picture only, at the checkpoints only. Sound and
  memory are not compared with mGBA on a route, and neither are the frames
  between checkpoints.
- **Recompiled build against gbaa's interpreter:** the picture of every frame
  on the route, and the CPU registers after the last one. The exceptions are
  written in the route's file, each with its reason: one frame on each of
  two routes may differ in its picture, and on two routes the registers may
  differ and are then held to a second interpreter that keeps time the way
  compiled code does.
- **Not against mGBA yet:** the 4 checkpoints of one route, for Kirby:
  Nightmare in Dream Land. It was written for the native check described
  below, and its hashes are gbaa's own picture.

So "matches mGBA" in this repo always means "draws the same picture at these
frames, on this input". It is not a claim about every frame of every game.

mGBA is sometimes wrong. A checkpoint where mGBA is wrong carries two hashes,
the one gbaa must draw and the one mGBA draws, with the reason written beside
them. That is a **known mismatch**, and 16 of the 91 checkpoints have one. An
example from Emerald's main menu: mGBA, in the 32-bit colour build used here,
darkens a blended menu box one step further in green and blue than the
hardware, which works in 5 bits per colour channel, does. If mGBA or gbaa
later draws anything else there, the check fails.

Nine of the 16 are colour rounding of that kind. What stands for the hardware
there is the 5-bit formula in GBATEK, the standard reference for GBA hardware,
and mGBA's own 16-bit colour build; no capture from a console backs them. The
rest are CPU, BIOS and bus cases, among them cases of a test ROM that mGBA
0.10.5 itself fails. Each reason is in its route's file, in the `mgba_why`
field.

Smaller checks sit under the routes, each against its own reference. As run
on 2026-11-18:

- **Single instructions.** The
  [SingleStepTests](https://github.com/SingleStepTests/ARM7TDMI) suite gives
  the CPU state before and after one instruction. The test compares the
  registers, the status registers and the data the instruction reads and
  writes; it does not compare cycle counts or instruction fetches. The
  interpreter passes 2,173,242 of the 2,224,898 cases it runs; 25,102 cases of
  `blx`, an instruction the GBA's CPU doesn't have, are left out. The misses
  are in 4 of the 45 files (`arm_mrs`, `thumb_ldm_stm`, `thumb_push_pop` and
  `thumb_undefined_bcc`), and in every one the first thing that differs is
  the program counter, on an encoding where gbaa disagrees with
  NanoBoyAdvance, the emulator the suite was generated with. One of the four,
  `thumb_undefined_bcc`, fails all 50,000 of its cases: it is a branch
  encoding that the ARM specification leaves undefined. Each file's expected
  pass count is recorded in the test
  ([sst.rs](../crates/armv4t/tests/common/sst.rs)), so a change in either
  direction fails.
- **Test ROMs.** jsmolka's [gba-tests](https://github.com/jsmolka/gba-tests)
  are ROMs that test themselves and report a result. The interpreter passes
  the five it runs: `arm`, `thumb`, `memory`, `nes` and `bios`.
- **Jump tables.** A `switch` compiles to a table of code addresses that the
  recompiler has to find. The truth comes from a decompilation: C source that
  volunteers wrote by hand until it compiles back to the exact ROM, so its
  symbol table says where every function and table really is. Against
  [pokeemerald](https://github.com/pret/pokeemerald), the decompilation of
  Pokémon Emerald, gbaa recovers 676 of 676 tables in the ROM as shipped and
  610 of 617 in a build of the same source with a modern GCC. The other 7 are
  not recovered, or only in part; no table in either build is recovered with a
  wrong target.

## How the recompiler works

**Each instruction is defined once.** The meaning of every ARM instruction is
one Rust function, generic over a trait whose associated types say what a
value is:

```rust
pub trait Sem: Sized {
    /// A 32-bit value.
    type V: Copy;
    /// A one-bit value (flags and conditions).
    type B: Copy;

    fn imm(&mut self, x: u32) -> Self::V;
    /// Wrapping addition, no flags.
    fn add(&mut self, a: Self::V, b: Self::V) -> Self::V;
    // ... and the other operations
}
```

With a `u32` for the value it is the interpreter. With an id into an
intermediate representation it is the first stage of the recompiler. With a
set of possible values it is the abstract interpreter that finds jump tables.
Two hand-written copies of the instruction set would drift apart; one copy
can't, so the interpreter and the recompiled build disagree only when
something after that shared definition is wrong. The comparison of every frame
on a route, described above, looks for exactly that.

**Finding the code.** A ROM mixes code and data, and nothing in it says which
is which. **Code discovery** follows branches and calls from the entry point,
then scans the ROM for pointers that might be functions and keeps the ones
that hold up as code. It is scored against pokeemerald's symbol table too. Of
15,190 function starts gbaa finds in Emerald, 15,185 are real (precision
0.9997), out of 15,859 that exist (recall 0.9575).

**A miss is slow, not wrong.** Finding all the code in a binary is undecidable
in general, so discovery will miss some. When the game jumps to an address
with no compiled code, the interpreter fallback runs it and hands back at the
next compiled function, and the address is written to a file beside the build.
(Nothing reads that file yet; feeding it into the next recompile is planned.)
A miss lowers the compiled share and changes nothing else.

**The output.** One Rust function per function of the game. Where the
control-flow graph allows, the game's branches are rebuilt as `if`, `loop` and
`match`; the rest stays a loop over a `match` on the program counter. rustc
type-checks all of it, and cargo builds it with the runtime.

## Native ports

A recompiled build still runs on an emulated machine: its code moves values
between the GBA's registers and reads the GBA's memory map. gbaa can also
**lift** a game, which drops that machine and leaves an ordinary Rust program.
Lifted code has no emulated CPU. Its functions take a struct of the game's
state, and where the original wrote to hardware registers they call a
**hardware library** (the runtime's screen and sound code behind a plain
interface). The result is a **native port**, and the part of gbaa that
produces it is the lifter. It exists so that a game can be modded, that is
changed by its players, by editing Rust. It is at an early stage, and where
it stands is listed below.

A native port is tested against the game's recompiled build, and this
comparison, the **native check**, is much wider than the one against mGBA. It
compares every frame, and in each frame the picture, RAM, the I/O registers,
video memory and the audio samples, byte for byte. A few things are left out,
chiefly the stack and the code a game copies into RAM. Note what it shows:
that the port matches gbaa's recompiled build, which is only as right as its
own checks against mGBA.

The check itself is checked. A **planted fault** is one store in the lifted
code changed on purpose; the native check has to fail on it and name the frame
and the byte. A check that has never failed may not be able to.

Where it stands:

- Metroid: Zero Mission, a commercial game, lifts to 2,623 functions in about
  325,000 lines of Rust and passes the native check for 2,700 frames, from
  reset through the title screen into the first room, with 0 differences.
  That is agreement with gbaa's recompiled build. Against mGBA, the route has
  4 checkpoints.
- The lifted code is literal. It is not yet something you would want to read
  or edit.
- Typing the data is partly done. A global at a fixed address is a struct
  field. Most pointers are still raw GBA addresses.

This is `pen_set` from one of the repo's own C test programs
(`c/switch/main.c`), and what the lifter makes of it today. In the ROM,
`gPen.state` and `gPen.step` are the bytes at addresses 0x0300000c and
0x0300000d. The lifter has turned each into a field of the struct that holds
the game's RAM, named after its address (`d_0300000c`) because this program
has no file of names. The rest is the literal part: the CPU's registers are
still there as local variables, and the function still loads `gPen`'s address
into `r1` and a zero into `r2` as the machine code did, though nothing reads
them now. The long line that unpacks the registers is shortened here.

```c
void pen_set(u8 state)
{
    gPen.state = state;
    gPen.step = 0;
}
```

```rust
pub fn x_08000271(g: &mut Game, s: Regs) -> Result<(), Exit> {
    let Regs { r: [mut r0, mut r1, /* r2 to r14 */], f: [mut nf, mut zf, mut cf, mut vf] } = s;
    runtime::guest_pc::set(0x08000271);
    r1 = 0x3000008u32;
    r2 = 0x0u32;
    g.ram.d_0300000c = r0 as u8;
    g.ram.d_0300000d = 0x0u32 as u8;
    let v22: u32 = r14;
    return Ok(());
}
```

If you work on a decompilation: names come from its symbols through a **source
map**, a file of addresses and names kept apart from the generated code, so
regenerating the code keeps every name. A source map holds no game code. A mod
made from a native port runs only in gbaa. A native port can't be turned back
into a GBA ROM.
