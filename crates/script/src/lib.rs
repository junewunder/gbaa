//! mGBA's Lua scripting, over our runtime (docs/research/lua-scripting.md).
//!
//! A [`Host`] owns a Lua 5.4 state with mGBA 0.10.5's globals: `emu`
//! (memory, `emu.memory` domains, ROM info, keys, registers, states,
//! `screenshot`, `step`, `runFrame`, `reset`), `callbacks`, `console` and
//! its text buffers, `socket` (mGBA's Lua over our `_socket`), `C`, `util`
//! and `system`; and 0.11-dev's `storage` ([`storage`], kept where
//! [`Host::set_storage`] says), `canvas` and `image` ([`canvas`]: overlays
//! the front end draws from [`Host::overlay`]), `input` ([`input`]: the
//! keyboard and mouse the front end hands [`Host::input`]) and
//! breakpoints and
//! watchpoints (`points`:
//! while one is set, frames run in the interpreter through
//! [`Host::run_frame_debug`]). The front end lends it the
//! machine at fixed points, between frames, never inside generated code:
//! [`Host::keys_read`] before each frame, [`Host::frame`] after it, and
//! [`Host::fire`] for `start`, `reset`, `stop`, `shutdown`, `crashed` and
//! `savedataUpdated` (`emu:reset` fires `reset` itself, as mGBA's does). A name mGBA has that we don't answers with a function
//! returning nil, and warns once on the console.
//!
//! A callback that runs past [`BUDGET`] Lua instructions (about a frame's
//! time) is stopped with a console error and removed, so a script can't
//! freeze the game.
//!
//! Each script runs in its own `_ENV`, as in mGBA, whose `require` finds
//! modules beside it. A script loaded [`Access::Sandboxed`] (the owner's
//! choice: limited, with a per-script full-access switch) may touch files
//! only under its own folder and run nothing outside the game; one loaded
//! [`Access::Full`] has every standard library, as in mGBA.
//!
//! The front end's console types Lua at [`Host::eval`], which sees what
//! the scripts defined, and [`Host::unload`] drops a script's callbacks
//! and buffers.

use mlua::chunk::ChunkMode;
use mlua::{Function, HookTriggers, Lua, Table, Value, VmState};
use runtime::{Machine, Platform};
use std::cell::{Cell, RefCell};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

/// A front-end action on the machine, for [`Hooks`].
pub type Action<P> = Box<dyn FnMut(&mut Machine<P>)>;
/// `emu:loadState*`'s: a state's bytes, ours or mGBA's.
pub type LoadState<P> = Box<dyn FnMut(&mut Machine<P>, Vec<u8>) -> Result<(), String>>;
/// `emu:screenshot`'s: the last frame as a PNG at the path.
pub type Screenshot<P> = Box<dyn FnMut(&Machine<P>, &Path) -> Result<(), String>>;
/// `emu:saveStateSlot`'s and `loadStateSlot`'s: the front end's slot `n`.
pub type Slot<P> = Box<dyn FnMut(&mut Machine<P>, i64) -> Result<(), String>>;

/// How the front end runs, resets, loads and shows the machine, for the
/// parts of `emu` that need more than the machine.
pub struct Hooks<P: Platform> {
    /// `emu:runFrame`: one frame, compiled or interpreted.
    pub run_frame: Action<P>,
    /// `emu:reset`: the machine as at power-on, keeping the cartridge save.
    pub reset: Action<P>,
    /// `emu:loadState*`.
    pub load_state: LoadState<P>,
    /// `emu:screenshot`.
    pub screenshot: Screenshot<P>,
    /// `emu:saveStateSlot`: the machine's state into the slot.
    pub save_slot: Slot<P>,
    /// `emu:loadStateSlot`: the slot's state, save data and all.
    pub load_slot: Slot<P>,
}

/// The Lua instructions a callback may run, in hook ticks of [`TICK`]:
/// about 5 million, about 12 ms on quack, under a 16.7 ms frame.
pub const BUDGET: u64 = 5_000;
/// A script's top level (its loading) may run this many callbacks' worth.
const LOAD_BUDGET: u64 = 100 * BUDGET;
/// Prime, so the hook doesn't land at the same place in a loop each time:
/// once the budget is spent it errors at every tick, and a loop whose
/// length divided the tick could catch every one in a `pcall` and spin on.
const TICK: u32 = 997;

struct Shared<P: Platform> {
    /// The machine, while it is lent; null otherwise.
    m: Cell<*mut Machine<P>>,
    hooks: RefCell<Hooks<P>>,
    /// What `getKeys` answers, and whether a script set them since the
    /// last [`Host::keys_read`].
    keys: Cell<u16>,
    keys_set: Cell<bool>,
    frame: Cell<u64>,
    /// Hook ticks left to the running callback; `u64::MAX` when none is.
    budget: Cell<u64>,
    console: RefCell<Vec<String>>,
    /// The sandboxed scripts' folders (canonical), by id: what `image`
    /// and painters may read and write ([`canvas::Ctx`]).
    roots: RefCell<std::collections::HashMap<u32, PathBuf>>,
    /// Console lines already handed out by [`Host::take_console`].
    taken: Cell<usize>,
    points: RefCell<points::Points>,
}

/// The lent machine.
#[allow(clippy::mut_from_ref)]
fn machine<P: Platform>(s: &Shared<P>) -> mlua::Result<&mut Machine<P>> {
    let p = s.m.get();
    if p.is_null() {
        return Err(mlua::Error::runtime("no game is running"));
    }
    // SAFETY: `p` is set only by `Host::lend`, from a `&mut Machine` that
    // outlives the call, and each API function takes it for its own body
    // only: none calls back into Lua while holding it.
    Ok(unsafe { &mut *p })
}

