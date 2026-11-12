# native-tasks: what it draws

`just agb-tests` builds this into `$GBAA_DATA/agb/native-tasks.gba`. The
native lifter's check runs it (`just lift-check`, the `native-tasks` table
of `crates/codegen/lift-check.toml`, on `units/native-tasks-600.toml`);
what it exercises is in `src/main.rs`'s header (unit
`native/task-coroutines`).

Mode 3, BG2 on, from the first frame of the loop. Each frame `f` (from 0),
after the scheduler's pass, draws each task's `out` as a pixel at
`(f % 240, f / 240 % 50 + 50 * task)`, its low 15 bits as the BGR555
colour: rows 0-2 `task_tick`, rows 50-52 `task_fib`, rows 100-102
`task_burst`.

| Frame | tick (row 0) | fib (row 50) | burst (row 100) |
|-------|--------------|--------------|-----------------|
| 0     | `0x0003`     | `0x0000`     | `0x01f5`        |
| 2     | `0x0006`     | `0x0001`     | `0x01f5`        |
| 3     | `0x0006`     | `0x0001`     | `0x09cb`        |
| 10    | `0x000f`     | `0x0005`     | `0x74e6`        |
| 16    | `0x0018`     | `0x0015`     | `0x03e9`        |
| 100   | `0x0096`     | `0x3961`     | `0x4463`        |
| 599   | `0x0384`     | `0x0f99`     | `0x3f9e`        |

`task_tick` steps by 3 after waits of 1, 2 and 3 frames in turn;
`task_fib` moves one Fibonacci number every two frames (wrapping, low 15
bits); `task_burst` starts at `generation * 100` (frame 0: 100, `0x01f5`
after its first step), steps every three frames and is started again
after five steps (frame 16: generation 2, `0x03e9`). Nothing is cleared,
so by frame 600 rows 0-2, 50-52 and 100-102 hold frames 0-599. The rest of
the frame buffer is what agb's start-up left in VRAM, the same every run.

A lift whose `task_fib` stores one more (the check's planted control,
`planted_store = 0x08000288`) draws row 50 one more from frame 0. mGBA
0.10.5 draws the frames above: every pixel of the three rows matches a
model of the scheduler, and frame 600's hash is `ec4286836efdf369`
(`just mgba-diff ROM 600`, 2026-11-12).
