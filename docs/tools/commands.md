# Commands and tools

Moved out of AGENTS.md to keep it under the factory's line cap. Each entry
says what a recipe or tool does and what it must still pass.

`gbaa-lift`, `gbaa-recompile`, `gbaa-holes`, `gbaa-lift-check` and
`route-check` (but `--args` and `--missing`) refuse to run, exit 2, unless
under a memory cap of their own: the recipes cap each run with the
justfile's `cap` (`{{cap}} MemoryMax=8G …`, a systemd scope with no swap;
gbaa-lift 16G, gbaa-holes 3G); inside a factory job (`just capped`,
`just long start`) `cap` makes no scope, so the tool stays under the
job's cap and in its peak. A child spawned with `Command` runs in its
parent's scope. Run one
by hand through `just capped`, `just long start` or that systemd-run line;
`GBAA_UNCAPPED=1` skips the check, for a small input you've measured
(`analysis::cap`).

- `just sst` / `just sst-ir`: SingleStepTests against the interpreter, and
  through the IR builder and evaluator (also against the interpreter). Both
  print the per-file table and fail on any change from `EXPECTED` in
  `armv4t/tests/common/sst.rs`, the harness they share.
- `just gba-tests`: jsmolka's `arm`, `thumb`, `memory` and `nes` in the
  interpreter on the runtime; prints `pass` or `fail N` (r12, the failing
  test's number, from its `.asm`) for each and fails on any fail.
- `just armwrestler [interp|recompiled|both]`: walks all eight pages of both
  armwrestler builds headless with `--input`, through `gbaa-desktop` and the
  recompiled build (`just recompile` + `run-recompiled`, so heavy), dumps
  them to `out/armwrestler-pages/`, and has `aw-check` read the text back
  with the ROM's font. Prints codegen's counts and the fallback's. Fails
  on any BAD line, a short page (79 results in all), or a change in the
  original's known failure (its ARM V5TE item has no tests).
- `just disc-truth [ELF]` / `just disc-score FILE [SHOW] [TRUTH]`: the
  discovery-accuracy check (decision 5). Writes
  `$GBAA_DATA/truth/<name>.funcs.tsv` from any ELF (default the pokeemerald
  one), then scores a discovered-starts file against a truth file (default
  Emerald's). `just check` doesn't need them.
- `just dis ROM ADDR [LEN] [--arm] [--names FILE]`: the disassembler,
  `gbaa-dis` (`crates/codegen/src/bin/gbaa-dis.rs`). LEN bytes (hex,
  default 0x40) at ROM address ADDR (hex, 0x08000000 and up), one
  instruction a line as `080004c6: f00a ffa1  bl 0x0800b40c`, in
  `armv4t::text`'s objdump UAL form (`just insn-text` checks it); a
  Thumb `bl` pair is one line. Thumb unless `--arm`; an odd ADDR is
  Thumb, the bit stripped. `--names FILE`, a names map for this ROM
  (`$GBAA_DATA/names/<game>.toml`, loaded as `codegen::names` does, so
  its `rom_sha1` must match), prints `<name>:` at each named function's
  start and `<name>` after a branch to one. Paths are from where it's run;
  ROM only (no save states). Light: no cap.
- `just insn-text`: the check of `armv4t::text`, the assembly
  `gbaa-recompile --asm-text` prints in `insn!` and `--asm-comments` as
  `// 0x0805_9810 push {r4, lr}` (`Config::insn`; the readable form prints
  no `insn!` without one of them). What the markers cost the build
  (Emerald named, every function readable with `all.list`, quack,
  2026-10-30, one build each, deps warm): with `--asm-text` 3,969,472
  lines (815,356 of them in `insn!`), 291 s wall and 1,629 s CPU; without,
  3,154,111 lines, 270 s and 1,517 s: about 7% of the build.
  `insn-text-check` runs arm-none-eabi-objdump 15.2 from
  `$GBAA_DATA/toolchains/` (`-m armv4t`) and compares, after
  normalising both sides (comments, APCS names, number bases, `.n`, stack
  names of `ldm`/`stm`, a repeated first register): jsmolka's arm.gba and
  thumb.gba at every discovered instruction and at every word/halfword of
  the image, and every instruction of `pokeemerald.elf`. Not compared, and
  counted: words our decoder calls undefined (objdump knows later
  architectures), objdump's Thumb-2 (`it` blocks, 32-bit pairs), and
  encodings with should-be bits off (`canonical_arm`), which objdump reads
  strictly. Fails on any other mismatch. About 5 s.
- `just holes ROM [--frames N] [--press KEY@F-G/E]… [--no-summaries]
  [--no-run-roots] [--show N]`: the hole report
  (`docs/research/native-holes.md`): discovery, a 5,400-frame
  instant-timing interpreter run with the research press script,
  discovery again with the ROM targets of the run's calls through a
  register as function starts (`discover_with`; `--no-run-roots` skips
  it), the function summaries, the RAM copies found with the run
  (`analysis::holes::ram_copies`: BIOS copies given the run's entries,
  copies by DMA or the game's loops found by their bytes), then
  `analysis::holes::classify`. Prints one ranking line (hard holes by
  kind; bytes reached but not discovered; busy loops that call
  functions; the flash library's stack routines stood in for,
  `analysis::flash`) and each list, the RAM copies among them (the ones
  only the run shows also as `[[copy]]` tables for the lift's holes
  file), and last the runs of ROM code run but not discovered, with what
  discovery has around each (code before it, data over it). About a
  minute a game, capped at 3 GB. The
  doc's table is these lines; rerun the corpus after changing
  `analysis::holes` or `crates/hole-report` and update it.
  `--cover FILE` (`holes/coverage/<game>.toml`, native/hole-coverage)
  adds the file's coverage runs after the press script (reset, the
  owner's saves under `$GBAA_DATA/saves/`, Emerald's story saves and TAS
  states, each with seeded random presses), classifies against them all,
  and checks the file's `[[jump]]` list (each unresolved jump: the runs
  that reached it, its targets) against what they saw, failing on a
  difference; `--update` rewrites the list instead. Deterministic, so a
  rerun matches. 10 to 30 minutes a game: run it as a long job.
  `--line FILE` writes the ranking line's numbers to FILE as TOML.
- `just holes-all [--out DIR] [--against DIR2] [--only NAME]… [--cover]
  [--write-doc FILE] [holes flags…]`: the hole report over the corpus, `holes/corpus.toml`
  (each game's short name, ROM file name under the justfile's `roms`,
  coverage file), `gbaa-holes --corpus` (`crates/hole-report/src/corpus.rs`).
  Runs each game in turn as its own `gbaa-holes ROM … --line`, writing
  `DIR/<name>.txt` (the report), `DIR/<name>.log` (its timings), then
  prints one table, a row per game and a total: hard holes, ROM and
  overlay jumps, return arithmetic, RAM code changed, CPU state, BIOS,
  calls to non-entries, summaries, bytes not discovered, seconds. The
  numbers come from the report's own counts, saved in `DIR/summary.toml`
  (`[[game]]` each). `--against DIR2` reads DIR2's `summary.toml` and
  shows each change in brackets, `4 (-3)`, `10 (0)`. DIR defaults to
  `out/holes/<git short hash>` (`-dirty` with tracked changes). Other
  flags (`--frames N`, `--no-summaries`, `--press …`) go to every game;
  `--cover` gives each its coverage file (hours, not minutes). Fails if
  any game's run did. One 3 GB cap, no swap, for the whole run (the games
  run one at a time). About 12 minutes for the nine (Imperium the longest, 3 minutes): run it
  with `just long start holes -- just holes-all`. Use it, not a loop over `just
  holes`, to compare before and after a change: run it on main, then on
  the branch with `--against` the first DIR. `--write-doc
  docs/research/native-holes.numbers.md` also writes the table as that
  generated numbers file (header, date, commit); `--from DIR` runs
  nothing and renders DIR's saved `summary.toml` (with `--write-doc`, the
  file), for a run already made at the commit wanted.
- `just softfloat-check ROM [--cases N] [--pins]`
  (`gbaa-softfloat-check`, `crates/hole-report/src/bin/softfloat_check.rs`):
  the runtime's stand-ins for libgcc's `__muldf3` and `__divdf3`
  (`runtime::softfloat`) against the ROM's own copies, found by
  `analysis::softfloat::scan` and run in the interpreter: every pair of an
  edge grid (zeros, denormals, the normal range's ends, infinities, quiet
  and signalling NaNs), then N (default 50,000) random pairs of each kind
  (any bits, results near the denormals, near overflow, short mantissas).
  Prints each copy's count of differences and the first few; fails on
  any, or on a ROM with neither. `--pins` prints a smaller grid's results
  as `runtime::softfloat`'s test table holds them. About 2 s a ROM
  (moonbeam, Emerald Rogue and Imperium have them). Light.
- `just jt-score`: the jump-table check (decision 5c). Builds truth tables
  from `$GBAA_DATA/src/pokeemerald/pokeemerald{,_modern}.elf` and the ROMs
  beside them, runs `analysis::jumptable` over each ELF's code map, and
  prints exact/subset/wrong/missed. Fails below 676/676 (agbcc) or 610/617
  (modern GCC) exact, on any wrong target, or on a subset beyond the four
  checked by hand (the justfile names them). It also scores the far-jump `bl` rule (5a) on every
  Thumb `bl`, failing below 548/548 or 726/732 or on any false one.
  `jt-check ELF --trace ADDR` prints the value-set states around an address
  and `--dump FILE` every jump's result, for debugging.
- Code discovery (`analysis::discovery`, decision 5): `just discover ROM`
  writes `$GBAA_DATA/disc/<name>.starts` in the accuracy check's format and
  prints the counts. `just disc-pret [pokeemerald_modern]` runs it on a pret
  build and scores it: precision/recall (Emerald fails below the gbadisasm
  baseline, 0.9987/0.9349), then `jt-check --discover` (tables, far jumps
  and code coverage from discovery, no bar). `just disc-all` runs it on
  every ROM in `~/ROMs/gba/`. `GBAA_DISC_LOG=1` prints each round.
  `just swis [DIR]` (`disc-check swis ROM...`) lists every `swi` in each
  ROM's discovered code (IWRAM copies included) by call number, with its
  sites and whether the runtime implements it
  (`runtime::bios::call::IMPLEMENTED`); about a minute over `~/ROMs/gba/`.
  Its table: `docs/research/bios-calls.md`.
- Mod-view names (decision 2, docs/research/pret-names.md): `just fetch-pret`
  clones pokeemerald, pokefirered and agbcc pinned by commit
  (`scripts/fetch-pret.sh`) into `$GBAA_DATA/src/` and builds
  `pokeemerald{,_modern}.elf` and `pokefirered.elf` (their ROMs must match
  Emerald's and FireRed (U) 1.0's SHA1s; no ROM is fetched); a no-op when
  built. `just pret-names [pokeemerald_modern|pokefirered] [SHOW]` matches the ELF's symbols to
  codegen's function entries, the IWRAM copies and the data the pools point
  at, writes `$GBAA_DATA/names/<build>.toml` and prints the report, then
  loads that file as codegen's source map (`codegen::names`) and fails
  unless every function gets an identifier of its own. It also reads the
  ELF's DWARF 2 (`pret_names::dwarf`, gimli, agbcc's unterminated abbrev
  tables and short line programs mended first): `src` and `proto` on
  functions, `type` on data, `[locals]`, `[lines]` and `[types]`, and a
  `dwarf:` report line; and from the tree's `include/constants/` and
  `src/`, `[values]`: the `#define` families fields take
  (`docs/mod-view-design.md`, "Value names"), with a `values:` line; and
  the tree's `charmap.txt` as `[text]`, for the readable print's string
  comments ("Text"), with a `text:` line (none for a game
  `crates/codegen/text-tables.toml` lists: its own table loads at print
  time, "Other games' tables"). `target/release/pret-names --dwarf ELF [NAME…]`
  prints what DWARF gives, and each named function's or type's entry. About
  25 s and 400 MB on Emerald (discovery is most of it); the file is 14 MB.
- Songs out of a ROM (decision 2, docs/asset-extraction-design.md,
  Music): `just assets-songs [ROM] [OUT]` runs the ROM 300 frames in the
  interpreter with `--m4a-log` (each m4a player's song header when it
  changes, found from the word at 0x03007FF0, no game addresses), then
  `gbaa-assets songs` scans the ROM for the song table holding the first
  header played, decodes every song but the table's dummy and writes
  `OUT/songs/<name>.mid` (names from `$GBAA_DATA/names/pokeemerald.toml`)
  and `OUT/manifest.toml`'s `[songs]`. About 3 s. `just assets-check-songs`
  puts each `.mid` through pret's mid2agb with `midi.cfg`'s flags and
  compares the assembled bytes with pret's built `.s` (exact), then the
  decoded command streams (equal as events); it writes `OUT/pret-check.txt`
  and fails below 530 exact or 530 equal, of 530, or if pret's `.s` doesn't
  assemble to the ROM's bytes (the assembler's own check). About 2 s.
- Samples, voicegroups and soundfonts (docs/asset-extraction-design.md,
  Samples and voicegroups): `just assets-samples [OUT]`, after
  `assets-songs`, reads each song's voicegroup (and the keysplit and
  drum-set groups in it) to the programs and keys its songs play, finds the
  cry tables by shape (runs of DPCM tones) and the samples no tone reaches
  next to ones that are, and writes `OUT/samples/<name>.wav` (`smpl`, `agbp`,
  `agbl`; cries decoded) and `.pcm`, `OUT/voicegroups/<name>.toml`,
  `OUT/soundfonts/<voicegroup>.sf2` and the manifest's `[samples]` and
  `[voicegroups]`. About 1 s. `just assets-check-samples` puts each `.wav`
  through pret's wav2agb (`-b`, cries `-b -c -l 1 --no-pad`) against pret's
  `.bin` and the ROM's bytes and each `.pcm` against pret's; it writes
  `OUT/pret-check-samples.txt` and fails below 544 exact, of 544. About 3 s.
