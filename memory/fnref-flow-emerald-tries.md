---
title: Emerald's flow: what entries gain a round, and what didn't stop it
hook: not pointers, not forms; reruns, saturating, a shared set, closing forms all failed
open: true
---

Where the flow stands, how to measure: `fnref-flow-emerald-memory.md`.

**What the entries gain (2026-11-14, `GBAA_FNREF_GAINS=1`).** Not ROM
pointers stepping down: no ROM read "from here on" is new after round 2
(3,700, 3,152, 66, then 0), and only 5 ever start below one of their
object's. The origins all entries and results hold between them stop at
5,153 after round 1 (5,157 later); their function tokens keep growing
(12.4k, 14.9k, ...): the large sets (13-15k tokens; 17,900 registers over
4,096 after round 1, 38,500 after round 2) each gain other *forms* of
addresses already there (Lo, LoUp, Part, Cmp) and refs other sets already
hold, a step a round, and every entry sharing one changes. Memory still
grows too (pile 22.4k, 23.5k, 24.4k after rounds 2-4), so every round
reruns ~14.5k functions.

**Tried, didn't help (code not kept; `/tmp` copies gone).** (1) Running a
function dirtied mid-round in the same round when it comes later (order
alternating up and down by round): Metroid's output identical (27 rounds
against 23, same time), Emerald's rounds no shorter (r1 170 s, r2 406, r3
718, r4 1,063). (2) Saturating: an entry or result over 4,096 tokens takes
every form of every origin large ones hold: each became its own 32k-token
set (different refs and ROM pointers beside), held tokens headed for the
48M bound in round 2. (3) Making them all one shared set (every token any
holds, capped, forms closed): Metroid identical (largest set 664 tokens),
but on Emerald the shared set dragged its ROM pointers into every large
value: 9,251 new ROM reads in round 2, pile 56k, round 2 took 1,226 s, 47.5M
tokens held. So the next try must not hand ROM pointers and refs around:
close forms only, or collapse a large value's function tokens to a mark
("every function's every form") the ops and holes know.

**Closing forms doesn't help (2026-11-14, `native/fnref-forms-closed`).**
An entry or result over 4,096 tokens took, after each join that grew it,
(a) every form of each function it held, or (b) each form it held of any
function, for every function it held: no other sets' tokens. Same build,
`GBAA_FNREF_GAINS=1`, runs side by side on quack (so slower than alone):

| round | base: entries changed, end | (b): entries changed, end | pile base / (b) |
|---|---|---|---|
| 1 | 13,653, 171 s | 13,653, 211 s | 22,238 / 34,682 |
| 2 | 13,515, 637 s | 13,515, 728 s | 32,658 / 40,249 |
| 3 | 13,155, 1,140 s | 13,155, 1,460 s | 34,043 / 41,407 |

(a) was like (b): 13,653 in round 1 (245 s), pile 35,300, largest set 30k.
The entries changed are the same to the entry: forms never decide *which*
entries change; every entry that gains forms also gains refs (`Ref`s of
the pile, cells and ROM reads, new `Xf` maps) or ROM "from here on"
tokens the cap moves (`-[from] +[from]`) that round. Closing only made
sets twice the size (largest 26-28k against 12-17k function tokens),
pushed the forms into the pile (stores keep a function's form) and holes
nothing new (7,638 both). Metroid's `--callee-sets` output and trace were
identical with it (no set reaches 4,096). Code not kept.

Since main's table work (`fnref-table-ends`, `-table-fields`), the base
differs from the notes above: pile 22k after round 1 (16.9k before),
holes 7,638 from round 2 (9,414 before), 13.2-13.7k entries changed.
Next: what makes refs and "from" tokens move a round (the gains trace's
samples: small sets gaining a `Ref` one call deeper a round look like
plain propagation down call chains; the in-round reruns above didn't
shorten it), or collapse a large value to a mark the ops and holes know.
