---
title: Why the function-address flow runs out of memory on Emerald
hook: round 0 alone passes 10 GB: callee entries each copy 30k-token pile reads; next, chunked sets or a symbolic pile token (2026-11-07)
open: true
---

Measured 2026-11-07 on quack (`native/fnref-flow-emerald`), with `gbaa-recompile
--summaries ROM --callee-sets` built from the indirect-call-args branch (the only
place `--callee-sets` exists; `fnref.rs` is the same file on that branch and on
main, so a worktree of that branch with `fnref.rs` symlinked to the lane's copy
measures the lane's changes; build it with `CARGO_TARGET_DIR` set to the lane's
`target/`).

- The flow never finishes **round 0**: killed at 10-12 GB about 3-4 minutes in,
  about function 0x080d8b39 of 15,158. `GBAA_FNREF_TRACE=1` prints every hole
  with its operands, 160 MB of text by then: silence that print before tracing.
- The pile reaches about 30,000 tokens (function words from Emerald's ROM tables,
  read "from here on" and stored through heap pointers, in 3-4 forms each), so
  every load through a heap pointer is a value of that size.
- Where the memory is (distinct tokens kept, counted by `Rc` pointer): the
  functions' **entries** hold 100-180 million, everything else under 10 million
  (results 8M, cells and spills 3M, kept cell reads 2.5M, by-value memos 1.3M).
  Each entry slot holds its own copy, joined from a different version of the
  pile read. Every pile change makes a new read, so the old and new reads are
  different sets, and joining two different large sets flattens into a copy.
- One call alone, `UnkUtil_Run` (0x080d2aa5, a loop calling `data[i].func`
  through the pile), adds 2.3-3.2 GB: its about 10,000 callees each get the
  30k-token argument joined into their own entry.
- What I tried, and what it did:
  - A join whose result equals the other side shares that side's set. This
    helped a little (4.4 GB where 6.9 GB was, at the same function), and
    round 0 still didn't finish.
  - A two-level set (a shared base plus a few extra tokens) made no real
    difference, because two different large bases still flatten.
- Next: share structure when two large sets are joined. Either keep a set as a
  short list of shared, disjoint chunks (each pile version is the last one plus
  a new chunk), or make a pile read one symbolic token `Pile(w)` that is
  expanded only where tokens are looked at: holes, callees, load and store
  addresses, ops (whoever expands it sets `rd.pile`, so it reruns when the pile
  grows). Also give a call through a large callee set one shared argument
  pool per site, joined into each callee's entry when it runs (as `loose_args`
  is), not a copy per callee.
