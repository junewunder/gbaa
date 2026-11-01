//! Resume where you left off: the window keeps an auto state of the game
//! in its folder ([`crate::game_dir`]), `<name>.resume`, and at launch starts from it straight
//! away. It is apart from the four slots ([`crate::slots`]) and off by
//! default (june, 2026-10-30): `resume = on` in the settings file (or the
//! menu's "Resume on launch" row) turns it on, and `resume = off` or
//! leaving it out keeps it off; `--no-resume` boots the game from
//! its start for one run, which then keeps auto states as usual.
//!
//! It has to survive a crash or a pulled battery, so it is taken every
//! [`EVERY`] frames of play (about 3 seconds; none while the menu pauses
//! the game or rewind is held), right after each write of the `.sav`, and
//! once more on quit. Each is written by a thread of its own to
//! `<name>.resume.tmp`, flushed to the disk, and renamed into place; the
//! one before it is kept as `<name>.resume.old`, which launch falls back
//! to when the newest won't load. A crash mid-write leaves the last
//! whole one.
//!
//! A state holds the cartridge's save memory too. Launch keeps the
//! state's only when the auto state is at least as new as the `.sav`;
//! otherwise (the game saved after the last auto state, and the run
//! ended before the next) the `.sav`'s is kept, so an in-game save is
//! never undone by a resume. A run that ends in a fault keeps its auto
//! state as `<name>.resume.fault` instead, so the next launch boots
//! afresh, and doesn't run back into it. Turning the setting off deletes
//! them.
//!
//! An auto state saved from another ROM (another version of the game)
//! loads anyway, copied first to `<name>.resume.bak` ([`crate::slots::load`]);
//! if it faults within a second, the game starts afresh ([`crate::slots::Trial`]).

use crate::DesktopMachine;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Frames of play between auto states: about 3 seconds.
pub const EVERY: u64 = 180;
/// And at least this long between them, so fast-forward doesn't write
/// one after another.
pub const MIN_GAP: Duration = Duration::from_secs(1);

/// `<stem>.resume`, the newest auto state.
pub fn path(stem: &Path) -> PathBuf {
    stem.with_extension("resume")
}

/// `<stem>.resume.old`, the one before it.
pub fn old_path(stem: &Path) -> PathBuf {
    stem.with_extension("resume.old")
}

/// `<stem>.resume.fault`, the last auto state of a run that faulted.
pub fn fault_path(stem: &Path) -> PathBuf {
    stem.with_extension("resume.fault")
}

/// Write `bytes` as the newest auto state for `stem`: to a temporary file,
/// flushed to the disk, the newest so far renamed to the old one, then the
/// temporary file renamed into place.
pub fn write(stem: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let (new, old) = (path(stem), old_path(stem));
    let tmp = stem.with_extension("resume.tmp");
    let err = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    let mut f = std::fs::File::create(&tmp).map_err(|e| err(&tmp, e))?;
    f.write_all(bytes)
        .and_then(|()| f.sync_all())
        .map_err(|e| err(&tmp, e))?;
    drop(f);
    match std::fs::rename(&new, &old) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(err(&new, e)),
    }
    std::fs::rename(&tmp, &new).map_err(|e| err(&new, e))?;
    sync_dir(stem);
    Ok(())
}

