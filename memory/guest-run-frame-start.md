---
title: A guest run never returns per frame
hook: what run_frame does at a frame's start must be in dispatch::switch too
---

The interpreter runs a frame at a time (`Machine::run_frame`), so what it
does at a frame's start it does every frame. A compiled play build's run
(`dispatch::run_guest`) never returns: at a frame end `dispatch::switch`
calls the front end and goes on. Anything added to `run_frame`'s start
must go in `switch` too, or the two part.

Seen 2026-11-21 (`native/metroid-route-again`): `Machine::catch_up` (the
line events a V-blank left due, from the commit that made every V-blank a
frame end) was at `run_frame`'s and `run_raw`'s start only. Metroid's
first immediate DMA crosses two V-blanks, so its compiled play build
walked the leftover lines at the code's next tick, after its stores, and
`route-check --native` failed before it compared the lift: "the play
build's compiled code differs from its interpreter: frame 2: audio: play
4 samples, interpreter 0". The agb programs and the Amazing Mirror never
leave events due, so `just check` and the Mirror's route stayed green.

A route's `--native` failure of that kind (compiled against `--interp`)
is the play build's, not the lift's: look in `crates/runtime` first.
