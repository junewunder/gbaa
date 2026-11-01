//! The Lua probe (docs/research/lua-scripting.md): mGBA's scripting globals
//! (`emu`, `callbacks`, `console`, `C`, `socket`, `util`), stubbed over our
//! runtime's interpreter, to see how far real mGBA scripts get unchanged.
//!
//! `lua-probe ROM [--state FILE] [--frames N] SCRIPT…` loads the ROM (and
//! an mGBA or gbaa state), sets the globals up as mGBA has them while a game
//! is running, runs each script, then runs N frames, firing `keysRead`
//! before each and `frame` after. It prints what the scripts called, what
//! they asked for that the probe doesn't have, their errors, the console
//! and the text buffers, and what the Lua side cost per frame.
//!
//! What isn't implemented answers any key with a function that returns
//! nil and is counted under "missing", so a script runs on past it.

use desktop::state_file;
use mlua::{FromLuaMulti, Function, IntoLuaMulti, Lua, Table, Value};
use runtime::Machine;
use runtime::platform::{Frame, Platform};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// A front end whose keys are what the script set.
struct Pad {
    keys: Rc<Cell<u16>>,
}

impl Platform for Pad {
    fn present(&mut self, _frame: &Frame) {}
    fn keys(&mut self) -> u16 {
        self.keys.get()
    }
}

type Shared = Rc<RefCell<Box<Machine<Pad>>>>;

#[derive(Default)]
struct Log {
    used: BTreeMap<String, u64>,
    missing: BTreeMap<String, u64>,
}

type SharedLog = Rc<RefCell<Log>>;

/// A Rust function for Lua, counted under `name` each time it is called.
fn api<A, R>(
    lua: &Lua,
    log: &SharedLog,
    name: &str,
    f: impl Fn(&Lua, A) -> mlua::Result<R> + 'static,
) -> mlua::Result<Function>
where
    A: FromLuaMulti,
    R: IntoLuaMulti,
{
    let (log, name) = (log.clone(), name.to_string());
    lua.create_function(move |lua, a| {
        *log.borrow_mut().used.entry(name.clone()).or_default() += 1;
        f(lua, a)
    })
}

/// mGBA's GBA memory domains (`src/gba/core.c`): name, base, size.
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

fn read_state(m: &mut Machine<Pad>, bytes: Vec<u8>) -> Result<(), String> {
    if runtime::snap::is_ours(&bytes) {
        return m.load_state(&bytes).map_err(|e| format!("{e:?}"));
    }
    let f = state_file::unpack(bytes)?;
    m.load_mgba_state(&f.state).map_err(|e| format!("{e:?}"))?;
    // As desktop::slots::load: a PNG state's save data replaces the cart's.
    if let Some(save) = &f.save {
        let data = &mut m.bus.save.data;
        let n = save.len().min(data.len());
        data[..n].copy_from_slice(&save[..n]);
    }
    Ok(())
}

fn save_state(m: &mut Machine<Pad>) -> Vec<u8> {
    let mut v = Vec::new();
    m.save_state(&mut |b| v.extend_from_slice(b));
    v
}

/// One memory-access method set (`read8` … `readRange`), at `base`.
fn memory_methods(
    lua: &Lua,
    log: &SharedLog,
    m: &Shared,
    t: &Table,
    prefix: &str,
    base: u32,
) -> mlua::Result<()> {
    macro_rules! read {
        ($name:literal, $f:ident) => {{
            let m = m.clone();
            t.set(
                $name,
                api(
                    lua,
                    log,
                    &format!("{prefix}:{}", $name),
                    move |_, (_s, a): (Value, u32)| Ok(m.borrow().bus.$f(base.wrapping_add(a))),
                )?,
            )?;
        }};
    }
    macro_rules! write {
        ($name:literal, $f:ident, $ty:ty) => {{
            let m = m.clone();
            t.set(
                $name,
                api(
                    lua,
                    log,
                    &format!("{prefix}:{}", $name),
                    move |_, (_s, a, v): (Value, u32, i64)| {
                        m.borrow_mut().bus.$f(base.wrapping_add(a), v as $ty);
                        Ok(())
                    },
                )?,
            )?;
        }};
    }
    read!("read8", read8);
    read!("read16", read16);
    read!("read32", read32);
    write!("write8", write8, u8);
    write!("write16", write16, u16);
    write!("write32", write32, u32);
    let mm = m.clone();
    t.set(
        "readRange",
        api(
            lua,
            log,
            &format!("{prefix}:readRange"),
            move |lua, (_s, a, n): (Value, u32, u32)| {
                let m = mm.borrow();
                let bytes: Vec<u8> = (0..n)
                    .map(|i| m.bus.read8(base.wrapping_add(a + i)))
                    .collect();
                lua.create_string(&bytes)
            },
        )?,
    )?;
    Ok(())
}

