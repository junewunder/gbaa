---
title: Emerald's function-address flow settles: what stopped it
hook: three kinds of junk value dripped tokens; GBAA_FNREF_GAINS shows them; keep Metroid byte-same
---

How it got here before 2026-11-19 (where the memory went, the Ref fix, the
first speed-ups): `fnref-flow-emerald-history.md`.

**It settles (2026-11-19, `native/fnref-flow-emerald`).** `gbaa-recompile
--summaries ROM --callee-sets` on pret's Emerald: 24 rounds, exit 0; 14 to
18 min wall with four lanes running (flow 838 to 977 s), 1.05 GB peak
(`/usr/bin/time -v`, under `just long start NAME --mem 4G`). 554 indirect
calls, 103 with a complete callee set (4.2 callees on average). Main before
it: round 0 alone took 684 s (it had been 31 s on 2026-11-13: the units
merged since made it worse), round 1 19 minutes, never settled.

**What stopped it, in the order found.** Each was a kind of value that
grew a few tokens a round, with nothing in it the run could use:
1. Pointers into code. Return addresses and the sources of code copies are
   ROM pointers that are no function's key; each instruction is its own
   "run of data" two bytes long, so pointers "from here on" never took
   each other in: 67,000 different ones, 447 million summed over the
   registers after round 0. Now `Tok::RomDull`, one token for a pointer
   into bytes that hold nothing followed (`dull`: instructions, and
   number-only data before code), past 16 of them in a value.
2. Junk pointers from wide ROM reads. A read "from here on" of the data
   region takes every word that looks like a ROM address for a pointer;
   by chance some point into literal pools, each its own run: 1,300 in
   one value, each load through it reading them all. Past 64 runs
   (`FROMS`) the value is a dropped pointer (`lost_rom`) and holds
   `Tok::RomAny`, which takes in every pointer after (without it the
   value fills and drops again each round: the review's catch).
3. Large function sets. Tasks and callbacks are thousands; 40,000
   registers each held a 4k-29k-token copy (900 million summed), each
   gaining a few a round. Past 1,024 (`LARGE`) they go to one shared set
   and the value keeps a `Ref` to it (`Large::take`), with its memory and
   ROM reads folded in and its ROM pointers given up.
Plus: a `Ref` that stands for nothing (every form dropped) is no token,
and `join_capped` says "changed" only when it did (a spurious "changed"
kept native-spike going for 256 rounds once RomDull existed).

**How to look.** `GBAA_FNREF_TRACE=1 GBAA_FNREF_GAINS=1`: each round prints
what entries and results gained by kind (`gains round N: 4909 result r0
+[] -[] lost`), the tokens by kind distinct/summed over all registers, and
the set with the most pointers "from here on" with each one's run length.
The summed counts named every cause above: read them first. `eu-stack -p
PID` sampled 60 times says where a round's time goes (`perf` can't open
events on quack). Write output to files (`> x.out 2> x.log`): a piped grep
loses its buffer when the cap kills the job. Copy the binary out before
rebuilding ("Text file busy").

**Checking a flow change costs nothing elsewhere.** `just lift-metrics`
(15 s) must print "0 over their bars, 0 under": the agb and C programs
never reach the three bounds. Metroid settles in 20 s: run main's binary
and yours with `GBAA_FNREF_TRACE=1` and `cmp` the outputs (but for the
seconds) and the `hole at` lines; its last round must read 17 rounds, pile
about 1,500, holes 273, 68 of 70 sets complete. What broke Metroid on the
way, each silently (7 of 70 complete): collapsing every pointer into code
at once (keep up to 16 exact: Metroid walks pointers through routines);
calling a collapsed pointer `& mask` dropped (Metroid masks junk values
with 0xffff0000 and ~0x20: now a number, and `& ~3` stays a pointer); and
making a code address a plain number (an unknown number as an address
reads all of RAM, a known code pointer reads nothing: native-tasks lost
every proven word).

**Tried earlier, dropped (2026-11-14):** in-round reruns, saturating large
sets with every form, one shared set of whole values (it dragged ROM
pointers into every large value: the shared set now takes functions'
tokens only), closing large sets' forms. None changed which entries
changed.

**Other games, same day** (`gbaa-lift --holes-only --flow`, main against
this): WarioWare 6 hard holes both (273 s to 116 s), moonbeam 7 both (682
s to 170 s), Emerald 13 with the flow as without. Nightmare with
`skip_fn_flow` taken out of its holes file: 0 hard holes in 217 s (main:
3 and no build, 25 min), because its pile-wide sets (976 callees) are now
large values whose calls count as incomplete: 1 of 54 sets complete, 51
on main. A complete set of a thousand callees was what made its holes.

**Left.** A round is still 100 to 200 s while the pile grows (rounds 1 to
4): half of it joins in `give_all` through callee sets of thousands. 451
of Emerald's 554 sets are incomplete (`lost_rom`: ROM reads cut at 256 KB,
and the bounds above); making them sharper is flow-precision work, not
this unit's. One flag says both "may be a function the run dropped" and
"may be a ROM pointer it dropped": a call through a large value that only
lost pointers would still be complete with two.
