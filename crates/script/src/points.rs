//! mGBA 0.11-dev's script breakpoints and watchpoints (`emu:setBreakpoint`,
//! `setWatchpoint`, `setRangeWatchpoint`, `clearBreakpoint`, after
//! `src/core/scripting.c` and `src/arm/debugger/memory-debugger.c` at
//! mGBA's master), over [`runtime::debug`]. While any is set the front end
//! runs frames in the interpreter ([`Host::run_frame_debug`]), the owner's
//! choice: compiled code has no instruction boundary.
//!
//! As in mGBA: a breakpoint fires before the instruction at its address
//! (exactly that address: a Thumb function's without its low bit), with
//! `{address, segment = 0}`; a watchpoint on `[min, max)` fires for an
//! access that overlaps it, with `{address, width, segment = 0, oldValue,
//! newValue (stores only), accessType}`, values as mGBA's signed 32-bit
//! (a byte or halfword store's `newValue` sign-extended, so
//! `WRITE_CHANGE` compares them as mGBA does). Unlike mGBA, a watchpoint's
//! callbacks run after the instruction that hit it, as a callback can't
//! run with the machine in the middle of one; and an `ldm`/`stm` tells
//! each register's value, where mGBA's tells 0. DMA and BIOS calls aren't
//! watched. Several callbacks on one point fire in the order they were
//! set. Frames a callback runs (`emu:runFrame`) don't stop, as mGBA
//! doesn't enter its debugger again from one.

use crate::{BUDGET, Host, Shared};
use mlua::{Function, Lua, Table, Value};
use runtime::debug::{Access, Debugger, Watch};
use runtime::{Machine, Platform};
use std::rc::Rc;

/// mGBA's `WATCHPOINT_TYPE` bits.
const WRITE: u32 = 1;
const READ: u32 = 2;
const CHANGE: u32 = 4;

/// The points set, by the id `setBreakpoint` and its kin return.
#[derive(Default)]
pub(crate) struct Points {
    breaks: Vec<(i64, u32)>,
    /// Id, `[min, max)` and `WATCHPOINT_TYPE`.
    watches: Vec<(i64, u32, u32, u32)>,
    /// Hit in this instruction, to fire after it.
    hits: Vec<(i64, Hit)>,
    next: i64,
}

#[derive(Clone, Copy)]
enum Hit {
    Break(u32),
    Watch(Access, i32),
}

impl Points {
    pub(crate) fn any(&self) -> bool {
        !self.breaks.is_empty() || !self.watches.is_empty()
    }

    fn clear(&mut self, id: i64) -> bool {
        let n = self.breaks.len() + self.watches.len();
        self.breaks.retain(|p| p.0 != id);
        self.watches.retain(|p| p.0 != id);
        self.hits.retain(|h| h.0 != id);
        self.breaks.len() + self.watches.len() != n
    }
}

/// `v`'s low `width` bytes as the signed value mGBA's store shims pass on.
fn signed(v: u32, width: u32) -> i32 {
    match width {
        1 => v as u8 as i8 as i32,
        2 => v as u16 as i16 as i32,
        _ => v as i32,
    }
}

/// `__setPoint(min, max, type)` (type -1 a breakpoint at min) and
/// `__clearPoint(id)`, for the prelude's `emu` methods.
pub(crate) fn install<P: Platform + 'static>(lua: &Lua, s: &Rc<Shared<P>>) -> mlua::Result<()> {
    let g = lua.globals();
    let ss = s.clone();
    g.set(
        "__setPoint",
        lua.create_function(move |_, (lo, hi, kind): (i64, i64, i64)| {
            let mut p = ss.points.borrow_mut();
            p.next += 1;
            let id = p.next;
            let (lo, hi) = (lo as u32, hi as u32);
            match kind {
                -1 => p.breaks.push((id, lo)),
                k => p.watches.push((id, lo, hi, k as u32)),
            }
            Ok(id)
        })?,
    )?;
    let ss = s.clone();
    g.set(
        "__clearPoint",
        lua.create_function(move |_, id: i64| Ok(ss.points.borrow_mut().clear(id)))?,
    )?;
    Ok(())
}

