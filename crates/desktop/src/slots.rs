//! Our own save states ([`runtime::snap`]) in files, and the window's quick
//! save and load slots: `<name>.state1` to `<name>.state4` in the game's
//! folder ([`crate::game_dir`]).
//! Shift+F1–F4 saves to a slot and F1–F4 loads one; on a gamepad
//! `save-state` and `load-state` (unbound by default) save to and
//! load the slot last used, slot 1 at first. A state holds the cartridge's
//! save memory too, so loading one puts it back as it was then, as mGBA
//! does.
//!
//! Loading a state keeps the game as it was just before, so the load can
//! be undone ([`Action::UndoLoad`], as mGBA's "undo load state"): F5 on
//! the keyboard, or the menu's "Undo load state" row; no pad button.
//!
//! A state saved from another ROM (another version of the game) loads
//! anyway, copied first to `<name>.state<n>.bak` ([`load`]). If the
//! game then faults within [`TRIAL_FRAMES`] frames (another build's code
//! where this one's data is: pokeemerald modern's state from the original
//! ROM jumps to 0 in its first frame), the load is undone and the window
//! says why ([`Trial`]); the `.bak` stays.
//!
//! Scripts reach the same files, any slot, through `emu:saveStateSlot`
//! and `loadStateSlot` ([`crate::scripting::host`]).
//!
//! States are saved and loaded between frames, where the run loop calls
//! [`Slots::take`].

use crate::DesktopMachine;
use crate::gamepad;
use std::path::{Path, PathBuf};

/// The window's slots, 1 to this.
pub const SLOTS: u8 = 4;

/// What to do between frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Save(u8),
    Load(u8),
    /// Put back the game as it was before the last load.
    UndoLoad,
}

/// `<stem>.state<slot>`: the game's `<name>.state<slot>` in its folder
/// ([`crate::game_dir`]).
pub fn path(stem: &Path, slot: u8) -> PathBuf {
    stem.with_extension(format!("state{slot}"))
}

/// The machine's state, as [`runtime::snap`] writes it.
pub fn state(m: &mut DesktopMachine) -> Vec<u8> {
    let mut bytes = Vec::new();
    m.save_state(&mut |s| bytes.extend_from_slice(s));
    bytes
}

/// Write the machine's state to `path`, through a temporary file renamed
/// over it.
pub fn save(m: &mut DesktopMachine, path: &Path) -> Result<(), String> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, state(m))
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// `<path>.bak`, where a state from another ROM is kept before it loads.
pub fn bak_path(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".bak");
    p.into()
}

/// Load the state at `path`: ours, or mGBA's ([`crate::state_file`]),
/// whose save data replaces the cartridge's if it has any. Nothing is
/// changed if it fails. One of ours saved from another ROM (its header
/// has the ROM's CRC-32: another version of the game) loads anyway, as
/// the design has it (`docs/save-locations-design.md`), after the file is
/// copied to [`bak_path`]: whether such a state plays right isn't known
/// yet, and the next save over it mustn't lose it. `Some` names that copy.
pub fn load(m: &mut DesktopMachine, path: &Path) -> Result<Option<PathBuf>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    load_file(m, path, bytes)
}

/// [`load`], the file at `path` read already as `bytes`.
pub fn load_file(
    m: &mut DesktopMachine,
    path: &Path,
    bytes: Vec<u8>,
) -> Result<Option<PathBuf>, String> {
    let err = |e: String| format!("{}: {e}", path.display());
    let rom = m.bus.rom_crc();
    let other = runtime::snap::state_rom(&bytes).filter(|&s| s != rom);
    let Some(state) = other else {
        return load_bytes(m, bytes).map(|()| None).map_err(err);
    };
    let bak = bak_path(path);
    eprintln!(
        "gbaa-desktop: {} was saved from another ROM (CRC-32 {state:08x}, this one's \
         is {rom:08x}): copied to {} and loaded anyway",
        path.display(),
        bak.display()
    );
    // A load that goes wrong leaves the file as it was, so a copy that
    // fails is said, and the load goes ahead.
    if let Err(e) = std::fs::write(&bak, &bytes) {
        eprintln!("gbaa-desktop: {}: {e}", bak.display());
    }
    m.load_state_any_rom(&bytes)
        .map(|()| Some(bak))
        .map_err(|e| err(e.to_string()))
}

/// [`load`], from the state file's bytes, which must be from this ROM.
pub fn load_bytes(m: &mut DesktopMachine, bytes: Vec<u8>) -> Result<(), String> {
    let s = crate::state_file::unpack(bytes)?;
    if runtime::snap::is_ours(&s.state) {
        return m.load_state(&s.state).map_err(|e| e.to_string());
    }
    m.load_mgba_state(&s.state).map_err(|e| e.to_string())?;
    if let Some(save) = &s.save {
        let data = &mut m.bus.save.data;
        let n = save.len().min(data.len());
        data[..n].copy_from_slice(&save[..n]);
    }
    Ok(())
}

/// Frames a state from another ROM has to run without a fault before
/// it's kept ([`Trial`]): one second.
pub const TRIAL_FRAMES: u32 = 60;

