# The mod view without timing, played side by side

mod/no-timing-flag, 2026-10-30. For june: should the mod view keep mGBA's
timing model all the time, or only as the oracle for checking the decomps?

`gbaa-recompile --timing instant|static|exact` (default `instant` for a
game's build and a cut; every check passes `exact`):

| mode | what a block costs | what's gone from the code |
|---|---|---|
| **exact** | mGBA's model: fetch waits from `WAITCNT` at run time, each data access by the memory it hits, the prefetch buffer's overlap after each memory instruction | nothing |
| **static** | one constant per exit: the instructions' listed cycles at the cartridge waits games set (`WAITCNT` 3/1). Data accesses free. | the address on every `m.get`/`m.set`/`push!`/`pop!`, every `rt::stall`, `rt::fetch`, `rt::skipped`, `m.pool` |
| **instant** | every fetch as if the buffer always had it ready (no wait states): the fastest the CPU goes, like an overclock. Data accesses free. | the same as static, with smaller constants |

`instant` is the opposite end from the research's `no-prefetch` (the buffer
off, so every fetch pays the full wait, the slow case).

## The same function

`KirbyStartWaterMovement`, first lines (`out/kirby-*/parts/p08/src/m000.rs`):

```rust
// exact
push!(m, 0x0805_9810, v2.wrapping_add(0xffff_fff8), [m.cpu.r[4], m.cpu.r[14]]);
let v29: u32 = m
    .get(0x0805_9818, Ptr::<u8>::at(prim::add_with_carry(v20, 98, false).0));
if !v50 {
    rt::skipped(m, 8, true, 3, 2, 1);
}
if v50 {
    tick!(m, pc = 0x0805_982f, 14 + rt::fetch(m, 8, true, 10, 3), { … });
    m.set(0x0805_9834, Ptr::<u16>::at(v8), 59);
    m.set(0x0805_9836, Ptr::<u16>::at(r4.wrapping_add(4)), r2);
    m.set(0x0805_983c, Ptr::<u8>::at(v21), 14);
    call!(m, pc, sub_0806EC28 @ 0x0806_ec29 -> 0x0805_9845, 13 + rt::fetch(m, 8, true, 9, 4), { … });

// static (instant: the same, with 14 and 13 for 33 and 34)
push!(m, v2.wrapping_add(0xffff_fff8), [m.cpu.r[4], m.cpu.r[14]]);
let v29: u32 = m.get(Ptr::<u8>::at(prim::add_with_carry(v20, 98, false).0));
if v50 {
    tick!(m, pc = 0x0805_982f, 33, { … });
    m.set(Ptr::<u16>::at(v8), 59);
    m.set(Ptr::<u16>::at(r4.wrapping_add(4)), r2);
    m.set(Ptr::<u8>::at(v21), 14);
    call!(m, pc, sub_0806EC28 @ 0x0806_ec29 -> 0x0805_9845, 34, { … });
```

The whole game: 152,002 timing calls and 26,481 `m.pool` lines gone,
1,917,581 lines down to 1,634,996 (15% fewer). The addresses that stay
(`pc = 0x0805_982f`, `@ 0x0806_ec29`) are where the code goes next, which
any mode needs.

## What changes in play

Kirby & the Amazing Mirror, every function readable, the timing-lab's
route (4500 frames, the same input), each build against exact
(`just timing-modes`, `out/timing-modes/kirby/report.txt`):

| | exact | static | instant |
|---|---:|---:|---:|
| frames that look different | — | 136 | 1482 |
| first different frame | — | 6 (boot) | 5 (boot) |
| lag frames | 19 | 20 | 17 |
| lags | — | one more, at 260 (title) | the in-game one at 1505 gone (Kirby's jump starts on time); boot's shift a frame |
| RNG | — | differs on 64 frames between 262 and 388, then the same | differs on 1092 frames from 41 on, still at 4500 |
| helper Kirbys | — | the same | stand elsewhere from about 3700 (`instant/diff-4500.png`: the red/green/yellow trio in another order) |
| Kirby himself | — | same place every frame | his hop at 1506–1523 goes higher (the jump starts on time, so A is read once more), then the same |
| sound samples that differ | — | 46% | 86% |

- **static** behaves like the research's "zero" variant (stalls and data
  costs deleted): a little slower than real at the title, one lag frame,
  the RNG catches up. 136 frames differ, not the research's 1, because
  static also drops `rt::skipped` (a branch not taken is charged as taken)
  and prices boot's fetches at the game's `WAITCNT` before it writes it.
