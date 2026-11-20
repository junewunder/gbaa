# Retiring the mod view: what goes, what stays

For the mod view's retirement (unit `mod/retire`; june, 2026-11-19: deprecate
it once the lift has its wins). The parity table (`docs/mod-view-parity.md`)
is settled: its fourteen gaps are in the lift and the owner left the other
seven out (2026-11-20). This is the list june is asked to approve before
anything is deleted. Written 2026-11-20 from main at that date.

## What goes

**gbaa-recompile's mod-view modes** (`crates/codegen/src/bin/gbaa-recompile.rs`):
`--readable LIST`, `--mod DIR`, `--cut DIR FUNCS`, `--names-check`,
`--snapshots … --readable`, and naming the play build with `--names` /
`--names-dir` (only the readable and mod prints use names; the play build
is unnamed).

**Codegen modules, whole:**
- `snapshot.rs`'s readable snapshots, `find.rs` (`just mod-find`).
- `names/check.rs` (the named-vs-unnamed token walk) and its tests.

**Codegen, the readable parts of shared files:**
- `emit.rs`: `readable_items`, `readable_impl`, `readable_macros` (the
  `tick!`/`call!`/`ret!`/`regs!` macros) and the `readable` argument threaded
  through emit.
- `lower.rs`: `readable_plan`, `exit_readable`, `readable_blocks` and the
  readable printing of blocks.
- `func.rs`: the `readable` fields.
- `print.rs`: the readable printer, less the helpers the lift calls (below).
- `modcopy.rs`: the mod-copy cut and `--mod` build, less `merge_file` (below).
- `text.rs` where only the readable print uses it.
- `fn_diff.rs`: the "readable form of the picks" configuration.

**Other crates:**
- `crates/naming` (the naming loop; its gate is parked and the owner left it
  out of the lift).
- `crates/route-check/src/mod_run.rs` (`--mod-run`; the lift's
  `lift_mod_run.rs` stays).
- `crates/hole-report/src/nightly.rs`: the `readable` step of the nightly run.

**just recipes:** `readable-suites`, `readable-states`, `names-states`,
`mod-cut`, `mod-find`, `mod-states`, `mod-run`, `snapshots-readable`,
`naming`, `timing-modes` (the mod view without timing); the `--readable`
option of `sst-gen`, `fn-diff` and `own-state-check`; the mod-view cut in
`kirby-controller` (its trace and sprite tables stay; the cut moves to
`gbaa-lift --cut` or goes).

**Docs:** `docs/mod-view-design.md` gets a "retired" header and stays as the
record (or moves to `docs/research/`); the mod-view entries in
`docs/tools/commands.md`; mod-view paragraphs in `structured-codegen-design.md`
and `native-port-design.md` reworded to point at the lift. Research notes
(`agent-first-mod.md`, `mod-view-without-timing.md`, `naming-*.md`) stay as
history. `docs/decisions.md` records the retirement.

**Private (mods repo):** `mods/emerald/readable/`,
`mods/emerald/snapshots/emerald-readable.*`, and the mod copies below.

## What stays (the lift or the play build uses it)

- `codegen::names` (`SourceMap`, `sha1_hex`, `Home`, `Local`), `types.rs`,
  `dwarf.rs`, `io_regs.rs`: the lift names, types and comments from them.
- `lower.rs`'s structuring pass and the helpers the lift calls
  (`word_texts`, `word_note`, `counted`, `Sets`/`Val`/`known`,
  `MAX_INDENT`).
- `print.rs`'s `is_bit`, `linear`, `word`; `text.rs`'s `Token`/`Table`
  (lift text comments); `modcopy::merge_file` (the lift cut's three-way
  re-cut merge). These move to a small shared module if the rest goes.
- `crates/pret-names`, `crates/xmatch`, `just pret-names`,
  `just decomp-names`, `just library-names` and `$GBAA_DATA/names`: `just
  lift` reads the maps.
- The play build (`gbaa-recompile` without the flags above): `variants.rs`,
  `seen.rs`, `--timing`, and `locals.rs` behind `--locals` (kept off, the
  owner's call of 2026-10-26).
- `crates/insn-text-check` and armv4t's instruction text: `just dis` uses
  them.

## The mod copies

| mod | game | lift today | proposed |
|---|---|---|---|
| half-speed | Kirby & the Amazing Mirror | the lift reaches play and matches the play build (`native/mirror-native-route`, landing) | port to a lift cut (`gbaa-lift --cut`) |
| text-speed | Emerald | the lift doesn't build yet (`native/emerald-builds`) | drop, or wait for Emerald's lift |
| main-menu | Emerald | as above | drop, or wait |
| spike-hand, decomp-spike | Emerald | spikes, not mods | drop |

The card's gate, "not before a commercial game she mods runs as a native
build": Kirby & the Amazing Mirror (half-speed's game) runs natively into
play once `native/mirror-native-route` lands; Emerald doesn't yet.
