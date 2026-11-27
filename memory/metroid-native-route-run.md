---
title: Running Metroid's native route against a lift variant
hook: lift, three builds and route-check --native in one long job: 6 min on quack, so check a lifter change on a game
---

What checked `--fn-ids dense` on a game (native/fnref-dense, 2026-11-24, quack). One `just long start
NAME --mem 12G -- bash script.sh` ran it all in 6 minutes (the job's peak 849M; each crate build runs in
a scope of its own, sized by `just build-recompiled` from what is free):

```bash
ROM="$HOME/ROMs/gba/Metroid - Zero Mission (USA).gba"
target/release/gbaa-lift "$ROM" out/lift-metroid --name metroid --holes mods/metroid/holes.toml
target/release/gbaa-lift "$ROM" out/lift-metroid-dense --name metroid-dense --holes mods/metroid/holes.toml --fn-ids dense
just build-recompiled out/lift-metroid guest-pc
just build-recompiled out/lift-metroid-dense guest-pc
just recompile "$ROM" out/metroid --timing native
just build-recompiled out/metroid
target/release/route-check --native target/recompiled/release/gbaa-native-metroid \
  target/recompiled/release/gbaa-metroid units/metroid-native.toml --name metroid \
  --dense target/recompiled/release/gbaa-native-metroid-dense
```

- `--holes` must be given: with `--name metroid-dense` gbaa-lift would look for that name's file.
- The variant goes in as `--dense BIN` (records but RAM) or `--packed BIN` (records whole); a planted
  fault as `--planted BIN`. 2,700 frames.
- Records the same byte for byte doesn't mean the variant didn't run: Metroid's dense build kept every
  RAM byte (no function pointer it stores in RAM is proven), while the C programs' differ.
- The Amazing Mirror the same way (native/lift-libgcc-div, 2026-11-27): `gbaa-lift ROM OUT --name kirby-mirror
  --holes mods/kirby-mirror/holes.toml` (without `--holes` its [[copy]] functions are missing and the native
  build stops at frame 4, a call to 0x030017f0), `just build-recompiled OUT guest-pc`, the play build by
  `just recompile ROM out/kirby-mirror-play --timing native --name kirby-mirror-play`, then route-check
  --native … units/kirby-mirror-native.toml --name kirby-mirror: about 15 min under `--mem 12G`.
- A lifted function giving wrong values in a C program: `just fn-check ROM --fn-check KEY … --interp-every 1
  --frames 60` checks each against the interpreter and prints the first differing call's registers.