/// mGBA's GBA memory domains (`src/gba/core.c` at 0.10.5): name, base, size.
const DOMAINS: &[(&str, u32, u32)] = &[
    ("bios", 0x0000_0000, 0x4000),
    ("wram", 0x0200_0000, 0x4_0000),
    ("iwram", 0x0300_0000, 0x8000),
    ("io", 0x0400_0000, 0x400),
    ("palette", 0x0500_0000, 0x400),
    ("vram", 0x0600_0000, 0x1_8000),
    ("oam", 0x0700_0000, 0x400),
    ("cart0", 0x0800_0000, 0x200_0000),
    ("cart1", 0x0A00_0000, 0x200_0000),
    ("cart2", 0x0C00_0000, 0x200_0000),
    ("sram", 0x0E00_0000, 0x1_0000),
];

/// The register names mGBA's `readRegister` takes for the ARM core.
fn register(name: &str) -> Option<usize> {
    match name {
        "sp" => Some(13),
        "lr" => Some(14),
        "pc" => Some(15),
        _ => name.strip_prefix('r')?.parse().ok().filter(|&r| r < 16),
    }
}

/// What a script may do (docs/research/lua-scripting.md, Sandboxing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Files (`io.open`, `io.lines`, `io.input`/`output`, `os.remove`,
    /// `os.rename`, `loadfile`, `dofile`, `require`, `emu:saveStateFile`,
    /// `loadStateFile`, `screenshot`) only under the script's own folder,
    /// links resolved; Lua source only, no bytecode or C modules; no
    /// `os.execute`, `io.popen`, `os.exit`, `os.getenv`, `os.setlocale`,
    /// `os.tmpname` or `package.loadlib`; of `debug`, only `traceback` and
    /// `getinfo` (without `func`), so the hang guard's hook stays put;
    /// sockets on loopback only (`socket`'s module docs). A blocked call
    /// raises an error saying so.
    Sandboxed,
    /// Everything, as in mGBA.
    Full,
}

/// `p` as the OS would find it, through its links (`..` after a link
/// leaves the link's target), relative to the working directory. The parts
/// that don't exist yet are taken as written; `None` for a dangling link.
fn resolve(p: &Path) -> Option<PathBuf> {
    let p = std::env::current_dir().ok()?.join(p);
    let mut cur = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => cur.push(c),
            Component::CurDir => {}
            Component::ParentDir => {
                cur.pop();
            }
            Component::Normal(n) => {
                let next = cur.join(n);
                cur = match std::fs::canonicalize(&next) {
                    Ok(c) => c,
                    Err(_) if std::fs::symlink_metadata(&next).is_err() => next,
                    Err(_) => return None,
                };
            }
        }
    }
    Some(cur)
}

/// Whether a sandboxed script whose folder is `root` (canonical) may use
/// the file at `p`.
fn inside(root: &Path, p: &Path) -> bool {
    resolve(p).is_some_and(|p| p.starts_with(root))
}

/// The script host: one Lua state, any number of scripts.
pub struct Host<P: Platform + 'static> {
    lua: Lua,
    s: Rc<Shared<P>>,
    /// The prelude's: a script's `_ENV`, given its folder, whether it has
    /// full access, the sandbox's path check and its id.
    env: Function,
    /// The scripts loaded and not unloaded, by id (from 1), in order.
    loaded: RefCell<Vec<(u32, PathBuf)>>,
    next: Cell<u32>,
    storage: Rc<storage::Storage>,
    canvas: Rc<canvas::Canvas>,
    input: Rc<input::Input>,
}

