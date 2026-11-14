---
title: What a whole lift of each corpus game costs, and the holes-only one
hook: whole lifts 54 s-50 min at under 3 GB (not 16G: asking 16G blocks other jobs); holes-only 9 s-3.5 min under 0.6 GB (2026-11-14)
---

Measured on quack, 2026-11-14, commit 7f42cf22 (native/holes-match-lift),
holes files applied (`mods/<game>/holes.toml`), one job each:

| game | whole lift | peak | holes-only | --flow |
|---|---|---|---|---|
| Metroid | 30 s | | 9 s | |
| WarioWare | 283 s | | 14 s | |
| Kirby & the Amazing Mirror | ~50 min (heap pass 1,771 s) | 0.5 GB | 36 s | 1,054 s |
| Kirby: Nightmare (skip_fn_flow) | 54 s | 0.4 GB | 20 s | |
| moonbeam | 294 s | 0.15 GB | 35 s | |
| Minish Cap | 1,218 s (flow most of it) | 0.5 GB | 18 s | 1,076 s |
| Emerald Rogue | 1,838 s | 2.7 GB | 107 s | |
| Imperium | 1,641 s | 2.8 GB | 214 s | |
| Emerald | over 45 min, unfinished | | 60 s | |

- `just lift`'s 16G cap is far above any of these: `just long start …
  --mem 4G` is enough, and a 16G reservation queues every other job.
- Two runs of a 3G job at once on the same game waste a slot: a script
  looping over games keeps going after the game you wanted.
- holes-only skips the flow, the heap pass and the modules; the flow's
  callee sets add one summary hole each in Minish Cap and the Amazing
  Mirror (`--flow` finds them).
