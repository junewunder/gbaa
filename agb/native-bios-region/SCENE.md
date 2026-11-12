# native-bios-region: what it draws

`just agb-tests` builds this into `$GBAA_DATA/agb/native-bios-region.gba`.
The stage-A lifter's check runs it (`just lift-check`, the
`native-bios-region` table of `crates/codegen/lift-check.toml`, on
`units/native-bios-region-601.toml`); what it exercises is in
`src/main.rs`'s header (unit `native/lift-bios-region`).

Mode 3, BG2 on. Each pass of the main loop `f` (from 0) draws row `f %
160`, pixels 0-10, one per slot of `OUT`: the slot's value `v` as BGR555
`(v ^ v >> 15) & 0x7FFF`. What the slots hold on the hardware, mGBA and
the play build (2026-11-12, frame 601):

| Slot | Read | Value |
|------|------|-------|
| 0 | word at `0x4`, `main`'s start | `0xE3A02004` (after a `swi`: agb's start-up makes one) |
| 1 | word at 0, after `Div` | `0xE3A02004` |
| 2 | halfword at 2, after `Div` | `0xE3A0` |
| 3 | byte at 1, after `Div` | `0x20` |
| 4 | word at `0x3FFC`, after `Div` | `0xE3A02004` |
| 5 | word at `0x8`, in the V-blank handler | `0xE25EF004` |
| 6 | word at `0x10`, after V-blank's IRQ | `0xE55EC002` |
| 7 | byte at `0x13`, after V-blank's IRQ | `0xE5` |
| 8 | `Div`'s quotient, `(f + 1000) / 7` | |
| 9 | `f` | |
| 10 | word at 0, after `Halt` woken by V-blank's IRQ | `0xE3A02004` (the IRQ returns into the BIOS's `Halt`, which then returns from the call) |

Slot 5 is 0 until the first V-blank IRQ. A pass polls to line 170, then
halts to the next V-blank (agb's `wait_for_vblank` then skips its wait),
so passes come about once a frame and a half. Our exact interpreter draws
each pass a little before mGBA (at frames 598, 600 and 602 only the last
row's slots 8 and 9 differ), so the route stops at 601, where mGBA 0.10.5
draws hash `966182bb68b4b4b0` as our interpreter does.

A lift that panics on the BIOS region (stage A before this unit) stops
at slot 0; one that reads a fixed value draws slots 5 and 6 as slot 1; one that
leaves the IRQ's latch after a `Halt` draws slot 10 as slot 6.