/// A state from another ROM, just loaded, on trial: the machine as it
/// was before the load, put back if the game faults within
/// [`TRIAL_FRAMES`] frames ([`watch`]).
#[derive(Clone, Debug)]
pub struct Trial {
    before: Vec<u8>,
    left: u32,
    /// What was loaded, for the message: "state 2", "the auto state".
    what: String,
    /// What the machine is put back to, for the message.
    back_to: &'static str,
}

impl Trial {
    /// `before` is the machine's state (from this ROM) before the load of
    /// `what`; `back_to` says what putting it back gives, "load undone"
    /// or "started afresh".
    pub fn new(before: Vec<u8>, what: impl Into<String>, back_to: &'static str) -> Trial {
        Trial {
            before,
            left: TRIAL_FRAMES,
            what: what.into(),
            back_to,
        }
    }
}

/// After each frame run: if a load on `trial` has faulted, put the machine
/// back as it was before it, end the trial, and say why (for stderr and
/// the window's title). A trial that has run its frames ends quietly.
pub fn watch(m: &mut DesktopMachine, trial: &mut Option<Trial>) -> Option<String> {
    let t = trial.as_mut()?;
    let Some(fault) = m.fault else {
        t.left = t.left.saturating_sub(1);
        if t.left == 0 {
            *trial = None;
        }
        return None;
    };
    let t = trial.take().expect("matched above");
    let ran = TRIAL_FRAMES - t.left + 1;
    let what = t.what;
    // The state before is this ROM's own, saved a moment ago.
    Some(match m.load_state(&t.before) {
        Ok(()) => {
            // The cartridge save goes back too: write it out again.
            m.bus.save.dirty = true;
            format!(
                "{what} is from another ROM and crashed in frame {ran} ({fault}): {}",
                t.back_to
            )
        }
        Err(e) => format!("{what} is from another ROM and crashed ({fault}); not undone: {e}"),
    })
}

/// The window's slot controls: which slot the pad's buttons use, and the
/// action asked for since the run loop last looked.
#[derive(Clone, Debug)]
pub struct Slots {
    current: u8,
    /// The pad's state buttons at the last read, to act on presses only.
    pad_was: u16,
    pending: Option<Action>,
}

impl Default for Slots {
    fn default() -> Slots {
        Slots {
            current: 1,
            pad_was: 0,
            pending: None,
        }
    }
}

impl Slots {
    /// The pad buttons held now ([`gamepad::SAVE_STATE`] and
    /// [`gamepad::LOAD_STATE`] among them), and the slot key pressed since
    /// the last read, if any: `(slot, shift held)`; `undo`, whether F5 or
    /// the menu asked to undo the last load since then.
    pub fn input(&mut self, pads: u16, key: Option<(u8, bool)>, undo: bool) {
        let pressed = pads & !self.pad_was;
        self.pad_was = pads;
        let action = match key {
            Some((slot, true)) => Some(Action::Save(slot)),
            Some((slot, false)) => Some(Action::Load(slot)),
            None if pressed & gamepad::SAVE_STATE != 0 => Some(Action::Save(self.current)),
            None if pressed & gamepad::LOAD_STATE != 0 => Some(Action::Load(self.current)),
            None if undo => Some(Action::UndoLoad),
            None => None,
        };
        if let Some(a) = action {
            if let Action::Save(s) | Action::Load(s) = a {
                self.current = s;
            }
            self.pending = Some(a);
        }
    }

    /// The action asked for, once.
    pub fn take(&mut self) -> Option<Action> {
        self.pending.take()
    }
}

/// Do `action` on the machine, its slots at `stem` ([`path`]), and say what happened
/// (on stderr, and in the window's title by the caller). A slot with
/// nothing in it, or a state that won't load, leaves the machine running
/// as it was. `undo` is the game as it was before the last load that
/// worked, which [`Action::UndoLoad`] puts back, once. A load from another
/// ROM goes on `trial` ([`watch`]); any other load, or an undo, ends one.
pub fn act(
    m: &mut DesktopMachine,
    stem: &Path,
    action: Action,
    undo: &mut Option<Vec<u8>>,
    trial: &mut Option<Trial>,
) -> String {
    match action {
        Action::Save(n) => match save(m, &path(stem, n)) {
            Ok(()) => format!("saved state {n}"),
            Err(e) => format!("state {n} not saved: {e}"),
        },
        Action::Load(n) => {
            let p = path(stem, n);
            if !p.exists() {
                return format!("state {n} is empty");
            }
            let before = state(m);
            match load(m, &p) {
                // The cartridge save is as the state had it: keep that.
                Ok(bak) => {
                    m.bus.save.dirty = true;
                    *trial = bak
                        .as_ref()
                        .map(|_| Trial::new(before.clone(), format!("state {n}"), "load undone"));
                    *undo = Some(before);
                    match bak {
                        None => format!("loaded state {n}"),
                        Some(_) => format!("loaded state {n}, from another ROM (kept as .bak)"),
                    }
                }
                Err(e) => format!("state {n} not loaded: {e}"),
            }
        }
        Action::UndoLoad => match undo.take() {
            None => "no load to undo".to_string(),
            Some(before) => match m.load_state(&before) {
                Ok(()) => {
                    m.bus.save.dirty = true;
                    *trial = None;
                    "load undone".to_string()
                }
                Err(e) => format!("load not undone: {e}"),
            },
        },
    }
}
