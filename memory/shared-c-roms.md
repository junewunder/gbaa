---
title: The C test ROMs in $GBAA_DATA are shared by every lane
hook: another lane's `just c-tests` from an older branch swaps them under you; build main's into a symlinked private data dir (2026-11-24)
---

- `$GBAA_DATA/c/*.gba` is one copy for every lane on a machine. A lane on
  an older branch that runs `just c-tests` rebuilds them from its own
  `c/` source: on 2026-11-24 c-loops went back to before
  native/lift-for-index, so main's `names/c-loops-*.toml` (by sha1) no
  longer applied and lift-metrics failed with "the bar … names no metric".
- Check: `sha1sum $GBAA_DATA/c/c-loops-gcc.gba` against `rom_sha1` in
  `names/c-loops-gcc.toml`.
- Don't rebuild the shared copy under another lane. Make a private data
  dir of symlinks to everything but `c/`
  (`for e in ~/.local/share/gbaa/*; do [ "$(basename $e)" = c ] || ln -s $e /tmp/d/; done; mkdir /tmp/d/c`),
  then `GBAA_DATA=/tmp/d just c-tests` and run `GBAA_DATA=/tmp/d just
  lift-metrics` / `just long start … -- env GBAA_DATA=/tmp/d just lift-check`.
  The bars step of `just lift-check` lifts the C programs within a minute
  of its start, so the private dir matters most then.
