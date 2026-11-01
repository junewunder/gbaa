//! `--script`: mGBA Lua scripts ([`script`]) in the run loop.

use crate::{Code, Desktop, DesktopMachine, slots, write_png};
use runtime::Machine;
use script::Access;
use std::path::{Path, PathBuf};

pub type Host = script::Host<Desktop>;

/// A script host for the game in `rom`, whose `emu:runFrame` runs frames
/// as the run loop does (`code`, compiled, unless absent), and whose
/// `emu:saveStateSlot` and `loadStateSlot` use the slot files at `states`
/// ([`slots::path`]): the window's, in the game's folder, or headless,
/// beside the ROM.
pub fn host(
    rom: &'static [u8],
    states: &Path,
    code: Option<&'static Code>,
    interp_blocks: bool,
) -> Result<Host, String> {
    let mut stats = Box::new(runtime::dispatch::Stats::default());
    let save_to = states.to_path_buf();
    let load_from = states.to_path_buf();
    let hooks = script::Hooks {
        run_frame: Box::new(move |m: &mut DesktopMachine| {
            // A guest suspended at the frame end (src/coro.rs) can't run
            // under this one: detach it, and the frame loop drops it
            // after the callback (`script_moved`).
            m.nest.forget_run();
            match code {
                Some(c) if interp_blocks => {
                    runtime::dispatch::run_blocks(m, c, None, &mut stats);
                }
                Some(c) => runtime::dispatch::run_frame(m, c, &mut stats),
                None => m.run_frame(),
            }
        }),
        // Power-on, keeping the front end and the cartridge save.
        reset: Box::new(|m: &mut DesktopMachine| {
            let platform = std::mem::replace(&mut m.platform, Desktop::headless());
            let mut fresh = Machine::new(m.bus.rom, platform);
            fresh.set_timing(m.timing);
            fresh.spin_list = m.spin_list;
            std::mem::swap(&mut fresh.bus.save, &mut m.bus.save);
            *m = fresh;
        }),
        load_state: Box::new(slots::load_bytes),
        screenshot: Box::new(|m: &DesktopMachine, p: &std::path::Path| {
            write_png(p, &m.ppu.frame).map_err(|e| e.to_string())
        }),
        save_slot: Box::new(move |m: &mut DesktopMachine, n| {
            slots::save(m, &slots::path(&save_to, slot(n)?))
        }),
        load_slot: Box::new(move |m: &mut DesktopMachine, n| {
            slots::load(m, &slots::path(&load_from, slot(n)?))?;
            // The cartridge save is as the state had it: write it out.
            m.bus.save.dirty = true;
            Ok(())
        }),
    };
    let h = script::Host::new(rom, hooks)?;
    h.set_storage(storage_dir());
    Ok(h)
}

/// A script's slot number as the window's: mGBA takes any but a negative
/// one.
fn slot(n: i64) -> Result<u8, String> {
    u8::try_from(n).map_err(|_| format!("no state slot {n}"))
}

/// Where `storage`'s buckets are kept: `storage/` in
/// [`crate::gamepad::config_dir`], as mGBA keeps them in its config folder.
pub fn storage_dir() -> Option<PathBuf> {
    crate::gamepad::config_dir().map(|d| d.join("storage"))
}

/// What a headless run prints on exit: the console, then each text buffer.
pub fn report(h: &Host) -> String {
    let mut s = String::from("script console:\n");
    for l in h.console() {
        s += &format!("  {l}\n");
    }
    for (name, text) in h.buffers() {
        s += &format!("script buffer {name}:\n{text}");
        if !text.ends_with('\n') {
            s.push('\n');
        }
    }
    s
}

/// Where the full-access switch is remembered: `script-access.conf` in
/// [`crate::gamepad::config_dir`], beside `gamepad.conf`.
pub fn access_path() -> Option<PathBuf> {
    crate::gamepad::config_dir().map(|d| d.join("script-access.conf"))
}

/// The scripts `text` (a `script-access.conf`) gives full access: a line
/// `full = PATH` each, PATH as [`std::fs::canonicalize`] gives it, with `#`
/// comments.
pub fn full_scripts(text: &str) -> Result<Vec<PathBuf>, String> {
    let mut v = vec![];
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line.split_once('=') {
            Some((k, p)) if k.trim() == "full" => v.push(PathBuf::from(p.trim())),
            _ => return Err(format!("line {}: {line:?} isn't `full = PATH`", n + 1)),
        }
    }
    Ok(v)
}

/// What the script at `path` may do: everything if `asked` (`--script-full`),
/// which the file at `conf` then remembers, or if `conf` remembers it from
/// before; else sandboxed.
pub fn access(path: &Path, asked: bool, conf: Option<&Path>) -> Result<Access, String> {
    let at = |e: std::io::Error, p: &Path| format!("{}: {e}", p.display());
    let script = std::fs::canonicalize(path).map_err(|e| at(e, path))?;
    let Some(conf) = conf else {
        return Ok(if asked {
            Access::Full
        } else {
            Access::Sandboxed
        });
    };
    let text = match std::fs::read_to_string(conf) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        r => r.map_err(|e| at(e, conf))?,
    };
    let full = full_scripts(&text).map_err(|e| format!("{}: {e}", conf.display()))?;
    if full.contains(&script) {
        return Ok(Access::Full);
    }
    if !asked {
        return Ok(Access::Sandboxed);
    }
    if let Some(dir) = conf.parent() {
        std::fs::create_dir_all(dir).map_err(|e| at(e, dir))?;
    }
    let mut text = text;
    if text.is_empty() {
        text += "# Lua scripts given full access (--script-full); delete a line to take it back.\n";
    } else if !text.ends_with('\n') {
        text.push('\n');
    }
    text += &format!("full = {}\n", script.display());
    std::fs::write(conf, text).map_err(|e| at(e, conf))?;
    Ok(Access::Full)
}

/// Do what the window's console asked ([`crate::console::ScriptCmd`]). A
/// script loaded from the menu is sandboxed unless `script-access.conf`
/// gives it full access.
pub fn act(h: &Host, m: &mut DesktopMachine, c: crate::console::ScriptCmd) {
    use crate::console::ScriptCmd;
    match c {
        ScriptCmd::Load(path) => match access(&path, false, access_path().as_deref()) {
            Ok(a) => {
                h.load(m, &path, a);
            }
            Err(e) => eprintln!("gbaa-desktop: script {}: {e}", path.display()),
        },
        ScriptCmd::Unload(id) => h.unload(id),
        ScriptCmd::Eval(code) => h.eval(m, &code),
    }
}