- agb ROMs (#30): `just agb-build` builds june's first-frame ROM from its copy in
  `$GBAA_DATA/src/gba-cart1/` (never june's own checkouts), on
  the nightly its `rust-toolchain.toml` pins, into `$GBAA_DATA/agb/<name>.{elf,gba}`.
  `just disc-agb` scores discovery on that and the LLVM sample in
  `$GBAA_DATA/tools/jumptables/llvm-sample/`: fails below precision/recall
  1.0/0.98 (first-frame ROM) or 1.0/0.57 (LLVM
  sample, whose misses are linker veneers reached by `b`), and prints the
  BIOS copies (agb's IWRAM/EWRAM sections) and `jt-check --discover`'s far
  jumps (failing on a false one) and coverage. `disc-check rom ELF OUT` writes the ROM an ELF loads.
  agb programs of our own live in `agb/<name>/` (source only):
  `just agb-tests` syncs each to `$GBAA_DATA/src/<name>/` and builds
  `$GBAA_DATA/agb/<name>.{elf,gba}` (`iwram-rewrite`, #54, rewrites its
  own IWRAM function: `just first-frame` on it must pass with
  `overlay invalidations: 1`). `agb/template/` is the starter a
  screen-feature unit copies: a fixed mode-0 scene whose expected pixels
  `SCENE.md` lists (it matches mGBA and first-frame at frame 10). VRAM,
  palette and OAM take halfword stores only, in ROMs and in the runtime's
  PPU tests alike (`test_util::ppu`'s `fill16`, `tile4`, `store_bytes`).
- `cargo run -p desktop -- ROM` (`gbaa-desktop`) plays a ROM through the
  interpreter in a window (arrows, X/Z = A/B, A/S = L/R, Enter/Backspace =
  Start/Select, Ctrl+Q quits; Escape never does, as Steam's desktop
  controller layout sends it for B). The window, its drawing and the pads are
  SDL3 (`desktop::window`, on the main thread; the game runs on its own
  thread and hands frames over), built from source by the `sdl3` crate
  and linked statically: building it needs cmake and a C compiler, and
  for the X11 driver the X11 headers (`libX11-devel`; libXext's two are in
  `crates/desktop/sdl-build/`, and the extensions whose headers are
  missing are left out, `sdl-build/toolchain.cmake`), for Wayland
  `wayland-devel` and `libxkbcommon-devel`; a change to any of those
  (`sdl-build/inputs.txt`) rebuilds SDL from an empty CMake cache
  (`vendor/README.md`). It loads X11 or Wayland, GL
  and udev with dlopen at run time. The window ends with a `presents:`
  line on stderr: the gaps between frames drawn (mean, p50, p99, max, how
  many over 25 ms), for pacing and hitching; frames drawn fast-forwarded
  get their own line (`fast-forward:`; their mean is 16.7 ms at any
  speed, while p50 and p99 straddle it, as the frames that land on the
  60 Hz schedule are a whole number of fast frames apart), and a run that
  opened the menu adds the same for frames shown with it open
  (`with the menu:`) and the time
  each egui draw took (`menu draws:`). The menu (`desktop::menu`, egui
  drawn as SDL geometry by `desktop::egui_sdl`; Start+Select on a pad,
  F10 on the keyboard) pauses the game, sets fast-forward's speed,
  sets the volume (10% steps, saved to `settings.conf`) and mutes it
  (gain 0, the device still pacing; not saved; with the menu shut, -
  and = (or the keypad's - and +) step it and M mutes, a toast showing
  the level for a second, unless the mapping binds that key to a
  button), undoes the last state load (greyed when there's none), remaps the pad,
  switches fullscreen, loads and unloads Lua scripts (its Scripts page),
  sets where saves go (its Save folders page, below under Cartridge
  saves) and quits. Its Controls page (`desktop::menu::controls`, design in
  `docs/design/remap-design.md`) has a tab for the pads and one for the
  keyboard (its top row: left and right switch them); "+ Add" adds the
  next control pressed to a GBA button or shortcut, and a chip's × (or
  A, Enter, Delete, Backspace on it) removes one, for every game
  or this game (a `[hash …]` section of the mapping file, keyed by the
  ROM's FNV-1a 64); a control moved from another button is said in a
  toast that Y or Ctrl+Z undoes. The directions are one row of whole
  pads (D-pad, a stick, four keys) unless the "Directions bound
  separately" switch above them is on (`directions = apart` in
  `settings.conf`). The pads' tab ends with the sticks' and the triggers'
  deadzones (`deadzone = 40`, `trigger-deadzone = 20` in `settings.conf`,
  percent; 40 and 20 by default): a stick or trigger counts as pressed
  past it. The sticks' is wide because the GBA's pad is all or nothing.
  The "Up/down apart from left/right" switch over them (`deadzone-axes =
  apart`) splits the sticks' into two, `deadzone-horizontal` and
  `deadzone-vertical` (each the sticks' one until stepped), so stray ups
  can take a wider one while sideways stays quick.
  A direction held (D-pad, stick, arrow key) moves again after 0.5 s,
  then every 0.1 s. The menu moves once per stick push (past half, back inside the
  deadzone to move again; a spring's overshoot isn't a move), and the
  same move twice within 80 ms from two places (Steam Input sends a
  Steam Controller's D-pad as arrow keys too) is one. Each change saves the file in place
  (`desktop::remap`), comments kept; `[title …]` sections, by the
  header's title, are for hand edits. The mapping file (`gamepad.conf`) and
  `settings.conf` live in the config folder: `--config-dir DIR` if
  given, else `$GBAA_CONFIG_DIR` if set,
  else `$XDG_CONFIG_HOME/gbaa`, else `~/.config/gbaa` (Windows:
  `%APPDATA%\gbaa`); `--pad-config F` names another mapping file. Lanes
  set `GBAA_CONFIG_DIR` (or pass `--pad-config`) so the remap screen never
  writes the owner's own file. `--pad-script '3000 start+select dpad-down south'` presses an
  SDL virtual pad's controls (milliseconds to wait, or `+`-joined
  `--pad-config` control names), to drive the window unattended, as under
  `gamescope --backend headless -W 1280 -H 800` with `gamescopectl
  screenshot`. `--headless --frames N --dump F.png --hash`
  runs without one and writes the last frame and its FNV-1a hash;
  `--input 'start@60,down@90+2'` presses buttons on given frames
  (`a@100-400/60` every 60 frames). `--timing instant|exact|static|native`
  (`runtime::Timing`) sets how the interpreter counts cycles: instant by
  default when it plays a ROM (as play builds are), a recompiled build's
  own (`gbaa-recompile --timing`) for its fallback and `--interp`. Every
  check that runs `gbaa-desktop` or a build passes `--timing exact`
  (`route-check --args` prints it), and so must a new one, or it compares
  an instant run with mGBA. `gbaa-interp` is exact unless told. The PPU is
  `runtime/src/ppu.rs`: per line, each background is drawn by `bg_line`, then
  `compose` picks by priority; bitmap modes are `ppu/bitmap.rs`; tile
  backgrounds (#33) go in `ppu/bg*` and sprites (#34) in `ppu/obj*`.
- Codegen (`crates/codegen`, `runtime/src/dispatch.rs`; design in
  `docs/codegen-design.md`): `just recompile ROM` writes `out/<name>/`,
  `<name>` the game's name (deck/game-save-name; `runtime::name`): the
  ROM's file name, lowercase with each run of other characters one dash,
  or `--name NAME` in the same form, given in place of OUT or after it.
  The build carries the name, the key its saves are filed under
  (docs/save-locations-design.md); `gbaa-desktop ROM` names a game from
  the ROM's file name by the same rule. (A crate whose `main` is the desktop with the recompiled code, plus
  `report.txt` and `rom.gba`, a copy of the ROM that the binary carries
  inside it with `include_bytes!`, deck/embed-rom: run with no ROM, a
  build plays its own, as if it were `<name>.gba` beside the binary, so
  its `.sav` and states go there; a ROM given runs instead, and must be
  the one it was recompiled from. One file to send a friend, and netplay
  can't pair a build with the wrong ROM. `out/` and the builds stay out
  of git; a ROM is also run 1800 frames in the interpreter, for the
  IWRAM blocks it patches and the copies discovery missed, decision 7). It
  writes a file only when its bytes change and deletes only files no longer
  generated, so a rerun with no code change rebuilds nothing and a change
  rebuilds only the parts it touches; `gbaa-recompile ROM OUT
  --no-flag-liveness` keeps every flag write (`report.txt` counts the flag
  writes kept and the exits whose yield path writes flags), and
  `--no-dead-reg-writes` keeps register writes overwritten in their
  block, `--locals` holds registers and flags in Rust locals (off by
  default: step 9 of `docs/structured-codegen-design.md`; `report.txt`
  then counts the functions whose write-back check passed),
  `--callee-saves` (with `--locals`) doesn't reload r4-r11 and sp after a
  call to a function proven to keep them (`codegen::saves`), and
  `--no-structured` prints every function in v1's `loop { match pc }`
  form (by default each ROM function with a checked plan is printed as
  `if`s and loops and only its entry is a dispatch key; `report.txt`
  counts them, and why the rest stay v1), and each `--names FILE`
  layers a source map (`codegen::names`, `docs/mod-view-design.md`) for
  the mod view's named print: functions by name (link symbols stay
  `gbaa_f_<key>`) with a `///` name and file, and pool words landing in
  a `[data]` entry as `addr!(name + off)` (consts in `base::names`).
  Without `--names` the output is byte-identical to the play view's.
  `--names-dir DIR` (which `just recompile` always passes, as
  `$GBAA_DATA/names`) names only a readable or mod-view print
  (`--readable`, `--mod`, `--cut`, `--names-check`, a readable snapshot)
  with no `--names`, from the maps in DIR whose `[map]` names the ROM by
  sha1 (`codegen::names::files_for`), as fn-diff does; a plain recompile
  stays unnamed.
  `gbaa-recompile --names-check ROM --names FILE…` prints every function
  unnamed and named and checks the two token by token (Emerald with
  pret's map, 2026-10-27: 15,167 functions, 0 failed, 68,642 names and
  54,992 `addr!`s, 53 s, not heavy). With `--readable LIST`, LIST's
  functions print readable, so the check covers their DWARF names
  (`codegen::dwarf`: a value named from `[locals]` must stand where the
  unnamed print has its `vN`, and each use of a local must resolve to the
  same `let` in both prints, so a name can't shadow a value still in
  use) and skips their `note!` comments, and a
  second line counts them: register variables printed by name, only as
  comments and neither, and the line, stack-slot and call comments
  (Emerald, `mods/emerald/readable/all.list`, 2026-10-28: 0 failed; of
  20,446 variables 16,481 by name, 2,726 only as comments, 1,239
  neither; 203,328 line, 4,607 slot and 49,148 call comments; 90 s;
  `[types]` off for the walk, since a typed access restructures what it
  folds in);
  `gbaa-recompile --messages-check ROM JSON` decodes every message of an
  index game's table (`crates/codegen/text-tables.toml`'s `[game.index]`)
  and compares each with JSON, the decomp's text by bank and message, and
  exits 1 on any difference (Minish Cap against
  `$GBAA_DATA/src/tmc/translations/USA.json`, 2026-10-31: 3,699 match, 0
  differ, under a second);
  `just snapshots [ROM] [FILE]`
  rewrites `mods/emerald/snapshots/emerald.rs`, the smallest structured
  Emerald function of each shape, so a printer change shows as its diff
  (about 30 s, not heavy); `just run-recompiled out/<name> [desktop args]` builds and
  runs it (checks free RAM, caps it at 3 GB); `just recompile-check ROM`
  compares the recompiled build with `--interp` (last frame hash and
  registers; writes both runs' `.txt` and `.png` to `out/<name>/`);
  `just sst-gen [PER_FILE] [SEQS]` runs SingleStepTests through generated
  Rust (PER_FILE cases from each file, against the expected state and the
  interpreter), then SEQS (4000) sequence cases: alternately a random block
  of 2 to 6 of the suite's opcodes and a function of 2 to 4 such blocks
  joined by random `b<cond>`s (loops included) ending in `bx lr`, built by
  `build_func` from an image at `0x0800_0000` (timing ops, const-prop, flag
  liveness, as for a ROM), each run for up to 24 ticks. Against the
  interpreter with `armv4t::cycles` counted as `Machine::step` does, it
  compares the CPU, the bus accesses interleaved with `rt::multiply`,
  `rt::block` and `rt::stall`, and each tick's cycles (`rt::fetch` under
  made-up waits, less `rt::skipped`'s). Fails on any difference, except
  the CPSR's flags in a run that ended in a yield, excused and counted
  under "flags" as in `fn-diff` (before `structured/yield-flags` flag
  liveness left them stale there);
  T flipped inside a block is skipped and counted. Templates in
  `crates/codegen/sst-gen/`.
  They take the build lock themselves. Generated crates share `target/recompiled`.
- `gbaa-desktop ROM --script FILE` (repeatable, recompiled builds too) runs
  mGBA 0.10.5 Lua scripts (`crates/script` on mlua's vendored Lua 5.4;
  design in `docs/research/lua-scripting.md`): `emu` (memory and its
  `emu.memory` domains, ROM info, CRC32 as four big-endian bytes,
  `getGameCode` as `AGB-XXXX`, keys, registers, state buffers and files
  ours or mGBA's, `screenshot`, `step`, `runFrame`, `reset`), `callbacks`,
  `console` and text buffers, `socket` (mGBA's Lua over a non-blocking
  `std::net` `_socket`, polled each frame), `C`, `util`, `system`, and the standard
  libraries. Scripts load as into mGBA's scripting window
  with the game running: `emu` set, `start` not fired; each in its own
  `_ENV`, whose `require` finds modules beside the script. A script is
  sandboxed (the owner's choice): files (`io`, `os.remove`/`rename`,
  `loadfile`/`dofile`/`require`, state files, screenshots) only under its
  own folder, links and `..` resolved; source only, no C modules; no
  `os.execute`, `io.popen`, `os.exit`, `os.getenv`, `os.tmpname`; of
  `debug` only `traceback` and `getinfo`; sockets on loopback only (a
  `nil` bind address binds 127.0.0.1; others get `access denied`). A blocked call raises an error
  saying so. `--script-full FILE` gives it everything, as mGBA, and
  remembers that in `script-access.conf` beside `gamepad.conf` (a
  `full = PATH` line; delete it to take it back). The check is
  `cargo test -p script` (`crates/script/fixtures/sandbox/`). `keysRead` fires before each frame's pad
  poll and the keys a script sets replace the pad's for that frame;
  `frame` after it; `savedataUpdated` on the first frame the save
  changes; `crashed` on a fault; `stop` and `shutdown` on exit. A name
  mGBA has that we don't returns nil and warns once on the console. A
  callback past about 5 million Lua instructions (a frame's time) is
  stopped and removed. Headless prints the console and each buffer on
  exit; the window prints console lines to stderr as they come. Not
  with `--audio-pace`. `just script-check [ROM]` runs the fixture in
  `crates/script/fixtures/emerald/` and the pinned checkout's
  `res/scripts/pokemon.lua` on Emerald (USA) for 1500 frames from
  power-on and fails unless the output matches `expected.txt` there
  (the fixture writes a party mon in Birch's speech; pokemon.lua must
  show it); about a minute with the release build made. It also runs
  mGBA's `res/scripts/socketserver.lua` headless on a `b .` ROM: a
  loopback client connects, sends a line the script must log, and must
  get the keys line when `--input` presses A (about 10 s).
  mGBA 0.11-dev's `storage` is there too (`script::storage`): a
  bucket from `storage:getBucket(name)` is kept as `storage/<name>.json`
  in the config folder, flushed every 30 frames when it changed and on
  exit; its check is `crates/desktop/tests/script_storage.rs`, in
  `cargo test` (`crates/script/fixtures/storage/storage.lua` run headless
  twice: the second run reads what the first wrote).
  And 0.11-dev's `emu:setBreakpoint`, `setWatchpoint`,
  `setRangeWatchpoint`, `clearBreakpoint` and `C.WATCHPOINT_TYPE`
  (`crates/script/src/points.rs`, over `runtime::debug`): while any is
  set, frames run in the interpreter (`Machine::run_frame_debug`), even in
  a recompiled build, which goes back to compiled code once the last is
  cleared (june's choice; about 220 frames/s on quack). Info tables as
  mGBA's; unlike mGBA, a watchpoint's callbacks run after the instruction
  that hit it, and DMA and BIOS calls aren't watched. Its check is the
  `script_breakpoints` part of `just script-check [ROM] [BIN]`:
  `crates/script/fixtures/breakpoints/` on Emerald for 600 frames (a break
  at `CB2_InitTitleScreen` and at `Random+4`, write and read watches,
  each checked against a once-a-frame look) must match `expected.txt`;
  BIN runs it on a recompiled build instead of gbaa-desktop.
  And 0.11-dev's `canvas` and `image` (`crates/script/src/canvas.rs`):
  layers (`canvas:newLayer`, `layer:update`, `setPosition`, up to 10),
  images (`image.new`, `image.load` of a PNG, pixels, `drawImage`,
  `save`) and painters (rectangles, lines, circles, masks, text in a
  font file, `C.ALIGN`), with mGBA's pixel arithmetic; text is drawn by
  `ab_glyph`, not FreeType, so glyph edges and strokes differ a little.
  The layers are drawn over the game in the window and in `--dump` and
  `--dump-at`'s PNGs (headless too). `canvas:width()` is the 240×160
  picture's, but a layer placed past its left or right edge makes a
  sidebar there (up to 240 pixels a side, the picture's height; above
  and below is clipped), as mGBA 0.11-dev's canvas reaches into the
  window: the window widens for it at the game's scale, and the dumps
  are as wide as picture and sidebars, black under them. Where the
  sidebars would bring the picture below the whole scale it has alone
  (the Deck at 1280x800, fullscreen: 4x against 5x), they're a drawer
  (scripting/sidebar-drawer, `window::place`): the picture alone,
  centred, and the console's toggle (F9, or a pad's `console` if bound)
  opens the sidebars over it at the screen's edges with the console, the
  right one left of the console's panel (seen on Emerald with
  `panel.lua` in a 1280x800 `cu session`). A layer goes
  when its script is unloaded.
  `script.dir` and `script.path` are set, as 0.11-dev's. Its check is the
  `script_canvas` part of `just script-check`: mGBA's
  `res/scripts/demos/text-demo.lua` and `logo-bounce.lua` (fetched at
  fd9dc5c, hash-checked, into `$GBAA_DATA/tools/mgba-demos/` by
  `scripts/fetch-mgba.sh`) on Emerald must draw their nine labels and a
  moving logo, and `crates/script/fixtures/sidebar/panel.lua` must draw
  its 80-pixel panel right of the picture (seen too in a window in a
  `cu session`, which widened from 720 to 960 at 3x); `cargo test -p
  script` checks the painter's pixels, when a layer's changes show and
  the sidebars' extent.
  And a shim for BizHawk's drawn panels (scripting/bizhawk-gui,
  `crates/script/src/bizhawk.lua`): `gui.drawRectangle` (GDI+'s: the
  outline w+1 by h+1), `drawBox`, `drawLine`, `drawPixel`,
  `drawPolygon` (2.x's offsets or 1.x's `(points, line, back)`),
  `drawText`/`drawString`/`pixelText`/`text` (one built-in font, egui's
  Ubuntu Light, whatever the family; sizes in pixels), `drawImage` (PNG
  or a GIF's first frame, scaled, relative paths from the script's
  folder) and `drawImageRegion`, the `default*` colours, colours as
  numbers, .NET names or `#RRGGBB`; `client.SetGameExtraPadding` (the
  surface grows by it, origin at its top left: a right or left pad is a
  sidebar, above and below are clipped); `memory` and `mainmemory`
  (`read`/`write_u8` … `u32`, `s`, `_le`/`_be`, byte arrays, domains
  `System Bus`, `EWRAM` (the default), `IWRAM`, `ROM` and the rest at
  their bus bases); `emu.frameadvance`/`yield` (every script's main
  chunk runs as a coroutine, resumed each frame), `framecount`,
  `getsystemid`; `event.onframeend`/`onframestart`; `input.getmouse`;
  `bit`; `bizstring` (`hex`, `binary` and the string tests);
  `console.log` with a dot. Each script's surface is one layer,
  cleared at its next frame, as BizHawk's. `forms.*` is out of scope
  (warns, gives nil). `cargo test -p script` checks it;
  `crates/script/fixtures/bizhawk/panel.lua` with `--script` on Emerald
  draws a panel right of the game, and so does PokemonBizhawkLua's
  `pkmgen3.lua` (5a2be67, in `$GBAA_DATA/tools/PokemonBizhawkLua`; run
  from its folder with `--save` alice.sav): its party panel beside
  Emerald. It still lacks `forms.*` (its seed and settings dialogs), and
  its `UP_GAP`/`DOWN_GAP` rows above and below the picture are clipped.
  And 0.11-dev's `input` (`crates/script/src/input.rs`):
  `input:isKeyActive` (a one-character string or a number),
  `input:activeKeys()`, `input.seq`, the `key`, `mouseButton`,
  `mouseMove` and `mouseWheel` callbacks with their events, and
  `C.KEY`, `C.KMOD`, `C.INPUT_STATE`, `C.EV_TYPE`, `C.INPUT_DIR` and
  `C.MOUSE_BUTTON`. The window feeds it between frames from its
  keyboard while the game has it (not with the menu open or the
  console's line typing; those, or the window losing focus, let every
  key go) and from its mouse, in game-screen pixels. Keys give the codes
  mGBA's Qt window gives: a letter's capital, a shifted key's character
  as the layout types it (Shift+1 is `!`, or `!` and Shift held), `C.KEY`
  values for the rest (either Shift is `SHIFT`). Unlike mGBA, a release
  gives the code its press gave, so a key let go after Shift can't stay
  held. `activeGamepad` is always nil. Headless (or in the window too),
  `--script-keys PRESSES` holds keys as `--input` does buttons, e.g.
  `'shift@60+30,!@60+30'`; a press at frame N reaches the callbacks in
  the frames an `--input` press at N shows in `emu:getKeys`. Its check
  is `crates/desktop/tests/script_input.rs`, in `cargo test`
  (`crates/script/fixtures/input/keys.lua` must see Shift+! and A in the
  frames they're held, with their `key` events). PokeLua's
  `E_RNG_mGBA.lua` (2fedf75) on Emerald: its Shift+number save-slot and
  number load-slot hotkeys and Shift+Start TID bot are seen, and its
  `os.execute("mkdir states")` needs full access.
- Scripts' state slots (scripting/state-slots): `emu:saveStateSlot(n)`
  and `loadStateSlot(n)` use the window's slot files, `NAME.stateN` in
  the game's folder (headless, `ROM.stateN` beside the ROM; any `n` from 0 to 255; a negative one, or an empty slot, gives
  false). As mGBA 0.10.5's, `loadStateSlot` keeps the cartridge save as it
  is unless its flags include `C.SAVESTATE.SAVEDATA` (the window's F1-F4
  put the state's back). Its check is `crates/desktop/tests/script_slots.rs`,
  in `cargo test` (`crates/script/fixtures/slots/slots.lua` saves slot 7
  headless and loads it back). PokeLua's `E_RNG_mGBA.lua` on Emerald,
  `--script-full` with `--script-keys 'shift@300+4,!@301+2,1@600+2'`:
  no warnings, `NAME.state1` written, and the load takes its advances back
  to the save's (402 at frame 700, against 701 without it).
- The window's script console (scripting/console-ui, `desktop::console`):
  the scripts' text buffers (each folding) above their console, and a
  line to type Lua into, as mGBA's scripting window. F9, the pad's
  `console` (unbound by default) or the menu's Scripts page show
  and hide it: in a window, a panel right of the frame (the window
  widens for it, and narrows back); fullscreen, as on the Deck, over the
  frame's right. Clicking (on the Deck, tapping) the line gives it the
  keyboard (SDL's text input, which opens Steam's on-screen keyboard
  there); the game gets no keys then; Enter runs it, Up and Down step
  through the lines entered, Escape leaves it. A typed line runs as mGBA's
  prompt does (`script::Host::eval`): it finds what scripts defined
  (newest first, though each has its own `_ENV`), so Ironmon's commands
  work; it's echoed as `> line` and an expression's values are logged.
  The menu's Scripts page loads the `.lua` files in `scripts/` in the
  config folder and beside scripts already loaded (sandboxed unless
  `script-access.conf` says otherwise), and unloads one (its callbacks,
  buffers and names go; nothing fires, as mGBA 0.10 has no unload);
  both close the menu, as scripts run with the game. Seen in a private
  `cu session`: `crates/script/fixtures/console/give.lua` with
  `--script`, pokemon.lua loaded from the Scripts page, `give(5)` typed;
  the Party buffer shows the Torchic.
- `just cfg-survey ROM [TRUTH]`: the shape of every recompiled function's
  CFG (reducible or not, loops and their exits, irreducible regions, return
  sites, jump tables, Ramsey nesting) and what the structuring plan
  (`codegen::plan`) makes of each (planned, regions collapsed, failed,
  nesting), for structured codegen
  (`docs/research/structured-control-flow.md`). TRUTH, a `disc-truth`
  file, names the functions it lists. About 20 s and 300 MB on Emerald;
  not heavy, needs no build of the generated crate.
- `just fn-diff ROM [FUNCS] [--exact]` (heavy: builds a generated crate):
  the per-function differential runs ("Validation" in
  `docs/structured-codegen-design.md`). `gbaa-recompile --fn-diff` runs
  ROM 600 frames in the interpreter, picks up to FUNCS (200) of the
  functions entered whose callees number at most 40, records up to 3 entry
  states each (`Machine::save_payload`) and writes `out/fn-diff-<name>/`:
  the picks and their callees as one module per form (`reference`, the
  default configuration; one per `codegen::func::Config` switch flipped,
  now `liveness`, `dead_regs`, `locals` (switched on: registers in Rust
  locals), `saves` (locals, with no reload of r4-r11 and sp after a call
  to a function `codegen::saves` proves keeps them) and `v1`, every
  function in `loop { match
  pc }` form; `readable`, the picks on the readable list
  (`Ctx::readable`), their callees as the reference, named and typed
  from the source maps in `$GBAA_DATA/names/` whose `[map]` names ROM
  (`--names-dir`, `codegen::names::files_for`; none for other ROMs);
  `fault`, the reference with one byte of one
  single-exit function's cycle count changed) and `states.bin`. Every
  form dispatches at the reference's keys only: v1 with every block start
  a key ran compiled where the reference resumes in the interpreter,
  which ticks per instruction (191 runs apart in ticks alone, 2026-10-25). The crate
  (`crates/fn-diff` is its runner, its `runtime` built with the `trace`
  feature, which digests every bus access and tick in order) runs each
  state under 22 mutations (all 16 NZCV, random r12 and flags, r0-r3 each
  plus one, r0-r3 random) in every form, from the entry to the return
  address in lr or the first yield, and compares the end, the bus trace,
  the ticks and the full state with the reference's. Fails unless every
  switch matches and the fault is caught in its function's runs. At a
  yield the CPSR's flags are excused and counted (before
  `structured/yield-flags` flag liveness left them stale there);
  `--exact` counts them. Emerald (no input, 2026-10-24): 135 functions,
  290 states, 6,380 runs a form; liveness 0 differ (with `--exact`; 50
  only in the flags at a yield before yield-flags); fault caught; 75 s,
  most of it the build. A new transformation adds a `Config` field and a
  line to `codegen::fn_diff::configs`.
  `gbaa-recompile --fn-diff ROM OUT --hand FILE KEYS` (KEYS
  comma-separated hex) picks only KEYS and adds a `hand` form that
  pastes in FILE's `f_<key>` fns in their place (each `include!("X");`
  line replaced by X, so its macros can shadow the prelude's): hand-written code
  checked against the generated (the readable-form spike,
  `mods/emerald/decomp-spike/`, docs/mod-view-design.md). A key the run
  never enters borrows the entry states of up to eight Thumb functions that
  it does, its pc changed. Then `just build-recompiled OUT` and run
  `target/recompiled/release/gbaa-<OUT's name> ROM --exact`.
  `gbaa-recompile --print ROM KEY...` prints those functions with
  `--locals`, `--callee-saves` and `--asm-text` on, to stdout (about 40 s
  on Emerald). `gbaa-recompile --saves ROM [KEY...]` counts the functions
  `codegen::saves` proves keep r4-r11 and sp, groups why the rest don't,
  and answers for each KEY (20 s, 300 MB on Emerald: 8,894 of 15,158).
  `gbaa-recompile --summaries ROM [--names FILE]... [--names-dir DIR]
  [--show N] [--callee-sets] [KEY...]` gives every function's summary
  (`codegen::saves::summarize`: parameters r0-r3, results, callee-saves,
  holes) and, with a map, checks it against the DWARF prototypes
  (`codegen::saves::compare`): agree, disagree and no-prototype counts,
  the disagreements by category with N of each (default 10, spread over
  the list, each with the root of its chain of pass-ons), the same counts
  trusting indirect calls to read nothing, and each KEY's summary. No
  bar: it measures. Emerald (`--names $GBAA_DATA/names/pokeemerald.toml`,
  25 s, 430 MB): the counts and what each category is are in
  `docs/native-port-design.md` section 5. `--callee-sets` first follows
  function addresses (`codegen::native::callee_sets`, the lifter's
  `fnref` flow, over the ROM's code) and takes each indirect call with a
  complete callee set as calling exactly those
  (`codegen::saves::summarize_sets`; unit `native/indirect-call-args`),
  printing the counts without the sets too. About 5 s on an agb program;
  on Emerald the flow didn't finish in 35 minutes (killed at 6 GB, then
  stopped at 8 GB under a 16 GB cap), and after `native/fnref-flow-scale`
  it was killed at a 14 GB cap in 5 minutes (2026-11-07):
  `native/fnref-flow-emerald`'s work.
  fn-diff's summary line gives the same count and
  how many picks call a proven function. Emerald, `just fn-diff ROM 2000
  --exact` (2026-10-27): every entered function picked, 270, 591 states,
  13,002 runs a form, `saves` 0 differ, 146 picks calling a proven one.
  Emerald with `--exact`, 2026-10-27, the readable lowering: `readable`
  0 of 13,002 runs differ (737 of them yield); the spike's eight with
  `--hand`: `readable` and `hand` 0 of 1,584. 2026-10-28, pool folding:
  `readable` 0 of 13,002; loop locals and `for`: 0 of 13,002; indexing
  (`idx(i)`, constants in registers across proven calls): 0 of 13,002;
  named and typed (DWARF names, types, prototypes): 0 of 13,002.
- The readable form (`docs/mod-view-design.md`, "What the readable-form
  units should become"; the spike's form, with pool loads folded into
  the access after them, `m.get_via`, registers held in Rust locals across
  ticks and one-block counted loops as `for`, but without types yet): `gbaa-recompile ROM OUT --readable FILE` prints the
  functions FILE lists (one a line, by key `0x0806e415`/`f_0806e415` or by
  name from `--names`, `#` comments) readable and the rest as the flags
  say; `report.txt` counts them, how many of their blocks took the form
  (ARM code, a PSR or user-bank access, a guarded write, or a read of a
  register the block already wrote keeps a block in the play view's
  statements), and names any that aren't functions.
  `--timing instant|static|exact` sets how the generated code keeps time,
  for the whole crate (`codegen::func::Clock`;
  docs/research/mod-view-without-timing.md). A game's build and a `--cut`
  default to instant (june, 2026-10-30: what she plays and mods): each
  fetch with no wait states, data accesses free. static drops each
  access's address and every `rt::stall`/`rt::fetch`/`rt::skipped`/`m.pool`
  for one constant per exit. exact is mGBA's model, the oracle: every check
  recipe (and route-check's builds) passes `--timing exact`, and
  `--snapshots`/`--names-check` default to it. A new check that recompiles
  must pass it too, or it compares an instant build with mGBA.
  `--timing native` is the native port's oracle (decision 11, "Native
  timing"): code takes no time; idle loops, stuck spin loops (loops
  that call, past a budget; the per-game list is
  `crates/runtime/spins.toml`) and the BIOS's copies move it; its build
  runs only at `--timing native` or `--interp`.
  `just timing-modes` runs `out/kirby-{exact,static,instant}` (built first,
  the doc says how) along timing-lab's route and compares them with exact.
  `just snapshots-readable [ROM] [NAMES] [LIST] [FILE]` rewrites
  `mods/emerald/snapshots/emerald-readable.rs` from
  `emerald-readable.list` with pret's map (about 30 s with the saves proof, not heavy).
  `just sst-gen 500 4000 --readable` runs sst-gen with each generated fn
  readable (2026-10-27, the readable lowering: 22,500 cases and 4,000
  sequences, 0 differ).
- `just readable-states ROM NAMES LIST [--every N] ROUTE…` (heavy: builds
  two generated crates; NAMES `""`: unnamed): the named play view (`out/<name>-named`) and the
  same with `--readable LIST` (`out/<name>-readable`) built and run through
  `route-check --same-states` as `names-states` does. A list line `*` is
  every function printed, the IWRAM copies' and continuation roots' too
  (`emit::all_entries`): `mods/emerald/readable/all.list`.
  `mods/emerald/readable/200.list` is the 200 functions the build cost was
  measured on. Emerald, every function readable, the new-game route with
  `--every 250`, 2026-10-28: 15,155 of 15,157 listed functions readable
  (112,737 of 112,838 blocks), all 23 frames the same, 642,730 bytes each;
  named and typed with `pokeemerald.toml`, 2026-10-28: 15,167 of 15,167
  (112,738 of 112,940 blocks), all 23 the same.
  `just readable-suites [--every N]` runs it unnamed with `all.list` on
  the CPU suites: jsmolka's arm, thumb, memory, nes and bios tests and
  both armwrestler builds, along `units/jsmolka-*.toml` and
  `units/armwrestler-*.toml` (the armwrestler routes walk every page as
  `just armwrestler` does, each page a checkpoint). 2026-10-28,
  `mod/readable-exact`, `--every 25`: all seven the same at every point
  (12 points each for jsmolka's, 20 for armwrestler's); blocks in the
  readable form, arm 1 of 150, thumb 122 of 177, memory 0 of 77, nes 0
  of 3, bios 0 of 64, armwrestler 64 of 201 (the form takes Thumb ROM
  blocks only; the rest are ARM). The same day `just sst-gen 500 4000
  --readable`: 22,500 cases and 4,000 sequences, 0 differ. `just
  own-state-check ROM N K "" "--readable mods/emerald/readable/all.list"`
  checks our save states with every function readable: Emerald 1000 100
  (15,167 functions, 112,738 of 112,940 blocks readable) and
  armwrestler-fixed 200 150 pass (2026-10-28).
- `just liveness-states ROM [--every N] ROUTE…` (heavy: builds two
  generated crates, about 5 min each for Emerald): recompiles ROM with flag
  liveness on and off (`out/<name>-liveness`, `out/<name>-no-liveness`),
  builds both and runs `route-check --same-states A B [--every N]
  ROUTE…`, which runs each build (A and B: a binary, optionally followed
  by its flags, space-separated) once along each route (`units/*.toml`)
  from reset, saving our exact state after every checkpoint, and every
  Nth frame (`--dump-at` with `--dump-states`), and fails unless each pair
  is byte-identical, naming the differing byte ranges and keeping the pair
  in `out/same-states/<route>/`. The two forms share tick
  points, so any difference is a stale flag at a yield (step 2 of
  `docs/structured-codegen-design.md`). The checkpoints alone rarely catch
  one: use `--every 50`. Emerald's intro and new-game routes with
  `--every 50`, 2026-10-24: all 184 frames the same, 642,720 bytes each;
  25 min with both builds (before the fix, the new-game route's frame 950
  differed).
- The naming loop (`docs/mod-view-design.md`, "The naming loop";
  `crates/naming`; not heavy, but every `ask` spends the owner's model
  usage, so it runs only on the capped sample until june passes the
  naming gate). `just naming prepare ROM PRET OUT [--seed N]` draws the
  sample from the functions PRET (`$GBAA_DATA/names/pokeemerald.toml`)
  names, leaving out libgcc, m4a and the SDK's libraries, whose names
  every print is given: 100 per size class, about 30 whole subtrees first;
  it writes `OUT/sample.toml` (about 20 s on Emerald). `just naming ask
  ROM OUT ROUND [--jobs N] [--model M] [--limit N]` asks `claude -p`
  (no tools, safe mode, from an empty folder in `/tmp`; default model
  Sonnet 5.5, 6 at a time) for each function's name: ROUND `0` (code
  alone), `1` (bottom-up, callees' round-1 names, round 0's data vote),
  `2` (callers' names too), or the leak controls `nocode` and `recog`.
  Each answer, with its tokens, seconds and list cost, goes to
  `OUT/<round>/<key>.toml`, its prompt to `<key>.prompt`; a rerun asks
  only what is missing. `just naming score OUT` scores every round
  against pret (subtoken P/R/F1, exact match, data by vote, cost) and
  writes `OUT/report.md`. Emerald's sample is `work/mod-naming/s1/`.
  The sibling leak control: `just naming sibling ROM PRET SIB_ROM SIB_PRET
  OUT [--seed N] [--count 30]` (FireRed's ROM and names, then Emerald's)
  writes a sample of ROM's functions whose code, call offsets masked and
  8+ instructions, is the same as one of SIB_ROM's (unique in each) under
  another, non-placeholder pret name; `ask … 0` and `score` as above, the
  report adding how many answers give the sibling's name. About 40 s.
- Matching functions across a compiler change
  (`docs/research/cross-compiler-matching.md`; `crates/xmatch`; heavy only
  in that it runs discovery twice). `just xmatch score A.gba A.toml B.gba
  B.toml [SHOW]` builds each function of both ROMs in codegen's IR, matches
  every B function to one of A's or to none without looking at a name
  (byte anchors, IR fingerprints, then rounds spreading through matched
  callees, callers and RAM variables, a neighbourhood step and a prune), and
  scores the pairs against the two source maps by stage, by size and by
  call-graph agreement, listing SHOW wrong pairs. Emerald against its
  modern build: `just xmatch score $GBAA_DATA/src/pokeemerald/pokeemerald.gba
  $GBAA_DATA/names/pokeemerald.toml
  $GBAA_DATA/src/pokeemerald/pokeemerald_modern.gba
  $GBAA_DATA/names/pokeemerald_modern.toml` (`just pret-names
  pokeemerald_modern` first). `just xmatch sweep …` (same arguments)
  prints the bars side by side. `just xmatch name A.gba A.toml B.gba
  OUT.toml [--frames N]` writes A's names for B's matched functions as a source
  map for B (each line's comment: A's key, score, stage, round, sizes), and
  with `--frames N` runs B from reset that long in the interpreter and lists every
  function entered, named or not. About 1.5 min and 450 MB. `--library`
  (any subcommand) matches only the reference's library functions, the
  objects `crates/xmatch/libraries.toml` lists, at library mode's bars
  (`Config::library`): names for a game that shares no source with the
  reference. `just fetch-rogue` and `just rogue-names` build Emerald Rogue
  with symbols and make its map, the truth for Rogue. `xmatch name` takes
  several references (`A.gba A.toml` pairs, then `B.gba OUT.toml`;
  `--frames N` for the run) and keeps each function's strongest claim;
  placeholder names (`sub_…`) are never written. `xmatch matrix [--library]
  ROM MAP…` scores every ROM's names on every other one. `just
  fetch-decomps` builds zeldaret/tmc, metroidret/mzm and
  jiangzhengwenjz/katam byte-identical to june's ROMs (about 3 min the
  first time) and `just decomp-names` makes their maps in
  `$GBAA_DATA/names/`. `just library-names [ROMS]` (`xmatch
  library-names`, mod/default-library-names) names every ROM in ROMS
  (default `~/ROMs/gba`) that no full map names (one from symbols, not
  one xmatch wrote) with `name --library` from all five references, into
  `$GBAA_DATA/names/<rom>-library.toml`; about 4 min and 0.5 GB for june's
  11 ROMs (2026-10-29: Imperium 174 names, Rogue 186, Nightmare in Dream
  Land 80, Super Mariomon 341, WarioWare 45, the two homebrew 1 and 2). `xmatch judge OUT ROM MAP… [--limit N] [--model M] [--effort E]
  [--jobs N] [--gap N] [--min S] [--seed N]` (mod/opus-library-judge;
  spends the owner's model usage: `--limit` caps the new calls, default 0,
  which only scores what OUT holds) takes each ROM as the game, names it
  from the others' libraries, and asks `claude -p` (as `naming ask`, default
  Opus 5.5, 4 at a time) which of the three nearest reference library
  functions each unnamed library-looking function is (within `--gap` 16
  functions of named library code on both sides), or none; answers in
  `OUT/<game>/<key>.toml`, prompts beside them, the score in
  `OUT/report.md`. About 4 min without calls; the six-ROM trial is
  `work/opus-judge/trial/`.
- `just names-states ROM NAMES [--every N] ROUTE…` (heavy: builds two
  generated crates): `--names-check`, then the unnamed and named prints
  (`out/<name>-unnamed`, `out/<name>-named`, NAMES the source map, e.g.
  `$GBAA_DATA/names/pokeemerald.toml`) built and run through
  `route-check --same-states` as `liveness-states` does. Names change no
  lowering, so any difference is a naming bug.
- A mod copy (`docs/mod-view-design.md`, "A mod copy";
  `codegen::modcopy`). `just mod-cut ROM NAMES DIR FUNC…`
  (`gbaa-recompile ROM --names NAMES --cut DIR FUNC…`, FUNCs by pret name
  or key; about 20 s, not heavy) writes `DIR/mod.toml` (ROM sha1, gbaa
  commit, the map's `source`, the functions by key and name),
  `DIR/src/<Name>.rs` (readable and named, exported as `gbaa_f_<key>`;
  edit these; the header says what the addresses, costs and `regs!` are
  and what an edit does with them) and `DIR/orig/<Name>.rs` (the same, never edited). A re-cut
  over an edited `src/` file merges the new print in with `git merge-file`
  and fails on a conflict (a file cut with `insn!` lines has them recast
  as the new print has them first: none by default, comments with
  `--asm-comments`; 2026-10-30, two edited functions of the main-menu cut
  re-cut so, 0 conflicts). `gbaa-recompile ROM OUT --names NAMES --mod
  DIR` leaves the mod's functions out of the parts and adds a `modded`
  part crate whose modules are `DIR/src/*.rs` by path; the table reaches
  them by symbol. `report.txt`'s `mod` line names any function that prints
  differently now than at the cut. The desktop's report adds `mod: N of
  the resumes landed inside a modded function (M at a call the mod added
  …)` (where the interpreter, not the mod, runs; M counts returns to a
  call the mod added, `call!(m, pc, f)`, whose frame a load, rewind or
  unwind lost, which went on where the call's block began:
  docs/mod-view-design.md, "A call a mod adds"). Its check,
  `mods/katam/half-speed/` (Kirby & the Amazing Mirror at half speed: an
  added `VBlankIntrWait` in `GetInput`), by hand: `gbaa-recompile ROM
  out/katam-mod --names $GBAA_DATA/names/katam.toml --mod
  mods/katam/half-speed`, `just build-recompiled out/katam-mod`, then
  `gbaa-katam-mod --headless --input
  'start@400,start@700,a@800,a@950'` with `--frames 1002 --state-at 1001
  --state-out S` (frame 1001 ends inside the call), `--state S --frames
  300`, and `--frames 1300 --rewind 1000+31`: each loaded or rewound run
  prints `(1 at a call the mod added …)` and runs on. `just mod-states ROM NAMES DIR [--every N] ROUTE…`
  (heavy: two generated crates) builds the named print and the modded one
  (`out/<name>-named`, `out/<name>-mod`) and runs `route-check
  --same-states`, printing that resume count per route. To see an
  edit: `just mod-run ROM NAMES DIR [--dump-at N,…] ROUTE` (heavy: the
  same two crates; NAMES `""` names both from the maps in
  `$GBAA_DATA/names` that name ROM by sha1) runs both along ROUTE (`route-check --mod-run`),
  dumps the frames `--dump-at` names (default the route's checkpoints),
  says which pairs are the same and writes `out/mod-run/<route>/side-N.png`
  for each that differs: stock, the mod, and stock dimmed with the
  differing pixels in red, left to right. It never fails on a difference
  (that is what a mod is for), only when a build can't run.
- What to cut for a change to a struct field (`codegen::find`;
  `docs/research/agent-first-mod.md`, item 5). `just mod-find ROM NAMES
  FIELD` (`gbaa-recompile --find ROM FIELD --names NAMES`; NAMES "": the
  maps in `$GBAA_DATA/names` that name ROM; about 30 s and 0.5 GB on
  Emerald, not heavy) prints every function readable and named, and lists
  those whose print names `.FIELD()`: writers (the field is on the `Ptr`
  of an `m.set`/`set_bits` and their `_via` forms), then readers
  (anything else), one line a function with each pret `file:line` and the
  field's path (`SaveBlock2.optionsTextSpeed`). Exits 1 when nothing
  names the field.
- `just block-timed-states ROM [--every N] ROUTE…` (heavy: builds the
  game's crate in `out/<name>/`): the block-timed interpreter as a
  full-state reference (validation item 1 of
  `docs/structured-codegen-design.md`). Runs `route-check --same-states`
  with the recompiled build and the same binary with `--interp-blocks`,
  which runs every compiled block in the interpreter
  (`runtime::dispatch::run_blocks`): the block's instructions without
  ticking, then one tick at its exit with their cost priced as the
  generated code prices it (fetch wait states as of the exit; a failed
  condition's saving off the bus count), so I/O sees the same clock and
  IRQs land at the same instructions. Where each block ticks comes from
  the crate's `EXTENTS` table (`codegen::emit`, the owner function's
  block: its length, and a `bx rN` stub a call runs on the same tick); a
  taken branch ends it early. Keys with no compiled block go to the
  fallback as in a recompiled run. Inside a block neither the prefetch
  queue nor open bus moves, as in compiled code. Any difference is a
  codegen bug the interpreter can see and v1 can't. Emerald's intro and
  new-game routes with `--every 50`, 2026-10-24: all 184 frames
  byte-identical (before, plain `--interp` differed in 25 to 983 bytes);
  the six small ROMs' routes too. About as fast as `--interp` (1500
  Emerald frames in 11 s).
- `just provenance NAME`: where palette, VRAM and OAM bytes came from
  along route NAME (`units/NAME.toml`), docs/research/assets-provenance.md.
  Builds `gbaa-desktop` with the `provenance` feature (`runtime::prov`,
  off in every other build) into `target/prov` and runs it headless on the
  route's presses; at each checkpoint writes
  `out/provenance/NAME/frame-N.prov.txt` (non-zero bytes now, and every
  byte stored so far, by source and transform; for palette RAM, how much
  equals a ROM-copied RAM buffer) and `frame-N.blobs.tsv` (the copied ROM
  ranges, compressed blobs and guessed tables that reached them), and
  `times.txt`. Emerald's new-game route: about 25 s with the build warm,
  about 5% slower a frame than untagged.
- `just assets-record NAME [GAME] [NAMES]`: the load and use logs for
  asset export along route NAME (assets/load-log,
  docs/asset-extraction-design.md, Recording), into
  `out/assets/GAME/NAME/` (GAME defaults to `emerald`, NAMES to pret's
  source map). The `provenance` build (as above) runs the route headless
  with `--assets-log`: `loads.tsv` (every BIOS decompression, `CpuSet`,
  `CpuFastSet` and DMA whose source traces to ROM, with the bytes the
  decoder read or the copy's count; a RAM source's ROM runs; a load
  repeated unchanged isn't written again), `uses.tsv` (each distinct BG and
  OBJ tile use: layer, bpp, OBJ size and part, the tile's source, its blob
  and offset, palette bank and the palette's source, first and last frame
  and frames shown), and the check's inputs `reached.tsv` and `shown.tsv`,
  plus the spike's checkpoint reports. Then `gbaa-assets check-load-log`
  writes `check.txt` and fails if a compressed blob that reached the screen
  isn't decompressed in the log at its source map size, a copied range's
  byte isn't in a load, or a tile shown on the last frame has no use.
  Emerald's new-game route, 2026-10-28: 37 of 37 blobs whole, 58 of 58
  ranges, 2,624 of 2,624 truck tiles (1,973 from a loaded blob, 651 the
  text window's, computed), every bank's palette found; about a minute
  with the build warm.
- `just play-extract [ROM] [OUT] [RUNNER] [ARGS…]`: play ROM (default
  Emerald) in the window and extract what you see (assets/play-extract,
  docs/asset-extraction-design.md, Extract while playing):
  `gbaa-desktop --extract OUT` (default `out/assets/emerald`) in a
  provenance build, RUNNER `interp` (`target/prov`) or `recompiled` (the
  ROM's crate, `just recompile`'s, built with `desktop/provenance` into
  `target/recompiled-prov`: about 6 min on Emerald, needs 7 GB available).
  Each session's logs go to `OUT/play-YYYYMMDD-HHMMSS/`; F6 and closing
  export every session and route under OUT into it (as `gbaa-assets
  graphics`, keeping the names its manifest gave; 0.2 s). Recording costs
  the recompiled build about 40% a frame (0.75 to 1.05 ms), far inside a
  frame's 16.7 ms.
- `just play-extract-check`: its check. Needs `out/assets/emerald`
  exported (`just assets-record emerald-newgame`, `just assets-graphics`).
  Copies it to `out/assets/play-check`, runs a session headless with
  `--extract` (the new-game route's presses, then Start and the Bag,
  6,800 frames, about 30 s), then `gbaa-assets check-extract` writes
  `play-*/extract-check.txt` and fails if a tile or palette blob the
  session showed and loaded has no manifest entry with files, an earlier
  entry is lost or renamed, or fewer than 5 are new. 2026-10-29: 95
  shown, 0 missed, 106 kept, 10 new (the Bag's sheet, screen and
  palettes).
- `just kirby-assets [ROM] [OUT]`: Kirby & the Amazing Mirror's player
  controller and sprites (research/kirby-controller-probe,
  docs/research/kirby-controller.md), into `out/assets/kirby/`. From
  reset, menu presses reach the first room. Then `scripts/kirby/moves.lua`
  presses a fixed list of moves once Kirby first stands, and logs
  `gKirbys[0]`'s state every frame to `moves/trace.txt`. It runs in the
  `provenance` build with `--assets-log`, as `assets-record` does (no
  route file: routes take presses only, not scripts). Then `gbaa-assets
  graphics` on the recording, `gbaa-assets sprite-tables` (the game's
  own sprite tables at `0x083B909C`: whole frames of the 46 animations
  plain Kirby uses as Aseprite and PNG, every animation's script and
  `AnimCmd_6` hitboxes; research/kirby-sprite-tables), and the mod
  view's cut of 11 controller functions named in
  `names/kirby-amazing-mirror.toml` into `controller/src/`. Fails unless the 12 move states all appear in the
  trace. About 25 s once built, and two runs match byte for byte.
- `just timing-lab [ROM] [OUT] [TRAIN] [FRAMES]` (research/why-timing-is-runtime,
  docs/research/why-timing-is-runtime.md): what each prefetch stall costs
  and what Kirby does with its timing wrong. Builds gbaa-desktop with the
  `timing-lab` feature into `target/timing-lab` (a hook in `Bus::stall`,
  `runtime::timing_lab`; `desktop/src/timing_lab.rs` reads
  `GBAA_TIMING_LAB`: `log`, `no-prefetch`, `zero`, `baked`, `table`).
  Logs TRAIN (3000) frames of a fixed input to `train.tsv`, then runs
  FRAMES (4500) frames logged (`base.tsv`) and once per wrong timing, each
  with `scripts/kirby/timing.lua` (frame counter, RNG, lag, Kirby's
  position), frame hashes, a WAV and dumps; `gbaa-timing-report` writes
  `report.txt` and `NAME/diff-N.png` (real | variant | difference). Into
  `out/timing-lab/kirby/`, about 90 s once built.
- `just fuzz-coverage [NAME] [WORKERS] [MINUTES] [ROM]` (default `spike`,
  10, 45): the fuzzing spike (assets/fuzz-coverage-spike,
  docs/research/fuzz-coverage.md). `crates/fuzz`'s `gbaa-fuzz`, built
  with `provenance` into `target/prov`, runs WORKERS processes (one
  machine each: `runtime::prov` is process-wide) of Emerald in the
  interpreter: the new-game route, then the crafted `alice.sav` (`just
  emerald-saves`), then random presses from kept states, mutated saves
  (party, map, flags, bag, Pokédex, vars) and RAM pokes, keeping each state
  that reached a new ROM instruction, ROM load or ROM byte in palette, VRAM
  or OAM. v2 (assets/fuzz-v2) adds sweeps, each worker taking every
  WORKERS-th item: a wild battle against each species, normal then shiny
  (started from the overworld by setting `gEnemyParty` and calling
  `CB2_InitBattle`), a battle against each trainer, a boot of each map with
  the bag full and the Pokédex complete; and a shared corpus in
  `out/fuzz/NAME/shared/` (each worker's coverage bitmaps, merged every two
  minutes, and its states that found 100 things or more, deleted when the
  run ends). Before fuzzing (assets/fuzz-dex-bag-sweeps), each worker
  pages its share of the national Pokédex's 386 entry pages (chunks of
  32, read back by `sPokedexListItem`) and scrolls its share of four bag
  passes (every item in some pocket), both opened from the start menu
  with the overworld state poked in RAM; about 90 s a worker with 13.
  After the PC passes (assets/fuzz-move-sweep), its share of the 354 moves, a wild
  battle each (1,800 frames): the lead, a random species at level 100
  and three quarters of its HP (so heal moves work), knows only that
  move (the eighth badge flag set, or a foreign mon that high ignores
  orders), against a level-100 Squirtle knowing only Splash (pure Water,
  so no move misses it for type), the battle scene on, A every 8 frames
  (FIGHT, then the first move). `MoveSetup` in `crates/fuzz/src/worker.rs`
  sets up the moves that need more (who sleeps, a Ghost for Curse,
  genders for Attract, the foe's hit for Counter and Mirror Coat, its
  level for one-hit KOs). A `move-N` row each in `w*/scenes.tsv`; about
  4 minutes a worker with 16. Then (assets/fuzz-move-sweep-foe-side) the
  same with the roles swapped, the wild mon using each move on a
  Squirtle lead, for the backgrounds drawn when the player's side is the
  target (`foe-move-N` rows, another 4 minutes); Spit Up and Swallow get
  a stockpile poked in RAM.
  Writes `out/fuzz/NAME/w*/` (`events.tsv`, each first sighting, `table`
  rows the computed bytes' guessed ROM tables;
  `kept.tsv`; `stats.tsv`), then `just fuzz-report NAME`:
  `report.txt` (route against fuzz: graphics as blobs plus copied ranges,
  the source map's data symbols, instructions, pret's functions against
  discovery's starts, screens by `gMain.callback2`) and `curve.tsv`.
  About 165 frames a second a worker with 10 running. `just fuzz-coverage
  NAME WORKERS MINUTES ROM STORY` (assets/fuzz-story-seeds) seeds every
  worker with the story-point saves in folder STORY (`just
  emerald-story`): each booted after the sweeps and kept as a story seed,
  never dropped, picked one time in three; and a first sighting of
  graphics counts 8 times an instruction's in picking. The folder's
  mGBA states (`state-N.ss`, `just emerald-tas`) seed them too: each
  loaded, run 60 frames unpressed and kept the same way. Then
  (assets/fuzz-contest-frontier-sweeps) each worker plays its share of 32
  scenes from the last story save: each contest (five categories at four
  ranks, entered by a script written to RAM that does the lobby
  receptionist's part) and each Battle Frontier challenge (its attendant's
  script started), A every 8 frames and some screens their own presses,
  30,000 frames a contest and 36,000 a challenge; where each
  got to, a row every 600 frames, in `w*/scenes.tsv`. Before the story
  seeds, after the bag passes (assets/fuzz-pc-box-sweep), each worker
  plays its share of four PC passes: the 14 boxes filled in RAM with
  every species and Unown's forms, the PC opened by
  `ShowPokemonStorageSystemPC` from a script, each box's 30 slots walked
  on its own wallpaper, then the box menu and the party; one pass in move
  items mode, one on Walda's wallpapers (each pattern and icon). About
  10,400 frames a box pass, 3,000 the Walda one; rows in `scenes.tsv`
  (`pc-N`). `gbaa-fuzz worker … --pc-only 1` stops a worker after them.
- `just kirby-fuzz [NAME] [WORKERS] [MINUTES] [ROM]` (default `kirby`,
  12, 45; assets/kirby-fuzz, docs/research/fuzz-coverage.md, Kirby):
  the fuzzer on Kirby & The Amazing Mirror. `gbaa-fuzz` picks the Kirby
  worker (`crates/fuzz/src/kirby.rs`) by the ROM's game code `B8KE`;
  the save argument is `-`. Each worker plays a new file to the first
  room and the timing lab's walk (the route phase), sends Kirby through
  a door to its share of the 983 rooms and gives him its share of the 26
  copy abilities (game functions called between frames,
  `core::call`), plays its share of 17 screens the rooms don't open in
  each of the six languages (assets/kirby-fuzz-screens: the logo, title,
  intro, menus, the four sub-games, the endings, game over, the link
  screen, the pause menu and the collection room with every treasure,
  reached through the game's own fade to a screen, `gLanguage` patched
  in a copy of the ROM) and the pause menu's help with each ability in
  each language, then fuzzes from kept states with RAM pokes (a warp,
  an ability, a refill, visited doors and treasures). The workers share
  `crates/fuzz/src/core.rs` with Emerald's. Then `gbaa-assets scan
  --run out/fuzz/NAME --curve 300` scores it against the static scan's
  1,202 LZ77 blobs, the only denominator with no decomp:
  `scan-curve.tsv` and `scan/` (the blobs missed, greyscale) in
  `out/fuzz/NAME/`. About 200 frames a second a worker with 12 running.
- `just emerald-tas [EVERY] [ROM]` (assets/fuzz-tas-bios): TASVideos
  movie 6182 (drkspace's glitchless living dex, BizHawk 2.9), fetched
  once into `$GBAA_DATA/tas/6182.zip` and checked by hash, replayed by
  `just mgba-dump-tas` on the official BIOS with a state every EVERY
  frames (default 100,000) into `$GBAA_DATA/saves/emerald/story/`, then
  `gbaa-fuzz story` on them. It stays in sync on that fork (it doesn't on
  0.10.5). The whole movie, 3,094,273 frames, takes about half an hour.
- `just emerald-story [ROM]` (assets/fuzz-story-seeds,
  `crates/emerald-save/src/story.rs`): `alice.sav` moved along the main
  story, a save a point of `crates/emerald-save/story.toml` (flags set and
  cleared, vars, key items and HMs as pret's scripts leave them, the
  party's level, where the player stands), each with every point before
  it applied, into `$GBAA_DATA/saves/emerald/story/NN-name.sav`. Names
  are pret's, read from its constants headers (`#define` expressions and
  enums). A point's map and tile are checked only by booting it: Continue
  (`mgba-dump ROM --save S --input start@300,a@900,a@1100 --frames 1600
  --dump F.png`) should stand the player there.
- `just assets-coverage [RUN]` (default `out/fuzz/spike`): Emerald's
  graphics coverage against pret (assets/coverage-report,
  docs/asset-extraction-design.md, Graphics coverage), the denominator for
  june's ~90% target. `gbaa-assets coverage` (`crates/assets/src/coverage.rs`)
  lists every `INCGFX` under pret's `src/` as a file (source and output
  kind; an aggregate's elements share its symbol, split evenly when
  uncompressed and alike), finds its ROM range by the source map's symbol,
  and leaves out the ones never used in play with the reason: no symbol in
  the ROM, no aligned ROM word pointing into it, `unused` or `debug` in
  pret's name or path, the cut content listed in the code, or a tileset
  palette slot the map loader never copies (a primary's 6-15, a
  secondary's 0-5 and 13-15), or a battle-animation sheet or palette
  whose `ANIM_TAG_*` no `loadspritegfx` in `data/battle_anim_scripts.s`
  (nor C indexing the tag table by it) loads. RUN is a
  `just fuzz-coverage` folder (its `packed` and `copied` rows, palettes by
  `load` rows too, 1bpp images such as footprints by `table` rows, and a compressed file by a `table` row at its blob's start: its unpacked data rewritten in code, as contest backgrounds and PC wallpapers are; scored as the route phase and as a whole) or a route's
  `manifest.toml`. Images are found with palette (a palette pret keeps in
  the same folder or by the same name was found; a manifest says itself),
  greyscale, or unpaired (pret keeps none beside it). Writes
  `coverage.txt` (by class and kind) and `coverage.tsv` (a line per file)
  beside RUN, and for a fuzz run `coverage-curve.tsv` (`--curve 300`,
  assets/fuzz-to-90: the used files found every 5 minutes of run time,
  from `events.tsv`'s seconds; about 5 s more). With `--scan` (the recipe passes it) a "scan only" column:
  files RUN missed whose LZ77 blob the static scan finds
  (`just assets-scan`). About 2 s once built. Needs `just fetch-pret` and
  `just pret-names`.
- `just assets-scan [ROM] [OUT] [RUN…]` (default Emerald,
  `out/assets/emerald`, no runs): the static scan (assets/static-scan,
  docs/asset-extraction-design.md, Graphics coverage). `gbaa-assets scan`
  (`crates/assets/src/scan.rs`) finds every LZ77 blob by Nintenlord's rule
  and writes the ones no RUN reached (a `just fuzz-coverage` folder or a
  route's `manifest.toml`) as greyscale 4bpp sheets, `OUT/scan/lz_<addr>.png`
  and `OUT/scan/scan.toml`, marked found by scan with no palette or
  layout. No pret needed; about 2 s once built. For a fuzz-run RUN it
  prints the blobs its route phase and the whole run reached, and with
  `--curve STEP` writes `RUN/scan-curve.tsv`, the blobs reached every
  STEP seconds of run time (assets/kirby-fuzz).
- `just assets-graphics [ROM] [OUT]`: Emerald's tile sheets, palettes and
  BG tilemaps (assets/graphics-export, docs/asset-extraction-design.md,
  Graphics) from every route recorded under `out/assets/emerald`
  (`just assets-record NAME` first): `tiles/*.png` (indexed, the palette
  drawn with most in PLTE, index 0 transparent; greyscale if never drawn),
  `palettes/*.pal` (JASC), `tilemaps/*.bin` plus a drawn `.png`,
  `manifest.toml`'s `[tiles]`, `[palettes]` and `[tilemaps]` (other
  sections kept), and `$GBAA_DATA/names/pokeemerald-assets.toml`, the
  source map layer naming blobs pret's map doesn't (`gfx_`, `pal_`,
  `map_<address>`). Routes recorded with an OAM log (`oam.tsv`) give the
  sprite sheets too (assets/sprites, the design's Sprites and animations):
  `sprites/*.aseprite` (a sheet's frames, then each animation's, tagged;
  a sprite of several entries on one canvas, its arrangements tagged
  `composed`, a layer an entry, assets/sprites-composed; each file read
  back through aseprite-io and asefile before it's kept), `sprites.txt`
  (sheets, animations, arrangements, OAM blobs that aren't a sheet and
  why), `[sprites]`, and sprite sheets' `tiles/*.png` a frame across
  (refilled from ROM a frame at a time, or one composed arrangement
  across) or down. Under a second.
- `just assets-check-graphics [OUT]`: each blob joined by its source map
  symbol to the `INCGFX` in pret's `src/` that defines it, then through
  pret's `tools/gbagfx` with its flags: bytes (our unpacked bytes equal
  pret's file converted), rebuilds (our file converted and compressed
  equals the ROM's bytes), palette (drawn sheets: PLTE against pret's PNG,
  colour 0 skipped for 4bpp), image (size and indices). Writes
  `out/assets/emerald/pret-check-graphics.txt`; fails below 81 at bytes or
  81 at rebuilds. New-game route, 2026-10-28: 85 joined, bytes 81,
  rebuilds 81, palette 25 of 31, image 20 of 57 (why, the design's "As
  built").
- `just assets-check-sprites [OUT]`: each `[sprites]` sheet joined by
  symbol to pret's PNG: its `tiles/*.png` (size and indices) and each
  frame of its Aseprite file's `sheet` tag against pret's PNG cut into
  frames of the sprite's size in reading order. Writes
  `out/assets/emerald/pret-check-sprites.txt` with the animations found;
  fails below 3 object-event and Pokémon strips at the image level.
  New-game route, 2026-10-28: 3 of 3 (Brendan walking, the moving box,
  Lotad), other sheets 8 of 12 (EMERALD VERSION composed, 2026-10-28),
  6 animations (a composed sheet's per-entry ones dropped).
- `just assets-check-overrides [RUNNER] [OUT] [RECOMPILED]` (assets/overrides,
  the design's Overrides): after `just assets-graphics`, every exported
  file (each sheet's PNG, each sprite's Aseprite file, each `.pal` and
  tilemap `.bin`) encoded back to its blob's bytes (`gbaa-assets
  check-overrides round-trip`), which also writes
  `OUT/overrides/unedited/mod.toml`, a mod of them all; then
  `OUT/overrides/edited/`, Brendan's walking sprite (Aseprite, a band
  across every frame: a raw blob, so a patched ROM copy) and the truck's
  tileset (PNG, a grid on every tile: LZ77, so served by the BIOS call)
  edited. Per RUNNER (`interp`, `recompiled` or `both`, the default) the
  new-game route plain, with the unedited mod and with the edited one
  (`gbaa-desktop --assets-mod DIR --assets-manifest OUT/manifest.toml`),
  frame hashes and checkpoint PNGs in `OUT/overrides/runs/`; fails unless
  the unedited run draws all 5,400 frames as the plain run and the edited
  run differs at frames 3800 and 5400. The recompiled runner recompiles
  Emerald into RECOMPILED (default `out/pokemon-emerald-version`) and
  builds it: heavy, about 8 minutes in all on quack. 2026-10-28:
  106 of 106 files round-trip; both builds draw the unedited mod's 5,400
  frames as plain play and the edited mod's 228 changed frames alike.
- Steam Deck build (deck/deck-build): `just deck-build ROM` recompiles and
  builds the game, then `scripts/deck-package.sh` fills `out/deck/<name>/`
  with what goes to the Deck (`rsync -a out/deck/<name>
  deck@steamdeck:gbaa/`, see docs/machines.md): `gbaa-<name>` (stripped, its ROM built
  in), `play.sh` (what the non-Steam game entry runs: the binary
  with `--fullscreen` and no ROM) and `README.txt` (adding it to Game Mode, controls,
  saves). It fails if the binary links a library outside SteamOS's base or
  needs glibc newer than 2.35 (a build machine's glibc 2.43 gave `atan2f`,
  `acosf`, `asinf` and `log10f` new versions; the desktop crate's
  `build.rs` links `sdl-build/glibc-compat.c`, which keeps Rust's calls on
  the old ones). Files already there (the `.sav`, states)
  are kept. `just deck-check <name> [SECONDS]` (30 by default) runs it through `play.sh` in
  a podman Arch Linux container (`scripts/deck-check.Containerfile`: ALSA,
  the X libraries SDL3 dlopens (libX11 and libXext; libXcursor, libXi,
  libXfixes, libXrandr, libxkbcommon) and Mesa, as SteamOS has) on a 1280x800 Xvfb screen; it fails on a missing library, a
  failed run, a window short of the screen or a picture not centred
  between black bars, and writes `out/deck/<name>-check/screen.png`.
  `--fullscreen` starts in SDL's desktop fullscreen: the monitor the
  window is on, at the size the desktop gives it (on KDE Wayland at 1.5x,
  2561x1601 pixels for a 2560x1600 panel: SDL rounds the logical size
  up), asked of the X11 window manager or the Wayland compositor, the
  frame at the largest whole scale that fits (5x, 1200x800, on the Deck),
  borderless and without a pointer; F11, Alt+Enter (no Start with it) or
  the menu's Fullscreen row switch it on and off while playing, back to
  the window's last size (stretch/fullscreen-toggle). It opens at the
  monitor's size, so an X11 screen with no window manager (the Deck
  check's Xvfb) is filled too; switching there needs a window manager.
  Windowed and fullscreen checks run in a private `cu session`, never
  on june's screen.
- ES-DE and other frontends (deck/frontend-launch): gbaa has no game
  library; a frontend launches `gbaa-desktop [flags] ROM`.
  `deploy/es-de/` has a sample ES-DE system entry for
  `~/ES-DE/custom_systems/` (`es_systems.xml`: gbaa for `.gba`, mGBA
  through RetroArch kept as the alternative; `es_find_rules.xml`:
  `gbaa-desktop` on the PATH or in `~/Applications/gbaa/`). ES-DE passes
  only the ROM, so saves land in the default folder (and a `ROM.sav`
  beside the ROM is copied in on the first run); EmuDeck- or
  RetroDECK-style launch lines can add `--save-dir`, `--state-dir` and
  `--config-dir DIR` (settings.conf, gamepad.conf and the rest, over
  `$GBAA_CONFIG_DIR`). A recompiled game's build carries its ROM, so it
  goes in as a script (its `play.sh`, say in ES-DE's ports system) rather
  than under `gba`. Checking a launch without a screen:
  `SDL_VIDEO_DRIVER=offscreen SDL_AUDIO_DRIVER=dummy gbaa-desktop
  --fullscreen --frames 120 ROM` with `HOME`/`XDG_*` pointed at a scratch
  folder runs the window path whole (`--frames` turns auto states off).
- macOS build (stretch/macos): `just macos-app ROM [ICON] [FLAGS]`, on a Mac only (Apple
  silicon; Xcode's command line tools, cmake, rustup; FLAGS go to
  `gbaa-recompile`, with `""` for no icon), recompiles and
  builds the game for `aarch64-apple-darwin` (macOS 11 and up, stripped at
  link), then `scripts/macos-package.sh` makes `out/macos/<name>/gbaa-<name>.app`
  and `README.txt`. The bundle holds the binary
  (`Contents/MacOS/gbaa-<name>`, its ROM built in), which keeps saves and
  states in `~/Library/Application Support/gbaa/saves/<name>/`, not in
  the bundle (`desktop::game_dir`). `ICON`, a square PNG (box art), becomes the
  app's icon (sips and iconutil); the menu bar shows the ROM's file name.
  It fails if the binary isn't arm64 Mach-O or
  links a library outside macOS; the bundle is signed ad hoc and checked
  with `codesign --verify` and `plutil`. Why a Mac rather than a
  cross-compile from Linux: docs/research/macos-build.md. Cmd+Q quits there,
  as Ctrl+Q does.
- Windows build (stretch/windows): `just windows-exe ROM` recompiles the
  game and cross-compiles it for `x86_64-pc-windows-gnu` with MinGW-w64
  in a podman image (`scripts/windows.Containerfile`, built once by
  `just windows-image`: Fedora's MinGW, rustup's stable, Wine), then
  `scripts/windows-package.sh` puts `gbaa-<name>.exe` (stripped at link, its
  ROM built in) and `README.txt` in `out/windows/<name>/`; it fails if the
  .exe imports a DLL that isn't part of Windows. Started with no ROM
  (double-clicked), `gbaa-<name>` runs the ROM it carries, its save
  `<name>.sav` beside it.
  Config files are in `%APPDATA%\gbaa\` there. Heavy, as
  `build-recompiled`; its cargo output is in `target/windows/`. `just
  windows-check NAME [THROUGH]` (`route-check --windows`) runs
  `windows-exe` and `build-recompiled` for units/NAME.toml's ROM, then
  both headless to the route's first checkpoint (or THROUGH), the .exe
  under Wine in the container, and fails unless every frame's hash, the
  checkpoint PNGs, the last registers and the `.sav` match and the
  checkpoints draw the route's hashes; outputs in
  `out/units/NAME-windows/`. Why and how: docs/research/windows-build.md.
- `just native-spike` (native/agb-spike; docs/research/native-agb-spike.md):
  `agb/native-spike` (D-pad moves a sprite, a camera scrolls BG0; its
  `SCENE.md`; built by `just agb-tests`) recompiled at `--timing exact`
  and built, and `crates/native-spike`, the same program lifted by hand
  to native Rust (`native-spike --headless --frames N --input P
  --frame-hashes F`), built plain and with `--features plant` (one wrong
  store). `native-spike-check` runs the three on
  `crates/native-spike/check.toml`'s 600-frame script (which must hold a
  D-pad key in every 60 frames) into `out/native-spike/check/`; fails
  unless play and native hashes match on every frame and the planted
  build's differ. Passes with 510 distinct pictures, hashes file
  `5539f4ec2af9bb48`, planted caught at frame 212 (2026-11-01). Heavy:
  builds a generated crate.
- `just native-check` (native/oracle, native/hw-sound;
  docs/native-port-design.md, section 7): the native check. Recompiles
  `agb/native-spike`, `agb/psg-square` and `agb/direct-sound` (`just
  agb-tests`) at `--timing native` and builds them, builds
  `crates/native-spike`'s programs in the check layout (`--features
  guest-layout`: RAM as the ROM's EWRAM and IWRAM, each global at its
  link address), the spike also in the play layout, the spike and
  psg-square with their planted faults, then runs `route-check --native NATIVE PLAY
  ROUTE [--replaced FILE] [--packed BIN] [--planted BIN]...` on
  `units/native-spike.toml` (600 frames), `units/psg-square.toml`
  (120) and `units/direct-sound.toml` (300). Each build writes every
  frame's record at its present
  (`--native-record`, `crates/native-record`); the play build runs a
  second time with `--interp`, whose records must be the compiled run's
  and which reports the lowest sp (compiled code doesn't keep it). Fails
  at the first difference, naming the frame, region and byte, in the
  picture, EWRAM/IWRAM (but the stack from the lowest sp to IWRAM's end,
  code copies, and `crates/native-spike/check/*.replaced.toml`: agb's
  globals, heap and VRAM free list, which the native programs replaced),
  the I/O registers the library models (display, `DISPSTAT`'s writable
  bits, `VCOUNT`, the sound registers with `SOUNDCNT_X`'s status, wave
  RAM, the FIFOs' last stores, the timers, `IE`/`IF`/`WAITCNT`/`IME`),
  palette, VRAM, OAM, and each frame's audio (samples' count and hash):
  `runtime::hw` mixes with the play build's own sound path
  (native/hw-sound), so psg-square's FIFO A and PSG and direct-sound's
  two FIFOs with their timer-IRQ buffer swaps are compared sample for
  sample. The play layout must draw the same frames, and the planted
  faults must be caught: the spike's wrong store, psg-square's one wrong
  sample (`--features plant`). Play's sound against a native program's
  none (a host that records none) fails unless `--no-audio WHY`. A lowest
  sp outside IWRAM fails; a code copy's skip covers any variables copied
  with the code. Passes with 0 differences and audio the same in every
  frame for all three (2026-11-06). Writes `out/native-check/`. Heavy:
  builds three generated crates.
- `just lift ROM [OUT] [FLAGS]` (native/lift-stage-a;
  `docs/native-port-design.md`, "The stage-A lifter as built"): the
  stage-A lifter. `gbaa-lift ROM OUT [--name NAME] [--holes FILE]
  [--bars FILE] [--names DIR] [--layout guest|packed] [--drop-standin KEY]
  [--plant AT] [--copies N]` (`codegen::native`) writes the whole
  program to OUT (default `out/lift-<name>`) as `src/`: `main.rs` (the
  prelude, RAM, `ENTRY`), `fns.rs` (`FNS`) and the functions in modules
  of about 4,000 lines, `m_<first key>.rs`, which rustc builds in parallel
  (`codegen::native::modules`, native/lift-modules; `--copies N` writes
  them N times over under new names, a synthetic large lift for measuring
  the build); every
  function plain Rust over byte RAM (`Game::ram`, EWRAM then IWRAM) and
  `runtime::hw`, a RAM access at a constant address a typed global's
  field (`g.ram.d_020000c0`; `Game::ram` is a `Ram` struct of them at the
  guest's offsets, `--layout guest`, the default, which derefs to
  the bytes; `--layout packed`, the play layout, puts them in the
  lifter's order with the rest of RAM as `bytes`, and `g.ram[offset]`
  finds a byte in the field that holds it, native/lift-packed-layout; names from DIR's names files for this ROM, default `names/`;
  native/ram-globals), its registers locals, blocks a `loop { match }` over
  local labels, calls Rust calls; then the recipe builds it into
  `target/recompiled` (`gbaa-native-<name>`, the native programs' headless
  flags, `crates/native-host`). It reads `holes/<name>.toml` when there is
  one, else `mods/<name>/holes.toml` (a commercial game's, private: its
  stand-ins are ports of the game's code); `<name>` is `--name`'s when
  given (a file name with spaces: `just lift "…/Metroid - Zero Mission
  (USA).gba" out/lift-metroid --name metroid`), else the ROM's file name
  lowercased. The file has `[[standin]]` (a function's hand-written Rust,
  and why), `[[jump]]` (an unresolved jump's targets as keys, its cases), `[[copy]]` (code
  the game copies to RAM that discovery's static copies don't have:
  `src`, `dst`, `len`, `roots` and why, as `just holes` prints its run's
  copies; lifted at its RAM keys like a static copy, native/lift-ram-copies)
  and, at the top,
  `skip_fn_flow = "why"` (the function-address flow left out, saving
  the minutes it runs before its token bound stops it). A hard hole no stand-in
  covers (CPU state, an unresolved jump, a BIOS call the prelude lacks, an
  undefined instruction, a jump to no code, a summary that returns r4-r11
  or sp changed) fails the lift, naming each, and the build (a
  `compile_error!` each). `report.txt` lists the soft holes and ends
  with the metrics as `name value` lines (`codegen::native::metrics`):
  lines; functions (lifted, recognised, unrecognised, replaced, with a
  summary hole, four-register without one, bar 0 since `native/lift-params`); callee-saves unproven; hard
  holes; blocks; every load and store by how its address is known
  (`const` by region, `sp`, `computed` by the region the value-set
  analysis bounds it to, else `computed_unknown`; regions `ram`, `io`,
  `rom`, `display`, `sram`, `other`; `const_ram` counts only byte
  accesses, the globals' are `ram_global`, unbarred, with `ram_globals`
  the count of globals; `computed_ram` likewise, the ones the value
  sets bound to one global are `ram_object`, unbarred, checked by the
  prelude's `pin` and `ix` at run time, and `pointer_unexplained` the
  rest, native/ref-in-object; one the value sets don't bound that the
  heap's run types is a heap object's field through `Ref<T>`, counted in
  `ref_accesses` and `ref_types`, unbarred, and `ref_unexplained` those
  through an object's pointer no type explains, native/ref-heap); `call_fn` calls; jumps by value;
  gap targets. `--bars FILE` fails the lift (after writing it) on any
  metric over `[program.NAME.bars]`'s maximum, each named with both
  numbers (`gbaa-lift: affine: load_computed_unknown is 755, over its
  bar of 754`), and lists the metrics under their bars. Discovery's
  gaps (a jump into ROM it has no code at) are decoded by the lifter and
  listed (a `bl` whose return site is a known literal pool is taken never
  to return). `--drop-standin KEY` and `--plant AT` are the check's planted
  hard hole and wrong store; `--fall-as-tail-call` prints a fall into
  another function's entry as the old tail call (r4-r12 and the flags
  zeroed, native/lift-fallthrough), the fallthrough control. About 6 s to lift, 15 s-2 min to build.
  The lift runs capped at 16 GB, no swap (a commercial game's needs several).
- `just lift-check [--only NAME]` (native/lift-stage-a, native/lift-bars):
  the lifter's check, `gbaa-lift-check` driven by
  `crates/codegen/lift-check.toml`: one table per program (`rom` under
  `$GBAA_DATA`, `route`, optional `lift_flags`, `route_flags`,
  `planted_store`, `planted_hole`, `planted_flags`) and its bars,
  `[program.<name>.bars]`. Today `agb/native-spike`, `agb/affine`,
  `agb/mosaic-modes`, `agb/native-fallthrough`, `agb/native-irq-hold`,
  `agb/native-frame-locals`, `agb/native-ime-release`,
  `agb/native-bios-calls`, `agb/native-tasks`, `agb/native-dma`,
  `agb/native-save` (SRAM), `agb/native-bios-region` (the BIOS latch;
  `just agb-tests`; native-bios-calls is native/lift-bios-calls) on
  `units/<name>-600.toml` (the spike's is `units/native-spike.toml`,
  native-irq-hold's, native-ime-release's and native-bios-region's
  `-601`). First every program is lifted with `--bars`
  (seconds), failing before any build on a metric over its bar; then each
  lift and its play build (`--timing native`) are built and `route-check
  --native` runs them for 600 frames, capped at 3 GB, with no `--replaced`
  list: everything but the stack and the code copies is compared. The
  spike lifted with its `planted_store` (0x08000186, the V-blank handler's
  frame count) must be caught, and with its `planted_hole` (portable_atomic's
  `interrupt::disable` stand-in dropped) the lift and the build must fail
  naming `f_08002728`'s `mrs`, a critical section's, which the lifter
  leaves hard (it prints only the reset's and the IRQ wrapper's); native-tasks likewise (a task's store; its yield's
  stack switch); native-dma's (0x08000180, `dma_set`'s `DMAxCNT`, a unit
  more) and native-save's (0x080002ac, an SRAM `strb`) too.
  native-fallthrough, native-irq-hold, native-frame-locals and
  native-ime-release lifted with their `planted_flags`
  (`--fall-as-tail-call`, the old fall into another function;
  `--irq-at-waits`, the old IRQ run at a wait inside a critical section;
  `--plant-slot=0x08000182`, `fl_keep`'s store to a stack local made one
  more, native/frame-locals; `--ime-at-waits`, the old IRQ held past the
  store that sets `IME` again to the next wait, native/ime-release) must
  be caught too. Each program is lifted once more with `--layout packed`
  (`out/lift-<name>-packed`), passed as `--packed`: it must draw the guest
  layout's frames and record the same bytes, nothing skipped
  (native/lift-packed-layout). Passes with `every metric within its bar`,
  a `pass (60x frames, 0 differences …; the play layout draws the same 60x
  frames, its records the same …)` line per program, `planted fault caught
  (…-planted: frame 2: EWRAM byte 0x020000c0 …)`, `(…-fall-as-tail-call: frame 1: EWRAM byte 0x020000c4 …)`,
  `(…-plant-slot-0x08000182: frame 1: EWRAM byte 0x020000c4: play 0x61,
  native 0x62 …)` and `(…-irq-at-waits: frame 2: EWRAM byte 0x020000c8:
  play 0x00, native 0x01 …)`, `(…-ime-at-waits: frame 2: EWRAM byte
  0x020000cc: play 0x01, native 0x00 …)`, `planted hard hole caught` and
  `lift-check: passed`. Logs in `out/lift-*.txt`. About 10 minutes on
  quack. Heavy: builds twenty-nine generated crates; gbaa-lift-check runs capped at 8 GB (its lifts in its scope; route-check in a 3 GB scope and the builds in their own).

  `gbaa-lift --shadow-locals` (native/frame-locals; by hand, not part of
  the check) keeps every stack local in the frame's bytes too and prints
  `shadow local: AT loads X from lN, the frame's bytes hold Y` wherever a
  load differs: a write to the frame the escape analysis didn't see. Lift
  with `--name <name>-shadow`, `just build-recompiled`, then run
  `target/recompiled/release/gbaa-native-<name>-shadow --headless --frames
  601` and grep its stderr.

  **Lowering a bar** (every stage-B unit's check): `just lift-metrics
  --lower-bars` (`--metric`, `--only` to lower only some), or change the
  one `metric = N` line in the program's bars table, and run `just
  lift-check` (or `--only NAME`). Bars are only lowered; raising one is changing the
  oracle (ask june). The exception is `lines_prelude`, the hand-written
  prelude every lift starts with: it may rise when a unit adds prelude
  code, the evidence saying how many lines and why; `lines` is the rest
  (lifted code and stand-ins) and only goes down. A new metric needs a bar in every table (the lift
  fails on a metric with none); a new program is a new table, its bars
  at the values its first lift reports. `cargo test -p codegen --test
  lift_bars` shows a lowered bar failing the lift by name.
- `just lift-metrics [--only NAME]… [--metric REGEX] [--lower-bars] [--write-doc FILE]`
  (native/lift-bars): `gbaa-lift-check --metrics-only`, step 1 of `just
  lift-check` alone. Lifts every program of
  `crates/codegen/lift-check.toml` (the file is the list; a new table is a
  new column) with its `lift_flags` and `--bars`, into the same
  `out/lift-<name>` as the check, and reads each lift's
  `metrics.toml` (gbaa-lift writes it beside `report.txt`). Prints a
  table, a row per metric, a column per program: `v` at its bar, `v<b`
  under bar b, `v>b!` over it, `v` alone for a metric with no bar.
  `--metric REGEX` keeps the metrics whose names match, `--only` the
  programs. `--lower-bars` rewrites `lift-check.toml` (comments and
  order kept, `toml_edit`) with every shown bar that is over its metric
  lowered to the metric, and prints each `program: metric old -> new`;
  it never raises a bar (that stays a hand edit with a reason). It builds
  no crate and runs no route check and none of the planted controls, so
  it isn't the check: run `just lift-check` before calling a unit done.
  Fails on a metric over its bar. About 15 seconds (the lifts run at
  once).
  `--write-doc docs/native-stage-b.numbers.md` also writes the table as
  that generated numbers file (header, date, commit): rerun it when a
  unit changes the metrics, rather than copying numbers into the plan.
- `just native-ten [ROM]` (native/emerald-ten; scores in
  `docs/native-port-design.md`, "Emerald's ten, scored"): fn-diff for
  native functions (`crates/native-fn-diff`). Emerald's ten, lifted by
  hand in `mods/emerald/native-ten/` (private), built as the harness's
  `native-ten` binary (`--features emerald`). It takes each function's
  parameters from its DWARF prototype (the summary where none), records
  entry states over 600 no-input frames (borrowed for functions not
  entered), runs 96 input variants of each (every global the ten name
  scrambled first), and compares RAM (minus the stack below the entry
  `sp`), the display, `DISPSTAT`/`IE`/`IF`/`IME`, the timers, the beam's
  line and timer counters as read, every other I/O register unchanged,
  and the result,
  against the interpreter run untimed. Then one byte of Random's literal
  pool changed must be caught. ROM defaults to pret's build in
  `$GBAA_DATA/src/pokeemerald/`. Passes with `native 0 of 10 differ`
  (2026-11-01). About 30 s, most of it discovery and summaries; capped at
  3 GB. Not heavy (no generated crate).
- `just first-frame ROM [FRAMES] [TIMING]` (#32, milestone 2's check):
  `recompile-check` (at TIMING, exact by default; `native` for the
  native port's timing, decision 11), then the two PNGs must be identical. The recompiled
  run's `cycles:` line (in `out/<name>/recompiled.log`) splits guest time
  between compiled code and the fallback, how much of each ran in RAM
  (decisions 1 and 7; code copied to IWRAM is compiled as overlays, #54),
  and the log ends with `overlay invalidations: N`. It passes on june's ROM
  (`$GBAA_DATA/agb/gba-cart1.gba`), jsmolka's `arm`/`thumb`, and the agb
  sprite example (`$GBAA_DATA/tools/obj-sprites/obj-sprites.gba 120`,
  hash `afc0b93200e5577f`).
- DMA sound (#52, `runtime/src/sound.rs`; design in
  `docs/sound-design.md`): FIFOs A/B popped by timer overflows (walked one
  at a time by `sound::run_timers`) and refilled by FIFO-timed DMA1/2;
  `Platform::audio` gets an `AudioChunk` (raw FIFO samples and `i16`
  stereo). PSG channels 1–4 (`runtime/src/sound/psg.rs`, mGBA's model;
  "The PSG" in `docs/sound-design.md`) are mixed into each record; with
  no FIFO timer running, the grid's samples make the stream (`--wav`,
  playback) on their own.
  `gbaa-desktop --headless` takes `--wav F`,
  `--fifo-dump F`, `--psg-dump F` (the PSG's samples on mGBA's grid, `i16`
  LE pairs; prints `audio psg dropped N`), `--audio-hash` (prints `audio N
  records hash H` and `audio dropped N`) and `--audio-log F`.
- Desktop audio (#53, `desktop/src/audio*`; "Playback" in
  `docs/sound-design.md`): the window plays the sound through `cpal` and
  the device paces the emulator (`--mute`, or no device: a 59.73 Hz frame clock;
  headless never opens a device). Fast-forward (`desktop/src/fast.rs`):
  Tab held or ` toggled, or the pad's `fast`/`fast-toggle` (right/left
  stick click) runs the window at `--fast-speed N` times (1.1-16,
  fractional, or `unlimited`; else `fast-speed = N` in
  `settings.conf` in the config folder, else 2.5),
  sound muted (the queue emptied), redrawn on a fixed 60 Hz schedule (`DrawPacer`); `--fast-forward`
  starts it on. `--volume PCT` (0-200; 100 is the default level,
  `audio::DEFAULT_GAIN`'s -6 dB, 200 the stream ungained; else `volume =
  PCT` in `settings.conf`, else 100, which the menu's Volume row
  rewrites) sets the master gain for the window,
  `--link` and netplay, and headless for `--audio-pace`'s `--played-wav`
  (`desktop/src/settings.rs` reads and rewrites the file). Without sound the window paces by a 59.73 Hz frame clock. `just audio-pace [ROM]` runs the pacing
  simulation on a ROM's stream; `just audio-window [ROM]` runs the window
  with sound in a `cu session`, against its `cu-silent` null sink. `just audio-check ROM
  [FRAMES]` (heavy) needs identical audio hashes interpreted and recompiled,
  no dropped samples, and 280896/period samples a frame from frame 10;
  passes on agb ROMs at 18157 Hz (304) and 10512 Hz (176).
- `gbaa-present-sim [--scale X]… TIMES…` (deck/even-presents): what a
  60 Hz display shows of a single game's frames. TIMES: `gbaa-desktop
  --headless --frame-times` files (least of two runs on a busy machine);
  `--scale` multiplies costs (0.7 recompiled; 2.2 the Deck, estimated
  from its 2-2.5x quack's, not measured). Prints doubled, skipped, late
  and lag for frames handed over when done, 8 ms in, and after the
  slowest recent frame. Numbers: `docs/sound-design.md`, "When a frame…".
- mGBA oracle (#55, milestone 3): `just mgba-diff ROM FRAMES [INPUT]` runs
  ROM in mGBA (`crates/mgba-dump`, over a static libmgba that
  `scripts/fetch-mgba.sh` builds once into `$GBAA_DATA/tools/mgba/`, under
  the build lock) and in our interpreter, and fails unless the last
  frames' hashes match; on a mismatch it writes `mgba.png`, `ours.png` and
  `diff.png` to `out/mgba-diff/<name>/`. Known mismatches, with both frames'
  hashes pinned, are in `crates/mgba-dump/known-mismatches.tsv`; a listed
  ROM that matches fails until its line goes. `just mgba-oracle` runs it on
  milestone 3's ten ROMs. Without libmgba the crate still builds and
  `mgba-dump` says how to get it. The cartridge clock (`runtime::gpio`,
  for the RTC games in mGBA's override table) reads
  `runtime::gpio::FIXED_CLOCK` (2004-09-16 12:00:00 UTC) in `mgba-dump`
  and in every headless run of ours, so hashes don't depend on the day;
  only the desktop window follows the wall clock.
- Sound oracle (#63): `mgba-dump --fifo-dump F --audio-log F` writes
  mGBA's FIFO samples in `gbaa-desktop --fifo-dump`'s format (one `A B`
  pair per timer overflow that pops a FIFO, caught by linking libmgba with
  `--wrap=GBAAudioSampleFIFO`), `--psg-dump F` the PSG's samples (from
  `--wrap=GBAudioSamplePSG`, started after a state load) and `frame
  records` lines. `just mgba-audio-diff ROM FRAMES [STATE]` runs both and
  fails unless the FIFO and PSG dumps are byte-identical, printing the first differing record, its offset from
  our nearest buffer swap, and mGBA's shift when its stream is ours
  delayed; outputs in `out/mgba-audio-diff/<name>/`. Run it from an mGBA
  state (`mgba-dump ROM --frames 100 --save-state S`): from reset agb's
  boot runs faster in ours (no wait states) and the music starts 3 buffers
  early. Passes on the 18157 Hz ROM (600 frames) and the 10512 Hz ROM (1000) from
  their frame-100 states. `just psg-square [FRAMES]` runs it on
  `agb/psg-square` (built by `just agb-tests`) for 119 frames from mGBA's
  state after frame 1: the PSG square channels' check. A run whose last
  V-blank falls in the second half of a 1024-cycle sampling period ends
  with one PSG sample more in ours (ours runs that V-blank's DMA before it
  presents the frame; mGBA ends the frame first), so keep 119.
  `just psg-wave-noise [FRAMES]` does the same on `agb/psg-wave-noise`:
  the wave and noise channels' check.
- Loudness (volume/measure-level): `mgba-dump --wav F` writes mGBA's
  mixed output (every sample `GBAAudioSample` makes, at its default 100%
  volume, before its resampler; 16-bit stereo at 32768 Hz or what
  SOUNDBIAS sets, the run before a rate change stretched to the last
  rate). `loudness A.wav [B.wav] [--frames F..G]` (`crates/mgba-dump`)
  prints peak, RMS and BS.1770 LUFS, and B minus A. `just audio-loudness
  ROM FROM TO [INPUT]` runs both to frame TO and compares frames FROM–TO;
  WAVs in `out/audio-loudness/<name>/`, a few seconds to half a minute.
  Results: "Level" in `docs/sound-design.md`.
  The desktop's own output, with its default gain, comes from
  `gbaa-desktop ROM --headless --frames N --audio-pace 0 --played-wav F`
  (48 kHz); measure it against `--wav` with `loudness` (the "Applied"
  table there).
- Save states (decision 16): `mgba-dump ROM --frames N --save-state S`
  writes mGBA's state after frame N, uncompressed (`GBASerializedState`,
  0x61000 bytes, which mGBA loads too); `--state S` on `mgba-dump` and on
  `gbaa-desktop` (and so recompiled builds) starts from one instead of
  reset, with `--frames` and `--input` counting from it. The runtime's
  loader is `runtime::state`: it unwinds a wait in mGBA's HLE BIOS
  (`Halt`, `IntrWait`) into ours and refuses a state stopped elsewhere in
  the BIOS and one from another ROM (CRC-32). mGBA's GUI states (`.ss0`,
  a PNG screenshot with the state and the save data in chunks) load too:
  `desktop::state_file` unpacks them for both tools, and the save data
  replaces the cartridge's in both, as mGBA does. A raw state carries no
  save data. Unit
  states live in `$GBAA_DATA/states/`. `just savestate-check ROM N K`
  proves loading on a ROM where mGBA and we agree: K frames from mGBA's
  state at N must draw what N+K from reset do, in mGBA and our
  interpreter, and the recompiled build must match the interpreter from
  the state (passes on an agb ROM at 100 100).
- TAS replay (assets/fuzz-story-seeds): `mgba-dump ROM --frames N --bk2
  LOG` plays a BizHawk movie's input log (`Input Log.txt`, unzipped
  from the `.bk2`) instead of `--input`: line f is mGBA's frame f+1, as
  BizHawk's mGBA core runs it, and the power button resets.
  `--save-state-at N,N --state-dir D` writes the raw state after each
  frame listed (`D/state-N.ss`), and `gbaa-fuzz story D` prints where
  each is in Emerald's story (map, badges, party, story flags). About
  3,000 frames a second. With `--bk2`, mGBA is set up as BizHawk sets
  it for a movie: no idle loop skips, and the clock from `--clock-from
  T` (Unix seconds, default BizHawk's 2010-01-01) plus `frame * 4389 >>
  18` seconds. `--bios F` (assets/fuzz-tas-bios) runs a BIOS dump, its
  intro included, as BizHawk runs one in a movie, in place of mGBA's HLE
  BIOS (june's: `$GBAA_DATA/bios/gba_bios.bin`). Emerald TASes made on
  BizHawk's mGBA core desync under it either way: their mGBA isn't
  0.10.5 (docs/research/fuzz-coverage.md, Story seeds). `just
  mgba-dump-tas ARGS` runs the same tool linked to BizHawk 2.9's mGBA
  (TASEmulators' fork at 645282a, built by `scripts/fetch-mgba.sh tas`
  into `$GBAA_DATA/tools/mgba-tas/`, the binary in `target/mgba-tas/`),
  on which movie 6182 stays in sync with `--bios` (`just emerald-tas`).
- Our own save states (`runtime::snap`, `desktop::slots`): all of the
  machine at a frame boundary, the cartridge save included, versioned
  (`snap::VERSION`; a test pins the payload's length, so a layout change
  must bump it; older versions back to `snap::OLDEST` still load, a field
added since skipped by its version and pinned by an old state's test) and
keyed to the ROM's CRC-32. `gbaa-desktop --state-out
  FILE [--state-at N]` writes one after frame N (else the last); `--state
  FILE` loads ours or mGBA's; `--frame-hashes FILE` writes every frame's
  hash. Headless, `--frame-times FILE` writes every frame's run time
  (`frame N us T`, wall clock) and `--state-bench N` saves and loads the
  state N times after the last frame, whole and as the bare payload, and
  prints sizes and times (Emerald's numbers: docs/research/multiplayer.md,
  "Latency over the internet"). The window's slots are `NAME.state1`-`4` in the game's folder: Shift+F1-F4
  saves, F1-F4 loads; on a pad `save-state` saves to the slot last used,
  and `load-state` loads it, if bound (no pad control by default). F5 or the menu's "Undo load state" row puts back the game as it
  was just before the last load (once, as mGBA; no pad button, june's
  choice for now). `just own-state-check ROM N K [INPUT]` saves at N in a run of N+K
  frames and fails unless K frames from the state hash as frames N+1..N+K
  did, in the interpreter and the recompiled build (INPUT's presses must
  end by N).
- Resume on launch (`desktop::resume`, deck/resume-on-launch): the window
  keeps an auto state, `NAME.resume` in the game's folder (apart from the slots),
  every 180 frames of play (about 3 s, at least 1 s apart), right after
  each `.sav` write and on quit, and starts from it at launch. Each is
  written by its own thread to `NAME.resume.tmp`, fsynced and renamed in,
  the one before kept as `NAME.resume.old` (launch's fallback). The
  state's cartridge save is kept unless the `.sav` is newer than the auto
  state; a run that faults moves it to `NAME.resume.fault`. Off by default
  (deck/resume-off-default): `resume = on` in `settings.conf` or the
  menu's "Resume on launch" row turns it on; turning it off deletes
  them. `--no-resume` boots fresh for one run.
  Scripted window runs (`--frames`, `--input`, `--pad-script`) and
  `--no-save` keep none. Checked 2026-10-30 in a `cu session` on Kirby &
  the Amazing Mirror: kill -9 with the newest 1.1 s old, relaunched in the
  same spot; Ctrl+Q wrote one at quit and relaunched there; presents' p99
  18.6 ms, none over 25 ms, with the writes.
- Rewind (`desktop::rewind`): the window keeps the machine's state
  (`snap`'s payload, `save_payload`) after every frame, the newest whole
  and each older one as a delta from the one after it (equal runs skipped,
  about 0.25 ms a frame); R held, or the pad's `rewind` (right stick
  pushed left), steps back a frame a frame, sound muted, and the cartridge
  save goes back too. The oldest go past `--rewind-mb` (default 256; 0:
  off). Measured a minute of rewind (3600 frames, 2026-10-22): WarioWare
  31 MB, Emerald 22, agb sound ROM 13, Super Mariomon 7, 10512 Hz ROM 6,
  moonbeam 2, so 256 MB holds 8 minutes or more, a few % of the Deck's
  16 GB. Headless, `--rewind F+N` holds it for N frames after frame F,
  and stderr ends with `rewind: … MB a minute`. `just rewind-check ROM N K
  [INPUT]` fails unless a run rewound K frames at N shows every frame as
  the straight run drew it, in the interpreter and the recompiled build.
- Cartridge saves (`runtime::save`, `desktop::sav_file`): the window keeps
  the save in `NAME.sav` in the game's folder, mGBA's file (32 KB SRAM, 64 or
  128 KB flash, 8 KB EEPROM, then mGBA's 16-byte RTC record, kept as it is), written
  half a second after the game stops changing it and on exit; `--no-save`
  keeps none; on exit it copies it to `backups/NAME-YYYYMMDD-HHMMSS.sav`
  beside it. Headless runs keep none unless given `--save FILE`, so
  checks stay reset-clean. The game's folder (`desktop::game_dir`,
  docs/save-locations-design.md) is `saves/NAME/` in gbaa's data dir:
  `$XDG_DATA_HOME/gbaa` or `~/.local/share/gbaa` on Linux and the Deck,
  `~/Library/Application Support/gbaa` on macOS, `%APPDATA%\gbaa` on
  Windows; NAME is the game's name (a build's baked-in `--name`, else the
  ROM's file name, lower case, dashes for the rest). On a game's first run
  with none of its files in the folder, the window copies in the
  `ROM.sav`, `ROM.state1`-`4`, `ROM.resume` and `.resume.old` beside the
  ROM (a build's own: `NAME.gba` beside the binary), renamed to NAME,
  originals kept (`game_dir::move_in`); never over a file there. A
  launcher names the folder itself with `--save-dir DIR` (exact: every
  file in DIR, NAME-named) and `--state-dir DIR` (the slots and resume
  file; default: where the `.sav` goes); a flag folder's first run copies
  in the default folder's files first, else those beside the ROM, so a
  new launcher doesn't strand saves. Under the flags, the menu's Save
  folders page (`desktop::menu::saves`) sets this game's folder (exact:
  `save-dir` in `games/NAME.conf` in the config folder) and every game's
  (a parent, NAME/ inside: `save-dir` in `settings.conf`), with a folder
  picker a pad drives; the game's row is greyed "set by the launcher"
  under `--save-dir` or `--save`. A change counts from the next launch,
  whose first run in the new folder copies the files in from the folder
  the game used (`moved-from` in its file, settings.conf's
  `previous-save-dir`, then the default). `mgba-dump --save FILE` loads one into mGBA,
  to compare: work on a copy, never the owner's `~/ROMs` file.
- Emerald link-test saves (multiplayer/cable-club-saves, `crates/emerald-save`):
  `just emerald-saves [ROM]` writes `$GBAA_DATA/saves/emerald/alice.sav`
  (ALICE, female, ID 12345: Mudkip L5, Wurmple L3) and `bob.sav` (BOB,
  male, ID 24680: Torchic L5, Zigzagoon L3), outside git like ROMs. It
  runs the ROM to the new game's truck in the interpreter (the new-game
  route's presses), takes the game's own three save blocks as the base,
  patches in the trainer, party (dex marked), `FLAG_SYS_POKEMON_GET` and
  `FLAG_SYS_POKEDEX_GET` (every cable club attendant wants the dex) and a
  continue-game warp to Oldale's Pokémon Center 2F at 10,3, below the
  cable club attendant, and writes slot 1's 14 sectors. Continue (Start
  at 300, A at 900 and 1100) stands there by ~1300; then Up, A: "Welcome
  to the POKéMON CABLE CLUB", and Trade Center passes the party check to
  the save prompt (mGBA and ours, 2026-10-25). About 20 s, the same files
  every run (sha1 85a1e042… and 0e121747…). Run on a copy: the cable club
  saves the game. Struct offsets came from the ELF's DWARF
  (`arm-none-eabi-gdb -batch -ex 'ptype /o struct SaveBlock1' ELF`).
- The link hub's check (multiplayer/link-hub, `runtime::link`):
  `just emerald-link [ROM] [ARGS]` runs alice.sav (the parent) and bob.sav
  (the child) as two Emeralds on one cable in the interpreter: continue,
  Up, then A every 60 frames from 1400 to 3400 through the attendant,
  Trade Center, the save and the parent's confirmation. It prints each
  player's `gLink.state` and `gLinkStatus` changes, runs it all twice and
  passes when both reach `LINK_STATE_CONN_ESTABLISHED` (frame 3383) and the
  Trade Center (3679), with the same log and machine states both times
  (2026-10-25: 6201 transfers in 4000 frames). About a minute. `--input0`
  and `--input1` replace a player's presses (`a@900`, `a@1400-3400/60`,
  `+held`), `--dump DIR --dump-at N,N` writes both players' frames.
- The wireless adapter's check (multiplayer/wireless-adapter-local,
  `runtime::link::rfu`): `just emerald-wireless [ROM] [ARGS]` runs
  alice.sav and bob.sav as two Emeralds, each with an emulated wireless
  adapter in one radio, in the interpreter. Both continue, walk to
  Oldale's Union Room attendant (down two, left four, up two: the
  counter's posts block row 3) and go in (about frame 3500). BOB
  registers his Torchic at the room's attendant wanting WATER; ALICE
  checks the trading board, offers her Mudkip, BOB accepts, and the
  trade and its save run (back in the room by about 10200). It prints
  each player's map, `gMain.callback2`, the adapter's state and ID,
  position and broadcast as they change, and each adapter's commands;
  runs it all twice and passes when both were in the Union Room, the
  first Pokémon swapped, and the two runs printed the same and ended in
  the same machine states (2026-11-08: 10500 frames, about 2 minutes a
  run). `--once` skips the second run; `--input0`/`--input1` replace a
  player's presses; `--dump DIR --dump-at` takes frames as `N,N` or
  `FROM-TO/EVERY`.
- The radio on the server (multiplayer/wireless-radio-netplay):
  `just emerald-radio [PROFILE] [ROM] [ARGS]` is `emerald-wireless --net
  PROFILE`, the same trade with each Emerald in its own process in real
  time and the radio a `gbaa-server` room's (in-process; each path
  through the ping simulator's PROFILE, default calm). About 3 minutes.
  `--sim-ms N` (on `just emerald-wireless`) is it on simulated time in one
  process, N ms each way; `--heard-ms N` (default 3000; 0: only the hosts
  open now); numbers: research multiplayer.md, "The radio on the server".
- Wireless from the desktop (multiplayer/desktop-wireless-radio,
  `crates/desktop/src/wireless.rs`, which says how it runs):
  `gbaa-desktop ROM --server ADDR --wireless --host` (prints `room CODE`;
  `--players N`) or `--wireless --join CODE`: the window's GBA with a
  wireless adapter whose radio is the server room's, as `emerald-wireless
  --seat`; its own GBA only, interpreted, real time, from reset. A radio
  silent for 10 s is said (a server older than the radio, as fly's was).
  From the window (multiplayer/menu-wireless-room): the Netplay page's
  Link row, on Wireless, makes Host a room and Join wireless; the game
  alone after it goes on from the seat's state.
- Linked play: `gbaa-desktop ROM --link B.sav` runs a second GBA on the
  same ROM, on one cable with the first (`runtime::link`;
  `crates/desktop/src/link.rs`), the two screens side by side in one
  window; player 2's save is kept in `B.sav` as `--save` keeps player
  1's. Player 1 plays on the mapping's keys and the first pad plugged in,
  player 2 on I J K L, N = A, B = B, U = L, O = R, `.` = Start, `,` =
  Select and the second pad. Interpreter only; no fast-forward, rewind,
  save states or scripts yet; player 1's sound. Headless, `--input2`
  presses player 2's buttons, and `--dump`, `--hash` and `--dump-at` take
  both screens (480×160). `just emerald-trade [ROM] [--keep DIR]` is its
  check: copies of alice.sav and bob.sav through the cable club to a trade
  of their first Pokémon, each player first opening its Summary and
  backing out (8400 frames, `crates/emerald-save/src/bin/emerald-trade.rs`),
  passing when the saves the desktop wrote back hold the trade
  (2026-10-25: Mudkip and Torchic swap; the trade's link save is done by
  frame 8200). The Summary clears VRAM with one long `CpuSet`, which
  dropped the link while the BIOS ran it at once (2026-10-26). About a
  minute.
- Netplay in the desktop (multiplayer/desktop-netplay, `crates/desktop/src/room.rs`):
  `gbaa-desktop ROM --server ADDR --host` makes a room on a
  `gbaa-server` (ADDR a host or IP, port default 7374) of two seats, or
  `--players N` (2 to 4: Kirby's four, multiplayer/four-seats), prints
  `room CODE`, puts it in the window's title and waits until every seat
  is taken (the title counts them); `--server ADDR --join CODE` takes the
  next free seat (codes in any case), so players are numbered in the
  order they join, and one more is told the room is full. Every peer
  runs every seat's GBA, all on one cable.
  The window's menu does the same with no flags (multiplayer/
  menu-netplay, its Netplay page: F10 or Start+Select): the server
  (fly's 37.16.3.41 until set, then remembered as `server` in
  `settings.conf`), Players (2 to 4; A or right steps it), Host a room
  (the code shown big, and how many are in), the code to join
  typed on the keyboard or a pad (up/down step a letter), Leave the room;
  in a session it shows the seats, round trip, lead, rollbacks and frames
  run again. The game alone stops for the session and comes back after
  it, and neither restarts it, as a GBA doesn't when its cable goes in
  or out (multiplayer/join-without-restart): your seat starts from the
  game as it was when you asked for the room (or from `--state`), and
  the game alone goes on from where your seat left off. Checked
  2026-11-08 on quack, two Kirby windows in a private `cu session`, each
  at game select with a cursor moved since its `--state`: the session
  began on each window's own screen, player 2 left and played on in
  Rainbow Route, player 1 hosted again from its cable screen, and a new
  player 2 joined: Kirby's cable screen saw P1 and P2, both reached
  Rainbow Route, no desync in 1860 frames. With `--frames N` each run stops at N frames, and the window closes
  when a session ends there. Checked 2026-10-29 on quack: two Kirby
  windows in a private `cu session`, through a local `gbaa-server`, hosted
  and joined from the menus alone, both ending 3600 frames on one hash.
  It plays by rollback (`netplay::rollback`, plain UDP through the
  server's rooms; `--rollback` still parses and changes nothing), the
  only netplay since multiplayer/remove-lockstep-quic (2026-10-28).
  `--delay N` is the host's input delay (default 0).
  A player who quits or leaves the room says goodbye; one who sends
  nothing for 8 s (`room::QUIET`: a crash, a lost link, a long `SIGSTOP`)
  is taken to have left (multiplayer/peer-leaves). The other window then says so (a toast over
  the game, the Netplay page, the title; `left player N at frame F` on
  stdout), leaves the room, and plays on alone with the cable unplugged
  (`Gba::unplug`), so the game shows its own link error.
  In a room a small mark in the window's top right corner (`menu::Mark`,
  multiplayer/connected-mark) says how it is: connecting, waiting for
  players (1/2; 3/4 in a room of four with three in), connected (2/2,
  4/4), not hearing from them (nothing for 0.75 s, `room::SHAKY`) or lost
  (1/2, playing alone). With three or four players, one leaving unplugs
  every remaining player's cable too. Seen 2026-10-31 in a
  private `cu session` on quack, Kirby through a local `gbaa-server`, the
  joiner stopped with `SIGSTOP`.
  A desync (the peers' hashes of a frame differ; multiplayer/desync-report)
  is said when found: a toast and the mark (out of sync, red), a line on
  the Netplay page, `netplay notice: …` on stdout; the end lines add
  `netplay desyncs: N …, at frames …` (or `none`), and a headless run
  with one keeps its report in `$GBAA_DATA/netplay/` too. `--desync-at N`
  (`--desync-at1/2` in `gbaa-pingsim desktop`) forces one: from frame N
  this peer runs its own seat holding A (`Rollback::skew`).
  Both peers run both GBAs on one cable (`netplay::gba`), interpreted in
  `gbaa-desktop`; a recompiled game's binary (`gbaa-<game>`) runs them on
  its code (multiplayer/recompiled-netplay; `--interp` forces the
  interpreter), each console on a guest stack of its own (multiplayer/
  netplay-guest-stacks, 2026-10-29), and a function that never returns
  (the main loop) printed v1 so a rollback's restore resumes it compiled
  (multiplayer/netplay-compiled-after-restore, 2026-10-31): a structured
  build runs as much compiled as a v1 build (`just recompile ROM OUT
  --no-structured`), about as fast (Kirby on wander, quack: about 70%,
  3.4–3.7 ms a frame). The
  build ID names which, so every peer must run the same build, or every
  peer the interpreter. Headless, a recompiled run's end adds `netplay
  code: N% of cycles compiled, …`. The window shows your seat alone, keeps your save as
  `--save` does (default in the game's folder) and discards the other's. Your
  seat's sound plays and paces the window as the single window's does
  (`--mute`, or no device: a frame clock; `netplay::gba::Speaker`, which
  doesn't change what the consoles compute); the menu doesn't pause a
  session (multiplayer/menu-no-pause): the game plays on under it, your
  seat's pad let go, and the others never wait on it; no
  fast-forward, rewind, save states or scripts. The window's consoles get
  the host's wall clock, headless the fixed one. Headless it stops at
  `--frames`, answers 2 s more, and prints `netplay: room …, end HASH, …`
  (both consoles' hash: every peer's must match). `just emerald-netplay-trade
  [PROFILE] [ROM] [--keep DIR] [--stop SEAT@SECS+DUR]` is its check, by
  rollback at delay 0: `emerald-trade --netplay --profile`, the linked
  trade with Alice's desktop hosting and Bob's joining through a server's
  plain-UDP rooms started in-process (`relay::rooms::serve`), each path
  through the ping simulator's PROFILE (default calm); the presses land
  on the frames they name. Passes when both saves hold the trade
  and both players end on one hash, no desync, neither taken to have left
  (`--stop 2@20+4`: player 2 SIGSTOPped 20 s in for 4 s). Calm 160 s,
  rough 128 s with ~20% lost, both end 35d6b1be… (2026-10-28, quack). Seen in a private
  `cu session` too: two windows through a real `gbaa-server`, each its
  own seat, 60 fps with 10 waits, the trade in both. With sound
  (multiplayer/netplay-sound, 2026-10-25, quack): the host's window in a
  `cu session` on `cu-silent`, an agb ROM, 900 frames, 0 underruns,
  presents 16.7 ms apart, both peers ending on one hash.
- `just rollback-feel [PROFILE] [ROM] [ARGS]` (multiplayer/
  rollback-design; `gbaa-pingsim desktop`): two `gbaa-desktop` netplay
  peers (`--players N`: up to four, a second apart, `--input3`/`--input4`
  and the like for the others' own options; multiplayer/four-seats)
  through a local server's rooms, each joiner behind a proxy
  following `crates/netplay/profiles/PROFILE.toml` (default wander), ROM
  default Kirby & the Amazing Mirror. Windows to feel it (keyboard to the focused one, a pad to
  both); with `--headless --paced --frames N --input1 S --input2 S`,
  numbers: each peer's end lines (rollbacks, frames run again, freezes,
  frame times and how many ran over a frame, round trip, how far ahead
  of the other each ran), and it fails if the peers' end hashes differ. A run
  is real time (a minute per 3600 frames). `--delay D` goes to the host
  (0 by default); `--exe BIN` runs BIN for both
  peers instead of `gbaa-desktop` (a recompiled game's binary, from
  `just build-recompiled`, in `target/recompiled/release/`).
  Kirby's multiplayer from reset: `start@400,start@700,a@800,a@950,
  down@1050,a@1100` on both, then `a@1820,a@1950,a@2100` on player 1.
  `--state1 F`.. start a seat from a state (as a window's game goes into
  its session): Kirby from game select, player 1 at frame 1000 and
  player 2 at 1500 of the presses above, `--input1
  'down@50,a@100,a@820,a@950,a@1100,right@1200-1800/18+9' --input2
  'down@30,a@80,left@1200-1800/18+9' --frames 1800 --timing exact` on
  calm reach Rainbow Route on one end hash (`cdc7b0d1…`, 2026-11-08).
  `--menu1 SPANS` and `--menu2 SPANS` (headless) open each seat's menu
  over those frames as a player would (`gbaa-desktop --menu`: the
  seat's presses let go, the menu's report refreshed), the check of
  multiplayer/menu-dropout: the menu, which used to pause the session,
  had stopped the host answering, so the other peer froze 5 s ahead
  and, past 15 s more, ended the session ("the other player stopped
  answering") and played on alone (2026-10-31, quack: a SIGSTOPped
  host shows both). With `--menu1 '1200-2900/40+8' --menu2
  '1200-2900/40+8,1215-2900/97+3'` on wander and rough, 3000 frames:
  frozen 0 times, both seats on one hash (7a288ae6…).
- The server on fly.io (multiplayer/play-gate, `deploy/fly/`): the app
  `gbaa-server` (region ewr, one shared-cpu machine, **stopped between
  june's playtests**: `just fly-up` before playing or a test that needs
  it, `just fly-down` after) runs `gbaa-server fly-global-services:7374
  --min-version N`, the address fly routes a public UDP port to, turning
  away builds older than netplay version N (`netplay::VERSION`, raised by
  hand with the Dockerfile's N: docs/research/multiplayer.md,
  "Versions"). It keeps no files, so it
  needs no volume (the volume `gbaa_data`, which held the QUIC
  certificate, is unused since multiplayer/remove-lockstep-quic and
  still billed until june destroys it: `fly volumes destroy`). `just
  fly-setup` once (billed: a dedicated IPv4; fly serves UDP only on
  one), `just fly-deploy` after any server change (`--ha=false`: rooms
  live in one process, so the app must have one machine; built by fly's remote
  builder from `deploy/fly/Dockerfile`, the repo root as context,
  `.dockerignore` keeping out builds and data). The address is
  37.16.3.41 (`fly ips list -a gbaa-server`). Players: `gbaa-desktop ROM
  --server 37.16.3.41 --host`, the friend `--join CODE`; `just room-sim
  calm --server 37.16.3.41` checks it end to end with the toy game.
- Emerald Rogue's co-op (multiplayer/rogue-coop, `desktop::rogue`):
  `gbaa-desktop "Emerald Rogue.gba" --rogue ADDR` is Rogue Assistant
  built in (docs/research/multiplayer.md, "Rogue Assistant"). Each player
  runs their own game and picks Host or Join from the PokeConnect clerk
  in the hub; the host listens on ADDR's port (30025 if left out), the
  client connects to ADDR. ENet over UDP, as the assistant speaks it, so
  the other player may be on mGBA with Rogue Assistant. Prints `rogue:`
  lines to stderr as it connects. `cargo test -p desktop rogue` checks it
  over localhost against a stand-in for the game's side.
- The netplay session core's check (multiplayer/session-core, `crates/netplay`):
  `just emerald-netplay [ROM] [ARGS]` plays emerald-link's walk into the
  Trade Center (4200 frames) straight, both Emeralds on one cable with
  each frame on its own presses (input delay 0), then as a two-peer
  rollback session on `netplay::sim`'s simulated network: each peer runs
  both GBAs, starts with only its own save and takes the other seat's
  state from its owner, and inputs cross with 40 ms latency, up to 30 ms
  jitter and 10% loss. Passes when both peers' hashes every 60 frames
  and at the end are the straight run's, with no desync, and both
  players reached the link and the Trade Center (by lockstep, 2026-10-25:
  70 of 70 hashes, 7965 transfers). About 90 s. `--delay D`,
  `--latency MS`, `--jitter MS`, `--loss P`, `--seed N` change the
  network; `cargo test -p netplay` checks the same with a toy console and
  a tiny key-reading ROM, and that a deliberate desync is seen by every
  peer.
- The variable-ping simulator (multiplayer/ping-sim, `crates/netplay`):
  `just ping-sim [PROFILE] [ARGS]` runs `gbaa-pingsim run`: a UDP proxy
  (`netplay::proxy`) between two peer processes of `gbaa-pingsim peer`,
  by rollback, each on `netplay::udp` (unreliable datagrams; reliable messages in
  1200-byte chunks, acked and resent), playing in real time. The profile,
  `crates/netplay/profiles/PROFILE.toml` (default `rough`), sets the
  one-way latency, jitter, a sine wander, random spikes, loss, reordering
  and duplicates (fields: `netplay::proxy::Profile`). Passes when both
  peers' hashes every 60 frames and at the end are a straight run's, with
  no desync; prints the proxy's counts. `--console gba` plays two GBAs on a
  cable running a tiny key-reading ROM, `single-pak` homebrew booting a
  cartless seat by `MultiBoot`; `--frames N` (600), `--delay D` (0), `--fps
  F` (60), `--linger-ms M`. The pieces alone: `gbaa-pingsim proxy PROFILE
  A_ADDR B_ADDR`, `peer SEAT ADDR`, `straight`. `cargo test -p netplay
  --test ping_sim` runs every profile (GBAs, single-pak on `rough`), 20 s.
- The feel sim (multiplayer/rollback-feel-sim, `netplay::feel`):
  `just rollback-feel-sim [PROFILE] [ARGS]` runs `gbaa-pingsim feel`:
  every peer in one process, in simulated time, through the profile's
  network, each driven as `gbaa-desktop`'s netplay loop (`--pacing
  audio|muted|paced`) onto a 60 Hz display. A line a seat: frames late,
  doubled, skipped, rollbacks and depth, holds, the other player's
  jumps, lag to screen; before 2026-11-08's fixes and after
  (`--version`). Turns cost a model of two recompiled Kirbys (ms a run),
  or with `--rom ROM` a trace of ROM run once straight on interpreted
  GBAs, timed on the thread's CPU clock (~90 s for 5400 frames;
  `--trace-out F` keeps it, `--trace F` reuses it). Other ARGS: the
  justfile's comment. `cargo test -p netplay feel` holds the budgets.
- The server (multiplayer/relay-server, then multiplayer/remove-lockstep-quic,
  `crates/relay`): `gbaa-server [ADDR] [--min-version N]` serves
  plain-UDP rooms (`relay::rooms`, default `0.0.0.0:7374`): it seats the
  players by a six-letter code and passes their datagrams on, and keeps
  nothing on disk, so players need only its address. `--min-version N`
  refuses clients older than netplay version N, saying "this game is too
  old for the server (version V, needs N): get the new build"
  (multiplayer/server-version-cutoff). A player's seat is freed when
  it leaves (`L`) or sends nothing for 10 s, and a room closes with its
  last player; it says so on stderr (`room CODE opened`, `player 2 left`,
  `closed: every player gone`); `W` datagrams speak to a room's radio
  (`relay::radio`). `gbaa-server --rooms ADDR` asks one how
  many rooms are open and how many players are in them (no codes). `just room-sim [PROFILE]
  [ARGS]` is its check: `gbaa-pingsim room` starts a server in-process
  (or uses `--server ADDR`) and a proxy (`netplay::proxy`, `Proxy::to`)
  per peer between it and the server, both following the profile
  (default calm); seat 0 (`gbaa-pingsim room-peer 0 ADDR`) makes a room
  and prints its code, seat 1 joins by it, and they play the toy game by
  rollback over `netplay::udp` through the rooms. `--players N` (2-4)
  makes the room N seats and starts N - 1 joiners, each through a proxy
  of its own; `--players 4 --console gba` is four GBAs on one cable in
  every peer (multiplayer/four-seats, 2026-10-31: wander, every seat on
  the straight run's hashes). Passes as `ping-sim` does; ARGS are its. `cargo test -p relay` covers the rooms; `cargo test
  -p netplay --test ping_sim` runs calm, spiky and rough with GBAs
  through a local server.
- Factory units (`docs/factory.md`): `just unit NAME [THROUGH]` runs
  `route-check` (crates/route-check) on `units/NAME.toml`, a route: `rom`
  (may use `$HOME` and `$GBAA_DATA`), optional `state` (an mGBA save state
  to start all three from, frames counted from it), `[[press]]` tables
  (`button`; `at`, or `from`, `to` (inclusive) and `every`; optional
  `hold`, default 6 frames) and `[[checkpoint]]` tables in frame order
  (`name`; `frame`; `hash`, what our interpreter must draw; where mGBA is
  known wrong, `mgba_hash` with `mgba_why`; optional `min_compiled`, the
  least share of running guest cycles to that frame the recompiled build
  may spend outside the interpreter fallback, 0 to 1; optional
  `block_timed`, frames between the checkpoint before and this one where
  the recompiled picture may differ from the interpreter's because compiled
  code ticks once per block (an I/O store mid-block lands before the
  scanlines catch up), with `block_timed_why`; it fails once one of them
  matches again). The format, with an
  example, is in `crates/route-check/src/route.rs`; comments say how each
  hash was verified. A ROM may have several routes where paths diverge
  (the intro untouched, a new game). It clears `out/units/NAME/`, runs
  mGBA, the interpreter and the recompiled build once each to the last
  checkpoint (or THROUGH), each writing every frame's hash
  (`*-hashes.txt`) and each checkpoint's PNG (`mgba/`, `interp/`,
  `recompiled/frame-N.png`), and fails at the first checkpoint, by name,
  where mGBA doesn't draw `hash` (or `mgba_hash`), our interpreter
  doesn't (with a `diff-NAME.png` and the first frame ours parts from
  mGBA's), or the recompiled build's frames, last registers or compiled
  share part from the interpreter's. It prints `unit NAME: pass (...)`.
  Exit 1 is a FAIL; exit 75 (`could not run`) means a build refused or
  broke on this machine (often free RAM), not a verdict; exit 2 a bad
  route file. A new checkpoint's `hash` is mGBA's: `target/release/mgba-dump
  ROM --frames N --input S --frame-hashes F` (S as `out/units/NAME/`'s
  runs pass it) writes every frame's. Heavy: builds the game's crate in
  `out/<rom>/` once for all the route's checkpoints. `target/release/route-check
  --args units/NAME.toml` prints the arguments every runner gets, then
  `--timing exact` (a route is a check; `route-check --timing T …`, any
  of its forms, runs our builds and the recompile at T instead, e.g.
  `native` for `--same-states BIN "BIN --interp"`), one a line, to run another build
  along the route (v1 against structured:
  `mapfile -t a < <(route-check --args …); BIN "${a[@]}" --headless`).