fn install(
    lua: &Lua,
    log: &SharedLog,
    m: &Shared,
    keys: &Rc<Cell<u16>>,
    frame: &Rc<Cell<u64>>,
) -> mlua::Result<()> {
    let g = lua.globals();
    let (l2, l3) = (log.clone(), log.clone());
    g.set(
        "__missing",
        lua.create_function(move |_, name: String| {
            *l2.borrow_mut().missing.entry(name).or_default() += 1;
            Ok(())
        })?,
    )?;
    g.set(
        "__used",
        lua.create_function(move |_, name: String| {
            *l3.borrow_mut().used.entry(name).or_default() += 1;
            Ok(())
        })?,
    )?;

    let emu = lua.create_table()?;
    memory_methods(lua, log, m, &emu, "emu", 0)?;
    let memory = lua.create_table()?;
    for &(name, base, size) in DOMAINS {
        let d = lua.create_table()?;
        memory_methods(lua, log, m, &d, &format!("memory.{name}"), base)?;
        d.set(
            "base",
            api(lua, log, "domain:base", move |_, _: Value| Ok(base))?,
        )?;
        d.set(
            "bound",
            api(lua, log, "domain:bound", move |_, _: Value| Ok(base + size))?,
        )?;
        d.set(
            "size",
            api(lua, log, "domain:size", move |_, _: Value| Ok(size))?,
        )?;
        d.set(
            "name",
            api(lua, log, "domain:name", move |_, _: Value| Ok(name))?,
        )?;
        memory.set(name, d)?;
    }
    emu.set("memory", memory)?;

    let rom = m.borrow().bus.rom;
    let header = move |a: usize, b: usize| {
        let s = &rom[a..b];
        let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
        String::from_utf8_lossy(&s[..end]).into_owned()
    };
    emu.set(
        "getGameTitle",
        api(lua, log, "emu:getGameTitle", move |_, _: Value| {
            Ok(header(0xA0, 0xAC))
        })?,
    )?;
    // As mGBA 0.10, the release: "AGB-BPEE" (0.11-dev drops the prefix).
    emu.set(
        "getGameCode",
        api(lua, log, "emu:getGameCode", move |_, _: Value| {
            Ok(format!("AGB-{}", header(0xAC, 0xB0)))
        })?,
    )?;
    emu.set(
        "romSize",
        api(lua, log, "emu:romSize", move |_, _: Value| Ok(rom.len()))?,
    )?;
    emu.set(
        "platform",
        api(lua, log, "emu:platform", |_, _: Value| Ok(0))?,
    )?;
    emu.set(
        "frameCycles",
        api(lua, log, "emu:frameCycles", |_, _: Value| Ok(280_896))?,
    )?;
    emu.set(
        "frequency",
        api(lua, log, "emu:frequency", |_, _: Value| Ok(16_777_216))?,
    )?;
    let f = frame.clone();
    emu.set(
        "currentFrame",
        api(lua, log, "emu:currentFrame", move |_, _: Value| Ok(f.get()))?,
    )?;
    emu.set(
        "checksum",
        api(
            lua,
            log,
            "emu:checksum",
            move |lua, (_s, t): (Value, Option<i64>)| {
                // C.CHECKSUM.CRC32, big-endian, as mGBA gives it to Lua.
                if t.unwrap_or(0) != 0 {
                    return Ok(Value::Nil);
                }
                let crc = runtime::state::crc32(rom);
                Ok(Value::String(lua.create_string(crc.to_be_bytes())?))
            },
        )?,
    )?;

    macro_rules! keyfn {
        ($name:literal, |$k:ident, $v:ident| $body:expr) => {{
            let $k = keys.clone();
            emu.set(
                $name,
                api(
                    lua,
                    log,
                    concat!("emu:", $name),
                    move |_, (_s, $v): (Value, Option<u32>)| {
                        let $v = $v.unwrap_or(0);
                        Ok($body)
                    },
                )?,
            )?;
        }};
    }
    keyfn!("getKeys", |k, _v| k.get() as u32);
    keyfn!("setKeys", |k, v| k.set(v as u16));
    keyfn!("addKeys", |k, v| k.set(k.get() | v as u16));
    keyfn!("clearKeys", |k, v| k.set(k.get() & !v as u16));
    keyfn!("addKey", |k, v| k.set(k.get() | 1 << v));
    keyfn!("clearKey", |k, v| k.set(k.get() & !(1 << v)));
    keyfn!("getKey", |k, v| (k.get() >> v) as u32 & 1);

    let mm = m.clone();
    emu.set(
        "readRegister",
        api(
            lua,
            log,
            "emu:readRegister",
            move |_, (_s, n): (Value, String)| {
                let m = mm.borrow();
                Ok(match n.as_str() {
                    "cpsr" => Some(m.cpu.cpsr),
                    _ => register(&n).map(|r| if r == 15 { m.cpu.pc } else { m.cpu.r[r] }),
                })
            },
        )?,
    )?;
    let mm = m.clone();
    emu.set(
        "writeRegister",
        api(
            lua,
            log,
            "emu:writeRegister",
            move |_, (_s, n, v): (Value, String, u32)| {
                let mut m = mm.borrow_mut();
                match register(&n) {
                    Some(15) => m.cpu.pc = v,
                    Some(r) => m.cpu.r[r] = v,
                    None => {}
                }
                Ok(())
            },
        )?,
    )?;
    let mm = m.clone();
    emu.set(
        "saveStateBuffer",
        api(
            lua,
            log,
            "emu:saveStateBuffer",
            move |lua, _: mlua::MultiValue| lua.create_string(save_state(&mut mm.borrow_mut())),
        )?,
    )?;
    let mm = m.clone();
    emu.set(
        "loadStateBuffer",
        api(
            lua,
            log,
            "emu:loadStateBuffer",
            move |_, (_s, b): (Value, mlua::LuaString)| {
                Ok(read_state(&mut mm.borrow_mut(), b.as_bytes().to_vec()).is_ok())
            },
        )?,
    )?;
    let mm = m.clone();
    emu.set(
        "saveStateFile",
        api(
            lua,
            log,
            "emu:saveStateFile",
            move |_, (_s, p): (Value, String)| {
                Ok(std::fs::write(p, save_state(&mut mm.borrow_mut())).is_ok())
            },
        )?,
    )?;
    let mm = m.clone();
    emu.set(
        "loadStateFile",
        api(
            lua,
            log,
            "emu:loadStateFile",
            move |_, (_s, p): (Value, String)| {
                let Ok(b) = std::fs::read(p) else {
                    return Ok(false);
                };
                Ok(read_state(&mut mm.borrow_mut(), b).is_ok())
            },
        )?,
    )?;
    let mm = m.clone();
    emu.set(
        "screenshot",
        api(
            lua,
            log,
            "emu:screenshot",
            move |_, (_s, p): (Value, String)| {
                Ok(desktop::write_png(Path::new(&p), &mm.borrow().ppu.frame).is_ok())
            },
        )?,
    )?;
    let mm = m.clone();
    emu.set(
        "runFrame",
        api(lua, log, "emu:runFrame", move |_, _: Value| {
            mm.borrow_mut().run_frame();
            Ok(())
        })?,
    )?;
    let mm = m.clone();
    emu.set(
        "step",
        api(lua, log, "emu:step", move |_, _: Value| {
            mm.borrow_mut().step();
            Ok(())
        })?,
    )?;
    let mm = m.clone();
    let k = keys.clone();
    emu.set(
        "reset",
        api(lua, log, "emu:reset", move |lua, _: Value| {
            let rom = mm.borrow().bus.rom;
            **mm.borrow_mut() = Machine::new(rom, Pad { keys: k.clone() });
            lua.globals()
                .get::<Function>("__trigger")?
                .call::<()>("reset")
        })?,
    )?;
    g.set("emu", emu)?;
    lua.load(PRELUDE).set_name("prelude").exec()
}

