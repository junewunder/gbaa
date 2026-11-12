# Memory index

One line per file: a link and a few words. The fact lives in the file, not here.
Made from each file's frontmatter (`title`, `hook`, `open: true`) by `just memory`; don't edit it by hand.

- [Cross-building for Windows and running under Wine in podman](windows-wine.md) — SDL3 needs CMake's MinGW toolchain file; Wine without wine-common stalls 5 min at first boot (2026-10-24)
- [Debugging Emerald against mGBA](emerald-debugging.md) — symbols from the pokeemerald ELF, a stepping harness, and timeline frames
- [Emerald battles by RAM pokes, and why a move doesn't animate](emerald-battle-pokes.md) — a foreign OT above level 10 ignores orders (set 0x86E); moves that fail or miss skip their animation
- [Emerald Rogue and Imperium against mGBA](emerald-hacks.md) — timelines, menus that A doesn't leave, where ours differed, and Rogue's main loop (2026-10-23)
- [Generated-crate builds that hang or outlive their job](generated-crate-gotchas.md) — a name error can look like a 30-minute build; `just long stop` leaves rustc holding the lock; SLP crash on p37
- [Kirby & the Amazing Mirror: decomp, player struct, scripts](kirby-amazing-mirror.md) — katam matches our ROM; gKirbys at 0x02020EE0; Lua keysRead drives Kirby once stateFn = 0x0803FF65
- [mGBA's blending quirks (windows and sprites)](mgba-blend-quirks.md) — with the OBJ window on, mGBA brightens non-target sprites; semi sprites ignore the window's effect bit
- [pokeemerald's DWARF (locals, types, lines) through gdb](pokeemerald-dwarf.md) — binutils 15.2 rejects agbcc's .debug_abbrev; Fedora gdb reads it (info scope, ptype, info line) (2026-10-26)
- [SDL3's static build, and testing a window under gamescope](sdl-build-and-gamescope.md) — spotting SDL's X11 driver; gamescope in a lane needs WAYLAND_DISPLAY unset; cu type can overtake cu key
- [Timing a hand-patched generated build, and testing a value](variant-builds-and-mirror-runs.md) — patch call sites, not call!; a copied crate needs its own --target-dir; use a positive control
- [Where a generated crate's build time goes](generated-build-cost.md) — LLVM, not rustc's front end: p07 checks in 4.7 s of a 60 s build; how to measure one part (2026-10-24)

## Open

Unsettled investigations, one per file with `open: true` and the next step in its hook.
Drop the flag in the session that settles it.

- [Why the function-address flow ran out of memory on Emerald, and the Ref fix](fnref-flow-emerald-memory.md) — big reads are one Tok::Ref (1.5 GB); 2026-11-12 speed work: round 0 in 7 min, a whole run in 28 min at 2.1 GB, but unsettled; profile with eu-stack (perf is blocked)
