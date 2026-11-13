# What holds the function words: affine, traced word by word

Unit `native/fnref-unknown-addrs`, 2026-11-13 on quack. The function-address
flow (`crates/codegen/src/native/fnref.rs`; `docs/native-stage-b.md`, piece
3) proves 4 to 6 of each agb program's 59 to 66 function words, and three
units in a row (the heap by allocation site, call-site context, loads
through unknown addresses) each made the flow more exact without moving
that count by more than one word. This is why, measured.

**The answer.** No one kind of memory read holds the words, so no unit
that makes one kind exact moves the count. Two kinds together do: with
the loads that read everything and the reads through a pointer "anywhere
in an object" both made to give nothing, affine's count of words not
proven goes from 55 to 41; with either alone it stays 55. The unit's
premise, that loads through unknown addresses hold the count, is half of
that pair: those loads are down from 623 sites to 3 on affine, and the
count is 55.

Everything below was measured on the unit's branch,
`unit/native/fnref-unknown-addrs` at 233ba6c0 (it also carries the object
split and call-site context, none of it on main), on `agb/affine` unless
a program is named. Addresses are our own agb test programs'.

**These numbers are from before main's `Ref` tokens.** While this was
measured, main's function flow was rewritten to keep large pile and ROM
reads as one token each (`native/fnref-emerald-speed`, f52fe115). The
branch isn't merged with that yet (22 conflicts in `fnref.rs`, and the
branch's objects and RAM pointers must learn the new token), and nothing
here is re-measured on it: `native/fnref-branch-merge` does both, and
corrects this note. "Main" below is main before that commit.

## How it was measured

Both tools are on that branch only.

`GBAA_FNREF_WATCH=0x080008c8[,…]` (or `all`) makes `gbaa-lift` print each
place a ROM word's function address goes, once: read from which memory by
which load, stored where, passed in which register to which function,
returned by which, and each use not allowed. The lines before a word's
first `HOLE` are a road to it: the first the run found, in the order it
runs functions, not the only one. `GBAA_FNREF_TRACE=1` says where a
pointer into RAM is lost and why a callee set is incomplete.

`GBAA_FNREF_OFF=lost,pile,any,spill` makes kinds of read give nothing,
to see what the count would be if that kind were exact. It is unsound
(the lift says so) and an upper bound: an exact read still reads
something.

## The loads that read everything

A load through a pointer into RAM the run dropped reads every RAM cell,
every object and the pile. The unit's card counted 623 such sites on
affine; an earlier session on the branch got them to 8. On the branch
merged with main tonight they were back at 580 (the branch from before
the merge no longer builds beside main's other crates, so when they came
back wasn't checked). The cause: a RAM address plus another pointer
(into ROM, an object) marked the sum a lost pointer. No program adds two
pointers: these are two values' tokens paired. Fixed, 580 became 3.

Two more fixes on the branch, which don't move that count:

- Value-set bounds of 1 to 0xffffffff ("not null") on a load counted as
  bounds, so the load read every cell in them, which is every RAM cell
  and every function's frame, without being a load through a lost
  pointer.
- A function with no caller in sight (its vtable word is never read)
  called through what it was passed, found no function there, and its
  call was incomplete where main had every address-taken function. Giving
  such functions lost pointers as parameters brings all 623 back; instead
  their calls through an empty target take the address-taken functions,
  as on main.

The new metric `fn_loads_read_everything` (a site counts if any run of
its function read everything, an early round's too: what it read then
stays) is 3 on eleven programs, 5 on native-fallthrough and the spike,
and 533 on native-bios-calls (not looked at). The 3 on affine are loads
at 0x08000a24 to 0x08000a2c through a pointer into a caller's frame at
or above the callee's entry sp, which the run can't place.

## Where each word first goes wrong

Affine has 60 function words: 5 proven, 15 never read, 40 with a use not
allowed. The road the run found first to each of the 40:

| words | road |
|---|---|
| 11 | the pile, read by a load at a constant address (0x08000188, 0x0300001c) or through a mixed set (0x08000bc0). Five got there as bytes stored by a copy loop in IWRAM (0x03000164) through a lost pointer, three by a store through an unknown address (0x080009cc), one from the reset's `CpuSet`, two from the start's copies |
| 10 | the 3 loads left that read everything (0x08000a24 to 0x08000a2c), spilled and loaded back at 0x08000a40 |
| 6 | an array of (value, function) pairs in a caller's frame, read by index at 0x080036e4 and 0x080036e8 (a loop like `core::fmt::write`'s over its arguments): the pointer is "anywhere in the frame", so the value read holds the functions too, and the callee (one of a set of 12) uses its value as an address at 0x080037b2 |
| 6 | bytes of the function's own frame read by a pointer anywhere in it (0x08001418, 0x08001c86) |
| 3 | a frame word the run hadn't kept, read as everything stored in the frame (0x08001b66) |
| 3 | given to the BIOS: one is loaded two instructions before its `swi` (0x0800016c), two were left over in r2 and r3 |
| 1 | a ROM word read through a set of 11 ROM pointers merged from every caller (0x08003734) |