impl<P: Platform + 'static> Host<P> {
    /// A Lua state with mGBA's globals, for the game in `rom`.
    pub fn new(rom: &'static [u8], hooks: Hooks<P>) -> Result<Host<P>, String> {
        let s = Rc::new(Shared {
            m: Cell::new(std::ptr::null_mut()),
            hooks: RefCell::new(hooks),
            keys: Cell::new(0),
            keys_set: Cell::new(false),
            frame: Cell::new(0),
            budget: Cell::new(u64::MAX),
            console: RefCell::default(),
            roots: RefCell::default(),
            taken: Cell::new(0),
            points: RefCell::default(),
        });
        // As mGBA: every standard library, `debug` included (luaL_openlibs);
        // a sandboxed script's `_ENV` holds limited copies.
        let lua = unsafe { Lua::unsafe_new() };
        let storage = Rc::new(storage::Storage::default());
        lua.globals()
            .set("storage", storage::StorageRef(storage.clone()))
            .map_err(|e| e.to_string())?;
        let canvas = Rc::new(canvas::Canvas::default());
        let r = s.clone();
        let ctx = canvas::Ctx {
            allowed: Box::new(move |lua, p| {
                let id: u32 = lua.globals().get("__script").unwrap_or(0);
                r.roots.borrow().get(&id).is_none_or(|root| inside(root, p))
            }),
        };
        canvas::install(&lua, canvas.clone(), ctx).map_err(|e| e.to_string())?;
        let input = Rc::new(input::Input::default());
        input::install(&lua, input.clone()).map_err(|e| e.to_string())?;
        let env = install(&lua, &s, rom).map_err(|e| e.to_string())?;
        let b = s.clone();
        lua.set_global_hook(
            HookTriggers::new().every_nth_instruction(TICK),
            move |_, _| match b.budget.get() {
                0 => Err(mlua::Error::runtime(
                    "stopped: ran past its time budget (about a frame)",
                )),
                u64::MAX => Ok(VmState::Continue),
                n => {
                    b.budget.set(n - 1);
                    Ok(VmState::Continue)
                }
            },
        )
        .map_err(|e| e.to_string())?;
        Ok(Host {
            lua,
            s,
            env,
            loaded: RefCell::default(),
            next: Cell::new(1),
            storage,
            canvas,
            input,
        })
    }

    /// Keep `storage`'s buckets in `dir`, as `<name>.json` (mGBA's: its
    /// config folder's `storage/`); with none (the default), only in
    /// memory.
    pub fn set_storage(&self, dir: Option<PathBuf>) {
        self.storage.set_dir(dir);
    }

    /// Write the buckets as mGBA does at its end: those that autoflush and
    /// any set since its last flush. Dropping the host does it too.
    pub fn flush_storage(&self) {
        self.storage.shutdown();
    }

    /// Run `m` lent to the scripts for `f`.
    fn lend<T>(&self, m: &mut Machine<P>, f: impl FnOnce() -> T) -> T {
        let was = self.s.m.replace(m);
        let t = f();
        self.s.m.set(was);
        t
    }

    fn log(&self, line: String) {
        self.s.console.borrow_mut().push(line);
    }

    /// Load and run the script at `path` in its own `_ENV`, as mGBA does:
    /// `require` finds modules beside it. `access` says what it may do. An
    /// error goes to the console, as in mGBA; what it set up before the
    /// error stays, until [`Host::unload`]. Returns its id.
    pub fn load(&self, m: &mut Machine<P>, path: &Path, access: Access) -> u32 {
        let id = self.next.get();
        self.next.set(id + 1);
        self.loaded.borrow_mut().push((id, path.to_owned()));
        let r = self.lend(m, || -> mlua::Result<()> {
            let dir = path.parent().unwrap_or(Path::new("."));
            let dir = if dir.as_os_str().is_empty() {
                Path::new(".")
            } else {
                dir
            };
            let root = std::fs::canonicalize(dir).map_err(mlua::Error::external)?;
            let full = access == Access::Full;
            if !full {
                self.s.roots.borrow_mut().insert(id, root.clone());
            }
            let allowed = self
                .lua
                .create_function(move |_, p: String| Ok(inside(&root, Path::new(&p))))?;
            let env: Table = self.env.call((
                dir.display().to_string(),
                path.display().to_string(),
                full,
                allowed,
                id,
            ))?;
            let src = std::fs::read(path).map_err(mlua::Error::external)?;
            self.s.budget.set(LOAD_BUDGET);
            let chunk = self.lua.load(src).set_name(format!("@{}", path.display()));
            // Bytecode can break the VM's safety: sandboxed, source only.
            let chunk = if full {
                chunk
            } else {
                chunk.set_mode(ChunkMode::Text)
            };
            // As a coroutine, so a BizHawk script's `emu.frameadvance`
            // loop runs a step a frame (src/bizhawk.lua).
            let run: Function = self.lua.globals().get("__run")?;
            let r = self.as_script(id, || run.call(chunk.set_environment(env).into_function()?));
            self.s.budget.set(u64::MAX);
            r
        });
        if let Err(e) = r {
            self.log(format!("error: {}: {e}", path.display()));
        }
        id
    }

    /// Run `f` as script `id` (0 the prompt): the callbacks and buffers it
    /// makes are that script's.
    fn as_script<T>(&self, id: u32, f: impl FnOnce() -> mlua::Result<T>) -> mlua::Result<T> {
        let g = self.lua.globals();
        let was: Value = g.get("__script")?;
        g.set("__script", id)?;
        let r = f();
        g.set("__script", was)?;
        r
    }

    /// The scripts loaded, by id, in the order they were.
    pub fn scripts(&self) -> Vec<(u32, PathBuf)> {
        self.loaded.borrow().clone()
    }

    /// Unload script `id`: drop its callbacks, its text buffers and its
    /// canvas layers, and its names from the prompt. Nothing fires (mGBA 0.10 can only reset the
    /// whole scripting context).
    pub fn unload(&self, id: u32) {
        let Some(i) = self.loaded.borrow().iter().position(|(n, _)| *n == id) else {
            return;
        };
        let (_, path) = self.loaded.borrow_mut().remove(i);
        self.canvas.unload(id);
        self.s.roots.borrow_mut().remove(&id);
        let r = self
            .lua
            .globals()
            .get::<Function>("__unload")
            .and_then(|f| f.call::<()>(id));
        match r {
            Ok(()) => self.log(format!("unloaded {}", path.display())),
            Err(e) => self.log(format!("error: unloading {}: {e}", path.display())),
        }
    }

    /// Run `code` typed at the console's prompt, as mGBA's scripting
    /// window does, where a script's globals are seen (it runs in no
    /// script's `_ENV` but finds their names: newest script first). The
    /// command is echoed (`> code`); an expression's values are logged.
    /// With full access: it's the player's own typing.
    pub fn eval(&self, m: &mut Machine<P>, code: &str) {
        self.log(format!("> {code}"));
        let r = self.lend(m, || -> mlua::Result<()> {
            let env: Table = self.lua.globals().get("__prompt")?;
            let chunk = |src: String| {
                self.lua
                    .load(src)
                    .set_name("=prompt")
                    .set_environment(env.clone())
                    .into_function()
            };
            // An expression first, so `x` shows x; else a statement.
            let f = chunk(format!("return {code}")).or_else(|_| chunk(code.to_owned()))?;
            self.s.budget.set(LOAD_BUDGET);
            let r = self.as_script(0, || f.call::<mlua::MultiValue>(()));
            self.s.budget.set(u64::MAX);
            let vals = r?;
            if !vals.is_empty() {
                let tostring: Function = self.lua.globals().get("tostring")?;
                let parts = vals
                    .into_iter()
                    .map(|v| tostring.call::<String>(v))
                    .collect::<mlua::Result<Vec<_>>>()?;
                self.log(format!("log: {}", parts.join("\t")));
            }
            Ok(())
        });
        if let Err(e) = r {
            self.log(format!("error: {e}"));
        }
    }

    /// Fire the callbacks added for `name`, in the order they were added.
    pub fn fire(&self, m: &mut Machine<P>, name: &str) {
        self.lend(m, || fire(&self.lua, &self.s, name, None));
    }

    /// Hand the scripts what the front end saw (between frames, as mGBA's
    /// Qt front end does from its window's events): update `input`, and
    /// fire the `key`, `mouseButton`, `mouseMove` or `mouseWheel`
    /// callbacks with the event.
    pub fn input(&self, m: &mut Machine<P>, ev: input::Event) {
        let events = match self.input.events(&self.lua, ev) {
            Ok(e) => e,
            Err(e) => return self.log(format!("error: input: {e}")),
        };
        for (name, t) in events {
            self.lend(m, || fire(&self.lua, &self.s, name, Some(t)));
        }
    }

    /// Before the frame's pad poll: fire `keysRead` with `pad` as the keys
    /// (the last poll's), and return the keys a script set, if one did,
    /// to press instead.
    pub fn keys_read(&self, m: &mut Machine<P>, pad: u16) -> Option<u16> {
        if !self.s.keys_set.get() {
            self.s.keys.set(pad);
        }
        self.fire(m, "keysRead");
        self.s.keys_set.take().then(|| self.s.keys.get())
    }

    /// After frame `n` (from 1): update the canvas, as mGBA's own `frame`
    /// callback does before the scripts', then fire `frame`. Every [`storage::FLUSH_EVERY`] frames, flush the autoflushing
    /// buckets that changed.
    pub fn frame(&self, m: &mut Machine<P>, n: u64) {
        self.s.frame.set(n);
        self.canvas.update();
        self.fire(m, "frame");
        if n.is_multiple_of(storage::FLUSH_EVERY) {
            self.storage.flush_changed();
        }
    }

    /// The scripts' canvas layers to draw over the game picture and
    /// beside it ([`canvas::Overlay`]), transparent where none is; `None`
    /// while there are none.
    pub fn overlay(&self) -> Option<canvas::Overlay> {
        self.canvas.compose()
    }

    /// Every console line so far: `log: …`, `warn: …`, `error: …`.
    pub fn console(&self) -> Vec<String> {
        self.s.console.borrow().clone()
    }

    /// The console lines since the last call.
    pub fn take_console(&self) -> Vec<String> {
        let c = self.s.console.borrow();
        let new = c[self.s.taken.get().min(c.len())..].to_vec();
        self.s.taken.set(c.len());
        new
    }

    /// The text buffers, by name, with what they hold.
    pub fn buffers(&self) -> Vec<(String, String)> {
        let Ok(t) = self.lua.globals().get::<Table>("__buffers") else {
            return vec![];
        };
        t.sequence_values::<Table>()
            .flatten()
            .map(|b| {
                (
                    b.get("name").unwrap_or_default(),
                    b.get("text").unwrap_or_default(),
                )
            })
            .collect()
    }
}

