---
title: Where mGBA's beam is at the hand-over, and why ours differs
hook: mGBA hands over on line 126, H-blank ~91 cycles off (ours: line 0); moving ours alone puts Metroid a frame off (2026-11-18)
open: true
---

Found on the long-fill frame offset (oracle/long-fill-frame-offset). Next:
match mGBA's start and its BIOS calls' cycle costs together (the unit filed
for it); either alone breaks something.

**mGBA's start.** `mgba-dump` loads no BIOS and doesn't skip it, so mGBA's
HLE BIOS reset code runs (`src/gba/hle-bios.s`, `resetBase`: six
instructions and a ROM `ldrb`). `GBAVideoReset` (`src/gba/video.c`) with no
full BIOS sets vcount 0x7E (126, "TODO: Verify exact scanline on
hardware") and schedules H-blank in 117 cycles. Measured at the cartridge's
entry (timer 0 started, VCOUNT polled, 16 ROMs each with one more Thumb
`nop`, fingerprinting the poll's phase): ours matches mGBA's pattern with
the beam on line 126 and H-blank due **91** cycles after the hand-over (92
gives the same; 90 doesn't), so the reset code takes about 26 cycles. Ours
starts on line 0 (`Sched::default`, `Io::new`; the native `Hw::new` too).
So mGBA's first frame is about 41k cycles, ours a full 197k to V-blank.

**What shows it.** A program whose work before its first V-blank wait runs
past 41k cycles (c/template with a full-screen fill_rect: 618027 cycles,
identical in both, measured with cascaded timers) reaches its first wait in
a different frame: ours a frame ahead from then on. With line 126 / 91 it
matches mGBA at frame 600.

**Why it wasn't changed.** With the beam moved alone, Metroid's exact
interpreter parts from mGBA from frame 28 (the intro's fade a step off)
to 2700, where with line 0 it differs only in frames 167-1591 and matches
its pinned checkpoints. Likely: our BIOS calls (CpuSet,
RegisterRamReset…) charge more cycles than mGBA's HLE ones, so Metroid's
boot crosses mGBA's early first V-blank only in ours; line 0 was hiding
that. Unchecked. The move also fails 27 runtime tests that assume line 0
(`hw`, `sched`, `dma`, `bus::io_masks`, `snap`): give them a line-0 helper
(`Hw::set_beam(0, false)` exists) rather than rewriting each.

A `Machine::new` that builds into a local and then sets fields overflows
debug test stacks (the big struct is copied): set the start in
`Sched::default` and `Io::new` instead.