## Kinds of read switched off

Words not proven on affine (of 60; as built 55), by `GBAA_FNREF_OFF`:
`lost` the loads that read everything, `pile` the pile, `any` a pointer
anywhere in an object (it reads only the object's anywhere cell), `spill`
a function's spills (what it stored in its frame, read where the run
kept no word).

| off | not proven | | off | not proven |
|---|---|---|---|---|
| nothing | 55 | | `lost,any` | **41** |
| `lost` | 55 | | `lost,any,spill` | 41 |
| `pile` | 56 | | `lost,pile,any` | 40 |
| `any` | 55 | | all four | 39 |
| `spill` | 55 | | `pile,any` | 51 |
| `lost,pile` | 54 | | `pile,any,spill` | 51 |
| `any,spill` | 55 | | `lost,pile,spill` | 54 |

So `lost` and `any` together are worth 14 words, and nothing else much:
the 3 loads through a pointer into a caller's frame, and the reads by a
pointer moved in a frame by an index. Both are about stack frames. With
the pile off, 13 complete callee sets become incomplete (56 to 43), so
those rows also measure a flow with fewer callees.

With all four off, 13 words are "given to the BIOS", 12 of them (in a
run before the spill switch covered both its reads) by one road: the
address is returned in r3 to the reset function and holed at its `swi`
at 0x080000d8. That run has 15 incomplete sets passing their registers to
every function without a direct caller, which the flow as built hardly
does, so it shows the road exists, not how many words it holds as built.

## Registers nobody reads

A call passes r0 to r3 to its callee and takes back what the callee's
returns hold in them, whether or not the callee reads or sets them. So a
function's address left in a scratch register travels the whole program.
This one is measured as built, no switch: on native-irq-hold the
handler's pool word 0x080001bc is loaded into r3 at 0x08000188. Before
its first use not allowed it is passed in r3 39 times and returned in r3
36 times by 26 functions, stored once (a byte, by the copy loop in
IWRAM), and never read. It ends at the reset's `swi`, where all four
registers count as given to the BIOS.

Calls whose target is still empty in an early round, and complete when the
run settles, also pass their registers to every function without a direct
caller for good.

## Why the branch doesn't land

june's answer to the unit's question was to land the step on the count of
loads that read everything, once native-irq-hold has its 16 typed heap
accesses back. It doesn't yet. Twelve bars are over on the branch:

- **native-irq-hold** loses two words main proves, its own two handlers'
  (0x080001bc, 0x080001c0), by the road above, and gains one
  (0x0802448c); with the reset's `swi` out of the way the IWRAM copy
  loop's pile store and a load at 0x0800019a hold the two next. Both
  handlers are then in the `call_fn` sets (14 functions on main, 21 on
  the branch), the heap's typing follows every callee, and the IRQ
  list's type is gone: `ref_accesses` 16 to 0, `ref_unexplained` 48 to
  93, and with them `lines`, `load_computed_unknown` and
  `store_computed_unknown`; `fn_words_not_proven` 59 to 60.
- **`ref_unexplained`** on native-bios-more (94 to 136), native-bios-region
  (36 to 77), native-dma (43 to 85) and the spike (36 to 78). The branch
  reads more words (the spike: 24 never read on main, 18 on the branch),
  nearly every `call_fn` set is every address-taken function, so the
  sets grow (16 to 19 on the spike).
