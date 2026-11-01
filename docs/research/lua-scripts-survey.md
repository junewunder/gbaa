# What the community's Lua scripts do today

A survey for the scripting group (scripting/survey-scripts), asked by june
on 2026-10-24: what can the scripts people already use actually do, and
could our build run them? It starts from Real96/PokeLua and the Ironmon
tracker (already read in `lua-scripting.md`) and follows what that
community points to: RNG tools, viewers, trackers, randomizer and
multiworld helpers, speedrun and TAS tools, overlays, stream tools, and
the newer AI-agent and co-op bridges.

Surveyed 2026-10-24 on quack. Each repo was shallow-cloned and its `.lua`
files grepped for every emulator API call (the counts below), and the
call sites that matter were read. Two PokeLua scripts were run on Emerald
through the probe in `lua-probe/` (below). Commits:

| Repo | Commit (date) |
|---|---|
| Real96/PokeLua | `2fedf75` (2026-10-15) |
| Wi-Fi-Labs/PokeRNG-LuaScripts | `44aeee6` (2026-10-16) |
| besteon/Ironmon-Tracker | `41e6711` (2026-09-09) |
| ArchipelagoMW/Archipelago | `fcff008` (2026-10-23) |
| nikouu/mGBA-http | `873c411` (2026-08-17) |
| dmang-dev/mcp-mgba | `1c43ccf` (2026-10-23) |
| Gianni1707/FireRedCoop | `4728b74` (2026-10-18) |
| Code-Hdez/pokemon-firered-llm-mod | `228ed4b` (2026-02-25) |
| Clad3815/gpt-play-pokemon-firered | `9619b10` (2026-02-15) |
| Jengolus/ramanimator | `e966e3c` (2026-10-10) |
| rkevrellan/fe7-mgba-lua | `3fa179b` (2026-06-27) |
| mkdasher/PokemonBizhawkLua | `5a2be67` (2023-08-06) |
| yling/yPokeStats | `3c3e7fc` (2025-06-05) |
| EverOddish/PokeStreamer-Tools | `285032e` (2022-01-03, retired) |
| authorblues/bizhawk-shuffler-2 | `c0440b8` (2026-09-19) |
| alexjpaz-twitch/bizhawk-crowd-shuffler | current `main` |
| trysdyn/bizhawk-speedrun-lua | `55b9779` (2026-08-08) |
| ThunderAxe31/DankBot | current `main` |
| 40Cakes/pokebot-gen3 | `7f3d6be` (2026-10-24) |

## Summary

- **The RNG community's mGBA scripts need mGBA 0.11-dev, not the
  release.** PokeLua's README says "use latest development build". Its
  Gen 3 scripts call `input:isKeyActive` (117 sites: Shift+START to start
  a bot, number keys to save and load states), a write watchpoint (the TID
  bot) and two breakpoints in each held-item script. None of these exist
  in 0.10.5; there `input` is nil and the scripts stop with an error. This
  corrects `lua-scripting.md`, which says scripts in the wild don't use
  0.11-dev's additions: the trackers and bridges don't, the RNG tools do.
- **Almost everything else is the 0.10.5 surface the lua-host unit
  already plans:** memory reads (by far the most calls), memory writes,
  `frame` and `keysRead` callbacks, keys, console and text buffers, save
  states, `io` files. A host with that runs the trackers, viewers, cheat
  and RNG viewers (minus their hotkeys and hooks) and, with sockets, every
  bridge in the survey.
- **Sockets are the second biggest need.** Six of the ten mGBA projects
  whose code was read open a TCP socket: HTTP and MCP bridges, the AI agents that
  play FireRed, a two-player co-op mod and RAManimator's Aseprite link.
  This is the category growing fastest in 2026.
- **Breakpoints in the wild are mid-function, set at load and never
  cleared.** PokeLua's held-item scripts break at `CreateBoxMon+0x25a`
  and `SetWildMonHeldItem+0x1e` in Emerald. A check at block starts would
  not see them; "run interpreted while one is set" means the whole
  session runs interpreted, which on quack is still 3.7× real time.