/// What is plain bookkeeping in mGBA, written in Lua: the callback list, the
/// console and its text buffers, the constants, and the stubbed modules.
const PRELUDE: &str = r##"
local function stubbed(t, path)
  return setmetatable(t, {__index = function(_, k)
    __missing(path .. "." .. tostring(k))
    return function() return nil end
  end})
end
stubbed(emu, "emu")
for name, d in pairs(emu.memory) do stubbed(d, "memory." .. name) end

local cbs, nextId = {}, 1
__errors = {}
callbacks = stubbed({
  add = function(_, name, fn)
    __used("callbacks:add(" .. name .. ")")
    local id = nextId; nextId = nextId + 1
    cbs[id] = {name = name, fn = fn}
    return id
  end,
  remove = function(_, id) __used("callbacks:remove"); cbs[id] = nil end,
}, "callbacks")
function __trigger(name)
  for id, cb in pairs(cbs) do
    if cb.name == name then
      local ok, err = pcall(cb.fn)
      if not ok then
        local key = name .. ": " .. tostring(err)
        __errors[key] = (__errors[key] or 0) + 1
      end
    end
  end
end

__console, __buffers = {}, {}
local function out(kind) return function(_, msg)
  __used("console:" .. kind)
  table.insert(__console, kind .. ": " .. tostring(msg))
