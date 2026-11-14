# Memory index

One line per file: a link and a few words. The fact lives in the file, not here.
Made from each file's frontmatter (`title`, `hook`, `open: true`) by `just memory`; don't edit it by hand.

- [Cross-building for Windows and running under Wine in podman](windows-wine.md) — SDL3 needs CMake's MinGW toolchain file; Wine without wine-common stalls 5 min at first boot (2026-10-24)
- [Debugging Emerald against mGBA](emerald-debugging.md) — symbols from the pokeemerald ELF, a stepping harness, and timeline frames
- [Emerald battles by RAM pokes, and why a move doesn't animate](emerald-battle-pokes.md) — a foreign OT above level 10 ignores orders (set 0x86E); moves that fail or miss skip their animation
- [Emerald Rogue and Imperium against mGBA](emerald-hacks.md) — timelines, menus that A doesn't leave, where ours differed, and Rogue's main loop (2026-10-23)
- [Emerald's function-address flow: the memory fix (Ref tokens) and speed-ups](fnref-flow-emerald-history.md) — a large read is one Ref token; five slow spots; don't absorb to the object end
- [Finding where a native program and its play build part in time](native-check-divergence.md) — diff the records frame by frame in Python, then trace each sync's clock in both builds (a temporary `extern crate std` in runtime)
- [Generated-crate builds that hang or outlive their job](generated-crate-gotchas.md) — a name error can look like a 30-minute build; `just long stop` leaves rustc holding the lock; SLP crash on p37
- [Kirby & the Amazing Mirror: decomp, player struct, scripts](kirby-amazing-mirror.md) — katam matches our ROM; gKirbys at 0x02020EE0; Lua keysRead drives Kirby once stateFn = 0x0803FF65
- [mGBA's blending quirks (windows and sprites)](mgba-blend-quirks.md) — with the OBJ window on, mGBA brightens non-target sprites; semi sprites ignore the window's effect bit
- [pokeemerald's DWARF (locals, types, lines) through gdb](pokeemerald-dwarf.md) — binutils 15.2 rejects agbcc's .debug_abbrev; Fedora gdb reads it (info scope, ptype, info line) (2026-10-26)
- [SDL3's static build, and testing a window under gamescope](sdl-build-and-gamescope.md) — spotting SDL's X11 driver; gamescope in a lane needs WAYLAND_DISPLAY unset; cu type can overtake cu key
- [Timing a hand-patched generated build, and testing a value](variant-builds-and-mirror-runs.md) — patch call sites, not call!; a copied crate needs its own --target-dir; use a positive control
- [What a whole lift of each corpus game costs, and the holes-only one](lift-cost-corpus.md) — whole lifts 54 s-50 min at under 3 GB (not 16G: asking 16G blocks other jobs); holes-only 9 s-3.5 min under 0.6 GB (2026-11-14)
- [Where a generated crate's build time goes](generated-build-cost.md) — LLVM, not rustc's front end: p07 checks in 4.7 s of a 60 s build; how to measure one part (2026-10-24)

## Open

Unsettled investigations, one per file with `open: true` and the next step in its hook.
Drop the flag in the session that settles it.

- [Why Emerald's function-address flow doesn't settle yet](fnref-flow-emerald-memory.md) — next: print what 12k entries gain a round with memory still; suspect ROM pointers stepping down
