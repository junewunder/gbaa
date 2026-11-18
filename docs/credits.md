# Credits

gbaa is built on other people's work, and most of it was done for free.
This is what we used, roughly in order of how much we leaned on it. The
full reading list is in [docs/references.md](references.md), and what we took from each is in
the notes under [docs/research/](research/).

## What we check against

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

## Decompilations

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

## Games we ran

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

## Lua scripts we made run

The scripting host was checked by running the community's scripts in it. The survey, with commits, is in
[docs/research/lua-scripts-survey.md](research/lua-scripts-survey.md):
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

## Libraries

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

## Prior art we learned from

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

## Tools

The code was written by Claude (Anthropic) through Claude Code.

If we used your work and you aren't here, that's our mistake. Please
open an issue and we'll add you.