- **native-tasks**: `call_fn_four_complete` 0 to 56 and `ref_unexplained`
  0 to 35 (both bars are 0 on main; the branch has 56 complete sets), and
  `fn_words_not_proven` 57 to 58.

The count the unit was to move, `fn_words_not_proven`, is 55 on affine
against main's 56, and within one of main's on the others.

## What it points to

0. The branch merged with main's `Ref` tokens and these tables measured
   again (`native/fnref-branch-merge`): everything below waits on it.
1. Frames, as one unit (`native/fnref-frame-pointers`), since neither
   half moves the count alone: a pointer into a caller's frame at or
   above the entry sp followed to the callers' frames, not lost (the 3
   loads left); and a pointer moved by an index staying in its stack
   slot (`native/frame.rs` has the slots), an array of pairs keeping its
   fields apart (an offset known modulo the stride). The switches say up
   to 14 words on affine, so a falling count is a fair check for it.
2. Registers (`native/fnref-live-registers`): a call gives a callee only
   the registers it reads and takes back only those it sets (the
   summaries have both, `codegen::saves`), and a `swi` holes only what
   its BIOS call reads. It is what un-proves native-irq-hold's handlers
   on the branch, and nothing in it is about agb: a C game's scratch
   registers travel the same way.
3. Then the branch can land (`native/fnref-ram-pointers-land`), if
   native-irq-hold's handlers are proven again.
4. After those: loads at a constant address stop reading the pile (alone
   it un-proves a word and breaks 13 callee sets, so the sets need
   another source first); a copy routine's source and destination kept
   apart by caller.

A C game was the question this left, and it has a different answer:
the next section.

## A C game: Metroid, measured on main

Unit `native/fnref-c-game-words`, 2026-11-13 on quack, main at dac9dd7f
(with the `Ref` tokens). Everything above is about agb programs, which
are Rust. This is the same question for a game old GCC compiled from C:
Metroid Zero Mission, 2,623 functions, lifted with its holes file.
Addresses are the game's; nothing else of it is here. The counts and
the roads are the same on main at adfa45ee, where a function may take
r4 to r11 from its callers and Metroid lifts with no hard hole again.

**The answer.** What holds affine's words was not found holding
Metroid's. Nothing at the 19 instructions looked at, which hold 727 of
its 734 words, and no road of the 20 words watched (one or two from
each row below), goes through a stack frame, a heap object or a
formatter's argument array. That is a sample: for 180 of the 734 the
tool says only that the value held the address itself. So the frames unit
(`native/fnref-frame-pointers`), the branch's object split and RAM
pointers (`native/fnref-ram-pointers-land`), the merge that carries them
to main (`native/fnref-branch-merge`), the heap by allocation site
(`native/fnref-heap-sites`) and call-site context
(`native/fnref-context`) are agb work: Metroid's count needs none of
them.

It needs one filed follow-up, and only half of it: the registers unit
(`native/fnref-live-registers`) says a `swi` should hole only the
registers its BIOS call reads. That rule alone takes Metroid from 37
proven words to 466 of 804, on main, with nothing of the branch. It is
filed on its own now (`native/fnref-bios-reads`).

The rest of what holds a C game's words was filed nowhere: where a ROM
table ends, struct tables read by one field, and three things old GCC's
code does that LLVM's doesn't (a move written as an add of 0, a compare
with 0 whose carry nobody reads, a handler left in the register it was
called through).

The pile is the exception, on the larger games. It holds 13 of
Metroid's words, but it is the first road of most of Nightmare's and
the largest share of Emerald's (below). Not the heap by allocation site:
C has no allocator to split by. Its pile is stores through pointers into
globals.

**Counts**, main as built: 804 function words, 68 in literal pools and
736 in data (tables of handlers). 37 proven, 33 never read, 734 with a
use not allowed. 70 of the 72 `call_fn` calls have a complete callee
set, where nearly every set of an agb program is every address-taken
function; but they are wide, 73 callees on average, 18 of them 210 each.

### How it was measured

Three tools, on main with this unit, in `fnref.rs`; none prints or
changes anything unless its variable is set, and the word report is the
same with and without the first two.

