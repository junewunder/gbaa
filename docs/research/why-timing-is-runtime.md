# Why GBA timing can't be baked in, shown on Kirby

research/why-timing-is-runtime, 2026-10-30. For june, for bedtime.

The question: in the readable view, every load says where it is in the ROM
(`m.get(0x0803_ee2c, …)`), and some lines are just `rt::stall(m, 8,
0x0803_ee5a, false)`. Why can't the recompiler work those costs out once,
ahead of time, and print a number? And is "exact timing ahead of time" the
halting problem?

**Short answers**

- Each instruction's cost is a small, plain function: the instruction, how
  full the prefetch buffer is (9 states), which memory the pointer hits,
  and `WAITCNT`. A table of those prices **every one of Kirby's 36.7
  million memory instructions exactly** (0 wrong). So computing the cost
  isn't the halting problem.
- But two of those inputs only exist while the game runs: **where the
  pointer points** and **how full the buffer is**, which depends on the
  path taken to get there. That's what the address in `m.get` is for: the
  runtime compares it with where the buffer has fetched up to.
- Does it matter? Less than you'd think, for Kirby. Baking one number per
  instruction gives **the same 4500 frames, pixel for pixel**, the same RNG
  and the same lag frames; only 296 of 2.4 million sound samples move.
  Getting the *big* thing wrong (the prefetch buffer off) changes 1866 of
  4500 frames, adds lag frames, shifts the RNG, makes the helper Kirbys
  wander elsewhere and puts the music a frame late.

All numbers are from `just timing-lab` (about 90 s): Kirby & the Amazing
Mirror from reset, menu presses, then a short fixed input (walk, run, jump,
float, puff, slide, inhale, then float over a cloud wall into new ground),
4500 frames, about 75 seconds of game.

## 1. What the address is for

The GBA's cartridge is slow. To hide that, the CPU has a little queue, the
**prefetch buffer**, that keeps reading the next instructions from the
cartridge while the CPU is busy with something else, like a load from RAM.
It holds at most 8 halfwords (8 Thumb instructions).

```
 cartridge ──► [ ][ ][ ][ ][ ][ ][ ][ ] ──► CPU
                prefetch buffer, 8 halfwords

 ldrb r0, [r5]      the CPU waits on RAM for a few cycles...
                    ...and the buffer uses them to fetch ahead,
                    if it has room.
```

So what a load costs depends on how far ahead the buffer already is when
the load starts. The runtime keeps one number, the address the buffer has
fetched up to (`bus.prefetched`), and each `m.get(at, …)` and `rt::stall`
compares it with `at`:

```
  at+4                  prefetched
   │◄──── the lead ────►│        lead = prefetched - (at + 4)
   ▼                    ▼        0, 2, 4 … 14 bytes, or "none"
 ──┬──┬──┬──┬──┬──┬──┬──┬──      (behind, or emptied by a jump)
```

That's mGBA's `GBAMemoryStall`, which we copy (`runtime/src/waits.rs`,
`Waits::stall`). The address is the *only* way the runtime knows where this
instruction sits relative to the buffer.

## 2. Does one instruction's cost change between visits?

Mostly no. Over the 4500 frames:

| | sites | share of the 36.7 M runs |
|---|---:|---:|
| always one cost | 18365 | 98.7% |
| two costs | 65 | 0.7% |
| three costs | 14 | 0.6% |

(A "site" is one memory instruction in the ROM; "cost" is what `rt::stall`
adds to the instruction's listed price, in cycles. It's often negative: the
listed price assumes the next fetch is a slow one, and the buffer usually
has it ready.)

The 79 sites that vary, and why (the first explanation that fits every
visit):

| what decides it | sites | share of all runs |
|---|---:|---:|
| how full the buffer was | 54 | 0.9% |
| which memory the pointer hit | 21 | 0.4% |
| `WAITCNT` | 4 | 0.0% |

Two that run the most:

**The music engine's track loop** (`0x0814FAE0`, `ldrb r0, [r5]`; by its
shape m4a's `MPlayMain` asking "is this track playing?"). It always reads EWRAM, always the
same 3 cycles. But it runs once per track, after 8 different memory
instructions, so it arrives with the buffer at different fills:

