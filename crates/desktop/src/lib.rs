//! The desktop front end: a window (SDL3) with integer scaling and
//! keyboard and gamepad input,
//! or a headless run that dumps the last frame as a PNG and prints its
//! hash, and the sound as a WAV and a FIFO dump. The window plays the
//! sound through the host's audio device, which paces the emulator.
//!
//! `gbaa-desktop` is [`main`] with no recompiled code. A recompiled game's
//! generated crate links this library and calls `main(Some(&CODE),
//! Some(Embedded { rom, name }))`, its ROM and the game's name embedded, so it has the same command line and the first-frame check can compare the two
//! (docs/codegen-design.md).

#[cfg(feature = "provenance")]
mod assets_log;
pub mod audio;
mod capture;
pub mod console;
mod coro;
mod dump;
mod egui_sdl;
pub mod fast;
pub mod game_dir;
pub mod gamepad;
mod input;
pub mod link;
pub mod menu;
pub mod menubar;
mod platform;
#[cfg(feature = "provenance")]
mod prov_report;
pub mod remap;
pub mod resume;
pub mod rewind;
pub mod rogue;
pub mod room;
pub mod sav_file;
mod scripting;
pub mod settings;
pub mod slots;
pub mod state_file;
#[cfg(feature = "timing-lab")]
mod timing_lab;
pub mod window;

pub use capture::{Capture, wav};
pub use dump::{frame_hash, write_png, write_png_over};
pub use input::Script;
pub use platform::Desktop;

use runtime::{Machine, Timing};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

/// The machine the desktop runs: generated code names it through this.
pub type DesktopMachine = Machine<Desktop>;

/// A recompiled game's code: the table its generated crate passes to
/// [`main`] (docs/codegen-design.md).
pub type Code = runtime::dispatch::Code<Desktop>;