/// The callbacks for what was hit, with mGBA's info table each.
fn fire<P: Platform>(lua: &Lua, s: &Shared<P>) {
    let hits = std::mem::take(&mut s.points.borrow_mut().hits);
    let Ok(points) = lua.globals().get::<Table>("__points") else {
        return;
    };
    for (id, hit) in hits {
        // An earlier callback may have cleared it.
        let Ok(p) = points.get::<Table>(id) else {
            continue;
        };
        let (Ok(f), Ok(script)) = (p.get::<Function>("fn"), p.get::<Value>("script")) else {
            continue;
        };
        let r = (|| -> mlua::Result<()> {
            let info = lua.create_table()?;
            info.set("segment", 0)?;
            let what = match hit {
                Hit::Break(addr) => {
                    info.set("address", addr)?;
                    "breakpoint"
                }
                Hit::Watch(a, new) => {
                    info.set("address", a.addr)?;
                    info.set("width", a.width)?;
                    info.set("oldValue", a.old as i32)?;
                    if a.write {
                        info.set("newValue", new)?;
                    }
                    info.set("accessType", if a.write { WRITE } else { READ })?;
                    "watchpoint"
                }
            };
            let outer = s.budget.get();
            s.budget.set(BUDGET);
            let g = lua.globals();
            let was: Value = g.get("__script")?;
            g.set("__script", script)?;
            let r = f.call::<()>(info);
            g.set("__script", was)?;
            let overran = s.budget.get() == 0;
            s.budget.set(outer);
            if let Err(e) = r {
                let mut c = s.console.borrow_mut();
                c.push(format!("error: {what} callback: {e}"));
                if overran {
                    c.push(format!("error: {what} callback removed"));
                    drop(c);
                    points.set(id, Value::Nil)?;
                    s.points.borrow_mut().clear(id);
                }
            }
            Ok(())
        })();
        if let Err(e) = r {
            s.console.borrow_mut().push(format!("error: {e}"));
        }
    }
}

/// The host as [`runtime::debug`] sees it, for one frame.
struct HostDebug<'a, P: Platform + 'static>(&'a Host<P>);

impl<P: Platform> Watch for HostDebug<'_, P> {
    fn access(&mut self, a: Access) {
        let mut p = self.0.s.points.borrow_mut();
        let p = &mut *p;
        let kind = if a.write { WRITE } else { READ };
        let lo = a.addr & !(a.width - 1);
        let hi = lo.wrapping_add(a.width);
        let new = signed(a.new, a.width);
        for &(id, min, max, t) in &p.watches {
            if t & kind == 0 || !(min < hi && lo < max) {
                continue;
            }
            if t & CHANGE != 0 && new as u32 == a.old {
                continue;
            }
            p.hits.push((id, Hit::Watch(a, new)));
        }
    }
}

impl<P: Platform> Debugger<P> for HostDebug<'_, P> {
    fn before(&mut self, m: &mut Machine<P>) {
        let pc = m.cpu.pc & if m.cpu.thumb() { !1 } else { !3 };
        {
            let mut p = self.0.s.points.borrow_mut();
            let p = &mut *p;
            for &(id, at) in &p.breaks {
                if at == pc {
                    p.hits.push((id, Hit::Break(at)));
                }
            }
            if p.hits.is_empty() {
                return;
            }
        }
        self.0.lend(m, || fire(&self.0.lua, &self.0.s));
    }

    fn watching(&self) -> bool {
        !self.0.s.points.borrow().watches.is_empty()
    }

    fn after(&mut self, m: &mut Machine<P>) {
        if !self.0.s.points.borrow().hits.is_empty() {
            self.0.lend(m, || fire(&self.0.lua, &self.0.s));
        }
    }
}

impl<P: Platform + 'static> Host<P> {
    /// Whether a script has a breakpoint or watchpoint set: the front end
    /// then runs frames with [`Host::run_frame_debug`].
    pub fn has_points(&self) -> bool {
        self.s.points.borrow().any()
    }

    /// One frame in the interpreter, firing the scripts' breakpoints and
    /// watchpoints as it goes.
    pub fn run_frame_debug(&self, m: &mut Machine<P>) {
        m.run_frame_debug(&mut HostDebug(self));
    }
}

/// The prelude's part: `emu`'s methods, `C.WATCHPOINT_TYPE`, and the
/// callbacks by id.
pub(crate) const PRELUDE: &str = r##"
__points = {}
local function point(fn, min, max, kind)
  local id = __setPoint(min, max, kind)
  __points[id] = {fn = fn, script = __script}
  return id
end
function emu:setBreakpoint(fn, address) return point(fn, address, address, -1) end
function emu:setWatchpoint(fn, address, kind) return point(fn, address, address + 1, kind) end
function emu:setRangeWatchpoint(fn, min, max, kind) return point(fn, min, max, kind) end
function emu:clearBreakpoint(id)
  __points[id] = nil
  return __clearPoint(id)
end
function __unloadPoints(script)
  for id, p in pairs(__points) do
    if p.script == script then emu:clearBreakpoint(id) end
  end
end
"##;