```
 buffer lead   none   4    8    12   14
 cost (cyc)     -3   -3   -2   -2   -1      95,634 runs
 share          42%   7%   7%   7%  36%
```

A full buffer is the *expensive* case: it has no room left to use the
load's wait, so that time is simply lost.

**A byte-copy loop** (`0x0815BD70`, `ldrb r0, [r3]`). The same instruction
copies from IWRAM (1 cycle a byte) or from the cartridge (4 cycles, and the
buffer can't help, because the cartridge bus is busy with the copy):

```
 source       IWRAM   cartridge
 cost (cyc)    -3       +4          111,617 runs
 share         70%      30%
```

Nothing in the code tells you which: it's whatever pointer the caller
passed. That's "the address it touched".

`WAITCNT` hardly matters for Kirby: it writes `0x4017` early in boot and
keeps it; `0x0000` and `0x4014` together cover 55,506 runs at the very
start. DMA and interrupts don't appear as their own cause: in mGBA's model
a DMA doesn't touch the buffer, and an interrupt shows up as a different
path in (the buffer is emptied by the jump).

## 3. What happens when the timing is wrong

Four wrong timings, each run over the same 4500 frames with the same
input, compared with the real run:

| variant | what it does | frames that differ | lag frames | RNG | sound samples that differ |
|---|---|---:|---:|---|---:|
| real | `rt::stall` as is | — | 19 | — | — |
| **no-prefetch** | as if the buffer were off | **1866** | **21** | differs from frame 262 | **68%** |
| **zero** | delete every `rt::stall` | 1 | 19 | same | 0.43% |
| **baked** | one number per site (its most common cost, learned from frames 1–3000) | **0** | 19 | same | 0.01% (296) |
| **table** | one number per site × buffer lead × `WAITCNT` (also learned from 1–3000) | **0** | 19 | same | 0.004% (100) |

For scale: a frame is 280,896 cycles. The buffer saves Kirby **32,955
cycles a frame (11.7%)**. Baking one number per site is off by about **107
cycles a frame (0.04%)**.

### The tear on frame 6

Kirby finishes booting partway through frame 6 and the screen goes from
black to white. Where that line falls is exactly how long boot took:

```
          real          no-prefetch      zero
 line 0  ██████████    ██████████
        ██████████    ██████████
 line 23 ──────────    ██████████
                      ██████████
 line 72               ──────────
 line 159
```

Real: white from line 23. Buffer off: line 72 (49 lines, about 60,000
cycles, later). Stalls deleted: line 0 (boot runs with the buffer off, so
its stalls are all extra cost, and deleting them makes it faster). That's a
mid-screen effect in its simplest form: *when* a register write lands
decides *which line* changes.

### One lag frame, then everything slides

With the buffer off, frame 260 of the title screen doesn't finish in time
(a **lag frame**: the main loop isn't back at its V-blank wait when the
frame ends). From then on:

- the title collage is one frame behind (frame 300: 10,491 pixels differ);
- the RNG is one draw behind for frames 262–388 (the title draws from it
  every other frame), until the title stops drawing and the late run
  catches up;
- two more lag frames: 1330 (loading the first room) and 1506, where
  Kirby's jump starts a frame late (his height differs for frames
  1505–1522, then he lands in the same place);
- the three helper Kirbys (CPU-controlled, RNG-driven) take different
  steps from frame 1839, and from frame 3703 they never agree again
  (frames 1990, 3700, 4100, 4500: they stand elsewhere, and the edge
  arrows that point at them move);
- the music: 77% of the differing samples are exactly the real sound one
  frame (264 samples) late. Each lag frame pushes the song back a frame.

Kirby himself always ends where the real run does: he's driven by the
input, and the game waits for V-blank, so finishing early or late inside a
frame mostly doesn't show.

### Baked: nothing visible

