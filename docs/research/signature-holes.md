# Function signatures: the summaries against the prototypes

Unit `native/lift-signature-holes` (2026-11-24, quack). Since the owner's
call of 2026-11-20 a lifted function whose names files give a prototype
takes its parameters and gives its result from it, hole or not
(`crates/codegen/src/native/shape.rs`, its module doc). This page is the
other half of that card: what the summaries alone give (`gbaa-lift
--no-prototype-shapes`, the shapes a game with no decomp gets until an
agent or an algorithm writes its prototypes), why they keep four
registers where they do, and how often they agree with the prototypes
where both speak. Every number is a line of the lift's `report.txt`
(`summary holes by kind`, `the summaries' shapes against the
prototypes`, `prototype refused`), for Emerald and Metroid lifted at
main f5a9e40 plus the unit, with their holes files as of mods 7f389b9.
`native/lift-pret-score` scores the same against pret in one table.

## What the prototypes changed

| | Emerald | Metroid |
|---|---:|---:|
| functions with a prototype | 15,052 | 2,557 |
| shaped from it | 14,164 | 2,489 |
| of those, with a summary hole | 3,700 | 561 |
| refused: the summaries prove it wrong | 888 | 68 |
| `regs_call_entries` (four registers packed into `Regs::call`) | 3,622 → 122 | 901 → 33 |
| `entries_beyond_one` (adapters `a_` for pointer calls) | +2,274 | +160 |

Both lifts build and run 600 of 600 frames headless. On the C programs
`just lift-check` passes, the planted wrong prototype (c-chain-agbcc's
`overlaps` taken to return nothing) caught by the native check at frame 2.

## Why the summaries keep four registers

A function counted once for each kind of hole its summary has (the
report's `summary holes by kind`, addresses left out).

| cause | Emerald | Metroid |
|---|---:|---:|
| functions with a summary hole | 3,912 | 594 |
| a callee with a hole (`calls …, which doesn't keep them`: the callee-saves proof fails through it) | 3,358 | 533 |
| an indirect call with an incomplete callee set | 511 | 51 |
| a jump by value to an unknown place | 27 | 6 |
| a callee whose summary has holes and no return | 14 | 12 |
| a fall or jump into code it doesn't follow (`jumps to …, which has no code here`, `reads r4-r11 at entry, only jumped into`) | 6 | 0 |
| r4-r11 taken from its callers, or their entry bits masked | 22 | 4 |
| a mode or PSR change | 3 | 3 |
| a stand-in (the holes file replaces it) | 1 | 3 |

Nearly all of it is one hole passed up the call graph: a function whose
callee may break the callee-saves proof can't be proven itself, so a
few hundred root causes (indirect calls, chiefly) leave thousands of
callers four-register. No caller in either game rests on a callee's
stopped run alone (`callee_hole` 0): the holes are the functions' own.

## Where both speak

Of the functions with a prototype, the summaries' own shape against it
(the report's `the summaries' shapes against the prototypes`):

| | Emerald | Metroid |
|---|---:|---:|
| no shape (a hole, a callee's, a stand-in) | 3,862 | 584 |
| the same parameters and results | 9,211 | 30 |
| fewer parameters (one the function never reads) | 320 | 20 |
| more parameters | 633 | 601 |
| other results | 1,337 | 1,939 |

Emerald's summaries agree with pret on 9,211 of the 11,190 they shape
(82%). Metroid's agree on 30: one jump by value to an unknown place makes
every function's results all four registers (the report's `read after a
tail call by value: {r0, r1, r2, r3}`; Emerald's is `{r0}`), since any
function might be where it goes. Parameters are the better half: an
unworked game's summaries get them right but for the reads they can't
rule out (an indirect call taken to read all four, GCC's push of r3 to
keep the stack aligned) and the parameters C declares and never reads.

## What the summaries refuse, and why

A prototype is refused only on a proof: an instruction (the function's,
or at the end of the callees it passes the register on to) or a BIOS call
reads a register the prototype doesn't pass; a caller's instruction reads
a result the prototype doesn't return, also through `return f(x);` and
falls; it returns a result as it came that it isn't passed; it takes or
gives r4-r11. An indirect call's "reads all four" is no proof.

- Emerald, 682 of 888: r2 read by one function. pret declares
  `GetMonData` both with two parameters and with three
  (`GetBoxMonData2`, `GetBoxMonData3`, one address), so a call of the
  two-parameter form passes whatever r2 holds, and `GetBoxMonData` reads
  it (for the fields that copy out, a nickname). 501 callers pass their
  own entry r2 on to it; 181 are callees whose callers pass on the r2
  they left. Taking the prototype there would be right wherever the
  field is a number and wrong where it isn't; only a per-call reading of
  the field could tell.
- Emerald, 194: a caller reads r2 after them, carried over `return f(x);`
  and falls from another function's proof (the report names no
  instruction for these: the proof is a caller's, not theirs), among them
  m4a's and the wireless adapter's.
- Emerald's last 12: 10 take r4-r11 from their callers; m4a's
  `ply_xwait` and a flash routine's caller read r2 themselves.
- Metroid's 68: a caller reads r0 after 43 (the IRQ callbacks, whose
  dispatcher reads r0 after them, and their kin), r3 after 9, r1 after
  1; 12 read r3 or r1 themselves (the save routines); 3 take r4-r11.
- The C test programs' GCC builds, 2 each in c-fnptr-gcc and
  c-loops-gcc: the push of r3 that keeps the stack 8-byte aligned reads
  it.

The refusals are conservative: none is a wrong prototype. A wrong one the
summaries can't see (a read past a hole) is what the native check and
fn-check catch, as the planted control shows.
