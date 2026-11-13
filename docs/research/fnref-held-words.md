# What holds the function words: affine, traced word by word

Unit `native/fnref-unknown-addrs`, 2026-11-13 on quack. The function-address
flow (`crates/codegen/src/native/fnref.rs`; `docs/native-stage-b.md`, piece
3) proves 4 or 5 of each agb program's 55 to 62 function words, and three
units in a row (the heap by allocation site, call-site context, loads
through unknown addresses) each made the flow more exact without moving
that count. This is why, measured.

**The answer.** No one kind of read holds the words. Each word reaches a
use that isn't allowed by several roads, and any one of them is enough.
The unit's premise, that loads through unknown addresses hold the count,
is wrong: with those loads cut from 623 sites to 3 on affine the count is
55, and with every mixing memory read switched off at once it is still 55.

Everything below was measured on the unit's branch,
`unit/native/fnref-unknown-addrs` (it also carries the object split and
call-site context, none of it on main), on `agb/affine` unless a program
is named. Addresses are our own agb test programs'.

## How it was measured

`GBAA_FNREF_WATCH=0x080008c8[,…]` (or `all`), on that branch, makes
`gbaa-lift` print each place a ROM word's function address goes, once:
read from which memory by which load, stored where, passed in which
register to which function, returned by which, and each use not allowed.
The lines before a word's first `HOLE` are its road. `GBAA_FNREF_TRACE=1`
says where a pointer into RAM is lost and why a callee set is incomplete.

The switches of "Every memory read switched off" were temporary and
aren't in the tree: each made one kind of read give nothing, which is
unsound, to see what the count would be if that kind were exact.

## The loads that read everything

A load through a pointer into RAM the run dropped reads every RAM cell,
every object and the pile. The unit's card counted 623 such sites on
affine; an earlier session on the branch got them to 8. On the branch
merged with main tonight they were back at 580 (the branch from before
the merge no longer builds beside main's other crates, so when they came
back wasn't checked). Three causes, fixed on the branch:

- A RAM address plus another pointer (into ROM, an object) marked the sum
  a lost pointer. No program adds two pointers: these are two values'
  tokens paired.
- Value-set bounds of 1 to 0xffffffff ("not null") on a load counted as
  bounds, so the load read every cell in them, which is every RAM cell
  and every function's frame, without being a load through a lost
  pointer. It wasn't in the count of 8.
- A function with no caller in sight (its vtable word is never read)
  called through what it was passed, found no function there, and its
  call was incomplete where main had every address-taken function. Giving
  such functions lost pointers as parameters brings all 623 back; instead
  their calls through an empty target take the address-taken functions,
  as on main.

With those, the new metric `fn_loads_read_everything` is 3 on eleven
programs, 5 on native-fallthrough and the spike, and 571 on
native-bios-calls (not looked at). The 3 left on affine are loads at
0x08000a24 to 0x08000a2c through a pointer into a caller's frame at or
above the callee's entry sp, which the run can't place.

## Where each word first goes wrong

Affine has 60 function words: 5 proven, 15 never read, 40 with a use not
allowed. What delivers each of the 40 to its first such use:

| words | road |
|---|---|
| 11 | the pile, read by a load at a constant address (0x08000188, 0x0300001c) or through a mixed set (0x08000bc0). Five got there as bytes stored by a copy loop in IWRAM (0x03000164) through a lost pointer, three by a store through an unknown address (0x080009cc), one from the reset's `CpuSet`, two from the start's copies |
| 10 | one of the 3 loads left that read everything (0x08000a2c) |
| 6 | an array of (value, function) pairs in a caller's frame, read by index at 0x080036e4 and 0x080036e8 (a loop like `core::fmt::write`'s over its arguments): the pointer is "anywhere in the frame", so the value read holds the functions too, and the callee (one of a set of 12) uses its value as an address at 0x080037b2 |
| 6 | bytes of the function's own frame read by a pointer anywhere in it (0x08001418, 0x08001c86) |
| 3 | a frame word the run hadn't kept, read as everything stored in the frame (0x08001b66) |
| 3 | given to the BIOS: one is loaded two instructions before its `swi` (0x0800016c), two were left over in r2 and r3 |
| 1 | a ROM word read through a set of 11 ROM pointers merged from every caller (0x08003734) |

