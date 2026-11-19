# gbaa

gbaa turns Game Boy Advance games into Rust. It has two parts.

**The recompiler** translates a game's ARM machine code into Rust ahead of
time and builds it into a native program with the console's hardware built
in. You run the game itself, with no emulator to set up, and you get save
states, rewind and fast-forward. This part works today. Pokémon Emerald
recompiles and is checked against [mGBA](https://mgba.io), an established
GBA emulator, and two fan-made versions of Emerald have been played on a
desktop and a Steam Deck.

**The decompiler** is in progress. Its aim is Rust that a person or an agent
can read and edit, with no emulated CPU underneath, so that a game can be
modded. Today it handles the opening of Metroid: Zero Mission, and its output
is not yet readable.

gbaa has run on Linux, Windows and macOS, and you bring your own ROM.

## What runs

Pokémon Emerald passes its checks. Emerald Rogue and Emerald Imperium, two
fan-made versions of it, have been played recompiled on a desktop and on a
Steam Deck. A recompiled game opens a window and plays with a controller or
keyboard. It keeps the game's own saves, and adds save states, rewind and
fast-forward.

Timing is an estimate, not cycle-accurate, so a game that depends on exact
timing would break. Games other than the ones named may run, or may stop at
something the runtime doesn't model.

No ROM is in this repo, and neither is the Rust generated from a commercial
game. You generate it from your own ROM.

## How it works

Each ARM instruction is defined once, as a Rust function generic over a
trait. That one definition is used as the interpreter, as the front of the
recompiler, and as the analysis that finds jump tables, so the three can't
drift apart.

gbaa finds a game's code by following branches from the entry point, and
writes one Rust function for each function of the game. Code it fails to
find runs in an interpreter built into the runtime, so a miss costs speed
and not correctness.

## How it's checked

A recompiled game that mostly works proves little, because a wrong
translation usually still boots and draws something. So a game is checked by
a program, against mGBA. A check is a scripted run of button presses. At
chosen frames along it, mGBA, gbaa's interpreter and the recompiled game must
all draw the same picture.

The repo has 46 of these runs with 91 checked frames, 87 of them compared
with mGBA. At 16 of those, gbaa is held to a different picture from mGBA's,
with the reason written down. Passing is a claim about those frames on that
input, not about every frame of every game.

Smaller checks sit underneath: 2.2 million single-instruction cases, a set
of test ROMs, and jump-table recovery scored against the
[pokeemerald](https://github.com/pret/pokeemerald) decompilation. The
results, and exactly what each check does and doesn't compare, are in
[docs/in-depth.md](docs/in-depth.md).

## The decompiler

A recompiled game still runs on an emulated machine: its code moves values
between the GBA's registers. The decompiler drops that machine and leaves an
ordinary Rust program, so that the game can be modded by editing Rust. (The
repo calls this lifting, and its result a native port.) It is at an early
stage. The first 2,700 frames of Metroid: Zero Mission match the recompiled
game in every frame's picture, memory and sound, but the code is not yet
something you would want to read or edit.

## Who wrote it

Claude agents wrote the code. The work is cut into units, each with its own
check, and a unit is done only when its check passes. An agent may not edit
a test or an expected result to get there; changing one takes june. The
rules are in [docs/factory.md](docs/factory.md).

june made the design decisions, which are written down with their
alternatives in [docs/decisions.md](docs/decisions.md), and played the games
at the points where a person had to look or listen.

## Try it

It has run on Linux, Windows and macOS. You need the Rust toolchain, the command
runner [`just`](https://github.com/casey/just), cmake and a C compiler, and
your own ROM.

```sh
# play a ROM in the interpreter
cargo run --release -p desktop --bin gbaa-desktop -- path/to/game.gba

# recompile it to out/<name>/, then build and run the result
just recompile path/to/game.gba
just run-recompiled out/<name>
```

`<name>` is the ROM's file name in lower case with dashes.
`just run-recompiled` builds before it runs. It needs 7 GB of RAM available
and a systemd user session, which it uses to cap memory.

To run the smaller checks, with no ROM of your own:

```sh
just fetch-tests   # clones the test suites into ~/.local/share/gbaa
just sst           # the single-instruction suite
just gba-tests     # the test ROMs
```

## Also in the repo

- Linked play over the internet, for games that joined two consoles with a
  cable.
- Lua scripting that runs scripts written for mGBA.
- Extracting a game's graphics and music to files.

## More

- [docs/in-depth.md](docs/in-depth.md): what is checked, how the recompiler
  works, and the decompiler (native ports), in detail.
- [docs/tools/commands.md](docs/tools/commands.md): every command.
- [docs/decisions.md](docs/decisions.md): each design decision, its
  alternatives and its cost.
- [docs/risks.md](docs/risks.md): what could force a design change.
- [docs/credits.md](docs/credits.md): the projects this is built on.
- [LICENSE](LICENSE): MIT.

Claude wrote this README from june's notes and decisions.