impl<P: Platform + 'static> Drop for Host<P> {
    fn drop(&mut self) {
        self.storage.shutdown();
    }
}

/// Fire `name`'s callbacks, each given `arg` if there is one.
fn fire<P: Platform>(lua: &Lua, s: &Shared<P>, name: &str, arg: Option<Table>) {
    let Ok(cbs) = lua.globals().get::<Table>("__callbacks") else {
        return;
    };
    let mut due: Vec<(i64, Value, Function)> = cbs
        .pairs::<i64, Table>()
        .flatten()
        .filter(|(_, cb)| cb.get::<String>("name").is_ok_and(|n| n == name))
        .filter_map(|(id, cb)| Some((id, cb.get("script").ok()?, cb.get("fn").ok()?)))
        .collect();
    due.sort_by_key(|(id, _, _)| *id);
    // A callback may fire others (`emu:reset` fires `reset`): each gets
    // its own budget, and the outer one's carries on after.
    let outer = s.budget.get();
    for (id, script, f) in due {
        s.budget.set(BUDGET);
        let g = lua.globals();
        let was: Value = g.get("__script").unwrap_or(Value::Nil);
        let _ = g.set("__script", script);
        let r = f.call::<()>(arg.clone());
        let _ = g.set("__script", was);
        let overran = s.budget.get() == 0;
        s.budget.set(outer);
        if let Err(e) = r {
            let mut c = s.console.borrow_mut();
            c.push(format!("error: {name} callback: {e}"));
            if overran {
                c.push(format!("error: {name} callback removed"));
                let _ = cbs.set(id, Value::Nil);
            }
        }
    }
}

/// `read8` … `readRange`, `write8` … `write32` on `t`, at `base`.
fn memory_methods<P: Platform + 'static>(
    lua: &Lua,
    s: &Rc<Shared<P>>,
    t: &Table,
    base: u32,
) -> mlua::Result<()> {
    macro_rules! read {
        ($name:literal, $f:ident) => {{
            let s = s.clone();
            t.set(
                $name,
                lua.create_function(move |_, (_s, a): (Value, u32)| {
                    Ok(machine(&s)?.bus.$f(base.wrapping_add(a)))
                })?,
            )?;
        }};
    }
    macro_rules! write {
        ($name:literal, $f:ident, $ty:ty) => {{
            let s = s.clone();
            t.set(
                $name,
                lua.create_function(move |_, (_s, a, v): (Value, u32, i64)| {
                    machine(&s)?.bus.$f(base.wrapping_add(a), v as $ty);
                    Ok(())
                })?,
            )?;
        }};
    }
    read!("read8", read8);
    read!("read16", read16);
    read!("read32", read32);
    write!("write8", write8, u8);
    write!("write16", write16, u16);
    write!("write32", write32, u32);
    let s = s.clone();
    t.set(
        "readRange",
        lua.create_function(move |lua, (_s, a, n): (Value, u32, u32)| {
            let m = machine(&s)?;
            let bytes: Vec<u8> = (0..n)
                .map(|i| m.bus.read8(base.wrapping_add(a).wrapping_add(i)))
                .collect();
            lua.create_string(&bytes)
        })?,
    )?;
    Ok(())
}

fn save_state<P: Platform>(m: &mut Machine<P>) -> Vec<u8> {
    let mut v = Vec::new();
    m.save_state(&mut |b| v.extend_from_slice(b));
    v
}