pub const USAGE: &str = "\
usage: gbaa-desktop ROM [options]

  A game's binary, gbaa-NAME, carries the ROM it was recompiled from and
  runs that when no ROM is given. A ROM given runs instead. The window
  keeps a game's save, states and resume file in its own folder,
  saves/NAME/ in gbaa's data dir (~/.local/share/gbaa on Linux and the
  Deck, ~/Library/Application Support/gbaa on macOS, %APPDATA%\\gbaa on
  Windows), NAME being the game's name: the one baked into the build, or
  the ROM's file name, lower case, dashes for the rest. On a game's first
  run there, the save, states and resume file beside the ROM (ROM.sav and
  so on) are copied in under NAME; the originals stay. The menu's Save
  folders page can keep this game's in a folder of its own, or all games'
  in one (each in NAME/ inside it), from the next launch. A launcher may
  name the folder itself with --save-dir and --state-dir, over both.

  --headless       no window: run --frames frames, then exit
  --frames N       stop after N frames (headless default: 60)
  --state FILE     start from an mGBA save state (uncompressed, as
                   mgba-dump --save-state writes, or the GUI's PNG .ss
                   file, whose save data replaces the cartridge's)
                   instead of reset, or from one of ours (--state-out,
                   or a window's slot); frames count from it
  --state-out FILE write our own save state after the last frame (or
                   after --state-at N frames), which --state loads
  --state-at N     with --state-out: save after frame N, then run on
  --assets-mod DIR serve the edited assets of the mod at DIR (its
                   mod.toml's [assets]: manifest address = file) when the
                   game loads them, laid out as --assets-manifest's
                   export says (docs/asset-extraction-design.md)
  --extract DIR    a provenance build (`just play-extract`): record every
                   asset shown as you play into DIR/play-YYYYMMDD-HHMMSS/
                   (the load, use and OAM logs, as `just assets-record`'s)
                   and export the graphics of every session and route
                   under DIR into it (tiles, palettes, tilemaps, sprites,
                   manifest.toml) on F6 and on exit, keeping the names an
                   export there gave (docs/asset-extraction-design.md,
                   Extract while playing)
  --frame-hashes F write each frame's hash (as --hash prints it) to F;
                   frames shown while rewinding are written too
  --frame-times F  headless: write each frame's run time to F, as
                   `frame N us T` lines (wall clock, microseconds)
  --state-bench N  headless: after the last frame, save and load our state
                   N times each, whole (as --state-out) and as rewind's
                   payload, and print the sizes and times
  --rerun-bench N  headless: after the last frame, run the next frame N
                   times drawn and N undrawn from rollback's snapshot
                   (as netplay runs a frame again), and print the times
  --rewind F+N     headless: after frame F, hold rewind for N frames
                   (step back N frames), then let go
  --rewind-mb N    cap what rewind keeps at N MB (default 256; 0: no
                   rewind in the window)
  --save FILE      keep the cartridge save in FILE, an mGBA .sav (the
                   window's default: NAME.sav in the game's folder,
                   ~/.local/share/gbaa/saves/NAME/ on Linux, NAME the
                   game's name; headless runs keep none unless given
                   this); on closing, the window copies it to
                   backups/NAME-YYYYMMDD-HHMMSS.sav beside it (local
                   time) unless the newest there is the same: backups
                   to go back to
  --save-dir DIR   window: keep the game's files in DIR itself (NAME.sav,
                   its backups/, the state slots, NAME.resume), not in
                   the default folder; for launchers. The first run with
                   none of the game's files in DIR copies them in from
                   the folder it used before, else from beside the ROM
  --state-dir DIR  window: keep the state slots and NAME.resume in DIR
                   itself (default: where the .sav goes)
  --no-save        window: keep no .sav (and no auto state)
  --no-resume      window: start the game from its start this time, not
                   where the last run left off (NAME.resume in the
                   game's folder, an auto state kept every few seconds of play and
                   on quit; off unless `resume = on` in settings.conf,
                   or the menu's row, turns it on)
  --dump FILE.png  write the last frame as a PNG on exit
  --hash           print the last frame's hash (FNV-1a 64) on exit
  --dump-at N,N    headless, with --dump-dir: write frame N as DIR/frame-N.png
                   after it runs; a recompiled build also prints its cycle
                   split so far to stderr (`at frame N cycles: ...`); a
                   build with the provenance feature also writes
                   DIR/frame-N.prov.txt and .blobs.tsv (src/prov_report.rs)
  --dump-dir DIR   where --dump-at's PNGs go
  --dump-states    with --dump-at, also write our state (as --state-out)
                   after each of those frames, as DIR/frame-N.state
  --native-record FILE  headless: write each frame's RAM, I/O, display
                   memories and audio to FILE as each is presented, and
                   (with --interp) the lowest sp reached to stderr at the
                   end, for the native check (route-check --native;
                   crates/native-record)
  --regs           print the CPU registers on exit, once the CPU has
                   stepped on to the head of the loop it is in
  --input PRESSES  press buttons on given frames, e.g. 'start@60,down@90+2':
                   button@frame[+frames held, default 6], frames from 0,
                   seen from that frame's V-blank;
                   buttons a b select start right left up down r l
  --scale N        window scale to open at (default 3); drag the window's
                   edge to resize it, the frame kept at the largest whole
                   scale that fits, centred on black
  --fullscreen     window: start fullscreen, filling the monitor it's on
                   (at its real size, scaled or not), the frame at the
                   largest whole scale that fits, centred on black, with
                   no border or mouse pointer; F11, Alt+Enter or the
                   menu's Fullscreen row switch it on and off while playing
  --config-dir DIR keep the config files (settings.conf, gamepad.conf,
                   scripts/ and the rest) in DIR itself, over
                   $GBAA_CONFIG_DIR and the default; for launchers
  --pad-config F   window: the gamepad and keyboard mapping file (default
                   gamepad.conf in the config folder, if there is one:
                   --config-dir if given, else $GBAA_CONFIG_DIR if set,
                   else $XDG_CONFIG_HOME/gbaa, else ~/.config/gbaa; on
                   Windows %APPDATA%\\gbaa): lines of
                   `button = controls`, e.g. `a = east west`, each
                   replacing that button's pad default, and of
                   `key button = keys`, e.g. `key a = z space`, its
                   keyboard default (keys by SDL's names, lower case,
                   _ for spaces: left_shift, keypad_enter, `); controls are south
                   east north west (by position), label-a/b/x/y (the face
                   button with that letter on it: Xbox's places on pads
                   without letters), lb rb lt rt select start mode lthumb
                   rthumb dpad-up/down/left/right, lstick-/rstick-
                   up/down/left/right, left-/right-paddle1/2 (grips),
                   misc1-6 and touchpad; then `[title T]` and `[hash H]`
                   sections (the header's title, the ROM's FNV-1a 64 in
                   16 hex digits) for one ROM, laid over those; the menu's
                   Controls screen edits it, pads and keyboard
  --pad-script S   window: press a virtual gamepad's controls, for driving
                   it unattended: words of milliseconds to wait, or
                   controls (as in --pad-config) joined by + to press
                   together, e.g. '3000 start+select dpad-down south'
  --interp         in a recompiled game's binary, run the interpreter instead
                   (in netplay too: every peer must run the same build, or
                   every peer the interpreter)
  --timing T       how the interpreter counts cycles: instant (every fetch
                   as if the prefetch buffer had it, data accesses free:
                   play's), exact (mGBA's model: what checks run) or static;
                   instant by default, or a recompiled build's own
                   (gbaa-recompile --timing). Every netplay peer must match
  --interp-blocks  in a recompiled game's binary, run the interpreter over
                   the compiled blocks, ticking once at each block's exit
                   as compiled code does: its exact state is the compiled
                   build's at every tick (runtime::dispatch::run_blocks);
                   an exact build only, with --timing exact
  --unwind         in a recompiled game's binary, unwind compiled code to
                   the frame loop at each frame end, instead of running it
                   on its own stack (src/coro.rs)
  --wav FILE.wav   headless: write the sound as 16-bit stereo PCM
  --fifo-dump FILE headless: write the sound FIFOs' samples, A and B
                   interleaved, one i8 each (docs/sound-design.md)
  --psg-dump FILE  headless: write the PSG's share of the mix on mGBA's
                   sample grid, left and right i16 LE (docs/sound-design.md)
  --audio-hash     headless: print the FIFO dump's hash (FNV-1a 64)
  --audio-log FILE headless: per frame, the samples and the FIFO DMA
                   restarts (sample indices)
  --mute           window: no sound; a frame clock paces at 59.73 Hz
  --fast-speed N   window: fast-forward at N times normal speed (1.1-16,
                   fractions too, e.g. 1.5), or `unlimited`; default the
                   settings file's (settings.conf in the config folder,
                   beside gamepad.conf: a line `fast-speed = N`), else 2.5
  --volume PCT     window: the sound's volume, 0-200 percent; 100 is the
                   default level (-6 dB on the stream), 200 the stream as
                   it is; default the settings file's (`volume = PCT`),
                   else 100. Headless, it sets --played-wav's level
  --fast-forward   window: start with fast-forward toggled on
  --audio-pace PCT headless: play --frames frames' worth of time to a
                   simulated 48 kHz device whose clock is PCT percent fast
                   (negative: slow), paced as the window is; fails on an
                   underrun after the first second or a queue outside
                   20-100 ms
  --played-wav FILE.wav  with --audio-pace: write what the device played,
                   the desktop's output with its gain, as 16-bit stereo
                   PCM at 48 kHz
  --link FILE      linked play: a second GBA running the same ROM, on one
                   link cable with the first (runtime::link), its save
                   kept in FILE (an mGBA .sav) as --save keeps the
                   first's; the window shows both screens side by side,
                   the first on the left. Player 1 plays on the mapping's
                   keys and the first pad plugged in, player 2 on I J K L
                   (D-pad), N = A, B = B, U = L, O = R, . = Start,
                   , = Select and the second pad. The interpreter runs
                   both (a recompiled build's too); no fast-forward,
                   rewind or save states, and only player 1's sound.
                   With --frames, --input, --input2, --save, --dump,
                   --hash, --dump-at/--dump-dir (both screens), --mute,
                   --scale, --fullscreen, --pad-config, --pad-script
  --input2 PRESSES with --link: player 2's presses, as --input
  --server ADDR    netplay through gbaa-server at ADDR (host:port, its
                   default port 7374), with --host or --join: 2 to 4
                   GBAs on one cable, as --link links two, each player
                   in their own window, maybe on another machine. Every
                   peer runs every GBA and they swap only inputs (and at
                   the start each player's console, save included); the
                   window shows your seat, keeps your save as --save
                   keeps it and discards the others'. Your save goes to
                   the other players for the session. Muted; no fast-forward,
                   rewind, save states or scripts. Your console starts
                   where your game is: the window's, as it was when you
                   asked for the room (and after the session the game
                   alone goes on from where the session left it), or
                   --state's. With --frames, --input, --state,
                   --save, --dump, --hash, --dump-at/--dump-dir (your
                   screen), --scale, --fullscreen, --pad-config,
                   --pad-script
  --host           netplay: make a room (--players seats, 2 by default),
                   print `room CODE` and wait until every seat is taken
                   (headless, 10 minutes); you are player 1, the
                   cable's parent
  --players N      netplay, the host's: seats in the room, 2-4; the
                   session starts once all N are in
  --join CODE      netplay: take the next free seat in the room CODE.
                   Netplay is by rollback: your game never waits for the
                   other player's input; it guesses (their last input),
                   and when the real one differs it goes back and runs
                   those frames again, so they jump instead of the game
                   stopping. Waits only when 5 seconds ahead of the other
                   player
  --delay N        netplay, the host's: input delay in frames (default
                   0; 0-30)
  --paced          netplay, headless: run at the GBA's frame rate, as the
                   window does, not flat out; the end line counts the
                   freezes (waits over a frame) and the frame times
  --menu SPANS     netplay, headless: open the menu over these frames as
                   the window's player would, `frame[+frames]` (6 if
                   left out), frame maybe `from-to/every`, e.g.
                   '600-3000/90+20': your presses let go and the menu's
                   report brought up to date while it's open
  --desync-at N    netplay, for a check: from frame N on, this peer's
                   consoles see your seat holding A, while the other
                   peer gets your real presses: a desync, which the
                   window, the end lines and the kept report then show
  --rogue ADDR     Emerald Rogue's co-op (Multiplayer Adventures) with
                   Rogue Assistant built in: when the game hosts, listen on
                   ADDR's port; when it joins, connect to ADDR (host[:port],
                   port 30025 if left out). ENet as the assistant speaks
                   it, so the other player may be on mGBA with Rogue
                   Assistant. Pick Host or Join in the game's own menu
  --script FILE    run an mGBA Lua script (repeatable; mGBA 0.10.5's API,
                   docs/tools/commands.md); headless prints its console
                   and text buffers on exit, the window its console to
                   stderr as it goes and shows them in its script console
                   (F9; the menu's Scripts page loads more, from
                   scripts/ in the config folder). Sandboxed: files only in its own
                   folder, no programs run (os.execute, io.popen), unless
                   given full access
  --script-full FILE  as --script, with full access as in mGBA, remembered
                   for later runs (a `full = PATH` line in
                   script-access.conf, beside gamepad.conf; delete the
                   line to take it back)
                   A script's canvas layers (mGBA 0.11-dev's `canvas`) are
                   drawn over the game, in the window and in --dump and
                   --dump-at's PNGs. Their `input` (0.11-dev's) is the
                   window's keyboard and mouse while the game has them
  --script-keys PRESSES  hold keyboard keys for the scripts' `input`, as
                   --input's presses: key@frame[+frames], e.g.
                   'shift@60+30,!@60+30'; a key is a character as mGBA's
                   window gives it (a letter's capital, a shifted key's
                   character), u+HEX (a codepoint: u+2c for a comma),
                   space, or a C.KEY name (shift, f1, kp_0)

keys (by default; `key` lines in --pad-config): arrows, X = A, Z = B,
      A = L, S = R, Enter = Start, Backspace = Select, Tab held =
      fast-forward, ` toggles it, R held rewinds;
      F1-F4 load save state 1-4 (NAME.state1 in the game's folder),
      Shift+F1-F4 save it, F5 undoes the last load, F6 exports what
      --extract recorded, F9 shows the script
      console (click its line to type Lua; Escape leaves it), F10 opens
      the menu, as Escape does while no pad is plugged in (arrows,
      Enter or X, Escape or Z closes it), - and = step the
      volume and M mutes (a toast shows the level; a key bound to a
      button is the button's), Ctrl+Q (Cmd+Q on macOS) quits (Escape doesn't: Steam's
      desktop controller layout sends it for a pad's B)
gamepads (any number, plugged in at any time): D-pad or left stick,
      the button labelled A = A, B = B (Xbox's south and east, as on
      the Deck; Nintendo's east and south), and the ones across from
      them the same, Y = A, X = B; shoulders or triggers = L
      and R, Start, Select (Back/View), right stick click held =
      fast-forward, left stick click toggles it (`fast` and `fast-toggle`
      in --pad-config); fast-forward mutes the sound; saving and loading
      a state to the slot last used (1 at first) have no pad control
      unless bound (`save-state`, `load-state`); the right stick
      pushed left rewinds while held (`rewind`); the script console
      (`console`) has no pad control unless bound; Start and Select
      together open the menu (pause, fast-forward's speed, volume, mute,
      resume on launch, quit): D-pad
      or left stick to move, the button labelled A to choose, B or Start
      to close; a stick counts past 40% of its travel, a trigger past
      20% (`deadzone = PCT`, `trigger-deadzone = PCT` in settings.conf,
      or the Controls screen; `deadzone-axes = apart` gives up and down
      their own, `deadzone-horizontal` and `deadzone-vertical`)";

/// What the command line asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// The ROM given, or for a build's embedded ROM, `NAME.gba` beside
    /// the binary ([`embedded_path`]): what errors call it, and where a
    /// headless run's script keeps its state slots.
    pub rom: PathBuf,
    /// The build's own ROM, when none is given ([`Options::parse_for`]).
    pub embedded: Option<Embedded>,
    /// The game's name, the key its saves, states and resume file are
    /// filed under (docs/save-locations-design.md): the embedded build's
    /// own, or the ROM's file name's ([`runtime::name::GameName`]).
    pub game: String,
    pub headless: bool,
    pub frames: Option<u64>,
    pub dump: Option<PathBuf>,
    pub hash: bool,
    pub regs: bool,
    pub input: Script,
    /// `--state`: the save state to start from, mGBA's or ours.
    pub state: Option<PathBuf>,
    /// `--state-out`: where to write our state, and `--state-at`: after
    /// which frame (else the last).
    pub state_out: Option<PathBuf>,
    pub state_at: Option<u64>,
    /// `--frame-hashes`: where to write each frame's hash.
    pub frame_hashes: Option<PathBuf>,
    /// `--frame-times`: where to write each frame's run time.
    pub frame_times: Option<PathBuf>,
    /// `--state-bench`: how many times to save and load the state at the end.
    pub state_bench: Option<u32>,
    /// `--rerun-bench`: how many times to run the last frame again, drawn
    /// and undrawn, at the end.
    pub rerun_bench: Option<u32>,
    /// `--dump-at`: the frames to write as PNGs into `--dump-dir`.
    pub dump_at: Vec<u64>,
    pub dump_dir: Option<PathBuf>,
    /// `--assets-log`: where a `provenance` build writes the load and use
    /// logs (src/assets_log.rs).
    pub assets_log: Option<PathBuf>,
    /// `--assets-mod` and `--assets-manifest`: a mod folder whose edited
    /// assets the game loads instead of the ROM's, and the export's
    /// manifest giving their layouts ([`asset_overrides`]).
    pub assets_mod: Option<PathBuf>,
    pub assets_manifest: Option<PathBuf>,
    /// `--extract`: where a `provenance` build records each play session
    /// and exports what it showed ([`extract`]).
    pub extract: Option<PathBuf>,
    /// `--dump-states`: a state beside each `--dump-at` PNG.
    pub dump_states: bool,
    /// `--native-record`: each frame's record for the native check
    /// ([`native_frame`]).
    pub native_record: Option<PathBuf>,
    /// `--rewind F+N`: hold rewind for N frames after frame F.
    pub rewind: Option<(u64, u64)>,
    /// `--rewind-mb`: rewind's cap, in MB.
    pub rewind_mb: usize,
    /// `--save`: the .sav to keep the save in; the window's default is
    /// `<name>.sav` in the game's folder ([`game_dir`]).
    pub save: Option<PathBuf>,
    /// `--save-dir` and `--state-dir`: the exact folders for the game's
    /// files ([`game_dir::Layers`]).
    pub save_dir: Option<PathBuf>,
    pub state_dir: Option<PathBuf>,
    /// The state slots and resume file are this with their extensions
    /// (`.state1`, `.resume`): `<name>` in the game's folder for the
    /// window, the ROM's path headless.
    pub states: PathBuf,
    /// The window's files to copy into the game's folder on its first
    /// run there: each entry one [`game_dir::move_in`], its places tried
    /// in order (the default folder when --save-dir moved it, then beside
    /// the ROM), each file with where it goes. None headless, or with no
    /// folder.
    pub move_in: Vec<game_dir::Sources>,
    /// [`Options::place`] chose `save`: no `--save` gave it.
    pub save_placed: bool,
    pub no_save: bool,
    /// `--no-resume`: boot this run, not resume from the auto state.
    pub no_resume: bool,
    pub scale: usize,
    /// `--fullscreen`: start fullscreen.
    pub fullscreen: bool,
    /// `--config-dir`: the config folder, over `$GBAA_CONFIG_DIR`
    /// ([`gamepad::set_config_dir`]).
    pub config_dir: Option<PathBuf>,
    /// `--pad-config`: the gamepad mapping file.
    pub pad_config: Option<PathBuf>,
    /// `--pad-script`: a virtual pad's presses.
    pub pad_script: Option<gamepad::Script>,
    pub interp: bool,
    /// `--timing`: how the interpreter counts cycles ([`Options::timing`]).
    pub timing: Option<Timing>,
    /// `--interp-blocks`: the block-timed interpreter.
    pub interp_blocks: bool,
    /// `--unwind`: no guest stack.
    pub unwind: bool,
    pub wav: Option<PathBuf>,
    pub fifo_dump: Option<PathBuf>,
    pub psg_dump: Option<PathBuf>,
    pub audio_hash: bool,
    pub audio_log: Option<PathBuf>,
    pub mute: bool,
    /// `--fast-speed`: fast-forward's multiple of normal speed; 0 is
    /// unthrottled. `None` leaves it to the settings file
    /// ([`fast::window_speed`]).
    pub fast_speed: Option<f64>,
    /// `--fast-forward`: start with it toggled on.
    pub fast_forward: bool,
    /// `--volume`: the master volume, percent. `None` leaves it to the
    /// settings file ([`settings`]), else [`audio::DEFAULT_VOLUME`].
    pub volume: Option<u32>,
    /// `--audio-pace`'s clock error, as a fraction.
    pub audio_pace: Option<f64>,
    /// `--played-wav`: what `--audio-pace`'s device played.
    pub played_wav: Option<PathBuf>,
    /// `--script` and `--script-full`: mGBA Lua scripts to run, in order,
    /// and whether full access was asked for ([`scripting::access`]).
    pub scripts: Vec<(PathBuf, bool)>,
    /// `--script-keys`: keyboard keys held for the scripts' `input`.
    pub script_keys: input::KeyScript,
    /// `--link`: player 2's save, for linked play ([`link`]).
    pub link: Option<PathBuf>,
    /// `--input2`: player 2's presses.
    pub input2: Script,
    /// `--server`: netplay's server ([`room`]).
    pub server: Option<String>,
    /// `--rogue`: Emerald Rogue's co-op address ([`rogue`]).
    pub rogue: Option<String>,
    /// `--host` or `--join`.
    pub role: Option<room::Role>,
    /// `--delay`: netplay's input delay, in frames.
    pub delay: Option<u8>,
    /// `--players`: the seats in the host's room (2-4).
    pub players: Option<usize>,
    /// `--paced`: headless netplay at the GBA's frame rate, not flat out.
    pub paced: bool,
    /// `--menu`: headless netplay, when the menu is open.
    pub menu: input::Spans,
    /// `--desync-at`: netplay, a forced desync from this frame on.
    pub desync_at: Option<u32>,
}

impl Options {
    /// Whether any sound output is asked for.
    pub fn audio(&self) -> bool {
        self.wav.is_some()
            || self.fifo_dump.is_some()
            || self.psg_dump.is_some()
            || self.audio_hash
            || self.audio_log.is_some()
            || self.native_record.is_some()
    }

    /// The interpreter's timing for a run of `code` (the build's, before
    /// `--interp`): `--timing`'s, else the build's, else instant, as play
    /// builds are (`docs/decisions.md` 11).
    pub fn timing(&self, code: Option<&Code>) -> Timing {
        self.timing
            .or(code.map(|c| c.timing))
            .unwrap_or(Timing::Instant)
    }

    /// The master gain the desktop plays at: `--volume`'s (or the
    /// settings file's, once [`main`] has read it), else the default.
    pub fn gain(&self) -> f32 {
        audio::volume_gain(self.volume.unwrap_or(audio::DEFAULT_VOLUME))
    }
}

/// Headless runs need an end; this is it unless `--frames` says otherwise.
pub const HEADLESS_FRAMES: u64 = 60;

/// The most instructions [`settle`] steps.
const SETTLE_STEPS: u32 = 100_000;

/// Before `--regs`: step on to the head of the loop the CPU is in, so a
/// run that ends its last frame mid-block (the interpreter, after any
/// instruction) and one that ends it only at a block start (compiled code,
/// decision 11) print the same registers. Steps the interpreter, taking no
/// IRQ, until a branch goes backwards (at most [`SETTLE_STEPS`]); leaves a
/// halted CPU or a fault as it is. The frame is already drawn.
fn settle(m: &mut DesktopMachine) {
    for _ in 0..SETTLE_STEPS {
        if m.sched.halted || m.fault.is_some() {
            return;
        }
        let pc = m.cpu.pc;
        m.step();
        if m.cpu.pc < pc {
            return;
        }
    }
}

/// A recompiled build's own copy of its ROM (`include_bytes!` in the
/// generated crate) and the game's name, which `gbaa-recompile` baked in
/// (`--name`, or the ROM's file name). Its `Debug` gives the ROM's size,
/// not the bytes.
#[derive(Clone, Copy, PartialEq)]
pub struct Embedded {
    pub rom: &'static [u8],
    pub name: &'static str,
}

impl std::fmt::Debug for Embedded {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Embedded({:?}, {} bytes)", self.name, self.rom.len())
    }
}

impl Options {
    /// The command line of a build without a ROM of its own.
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
        Options::parse_for(args, None, None)
    }

    /// The command line of a build carrying `embedded`, its ROM and name,
    /// run as `exe`: with no ROM given, it runs that, under its own name
    /// ([`embedded_path`]). A ROM given is named from its file name.
    pub fn parse_for(
        args: impl IntoIterator<Item = String>,
        embedded: Option<Embedded>,
        exe: Option<&Path>,
    ) -> Result<Options, String> {
        let mut rom = None;
        let mut o = Options {
            rom: PathBuf::new(),
            embedded: None,
            game: String::new(),
            headless: false,
            frames: None,
            dump: None,
            hash: false,
            regs: false,
            input: Script::default(),
            state: None,
            state_out: None,
            state_at: None,
            frame_hashes: None,
            frame_times: None,
            state_bench: None,
            rerun_bench: None,
            dump_at: Vec::new(),
            dump_dir: None,
            assets_log: None,
            assets_mod: None,
            extract: None,
            assets_manifest: None,
            dump_states: false,
            native_record: None,
            rewind: None,
            rewind_mb: rewind::DEFAULT_MB,
            save: None,
            states: PathBuf::new(),
            save_dir: None,
            state_dir: None,
            move_in: Vec::new(),
            save_placed: false,
            no_save: false,
            no_resume: false,
            scale: 3,
            fullscreen: false,
            config_dir: None,
            pad_config: None,
            pad_script: None,
            interp: false,
            timing: None,
            interp_blocks: false,
            unwind: false,
            wav: None,
            fifo_dump: None,
            psg_dump: None,
            audio_hash: false,
            audio_log: None,
            mute: false,
            fast_speed: None,
            fast_forward: false,
            volume: None,
            audio_pace: None,
            played_wav: None,
            scripts: Vec::new(),
            script_keys: input::KeyScript::default(),
            link: None,
            input2: Script::default(),
            server: None,
            rogue: None,
            role: None,
            delay: None,
            players: None,
            paced: false,
            menu: input::Spans::default(),
            desync_at: None,
        };
        let mut args = args.into_iter();
        while let Some(a) = args.next() {
            let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
            match a.as_str() {
                "--headless" => o.headless = true,
                "--hash" => o.hash = true,
                "--regs" => o.regs = true,
                "--interp" => o.interp = true,
                "--interp-blocks" => o.interp_blocks = true,
                "--timing" => {
                    let v = value("--timing")?;
                    o.timing = Some(
                        Timing::parse(&v)
                            .ok_or(format!("--timing {v}: instant, exact, static or native"))?,
                    );
                }
                "--unwind" => o.unwind = true,
                "--audio-hash" => o.audio_hash = true,
                "--mute" => o.mute = true,
                "--no-save" => o.no_save = true,
                "--no-resume" => o.no_resume = true,
                "--fast-forward" => o.fast_forward = true,
                "--fast-speed" => {
                    o.fast_speed = Some(fast::parse_speed(&value("--fast-speed")?)?);
                }
                "--volume" => o.volume = Some(audio::parse_volume(&value("--volume")?)?),
                "--audio-pace" => {
                    let v = value("--audio-pace")?;
                    o.audio_pace = Some(
                        v.parse::<f64>()
                            .ok()
                            .filter(|p| p.abs() < 50.0)
                            .ok_or(format!("bad clock error {v:?} (percent)"))?
                            / 100.0,
                    );
                }
                "--wav" => o.wav = Some(value("--wav")?.into()),
                "--played-wav" => o.played_wav = Some(value("--played-wav")?.into()),
                "--fifo-dump" => o.fifo_dump = Some(value("--fifo-dump")?.into()),
                "--psg-dump" => o.psg_dump = Some(value("--psg-dump")?.into()),
                "--audio-log" => o.audio_log = Some(value("--audio-log")?.into()),
                "--frames" => {
                    let v = value("--frames")?;
                    o.frames = Some(
                        v.parse()
                            .ok()
                            .filter(|&n| n > 0)
                            .ok_or(format!("bad frame count {v:?}"))?,
                    );
                }
                "--scale" => {
                    let v = value("--scale")?;
                    o.scale = v
                        .parse()
                        .ok()
                        .filter(|s| (1..=16).contains(s))
                        .ok_or(format!("bad scale {v:?} (1-16)"))?;
                }
                "--fullscreen" => o.fullscreen = true,
                "--input" => o.input = Script::parse(&value("--input")?)?,
                "--input2" => o.input2 = Script::parse(&value("--input2")?)?,
                "--link" => o.link = Some(value("--link")?.into()),
                "--server" => o.server = Some(value("--server")?),
                "--rogue" => o.rogue = Some(value("--rogue")?),
                "--host" => o.role = Some(room::Role::Host),
                "--join" => o.role = Some(room::Role::Join(room::parse_code(&value("--join")?)?)),
                // Rollback is the only netplay; the old flag still parses.
                "--rollback" => {}
                "--paced" => o.paced = true,
                "--menu" => o.menu = input::Spans::parse(&value("--menu")?)?,
                "--desync-at" => {
                    let v = value("--desync-at")?;
                    o.desync_at = Some(v.parse().map_err(|_| format!("bad frame {v:?}"))?);
                }
                "--players" => {
                    let v = value("--players")?;
                    o.players = Some(
                        v.parse()
                            .ok()
                            .filter(|n| (2..=room::MAX_SEATS).contains(n))
                            .ok_or(format!("bad --players {v:?} (2-{})", room::MAX_SEATS))?,
                    );
                }
                "--delay" => {
                    let v = value("--delay")?;
                    o.delay = Some(
                        v.parse()
                            .ok()
                            .filter(|d| (0..=30).contains(d))
                            .ok_or(format!("bad delay {v:?} (0-30 frames)"))?,
                    );
                }
                "--dump" => o.dump = Some(value("--dump")?.into()),
                "--state" => o.state = Some(value("--state")?.into()),
                "--state-out" => o.state_out = Some(value("--state-out")?.into()),
                "--frame-hashes" => o.frame_hashes = Some(value("--frame-hashes")?.into()),
                "--frame-times" => o.frame_times = Some(value("--frame-times")?.into()),
                "--state-bench" => {
                    let v = value("--state-bench")?;
                    o.state_bench = Some(
                        v.parse()
                            .ok()
                            .filter(|&n| n > 0)
                            .ok_or(format!("bad count {v:?}"))?,
                    );
                }
                "--rerun-bench" => {
                    let v = value("--rerun-bench")?;
                    o.rerun_bench = Some(
                        v.parse()
                            .ok()
                            .filter(|&n| n > 0)
                            .ok_or(format!("bad count {v:?}"))?,
                    );
                }
                "--dump-at" => o.dump_at = frame_list(&value("--dump-at")?)?,
                "--dump-dir" => o.dump_dir = Some(value("--dump-dir")?.into()),
                "--assets-log" => o.assets_log = Some(value("--assets-log")?.into()),
                "--assets-mod" => o.assets_mod = Some(value("--assets-mod")?.into()),
                "--extract" => o.extract = Some(value("--extract")?.into()),
                "--assets-manifest" => o.assets_manifest = Some(value("--assets-manifest")?.into()),
                "--dump-states" => o.dump_states = true,
                "--native-record" => o.native_record = Some(value("--native-record")?.into()),
                "--state-at" => {
                    let v = value("--state-at")?;
                    o.state_at = Some(
                        v.parse()
                            .ok()
                            .filter(|&n| n > 0)
                            .ok_or(format!("bad frame {v:?}"))?,
                    );
                }
                "--rewind" => {
                    let v = value("--rewind")?;
                    o.rewind = Some(
                        v.split_once('+')
                            .and_then(|(f, n)| Some((f.parse().ok()?, n.parse().ok()?)))
                            .filter(|&(_, n)| n > 0)
                            .ok_or(format!("bad rewind {v:?} (FRAME+FRAMES)"))?,
                    );
                }
                "--rewind-mb" => {
                    let v = value("--rewind-mb")?;
                    o.rewind_mb = v.parse().map_err(|_| format!("bad size {v:?} (MB)"))?;
                }
                "--save" => o.save = Some(value("--save")?.into()),
                "--save-dir" => o.save_dir = Some(value("--save-dir")?.into()),
                "--state-dir" => o.state_dir = Some(value("--state-dir")?.into()),
                "--script" => o.scripts.push((value("--script")?.into(), false)),
                "--script-full" => o.scripts.push((value("--script-full")?.into(), true)),
                "--script-keys" => {
                    o.script_keys = input::KeyScript::parse(&value("--script-keys")?)?
                }
                "--config-dir" => o.config_dir = Some(value("--config-dir")?.into()),
                "--pad-config" => o.pad_config = Some(value("--pad-config")?.into()),
                "--pad-script" => {
                    o.pad_script = Some(gamepad::Script::parse(&value("--pad-script")?)?)
                }
                "-h" | "--help" => return Err(String::new()),
                s if s.starts_with('-') => return Err(format!("unknown option {s}")),
                _ if rom.is_some() => return Err(format!("unexpected argument {a:?}")),
                _ => rom = Some(PathBuf::from(a)),
            }
        }
        o.rom = match (rom, embedded) {
            (Some(rom), _) => {
                let stem = rom.file_stem().map(|s| s.to_string_lossy());
                o.game = runtime::name::GameName(stem.as_deref().unwrap_or("")).to_string();
                rom
            }
            (None, Some(e)) => {
                o.embedded = Some(e);
                o.game = e.name.to_string();
                embedded_path(
                    exe.ok_or("no ROM given, and no path to the binary")?,
                    e.name,
                )
            }
            (None, None) => return Err("no ROM given".into()),
        };
        if o.link.is_none() && o.input2 != Script::default() {
            return Err("--input2 is player 2's, with --link".into());
        }
        if o.server.is_some() || o.role.is_some() {
            room::check(&o)?;
        } else if o.delay.is_some()
            || o.players.is_some()
            || o.paced
            || o.menu != input::Spans::default()
            || o.desync_at.is_some()
        {
            return Err(
                "--delay, --players, --paced, --menu and --desync-at are netplay's, with --server"
                    .into(),
            );
        } else if o.link.is_some() {
            link::check(&o)?;
        }
        if o.fullscreen && o.headless {
            return Err("--fullscreen is for the window".into());
        }
        if (o.audio() || o.audio_pace.is_some()) && !o.headless {
            return Err(
                "--wav, --fifo-dump, --psg-dump, --audio-hash, --audio-log and --audio-pace need \
                 --headless"
                    .into(),
            );
        }
        if o.played_wav.is_some() && o.audio_pace.is_none() {
            return Err("--played-wav needs --audio-pace".into());
        }
        if !o.scripts.is_empty() && o.audio_pace.is_some() {
            return Err("--script and --audio-pace don't go together".into());
        }
        if o.mute && o.headless {
            return Err("--mute is for the window: headless never plays sound".into());
        }
        if o.volume.is_some() && o.headless && o.audio_pace.is_none() {
            return Err("--volume is for the window, or headless with --audio-pace".into());
        }
        if (o.fast_forward || o.fast_speed.is_some()) && o.headless {
            return Err(
                "--fast-forward and --fast-speed are for the window: headless runs \
                        unthrottled already"
                    .into(),
            );
        }
        if (o.pad_config.is_some() || o.pad_script.is_some()) && o.headless {
            return Err(
                "--pad-config and --pad-script are for the window: headless reads no gamepads"
                    .into(),
            );
        }
        if o.no_save && (o.headless || o.save.is_some()) {
            return Err("--no-save is for the window, without --save".into());
        }
        if o.no_resume && o.headless {
            return Err("--no-resume is for the window".into());
        }
        if (o.save_dir.is_some() || o.state_dir.is_some()) && o.headless {
            return Err("--save-dir and --state-dir are for the window".into());
        }
        o.place(&Placing::default(), game_dir::data_dir().as_deref());
        if o.headless && o.frames.is_none() {
            o.frames = Some(HEADLESS_FRAMES);
        }
        if o.rewind.is_some() && (!o.headless || o.rewind_mb == 0) {
            return Err("--rewind is for headless runs, with rewind on".into());
        }
        if o.dump_at.is_empty() != o.dump_dir.is_none() || !o.dump_at.is_empty() && !o.headless {
            return Err("--dump-at and --dump-dir go together, headless".into());
        }
        if (o.frame_times.is_some() || o.state_bench.is_some() || o.rerun_bench.is_some())
            && !o.headless
        {
            return Err(
                "--frame-times, --state-bench and --rerun-bench are for headless runs".into(),
            );
        }
        if o.assets_log.is_some() && !(o.headless && cfg!(feature = "provenance")) {
            return Err("--assets-log is for headless runs of a provenance build".into());
        }
        if o.extract.is_some() && !cfg!(feature = "provenance") {
            return Err(
                "--extract needs a provenance build (`just play-extract`, or cargo's --features provenance)"
                    .into(),
            );
        }
        if o.extract.is_some()
            && (o.assets_log.is_some()
                || o.assets_mod.is_some()
                || o.link.is_some()
                || o.server.is_some())
        {
            return Err(
                "--extract is for one machine's plain ROM: not with --assets-log, --assets-mod, --link or --server"
                    .into(),
            );
        }
        if o.assets_mod.is_some() != o.assets_manifest.is_some() {
            return Err("--assets-mod and --assets-manifest go together".into());
        }
        if o.assets_mod.is_some() && (o.link.is_some() || o.server.is_some()) {
            return Err("--assets-mod is for one machine, not --link or --server".into());
        }
        if o.dump_states && o.dump_at.is_empty() {
            return Err("--dump-states needs --dump-at".into());
        }
        if let Some(&n) = o.dump_at.iter().max()
            && o.frames.is_none_or(|f| n > f)
        {
            return Err(format!("--dump-at {n} is past the last frame"));
        }
        if let Some(n) = o.state_at {
            if o.state_out.is_none() {
                return Err("--state-at needs --state-out".into());
            }
            if o.frames.is_none_or(|f| n > f) {
                return Err(format!("--state-at {n} is past the last frame"));
            }
        }
        Ok(o)
    }
}

/// What [`Options::place`] goes by besides the flags: the settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Placing {
    /// The game's own setting and where the menu moved it from
    /// ([`game_dir::GameConf`]).
    pub game: game_dir::GameConf,
    /// settings.conf's `save-dir` and `previous-save-dir`: parents.
    pub global: Option<PathBuf>,
    pub previous_global: Option<PathBuf>,
}

impl Options {
    /// Put the window's files in the game's folder ([`game_dir::resolve`],
    /// the flags over `p`'s settings over the default in `data`); beside
    /// the ROM only when there is no data dir to put it in (no HOME, say).
    /// [`Options::parse_for`] places them by the flags alone; the window
    /// places them again once it has read the settings.
    pub fn place(&mut self, p: &Placing, data: Option<&Path>) {
        if self.save_placed {
            self.save = None;
            self.save_placed = false;
        }
        self.move_in = Vec::new();
        let layers = game_dir::Layers {
            save_flag: self.save_dir.clone(),
            state_flag: self.state_dir.clone(),
            per_game: p.game.save_dir.clone(),
            global: p.global.clone(),
        };
        let folders = match self.headless {
            true => None,
            false => game_dir::resolve(&layers, &self.game, data),
        };
        self.states = match &folders {
            Some(f) => f.states_stem(&self.game),
            None => self.rom.clone(),
        };
        if !self.headless && !self.no_save && self.save.is_none() {
            self.save = Some(match &folders {
                Some(f) => f.sav(&self.game),
                None => sav_file::beside(&self.rom),
            });
            self.save_placed = true;
        }
        // Saves from elsewhere move in on the folder's first run that keeps
        // its save there: never with --save or --no-save, which would copy
        // the rest and leave the .sav behind. First from the folders the
        // game used before, so a new launcher's flags or a setting changed
        // in the menu don't strand the saves played under the old one: the
        // folder the menu moved it from, the last global setting's, the
        // default. Else from beside the ROM, where they were before there
        // were folders. When only --state-dir moved the states, the default
        // folder's states follow on their own first run there.
        let Some(f) = &folders else { return };
        let sav = f.sav(&self.game);
        if self.save.as_deref() != Some(sav.as_path()) {
            return;
        }
        let beside = game_dir::move_in_pairs(&self.rom, &sav, &self.states);
        let default = game_dir::resolve(&Default::default(), &self.game, data);
        // A folder before had the states with the saves, unless a flag
        // keeps them apart.
        let before = |saves: PathBuf| game_dir::Folders {
            states: self.state_dir.clone().unwrap_or_else(|| saves.clone()),
            saves,
        };
        let mut earlier: Vec<game_dir::Folders> = Vec::new();
        let candidates = [
            p.game.moved_from.clone().map(before),
            p.previous_global
                .as_ref()
                .map(|g| before(g.join(&self.game))),
            default.clone(),
        ];
        for e in candidates.into_iter().flatten() {
            if e.saves != f.saves && !earlier.iter().any(|x| x.saves == e.saves) {
                earlier.push(e);
            }
        }
        let mut first: game_dir::Sources = earlier
            .iter()
            .map(|e| game_dir::folder_pairs(e, f, &self.game))
            .collect();
        first.push(beside);
        let only_beside = first.len() == 1;
        self.move_in = vec![first];
        if let Some(d) = default
            && only_beside
            && d.states != f.states
        {
            self.move_in
                .push(vec![game_dir::folder_pairs(&d, f, &self.game)]);
        }
    }

    /// The ROM's bytes: the file given, or the build's own copy.
    pub fn rom_bytes(&self) -> Result<std::borrow::Cow<'static, [u8]>, String> {
        let rom = match self.embedded {
            Some(Embedded { rom, .. }) => rom.into(),
            None => std::fs::read(&self.rom)
                .map_err(|e| format!("{}: {e}", self.rom.display()))?
                .into(),
        };
        Ok(rom)
    }

    /// [`rom_bytes`](Options::rom_bytes), checked for size, for the run.
    pub fn load_rom(&self) -> Result<&'static [u8], String> {
        let rom = self.rom_bytes()?;
        if rom.len() > runtime::bus::ROM_MAX {
            return Err(format!("{}: larger than 32 MB", self.rom.display()));
        }
        Ok(match rom {
            std::borrow::Cow::Borrowed(b) => b,
            std::borrow::Cow::Owned(v) => Box::leak(v.into_boxed_slice()),
        })
    }
}

/// Where a build's embedded ROM, named `name`, is taken to be when run
/// with none given (double-clicked, say): `name.gba` beside the binary
/// `exe`. The file needn't exist; it names the game in errors, and a
/// headless script's state slots go beside it. The window keeps the save
/// in the game's folder ([`game_dir`]).
pub fn embedded_path(exe: &Path, name: &str) -> PathBuf {
    exe.with_file_name(format!("{name}.gba"))
}

/// `N,N,…`: frame numbers from 1, for `--dump-at`.
pub fn frame_list(s: &str) -> Result<Vec<u64>, String> {
    s.split(',')
        .map(|n| {
            n.trim()
                .parse()
                .ok()
                .filter(|&n| n > 0)
                .ok_or(format!("bad frame {n:?} in {s:?}"))
        })
        .collect()
}

/// The PNG `--dump-at` writes for frame `n`.
pub fn dump_at_path(dir: &std::path::Path, n: u64) -> PathBuf {
    dir.join(format!("frame-{n}.png"))
}

/// `--extract`'s export: the graphics of every session and route recorded
/// under `dir` for `rom`, named as the export already there named them
/// (docs/asset-extraction-design.md, Extract while playing).
#[cfg(feature = "provenance")]
fn extract(rom: &[u8], dir: &Path) -> Result<String, String> {
    let (named, sizes) = assets::export::manifest_names(dir);
    assets::export::graphics(rom, dir, &named, &sizes).map(|e| e.summary)
}

/// Parse the command line and run. Generated crates call this with their
/// code, their ROM (`include_bytes!`) and its name; `gbaa-desktop` with
/// `None`s.
pub fn main(code: Option<&'static Code>, rom: Option<Embedded>) -> ExitCode {
    let exe = std::env::current_exe().ok();
    let mut opts = match Options::parse_for(std::env::args().skip(1), rom, exe.as_deref()) {
        Ok(o) => o,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("gbaa-desktop: {e}");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    if let Some(dir) = &opts.config_dir {
        gamepad::set_config_dir(dir.clone());
    }
    // The game's folders ([`game_dir`]), for the window's files, placed
    // again now the settings are read. (A settings file that won't parse
    // is reported as the window starts.)
    let mut placing = Placing::default();
    if !opts.headless {
        let s = settings::load(settings::path().as_deref()).unwrap_or_default();
        let conf = gamepad::config_dir().map(|c| game_dir::GameConf::path(&c, &opts.game));
        let game = game_dir::GameConf::load(conf.as_deref()).unwrap_or_else(|e| {
            eprintln!("gbaa-desktop: {e}");
            Default::default()
        });
        placing = Placing {
            game,
            global: s.save_dir,
            previous_global: s.previous_save_dir,
        };
        opts.place(&placing, game_dir::data_dir().as_deref());
        let dirs = [opts.save.as_deref(), Some(opts.states.as_path())];
        for dir in dirs.into_iter().flatten().filter_map(Path::parent) {
            if let Err(e) = std::fs::create_dir_all(dir) {
                eprintln!("gbaa-desktop: {}: {e}", dir.display());
                return ExitCode::FAILURE;
            }
        }
        let moved: Result<Vec<_>, _> = opts.move_in.iter().map(|m| game_dir::move_in(m)).collect();
        match moved.map(|m| m.concat()) {
            Ok(copied) => {
                for p in copied {
                    eprintln!("gbaa-desktop: copied in: {}", p.display());
                }
            }
            Err(e) => {
                eprintln!("gbaa-desktop: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    let result = if opts.headless {
        game_thread(move || run(&opts, code, None, &mut None))
            .join()
            .unwrap_or_else(|_| Err("the run thread panicked".into()))
    } else {
        let mut server = None;
        let mut directions_apart = false;
        let mut deadzones = (None, None);
        let mut deadzone_axes_apart = false;
        let mut axis_deadzones = [None, None];
        let mut resume = false;
        settings::load(settings::path().as_deref())
            .and_then(|s| {
                opts.fast_speed = opts.fast_speed.or(s.fast_speed);
                opts.volume = opts.volume.or(s.volume);
                server = s.server.clone();
                directions_apart = s.directions_apart.unwrap_or(false);
                deadzones = (s.deadzone, s.trigger_deadzone);
                deadzone_axes_apart = s.deadzone_axes_apart.unwrap_or(false);
                axis_deadzones = [s.deadzone_horizontal, s.deadzone_vertical];
                resume = s.resume_on();
                window_controls(&opts)
            })
            .and_then(|controls| {
                let o = window::Options {
                    scale: opts.scale,
                    fullscreen: opts.fullscreen,
                    controls,
                    fast_speed: opts.fast_speed.unwrap_or(fast::DEFAULT_SPEED),
                    volume: opts.volume.unwrap_or(audio::DEFAULT_VOLUME),
                    pad_script: opts.pad_script.clone(),
                    screens: if opts.link.is_some() { 2 } else { 1 },
                    server: opts
                        .server
                        .clone()
                        .or(server)
                        .unwrap_or_else(|| room::DEFAULT_SERVER.into()),
                    directions_apart,
                    deadzones: [
                        deadzones.0.unwrap_or(gamepad::DEFAULT_DEADZONE),
                        deadzones.1.unwrap_or(gamepad::DEFAULT_TRIGGER_DEADZONE),
                    ],
                    deadzone_axes_apart,
                    axis_deadzones,
                    resume,
                    saves: menu::saves::save_folders(
                        &opts.game,
                        opts.save.as_deref(),
                        &placing.game,
                        placing.global.clone(),
                        // --save names the .sav itself.
                        opts.save_dir.is_some() || (opts.save.is_some() && !opts.save_placed),
                    ),
                };
                window::run(o, move |screen| run_window(&opts, code, screen))
            })
            .and_then(|r| r)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("gbaa-desktop: {e}");
            ExitCode::FAILURE
        }
    }
}

/// On closing the window: copy the save at `sav` to its backups, and prune
/// them ([`sav_file::backup`]).
fn back_up_save(sav: &Path) {
    match sav_file::backup(sav, &sav_file::stamp_now()) {
        Ok(Some(p)) => eprintln!("gbaa-desktop: save backed up to {}", p.display()),
        Ok(None) => {}
        Err(e) => eprintln!("gbaa-desktop: save not backed up: {e}"),
    }
    let dir = sav_file::backup_dir(sav);
    match sav_file::prune(&dir, sav_file::BACKUP_CAP) {
        Ok(gone) if !gone.is_empty() => {
            eprintln!("gbaa-desktop: {} old save backups deleted", gone.len())
        }
        Ok(_) => {}
        Err(e) => eprintln!("gbaa-desktop: old save backups not pruned: {e}"),
    }
}

/// Run `f` on the game's own thread. Guest calls become host calls in
/// recompiled code, so it gets a big stack (docs/codegen-design.md).
pub fn game_thread<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> std::thread::JoinHandle<T> {
    std::thread::Builder::new()
        .name("gbaa".into())
        .stack_size(256 << 20)
        .spawn(f)
        .expect("spawning the run thread")
}

/// The window's gamepad mapping file, for the ROM: `--pad-config`'s, else
/// the config file's if there is one (or will be, once the menu saves
/// one), else the default.
fn window_controls(opts: &Options) -> Result<remap::Controls, String> {
    let rom = opts.rom_bytes()?;
    let rom = remap::RomId::of(&rom);
    match &opts.pad_config {
        Some(path) => remap::Controls::load(path, true, rom),
        None => match gamepad::config_path() {
            Some(p) => remap::Controls::load(&p, false, rom),
            None => Ok(remap::Controls {
                rom,
                ..remap::Controls::default()
            }),
        },
    }
}

/// Run `f` `n` times: the mean and the worst time.
fn time(n: u32, mut f: impl FnMut()) -> (Duration, Duration) {
    let (mut total, mut worst) = (Duration::ZERO, Duration::ZERO);
    for _ in 0..n {
        let t = Instant::now();
        f();
        let d = t.elapsed();
        total += d;
        worst = worst.max(d);
    }
    (total / n, worst)
}

fn us((mean, worst): (Duration, Duration)) -> String {
    format!(
        "{:.1} us (worst {:.1})",
        mean.as_secs_f64() * 1e6,
        worst.as_secs_f64() * 1e6
    )
}

/// `--rerun-bench`: what a frame rollback runs again costs. From the
/// machine's state at the end, `n` times each: load the picture-less
/// snapshot rollback keeps ([`Machine::save_unseen`]) and run one frame
/// drawn, then the same frame undrawn ([`runtime::Ppu::draw`]), as
/// rollback runs it now; and the snapshot's save and load alone. Fails if
/// the undrawn frame computes anything the drawn one didn't. The machine
/// ends as it began.
fn rerun_bench(
    m: &mut DesktopMachine,
    n: u32,
    mut frame: impl FnMut(&mut DesktopMachine),
) -> Result<String, String> {
    let mut whole = Vec::new();
    m.save_payload(&mut |s| whole.extend_from_slice(s));
    let mut snap = Vec::new();
    let save = time(n, || {
        snap.clear();
        m.save_unseen(&mut |s| snap.extend_from_slice(s));
    });
    let mut failed = None;
    let load = time(n, || failed = failed.take().or(m.load_unseen(&snap).err()));
    let mut run = |m: &mut DesktopMachine, draw: bool| {
        m.ppu.draw = draw;
        let (mut total, mut worst) = (Duration::ZERO, Duration::ZERO);
        for _ in 0..n {
            failed = failed.take().or(m.load_unseen(&snap).err());
            let t = Instant::now();
            frame(m);
            let d = t.elapsed();
            total += d;
            worst = worst.max(d);
        }
        let mut after = Vec::new();
        m.save_unseen(&mut |s| after.extend_from_slice(s));
        ((total / n, worst), after)
    };
    let (drawn, want) = run(m, true);
    let (undrawn, got) = run(m, false);
    m.ppu.draw = true;
    failed = failed.or(m.load_payload(&whole).err());
    if let Some(e) = failed {
        return Err(format!(
            "--rerun-bench: our own snapshot didn't load: {e:?}"
        ));
    }
    if got != want {
        return Err("--rerun-bench: an undrawn frame computed something else".into());
    }
    Ok(format!(
        "rerun-bench x{n}: snapshot {} bytes, save {}, load {}; frame drawn {}, undrawn {}",
        snap.len(),
        us(save),
        us(load),
        us(drawn),
        us(undrawn)
    ))
}

/// `--state-bench`: save and load the machine's state `n` times each, whole
/// (with its header and CRC-32s, as a save state file holds it) and as
/// rewind's bare payload, and say how big each is and how long each took,
/// on average and at worst. The machine ends as it began.
fn state_bench(m: &mut DesktopMachine, n: u32) -> Result<String, String> {
    let mut whole = Vec::new();
    let save = time(n, || {
        whole.clear();
        m.save_state(&mut |s| whole.extend_from_slice(s));
    });
    let mut failed = None;
    let load = time(n, || failed = failed.take().or(m.load_state(&whole).err()));
    let mut payload = Vec::new();
    let save_payload = time(n, || {
        payload.clear();
        m.save_payload(&mut |s| payload.extend_from_slice(s));
    });
    let load_payload = time(n, || {
        failed = failed.take().or(m.load_payload(&payload).err())
    });
    if let Some(e) = failed {
        return Err(format!("--state-bench: our own state didn't load: {e:?}"));
    }
    Ok(format!(
        "state-bench x{n}: whole {} bytes, save {}, load {}; payload {} bytes, save {}, load {}",
        whole.len(),
        us(save),
        us(load),
        payload.len(),
        us(save_payload),
        us(load_payload)
    ))
}

/// What the bus takes for `--assets-mod DIR` (docs/asset-extraction-design.md,
/// "Overrides"): the ROM, patched with the raw blobs' overrides (same
/// length) when there are any, with the original's CRC to keep; and the
/// compressed blobs' unpacked overrides, sorted, for the BIOS calls. A
/// recompiled build refuses a patch over bytes its code folded in
/// ([`Code::folded`]): the code would keep the original's.
#[allow(clippy::type_complexity)]
fn asset_overrides(
    rom: &'static [u8],
    dir: &Path,
    manifest: &Path,
    code: Option<&'static Code>,
) -> Result<(&'static [u8], &'static [(u32, &'static [u8])], Option<u32>), String> {
    let text =
        std::fs::read_to_string(manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let o = assets::overrides::load(dir, &text, rom)?;
    if let Some(c) = code {
        for (a, b) in &o.raw {
            if let Some((s, e)) = c.folded_in(*a, *a + b.len() as u32) {
                return Err(format!(
                    "--assets-mod: the override at {a:#010x} covers ROM {s:#010x}..{e:#010x}, \
                     which this recompiled build's code read at build time; run it with --interp"
                ));
            }
        }
    }
    eprintln!(
        "gbaa-desktop: {} asset overrides from {} ({} compressed, {} raw)",
        o.unpacked.len() + o.raw.len(),
        dir.display(),
        o.unpacked.len(),
        o.raw.len()
    );
    let table: Vec<(u32, &'static [u8])> = o
        .unpacked
        .iter()
        .map(|(a, b)| (*a, &*Box::leak(b.clone().into_boxed_slice())))
        .collect();
    let table = Box::leak(table.into_boxed_slice());
    match o.patched(rom) {
        Some(p) => {
            let crc = runtime::state::crc32(rom);
            Ok((Box::leak(p.into_boxed_slice()), table, Some(crc)))
        }
        None => Ok((rom, table, None)),
    }
}

/// Run in the window `screen` as `opts` says; then, while the window is
/// open, each netplay session its menu asks for, and the game alone again
/// after it. `--frames` bounds each run, and the window closes at a
/// session's end. Each run starts where the one before it ended, as a GBA
/// does when its cable goes in or out (multiplayer/join-without-restart):
/// a session's seat from the game alone's state, and the game alone after
/// it from that seat's.
fn run_window(
    opts: &Options,
    code: Option<&'static Code>,
    screen: window::Screen,
) -> Result<(), String> {
    let mut o = opts.clone();
    let mut carry = None;
    loop {
        let netplay = o.server.is_some();
        let r = run(&o, code, Some(screen.clone()), &mut carry);
        let ended = r.is_ok();
        if netplay {
            screen.end_session(match &r {
                Ok(()) => room::State::Ended("You left the room.".into()),
                // The server turning the game away (too old a build, no
                // such room) is said as it is: no session began.
                Err(e) => match e.strip_prefix(netplay::udp::REFUSED) {
                    Some(why) => room::State::Ended(format!("The server said no: {why}")),
                    None => room::State::Ended(format!("The session ended: {e}")),
                },
            });
            if let Err(e) = &r {
                eprintln!("gbaa-desktop: {e}");
            }
            // The game alone goes on: the room's title goes.
            screen.set_title("gbaa");
        } else {
            r?;
        }
        if !screen.is_open() || netplay && ended && opts.frames.is_some() {
            return Ok(());
        }
        // The game alone stops for a session or a closed window.
        o = Options {
            server: None,
            role: None,
            players: None,
            ..opts.clone()
        };
        let Some(ask) = screen.take_netplay_ask() else {
            if netplay {
                continue;
            }
            return Ok(());
        };
        let asked = Options {
            server: Some(ask.server),
            players: (ask.role == room::Role::Host).then_some(ask.players),
            role: Some(ask.role),
            ..o.clone()
        };
        match room::check(&asked) {
            Ok(()) => o = asked,
            Err(e) => screen.end_session(room::State::Ended(format!("No netplay: {e}"))),
        }
    }
}

/// The spin loops `crates/runtime/spins.toml` lists for `rom`
/// ([`runtime::spins`]), sorted, for a machine with no build to carry
/// them ([`runtime::Machine::spin_list`]).
pub fn spin_list(rom: &[u8]) -> &'static [u32] {
    let mut v = Vec::new();
    runtime::spins::listed(&runtime::sha1::sha1(rom), |e| v.push(e));
    v.sort_unstable();
    v.dedup();
    Box::leak(v.into_boxed_slice())
}

/// Load the ROM, run it as `opts` says (in the window `screen`, unless
/// headless), and write what it asks for at the end. `carry`: the
/// machine's state to start from, if any (taken: the window's last run's,
/// [`run_window`]); a window's run leaves the state it ended in there,
/// for the next.
pub fn run(
    opts: &Options,
    code: Option<&'static Code>,
    screen: Option<window::Screen>,
    carry: &mut Option<Vec<u8>>,
) -> Result<(), String> {
    let timing = opts.timing(code);
    // Native timing's generated code ticks nothing and its interpreter
    // nothing per instruction: neither mixes with the other timings.
    if let Some(c) = code.filter(|_| !opts.interp)
        && (c.timing == Timing::Native) != (timing == Timing::Native)
    {
        return Err(format!(
            "--timing {} with a build made at --timing {}: native timing needs both, or --interp",
            timing.name(),
            c.timing.name()
        ));
    }
    if opts.link.is_some() {
        return link::run(opts, timing, screen);
    }
    if opts.server.is_some() {
        return room::run(opts, code.filter(|_| !opts.interp), timing, screen, carry);
    }
    #[cfg(feature = "timing-lab")]
    timing_lab::install()?;
    let rom = opts.load_rom()?;
    let platform = if opts.headless {
        let d = Desktop::headless();
        let d = match opts.audio_pace {
            Some(_) => {
                let mut p = audio::Player::simulated(PACE_RATE);
                p.set_gain(opts.gain());
                d.with_player(p)
            }
            None => d,
        };
        match (opts.audio(), opts.wav.is_some()) {
            (false, _) => d,
            (true, false) => d.with_capture(Capture::default()),
            (true, true) => d.with_capture(Capture::default().with_samples()),
        }
    } else {
        let screen = screen.ok_or("a window run needs its window")?;
        Desktop::window(screen, opts.mute, opts.gain()).with_fast(fast::FastForward::new(
            opts.fast_speed.unwrap_or(fast::DEFAULT_SPEED),
            opts.fast_forward,
        ))
    }
    .with_input(opts.input.clone());
    // The game's listed spin loops, for the interpreter too (`--interp`,
    // or no build at all).
    let spin_list = match code {
        Some(c) => c.spins,
        None if timing == Timing::Native => spin_list(rom),
        None => &[],
    };
    let code = code.filter(|_| !opts.interp);
    // The block-timed interpreter prices blocks as exact code does.
    if opts.interp_blocks
        && let Some(c) = code
        && (c.timing != Timing::Exact || timing != Timing::Exact)
    {
        return Err("--interp-blocks needs an exact build and --timing exact".into());
    }
    if let Some(c) = code
        && runtime::dispatch::rom_hash(rom) != c.rom_hash
    {
        return Err(format!(
            "{}: not the ROM this build was recompiled from",
            opts.rom.display()
        ));
    }
    let (rom, overrides, crc) = match (&opts.assets_mod, &opts.assets_manifest) {
        (Some(dir), Some(manifest)) => asset_overrides(rom, dir, manifest, code)?,
        _ => (rom, &[][..], None),
    };
    let mut m = Box::new(Machine::new(rom, platform));
    m.set_timing(timing);
    m.spin_list = spin_list;
    m.bus.asset_overrides = overrides;
    if let Some(crc) = crc {
        m.bus.keep_rom_crc(crc);
    }
    let mut sav = match &opts.save {
        Some(path) => Some(sav_file::SavFile::open(path, &mut m.bus.save)?),
        None => None,
    };
    if let Some(path) = &opts.state {
        slots::load(&mut m, path)?;
    }
    // Where the window's last run (a netplay session's seat) ended. Its
    // cartridge save is the one just flushed to the .sav, read above.
    let carried = carry.take();
    if let Some(s) = &carried {
        m.load_state(s)
            .map_err(|e| format!("the game's state as the session left it: {e}"))?;
    }
    // A state from another ROM just loaded, put back if it faults
    // ([`slots::watch`]).
    let mut trial = None;
    // Auto states ([`resume`]): a window's play, not a scripted run's.
    let mut auto = (!opts.headless
        && opts.save.is_some()
        && opts.frames.is_none()
        && opts.pad_script.is_none()
        && opts.input == Script::default())
    .then(|| m.platform.resume_on())
    .map(|on| resume::Auto::new(&opts.states, on));
    if auto.as_ref().is_some_and(|_| m.platform.resume_on())
        && opts.state.is_none()
        && carried.is_none()
        && !opts.no_resume
        && let Some(said) = resume::load(&mut m, &opts.states, opts.save.as_deref(), &mut trial)
    {
        eprintln!("gbaa-desktop: {said}");
        m.platform.set_title(&format!("gbaa: {said}"));
    }
    let mut rogue = match &opts.rogue {
        Some(addr) => Some(rogue::Assistant::new(rom, addr)?),
        None => None,
    };
    // Made at the first script, from the command line or the window's
    // console ([`console`]).
    let mut host = match opts.scripts.is_empty() {
        true => None,
        false => Some(scripting::host(
            rom,
            &opts.states,
            code,
            opts.interp_blocks,
        )?),
    };
    // As mGBA's scripting window loads a script into a running game: `emu`
    // is set and `start` doesn't fire (it fires only for scripts loaded
    // before the game, whose `emu` is nil while they load). Scripts written
    // for both, like mGBA's pokemon.lua, so set up once.
    if let Some(h) = &host {
        let conf = scripting::access_path();
        for (s, full) in &opts.scripts {
            h.load(&mut m, s, scripting::access(s, *full, conf.as_deref())?);
        }
        if !opts.headless {
            let lines = h.take_console();
            for l in &lines {
                eprintln!("script: {l}");
            }
            m.platform.show_scripts(h, lines);
        }
    }
    // The --script-keys keys held in the frame before.
    let mut script_held = std::collections::BTreeSet::new();
    // Whether the cartridge save changed in the frame before, for
    // `savedataUpdated`, which fires on the first frame of a change.
    let mut save_was_dirty = false;
    let mut hashes = opts.frame_hashes.as_ref().map(|_| String::new());
    let mut record = match &opts.native_record {
        Some(p) => Some((
            native_record::Writer::create(p).map_err(|e| format!("{}: {e}", p.display()))?,
            native_record::Frame::new(),
            (0, 0),
        )),
        None => None,
    };
    let mut times = opts.frame_times.as_ref().map(|_| String::new());
    #[cfg(feature = "provenance")]
    let mut assets = match (&opts.assets_log, &opts.extract) {
        (Some(d), _) => Some(assets_log::Recorder::new(d)?),
        (None, Some(d)) => {
            let session = d.join(format!("play-{}", sav_file::stamp_now()));
            eprintln!("gbaa-desktop: extracting into {}", session.display());
            Some(assets_log::Recorder::new(&session)?)
        }
        (None, None) => None,
    };
    // A pointer, as the guest on its own stack reaches it (src/coro.rs).
    let stats_ptr: *mut runtime::dispatch::Stats = Box::into_raw(Box::default());
    let mut frames = 0;
    let frame = |m: &mut DesktopMachine, stats: &mut runtime::dispatch::Stats| match code {
        Some(c) if opts.interp_blocks => {
            runtime::dispatch::run_blocks(m, c, None, stats);
        }
        Some(c) => runtime::dispatch::run_frame(m, c, stats),
        None => m.run_frame(),
    };
    let mut rewind = (opts.rewind.is_some() || !opts.headless && opts.rewind_mb > 0)
        .then(|| rewind::Rewind::new(opts.rewind_mb << 20));
    if let Some(r) = &mut rewind {
        r.push(0, rewind::state(&mut m));
    }
    // Steps of the loop: frames run or stepped back.
    let mut steps = 0;
    let mut pace = None;
    // Owned through a pointer, as the guest needs (src/coro.rs): `m` is
    // the owner's until a guest starts, then the one it hands out.
    let owner = Box::into_raw(m);
    // SAFETY: no guest yet.
    let mut m: &mut DesktopMachine = unsafe { &mut *owner };
    // With a guest, the cycles it ran (and its overlays'), for the
    // compiled share ([`guest_counts`]), and the deepest its stack went.
    // The audio-pace run steps frames itself, through `frame`: no guest.
    let guest_code =
        code.filter(|_| !opts.interp_blocks && !opts.unwind && opts.audio_pace.is_none());
    let mut ran = (0, 0);
    let mut guest_stack = None;
    if let Some(drift) = opts.audio_pace {
        let device = audio::sim::Device {
            rate: PACE_RATE,
            drift,
            quantum: PACE_QUANTUM,
            jitter: true,
            record: opts.played_wav.is_some(),
        };
        let n = opts.frames.unwrap_or(HEADLESS_FRAMES);
        let seconds = (n * runtime::sched::FRAME_CYCLES) as f64 / capture::CLOCK_HZ as f64;
        let report = audio::sim::run(
            &mut *m,
            |m| m.platform.player.as_mut().expect("a simulated player"),
            |m| {
                let t = std::time::Instant::now();
                // After a fault the machine stands still; time still passes.
                if m.fault.is_none() {
                    // SAFETY: only the frame loop has it.
                    frame(m, unsafe { &mut *stats_ptr });
                    frames += 1;
                }
                t.elapsed().as_secs_f64()
            },
            device,
            seconds,
        );
        println!("audio-pace {:+.1}%: {report}", drift * 100.0);
        if let Some(path) = &opts.played_wav {
            let pcm: Vec<(i16, i16)> = report
                .played
                .iter()
                .map(|&[l, r]| (capture::pcm16(l), capture::pcm16(r)))
                .collect();
            capture::write(path, &capture::wav(&pcm, PACE_RATE))?;
        }
        pace = Some(report);
    } else {
        let mut undo_load = None;
        let mut guest: Option<coro::Guest> = None;
        while opts.frames.is_none_or(|n| frames < n) && m.platform.is_open() {
            // The window's cartridge clock is the wall clock (in UTC);
            // headless runs keep runtime::gpio::FIXED_CLOCK, as mgba-dump does.
            if !opts.headless
                && let Ok(t) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            {
                m.bus.gpio.clock = t.as_secs() as i64;
            }
            steps += 1;
            let back = m.platform.rewind
                || opts
                    .rewind
                    .is_some_and(|(at, n)| steps > at && steps <= at + n);
            if back && let Some(r) = &mut rewind {
                if let Some((f, s)) = r.back() {
                    if end_guest(&mut guest, &mut guest_stack) {
                        // SAFETY: the guest is gone.
                        m = unsafe { &mut *owner };
                    }
                    m.load_payload(s).expect("rewind's own states load");
                    #[cfg(feature = "provenance")]
                    runtime::prov::forget();
                    // The cartridge save goes back too: write it out again.
                    m.bus.save.dirty = true;
                    frames = f;
                }
                m.platform.rewound(frames, &m.ppu.frame);
            } else {
                // What the window's console asked: loads, unloads and
                // typed commands, between frames as callbacks run.
                let cmds = m.platform.take_script_cmds();
                if !cmds.is_empty() {
                    if host.is_none() {
                        host = Some(scripting::host(
                            rom,
                            &opts.states,
                            code,
                            opts.interp_blocks,
                        )?);
                    }
                    let h = host.as_ref().expect("made above");
                    let cpu = m.cpu.clone();
                    for c in cmds {
                        scripting::act(h, m, c);
                    }
                    let lines = h.take_console();
                    for l in &lines {
                        eprintln!("script: {l}");
                    }
                    m.platform.show_scripts(h, lines);
                    if script_moved(m, m.cpu != cpu, &mut guest, &mut guest_stack) {
                        // SAFETY: the guest is gone.
                        m = unsafe { &mut *owner };
                    }
                }
                // The keyboard and mouse, the window's and --script-keys',
                // for `input`, before the frame as mGBA's events come.
                // A --script-keys press at frame N comes before frame
                // N + 1, so callbacks see it with an --input press at N in
                // `emu:getKeys` (the keys of the poll before).
                let mut events = m.platform.take_script_input();
                let (held, mods) = match frames.checked_sub(1) {
                    Some(f) => opts.script_keys.held(f),
                    None => Default::default(),
                };
                let key = |key, state| script::input::Event::Key { key, state, mods };
                let up = script_held.difference(&held);
                events.extend(up.map(|&k| key(k, script::input::State::Up)));
                let down = held.difference(&script_held);
                events.extend(down.map(|&k| key(k, script::input::State::Down)));
                script_held = held;
                if let Some(h) = &host {
                    let (pad, cpu) = (m.platform.keys_held, m.cpu.clone());
                    for ev in events {
                        h.input(m, ev);
                    }
                    m.platform.script_keys = h.keys_read(m, pad);
                    if script_moved(m, m.cpu != cpu, &mut guest, &mut guest_stack) {
                        // SAFETY: the guest is gone.
                        m = unsafe { &mut *owner };
                    }
                }
                let t = std::time::Instant::now();
                // A script's breakpoint or watchpoint: the frame runs in
                // the interpreter, which stops at each instruction.
                let debug = host.as_ref().filter(|h| h.has_points());
                match guest_code {
                    _ if let Some(h) = debug => {
                        if end_guest(&mut guest, &mut guest_stack) {
                            // SAFETY: the guest is gone.
                            m = unsafe { &mut *owner };
                        }
                        h.run_frame_debug(m);
                    }
                    Some(c) => {
                        let g = guest.get_or_insert_with(|| coro::Guest::new(owner, c, stats_ptr));
                        let (t, o) = (m.now(), m.bus.overlays.cycles);
                        match g.frame() {
                            Some(p) => {
                                // SAFETY: the guest's, until the next frame.
                                m = unsafe { &mut *p };
                                m.show_frame();
                            }
                            None => {
                                // A fault ended the guest.
                                end_guest(&mut guest, &mut guest_stack);
                                // SAFETY: the guest is gone.
                                m = unsafe { &mut *owner };
                            }
                        }
                        ran.0 += m.now() - t;
                        ran.1 += m.bus.overlays.cycles - o;
                    }
                    // SAFETY: only the frame loop has it.
                    None => frame(m, unsafe { &mut *stats_ptr }),
                }
                frames += 1;
                if let Some(r) = &mut rogue {
                    r.frame(&mut m.bus);
                    for l in r.take_log() {
                        eprintln!("rogue: {l}");
                    }
                }
                if let Some(s) = &mut times {
                    s.push_str(&format!("frame {frames} us {}\n", t.elapsed().as_micros()));
                }
                if let Some(h) = &host {
                    let cpu = m.cpu.clone();
                    h.frame(m, frames);
                    let dirty = m.bus.save.dirty;
                    if dirty && !save_was_dirty {
                        h.fire(m, "savedataUpdated");
                    }
                    if script_moved(m, m.cpu != cpu, &mut guest, &mut guest_stack) {
                        // SAFETY: the guest is gone.
                        m = unsafe { &mut *owner };
                    }
                    save_was_dirty = dirty;
                    if sav.is_none() {
                        m.bus.save.dirty = false;
                    }
                    if !opts.headless {
                        let lines = h.take_console();
                        for l in &lines {
                            eprintln!("script: {l}");
                        }
                        m.platform.show_scripts(h, lines);
                        m.platform.show_overlay(h);
                    }
                }
                if let Some(r) = &mut rewind {
                    r.push(frames, rewind::state(m));
                }
            }
            if let Some(h) = &mut hashes {
                h.push_str(&format!(
                    "frame {frames} hash {:016x}\n",
                    frame_hash(&m.ppu.frame)
                ));
            }
            if let Some((w, f, mark)) = &mut record
                && !back
            {
                native_frame(m, frames, f, mark);
                w.write(f).map_err(|e| format!("--native-record: {e}"))?;
            }
            #[cfg(feature = "provenance")]
            if let Some(a) = &mut assets
                && !back
            {
                a.frame(m, frames);
                if let Some(d) = &opts.extract
                    && m.platform.take_export()
                {
                    let said = a.write().and_then(|()| extract(rom, d));
                    let said = said.unwrap_or_else(|e| format!("export failed: {e}"));
                    eprintln!("gbaa-desktop: {said}");
                    m.platform
                        .set_title(&format!("gbaa: {}", said.lines().next().unwrap_or("")));
                }
            }
            if let Some(dir) = &opts.dump_dir
                && !back
                && opts.dump_at.contains(&frames)
            {
                let path = dump_at_path(dir, frames);
                let overlay = host.as_ref().and_then(|h| h.overlay());
                write_png_over(&path, &m.ppu.frame, overlay.as_ref())
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                if opts.dump_states {
                    slots::save(m, &path.with_extension("state"))?;
                }
                #[cfg(feature = "provenance")]
                {
                    let (report, blobs) = prov_report::report(m, frames);
                    for (ext, text) in [("prov.txt", report), ("blobs.tsv", blobs)] {
                        let p = path.with_extension(ext);
                        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
                    }
                }
                if code.is_some() {
                    // SAFETY: read between frames.
                    let s = guest_counts(unsafe { &*stats_ptr }, guest_code.map(|_| ran));
                    eprintln!(
                        "at frame {frames} cycles: {} compiled, {} in the interpreter",
                        s.compiled_cycles, s.interpreted_cycles
                    );
                }
            }
            if let Some(path) = &opts.state_out
                && opts.state_at == Some(frames)
            {
                slots::save(m, path)?;
            }
            if !back && let Some(said) = slots::watch(m, &mut trial) {
                // The fault ended any guest; this one is still the load's.
                if end_guest(&mut guest, &mut guest_stack) {
                    // SAFETY: the guest is gone.
                    m = unsafe { &mut *owner };
                }
                #[cfg(feature = "provenance")]
                runtime::prov::forget();
                undo_load = None;
                m.platform.set_undoable(false);
                eprintln!("gbaa-desktop: {said}");
                m.platform.set_title(&format!("gbaa: {said}"));
            }
            if let Some(a) = m.platform.slots.take() {
                // A load replaces the machine's state under the guest; a
                // save costs the next frame one resume.
                if end_guest(&mut guest, &mut guest_stack) {
                    // SAFETY: the guest is gone.
                    m = unsafe { &mut *owner };
                }
                #[cfg(feature = "provenance")]
                if !matches!(a, slots::Action::Save(_)) {
                    runtime::prov::forget();
                }
                let said = slots::act(m, &opts.states, a, &mut undo_load, &mut trial);
                eprintln!("gbaa-desktop: {said}");
                m.platform.set_title(&format!("gbaa: {said}"));
                m.platform.set_undoable(undo_load.is_some());
            }
            let saved = match &mut sav {
                Some(f) => f.frame(&mut m.bus.save)?,
                None => false,
            };
            if let Some(a) = &mut auto {
                a.set(m.platform.resume_on());
                a.frame(m, !back, saved);
            }
            if m.fault.is_some() {
                if let Some(h) = &host {
                    h.fire(m, "crashed");
                }
                break;
            }
        }
        end_guest(&mut guest, &mut guest_stack);
    }
    // SAFETY: no guest is left, and these were made by Box::into_raw.
    let mut m = unsafe { Box::from_raw(owner) };
    let stats = guest_counts(
        &unsafe { *Box::from_raw(stats_ptr) },
        guest_code.map(|_| ran),
    );
    if let Some(h) = &host {
        h.fire(&mut m, "stop");
        h.fire(&mut m, "shutdown");
        h.flush_storage();
        if opts.headless {
            print!("{}", scripting::report(h));
        } else {
            for l in h.take_console() {
                eprintln!("script: {l}");
            }
        }
    }
    if let Some(r) = &rewind {
        eprintln!("gbaa-desktop: {}", r.summary());
    }
    if let Some(f) = &mut sav {
        f.flush(&mut m.bus.save)?;
        if !opts.headless {
            back_up_save(&f.path);
        }
    }
    if let Some(a) = auto {
        a.finish(&mut m);
    }
    if let Some(path) = &opts.state_out
        && opts.state_at.is_none()
    {
        slots::save(&mut m, path)?;
    }
    if let (Some(path), Some(h)) = (&opts.frame_hashes, &hashes) {
        std::fs::write(path, h).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if let Some((mut w, _, _)) = record {
        w.flush().map_err(|e| format!("--native-record: {e}"))?;
        // Only the interpreter sees every sp (runtime::Machine::low_sp).
        if code.is_none() {
            eprintln!("gbaa-desktop: lowest sp {:#010x}", m.low_sp);
        } else {
            eprintln!("gbaa-desktop: lowest sp not kept by compiled code (run with --interp)");
        }
    }
    #[cfg(feature = "timing-lab")]
    timing_lab::finish()?;
    if let (Some(path), Some(s)) = (&opts.frame_times, &times) {
        std::fs::write(path, s).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    #[cfg(feature = "provenance")]
    if let Some(a) = assets {
        a.finish()?;
        if let Some(d) = &opts.extract {
            eprintln!("gbaa-desktop: {}", extract(rom, d)?);
        }
    }
    if let Some(n) = opts.state_bench {
        println!("{}", state_bench(&mut m, n)?);
    }
    if let Some(n) = opts.rerun_bench {
        let mut stats = runtime::dispatch::Stats::default();
        let frame = |m: &mut DesktopMachine| match code {
            Some(c) => runtime::dispatch::run_frame(m, c, &mut stats),
            None => m.run_frame(),
        };
        println!("{}", rerun_bench(&mut m, n, frame)?);
    }
    if let Some(c) = code {
        // Before the lookup below, which may check a dirty overlay.
        let invalidations = m.bus.overlays.invalidations;
        let k = runtime::dispatch::key(&m.cpu);
        eprintln!(
            "recompiled: {} dispatcher calls ({} resuming a function past its \
             entry, {} returning to yield), {} instructions interpreted; \
             fallback entered at {} ROM addresses{}",
            stats.calls,
            stats.resumes,
            stats.yields,
            stats.interpreted,
            stats.n_seeds,
            if c.lookup(&mut m.bus, k).is_some() {
                ""
            } else {
                " (now in the fallback)"
            },
        );
        if !c.modded.is_empty() {
            eprintln!(
                "mod: {} of the resumes landed inside a modded function ({} at a \
                 call the mod added whose frame was gone, going on where its \
                 block began)",
                stats.mod_resumes, stats.mod_call_resumes
            );
        }
        // Decision 1's "would change our minds", in guest cycles: the share
        // of the running (not halted) time the fallback took.
        let running = (stats.compiled_cycles + stats.interpreted_cycles).max(1);
        let pct = |c: u64| c as f64 * 100.0 / running as f64;
        eprintln!(
            "cycles: {} compiled ({:.1}%, compiled in RAM {:.1}%), {} in the interpreter \
             fallback ({:.1}%, {:.1} points of it in RAM code), {} halted",
            stats.compiled_cycles,
            pct(stats.compiled_cycles),
            pct(stats.compiled_ram_cycles),
            stats.interpreted_cycles,
            pct(stats.interpreted_cycles),
            pct(stats.interpreted_ram_cycles),
            stats.halted_cycles,
        );
        eprintln!(
            "settled: {} calls run on to their return site in place ({} left \
             elsewhere and unwound), {} IRQs and halts taken in the tick ({} \
             unwound), nested {} deep at most; host stack {} bytes deep at most",
            stats.settles,
            stats.unsettled,
            stats.irqs_in_place,
            stats.irqs_unwound,
            stats.max_depth,
            stats.max_stack,
        );
        if guest_code.is_some() {
            eprintln!(
                "guest stack: {} frame ends switched to the frame loop; {} of {} KiB used",
                stats.switches,
                guest_stack.map_or("?".into(), |b: usize| format!("{} bytes", b)),
                coro::STACK >> 10,
            );
        }
        eprintln!(
            "overlays: {}; overlay invalidations: {}",
            c.overlays.len(),
            invalidations
        );
        // Only where the generated crate is: a binary copied elsewhere (a
        // Steam Deck build) has no folder to write it in.
        if let Some(path) = c.seeds_file
            && std::path::Path::new(path)
                .parent()
                .is_some_and(|d| d.is_dir())
        {
            let seeds: String = stats.seeds[..stats.n_seeds]
                .iter()
                .map(|k| format!("{k:#010x}\n"))
                .collect();
            std::fs::write(path, seeds).map_err(|e| format!("{path}: {e}"))?;
        }
    }
    if let Some(path) = &opts.dump {
        let overlay = host.as_ref().and_then(|h| h.overlay());
        write_png_over(path, &m.ppu.frame, overlay.as_ref())
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if opts.hash {
        println!("frame {frames} hash {:016x}", frame_hash(&m.ppu.frame));
    }
    if let Some(c) = &m.platform.capture {
        audio_out(opts, c, &m.bus.sound.swap)?;
    }
    if !opts.headless
        && let Some(p) = &m.platform.player
    {
        println!("{}", p.summary());
    }
    if opts.regs {
        settle(&mut m);
        let c = &m.cpu;
        println!("pc {:08x} cpsr {:08x}", c.pc, c.cpsr);
        for (i, r) in c.r.iter().enumerate() {
            print!("r{i}={r:08x}{}", if i % 5 == 4 { "\n" } else { " " });
        }
    }
    // The next run in the window (a netplay session) starts from here.
    if !opts.headless && m.fault.is_none() {
        *carry = Some(slots::state(&mut m));
    }
    match (m.fault, pace) {
        (Some(fault), _) => Err(format!("stopped after {frames} frames: {fault}")),
        (None, Some(r)) if !r.passes() => Err(
            "audio-pace: an underrun after the first second, or the queue outside 20-100 ms".into(),
        ),
        _ => Ok(()),
    }
}

/// Drop the guest, if there is one (src/coro.rs), noting how deep its
/// stack went in `deepest`. Whether there was one: the machine is the
/// owner's again.
/// Frame `n`'s record for the native check (`--native-record`), into
/// `f`: the picture's hash, the RAM, the I/O block as stored (but
/// `DISPSTAT` its writable bits and `VCOUNT` the beam's line), the
/// display's memories, and the sound captured since the frame before,
/// `mark` being how far into the capture's FIFO and PSG bytes that was.
fn native_frame(
    m: &DesktopMachine,
    n: u64,
    f: &mut native_record::Frame,
    mark: &mut (usize, usize),
) {
    use native_record::Region;
    use runtime::io::reg;
    f.frame = n;
    f.hash = frame_hash(&m.ppu.frame);
    let b = &m.bus;
    f.region_mut(Region::Ewram).copy_from_slice(&b.ewram);
    f.region_mut(Region::Iwram).copy_from_slice(&b.iwram);
    f.region_mut(Region::Palette)
        .copy_from_slice(&b.video.palette);
    f.region_mut(Region::Vram).copy_from_slice(&b.video.vram);
    f.region_mut(Region::Oam).copy_from_slice(&b.video.oam);
    for o in (0..0x400).step_by(2) {
        f.set_io16(o, b.io.raw16(o));
    }
    f.set_io16(reg::DISPSTAT, b.io.raw16(reg::DISPSTAT) & DISPSTAT_WRITABLE);
    f.set_io16(reg::VCOUNT, b.io.read16(reg::VCOUNT));
    f.audio = m.platform.capture.as_ref().map(|c| {
        let (fifo, psg) = (&c.fifo[mark.0..], &c.psg[mark.1..]);
        let a = native_record::Audio {
            samples: (fifo.len() / 2 + psg.len() / 4) as u32,
            hash: native_record::fnv(native_record::fnv(native_record::FNV, fifo), psg),
            silent: fifo.iter().chain(psg).all(|&b| b == 0),
        };
        *mark = (c.fifo.len(), c.psg.len());
        a
    });
}

/// `DISPSTAT`'s bits the CPU writes: the three IRQ enables and the
/// V-count line.
pub const DISPSTAT_WRITABLE: u16 = 0xFF38;

fn end_guest(guest: &mut Option<coro::Guest>, deepest: &mut Option<usize>) -> bool {
    let Some(g) = guest.take() else {
        return false;
    };
    if let Some(b) = g.high_water() {
        *deepest = Some(deepest.map_or(b, |d| d.max(b)));
    }
    true
}

/// After script callbacks: if they `moved` the CPU, or ran or replaced
/// the machine (`emu:step`, `writeRegister`, `loadState*`, `runFrame`,
/// `reset`), the suspended guest no longer matches it, so drop it
/// ([`end_guest`]); the next frame starts a new one from `cpu.pc`.
/// Whether it did.
fn script_moved(
    m: &DesktopMachine,
    moved: bool,
    guest: &mut Option<coro::Guest>,
    deepest: &mut Option<usize>,
) -> bool {
    // `runFrame` and `reset` leave no switch (scripting::host).
    (moved || m.nest.frame_end.is_none()) && end_guest(guest, deepest)
}

/// `stats` with a guest's compiled cycles: the `ran` cycles (and its
/// overlays') less the interpreter's and the halts'. Compiled code
/// suspended at a frame end returns to the dispatcher, which counts its
/// cycles, only when the guest is dropped, if ever.
fn guest_counts(
    stats: &runtime::dispatch::Stats,
    ran: Option<(u64, u64)>,
) -> runtime::dispatch::Stats {
    let mut s = stats.clone();
    if let Some((all, ram)) = ran {
        s.compiled_cycles = all.saturating_sub(s.interpreted_cycles + s.halted_cycles);
        s.compiled_ram_cycles = ram;
    }
    s
}

/// `--audio-pace`'s device: 48 kHz, taking up to 1024 frames a pull
/// (21 ms, PipeWire's default quantum), irregularly.
pub const PACE_RATE: u32 = 48000;
pub const PACE_QUANTUM: usize = 1024;

/// Write and print the sound outputs `opts` asks for.
fn audio_out(opts: &Options, c: &Capture, swap: &runtime::sound::SwapStats) -> Result<(), String> {
    if let Some(path) = &opts.wav {
        capture::write(
            path,
            &capture::wav(c.samples.as_deref().unwrap_or(&[]), c.rate()),
        )?;
    }
    if let Some(path) = &opts.fifo_dump {
        capture::write(path, &c.fifo)?;
    }
    if let Some(path) = &opts.psg_dump {
        capture::write(path, &c.psg)?;
        println!("audio psg dropped {}", c.psg_dropped);
    }
    if let Some(path) = &opts.audio_log {
        capture::write(path, c.log().as_bytes())?;
    }
    if opts.audio_hash {
        println!("audio {} records hash {:016x}", c.records, c.hash());
    }
    println!("audio dropped {}", c.dropped);
    // Timing detail that may differ between the interpreter and recompiled
    // code, so not on stdout, which the checks compare.
    eprintln!(
        "audio rate {} Hz (period {}), swaps {}, relatches {}, swap to relatch max {} cycles \
         {} samples {} refills",
        c.rate(),
        c.period,
        swap.swaps,
        swap.relatches,
        swap.max_cycles,
        swap.max_pops,
        swap.max_refills,
    );
    Ok(())
}

#[cfg(test)]
mod tests;
