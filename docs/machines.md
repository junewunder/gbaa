# Machines

The only place facts about this factory's machines live: which machines run
it, how many lanes each runs, the account each app runs under, and what each
machine can do. Every other doc points here instead of restating them. The
overseer reads this file at the start; each loop session gets its own
machine's section (`## <machine>`) in its prompt. When the owner says
anything about a machine or an account, it goes in here and is committed
before anything else happens.

`init.sh` wrote the table from the owner's profile (`scripts/profile.sh
machines` in the skill). One machine with one lane is the normal case; more
machines are opt-in (the skill's docs/scaling-out.md).

## Lanes

The scripts read this table (`factory/machines.sh`); keep its columns.

| Machine | ssh | Lanes | Takes | Checkout | Can | Cache | Heavy | Reserve | GPU |
|---|---|---|---|---|---|---|---|---|---|
| quack | quack | 2 | any | ~/Documents/homelab/gbaa | — | 100G | 2 | 3G | 0 |
| pyon | pyon | 0 | any | ~/Documents/homelab/gbaa | — | 100G | 1 | 3G | 0 |

- **Machine:** its short hostname (`hostname -s`). A unit pinned with
  `--host <machine>` runs only there.
- **ssh:** the address `just fleet` reaches it at from the other machines.
- **Lanes:** how many loops run on it at once, each taking any unit the
  machine can do. Start at 1; raise it once there are tokens and CPU for
  more. 0 means the factory never runs there.
- **Takes:** `any` (every unit this machine can do) or `pinned` (only units
  pinned to it with `--host`, leaving the rest to the other machines).
- **Checkout:** the repo on that machine, where the owner and the overseer
  work; it stays on main. With an origin, each lane runs in a plain clone
  beside it (`<checkout>-lane1`, `-lane2`, …; never a worktree) whose
  `work/` links to the checkout's, so a machine keeps one `work/`. A project
  that needs more in a fresh clone (a virtualenv, generated stubs) does it in
  `factory/lane-setup`.
- **Can:** what only some machines can do, as short names separated by commas
  (e.g. `gpu`), or `—`. A unit that needs one (`just tracker edit <id>
  --caps gpu`) goes only to a machine whose Can lists it. This, not pins,
  is what ties work to a machine. Each name is explained in the machine's
  section below.
- **Cache:** how much the groups' data copied down from the store may take on
  this machine, e.g. `100G`, or `—` for no limit. Only matters with a store.
  Inputs aren't counted. Over it, the loop deletes the least recently used
  groups that are safe on the store before taking a new lease
  (`factory/store.sh`).
- **Heavy:** how many heavy jobs (`just capped`, `just long start`) run on
  it at once, across all its lanes; the rest wait their turn. Two lanes'
  Blender sweeps plus an editor on one 8-core laptop ran about 4 times slower
  than one sweep alone, so it's 1 unless the machine has the cores for more.
  Blank means 1. A command that barely uses the CPU skips the queue with
  `--light`.
- **Reserve:** memory kept back from the heavy jobs for the desktop and the
  sessions, e.g. `3G` (blank: 3G). The rest of the machine's memory is the
  budget the heavy jobs (`just capped`, `just long start`) reserve their
  shares from, across all its lanes; `just heavy` shows it. No heavy job
  starts while the memory available is under the reserve plus GPU.
- **GPU:** memory no job's cap counts that the machine needs anyway, measured,
  e.g. `6G` for an editor's GPU buffers (blank: 0).

## The ROM store

june's NAS holds the ROM sets. Where it is and how to reach it are in
`docs/machines.private.md` (not in git; on quack only). Lanes don't copy
from it. ROMs never go in git.

## quack

- **Accounts:** none recorded
- **Can do:** anything any machine can
- **Cache:** none used (no store); `$GBAA_DATA` is shared by both lanes
- **Notes:** 24 cores, 30 GB RAM of which ~10 GB is usually available; 7 GB
  swap, often full. Two lanes, june's choice (2026-10-20); raise to 3 if
  build-lock waits stay under ~10% of unit time. Heavy 2, june's choice
  (2026-11-05): 24 threads, and the memory budget gates each job. Each lane clone has its own
  `target/`.

## pyon

- **Accounts:** none recorded
- **Can do:** anything any machine can
- **Cache:** none used
- **Notes:** off (0 lanes). It has no `$GBAA_DATA` or ROMs yet.

## deck

june's Steam Deck, for play tests only (not a lane: no row in the table).
A lane may use it only after asking june each time and getting a yes. How to
reach it, its accounts and how games get there are in
`docs/machines.private.md` (not in git; on quack only).