/// Flush the renames in the game's folder to the disk, where that can be
/// asked for (not on Windows, whose renames need no such step).
fn sync_dir(stem: &Path) {
    #[cfg(unix)]
    {
        let dir = match stem.parent() {
            Some(d) if !d.as_os_str().is_empty() => d,
            _ => Path::new("."),
        };
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = stem;
}

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Start the machine from `stem`'s auto state, the newest that loads.
/// `sav` is the `.sav` the cartridge save was read from: newer than the
/// state, its save is kept ([the module's](self) rule). `None` if there
/// is no auto state; else what happened, said for the window's title.
/// One from another ROM goes on `trial` ([`crate::slots::watch`]): if it
/// faults, the game starts afresh.
pub fn load(
    m: &mut DesktopMachine,
    stem: &Path,
    sav: Option<&Path>,
    trial: &mut Option<crate::slots::Trial>,
) -> Option<String> {
    let mut failed = Vec::new();
    // A load that fails changes nothing, so this is the machine before
    // whichever one works.
    let before = crate::slots::state(m);
    for p in [path(stem), old_path(stem)] {
        let Ok(bytes) = std::fs::read(&p) else {
            continue;
        };
        let cart = m.bus.save.data.to_vec();
        let other_rom = match crate::slots::load_file(m, &p, bytes) {
            Ok(bak) => bak.is_some(),
            Err(e) => {
                failed.push(e);
                continue;
            }
        };
        let sav_newer = sav
            .and_then(modified)
            .zip(modified(&p))
            .is_some_and(|(s, r)| s > r);
        if sav_newer {
            m.bus.save.data.copy_from_slice(&cart);
        } else if m.bus.save.data[..] != cart[..] {
            // The state's save is the newer: the .sav gets it.
            m.bus.save.dirty = true;
        }
        for f in &failed {
            eprintln!("gbaa-desktop: auto state not loaded: {f}");
        }
        let said = match (failed.is_empty(), sav_newer) {
            (true, false) => "resumed where you left off",
            (true, true) => "resumed where you left off, with the newer .sav",
            (false, _) => "resumed from the auto state before the last",
        };
        if other_rom {
            *trial = Some(crate::slots::Trial::new(
                before,
                "the auto state",
                "started afresh",
            ));
        }
        return Some(if other_rom {
            format!("{said}, from another ROM (kept as .bak)")
        } else {
            said.into()
        });
    }
    for f in &failed {
        eprintln!("gbaa-desktop: auto state not loaded: {f}");
    }
    (!failed.is_empty()).then(|| "the auto state didn't load: started afresh".into())
}

/// Remove `stem`'s auto states (the setting turned off).
pub fn remove(stem: &Path) {
    for p in [path(stem), old_path(stem)] {
        let _ = std::fs::remove_file(p);
    }
}

/// What the writer thread is handed: the newest state not yet written
/// (a newer one replaces it), whether to remove the auto states instead
/// (after any write under way, which would bring one back), and whether
/// the run is over.
#[derive(Default)]
struct Queue {
    next: Option<Vec<u8>>,
    remove: bool,
    over: bool,
}

/// The run's auto states: when to take them, and the thread writing them.
pub struct Auto {
    stem: PathBuf,
    on: bool,
    /// Frames played since the last one, and when it was taken.
    since: u64,
    last: Instant,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Auto {
    /// Auto states at `stem` (`<name>` in the game's folder), taken while
    /// `on`.
    pub fn new(stem: &Path, on: bool) -> Auto {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let q = queue.clone();
        let r = stem.to_owned();
        let thread = std::thread::Builder::new()
            .name("auto state".into())
            .spawn(move || writer(&r, &q))
            .ok();
        Auto {
            stem: stem.to_owned(),
            on,
            since: 0,
            last: Instant::now(),
            queue,
            thread,
        }
    }

    /// The setting, as the menu has it now: turned off, the auto states
    /// go.
    pub fn set(&mut self, on: bool) {
        if self.on && !on {
            let mut q = self.lock();
            q.next = None;
            q.remove = true;
            drop(q);
            self.queue.1.notify_one();
            eprintln!("gbaa-desktop: resume on launch off: auto states removed");
        }
        self.on = on;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn hand(&self, state: Vec<u8>) {
        self.lock().next = Some(state);
        self.queue.1.notify_one();
    }

    /// After each step of the run loop: `played`, whether a frame ran (not
    /// rewound); `saved`, whether the `.sav` was just written. Takes an
    /// auto state when one is due.
    pub fn frame(&mut self, m: &mut DesktopMachine, played: bool, saved: bool) {
        if m.fault.is_some() {
            return;
        }
        self.since += u64::from(played);
        let due = self.since >= EVERY && self.last.elapsed() >= MIN_GAP;
        if self.on && (saved || due) {
            self.since = 0;
            self.last = Instant::now();
            self.hand(crate::slots::state(m));
        }
    }

    /// The run is over: take the last auto state, unless it ended in a
    /// fault, which sets the auto state aside instead. Waits for it to be
    /// written.
    pub fn finish(mut self, m: &mut DesktopMachine) {
        let fault = m.fault.is_some();
        if self.on && !fault {
            self.hand(crate::slots::state(m));
        }
        self.lock().over = true;
        self.queue.1.notify_one();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if self.on && fault {
            let _ = std::fs::rename(path(&self.stem), fault_path(&self.stem));
            let _ = std::fs::remove_file(old_path(&self.stem));
            eprintln!(
                "gbaa-desktop: the game faulted: its auto state is set aside as {}",
                fault_path(&self.stem).display()
            );
        }
    }
}

impl Drop for Auto {
    /// A run that ended in an error: the thread finishes what it has.
    fn drop(&mut self) {
        self.lock().over = true;
        self.queue.1.notify_one();
    }
}

/// The writer thread: write each state handed over, the newest only, until
/// the run is over and none is left. Says each different failure once.
fn writer(stem: &Path, q: &(Mutex<Queue>, Condvar)) {
    let mut said = String::new();
    loop {
        let next = {
            let mut g = q.0.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if let Some(s) = g.next.take() {
                    break Some(s);
                }
                if std::mem::take(&mut g.remove) {
                    break None;
                }
                if g.over {
                    return;
                }
                g = q.1.wait(g).unwrap_or_else(|e| e.into_inner());
            }
        };
        let Some(next) = next else {
            remove(stem);
            continue;
        };
        if let Err(e) = write(stem, &next)
            && e != said
        {
            eprintln!("gbaa-desktop: auto state not written: {e}");
            said = e;
        }
    }
}
