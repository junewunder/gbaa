# Where a game's saves, states and resume file live

Settled with june on 2026-10-30, answering "Where should a game's saves,
states and resume file live?" (q/where-game-s-saves-states). Today the
`.sav`, the four `.stateN` slots and the `.resume` auto state sit beside
the ROM (`sav_file.rs`, `slots.rs`, `resume.rs`), and backups go in
`saves/` beside the `.sav`. This replaces that.

## The game's name is the key

Each game is filed under a readable name, not its ROM hash: lowercase,
with every run of other characters becoming one dash, so it is a safe
folder name on any OS. "Kirby & The Amazing Mirror" becomes
`kirby-the-amazing-mirror`. This is the rule `just recompile` already
uses for `out/<name>/`.

- A recompiled build carries its name inside it. By default the name comes
  from the ROM's file name, and whoever runs the recompile can set it
  (`--name`). That person names the game, so Emerald and a Kaizo Emerald
  are two builds with two names, and a new version of a hack keeps the old
  name so it keeps its saves. gbaa ships no table of known ROMs, doesn't
  remember names per hash, and doesn't ask "same game or a different one?"
- The interpreter is only a fallback (we want to ship as little of it as
  possible). It uses the same rule on the ROM's file name, so renaming
  the file there loses the link to its saves, and that's accepted.

## Where the folder is

Highest first:

1. Command-line flags. `--save-dir DIR` is the exact folder for this
   game's files, not a parent (as RetroArch's `--save DIR` works).
   `--state-dir DIR` does the same for states and the resume file, and
   follows `--save-dir` when not given.
2. A per-game setting in the game's menu ("keep this game's saves in…"),
   also an exact folder. When a flag is in charge, the menu shows it
   greyed out with "set by the launcher". The flag wins for now.
3. A global setting in settings.conf ("keep all saves in…"). This is a
   parent: each game gets a `<name>/` folder inside it, so one synced
   folder (Syncthing and the like) can hold every game.
4. The default, also a parent: `<OS data dir>/gbaa/saves/<name>/`. That's
   `~/.local/share/gbaa/saves/<name>/` on Linux and the Deck
   (`$XDG_DATA_HOME`), `~/Library/Application Support/gbaa/saves/<name>/`
   on macOS and `%APPDATA%\gbaa\saves\<name>\` on Windows.

The two settings live in the menu's Save folders page, a pad's folder
picker for each. The game's is `save-dir` in `games/<name>.conf` in the
config folder, the global `save-dir` in settings.conf. A change counts
from the next launch: this run's files stay put, and that launch copies
them into the new folder (as on any first run there, never over a file),
from the folder the menu moved them from (`moved-from`, or the global's
`previous-save-dir`) before the default and beside the ROM.

ES-DE passes no save location to emulators (its launch lines only fill in
the ROM), so for ES-DE it's the default that counts. The flags are for
EmuDeck/RetroDECK-style setups and hand-written launch lines.

## File names

Files are always named after the game, even inside its own folder, so a
copied file still says what it is and a flag can point many games at one
shared folder: `<name>.sav`, `<name>.state1`–`.state4`, `<name>.resume`
(with its `.old` and `.fault` companions), and the timestamped backups in
`backups/` in the same folder.

## States and the resume file across versions

A `.sav` is the game's own data, so a new version of the game reads it,
and sharing it across versions is right. A state is a snapshot of the
whole machine, and whether one still loads after a ROM update or a gbaa
rebuild hasn't been tested. june relies on the resume file in place of
saving, so refusing it would be worse than the risk. For now:

- Each state and resume file records the hash of the ROM that made it
  (its CRC-32, in the header `runtime::snap` writes).
- On a mismatch it loads anyway, after copying the old file aside
  (`<file>.bak`, as `game.state1.bak` or `game.resume.bak`), so a bad
  load can't destroy it (`slots::load`). mGBA's states, imported rather
  than ours, are still refused from another ROM.
- A research unit tested loads across a version change and across
  rebuilds (below). The layout stays.

What the test found (2026-10-30, Emerald; one of our states made by the
interpreter 200 frames after the story's mGBA state at frame 100,000,
then 600 frames with walking input):

- **Rebuilds of the same ROM** load and play exactly: the interpreter and
  four recompiled builds from 2026-10-28 to 2026-10-30, a mod copy and
  the text-speed mod among them, drew the same 600 frames from it, and a
  recompiled build's state loaded in the interpreter. The payload is the
  guest machine alone, and a resume landing mid-function falls back to
  the interpreter, so codegen changes don't reach it.
- **A change of the state's format** does: builds from 2026-10-24 read
  version 2 and refuse version 3 ("a gbaa save state of version 3; this
  build reads version 2"). Since then a build reads every older version
  back to `snap::OLDEST` (2): a field added in a version is skipped in
  older states and loads as its default, so a format bump no longer
  strands states and resume files made before it. A test loads a pinned
  state of each older version (`snap/v2-counter.state.zz`).
- **A data-only revision** (a ROM byte changed where no code is; another
  CRC-32, same code) loads after the `.bak` copy and plays the same 600
  frames as the original.
- **A revision whose code moved** (pokeemerald built with the modern
  compiler) loads, then crashes in its first frame: the state's return
  addresses and function pointers point into the old layout (here, a
  jump to 0). So a state from another ROM is on trial for its first
  second (`slots::Trial`): if the game faults in that time, the machine
  is put back as it was before the load (for an auto state on launch,
  the game starts afresh), the window's title says why, and the `.bak`
  keeps the file.

## Saves already beside a ROM

The first time a game runs and its folder has no save, gbaa copies the
`.sav`, states and resume file it finds beside the ROM (or beside the
built game) into the folder, renaming them to the game's name, and leaves
the originals in place. After that it uses only the folder. It never
copies over a save already there.
