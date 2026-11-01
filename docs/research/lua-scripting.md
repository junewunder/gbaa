# Lua scripting, mGBA-compatible

Research for the Lua scripting unit (research/lua-scripting). june wants
scripting like mGBA's, ideally running existing mGBA Lua scripts unchanged.
This covers mGBA's scripting API, what popular scripts use, which parts fit a
recompiled build, a Lua runtime for Rust, sandboxing, a probe that ran real
scripts against our runtime, a recommended design and a plan of units.

Researched 2026-10-23 on quack. Pointers into other repos are
`FILE:LINE` at these commits:

| Repo | Commit |
|---|---|
| mgba-emu/mgba (`master`, 0.11.0-dev) | `fd9dc5c2cbf234365068855851107e762fc6211b` |
| mgba-emu/mgba (tag `0.10.5`, the latest release; `scripts/fetch-mgba.sh`'s pin) | `26b7884bc25a5933960f3cdcd98bac1ae14d42e2` |
| besteon/Ironmon-Tracker (v9.4.0) | `41e671124fbc1e944480adfe62fc62dac26fd5b8` |
| nikouu/mGBA-http | `873c41136e43b6923b2fea9418b5c838e77ab6e9` |

## Summary

- **Two mGBA APIs, and they disagree.** The latest release, 0.10.5, has
  `emu`, `callbacks`, `console`, `socket` and `C`. Breakpoints,
  watchpoints, `storage`, `input` and `canvas` exist only in unreleased
  0.11-dev, so most scripts in the wild don't use them (but the RNG
  community's PokeLua does: `lua-scripts-survey.md`). The two disagree on
  `emu:getGameCode()` (`AGB-BPEE` in 0.10, `BPEE` in 0.11-dev), and each
  version's own `pokemon.lua` recognises Emerald only in its own format
  (probe, below). Follow the release where they differ.
- **Compatibility is high for what scripts actually use.** The popular
  scripts read and write memory, read the ROM header, set keys, hook the
  `frame` and `keysRead` callbacks, print to the console and to text
  buffers, and save and load states. All of that maps onto our runtime at
  frame boundaries, where the CPU state is exact.
- **The probe ran two real scripts unchanged.** mGBA's own `pokemon.lua`
  party viewer and the Ironmon tracker (v9.4.0, the most popular mGBA
  script) loaded and ran 900 Emerald frames against a stub of mGBA's
  globals over our runtime, with **no call to anything the stub lacked**
  and no callback errors. Ironmon drew all 20 of its text-buffer screens.
  Its Lua side cost 0.1 ms a frame on quack (0.6% of a 16.7 ms frame).
- **What can't map, or costs:** breakpoints and per-instruction hooks
  (compiled code has no instruction boundary to stop at), watchpoints
  (a check on every guest store, in the hottest path), `emu:loadFile` of
  another ROM (a recompiled build only runs the ROM it was made from,
  which is how Ironmon starts each new run), and ROM writes. None of the
  three scripts surveyed uses breakpoints or watchpoints.
- **The interpreter is a usable fallback.** Measured on quack, Emerald runs
  ~1270 frames/s recompiled and ~220 frames/s in our interpreter (3.7×
  real time), so "run interpreted while a breakpoint is set" or "run
  interpreted on a ROM we didn't recompile" keeps full speed on a PC.
- **Runtime: mlua with PUC Lua 5.4, vendored.** Scripts written for mGBA
  use Lua 5.3+ syntax (`pokemon.lua` has 77 lines with `<<`, `>>`, `&`,
  `|`), which LuaJIT and Luau can't parse; piccolo lacks most of the
  standard library. mlua's vendored 5.4 builds in 11 s cold, adds ~470 KB
  to a stripped binary, and needs only a C compiler on Linux, macOS and
  Windows.
- **Sandboxing is a real choice.** mGBA gives scripts the whole standard
  library. Ironmon uses it: in the probe it shelled out to `curl` for its
  update check; it also runs `os.execute` to list folders, run the
  randomizer's Java jar and open URLs. A sandbox breaks those parts.
  Asked of june (below).

## mGBA's scripting API

This section describes `master` (0.11.0-dev). The release, 0.10.5, has a
subset: `emu` without breakpoints, watchpoints, sensors, `currentCycle` or
`screenshotToImage` (its `emu` is `mCore` plus `memory`; `src/core/scripting.c`
at the tag); `callbacks`, `console` and buffers, `socket`, `util`, `C`; no
`storage`, `input`, `canvas` or `image`. Where they differ, 0.10's
`getGameCode` returns `"AGB-"` and the four characters
(`src/gba/gba.c:778` at the tag); 0.11-dev returns the four characters
alone.

mGBA's scripting is engine-neutral C (`src/script/`), bound to Lua in
`src/script/engines/lua.c`. A script gets these globals (where each is set
is in brackets):

- **`emu`**, the running core (`src/core/scripting.c:1501`), a
  `mScriptCoreAdapter` wrapping `mCore` (`scripting.c:549-640`,
  `1214-1267`):
  - memory on the bus: `read8/16/32`, `write8/16/32`, `readRange`;
  - `emu.memory`, a table of domains (`bios`, `wram`, `iwram`, `io`,
    `palette`, `vram`, `oam`, `cart0`-`cart2`, and the save), each with
    `read*`/`write*`/`readRange` at an offset, `base`, `bound`, `size`,
    `name` (`src/gba/core.c:61-71`);
  - the game: `getGameTitle`, `getGameCode` (four characters, `BPEE`),
    `romSize`, `checksum(C.CHECKSUM.CRC32|MD5|SHA1)` (CRC32 returned as four
    big-endian bytes, `scripting.c:349-376`), `platform`;
  - time: `currentFrame`, `frameCycles`, `frequency`, `currentCycle`;
  - keys: `setKeys`, `addKey(s)`, `clearKey(s)`, `getKey(s)`;
  - running: `reset` (the adapter's fires the `reset` callback), `runFrame`,
    `step` (one instruction);
  - CPU: `readRegister`, `writeRegister` by name;
  - states: `saveStateBuffer`/`loadStateBuffer`, `saveStateFile`/
    `loadStateFile`, `saveStateSlot`/`loadStateSlot`, flags from
    `C.SAVESTATE`;
  - `screenshot(path)`, `screenshotToImage`;
  - `loadFile` (another ROM), `autoloadSave`, `loadSaveFile`;
  - with debuggers built in (`ENABLE_DEBUGGERS`, on by default):
    `setBreakpoint(cb, addr)`, `setWatchpoint(cb, addr, type)`,
    `setRangeWatchpoint`, `clearBreakpoint`;
  - sensors: `setRotationCallbacks`, `setSolarSensorCallback`.
- **`callbacks`**: `add(name, fn)` returns an id, `remove(id)`
  (`src/script/stdlib.c:198-208`). Names fired: `frame`, `keysRead`,
  `crashed`, `sleep`, `stop`, `savedataUpdated`, `alarm`,
  `memoryBlocksChanged` (`scripting.c:1437-1444`), `start`, `reset`,
  `shutdown` (`src/core/thread.c:262,280,412`), `rumble`, and input events
  `key`, `mouseButton`, `mouseMove`, `mouseWheel`, `gamepadButton`,
  `gamepadHat`, `trigger` (`src/script/input.c:12-20`). `keysRead` fires
  when the game reads `KEYINPUT`; a script calls `setKeys` there to
  override the pad.
- **`console`**: `log`, `warn`, `error`, `createBuffer(name)`. A text
  buffer has `print`, `clear`, `setSize`, `moveCursor`, `advance`,
  `setName`, `getX`, `getY`, `cols`, `rows` (`src/script/console.c`). The
  Qt front end shows each buffer as a tab in its scripting window. `print`
  is routed to the console too (`lua.c:440`).
- **`socket`**: TCP, `socket.bind`, `socket.connect`, `socket.tcp`; a
  socket has `listen`, `accept`, `send`, `receive`, `close`, and
  `add("received"|"error", fn)`. It is **written in Lua** over a small
  native `_socket` (`create`, `bind`, `listen`, `accept`, `connect`, `send`,
  `recv`, `select`, `close`): the prelude is `lua.c:67-196`, and a socket
  with callbacks polls itself from a `frame` callback (`lua.c:131`).
- **`storage`**: `getBucket(name)`, a table that persists across runs, with
  `reload`, `flush`, `enableAutoFlush`; `flushAll` (`src/script/storage.c`).
- **`input`** (0.11-dev): `isKeyActive`, `activeKeys`, gamepads.
- **`canvas`** and **`image`** (0.11-dev): overlay layers on the game
  picture (`newLayer`, `update`, `setPosition`), images and a painter
  (`drawRectangle`, `drawText`, fonts) (`src/script/canvas.c`,
  `src/script/image.c`).
- **`util`** (`makeBitmask`, `expandBitmask`, `newRectangle`, `newSize`),
  **`system`** (`version`, `program`, `branch`, `commit`), and **`C`**,
  the constants (`PLATFORM`, `CHECKSUM`, `SAVESTATE`, `GBA_KEY` as bit
  numbers, `WATCHPOINT_TYPE`) (`stdlib.c:210-300`).

The Lua state gets `luaL_openlibs`: every standard library, `io`, `os` and
`debug` included (`lua.c:420`). Each script runs in its own `_ENV` whose
`require` resolves next to the script (`lua.c:985-1040`). mGBA builds
against Lua 5.4 by default and accepts 5.1-5.4 or LuaJIT (`CMakeLists.txt:791`);
the 0.10 release (2022) was the first with scripting.

## What popular scripts use

| Script | API it uses (counted by grep; the probe confirmed the first two) |
|---|---|
| `res/scripts/demos/pokemon.lua` (mGBA's party viewer) | `emu:read8/16/32`, `emu:readRange`, `emu.memory.cart0`, `emu:checksum`, `emu:getGameCode`, `emu:platform`, `callbacks:add("start"/"frame")`, `console:log/error`, `console:createBuffer`, buffer `print`/`clear` |
| Ironmon tracker v9.4.0 (FireRed, LeafGreen, Ruby, Sapphire, Emerald) | `emu:read8`, `emu:write8/16/32`, `emu:getKeys`, `emu:getGameTitle`, `emu:checksum`, `emu:saveStateFile`/`loadStateFile` (crash recovery), `emu:loadFile` (next run's ROM), `emu:reset`, callbacks `frame`, `keysRead`, `start`, `reset`, `stop`, `shutdown`, `crashed`; 20 text buffers as its screens; commands typed into the scripting console as Lua calls; `io.open` (38 sites), `os.execute` (8: `curl` updates, folder listings, the randomizer jar, opening URLs) |
| mGBA-http (an HTTP bridge for other programs) | nearly all of `emu` (memory, domains, keys, registers, states, `screenshot`, `step`, `loadFile`), `socket.bind`, `listen`, `accept`, `add("received")`, `receive`, `send`, `callbacks:add("frame")` |

Others work the same way: bridges for bots and tools (mGBA-http,
mcp-mgba's loopback JSON server) need sockets; trackers and viewers need
memory reads, buffers and the frame callback. On mGBA, Ironmon is the
"lite" edition: text buffers rather than BizHawk's drawn panel, since the
released mGBA has no drawing API. Its memory layer reads only `emu:read8`
and builds 16/32-bit values itself (`ironmon_tracker/Memory.lua:33-40`).

## What maps onto a recompiled build

Our recompiled code returns to the dispatcher at the end of each frame
(`crates/runtime/src/dispatch.rs:300-317`), where `cpu.pc` and the
registers are exact, memory is current, and save states are taken
(`snap.rs:248`). mGBA fires `frame` at the same point, the start of
V-blank. So most of the API runs between frames, outside generated code.

**Works as it is:**

- Memory reads and writes through `Bus::read*`/`write*`
  (`crates/runtime/src/bus.rs:290-440`). A write into IWRAM code goes
  through the overlay guard (`overlays.wrote`), so a script patching
  copied code stays correct.
- Memory domains: our regions are mGBA's, at the same bases.
- ROM header, CRC32 (`runtime::state::crc32`), MD5/SHA1 need a small
  hash crate or code.
- Keys: `setKeys` from `keysRead` works. Ours polls the pad once a frame
  (`Platform::keys`), so `keysRead` fires once a frame, before the poll,
  rather than on each `KEYINPUT` read; a game reading keys once a frame
  (Emerald does) sees the same thing.
- Registers between frames; `emu:step` in the interpreter
  (`Machine::step`).
- States: `saveStateBuffer`/`loadStateBuffer` hand over our own format
  (scripts treat it as opaque bytes); `loadStateFile` takes ours or
  mGBA's, as `desktop::slots::load` already does.
- `reset`, `start`, `stop`, `shutdown` from the front end's events;
  `crashed` from `Machine::fault`; `savedataUpdated` from the `.sav`
  writer; `screenshot` from `desktop::write_png`.
- `console`, text buffers, `storage`, `util`, `system`, `C`: no emulator
  involved, only somewhere to show the buffers.
- `socket`: `std::net` non-blocking sockets as `_socket`, polled from the
  frame callback exactly as mGBA's prelude does.

**Works with care:**

- `emu:runFrame` called from a `frame` callback re-enters the run loop
  (mGBA warns against it too); allow it, without firing `frame` inside.
- `currentCycle` mid-frame isn't meaningful; between frames it is
  `Sched`'s clock.
- Writes to `cart0` (ROM): mGBA patches its copy; ours is
  `&'static [u8]`. Data patches would need a mutable ROM copy; code
  patches would never run in compiled code. Rare in scripts; ignore with a
  console warning.

**Can't, or costs:**

- **Breakpoints** (`setBreakpoint`): compiled code has no per-instruction
  boundary, and calls between compiled functions are direct Rust calls
  (decision 8), so the dispatcher never sees most addresses. Options: (a)
  run in the interpreter while any breakpoint is set: exact, ~5.8× slower
  than compiled (220 vs 1270 frames/s on quack), still 3.7× real time; on
  the Deck's slower cores it may be near real time and is unmeasured;
  (b) emit a check at every block start in an instrumented build: one
  load and branch per block, always paid, and a second build per game;
  (c) leave them out.
- **Watchpoints** (`setWatchpoint`, `setRangeWatchpoint`): every guest
  store goes through `Bus::write*`, inlined into generated code. A check
  there (a per-page "watched" bitmap test) costs a load and branch on every
  store, all the time, unless it is compiled in only when wanted. A cheaper
  stand-in: compare watched ranges once a frame and fire for those that
  changed (`WRITE_CHANGE` at frame granularity): free when no watch is set,
  but late and blind to values that changed back. Or (a) above: watch in
  the interpreter.
- **`emu:loadFile` of another ROM**: a recompiled build refuses any ROM
  but its own (`crates/desktop/src/lib.rs:486-493`). Ironmon's whole
  loop is "randomize the ROM, load it": a randomized Emerald keeps
  Emerald's code and changes its data tables, mostly. Options: run other
  ROMs in the interpreter (`gbaa-desktop` already does), or accept a ROM
  whose code bytes match the recompiled functions (check each function's
  bytes at load and send a changed one to the interpreter fallback); the
  randomizer's code patches would decide how much stays compiled.
  Unmeasured.
- **`canvas`/`image`**: done (scripting/canvas): the window blends the
  layers over the picture at 240×160 before scaling it, and headless
  dumps show them too (`docs/tools/commands.md`).
- Sensors (`setRotationCallbacks`, solar), `rumble`, `alarm`,
  `memoryBlocksChanged`: no game of ours uses them; stub.

## The probe

`docs/research/lua-probe/` is a small binary, outside the workspace, that
sets mGBA's globals up over our runtime's interpreter
(`Machine<Pad>`, a platform whose keys the script sets): `emu` with
memory, domains, ROM info, keys, registers, states, `screenshot`,
`runFrame`, `step`, `reset`; `callbacks`, `console` and text buffers in a
Lua prelude; `C`, `util`; and `socket`, `input`, `storage`, `canvas`,
`image` as empty tables. Anything missing answers with a function that
returns nil and is counted, so a script runs on and the count shows how far
compatibility goes. It loads the scripts as mGBA does when a game is
already running (`emu` set; `start` already past), then runs N frames,
firing `keysRead` before each and `frame` after.

```
cargo build --release --manifest-path docs/research/lua-probe/Cargo.toml
lua-probe ROM [--state FILE] [--frames N] SCRIPT...
```

`scripts/continue-emerald.lua` presses through Emerald's title screen with
`setKeys` from `keysRead` (its first test). The state was
`~/ROMs/gba/Pokemon - Emerald Version.ss0`, which is at the title
screen of a game with no save, so the party is empty in both runs.

**`pokemon.lua`**, 900 frames: every call implemented, none missing, no
errors. It found the game by CRC32 ("Found game: Emerald (USA)"), made its
`Party` buffer and read the party count each frame. Lua cost 2.8 ms over
900 frames.

**Ironmon tracker v9.4.0**, 900 frames, unchanged: loaded ("Ironmon
Tracker v9.4.0 successfully loaded"), registered `frame`, `keysRead` (2),
`start`, `reset`, `stop`, `shutdown`, `crashed`, made 20 text buffers and
filled them (settings, gameplay options, new-run setup, …), read memory
9,708 times, called `getKeys` each frame. Nothing missing, no errors. Lua
cost 91 ms over 900 frames: 0.10 ms a frame. Unasked, it ran `curl`
through `os.execute` to check for updates (the probe had mGBA's full
standard library); that is the sandboxing question below in practice.

**The two `pokemon.lua`s against the game code**, 300 frames each.
0.11-dev's finds Emerald by its game code `BPEE` (its CRC32 table has no
Emerald (USA)); 0.10.5's (`res/scripts/pokemon.lua` in the pinned
checkout) by `AGB-BPEE`. With the probe returning `BPEE`, 0.10.5's said
"Unknown game!"; returning `AGB-BPEE`, 0.11-dev's did. Nothing else
differed: no missing calls in any of the four runs. The probe now returns
0.10's format.

**mGBA-http** was read, not run: it opens a TCP server, which the probe
doesn't have, and running more downloaded code with full OS access wasn't
worth it. Its use of `socket` is the prelude's API above, so implementing
`_socket` makes it work.

The game ran in the interpreter in the probe (~310 frames/s with the
scripts); the per-frame Lua cost doesn't depend on that.

## A Lua runtime for Rust

| Runtime | Language | Fits mGBA scripts? | Notes |
|---|---|---|---|
| **mlua** 0.12.2, `lua54` + `vendored` | PUC Lua 5.4, in C | **Yes**: mGBA's default Lua | Mature, widely used. `vendored` builds Lua from source (`lua-src` crate) with `cc`: a C compiler is all it needs, on Linux, macOS and Windows (MSVC or GNU). Measured: cold build 11 s at 6 jobs; a stripped hello-world is 820 KB (~470 KB of it Lua). Memory limit (`set_memory_limit`) and instruction hooks (`set_hook`) for runaway scripts. |
| mlua, `luajit` | Lua 5.1 + extensions | **No**: no integer type or `<<`, `&`, `//` syntax; `pokemon.lua` doesn't parse | Faster, but scripts spend ~0.1 ms a frame anyway. |
| mlua, `luau` | Roblox's Luau (5.1 dialect) | **No**: same syntax gap, different library | Best sandbox, wrong language. |
| mlua, `lua55` | Lua 5.5 | Mostly | Newer than any mGBA build; 5.4 matches mGBA. |
| piccolo 0.3 | Pure-Rust Lua VM | **No, today** | Its README: "experimental", "still very much WIP", `io`, `os`, `package`, `string`, `table`, `utf8` "missing or very sparsely implemented", slower than PUC Lua. |
| hematita, purua, others | Pure-Rust | No | Toy or unmaintained. |

The Deck runs the Linux x86_64 build: Lua is linked statically, no
system library needed. macOS and Windows builds (the stretch group) need
the target's C compiler, which they need for other crates already
(`cpal`, `gilrs`). The runtime crate stays `no_std` (decision 13): Lua
lives in a new desktop-side crate, never in `runtime`.

## Sandboxing

mGBA trusts scripts fully. The choices for us:

- **Full, as mGBA**: `io`, `os`, `debug`, `package` all there. Every
  script works as in mGBA, Ironmon's updater, folder listing and
  randomizer launch included. A downloaded script can do anything the
  user can.
- **Sandboxed by default, full on request**: drop `os.execute`,
  `io.popen`, `os.remove`/`rename` outside allowed folders and `debug`;
  `io.open` only under the script's folder and a per-script data folder;
  sockets only on loopback unless allowed. A per-script "full access"
  switch, remembered, restores mGBA's behaviour. Ironmon's tracking works
  sandboxed; its updater, quickload folder listing and randomizer launch
  don't until it's given full access.
- **Always sandboxed**: simplest to reason about; Ironmon's new-run
  automation never works.

Either way the host guards against hangs: an instruction-count hook stops
a callback that runs too long (a frame's budget), with a console error,
as a frozen game is the worst failure on the Deck.

## Recommended design

- A new crate `crates/script` (std, desktop-side) on mlua with vendored
  Lua 5.4. It owns the Lua state and implements mGBA's globals over a
  `&mut Machine<P>` it is lent between frames. Like mGBA, the plain
  bookkeeping (callback list, buffers, the socket wrapper) can be Lua
  inside the crate; the emulator-facing parts are Rust.
- The front end lends the machine to the script host at fixed points:
  before the pad poll (`keysRead`), after each frame (`frame`), and on
  start, reset, stop, shutdown and a fault. The host never runs inside
  generated code, so compiled code pays nothing when no script is loaded
  and nothing per instruction when one is.
- `gbaa-desktop --script FILE` (repeatable) for the command line and the
  headless checks; loading and unloading from an in-game menu on the Deck,
  once the UI library is chosen.
- The console and text buffers shown in a side panel on the desktop and an
  overlay on the Deck (toggled by a button), with a line to type Lua into
  (Ironmon's commands are typed Lua calls; on the Deck, Steam's
  on-screen keyboard).
- Scripts written against mGBA run unchanged: same global names, same
  method names and argument orders, mGBA's constants, CRC32 big-endian.
  The target is the 0.10.5 release's API, plus 0.11-dev's additions where
  they don't clash (`storage`, `input`, `canvas`, `emu:read8` on the
  adapter); where they clash (`getGameCode`), 0.10's behaviour, until a
  0.11 release makes scripts move.
- Breakpoints, watchpoints and other ROMs per june's answers below.

## Plan of units

Filed in a new group, "Lua scripting, mGBA-compatible" (scripting), which
waits on this research:

1. **Run mGBA Lua scripts in the desktop build** (scripting/lua-host): the
   crate, `--script`, `emu` (memory, domains, ROM info, keys, registers,
   states, `screenshot`, `step`, `runFrame`, `reset`), `callbacks`
   (`frame`, `keysRead`, `start`, `reset`, `stop`, `shutdown`, `crashed`,
   `savedataUpdated`), `console` with buffers kept in memory and printed
   headless, `C`, `util`, `system`; the hang guard. Check: a fixture script
   in the repo and mGBA's `pokemon.lua` (from the pinned mGBA checkout,
   `scripts/fetch-mgba.sh`) run headless on Emerald with the expected
   output and no missing calls.
2. **Sandbox scripts** (scripting/sandbox): per june's answer.
3. **Show the script console and buffers in the window**
   (scripting/console-ui): side panel or overlay, the typed-Lua line, and
   loading scripts from the menu; needs the UI library choice.
4. **Script sockets, as mGBA's** (scripting/sockets): `_socket` over
   `std::net` and the prelude's API; check: a loopback client and server
   script talk in a headless run.
5. **Script storage buckets** (scripting/storage): `storage:getBucket`,
   persisted per bucket as JSON in the config folder's `storage/`.
6. **The Ironmon tracker on Emerald in our build** (scripting/ironmon):
   the tracker runs on the recompiled Emerald, its screens fill from a
   save with a party, and a new run loads; needs 1-5 and the answer on
   other ROMs.
7. **Script breakpoints and watchpoints** (scripting/breakpoints): per
   june's answer.
8. **A recompiled build runs a randomized copy of its ROM**
   (scripting/randomized-rom): per june's answer.
9. **Script overlays: mGBA's canvas and image API** (scripting/canvas):
   after 3.

## Questions for june

Asked with `just ask`:

- How much may a script do (full access as mGBA, sandboxed with a
  per-script full-access switch, or always sandboxed)? Default: sandboxed
  with the switch.
- Breakpoints and watchpoints: leave out, run interpreted while one is
  set, or build them into compiled code? Default: leave out for now (no
  released mGBA has them in scripts), run interpreted when wanted.
- Ironmon's new runs load a freshly randomized ROM: run other ROMs in the
  interpreter, try to keep a randomized ROM compiled where its code is
  unchanged, or not support it? Default: try to keep it compiled, the
  interpreter where it can't.

## Sources

- mGBA scripting docs: <https://mgba.io/docs/scripting.html>; the
  announcement: <https://mgba.io/2022/05/29/scripting/>.
- Ironmon tracker: <https://github.com/besteon/Ironmon-Tracker>.
- mGBA-http: <https://github.com/nikouu/mGBA-http>.
- mcp-mgba (a loopback JSON bridge in Lua):
  <https://cdn.jsdelivr.net/npm/mcp-mgba@0.3.3/README.md>.
- mlua: <https://crates.io/crates/mlua> (0.12.2); piccolo:
  <https://github.com/kyren/piccolo> (0.3.3).
