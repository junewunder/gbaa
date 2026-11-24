---
title: What a whole lift of each corpus game costs, and the holes-only one
hook: whole lifts 1-30 min under 3 GB (ask no 16G: it blocks other jobs); holes-only 9 s-3.5 min, 0.6 GB
---

Measured on quack, 2026-11-14, commit 7f42cf22 (native/holes-match-lift),
holes files applied (`mods/<game>/holes.toml`), one job each:

| game | whole lift | peak | holes-only | --flow |
|---|---|---|---|---|
| Metroid | 30 s | | 9 s | |
| WarioWare | 283 s | | 14 s | |
| Kirby & the Amazing Mirror | ~50 min (heap pass 1,771 s) | 0.5 GB | 36 s | 1,054 s |
| Kirby & the Amazing Mirror, flow on (2026-11-24, lift+build+run) | 6 min | 0.76 GB | | |
| Kirby: Nightmare (skip_fn_flow) | 54 s | 0.4 GB | 20 s | |
| Kirby: Nightmare, flow on (2026-11-14) | 491 s | 0.45 GB | | |
| Kirby: Nightmare, flow on (2026-11-20, lift+build+run) | ~4 min | 0.44 GB | | |
| moonbeam | 294 s | 0.15 GB | 35 s | |
| Minish Cap | 1,218 s (flow most of it) | 0.5 GB | 18 s | 1,076 s |
| Emerald Rogue | 1,838 s | 2.7 GB | 107 s | |
| Imperium | 1,641 s | 2.8 GB | 214 s | |
| Emerald (25fa1c4e, 2026-11-17; flow stops at its bound) | 1,358 s | 3.0 GB | 60 s | |
| Kirby & the Amazing Mirror (0711e3d2, 2026-11-20, lift+build+run) | 3 min | 0.66 GB | | |
| Metroid (0711e3d2, 2026-11-20, lift+build+run) | 1 min | 0.28 GB | | |
| Kirby: Nightmare (0711e3d2, 2026-11-20, lift+build+run) | 4 min | 0.43 GB | | |
| Kirby & the Amazing Mirror, typed (24f4b797, 2026-11-20): lift / release build | 79 s / 53 s | 0.48 GB / 4.1 GB | | |
| Emerald, named and typed (55cc3c8c, 2026-11-20, `just lift`: lift+build+run 600 frames) | ~22 min (lift ~13, build+run ~9) | under `just long --mem 12G`; the build under its own 10 GB cap | 80 s | |

- The Mirror's release build with katam's types (74,751 named-field
  accesses, native/mirror-typed-build; `just build-recompiled OUT guest-pc`
  under `/usr/bin/time -v`, 6 jobs): 53 s wall, 105 s user, peak 4.1 GB
  (the largest rustc), far under the 12 GB once feared; then 600 of 600
  frames, no stop. `just long start … --mem 6G` covers lift and build.
- `just lift`'s 16G cap is far above any of these: `just long start …
  --mem 4G` is enough, and a 16G reservation queues every other job.
- Two runs of a 3G job at once on the same game waste a slot: a script
  looping over games keeps going after the game you wanted.
- holes-only skips the flow, the heap pass and the modules; the flow's
  callee sets add one summary hole each in Minish Cap and the Amazing
  Mirror (`--flow` finds them). Nightmare's (MPlayMain's entry r8 left
  in r0, native/nightmare-fn-flow-again) went with native/fnref-flow-emerald
  (a pile-wide set is a large value, its calls incomplete): its holes file
  has no skip_fn_flow since native/nightmare-flow-on (2026-11-20; 0 hard
  holes, builds, 600 of 600 frames as with the flow off). Mirror's likewise
  since native/mirror-flow-on (2026-11-24): the flow that took 37 min
  on 2026-11-14 now takes the whole lift+build+run 6 min.
- `just holes-all` over the nine took 29 min at 0711e3d2 (2026-11-20, two
  runs; Imperium 9 min, Rogue 5). Its first-stop
  column reads the lifts when it runs: `--from DIR` keeps DIR's saved stops,
  so lift first, then run it.
