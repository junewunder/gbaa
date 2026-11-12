---
title: Why the function-address flow ran out of memory on Emerald, and the Ref fix
hook: big reads (pile, every cell, ROM tables) are now one Tok::Ref; round 0 holds 1.5 GB but runs ~60 s per 500 fns: speed is next (2026-11-08)
open: true
---

**How to measure** (quack). `--callee-sets` is on main since 2026-11-08
(indirect-call-args landed): `just long start … --mem 9G -- bash -c
'target/release/gbaa-recompile --summaries ROM --callee-sets …'` (copy the
binary out first if you'll rebuild while it runs: "Text file busy"). Run it as
`just long start … --mem 9G -- bash -c '… > /tmp/x.out 2> /tmp/x.log'`
(write to files: a piped grep loses its buffer when the cap kills it).
`GBAA_FNREF_TRACE=` (empty) still turns tracing on: use `env -u`.
A progress print every 500 functions (RSS from /proc/self/statm, token counts
of entries, rets, cells, by Rc pointer) found each source below fast.

**Where the memory went, in the order found (2026-11-07/08).** Each was a
set of 10-30k tokens, made again at each store, that every function entry
joined into its own copy:
1. pile reads (a load through a heap pointer);
2. every-cell reads (a load from an unknown address);
3. compares of those (`Cmp` forms: 40% of what was left), and `lsl/lsr #16`
   u16 zero-extension (`LoUp`/`Lo` forms);
4. ROM table reads by an unknown index ("from here on" up to next code):
   struct tables holding function pointers, read by byte fields (`Part`);
5. words stored narrower (`strh`/`strb` copies) expanding into cells.

**The fix (b6d5ed3f).** A large read is one token, `Tok::Ref { src, xf }`:
`src` is Pile(w), Cells(w), RomFrom(a, w) or Hull(lo, hi, w); `xf` is a
per-form map (shift, mvn, compare, narrower load or store composed with
`Xf::then`). Memory stores Refs; whoever looks at tokens expands
(`G::expand`/`ref_val`, cached until memory changes, ROM-only ones for
good) and so reads that memory (`rd` bits: reruns when it grows). Under 64
tokens a Ref is expanded where an op makes it, so the agb programs come
out as before. Entries at 8,500 functions: 5.6M tokens (135M at 7,000
before); RSS flat at 1.5 GB.

**Open:** round 0 is slow (about 60 s per 500 functions late on); suspects
are per-op scans of 30k-token Refs (`split_refs`' rest, `cmp_refs`'
difference, `hole` on fresh expansions, `ref_make` redone after each
store). Unit: native/fnref-emerald-speed.