- **Its watchpoint is the kind a once-a-frame check can serve.** The TID
  bot's write watchpoint only sets a flag that its `frame` callback reads.
  The same bot exists in all three dialects, each with its write hook
  (`setWatchpoint`, BizHawk's `event.onmemorywrite`, VBA-rr's
  `memory.registerwrite`): write hooks are a standing RNG-tool need.
- **ROM writes happen, and patch code.** RAManimator writes Thumb
  instructions into `cart0` at load to turn off the game's own sprite
  animations. Compiled code wouldn't see them. `lua-scripting.md` called
  ROM writes rare; this is the first real one.
- **BizHawk matters for one thing: Archipelago.** Every GBA multiworld
  game in Archipelago (Pokémon Emerald, Castlevania: Circle of the Moon,
  Mario & Luigi: Superstar Saga, Yu-Gi-Oh! 2006, Mega Man Battle Network
  3) connects only through a BizHawk Lua connector. The generic connector
  is small (13 distinct BizHawk calls) but needs LuaSocket's native
  module. Other BizHawk scripts draw overlays and WinForms dialogs, which
  a shim can't give them. A narrow shim aimed at Archipelago is worth
  having later; full BizHawk compatibility isn't.
- **The VBA-rr and DeSmuME dialect is the past.** Its Gen 3 scripts
  (Kaphotics-era RNG scripts, yPokeStats, PokeStreamer-Tools) have been
  ported to mGBA or BizHawk, or retired. Not worth a shim.

## The scripts

