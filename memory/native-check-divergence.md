---
title: Finding where a native program and its play build part in time
hook: diff the records frame by frame in Python, then trace each sync's clock in both builds (a temporary `extern crate std` in runtime)
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