- `GBAA_FNREF_WATCH=roads`: at each instruction that gives words their
  first use not allowed, a line: where, why, and what the instruction's
  operands held the words by (the value's own token, or one of its
  `Ref`s: a pile read, an every-cell read, a ROM read by index, a
  bounded ROM read), with the words. When two `Ref`s hold a word the
  first is named, pile before ROM.
- `GBAA_FNREF_WATCH=0x0875e6a8[,…]`: the branch's watch, brought over
  and made to look into `Ref`s: each place those words' addresses go,
  up to 400 places a word (a word left in a scratch register goes to
  millions: the first try printed 5.8 million lines).
- `GBAA_FNREF_OFF=bios,move,flags,tables,pile,cells`: kinds of hole
  made not to happen, as the branch's switch does for kinds of read (its
  kinds are others; the merge should keep both lists). For measuring
  only. `bios`: a BIOS call holes only the registers it reads. `move`:
  `x + 0` is `x`. `flags`: a carry, overflow or bit test is no hole.
  `tables`: a ROM read by index stops at the next object's start even
  between function words. `pile`, `cells`: those reads give nothing.
  `move` is a sound rule, and `bios` is for compiled code once it is
  settled what the registers hold after the call (below); the others
  are unsound, upper bounds. A report made with any of them says so in
  its first line. (`bios` is the rule since `native/fnref-bios-reads`
  and no longer a kind: "The BIOS rule, as landed", at the end.)

The run is the lift alone, no build: `just capped --mem 2G
target/release/gbaa-lift ROM OUT --name metroid --holes
mods/metroid/holes.toml`, about a minute, peak 326 MB.

### Where each word first goes wrong

The 734, by the first use not allowed the run found, from the roads and
a watch of 20 words, one or two from each row:

| words | road |
|---|---|
| 429 | left in r0 to r3 at a BIOS call that reads none of them: the `swi` in `VBlankIntrWait`'s wrapper (0x080051ce). 400 came out of three tables of handlers read by index (0x0875e6a8 and 0x0875e7b4, 256 words each as read, and 0x0875f974, 77), 29 out of a fourth (0x0808cefc, called through r1 at 0x08001ae0). For 0x0875e6a8: loaded at 0x0800a4c6 into r1 and called through r1 at 0x0800a4cc. GCC calls a pointer with `bl` to a `bx r1` stub, so the handler's address is still in r1 when the handler starts. It is then passed on and returned in r1 by functions that never touch r1, until one calls the wrapper |
| 99 | a register moved. Old GCC moves a low register with `adds rd, rs, #0`, and the flow follows an add that sets flags only as a compare, so an add of 0 is arithmetic. 77 at 0x0800535a and 13 at 0x08001416 are a call's result, r0 as a callee that sets none left it. The 77: the call at 0x08005326 has an empty callee set, so it takes back what every address-taken function returns in r0, and a handler called through r0 that returns nothing returns its own address. The 13: a table's handlers are called through r0 at 0x08000208, one of them is a bare return (0x08000c00), and that function is in the set of the call before 0x08001416 too. 8 at 0x080031f0 are a callback passed as a parameter, 1 at 0x0800cd54 |
| 77 | a read past a table's end. 64 at 0x08067f72 and 13 at 0x08061aae: a byte array read by an index that came out of the pile, so the run can't place the address and reads all the value sets bound it to, which is the index's width: 64 KB from 0x087600dc, 256 bytes from 0x0875fe58, with the bytes of every function word in them |
| 66 | a struct table read by one field. 33 at 0x0805b4f8: records of 24 bytes from 0x08345d00, the byte at +0 read by index, functions at +4 and +12 of each, so the read "from here on" gives their bytes too. 15 at 0x080609b4 (records of 16 bytes) and 18 at 0x08069dd8 (the byte passed on as a parameter first) the same |
| 39 | `cmp r0, #0` on a callback, in the three functions that set one and put in a default when given none (0x08000aea, 0x08000baa, 0x08000bea). The compare's carry counts as an ordered compare, and nothing reads it before the return |
| 13 | the pile and the cells. 5 at 0x0800124e and 1 at 0x0807aa80 by a load through a pointer loaded from memory, which reads every cell or the pile; 7 by a load at a constant RAM address, which reads its cell and the pile (0x0806054e, 0x080026c2, 0x08088f9c) |
| 5 | stored outside RAM, lost (0x080025d2, 0x080025e6, 0x080025fa, 0x08057dd0, 0x0808797a) |
| 4 | left in r2 at a jump to a set (0x0806a432, 210 callees when the run settles), one of which (0x0806c6b5) reads r2 as a parameter. Whether the table's real entries include it wasn't checked |
| 2 | 0x0800e008 and 0x03003bb4, not looked at |