fn install<P: Platform + 'static>(
    lua: &Lua,
    s: &Rc<Shared<P>>,
    rom: &'static [u8],
) -> mlua::Result<Function> {
    let g = lua.globals();
    let c = s.clone();
    g.set(
        "__console",
        lua.create_function(move |_, (kind, msg): (String, String)| {
            c.console.borrow_mut().push(format!("{kind}: {msg}"));
            Ok(())
        })?,
    )?;

    let emu = lua.create_table()?;
    memory_methods(lua, s, &emu, 0)?;
    let memory = lua.create_table()?;
    for &(name, base, size) in DOMAINS {
        let d = lua.create_table()?;
        memory_methods(lua, s, &d, base)?;
        d.set("base", lua.create_function(move |_, _: Value| Ok(base))?)?;
        d.set(
            "bound",
            lua.create_function(move |_, _: Value| Ok(base + size))?,
        )?;
        d.set("size", lua.create_function(move |_, _: Value| Ok(size))?)?;
        d.set("name", lua.create_function(move |_, _: Value| Ok(name))?)?;
        memory.set(name, d)?;
    }
    emu.set("memory", memory)?;

    let header = move |a: usize, b: usize| {
        let h = rom.get(a..b).unwrap_or_default();
        let end = h.iter().position(|&c| c == 0).unwrap_or(h.len());
        String::from_utf8_lossy(&h[..end]).into_owned()
    };
    emu.set(
        "getGameTitle",
        lua.create_function(move |_, _: Value| Ok(header(0xA0, 0xAC)))?,
    )?;
    // mGBA 0.10's format (`src/gba/gba.c:778` at 0.10.5): "AGB-BPEE".
    emu.set(
        "getGameCode",
        lua.create_function(move |_, _: Value| Ok(format!("AGB-{}", header(0xAC, 0xB0))))?,
    )?;
    emu.set(
        "romSize",
        lua.create_function(move |_, _: Value| Ok(rom.len()))?,
    )?;
    emu.set("platform", lua.create_function(|_, _: Value| Ok(0))?)?;
    emu.set(
        "frameCycles",
        lua.create_function(|_, _: Value| Ok(runtime::sched::FRAME_CYCLES))?,
    )?;
    emu.set(
        "frequency",
        lua.create_function(|_, _: Value| Ok(16_777_216))?,
    )?;
    let f = s.clone();
    emu.set(
        "currentFrame",
        lua.create_function(move |_, _: Value| Ok(f.frame.get()))?,
    )?;
    emu.set(
        "checksum",
        lua.create_function(move |lua, (_s, t): (Value, Option<i64>)| {
            // C.CHECKSUM.CRC32, as four big-endian bytes, as mGBA gives it.
            if t.unwrap_or(0) != 0 {
                return Ok(Value::Nil);
            }
            let crc = runtime::state::crc32(rom);
            Ok(Value::String(lua.create_string(crc.to_be_bytes())?))
        })?,
    )?;

    macro_rules! keyfn {
        ($name:literal, |$k:ident, $v:ident| $body:expr) => {{
            let $k = s.clone();
            emu.set(
                $name,
                lua.create_function(move |_, (_s, $v): (Value, Option<u32>)| {
                    let $v = $v.unwrap_or(0);
                    Ok($body)
                })?,
            )?;
        }};
    }
    fn set<P: Platform>(s: &Shared<P>, k: u16) {
        s.keys.set(k & runtime::io::key::ALL);
        s.keys_set.set(true);
    }
    keyfn!("getKeys", |k, _v| k.keys.get() as u32);
    keyfn!("setKeys", |k, v| set(&k, v as u16));
    keyfn!("addKeys", |k, v| set(&k, k.keys.get() | v as u16));
    keyfn!("clearKeys", |k, v| set(&k, k.keys.get() & !v as u16));
    keyfn!("addKey", |k, v| set(&k, k.keys.get() | 1 << (v & 15)));
    keyfn!("clearKey", |k, v| set(&k, k.keys.get() & !(1 << (v & 15))));
    keyfn!("getKey", |k, v| (k.keys.get() >> (v & 15)) as u32 & 1);

    let ss = s.clone();
    emu.set(
        "readRegister",
        lua.create_function(move |_, (_s, n): (Value, String)| {
            let m = machine(&ss)?;
            Ok(match n.as_str() {
                "cpsr" => Some(m.cpu.cpsr),
                _ => register(&n).map(|r| if r == 15 { m.cpu.pc } else { m.cpu.r[r] }),
            })
        })?,
    )?;
    let ss = s.clone();
    emu.set(
        "writeRegister",
        lua.create_function(move |_, (_s, n, v): (Value, String, u32)| {
            let m = machine(&ss)?;
            match (n.as_str(), register(&n)) {
                ("cpsr", _) => m.cpu.cpsr = v,
                (_, Some(15)) => m.cpu.pc = v,
                (_, Some(r)) => m.cpu.r[r] = v,
                _ => {}
            }
            Ok(())
        })?,
    )?;
    let ss = s.clone();
    emu.set(
        "saveStateBuffer",
        lua.create_function(move |lua, _: mlua::MultiValue| {
            lua.create_string(save_state(machine(&ss)?))
        })?,
    )?;
    let ss = s.clone();
    emu.set(
        "loadStateBuffer",
        lua.create_function(move |_, (_s, b): (Value, mlua::LuaString)| {
            let m = machine(&ss)?;
            Ok((ss.hooks.borrow_mut().load_state)(m, b.as_bytes().to_vec()).is_ok())
        })?,
    )?;
    let ss = s.clone();
    emu.set(
        "saveStateFile",
        lua.create_function(move |_, (_s, p): (Value, String)| {
            Ok(std::fs::write(p, save_state(machine(&ss)?)).is_ok())
        })?,
    )?;
    let ss = s.clone();
    emu.set(
        "loadStateFile",
        lua.create_function(move |_, (_s, p): (Value, String)| {
            let m = machine(&ss)?;
            let Ok(b) = std::fs::read(p) else {
                return Ok(false);
            };
            Ok((ss.hooks.borrow_mut().load_state)(m, b).is_ok())
        })?,
    )?;
    let ss = s.clone();
    emu.set(
        "screenshot",
        lua.create_function(move |_, (_s, p): (Value, String)| {
            let m = machine(&ss)?;
            Ok((ss.hooks.borrow_mut().screenshot)(m, Path::new(&p)).is_ok())
        })?,
    )?;
    let ss = s.clone();
    emu.set(
        "saveStateSlot",
        lua.create_function(move |_, (_s, n): (Value, i64)| {
            let m = machine(&ss)?;
            Ok((ss.hooks.borrow_mut().save_slot)(m, n).is_ok())
        })?,
    )?;
    // mGBA's flags default to `C.SAVESTATE.ALL` less `SAVEDATA` here
    // (`src/core/scripting.c:574` at 0.10.5): without it the cartridge save
    // stays as it was.
    let ss = s.clone();
    emu.set(
        "loadStateSlot",
        lua.create_function(move |_, (_s, n, flags): (Value, i64, Option<i64>)| {
            let m = machine(&ss)?;
            let keep = flags.unwrap_or(31 & !2) & 2 == 0;
            let save = if keep { Some(m.bus.save.data) } else { None };
            let ok = (ss.hooks.borrow_mut().load_slot)(m, n).is_ok();
            if let (true, Some(data)) = (ok, save) {
                m.bus.save.data = data;
            }
            Ok(ok)
        })?,
    )?;
    // From a `frame` callback this runs a frame without firing `frame`
    // again (mGBA warns against it too).
    let ss = s.clone();
    emu.set(
        "runFrame",
        lua.create_function(move |_, _: Value| {
            let m = machine(&ss)?;
            (ss.hooks.borrow_mut().run_frame)(m);
            Ok(())
        })?,
    )?;
    let ss = s.clone();
    emu.set(
        "step",
        lua.create_function(move |_, _: Value| {
            machine(&ss)?.step();
            Ok(())
        })?,
    )?;
    let ss = s.clone();
    emu.set(
        "reset",
        lua.create_function(move |lua, _: Value| {
            (ss.hooks.borrow_mut().reset)(machine(&ss)?);
            fire(lua, &ss, "reset", None);
            Ok(())
        })?,
    )?;
    g.set("emu", emu)?;

    let sock = lua.create_table()?;
    sock.set(
        "create",
        lua.create_function(|_, ()| Ok(socket::Socket::new(true)))?,
    )?;
    g.set("_socket", sock)?;
    g.set(
        "__loopbackSocket",
        lua.create_function(|_, ()| Ok(socket::Socket::new(false)))?,
    )?;
    let sockerr = lua.create_table()?;
    for &(name, code) in socket::SOCKERR {
        sockerr.set(name, code)?;
    }
    g.set("__SOCKERR", sockerr)?;
    let align = lua.create_table()?;
    for &(name, v) in canvas::ALIGN {
        align.set(name, v)?;
    }
    g.set("__ALIGN", align)?;
    points::install(lua, s)?;
    lua.load(points::PRELUDE).set_name("=prelude").exec()?;
    let env = lua.load(PRELUDE).set_name("=prelude").eval()?;
    lua.load(BIZHAWK).set_name("=bizhawk").exec()?;
    Ok(env)
}

