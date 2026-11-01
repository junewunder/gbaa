//! The cartridge save kept in a file, mGBA's `.sav`: the save data
//! ([`runtime::save::Kind::file_size`] bytes), then whatever mGBA put after it, kept as
//! it is (for a game with a clock, mGBA's 16-byte RTC record: the time,
//! the control byte and the last latch). The window uses `<name>.sav` in
//! the game's folder ([`crate::game_dir`]); headless runs only `--save
//! FILE`, so the checks' runs stay reset-clean.
//!
//! The file is written once the save has been still for [`SETTLE_FRAMES`]
//! (a game's save is many erases and programs over several frames), and on
//! exit, through a temporary file renamed over it. On exit the window
//! also copies it to `backups/<name>-YYYYMMDD-HHMMSS.sav` beside it (local
//! time; [`backup`]), so a bad save can be undone. Past [`BACKUP_CAP`]
//! bytes the oldest backups go, but never a game's newest ([`prune`]).

use runtime::save::Save;
use std::path::{Path, PathBuf};

/// Frames without a change before the file is written.
pub const SETTLE_FRAMES: u32 = 30;

pub struct SavFile {
    pub path: PathBuf,
    /// The file's bytes after the save data, written back unchanged.
    trailer: Vec<u8>,
    /// Frames since the last change not yet written.
    pending: Option<u32>,
}

/// `<rom>.sav`, where mGBA looks: the window's, when there is no data dir
/// for the game's folder.
pub fn beside(rom: &Path) -> PathBuf {
    rom.with_extension("sav")
}

impl SavFile {
    /// Load `path` into `save` if it exists; either way, keep the save
    /// there from now on.
    pub fn open(path: &Path, save: &mut Save) -> Result<SavFile, String> {
        let mut trailer = Vec::new();
        match std::fs::read(path) {
            Ok(bytes) => {
                let n = bytes.len().min(save.kind.file_size());
                save.data[..n].copy_from_slice(&bytes[..n]);
                trailer = bytes[n..].to_vec();
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
        save.dirty = false;
        Ok(SavFile {
            path: path.to_owned(),
            trailer,
            pending: None,
        })
    }

    /// After each frame: note a change, and write the file once the save
    /// has settled. Whether it wrote it.
    pub fn frame(&mut self, save: &mut Save) -> Result<bool, String> {
        if save.dirty {
            save.dirty = false;
            self.pending = Some(0);
        } else if let Some(n) = &mut self.pending {
            *n += 1;
            if *n >= SETTLE_FRAMES {
                self.write(save)?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// On exit: write a change not yet written.
    pub fn flush(&mut self, save: &mut Save) -> Result<(), String> {
        if save.dirty || self.pending.is_some() {
            save.dirty = false;
            self.write(save)?;
        }
        Ok(())
    }

    fn write(&mut self, save: &Save) -> Result<(), String> {
        let mut bytes = save.data[..save.kind.file_size()].to_vec();
        bytes.extend_from_slice(&self.trailer);
        let tmp = self.path.with_extension("sav.tmp");
        std::fs::write(&tmp, &bytes)
            .and_then(|()| std::fs::rename(&tmp, &self.path))
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        self.pending = None;
        Ok(())
    }
}

/// The backups folder beside `sav`.
pub fn backup_dir(sav: &Path) -> PathBuf {
    sav.parent().unwrap_or(Path::new("")).join("backups")
}

/// Copy `sav` to `backups/<name>-<stamp>.sav` beside it, `stamp` being
/// YYYYMMDD-HHMMSS; unless there is no `sav`, or the newest backup holds
/// the same bytes. Returns the backup written.
pub fn backup(sav: &Path, stamp: &str) -> Result<Option<PathBuf>, String> {
    let err = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    let bytes = match std::fs::read(sav) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(err(sav, e)),
    };
    let dir = backup_dir(sav);
    let name = sav.file_stem().unwrap_or_default().to_string_lossy();
    let prefix = format!("{name}-");
    // The stamps sort as the times do, so the newest is the last name.
    let newest = match std::fs::read_dir(&dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|f| {
                f.strip_prefix(&prefix)
                    .and_then(|r| r.strip_suffix(".sav"))
                    .is_some_and(is_stamp)
            })
            .max(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(err(&dir, e)),
    };
    if let Some(f) = newest
        && std::fs::read(dir.join(&f)).is_ok_and(|b| b == bytes)
    {
        return Ok(None);
    }
    std::fs::create_dir_all(&dir).map_err(|e| err(&dir, e))?;
    let path = dir.join(format!("{prefix}{stamp}.sav"));
    let tmp = path.with_extension("sav.tmp");
    std::fs::write(&tmp, &bytes)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| err(&path, e))?;
    Ok(Some(path))
}

/// How big the backups folder may grow: about 1 GB, 8000 or so 128 KB
/// saves.
pub const BACKUP_CAP: u64 = 1 << 30;

/// Delete the oldest backups in `dir` until those left total at most
/// `cap` bytes, keeping each game's newest. Returns the backups deleted.
pub fn prune(dir: &Path, cap: u64) -> Result<Vec<PathBuf>, String> {
    let err = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(err(dir, e)),
    };
    // (stamp, game, name, size) for each `<game>-<stamp>.sav`.
    let mut backups = Vec::new();
    for e in entries.filter_map(|e| e.ok()) {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some((game, stamp)) = name
            .strip_suffix(".sav")
            .and_then(|r| r.split_at_checked(r.len().checked_sub(15)?))
            .and_then(|(g, s)| Some((g.strip_suffix('-')?, s)))
            .filter(|&(_, s)| is_stamp(s))
        else {
            continue;
        };
        let (game, stamp) = (game.to_owned(), stamp.to_owned());
        let size = e.metadata().map_or(0, |m| m.len());
        backups.push((stamp, game, name, size));
    }
    let mut total: u64 = backups.iter().map(|b| b.3).sum();
    // Newest first, so a game's first backup seen is its newest.
    backups.sort_by(|a, b| b.cmp(a));
    let mut games = std::collections::HashSet::new();
    let mut deletable: Vec<_> = backups
        .into_iter()
        .filter(|b| !games.insert(b.1.clone()))
        .collect();
    let mut deleted = Vec::new();
    while total > cap
        && let Some((_, _, name, size)) = deletable.pop()
    {
        let path = dir.join(name);
        std::fs::remove_file(&path).map_err(|e| err(&path, e))?;
        total -= size;
        deleted.push(path);
    }
    Ok(deleted)
}

fn is_stamp(t: &str) -> bool {
    t.len() == 15
        && t.bytes().enumerate().all(|(i, b)| {
            if i == 8 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
}

/// The local time now as YYYYMMDD-HHMMSS.
pub fn stamp_now() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as libc::time_t;
    // SAFETY: localtime_r (localtime_s on Windows) only writes the tm it
    // is given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    #[cfg(not(windows))]
    let failed = unsafe { libc::localtime_r(&t, &mut tm) }.is_null();
    #[cfg(windows)]
    let failed = unsafe { libc::localtime_s(&mut tm, &t) } != 0;
    if failed {
        return "00000000-000000".into();
    }
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}
