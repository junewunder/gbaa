---
title: Finding where a native program and its play build part in time
hook: diff the records frame by frame in Python, then trace each sync's clock in both builds
---

What found Metroid's six timing causes in one session (native/metroid-native-check, 2026-11-14).

- **Read the records yourself.** `route-check --native` stops at the first byte. The records
  (`out/native-check/<route>/{play,native,interp}.rec`, format in `crates/native-record/src/lib.rs`:
  magic, then a frame number and hash `u64`s, an audio flag (1: `u32` count, `u64` hash, `u8` silent),
  then six regions each `u8` changed plus its bytes) are easy to parse in a few lines of Python. Diffing
  every frame with the check's skips (the stack, the `[[copy]]` ranges, DMA and `POSTFLG` aren't
  compared) shows whether native is a frame ahead, or only the clock at the present is off (VCOUNT and
  the audio sample count differ, RAM doesn't).
- **Trace the clock.** The runtime is `no_std`, but for a throwaway build add `extern crate std;` under
  `#![no_std]` in `crates/runtime/src/lib.rs` and `std::eprintln!` in `Machine::sync` (play) and
  `hw::sync` (native): the clock at entry, the cycles, the clock after, the line, for a window of frames.
  Both builds then print the same lines until they part; `diff` shows the first sync that differs. The
  same trick in `sound::overflowed` (each pop), `timer::control_written` and `Machine::take_irq` (pc,
  `IF`) found the rest. Rebuild with `just build-recompiled out/<lift>` and `out/<play>`; revert the
  trace before committing.
- **Run play and its interpreter alone** to check a play-build change: `gbaa-<name> ROM --headless
  --frames N --input … --timing native [--interp] --native-record F`, then `cmp` the two files
  (`route-check --args ROUTE` prints the input).
- The causes were all "where time is brought up to date": an idle loop's not-taken exit, a present
  mid-walk, a BIOS stall in one piece, the sound walked after a DMA, a timer re-run by a stale event,
  an IRQ taken at the next tick. Under native timing look first at which sync an event lands in.
- **Audio only, RAM and I/O equal** (Amazing Mirror, native/mirror-native-route): hash each chunk's
  `fifo` and `psg` apart in `sound::hand_off` (native) and `Machine::hand_off_audio` (play; it doesn't
  call `hand_off`), run both with `--native-record` (else play makes no chunks). PSG alone differing
  meant a store's width: native `st8` made a byte store to a sound register a halfword, retriggering.
- **VCOUNT only, frame 1**: the native `sync` presented after its whole walk, the play build's stops at
  the V-blank (sched.rs `catch_up`, 2026-11-18). A big boot DMA overshoots by tens of lines.
- **route-check stops at the first failing check** (Emerald, native/emerald-native-route, 2026-11-23):
  when the play build's compiled run differs from its interpreter, the lift isn't compared at all, but
  all three records are written. Diff `interp.rec` against `native.rec` yourself (the Python reader
  above, skipping the stack from the lowest sp, the code copies and `KEYINPUT`, which the check
  doesn't compare) to see every cause at once. Two 1.7 GB records diff in a few minutes in plain
  Python if each region is compared in 256-byte chunks before byte by byte.
- **What Emerald showed**: an RTC the play build has and the lift lacked (rtc.c's `sErrorStatus`
  0x90, frame 10; now `Hw::gpio`); a function only data points at (tileset callbacks; the holes
  file's `[[roots]]`); bytes copied from uninitialised stack (struct padding), which differ because
  the lift never writes folded pushes to IWRAM. Name each address with the pokeemerald ELF's `nm -S`
  and the `.map` (the `.bss` of an object file when `nm` has no symbol).