So 429 + 99 + 4 are about registers and what GCC does with them, 77 +
66 about where a ROM table's entries are, 39 about flags, and 13 about
the memory model, which holds 36 of affine's 40.

### Kinds of hole switched off

Words not proven, of 804 (33 never read as built), by `GBAA_FNREF_OFF`:

| off | not proven | | off | not proven |
|---|---|---|---|---|
| nothing | 767 | | `bios,move` | 340 |
| `bios` | **338** | | `bios,move,flags` | 340 |
| `move` | 769 | | `bios,pile` | 276 |
| `flags` | 767 | | `bios,tables` | 273 |
| `tables` | 452 | | `bios,tables,pile` | 272 |
| `pile` | 324 | | `bios,move,flags,tables` | 274 |
| `cells` | 766 | | all six | 272 |

`bios` alone proves all 429 words of the first row and nothing else
moves: they had no other use not allowed. `move` alone un-proves two
words main proves (0x0807abc0, 0x08081e24: followed past the move, they
meet a hole) and reads two it didn't. `tables` and `pile` alone
reach most of the same words another way (with narrower callee sets, or
no callbacks out of the pile, the handlers left in r1 don't reach the
wrapper), and both are unsound as switched; with `pile` four complete
sets become incomplete.

After `bios`, `tables` or `pile` is worth about 63 more, the same words
either way (the reads past a table's end, whose index came out of the
pile), and `move` and `flags` nothing: a word let past the move meets
the next hole. With `bios,move,flags` the 77 results are held at the
compare after the move (0x0800535c: a value that may be a function or a
number, compared with 0, counts as compared with the number), and 34 of
the 39 callbacks at 0x08000b06, where the callback's global is loaded
and tested before it is called: a load at a constant address reads the
pile too, so the value holds the pile's numbers.

With all six off 272 are left: 44 never read (the switch cuts reads
short) and 228 held, which no switch here reaches: the 77 results of the
empty callee set, the struct fields (67 here), 45 at 0x08001910 (new
with the switches, not looked at), 13 lost outside RAM, and small ones.

### Which follow-ups a C game needs

In the order they'd pay on Metroid:

1. **A BIOS call holes only the registers it reads**
   (`native/fnref-bios-reads`, new; it was a clause of
   `native/fnref-live-registers`). 767 to 338 not proven on Metroid, on
   main, about twenty lines. `VBlankIntrWait` takes no arguments. What
   the rule must settle: the flow makes r0 to r3 unknown after a `swi`,
   so a register the BIOS leaves as it came loses its address there.
2. **A callee's results are only the registers it sets**, the rest of
   `native/fnref-live-registers`: the 77 + 13 results of callees that
   set no r0, and the handlers left in r0 to r3 by the call stub, which
   is what feeds row one. Its card says to work on the branch; nothing
   it needs is there (the summaries are on main).
3. **Where a ROM table ends** (`native/fnref-table-ends`, new). A table
   read by a byte index is read 256 entries long and one read by a
   halfword 64 KB, through its neighbours: C lays tables of handlers
   back to back, and `object_end` doesn't stop between function words,
   so that no table of handlers is cut short. That is the 77 words read
   past an end, the 256-word reads of row one, and the width of the
   callee sets (18 sets of 210; 162 with the switch). On the two larger
   games it holds over a thousand words each (below).
4. **Old GCC's moves and compares** (`native/fnref-gcc-moves`, new):
   `adds rd, rs, #0` is a move, and its carry and overflow are 0; `cmp
   x, #0` carries always and never overflows; a value that may be a
   function or a number compared with 0 is no hole; a flag nobody reads
   before the return is no use. All sound, all small, and worth nothing
   alone on Metroid (767 to 769): 138 words pass through one first, and
   they count once what's behind is cleared.
