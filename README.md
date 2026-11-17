# GBA Advanced (gbaa)

gbaa is a static recompiler for Game Boy Advance games. It translates a
ROM's ARM and Thumb machine code ahead of time into a Rust crate, and
compiles that crate with a small runtime (the bus, the PPU, sound, timers,
DMA and the BIOS calls) into a native program. Pokémon Emerald and two
romhacks of it play this way, and I've played one of them, Emerald
Imperium, for half an hour on a Steam Deck.

Agents wrote most of the code. What decides whether their work is right is
a check against the [mGBA](https://github.com/mgba-emu/mgba) emulator: along
a scripted run of a game, the screen mGBA draws at each checkpoint has to be
the screen gbaa draws, and the recompiled build has to draw the same frame
as gbaa's own interpreter on every frame of the run.

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
decides when the unit is done, not the agent that did it and not me. The
check catches most bugs. The ones it misses are the useful part: each one
says where the check, or the factory around it, needs tuning. One example:
the check for native ports (below) wasn't in the merge gate, and it stayed
broken from 2026-11-06 to 2026-11-13 without anyone seeing. Now
`just check` runs it, and a deliberately planted break shows that it fails
when it should.

## What's compared, exactly

| Check | What it compares | How often |
|---|---|---|
| Routes (`just unit NAME`, the 40 files in [units/](units/)) | mGBA 0.10.5's screen hash against our interpreter's; then the recompiled build's screen hash against the interpreter's | mGBA at each checkpoint; the recompiled build on every frame, plus the CPU registers at the end |
| CPU ([SingleStepTests' ARM7TDMI suite](https://github.com/SingleStepTests/ARM7TDMI)) | registers and bus transactions after one instruction | every case: 40 of its 45 files pass all 50,000; the other five, and why, are in [crates/armv4t/tests/common/sst.rs](crates/armv4t/tests/common/sst.rs) |
| Hardware test ROMs (jsmolka's gba-tests, armwrestler) | the ROM's own pass/fail result | each test |
| Native ports (`just native-check`) | a lifted program against the recompiled build: picture, RAM, the I/O registers, palette, VRAM, OAM and the audio samples | every frame, with a planted fault that must be caught |

Where mGBA itself is wrong, the route says so instead of quietly trusting
it. Emerald's intro route ([units/emerald-intro.toml](units/emerald-intro.toml))
has six checkpoints from reset to the title screen. At two of them mGBA's
32-bit build rounds blended colours up by one 5-bit step, so the expected
hash there is ours, with mGBA's hash and the reason kept beside it; our
interpreter, given mGBA's rounding, draws mGBA's hash exactly. Three of
jsmolka's tests that mGBA 0.10.5 fails are listed the same way in
[crates/mgba-dump/known-mismatches.tsv](crates/mgba-dump/known-mismatches.tsv).

What the routes don't cover: anything past their last frame. Past that,
the games were checked by playing them, at gates that only I could pass
(the Steam Deck play test below is one).

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
  lifts and builds. The plan and the numbers are in
  [docs/native-stage-b.md](docs/native-stage-b.md).
- **Side projects on the same runtime**: an mGBA-compatible Lua scripting
  host, and linked multiplayer over the internet with rollback (Emerald's
  trade check passes over a simulated bad connection; I haven't yet traded
  with a friend for real).
- The tracker holds 473 units of work: 394 done, 27 open, the rest
  parked, dropped, blocked or in progress.

## Who it's for

- **Modders, and people starting a decompilation or a native port of a
  GBA game.** gbaa gives you a Rust crate of the whole game that builds and
  runs like the original from day one, so you can change one function and
  play the result. With a decompilation's symbols (pret's pokeemerald, for
  Emerald) it names functions, and `just mod-cut` copies one function out
  as readable Rust for you to edit; an agent's first small Emerald mod, and
  what was hard about it, is in
  [docs/research/agent-first-mod.md](docs/research/agent-first-mod.md).
  Without symbols it won't name anything for you, and the readable form
  doesn't read like the decompilation's C yet.
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
no arguments. Building Emerald's crate is heavy (several minutes and
several GB of RAM). Every command is described in
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
This is what we used, roughly in order of how much we leaned on it. The
full reading list is in [docs/references.md](docs/references.md), and what we took from each is in
the notes under [docs/research/](docs/research/).

### What we check against

- **[mGBA](https://github.com/mgba-emu/mgba)**, by Vicki Pfau (endrift) and
  its contributors. It is the oracle: at each checkpoint of a route, gbaa
  has to draw the screen it draws. Its source was our reference for the PPU, sound, DMA,
  timers and its HLE BIOS, its Lua API is the one our scripting host
  copies, and its demo scripts are among our tests.
- **[GBATEK](https://problemkaputt.de/gbatek.htm)**, by Martin Korth: the
  hardware spec everyone actually uses.
- **Arm's** ARM7TDMI Technical Reference Manual and the ARM Architecture
  Reference Manual (ARMv4T), and the AAPCS
  ([ARM-software/abi-aa](https://github.com/ARM-software/abi-aa)).
- **[SingleStepTests' ARM7TDMI suite](https://github.com/SingleStepTests/ARM7TDMI)**:
  before and after state for every instruction. It was generated with
  **[NanoBoyAdvance](https://github.com/nba-emu/NanoBoyAdvance)** by
  fleroviux, whose rules we follow wherever ARMv4 leaves behaviour
  unpredictable.
- **[jsmolka's gba-tests](https://github.com/jsmolka/gba-tests)**, including
  the `bios.gba` that checks our BIOS calls.
- **armwrestler**, originally by Mic, in
  [destoer's fixed GBA build](https://github.com/destoer/armwrestler-gba-fixed).
- **[zaydlang's multiplication-algorithm](https://github.com/zaydlang/multiplication-algorithm)**:
  our multiplier's carry flag is ported from its model of the hardware.
- **The Cult-of-GBA BIOS**: our reference, with mGBA's, for what the BIOS
  calls do. `MidiKey2Freq`'s `exp2f` is ported from glibc's so it rounds
  the same way.

### Decompilations

- **[pret](https://github.com/pret)** et al., above all
  **[pokeemerald](https://github.com/pret/pokeemerald)**. Its symbols are
  the ground truth that code discovery and jump-table recovery are scored
  against, its m4a sources taught us the sound engine, and its names are
  the ones the readable output uses. Also
  [pokefirered](https://github.com/pret/pokefirered),
  [agbcc](https://github.com/pret/agbcc) (the compiler the games were built
  with, which we build them with again), and camthesaxman's
  [gbadisasm](https://github.com/camthesaxman/gbadisasm), the prior art for
  finding code in a GBA ROM.
- jiangzhengwenjz's **[katam](https://github.com/jiangzhengwenjz/katam)**
  (Kirby & The Amazing Mirror) and
  [agbcc fork](https://github.com/jiangzhengwenjz/agbcc), zeldaret's
  **[tmc](https://github.com/zeldaret/tmc)** (The Minish Cap) and
  metroidret's **[mzm](https://github.com/metroidret/mzm)** (Zero Mission),
  which we build for symbols and library matching.
- huderlem's [poryscript](https://github.com/huderlem/poryscript), needed to
  build Emerald Rogue.

### Games we ran

Every one of these was a target, and some found bugs nothing else would
have. None of them are included here.

- Pokémon Emerald (Game Freak, Nintendo), Kirby: Nightmare in Dream Land and
  Kirby & The Amazing Mirror (HAL Laboratory, Nintendo), WarioWare, Inc.:
  Mega Microgame$! (Nintendo).
- **[Pokémon Emerald Rogue](https://github.com/Pokabbie/pokeemerald-rogue)**
  by Pokabbie. Our online multiplayer for it follows Pokabbie's
  [Rogue Assistant](https://github.com/Pokabbie/pokeemerald-rogue-assistant)
  closely enough to be compatible with it on the wire.
- **Pokémon Emerald Imperium** by iriv24, the one played on the Steam
  Deck.
- **Super Mariomon** by Kobazco and team.
- **[moonbeam](https://ledoux.itch.io/moonbeam)** by Adam Ledoux, built
  with his [moonbeamkit](https://codeberg.org/adamledoux/moonbeamkit)
  (devkitARM GCC, scripted in JavaScript through Duktape).
- ROMs randomized with the
  [Universal Pokemon Randomizer ZX](https://github.com/Ajarmar/universal-pokemon-randomizer-zx)
  (Ajarmar and the randomizer's earlier authors), with the Kaizo IronMON
  settings and Corvimae's
  [kaizo-ironmon-plus](https://github.com/Corvimae/kaizo-ironmon-plus).
- Our own test programs are written with **[agb](https://github.com/agbrs/agb)**,
  the Rust GBA library.

### Lua scripts we made run

The scripting host was checked by running the community's scripts in it. The survey, with commits, is in
[docs/research/lua-scripts-survey.md](docs/research/lua-scripts-survey.md):
Real96's [PokeLua](https://github.com/Real96/PokeLua),
[PokeRNG-LuaScripts](https://github.com/Wi-Fi-Labs/PokeRNG-LuaScripts),
besteon's [Ironmon-Tracker](https://github.com/besteon/Ironmon-Tracker),
MKDasher's [PokemonBizhawkLua](https://github.com/mkdasher/PokemonBizhawkLua),
nikouu's [mGBA-http](https://github.com/nikouu/mGBA-http),
dmang-dev's [mcp-mgba](https://github.com/dmang-dev/mcp-mgba),
Gianni1707's [FireRedCoop](https://github.com/Gianni1707/FireRedCoop),
Jengolus's [RAManimator](https://github.com/Jengolus/ramanimator),
[Archipelago](https://github.com/ArchipelagoMW/Archipelago)'s connectors,
and the rest listed there.

### Libraries

- **[unarm](https://github.com/AetiasHax/unarm)** by AetiasHax: the ARM and
  Thumb decoder under `crates/armv4t`.
- dtolnay's [quote](https://github.com/dtolnay/quote),
  [syn](https://github.com/dtolnay/syn),
  [prettyplease](https://github.com/dtolnay/prettyplease) and
  [serde](https://github.com/serde-rs/serde): the code generator is built
  on them.
- [object](https://github.com/gimli-rs/object) and
  [gimli](https://github.com/gimli-rs/gimli) for reading the decomps' ELFs
  and DWARF.
- [SDL](https://github.com/libsdl-org/SDL) through
  [sdl3-rs](https://github.com/vhspace/sdl3-rs),
  [egui](https://github.com/emilk/egui) by Emil Ernerfeldt (with
  [egui-sdl3](https://github.com/abus-sh/egui-sdl3) as the model for our
  glue), [cpal](https://github.com/RustAudio/cpal) and
  [ab_glyph](https://github.com/alexheretic/ab-glyph) for the desktop
  frontend.
- [Lua](https://www.lua.org) (PUC-Rio) through
  [mlua](https://github.com/mlua-rs/mlua) for scripting.
- [ENet](https://github.com/lsalzman/enet) by Lee Salzman, through
  jabuwu's [rusty_enet](https://github.com/jabuwu/rusty_enet), for
  multiplayer.
- Amanieu d'Antras's [corosensei](https://github.com/Amanieu/corosensei)
  for running guest code as a coroutine.
- [png](https://github.com/image-rs/image-png),
  [miniz_oxide](https://github.com/Frommi/miniz_oxide),
  [asefile](https://crates.io/crates/asefile),
  [toml](https://github.com/toml-rs/toml), [cc](https://github.com/rust-lang/cc-rs)
  and [libc](https://github.com/rust-lang/libc).
- The [Arm GNU Toolchain](https://developer.arm.com/downloads/-/arm-gnu-toolchain-downloads)
  for building the decomps.

### Prior art we learned from

- **[AnyPS5](https://github.com/boykopovar/AnyPS5)** by boykopovar, which
  relinks PS5 games into native programs, is where the idea came from.
- The static recompilers before us: Wiseguy's
  [N64Recomp](https://github.com/N64Recomp/N64Recomp) and
  [N64ModernRuntime](https://github.com/N64Recomp/N64ModernRuntime),
  hedge-dev's [XenonRecomp](https://github.com/hedge-dev/XenonRecomp),
  [ReXGlue](https://github.com/rexglue/rexglue-sdk),
  [DolRecomp](https://github.com/ExpansionPak/DolRecomp), ran-j's
  [PS2Recomp](https://github.com/ran-j/PS2Recomp), and the other GBA
  recompilers: mstan's [gbarecomp](https://github.com/mstan/gbarecomp) and
  [arm-recomp-core](https://github.com/mstan/arm-recomp-core), JRickey's
  [gba-recomp](https://github.com/JRickey/gba-recomp), Asphaltian's
  [GBARecomp](https://github.com/Asphaltian/GBARecomp) and the
  [Kirby & The Amazing Mirror recomp](https://github.com/Felipe-Gomes-Amorim/kirby-amazing-mirror-recomp).
  Also Graham Toal's static binary translation HOWTO and Andrew Kelley's
  jamulator.
- The structured output implements Norman Ramsey's "Beyond Relooper:
  Recursive Translation of Unstructured Control Flow to Structured Control
  Flow" (ICFP 2022). Jump-table recovery is value-set analysis after
  Balakrishnan and Reps, "WYSINWYX: What You See Is Not What You eXecute"
  (TOPLAS 2010), with Cifuentes and Van Emmerik's "Recovery of Jump Table
  Case Statements from Binary Code" (2001).
- Decompilers and lifters we read the source of:
  [Ghidra](https://github.com/NationalSecurityAgency/ghidra), IDA and
  Hex-Rays, [angr](https://github.com/angr/angr),
  [rev.ng](https://github.com/revng/revng),
  [RetDec](https://github.com/avast/retdec),
  [remill](https://github.com/lifting-bits/remill),
  [c2rust](https://github.com/immunant/c2rust) and
  [Cheerp](https://github.com/leaningtech/cheerp-compiler)'s stackifier.
- Emulators and translators we read the source of:
  [gpsp](https://github.com/libretro/gpsp), [QEMU](https://gitlab.com/qemu-project/qemu),
  [FEX](https://github.com/FEX-Emu/FEX), [Box64](https://github.com/ptitSeb/box64),
  [Dolphin](https://github.com/dolphin-emu/dolphin),
  [BizHawk](https://github.com/TASEmulators/BizHawk) and
  [Mesen2](https://github.com/SourMesen/Mesen2).
- For multiplayer, afska's
  [gba-link-connection](https://github.com/afska/gba-link-connection) (the
  best documentation of the link hardware) and
  [GGRS](https://github.com/gschup/ggrs)'s sync-test idea. For pulling
  music out of ROMs, ipatix's [agbplay](https://github.com/ipatix/agbplay)
  and Kermalis's [VGMusicStudio](https://github.com/Kermalis/VGMusicStudio).
- For coroutines in emulators, Sanjeet's
  "[async/await emulators](https://sanjeetnd.com/async-await-emulators)" and
  near's "[cooperative threading](https://archive.ares-emu.net/near.sh/articles/design/cooperative-threading.html)".

### Tools

The code was written by Claude (Anthropic) through Claude Code. Claude
also wrote this README, from my notes and decisions.

If we used your work and you aren't here, that's our mistake. Please
open an issue and we'll add you.

## License

[MIT](LICENSE).