- **instant** runs faster than the real GBA. Kirby waits for V-blank, so most
  of that is idle time, but a lag frame the real game has goes away, the
  RNG's frame-by-frame draws shift, and the CPU-driven helpers end up
  somewhere else. june played it (2026-10-30) and couldn't tell a
  difference, except that it felt more responsive.
- The sound numbers are large because a register write landing a few
  samples earlier shifts what follows; whether you can hear it is what
  playing will tell.

When these runs were made, the interpreter ran exact in every mode (code
that resumes there, IWRAM not recompiled). Since 2026-10-31 a build's
interpreter fallback follows the build's own timing (`runtime::Timing`,
decision 11), so a rerun of `just timing-modes` won't give these numbers
exactly.

## Play them

From this worktree (each binary carries the ROM; all three share the
save in `~/.local/share/gbaa/saves/kirby-the-amazing-mirror/`):

```
just run-recompiled out/kirby-exact
just run-recompiled out/kirby-static
just run-recompiled out/kirby-instant
```

To rebuild one: `just recompile ~/ROMs/gba/"Kirby & The Amazing Mirror.gba"
out/kirby-MODE --readable mods/emerald/readable/all.list --timing MODE`,
then `just build-recompiled out/kirby-MODE` (heavy, about 5 minutes each).
`just timing-modes` reruns the comparison (writes the `diff-N.png`s in
`out/timing-modes/kirby/MODE/`).

Emerald wasn't built: three Emerald builds are about three times Kirby's
cost and have no fixed route with an RNG script yet. The flag works the
same on it.

## Is instant ever impossible on hardware?

No, on this route (a subagent's check, 2026-10-30, the same two runs
probed with a Lua script). Every difference comes from which frames lag:

- The main loop waits in `VBlankIntrWait` and then for V-blank to end, so
  game logic runs at most once a frame in both builds. The four V-blank
  upload steps never ran past V-blank (`0x030035D4` was `0xFF` every
  frame), so nothing was torn or skipped.
- Lags: exact at 1–5, 7, 39 and 1505; instant at 1–4, 6 and 38. So instant is
  one logic pass ahead from frame 7 and two ahead from 1505.
- The RNG (`0x030068D8`) draws the same sequence one frame earlier from
  41 to 1504. The helpers (`0x02020EE0 + i*0x1A8`) match exact one frame
  later through 1832–1990, and from 3546 really go their own way: two
  passes ahead, the game's own per-pass counters are out of phase.
- The sound lines up with exact's shifted by a frame (0 after 1505), and
  stays about 12 samples (0.75 ms) earlier, likely the sound timer starting
  earlier in boot (not checked).

Where instant could show more: a busy scene whose uploads overrun V-blank on
hardware (instant would lag less), the `VCOUNT` polling in boot (only around
the corrupted-save screen), and the link cable (Timer3). Not checked:
those, boss fights, and the cause of the sound offset.

## Decided: instant to play, exact to check

june, 2026-10-30, after playing instant ("it's literally perfect and I
genuinely cannot tell a difference", and more responsive): instant is the
default for every build she plays, the mod view included; exact is kept
for the checks, so mGBA stays the oracle for the decomps and the routes.
That works because the flag changes only how cycles are counted, not what
the code does. In the recompiler, a game's build and `--cut` default to
instant; `--snapshots` and `--names-check` default to exact, every check
recipe passes `--timing exact`, and so does route-check's build.

What the split costs, and june's take:

- A mod's cut is printed in its build's form: an instant cut has no
  addresses and doesn't compile into an exact build, so the unedited-cut
  check (`mod-states`) needs a cut made with `--timing exact`. Not a
  problem: a mod is a one-time fork, and the code works at that point.
- Netplay peers and recorded inputs (TAS, routes) are frame-timed: both
  sides run the same mode, and a recording made on mGBA drifts on instant.
  Play builds are all instant, so peers match.
- Another game may lean on timing Kirby doesn't (mid-frame raster
  effects, timed busy loops, a timer as RNG, slowdown it was tuned
  around): look at each in instant, and mod timing back in where one
  needs it.