## Every memory read switched off

The count of words not proven on affine, as built 55, with kinds of read
made to give nothing:

| switched off | not proven |
|---|---|
| nothing | 55 |
| loads that read everything | 55 |
| the pile | 56 |
| a pointer anywhere in an object (it reads only the object's anywhere cell) | 55 |
| a frame word not kept (one of its two reads) | 55 |
| all four | 55 |

With all four off, 12 of the words are held by one road: the address is
returned in r3 to the reset function and holed at its `swi` at
0x080000d8. Nothing read r3 on the way.

## Registers nobody reads

A call passes r0 to r3 to its callee and takes back what the callee's
returns hold in them, whether or not the callee reads or sets them. So a
function's address left in a scratch register travels the whole program.
On native-irq-hold the handler's pool word 0x080001bc is loaded into r3 at
0x08000188. Before its first use not allowed it is passed in r3 39 times
and returned in r3 36 times by 26 functions, stored once (a byte, by the
copy loop in IWRAM), and never read. It ends at the reset's `swi`, where
all four registers count as given to the BIOS.

Calls whose target is still empty in an early round, and complete when the
run settles, also pass their registers to every function without a direct
caller for good.

## Why the branch doesn't land

june's answer to the unit's question was to land the step on the count of
loads that read everything, once native-irq-hold has its 16 typed heap
accesses back. It doesn't yet. Twelve bars are over on the branch:

- **native-irq-hold** loses two words main proves, its own two handlers'
  (0x080001bc, 0x080001c0), by the road above; with the reset's `swi` out
  of the way the IWRAM copy loop's pile store and a load at 0x0800019a
  hold them next. Both handlers are then in every `call_fn` set (14 functions
  on main, 21 on the branch), the heap's typing follows every callee, and
  the IRQ list's type is gone: `ref_accesses` 16 to 0, `ref_unexplained`
  48 to 93, and with them `lines`, `load_computed_unknown` and
  `store_computed_unknown`; `fn_words_not_proven` 59 to 60.
- **`ref_unexplained`** on native-bios-more (94 to 136), native-bios-region
  (36 to 77), native-dma (43 to 85) and the spike (36 to 78). The branch
  reads more words (the spike: 24 never read on main, 18 on the branch),
  every `call_fn` set is every address-taken function, so every set
  grows (16 to 19 on the spike).
- **native-tasks**: `call_fn_four_complete` 0 to 56 and `ref_unexplained`
  0 to 35 (both bars are 0 on main; the branch has 56 complete sets), and
  `fn_words_not_proven` 57 to 58.

The count the unit was to move, `fn_words_not_proven`, is 55 on affine
against main's 56, and within one of main's on the others.

## What it points to

1. Registers first (`native/fnref-live-registers`): a call gives a callee
   only the registers it reads and takes back only those it sets (the
   summaries have both, `codegen::saves`), and a `swi` holes only what its
   BIOS call reads. It is the one road that holds 12 words by itself with
   memory exact, it is what un-proves native-irq-hold's handlers, and
   nothing in it is about agb: a C game's scratch registers travel the
   same way.
2. Then the branch can land (`native/fnref-ram-pointers-land`), if
   native-irq-hold's handlers are proven again.
3. The roads through memory come after: a pointer moved by an index stays
   in its stack slot (the frames' slots are known, `native/frame.rs`); an
   array of pairs keeps its fields apart (an offset known modulo the
   stride); loads at a constant address stop reading the pile; a copy
   routine's source and destination kept apart by caller.

No single one of these lowers `fn_words_not_proven`: a unit whose check
is that count falling will fail as these three did. Each needs a direct
measure (the words whose first road it is, from `GBAA_FNREF_WATCH=all`).

One question this leaves for june: agb programs are test inputs, and
their words are held by Rust's shapes (formatter arguments in stack
arrays, trait objects, a byte copy of closures). Metroid's flow settles
(2,616 functions); what holds a C game's words hasn't been looked at, and
may be a shorter list.