5. **Struct tables read by one field** (`native/fnref-table-fields`,
   new): a read by index at a known offset in records of a known size
   reads that field only. 66 words, left with every switch off. The
   frames unit has the same idea for arrays of pairs in a stack frame;
   only that half of it carries over.
6. **The pile split by global** (`native/fnref-pile-globals`, new; with
   it item 4 of the list above, a load at a constant RAM address reads
   its cell and not the pile). On Metroid at most 62 words after `bios`
   (338 to 276), the same ones `tables` reaches; on Nightmare and
   Emerald the first road of more words than anything else. C keeps its
   state in globals, and the value sets bound 8,582 of Metroid's stores
   to one global (3,989 to none), so a split by those bounds might pay
   here where it proved nothing on agb's heap stores.

Agb only, by this measure: frames, the object split and RAM pointers,
the heap by allocation site, call-site context, and the branch merge as
far as it serves those. The merge isn't needed for the tools any more:
the watch is on main. Whether a C game wants call-site context later
can't be told from first roads; nothing here asks for it.

One thing this doesn't measure. At an ordinary call the flow passes r0
to r3 and gives the callee unknowns in r4 and up, though since adfa45ee
a function may take r4 to r11 from its caller
(`native/fnref-taker-regs`). To the flow an unknown is never a
function's address, so a handler passed that way would be a use it
misses, not a word it holds, and no count here would show it. Two of
the 20 words watched do travel in r4 and up, at tail calls and falls,
which pass every register.

### Nightmare and Emerald