"Our build" says what running it would take: **host** is the planned
lua-host unit alone (mGBA 0.10.5's API); a later unit's name means that
unit too; **no** says why not. Today our build runs no script at all.

| Script | What it does for a player | Emulator, dialect | API it uses (counted by grep) | Our build |
|---|---|---|---|---|
| **PokeLua** Gen 3 `E/FRLG/RS_RNG_mGBA.lua` (Real96; also copied into Wi-Fi-Labs/PokeRNG-LuaScripts) | RNG manipulation: shows the initial and current seed, advances, the wild, gift, egg and roamer Pokémon the next frame would give (PID, nature, IVs, shininess), trainer IDs; a TID bot that resets until a wanted ID; an initial-seed bot (FRLG) | mGBA **0.11-dev** | `emu:read8/16/32` (214), `input:isKeyActive` (117), `console:createBuffer` (7 buffers), `callbacks:add` `frame`, `reset`; `emu:reset`, `getKey/addKey/clearKey`, `currentFrame`, `saveStateSlot/loadStateSlot`, `saveStateBuffer/loadStateBuffer`; `emu:setWatchpoint` (TID bot); `io.open` and `os.execute("mkdir states")` for per-slot seed notes | Viewer: **host**, shown by the probe. Hotkeys and bots: needs `input` (0.11-dev; not in any unit's plan yet) and a write watch (breakpoints unit, or a once-a-frame check). Its state notes write files: sandbox unit |
| **PokeLua** `Side/*_Held_Item_RNG_mGBA.lua` | Predicts a wild Pokémon's held item | mGBA **0.11-dev** | as above, plus `emu:setBreakpoint` at two ROM addresses | Needs breakpoints: the whole point of the script. Interpreted only |
| PokeRNG-LuaScripts Gen 3 `VBA/*_RNG_2.0.lua`, `BizHawk/*_RNG_BizHawk_SM.lua` | The same RNG viewers and bots, older versions | VBA-rr (Lua 5.1); BizHawk | VBA: `gui.text` (106), `gui.line`, `input.get`, `joypad.get/set`, `savestate.create/save/load`, `memory.registerwrite`, `emu.frameadvance` loop. BizHawk: the same with `gui.drawBox`, `userdata`, `event.onmemorywrite`, `event.onloadstate`, `client.reboot_core` | **No**: draws on the game picture, uses write hooks, other dialects. The mGBA versions replace them |
| **Ironmon tracker** v9.4.0 (besteon) | The IronMON challenge's companion: party, enemy and move info, stat predictions, notes, new-run setup with the randomizer, Twitch requests through Streamer.bot | mGBA 0.10+ and BizHawk, one codebase (`Main.IsOnBizhawk()`, 99 sites) | mGBA side: see `lua-scripting.md` (memory, keys, states, `loadFile`, 7 callbacks, 20 buffers, `io`, `os.execute`). Twitch link: JSON request and response files, `io` only. BizHawk side: `gui.drawRectangle` (185), `drawLine` (164), `drawPixel`, `drawText`, `forms.*` dialogs | mGBA side: **host**, shown by the probe; new runs need the randomized-ROM unit; `curl` and the randomizer jar need the sandbox's full-access switch |
| mGBA's `pokemon.lua` | Party viewer | mGBA 0.10 and 0.11-dev (each its own game-code format) | memory, `readRange`, `checksum`, `getGameCode`, `frame`, buffers | **host**, shown by the probe |
| **mGBA-http** (nikouu) | Lets any program drive mGBA over HTTP: bots, overlays, tools | mGBA 0.10+ | nearly all of `emu`, including `loadFile`, `step`, registers; `socket.bind`; `frame` | **host** + sockets. `loadFile` of another ROM: randomized-ROM unit or refused |
| **mcp-mgba** (dmang-dev) | Lets an AI assistant play and inspect a game (MCP server) | mGBA 0.10+ | `readRange`, `write8/16/32`, `setKeys`, states, `screenshot`, `pause/unpause`, `step`, `runFrame`, `frameAdvance`; `socket.tcp`; a bundled `json.lua` via `require` | **host** + sockets; `require` of a file beside the script must work |
| **FireRedCoop** (Gianni1707) | Two-player co-op FireRed: see the partner on your map, battle and trade over a "virtual cable", double battles together | mGBA 0.10+, on a ROM built from the pokefirered decomp | `emu:read*` (108), `write*` (23), `frame` and `keysRead`, `setKeys`, `screenshot`, `socket.bind/connect`; a mailbox in RAM polled each frame; `setWatchpoint` only in a test driver | **host** + sockets, on a build recompiled from the modded ROM (it is its own ROM, not FireRed) |
| pokemon-firered-llm-mod (Code-Hdez) | Sends NPC dialogue to a language model and writes the reply back into the game | mGBA 0.10+ | memory, `currentFrame`, `frame`, `socket.connect` | **host** + sockets |
| gpt-play-pokemon-firered (Clad3815) | An AI agent plays FireRed live | mGBA 0.10+ | memory, keys, `screenshot`, `saveStateFile`, `reset`, `socket.bind` | **host** + sockets |
| **RAManimator** (Jengolus) | Animated battle sprites in Gen 1-3 Pokémon and hacks, drawn with Aseprite | mGBA 0.10.5+ (Lua 5.3+ required) | `emu.memory.*` searches, `readRange`, `write8/16` (sprite data into RAM and VRAM), palette, `checksum`, `frame`, `start`; **`emu.memory.cart0:write8` of Thumb code at load** (turns off the game's own animation code); `socket.bind` for a live link to Aseprite; ~40 `require`d modules | **host** for the data side; its ROM code patches need a compiled build to notice changed code (the randomized-ROM mechanism) or the interpreter; Aseprite link needs sockets. Read, not run: running it was refused as too much external code |
| fe7-mgba-lua (rkevrellan) | Cheats for Fire Emblem 7 (EXP, stats) | mGBA 0.10.5 | `read`, `write`, `frame`, `console:log` | **host** |
| Lockeweb-Companion (vngnzz) | Nuzlocke web companion: team, encounters, Showdown export, fed live from the game | mGBA, Lua sender + local server | its Lua senders aren't in the public repo | Unknown; by its README, memory reads and a local server |
| **Archipelago** `connector_bizhawk_generic.lua` + the Emerald client | Multiworld randomizer: items found in your game unlock items in others', with death link and trades; also the tracker | **BizHawk** 2.7+ (BizHawk's GBA core is mGBA) | 13 calls: `memory.read/write_bytes_as_array`, `getmemorydomainsize`, `event.onframeend`, `event.onexit`, `emu.frameadvance` loop, `emu.getsystemid`, `gui.addmessage`, `client.getversion/getconfig`; `require("socket")`: LuaSocket's native module, shipped as a DLL. Domains `System Bus` and `ROM`. The ROM is Emerald with a base patch and per-seed data | **No** as it is (BizHawk dialect, LuaSocket). A BizHawk shim over the host with a LuaSocket-shaped `socket` would cover it; the patched ROM needs the randomized-ROM unit or its own recompile |
| Archipelago `connector_mmbn3.lua` | The same for Mega Man Battle Network 3 | BizHawk | `memory.read/write_u8/u16/u32_le` and `s16`, `gui.addmessage`, `gui.text`, `socket.bind` | Same as above |
| **PokemonBizhawkLua** (MKDasher; Ironmon's ancestor) | Gen 3 battle and encounter info drawn beside the game; RNG view | BizHawk | `gui.drawRectangle` (34), `drawText`, `drawImage`, `drawPolygon`, `forms.*`, `client.SetGameExtraPadding`, `input.getmouse`, `memory.read_u*` | **No**: it is a drawn panel and WinForms; would need canvas plus a forms toolkit |
| yPokeStats (yling) | Party and enemy stats, IVs and EVs on screen, Gen 1-5 | VBA-rr, DeSmuME (author: not mGBA, "the GUI functions of mGBA didn't allow me to display the information") | `gui.text` (41), `gui.box`, `memory.readdword` (39), `readbyterange`, `bit.*`, `input.get` | **No**: draws on screen; VBA dialect. With canvas and a VBA shim it could |
| PokeStreamer-Tools (EverOddish, retired 2022 for PokeLink) | Updates a stream layout's party sprites when the party changes | VBA-rr (Gen 3) | `memory.readword/dword`, `gui.line`, **`os.execute` (21): copies sprite files** | **No** (VBA dialect, retired) |
| bizhawk-shuffler-2 (authorblues) | Stream challenge: swaps between games when you take damage; plugins per game | BizHawk | `memory.read_u8` (141) and the like, `forms.*` (setup window), `client.openrom`, `savestate`, `io`, `os.execute` | **No**: switches ROMs, which a recompiled build can't, and is BizHawk-wide |
| bizhawk-crowd-shuffler (alexjpaz) | Twitch chat picks the next game | BizHawk + a Node helper | `comm.socketServer*`, `client.openrom`, `savestate`, `userdata` | **No**: switching ROMs |
| bizhawk-speedrun-lua (trysdyn) | Autosplitters and run monitors (one GBA game: FF V's Four Job Fiesta) | BizHawk | `memory.readbyte`, `gui.drawText`, `event.onmemorywrite` (the GB splitter), `io`, `console.log` | **No** as it is; what it does (watch RAM, write split files) the host does in mGBA dialect |
| DankBot (ThunderAxe31) | TAS helper: searches RNG outcomes by replaying from savestates | BizHawk (its route is a GBC game) | `memorysavestate.*`, `savestate`, `joypad.set`, `movie.*`, `emu.totalexecutedcycles` | **No**: needs BizHawk's movie and core-state API; TASing is BizHawk's job (decision: long routes use an existing TAS) |
| pokebot-gen3 (40Cakes), emerald-shiny-hunter | Shiny-hunting and botting | Python on libmgba (they embed mGBA; no Lua) | none | **No**: they are their own emulator front ends |

## The probe runs

`docs/research/lua-probe` (from research/lua-scripting) stubs mGBA's
globals over our runtime's interpreter and counts every call it lacks.
Built and run on quack, from the title-screen state
`~/ROMs/gba/Pokemon - Emerald Version.ss0`, scratch directory as the
working directory:

```
lua-probe "Pokemon - Emerald Version.gba" --state "Pokemon - Emerald Version.ss0" \
  --frames 900 "PokeLua/Gen 3/mGBA/E_RNG_mGBA.lua"
```

**`E_RNG_mGBA.lua`**, 900 frames: loaded, called `emu:reset()` as its
first act, found "Version: Emerald, Language: EUR/USA", made its 7
buffers and filled them each frame (seed 0000, current seed and advances
901 after 900 frames: Emerald advances its RNG once a frame; the gift
mode's Bulbasaur preview). Calls: `read8` 6,302, `read16` 25,200,
`read32` 46,805, `getKeys` 900. Missing: `emu.setWatchpoint` once (at
load) and `input.isKeyActive` 17,100 times (19 a frame). No callback
errors, because the probe's stubs return nil; in mGBA 0.10.5 the same
script fails at load on `setWatchpoint`, and `input` would be a nil
global. Lua cost 38 ms over 900 frames, 0.04 ms a frame.

**`Side/E_Held_Item_RNG_mGBA.lua`**, 600 frames: loaded, read the game,
filled its 2 buffers. Missing: `emu.setBreakpoint` twice (at load) and
`input.isKeyActive` 10,800 times. Without the breakpoints it shows the
seed but never the held item, its purpose. Lua cost 0.02 ms a frame.

The breakpoint addresses, looked up in `pokeemerald.sym`
(`$GBAA_DATA/symbols`): `0x08067E16` is `CreateBoxMon+0x25a`, after its
IV random calls; `0x0806EA86` is `SetWildMonHeldItem+0x1e`. Both are
inside functions.

RAManimator wasn't run: the session's permission check refused it as
too much external code (some 40 modules and a socket server). Its calls
were read instead.

mGBA itself wasn't run: the installed `mgba-qt` is 0.10.5, which has none
of the calls PokeLua needs and no command-line script option, so it could
only confirm the failure the source already shows.

## What it means for the scripting group

**Which API surface covers most scripts.** Ranked by how many surveyed
mGBA projects use each part:

Out of the ten mGBA projects whose code was read:

1. Memory reads (all 10), `frame` callback (all), console and buffers
   (all): the lua-host unit.
2. Memory writes (7), keys (7), save states (7), and loading Lua files
   beside the script with `require` or `dofile` (4: Ironmon, mcp-mgba,
   the LLM mod, RAManimator): the lua-host unit. The file loading should
   be in its check, since those four break without it.
3. Sockets (6): the sockets unit. It serves every bridge, the AI agents,
   co-op and the Aseprite link, so it should come right after the host.
4. `io` files (3) and `os.execute` (2): the sandbox unit's question.
5. 0.11-dev's `input` (1 project, but it is the RNG community's main
   one): not in any unit yet. It is keyboard state the window already
   has, so it is cheap; filed as its own unit
   (scripting/input-keys).
6. Breakpoints (1) and watchpoints (1, plus a test driver): the
   breakpoints unit.
7. ROM writes (1, code): the randomized-ROM unit's mechanism.
8. Drawing (`canvas`): none of the mGBA scripts, because the release has
   none; every BizHawk and VBA viewer draws. If scripts move to 0.11's
   canvas once it is released, this rises.

**Is BizHawk compatibility worth having?** Not in general: what BizHawk
scripts mostly do that ours can't is draw panels and WinForms dialogs,
and switch ROMs. One narrow shim is worth having later: enough of
BizHawk's `memory`, `event.onframeend`, `emu.frameadvance` (a coroutine
the host resumes each frame), `gui.addmessage`, `client.getversion` and a
LuaSocket-shaped `socket`, to run Archipelago's generic connector, which
is the only way any GBA game joins an Archipelago multiworld. That waits
on the host, sockets and the randomized-ROM unit (an Archipelago ROM is a
patched and per-seed one). Not filed: it is new scope for june to want or
not.

**The three open questions, in this light** (their defaults unchanged):

- *How much may a script do.* Of the surveyed mGBA scripts, three use
  `io` (PokeLua's seed notes, Ironmon's settings and Twitch files,
  FireRedCoop's error log) and two `os.execute`: `mkdir` (PokeLua) and
  Ironmon's `curl`, folder listings, the randomizer jar and URLs. Four
  load Lua files from beside themselves, which a sandbox must still
  allow. Files inside the script's own folder
  cover nearly all the `io`; `mkdir` there is safe to allow. The rest
  is Ironmon, as before. The default, sandboxed with a per-script switch,
  fits what the survey found.
- *Breakpoints and watchpoints.* They are no longer theoretical: the
  RNG community's scripts use them, on mGBA's dev builds. What they
  need: breakpoints at mid-function addresses for a whole session, and a
  write watch whose callback only sets a flag. So "run interpreted while
  one is set" fits the breakpoints (a whole session interpreted, 3.7×
  real time on quack); a once-a-frame changed-value check would serve
  this watchpoint for free, missing only a write of the same value (1 in
  65,536 for a TID). Block-start checks in compiled code would miss
  both breakpoints.
- *Randomized ROMs.* Three more cases than Ironmon need a ROM that
  differs from the one recompiled: Archipelago (a base code patch plus
  per-seed data), FireRedCoop (a decomp-built ROM, better recompiled as
  its own game) and RAManimator (code patched by the script at load).
  The default, keep compiled what is unchanged and interpret the rest,
  is the one mechanism that serves all four; RAManimator adds that ROM
  writes from a script should go through it too (a patched function
  falls back to the interpreter).

## Sources

- PokeLua: <https://github.com/Real96/PokeLua>; PokeRNG-LuaScripts:
  <https://github.com/Wi-Fi-Labs/PokeRNG-LuaScripts>.
- Ironmon tracker: <https://github.com/besteon/Ironmon-Tracker>.
- Archipelago: <https://github.com/ArchipelagoMW/Archipelago>
  (`data/lua/`, `worlds/_bizhawk/`, `worlds/pokemon_emerald/`); its
  Emerald setup guide: <https://archipelago.gg/tutorial/Pokemon%20Emerald/setup_en>.
- mGBA-http: <https://github.com/nikouu/mGBA-http>; mcp-mgba:
  <https://github.com/dmang-dev/mcp-mgba>.
- FireRedCoop: <https://github.com/Gianni1707/FireRedCoop>;
  pokemon-firered-llm-mod: <https://github.com/Code-Hdez/pokemon-firered-llm-mod>;
  gpt-play-pokemon-firered: <https://github.com/Clad3815/gpt-play-pokemon-firered>.
- RAManimator: <https://github.com/Jengolus/ramanimator>;
  fe7-mgba-lua: <https://github.com/rkevrellan/fe7-mgba-lua>;
  Lockeweb-Companion: <https://github.com/vngnzz/Lockeweb-Companion>.
- PokemonBizhawkLua: <https://github.com/mkdasher/PokemonBizhawkLua>;
  yPokeStats: <https://github.com/yling/yPokeStats>;
  PokeStreamer-Tools: <https://github.com/EverOddish/PokeStreamer-Tools>.
- bizhawk-shuffler-2: <https://github.com/authorblues/bizhawk-shuffler-2>;
  bizhawk-crowd-shuffler: <https://github.com/alexjpaz-twitch/bizhawk-crowd-shuffler>;
  bizhawk-speedrun-lua: <https://github.com/trysdyn/bizhawk-speedrun-lua>;
  DankBot: <https://github.com/ThunderAxe31/DankBot>.
- pokebot-gen3: <https://github.com/40Cakes/pokebot-gen3>;
  emerald-shiny-hunter: <https://github.com/realgarit/emerald-shiny-hunter>.
