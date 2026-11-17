# GBA Advanced (gbaa)

gbaa is a static recompiler for Game Boy Advance games. It translates a
ROM's ARM and Thumb machine code ahead of time into a Rust crate, and
compiles that crate with a small runtime (the bus, the PPU, sound, timers,
DMA and the BIOS calls) into a native program. Pokémon Emerald and two
romhacks of it play this way, and I've played one of them, Emerald
Imperium, for half an hour on a Steam Deck.

Agents wrote most of the code. What decides whether their work is right is
a check against the [mGBA](https://github.com/mgba-emu/mgba) emulator. A
route is a scripted run of a game with a few checkpoint frames. At each
checkpoint, gbaa's interpreter has to draw the screen mGBA draws, and on
every frame of the run the recompiled build has to draw what the
interpreter draws. (That second part checks the recompiler against our own
interpreter, not against mGBA.)

> **Not ready for general use.** This is public so people can watch it
> being built. It plays Pokémon Emerald and a couple of romhacks well for
> me; everything else is in progress, and nothing is stable.

## Why it's built around a check

Writing code got cheap. Reviewing it didn't. A recompiler is a lot of
code, and a mistake in it doesn't crash where you can see it: you boot a
game and random things go wrong, a sprite in the wrong place, a sound that
cuts out, a save that doesn't load. Nobody wants to find all of those by
reading the code, me included.

So in gbaa every unit of work ends in a check a machine runs, and the check
decides when the unit is done, not the agent that did it and not me. When
Metroid's native port first ran against the recompiled build, for
example, the check stopped at the first byte that differed, and each cause
became a unit of its own: DMA by address, the save chip, loads from the
BIOS region, and a run of timing details. The check catches most bugs.
The ones it misses are the useful part: each one
says where the check, or the factory around it, needs tuning. One example:
the check for native ports (below) wasn't in the merge gate, and it stayed
broken from 2026-11-06 to 2026-11-13 without anyone seeing. Now
`just check` runs it, and a deliberately planted break shows that it fails
when it should.

That's the work I'm looking for: building the checks that let
agent-written code be trusted. More [about me](#about-me) at the end.

## What's compared, exactly

| Check | What it compares | How often |
|---|---|---|
| Routes (`just unit NAME`, the 40 files in [units/](units/)) | mGBA 0.10.5's screen hash against our interpreter's; then the recompiled build's screen hash against the interpreter's | mGBA at each checkpoint; the recompiled build on every frame, plus the CPU registers at the end |
| CPU ([SingleStepTests' ARM7TDMI suite](https://github.com/SingleStepTests/ARM7TDMI), generated with NanoBoyAdvance, not mGBA) | registers and bus transactions after one instruction | every case: 40 of its 45 files pass all 50,000. The other five are cases where NanoBoyAdvance does something ARMv4T leaves unpredictable or undefined, plus ARMv5 `blx` cases left uncounted; the causes are noted in [crates/armv4t/tests/common/sst.rs](crates/armv4t/tests/common/sst.rs) |
| Hardware test ROMs (jsmolka's gba-tests, armwrestler) | the ROM's own pass/fail result | each test |
| Sound (`just mgba-audio-diff`) | mGBA's direct-sound and PSG samples against ours, byte for byte | every sample, on a few test programs, starting from an mGBA save state |
| Native ports (`just native-check`) | a lifted program against the recompiled build: picture, RAM, the I/O registers, palette, VRAM, OAM and the audio samples | every frame, with a planted fault that must be caught |

Where mGBA itself is wrong, the route says so instead of quietly trusting
it. Emerald's intro route ([units/emerald-intro.toml](units/emerald-intro.toml))
has six checkpoints from reset to the title screen. At two of them mGBA's
32-bit build rounds blended colours up by one 5-bit step, so the expected
hash there is ours, with mGBA's hash and the reason kept beside it; our
interpreter, tried once with mGBA's rounding (2026-10-22; it isn't a
standing test), drew mGBA's hash exactly. Across all 40
routes there are 83 checkpoints, and at 16 of them mGBA is overridden this
way: 9 for that colour rounding in blends and fades, 3 for jsmolka's tests
that mGBA 0.10.5 fails (also listed in
[crates/mgba-dump/known-mismatches.tsv](crates/mgba-dump/known-mismatches.tsv)),
and 4 for details of mGBA's high-level BIOS, SRAM and serial port. Each
says why in its route file.

What isn't compared with mGBA:

- Frames between checkpoints. Our builds write every frame's hash, so it's
  easy to look: on the `jsmolka-memory` route below, mGBA and both our
  builds agree on 299 of its 300 frames (compare the `*-hashes.txt` files
  in `out/units/jsmolka-memory/`). Frame 1 differs on a single
  scanline (row 126 is black in mGBA's frame and white in ours), which I
  haven't traced yet. Only the checkpoints decide.
- Timing, except through what it draws and plays. gbaa isn't cycle-exact
  against mGBA: from reset, a program built with agb boots faster in ours,
  which is why the sound checks start from an mGBA save state.
- Anything past a route's last frame. Past that, the games were checked by
  playing them, at gates only a person could pass (the Steam Deck play
  test below is one).

## Where it is

As of 2026-11-15, from the [tracker](tracker/):

- **Pokémon Emerald, Emerald Rogue and Emerald Imperium play recompiled**,
  with in-game saves, save states, fast-forward, rewind and a controller
  (passed 2026-10-24). On 2026-10-24 I played Imperium on a Steam Deck in
  Game Mode from 21:52 to 22:25: no problems, no audio underruns, and
  98.9% of the guest's cycles ran in compiled code rather than the
  interpreter fallback.
- There are builds for Linux, macOS and Windows; I played the macOS and
  Windows builds on real machines on 2026-10-28.
- **Two forms of output.** A flat one, and a structured one (loops and
  `if`s rather than a block dispatcher) that passes the same routes. For
  Emerald the structured binary is 4.2% smaller (128.1 MB against
  133.8 MB) and the whole build took 255 s against 294 s, one measurement
  on a busy machine ([docs/structured-codegen-design.md](docs/structured-codegen-design.md)).
- **Native ports**, where a game becomes a Rust program with no emulated
  CPU or dispatcher, are the current work. So far the output is literal
  (registers as Rust locals, RAM as bytes), and each game needs a few
  hand-written stand-ins for code the lifter can't handle yet. Metroid:
  Zero Mission lifts to 2,624 functions that build, and runs the 2,700
  frames of its route (reset to the first room) with no difference from
  the recompiled build. Kirby: Nightmare in Dream Land passes the same
  check for 900 frames, into its first level; Kirby & The Amazing Mirror
  lifts and builds. Those stand-ins (8 for Metroid, 5 for Nightmare) port
the game's own code, so they live in the private, gitignored `mods/`. The
plan and the numbers are in
  [docs/native-stage-b.md](docs/native-stage-b.md).
- **Side projects on the same runtime**: an mGBA-compatible Lua scripting
  host, and linked multiplayer over the internet with rollback (Emerald's
  trade check passes over a simulated bad connection; I haven't yet traded
  with a friend for real).
- The tracker holds 473 units of work: 394 done, 27 open, the rest
  parked, dropped, blocked or in progress. Its log records 422 times a
  unit was marked done and 56 times one was reopened.

## Who it's for

- **Modders, and people starting a decompilation or a native port of a
  GBA game.** gbaa gives you a Rust crate of the whole game that builds and
  runs like the original from day one, so you can change one function and
  play the result. With a decompilation's symbols it names functions: for
  Emerald, `just fetch-pret` builds pokeemerald from pret's source and
  `just pret-names` turns its symbols into names (the same goes for
  FireRed; `just rogue-names` and `just decomp-names` cover Emerald Rogue,
  The Minish Cap, Metroid: Zero Mission and Kirby & The Amazing Mirror). Then `just mod-cut`
  copies a function out as readable Rust for you to edit, and the game is
  rebuilt with your copy in place. An agent's first small Emerald mod, and
  what was hard about it, is in
  [docs/research/agent-first-mod.md](docs/research/agent-first-mod.md).
  What you don't get: matching C, or anything agbcc could build. The Rust
  stays close to the machine code (locals named `v7` and `v11`, pointer
  arithmetic, with field names only where pret's types reach), and
  without symbols nothing is named. Of the agent's complaints, bitfield
  writes and `#define` values now print by name; enums still print as
  numbers.
- **People who want to play their own games natively.** For the three
  Emerald games it already works nicely for me. The UI is plain, and other
  games aren't there yet.

## Try it

You need Rust (the toolchain is pinned in `rust-toolchain.toml`),
[just](https://just.systems), CMake and a C compiler (SDL3 and Lua are
built from source). Most of it is Linux-first: the heavy recipes run
under a systemd memory cap.

Without a ROM of your own, the hardware test suites show the whole method
end to end:

```sh
git clone https://github.com/junewunder/gbaa && cd gbaa
just fetch-tests            # the CPU and hardware test suites, about 1.3 GB, into ~/.local/share/gbaa
cargo test --workspace      # unit and integration tests
just gba-tests              # jsmolka's five test ROMs in the interpreter
just unit jsmolka-memory    # one route: mGBA, the interpreter and the recompiled build
```

On a fresh clone (Linux, two build jobs, 2026-11-17) the tests reported
1,165 passed, 0 failed and 11 ignored, after about six minutes of building.
`just gba-tests` printed `pass` for all five ROMs. The route builds mGBA
0.10.5 from source the first time (about eight minutes in all), then
prints:

```
unit jsmolka-memory: mGBA and the interpreter draw every checkpoint
cycles: 84185037 compiled (100.0%, ...), 0 in the interpreter fallback (0.0%, ...)
unit jsmolka-memory: pass (jsmolka-memory to frame 300; mGBA matches; recompiled = interpreted)
```

`just check`, the merge gate, needs more than a fresh clone has (agb test
programs built into `~/.local/share/gbaa`, and the game ROMs the routes
name), so it isn't the place to start.

**No ROMs are included**, and none ever were. To recompile a game, bring
your own dump:

```sh
just recompile path/to/game.gba          # writes a crate to out/<name>/
just run-recompiled out/<name>            # builds it and plays it
```

The built binary carries the ROM inside it and plays it when started with
no arguments. The Emerald routes expect the US ROM that pokeemerald builds
(SHA-1 `f3ae088181bf583e55daf962a92bb46f4f1d07b7`). Building Emerald's
crate is heavy: on my machine it took about four minutes with six build
jobs, and the compilers together peaked at about 5 GB of RAM. Every command is described in
[docs/tools/commands.md](docs/tools/commands.md).

## How it works

1. **Decode** (`crates/armv4t`): every ARMv4T instruction, with its semantics
   written once against a `Sem` trait. The same definition drives the
   interpreter, an IR builder and the code generator, so there is one
   description of what each instruction does.
2. **Discover** (`crates/analysis`): find the code. Telling code from data in
   a ROM is undecidable in general, so discovery combines recursive descent,
   jump-table recovery by value-set analysis, and profiles of real runs.
   Anything it misses still runs, through an interpreter fallback that hands
   control back to compiled code as soon as it can.
3. **Emit** (`crates/codegen`): one Rust function per guest function, built
   with `quote!` and `prettyplease`, with the runtime's memory accessors
   inlined. Code the game copies into RAM is handled too.
4. **Run** (`crates/runtime`, `#![no_std]`; `crates/desktop` for the SDL
   front end).

The reasoning behind each choice, with the alternatives and what would
change my mind, is in [docs/decisions.md](docs/decisions.md).

## How it was built

Claude agents wrote most of this code between mid-October and mid-November
2026, working from a tracker of small units of work. Each unit names the
check that decides it's done, usually a route like the ones above, and
lanes of agents on several machines claimed, implemented, checked and
merged units unattended. I made the design decisions, wrote the specs,
pruned the tracker, and played the games at the gates that only a person
could pass. The setup is [WFactory](https://github.com/junewunder/wfactory);
how this project uses it is in [docs/factory.md](docs/factory.md). It works
here because the oracle is exact: a frame either matches or it doesn't.

## Developed in the open

The tracker (`tracker/`), the agents' instructions (`AGENTS.md`) and their
notes (`memory/`) are all here, as the agents use them; that's why the
commit log is mostly tracker commits. Earlier history lives in a private
repo, because it held code translated from the games. Game-derived code
(mod copies, hand ports, snapshots of the output on a real game) stays out
of git, so the Emerald, Metroid and Kirby results above need your own ROM
to reproduce.

gbaa isn't affiliated with Nintendo, Game Freak, The Pokémon Company or
HAL Laboratory. Their games and names belong to them.

Issues are welcome. I'm not taking pull requests yet.

## About me

I'm june wunder, and I'm looking for work. I build machine-driven checks
for code that people and agents write. gbaa is built with
[WFactory](https://github.com/junewunder/wfactory), my setup for running
agents through a project unattended.

## References and thanks

gbaa is built on other people's work, and most of it was done for free.
Above all: [mGBA](https://github.com/mgba-emu/mgba) by Vicki Pfau
(endrift) and its contributors, the oracle;
[GBATEK](https://problemkaputt.de/gbatek.htm) by Martin Korth;
[SingleStepTests' ARM7TDMI suite](https://github.com/SingleStepTests/ARM7TDMI),
generated with fleroviux's [NanoBoyAdvance](https://github.com/nba-emu/NanoBoyAdvance);
[jsmolka's gba-tests](https://github.com/jsmolka/gba-tests); armwrestler,
by Mic, in [destoer's fixed build](https://github.com/destoer/armwrestler-gba-fixed);
[pret](https://github.com/pret)'s [pokeemerald](https://github.com/pret/pokeemerald),
whose symbols are the ground truth for code discovery; the other
decompilations (katam, tmc, mzm); [agb](https://github.com/agbrs/agb);
AetiasHax's [unarm](https://github.com/AetiasHax/unarm); and the static
recompilers before this one, Wiseguy's N64Recomp among them.

The full list, with what we used each for, the games and romhacks we ran,
the Lua scripts we made run and the libraries, is in
[docs/credits.md](docs/credits.md). The reading list is in
[docs/references.md](docs/references.md). If we used your work and you
aren't there, that's our mistake: please open an issue and we'll add you.

Claude (Anthropic) wrote the code through Claude Code, and wrote this
README from my notes and decisions.

## License

[MIT](LICENSE).