Learning each site's most common cost from frames 1–3000 and baking it in
gave all 4500 frames byte-identical, the same RNG and the same lag frames,
including the 1500 frames of new ground the table never saw (0.30% of the
run's stalls had no entry and kept their real cost). Only sound moved: 296
samples, scattered single samples a step off, most likely a sound register
write landing one sample earlier or later.

## 4. Is it the halting problem?

Honestly: **not the cost, but the inputs, in a way.**

- *The cost of one instruction* is decidable and small. It's a function of
  (site, buffer lead, data region, `WAITCNT`): 9 leads, a handful of
  regions, a few `WAITCNT` values. The last table in section 5 prices every
  run exactly. No halting problem there.
- *The total time of a program* is undecidable in general (if you could
  always compute it, you'd know whether it stops), but no recompiler needs
  that: it adds costs as it goes, the same way the GBA does.
- *Knowing ahead of time which row of the table each visit uses* is where
  it bites. "Does this pointer ever point at the cartridge?" is a question
  about what values the program computes, and by Rice's theorem (the
  halting problem's big sibling) no analysis answers that for every
  program. The byte-copy loop is the everyday version: the same code is
  called with a RAM source and a ROM source.

So your instinct points the right way, just one step off: the halting
problem isn't in *the cost*, it's in *knowing the pointer and the path*.
In practice an analysis could prove most sites constant (98.7% of runs here
were at sites with one cost), but "most" isn't "exact", and our oracle is
byte-for-byte against mGBA.

## 5. Could a static table replace the address?

How each table, filled from the real run itself, prices that same run
(the best case, since it's tested on what it learned from):

| table key | runs priced wrong | cycles off a frame |
|---|---:|---:|
| site | 0.41% | 107 |
| site × `WAITCNT` | 0.41% | 107 |
| site × path in (previous memory instruction) × `WAITCNT` | 0.29% | 83 |
| site × address region × `WAITCNT` | 0.28% | 47 |
| site × buffer lead × `WAITCNT` | 0.10% | 54 |
| site × path in × address × `WAITCNT` | 0.16% | 24 |
| site × buffer lead × address × `WAITCNT` | **0** | **0** |

Only the last is exact, and two of its keys (buffer lead, address) are
runtime values. A table indexed by them is `Waits::stall` written out as
data: you'd still need the address to find the buffer lead, and you'd
still have to move the buffer's pointer on after each access, because the
next site's cost depends on it. Even the "path in" doesn't settle it, since
the previous instruction's own cost depends on *its* pointer.

What it would mean for the readable view:

- **Exact** (what we have): keep `at` in every access. It's how the runtime
  finds the buffer lead. It's also what ties a line to its ROM
  instruction, so it would stay in some form anyway.
- **Nearly exact**: the view could *hide* `at` (print `m.get(Ptr…)` and
  keep the address in the macro behind it), since it's mechanical. That
  changes nothing at runtime.
- **Baked**: print a constant per access and drop the buffer. For Kirby on
  this route, nothing you'd see changes; sound differs in a few hundred
  samples and the mGBA comparison fails. A game that reads a timer for its
  RNG, or splits the screen mid-frame from a busy loop, would show it.

## How to rerun it

`just timing-lab` builds gbaa-desktop with the `timing-lab` feature
(`target/timing-lab`; a hook in `Bus::stall`, `runtime/src/timing_lab.rs`,
off in every other build) and writes `out/timing-lab/kirby/`:
`report.txt` (everything above), `base.tsv` (every distinct stall: site,
inputs, cost, count), each run's `NAME.out` (`scripts/kirby/timing.lua`:
frame counter, RNG, lag, Kirby's position), `NAME.hashes`, `NAME.wav`, and
`NAME/diff-N.png` (real | variant | difference in magenta) for each frame
that differs. The pictures stay out of git; the recipe makes them again.

Caveats: the runs are in the interpreter, not the recompiled build; both
price stalls through the same `Bus::stall` (`rt::stall` calls it), so the
results carry over. The model is mGBA's, not measured hardware. One game,
one route of 75 seconds; Kirby waits for V-blank every frame and has slack
most of the time, which is why small errors vanish.