Both with `gbaa-recompile --summaries ROM --callee-sets` and the roads
(the ROM's code alone, no stand-ins), so only the words held are
counted, not the proven ones.

**Kirby: Nightmare in Dream Land** (5,177 functions). The flow settles
now, in 31 to 35 minutes at 2.0 GB (its holes file still skips it, as
not settling under 16 GB). 51 of 54 indirect calls have a complete set,
of 2,044 callees on average. 4,054 words are held, none at a BIOS call.
2,523 are first held by a pile read: 733 at 0x08006162, a byte out of
the pile passed as a parameter and shifted, 269 at 0x0800582a. 1,401
are held by bounded ROM reads of 64 KB, five byte arrays each read by a
halfword index: 1,260 at 0x0801c6b8 (the array at 0x08732ef0), 98, 23,
15 and 5 at four more (0x087336f0 to 0x08733af0): where a table ends.
With `pile` off (3 minutes, 503 MB) 1,599 are held, 1,396 of them at
0x08021c32 by the 64 KB read of the array at 0x087339f0: the pile is
worth at most 2,455 words there, and table ends most of the rest.

**Emerald** (15,159 functions). The flow stops at its token bound after
27,776 function runs (10 to 12 minutes, 2.4 GB, the same stop with the
roads on), so this is what it held when it stopped, 9,031 words, not a
count: a settled run would hold these and more, and may meet a word's
first hole elsewhere. By what gave the value: 3,701 a pile read, 2,013 a
ROM read by index, 1,209 a bounded ROM read of 64 KB, 2,107 the value's
own token, 1 an every-cell read. The sites over 150 words: 1,373 at
0x0801d874, an indirect call's result compared with 1 (item 2); 473 at
0x080a8fb2 and 458 at 0x0806a684, a parameter moved with `adds rd, rs,
#0` (item 4), the first the callback given to the function that makes a
task, the second a data pointer that a ROM table's leftovers reached;
394, 351 and 154 by 64 KB reads of byte tables (item 3); 333, 162 and
151 out of the pile (0x0800059e, 0x080135b8, 0x0817f504); 228 at
0x08098d72 and 208 at 0x0808f616 by ROM reads by index, not looked at.
One word is given to the BIOS.

## The BIOS rule, as landed

Unit `native/fnref-bios-reads`, 2026-11-13. Follow-up 1 above is the
rule now (`fnref.rs`, `bios_regs`), with the question it left settled:
what r0 to r3 hold after the call.

The flow follows the lifted program, so each call is what the lifter
prints for it (`native.rs`, `Plan::Swi`) and the prelude's routine,
which is the play build's (`runtime::bios`), read side by side. Three
things are said of each register: the call **reads** it (a function's
address there is a hole, given to the BIOS); it always **sets** it (a
number after, nothing of what it held); it **may set** it (after, what
it held or a number). A register in none of the three is as it came: an
address left there is followed on past the call.

| call | reads | sets | may set |
|---|---|---|---|
| `SoftReset` 0x00 (doesn't return), `Halt` 0x02, `Stop` 0x03, `SoundDriverVSyncOff` 0x28 | | | |
| `RegisterRamReset` 0x01, `SoundBias` 0x19 | r0 | | |
| `IntrWait` 0x04 | r0 r1 | r0 | |
| `VBlankIntrWait` 0x05 | | r0 r1 | |
| `Div` 0x06 | r0 r1 | r0 r1 r3 | |
| `Sqrt` 0x08 | r0 | r0 | |
| `ArcTan2` 0x0a | r0 r1 | r0 r3 | r1 |
| `CpuSet` 0x0b, `CpuFastSet` 0x0c | r0 r1 r2 | | r0 r1 r3 |
| `BgAffineSet` 0x0e | r0 r1 r2 | r2 | r0 r1 |
| `ObjAffineSet` 0x0f | r0 r1 r2 r3 | r2 | r0 r1 |
| `LZ77UnComp` 0x11, 0x12 | r0 r1 | | r0 r1 r3 |
| `HuffUnComp` 0x13, `RLUnComp` 0x14, 0x15 | r0 r1 | | r0 r1 |
| `MidiKey2Freq` 0x1f | r0 r1 r2 | r0 | |
| `MultiBoot` 0x25 | r0 r1 | r0 | |
| any other, and a number that is no call | all | all | |

Those are the 22 calls a native build can make
(`analysis::holes::NATIVE_BIOS`; a test holds the two lists together).
Every other call is a hard hole there, so there is no lifted call to
read: it holes and sets all four, as every call did before.

What decided the columns:

- **May set, not sets**, wherever some path leaves the register: the
  copies and LZ77 return at once on a source in the BIOS (so `CpuSet`'s
  r3 is 0x170 or as it came), a halfword `CpuSet` moves neither
  pointer, the affine sets leave r0 and r1 on a count of 0, `ArcTan2`
  leaves r1 on an axis. Every register in "sets" that the call doesn't
  read gets a constant or a number made from the ones it does.
- **`CpuFastSet`'s r3 is data**: the fill's word, or the first word of
  the last block it copied. So after the call r3 also holds what the
  copy read: the ROM words the flow already sends to the pile, or, from
  a source not in ROM, what a load the run can't place reads.
- **Where the console's BIOS overwrites what these leave** (r2 and r3
  in the waits, by its own code; r1 and r3 in most), the address is
  kept. The lifted program and the play build keep it, and kept is the
  safe side: the address is followed further than it lives on a
  console, and every use it meets is still judged.
- **A wait runs the IRQ handler**, which the BIOS's stub calls with
  r0 to r3 saved around it, and the lifted one as a function of its
  own: the waiting function's registers are as the call left them.

A test runs each of the 22 in the play build's BIOS from two sets of
registers and holds the table to it: a register outside "sets" and "may
set" comes back unchanged, and changing one outside "reads" changes no
other register and no RAM.

Left as it was, and filed (`native/fnref-bios-ram-copies`): the flow
follows a BIOS copy only out of ROM. One out of RAM moves no address,
and what the other calls read through a pointer is not looked at.

**Counts.** Metroid (the lift alone, about a minute, peak 312 MB):
`fn_words_proven` 37 to 466, `fn_words_not_proven` 767 to 338, as the
switch measured; `call_fn_calls` 70, `call_fn_complete` 68 and
`call_fn_four_complete` 37 as before. Keeping r2 and r3 past the waits
and r3 past the copies held no word the switch had proven.

The agb programs each had one word at stake, 0x08000178, the main
function's address, left in a register at a `swi` that reads none (in
affine `VBlankIntrWait`'s, at 0x0800088e). In twelve of the fifteen it
is proven now (`fn_words_not_proven` one lower, the bars with it). In
native-spike, native-tasks and native-bios-region the word is also
stored to the pile and meets arithmetic there, so the count stays and
its reason moves from "given to the BIOS" to "arithmetic".
