---
title: The C test ROMs in $GBAA_DATA are shared by every lane
hook: another lane's `just c-tests` rebuilds them from its own c/; bars over on c-* rows that you didn't touch: rerun `just c-tests` (3 s)
---

`just c-tests` writes `$GBAA_DATA/c/c-<name>-{agbcc,gcc}.gba`, one copy
for the machine, from the `c/` of whichever checkout ran it. A lane on a
branch from before a `c/` change (2026-11-24: native/lift-for-index's new
`c/loops/main.c`) rebuilds the old programs, and every other lane's
`just lift-check` / `just lift-metrics` then lifts them against bars made
for the new ones: dozens of metrics over on one C row (c-loops-gcc's
value_lets 189 over 137, "the bar … names no metric" lines), none on the
rest. Seen twice in one afternoon (12:31 new, 12:57 old again).

- Check: `ls -la --time-style=+%H:%M $GBAA_DATA/c/ | grep <name>` (size and
  time) against your branch's `c/` (merge main first if main changed it).
- Fix: `just c-tests` from your checkout (3 s), then rerun. It may flip
  again while a long lift-check runs; the bars step lifts first, within
  a minute of its start, so a rebuild just before starting it is enough.
