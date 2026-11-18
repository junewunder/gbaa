---
title: Where mGBA's beam is at the hand-over, and what matching it took
hook: mGBA hands over on line 126, H-blank 91 cycles off; ours does too, which needed each V-blank of a long DMA to end a frame (2026-11-18)
---

Found on the long-fill frame offset (oracle/long-fill-frame-offset), fixed
by oracle/boot-beam-and-bios-costs.

**mGBA's start.** `mgba-dump` loads no BIOS and doesn't skip it, so mGBA's
HLE BIOS reset code runs (`src/gba/hle-bios.s`, `resetBase`). `GBAVideoReset`
(`src/gba/video.c`) with no full BIOS sets vcount 0x7E (126) and schedules
H-blank in 117 cycles; measured at the cartridge's entry (16 ROMs, one more
Thumb `nop` each, fingerprinting a VCOUNT poll), H-blank is due **91**
cycles after the hand-over. Ours now starts there (`sched::START_LINE`,
`START_HBLANK_IN`, in `Sched::default`, `Io::new` and the native
`Hw::new`), so the first frame is about 41k cycles, not 197k.

**Two more mGBA details the early frame exposed.** mGBA's software renderer
starts with its own DISPCNT copy at 0x80 (forced blank) until the game
writes the register, and its output buffer starts white: line 126 is drawn
before most games write DISPCNT. Ours keeps "unwritten since power-on" in
DISPCNT's bit 3 (CGB mode, which the CPU can't write or read), cleared by
any write (`Io::write_masked`, `Video::write16`); the PPU draws it as forced
blank; `Ppu::new`'s frame is white.

**Why Metroid went a frame off (not BIOS call costs).** Metroid's first
call fills EWRAM by one immediate DMA3 of 0x10000 words (~450k cycles). Our
DMA runs whole and ticks the clock once; the sync's scanline walk then
crossed two V-blanks and set one `frame_ended`, merging two frames (mGBA
presents both, CPU stopped). With line 0 the DMA crossed only one, which
hid it. Now `Machine::sync` stops its walk at the V-blank it started and
`Machine::catch_up` (at each run's start) walks the rest, ending another
frame with no instruction run if it reaches one. With all three, Metroid's
exact interpreter matches mGBA from frame 1 to 166 with no input (167-1894
differ, as with line 0 its route differed from 167), and every route
checkpoint holds.

**Tests.** Runtime tests written for line 0 use `Machine::start_at_line_0`
(test_util's `machine_with` calls it) or `Hw::set_beam(0, false)`.
A `Machine::new` that builds into a local and then sets fields overflows
debug test stacks (the big struct is copied): set the start in the
constructors' parts instead.