/// BizHawk's drawing, memory and frame calls over the host's
/// (`src/bizhawk.lua`).
const BIZHAWK: &str = include_str!("bizhawk.lua");

/// What is plain bookkeeping in mGBA, written in Lua: the callback list,
/// the console and its text buffers, the constants, and the names we don't
/// have (a function returning nil, warned of once). It returns the maker of
/// each script's `_ENV` ([`Access`]).
const PRELUDE: &str = r##"
local warned = {}
local function stubbed(t, path)
  return setmetatable(t, {__index = function(_, k)
    local name = path .. "." .. tostring(k)
    if not warned[name] then
      warned[name] = true
      __console("warn", name .. " is not supported by gbaa")
    end
    return function() return nil end
  end})
end
__stubbed = stubbed
stubbed(emu, "emu")
for name, d in pairs(emu.memory) do stubbed(d, "emu.memory." .. name) end

-- The script running now (0 the console's prompt), whose callbacks and
-- buffers go when it's unloaded.
__script = 0
__callbacks = {}
local nextId = 1
callbacks = stubbed({
  add = function(_, name, fn)
    local id = nextId; nextId = nextId + 1
    __callbacks[id] = {name = name, fn = fn, script = __script}
    return id
  end,
  remove = function(_, id) __callbacks[id] = nil end,
}, "callbacks")

-- BizHawk's `console.log(msg)` too, called with a dot.
local function out(kind) return function(s, ...)
  local msg = ...
  if s ~= console then msg = s end
  __console(kind, tostring(msg))
end end
console = stubbed({log = out("log"), warn = out("warn"), error = out("error")}, "console")
__buffers = {}
function console:createBuffer(name)
  local b = {name = name or "", text = "", script = __script}
  function b:print(s) self.text = self.text .. tostring(s) end
  function b:clear() self.text = "" end
  function b:setName(n) self.name = n end
  function b:setSize() end
  function b:moveCursor() end
  function b:advance() end
  function b:getX() return 0 end
  function b:getY() return 0 end
  function b:cols() return 80 end
  function b:rows() return 24 end
  table.insert(__buffers, b)
  return stubbed(b, "buffer")
end
print = function(...)
  local t = {}
  for i = 1, select("#", ...) do t[i] = tostring((select(i, ...))) end
  __console("log", table.concat(t, "\t"))
end

C = stubbed({
  PLATFORM = {NONE = -1, GBA = 0, GB = 1},
  CHECKSUM = {CRC32 = 0, MD5 = 1, SHA1 = 2},
  SAVESTATE = {SCREENSHOT = 1, SAVEDATA = 2, CHEATS = 4, RTC = 8, METADATA = 16, ALL = 31},
  GBA_KEY = {A = 0, B = 1, SELECT = 2, START = 3, RIGHT = 4, LEFT = 5, UP = 6, DOWN = 7, R = 8, L = 9},
  SOCKERR = __SOCKERR,
  WATCHPOINT_TYPE = {WRITE = 1, READ = 2, RW = 3, WRITE_CHANGE = 5},
  ALIGN = __ALIGN,
}, "C")
-- mGBA 0.11-dev's `input` constants (src/input.rs).
for k, v in pairs(__INPUT_C) do rawset(C, k, v) end
util = stubbed({
  makeBitmask = function(l) local m = 0 for _, b in ipairs(l) do m = m | (1 << b) end return m end,
  expandBitmask = function(m) local l = {} for b = 0, 31 do if m & (1 << b) ~= 0 then l[#l + 1] = b end end return l end,
}, "util")
system = stubbed({version = "0.10.5", program = "gbaa", branch = "main", commit = ""}, "system")

-- mGBA's socket API (`src/script/engines/lua.c:61-196` at 0.10.5), over the
-- native `_socket`; each socket is its script's, closed when it's unloaded.
local sockets = setmetatable({}, {__mode = "k"})
-- A `socket` table over `create`'s sockets, with method tables of its own:
-- a sandboxed script's, if they were shared, could swap a method and be
-- handed a full-access script's `_socket`.
local function makeSocket(create)
  local socket
  socket = {
    ERRORS = {},
    tcp = function() return socket._create(create(), socket._tcpMT) end,
    bind = function(address, port)
      local s = socket.tcp()
      local ok, err = s:bind(address, port)
      if ok then return s end
      return ok, err
    end,
    connect = function(address, port)
      local s = socket.tcp()
      local ok, err = s:connect(address, port)
      if ok then return s end
      return ok, err
    end,
    _create = function(sock, mt)
      local s = setmetatable({_s = sock, _callbacks = {}, _nextCallback = 1}, mt)
      sockets[s] = __script
      return s
    end,
    _wrap = function(status)
      if status == 0 then return 1 end
      return nil, socket.ERRORS[status] or ('error#' .. status)
    end,
    _mt = {
      __index = {
        close = function(self)
          if self._onframecb then
            callbacks:remove(self._onframecb)
            self._onframecb = nil
          end
          self._callbacks = {}
          return self._s:close()
        end,
        add = function(self, event, callback)
          if not self._callbacks[event] then self._callbacks[event] = {} end
          local cbid = self._nextCallback
          self._nextCallback = cbid + 1
          self._callbacks[event][cbid] = callback
          return cbid
        end,
        remove = function(self, cbid)
          for _, group in pairs(self._callbacks) do
            if group[cbid] then group[cbid] = nil end
          end
        end,
        _dispatch = function(self, event, ...)
          if not self._callbacks[event] then return end
          for k, cb in pairs(self._callbacks[event]) do
            if cb then
              local ok, ret = pcall(cb, self, ...)
              if not ok then console:error(ret) end
            end
          end
        end,
      },
    },
    _tcpMT = {
      __index = {
        _hook = function(self, status)
          if status == 0 then
            self._onframecb = callbacks:add('frame', function() self:poll() end)
          end
          return socket._wrap(status)
        end,
        bind = function(self, address, port)
          return socket._wrap(self._s:open(address or '', port))
        end,
        connect = function(self, address, port)
          local status = self._s:connect(address, port)
          return self:_hook(status)
        end,
        listen = function(self, backlog)
          local status = self._s:listen(backlog or 1)
          return self:_hook(status)
        end,
        accept = function(self)
          local client = self._s:accept()
          if client.error ~= 0 then
            client:close()
            return socket._wrap(client.error)
          end
          local sock = socket._create(client, socket._tcpMT)
          sock:_hook(0)
          return sock
        end,
        send = function(self, data, i, j)
          local result = self._s:send(string.sub(data, i or 1, j))
          if result < 0 then return socket._wrap(self._s.error) end
          if i then return result + i - 1 end
          return result
        end,
        receive = function(self, maxBytes)
          local result = self._s:recv(maxBytes)
          if (not result or #result == 0) and self._s.error ~= 0 then
            return socket._wrap(self._s.error)
          elseif not result or #result == 0 then
            return nil, 'disconnected'
          end
          return result or ''
        end,
        hasdata = function(self)
          local status = self._s:select(0)
          if status < 0 then
            return socket._wrap(self._s.error)
          end
          return status > 0
        end,
        poll = function(self)
          local status, err = self:hasdata()
          if err then
            self:_dispatch('error', err)
          elseif status then
            self:_dispatch('received')
          end
        end,
      },
    },
    _errMT = {
      __index = function(tbl, key)
        return rawget(tbl, C.SOCKERR[key])
      end,
    },
  }
  setmetatable(socket._tcpMT.__index, socket._mt)
  setmetatable(socket.ERRORS, socket._errMT)
  -- `_mScriptSocketErrors` (`lua.c:191-209`).
  for code, msg in pairs({[-1] = "unknown error", "temporary failure", "address in use",
      "connection refused", "access denied", "failed", "network unreachable", "not found",
      "no data", "out of memory", "timeout", "unsupported"}) do
    socket.ERRORS[code] = msg
  end
  return socket
end
socket = makeSocket(_socket.create)

local G, rawload, rawloadfile, realRequire, rio, ros, rdebug = _G, load, loadfile, require, io, os, debug
local STD = {string = true, table = true, math = true, utf8 = true, coroutine = true,
  io = true, os = true, debug = true, package = true}
local function blocked(name)
  return function()
    error(name .. " is not allowed: the script is sandboxed (give it full access to use it)", 2)
  end
end
local function lib(src, name, keep, block)
  local t = {}
  for _, k in ipairs(keep) do t[k] = src[k] end
  for _, k in ipairs(block) do t[k] = blocked(name .. "." .. k) end
  return t
end

-- Each loaded script: its id, its `_ENV`, and what that held before it
-- ran, newest last.
__scripts = {}
-- The prompt's `_ENV`: a name a script set (newest script first), else the
-- global one, so commands call what scripts define (Ironmon's are typed Lua
-- calls). What it sets stays its own.
__prompt = setmetatable({}, {__index = function(_, k)
  for i = #__scripts, 1, -1 do
    local s = __scripts[i]
    local v = rawget(s.env, k)
    if v ~= nil and v ~= s.base[k] then return v end
  end
  return G[k]
end})
__prompt._G = __prompt

-- Unload script `id`: its callbacks, points, buffers, sockets and names at
-- the prompt.
function __unload(id)
  __unloadPoints(id)
  for s, sid in pairs(sockets) do
    if sid == id then s:close() end
  end
  for k, cb in pairs(__callbacks) do
    if cb.script == id then __callbacks[k] = nil end
  end
  for i = #__buffers, 1, -1 do
    if __buffers[i].script == id then table.remove(__buffers, i) end
  end
  for i = #__scripts, 1, -1 do
    if __scripts[i].id == id then table.remove(__scripts, i) end
  end
end

return function(dir, path, full, allowed, id)
  local env
  -- A path the script may use, or an error at its call.
  local function checked(name, p)
    -- Lua's io and os functions turn a number into a filename, so check it as one.
    if type(p) == "number" then p = tostring(p) end
    if full or type(p) ~= "string" or allowed(p) then return p end
    error(name .. ": " .. p .. " is outside the script's folder (the script is sandboxed;"
      .. " give it full access to use it)", 3)
  end
  if full then
    env = setmetatable({}, {__index = G})
  else
    env = {}
    -- Not the host's own (`__prompt` would reach the full globals).
    for k, v in pairs(G) do
      if type(k) ~= "string" or k:sub(1, 2) ~= "__" then env[k] = v end
    end
    env.io = lib(rio, "io", {"close", "read", "write", "type", "tmpfile", "stdin", "stdout", "stderr"}, {"popen"})
    env.io.open = function(p, ...) return rio.open(checked("io.open", p), ...) end
    env.io.lines = function(p, ...) return rio.lines(checked("io.lines", p), ...) end
    env.io.input = function(p) return rio.input(checked("io.input", p)) end
    env.io.output = function(p) return rio.output(checked("io.output", p)) end
    env.os = lib(ros, "os", {"clock", "date", "difftime", "time"},
      {"execute", "exit", "getenv", "setlocale", "tmpname"})
    env.os.remove = function(p) return ros.remove(checked("os.remove", p)) end
    env.os.rename = function(a, b)
      return ros.rename(checked("os.rename", a), checked("os.rename", b))
    end
    env.debug = lib(rdebug, "debug", {"traceback"}, {"debug", "gethook", "getlocal",
      "getmetatable", "getregistry", "getupvalue", "getuservalue", "sethook", "setlocal",
      "setmetatable", "setupvalue", "setuservalue", "upvalueid", "upvaluejoin", "setcstacklimit"})
    env.debug.getinfo = function(...)
      local a = table.pack(...)
      -- A level counts from the caller, not this function.
      if math.type(a[1]) == "integer" then a[1] = a[1] + 1 end
      local t = rdebug.getinfo(table.unpack(a, 1, a.n))
      if t then t.func = nil end
      return t
    end
    -- Its files only, and their paths, through a proxy whose metatable
    -- hides the real `emu`.
    local e = setmetatable({}, {__index = G.emu, __metatable = false})
    for _, k in ipairs({"saveStateFile", "loadStateFile", "screenshot"}) do
      e[k] = function(self, p, ...) return G.emu[k](self, checked("emu:" .. k, p), ...) end
    end
    env.emu = e
    env.socket = makeSocket(__loopbackSocket)
    env._socket = {create = __loopbackSocket}
  end
  env._G = env
  -- mGBA 0.11-dev's (`src/script/engines/lua.c`): where the script is.
  env.script = {dir = dir, path = path}
  local loaded = {}
  if full then
    env.package = setmetatable({}, {__index = G.package})
  else
    env.package = lib(G.package, "package", {"config"}, {"loadlib", "searchpath"})
    env.package.cpath = ""
  end
  env.package.path = dir .. "/?.lua;" .. dir .. "/?/init.lua"
  env.package.loaded = loaded
  env.load = function(...)
    local chunk, name, mode, e = ...
    if select("#", ...) < 4 then e = env end
    return rawload(chunk, name, full and mode or "t", e)
  end
  env.loadfile = function(...)
    local p, mode, e = ...
    if select("#", ...) < 3 then e = env end
    if p == nil and not full then error("loadfile: the script is sandboxed: give a path", 2) end
    return rawloadfile(checked("loadfile", p), full and mode or "t", e)
  end
  env.dofile = function(p)
    local f, err = env.loadfile(p)
    if not f then error(err, 2) end
    return f()
  end
  -- mGBA's: beside the script first, then (full access) Lua's own.
  env.require = function(name)
    if loaded[name] ~= nil then return loaded[name] end
    local file = tostring(name):gsub("%.", "/")
    local path
    for t in tostring(env.package.path):gmatch("[^;]+") do
      local p = t:gsub("%?", file)
      if full or allowed(p) then
        local h = rio.open(p)
        if h then h:close() path = p break end
      end
    end
    if not path then
      if full then return realRequire(name) end
      if STD[name] then return env[name] end
      error("module '" .. tostring(name) .. "' not found beside the script", 2)
    end
    local f, err = rawloadfile(path, full and "bt" or "t", env)
    if not f then error(err, 2) end
    local r = f(name, path)
    if r ~= nil then loaded[name] = r end
    if loaded[name] == nil then loaded[name] = true end
    return loaded[name], path
  end
  local base = {}
  for k, v in pairs(env) do base[k] = v end
  table.insert(__scripts, {id = id, env = env, base = base})
  return env
end
"##;

pub mod canvas;
pub mod input;
mod points;
mod socket;
pub mod storage;
#[cfg(test)]
mod tests;