end end
console = stubbed({log = out("log"), warn = out("warn"), error = out("error")}, "console")
function console:createBuffer(name)
  __used("console:createBuffer")
  local b = {name = name, text = ""}
  local function m(n, f) b[n] = function(self, ...) __used("buffer:" .. n); return f(self, ...) end end
  m("print", function(self, s) self.text = self.text .. tostring(s) end)
  m("clear", function(self) self.text = "" end)
  m("setName", function(self, n) self.name = n end)
  m("setSize", function() end)
  m("moveCursor", function() end)
  m("advance", function() end)
  m("getX", function() return 0 end)
  m("getY", function() return 0 end)
  m("cols", function() return 80 end)
  m("rows", function() return 24 end)
  table.insert(__buffers, b)
  return stubbed(b, "buffer")
end
print = function(...)
  local t = {}
  for i = 1, select("#", ...) do t[i] = tostring((select(i, ...))) end
  out("print")(nil, table.concat(t, "\t"))
end

C = {
  PLATFORM = {NONE = -1, GBA = 0, GB = 1},
  CHECKSUM = {CRC32 = 0, MD5 = 1, SHA1 = 2},
  SAVESTATE = {SCREENSHOT = 1, SAVEDATA = 2, CHEATS = 4, RTC = 8, METADATA = 16, ALL = 31},
  GBA_KEY = {A = 0, B = 1, SELECT = 2, START = 3, RIGHT = 4, LEFT = 5, UP = 6, DOWN = 7, R = 8, L = 9},
}
stubbed(C, "C")
util = stubbed({
  makeBitmask = function(l) local m = 0 for _, b in ipairs(l) do m = m | (1 << b) end return m end,
  expandBitmask = function(m) local l = {} for b = 0, 31 do if m & (1 << b) ~= 0 then l[#l + 1] = b end end return l end,
}, "util")
socket = stubbed({ERRORS = {}}, "socket")
input = stubbed({}, "input")
storage = stubbed({}, "storage")
canvas = stubbed({}, "canvas")
image = stubbed({}, "image")
"##;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut rom, mut state, mut frames, mut scripts) = (None, None, 600u64, vec![]);
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--state" => state = it.next(),
            "--frames" => frames = it.next().and_then(|n| n.parse().ok()).ok_or("--frames N")?,
            _ if rom.is_none() => rom = Some(a),
            _ => scripts.push(a),
        }
    }
    let rom = rom.ok_or("usage: lua-probe ROM [--state FILE] [--frames N] SCRIPT...")?;
    let rom: &'static [u8] = Box::leak(
        std::fs::read(&rom)
            .map_err(|e| e.to_string())?
            .into_boxed_slice(),
    );
    let keys = Rc::new(Cell::new(0));
    let m: Shared = Rc::new(RefCell::new(Box::new(Machine::new(
        rom,
        Pad { keys: keys.clone() },
    ))));
    if let Some(s) = state {
        read_state(
            &mut m.borrow_mut(),
            std::fs::read(&s).map_err(|e| e.to_string())?,
        )?;
    }
    let frame = Rc::new(Cell::new(0u64));
    let log = SharedLog::default();
    // As mGBA: every standard library, `debug` included (luaL_openlibs).
    let lua = unsafe { Lua::unsafe_new() };
    install(&lua, &log, &m, &keys, &frame).map_err(|e| e.to_string())?;
    for s in &scripts {
        let dir = Path::new(s)
            .parent()
            .unwrap_or(Path::new("."))
            .display()
            .to_string();
        let path: String = lua
            .load("return package.path")
            .eval()
            .map_err(|e| e.to_string())?;
        lua.globals()
            .get::<Table>("package")
            .and_then(|p| p.set("path", format!("{dir}/?.lua;{path}")))
            .map_err(|e| e.to_string())?;
        let src = std::fs::read_to_string(s).map_err(|e| e.to_string())?;
        if let Err(e) = lua.load(&src).set_name(format!("@{s}")).exec() {
            println!("script {s} stopped loading: {e}");
        }
    }
    let trigger: Function = lua.globals().get("__trigger").map_err(|e| e.to_string())?;
    let (mut in_lua, mut in_game) = (Duration::ZERO, Duration::ZERO);
    for _ in 0..frames {
        let t0 = Instant::now();
        trigger.call::<()>("keysRead").map_err(|e| e.to_string())?;
        let t1 = Instant::now();
        m.borrow_mut().run_frame();
        frame.set(frame.get() + 1);
        let t2 = Instant::now();
        trigger.call::<()>("frame").map_err(|e| e.to_string())?;
        in_lua += t1 - t0 + t2.elapsed();
        in_game += t2 - t1;
    }
    let log = log.borrow();
    println!("ran {frames} frames: game {in_game:?}, Lua callbacks {in_lua:?}");
    println!("\ncalled (implemented):");
    for (k, n) in &log.used {
        println!("  {k}: {n}");
    }
    println!("\nmissing (not in the probe):");
    for (k, n) in &log.missing {
        println!("  {k}: {n}");
    }
    let g = lua.globals();
    println!("\ncallback errors:");
    for (k, n) in g
        .get::<Table>("__errors")
        .map_err(|e| e.to_string())?
        .pairs::<String, u64>()
        .flatten()
    {
        println!("  {n}x {k}");
    }
    println!("\nconsole (first 40 lines):");
    for l in g
        .get::<Table>("__console")
        .map_err(|e| e.to_string())?
        .sequence_values::<String>()
        .flatten()
        .take(40)
    {
        println!("  {l}");
    }
    for b in g
        .get::<Table>("__buffers")
        .map_err(|e| e.to_string())?
        .sequence_values::<Table>()
        .flatten()
    {
        let name: String = b.get("name").unwrap_or_default();
        let text: String = b.get("text").unwrap_or_default();
        println!("\nbuffer {name}:\n{text}");
    }
    Ok(())
}
