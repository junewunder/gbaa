//! IR to Rust (`docs/codegen-design.md`, "IR to Rust" and "Exits"): one
//! Rust fn per guest function, `loop { match pc }` over its blocks, each
//! block's IR printed by [`crate::print`].
//!
//! The output names only what the generated crate's `prelude` brings in:
//! `M` (the machine), `Flow`, `rt` (the dispatch helpers), the `armv4t`
//! types and `prim`, and the `insn!` marker macro. So the same lowering
//! serves a ROM crate (`rt` is `runtime::dispatch`) and the SingleStepTests
//! crate (`rt` is its own). A ROM crate's prelude also has `call`, the
//! indirect call through its own table (`rt::call`).

use crate::cfg::Cfg;
use crate::dwarf::FnDwarf;
use crate::func::{Clock, Config, Ctx, Idle, Insn};
use crate::func::{ExitInfo, Func, GBlock, Plan, exits};
use crate::locals::{Locals, Point};
use crate::plan::{self, Edge, How, Node, Out, Planned};
use crate::print::{Hints, Printer};
use analysis::ir::{BinOp, Exit, Op, Timing, ValueId};
use armv4t::cycles::Cost;
use armv4t::decode::Instr;
use armv4t::sem::Width;
use armv4t::sem::{Flag, Reg};
use proc_macro2::{Ident, Literal, TokenStream};
use quote::{format_ident, quote};
use std::collections::BTreeMap;

/// The name of the fn for the function whose entry is `key`.
pub fn fn_name(key: u32) -> Ident {
    format_ident!("f_{:08x}", key)
}

/// `0x0800_0124`.
pub fn hex(x: u32) -> TokenStream {
    let s = format!("0x{:04x}_{:04x}", x >> 16, x & 0xFFFF);
    s.parse().unwrap()
}

fn lit(x: u32) -> Literal {
    Literal::u32_unsuffixed(x)
}

/// The cost of one instruction: the interpreter's own.
pub fn cost(i: &Instr) -> Cost {
    armv4t::cycles::cost(i)
}

/// The memory region (top byte) of code at `addr`, as the runtime's wait
/// tables index it.
fn region(addr: u32) -> usize {
    let r = (addr >> 24) as usize;
    if r < 16 { r } else { 0 }
}

/// The expression for `c`'s cycles, run from `addr`: its base and, at run
/// time, its fetches' wait states.
fn cost_expr(c: Cost, addr: u32, thumb: bool) -> TokenStream {
    let base = lit(c.base);
    if c.seq == 0 && c.nonseq == 0 {
        return quote!(#base);
    }
    let (r, s, n) = (
        Literal::usize_unsuffixed(region(addr)),
        lit(c.seq),
        lit(c.nonseq),
    );
    quote!(#base + rt::fetch(m, #r, #thumb, #s, #n))
}

/// The cycles of `gb`'s instructions before exit `x` (and the `bx`
/// stub a call goes through), as `clock` counts them: in mGBA's model, the
/// base and the fetches' wait states at run time; else one constant.
fn exit_cost(clock: Clock, gb: &GBlock, x: &ExitInfo) -> TokenStream {
    let thumb = gb.block.thumb;
    let mut c = gb
        .insns
        .iter()
        .take(x.insns)
        .fold(Cost::default(), |a, (_, i)| a.plus(cost(i)));
    c.base = c.base.max(1);
    let stub = match x.plan {
        Plan::CallVia { cost, stub, .. } => Some((cost, stub)),
        _ => None,
    };
    match clock {
        Clock::Exact => {
            let n = cost_expr(c, gb.key, thumb);
            match stub {
                Some((cost, stub)) => {
                    let s = cost_expr(cost, stub, thumb);
                    quote!(#n + #s)
                }
                None => n,
            }
        }
        Clock::Static => {
            let mut w = runtime::waits::Waits::default();
            w.set_waitcnt(crate::func::STATIC_WAITCNT);
            let at = |c: Cost, a: u32| c.base + w.fetch(region(a), thumb, c.seq, c.nonseq);
            let n = lit(at(c, gb.key) + stub.map_or(0, |(c, s)| at(c, s)));
            quote!(#n)
        }
        Clock::Instant => {
            let n = lit(c.base + stub.map_or(0, |(c, _)| c.base));
            quote!(#n)
        }
        Clock::Native => quote!(0),
    }
}

/// The statements that leave the block through exit `x`, ending in `then`
/// (v1's `continue`, to the `match`) or `return`. `ram`: the block is an
/// overlay's, whose ticks count as RAM code's. `kept`: the exit calls a
/// function proven to keep r4-r11 and sp ([`crate::saves`]).
fn lower_exit(
    p: &Printer,
    gb: &GBlock,
    x: &ExitInfo,
    ram: bool,
    kept: bool,
    then: TokenStream,
) -> TokenStream {
    let n = exit_cost(p.clock(), gb, x);
    let body = exit_body(p, gb, x, ram, kept, quote!(cycles), then);
    quote! {
        let cycles: u32 = #n;
        #body
    }
}

/// A BIOS call's comment in the readable form: `BIOS CpuSet (swi 0x0b)`,
/// or just the number if no call has it.
fn bios_note(gb: &GBlock, x: &ExitInfo) -> Option<String> {
    let Op::Exit(Exit::Swi { comment, .. }) = gb.block.insts[x.index].op else {
        return None;
    };
    let n = runtime::bios::number(comment, gb.block.thumb);
    Some(match runtime::bios::call::name(n) {
        Some(name) => format!("BIOS {name} (swi {n:#04x})"),
        None => format!("BIOS call {n:#04x} (no such call)"),
    })
}

/// [`lower_exit`]'s statements after the cycles are counted into `n`.
fn exit_body(
    p: &Printer,
    gb: &GBlock,
    x: &ExitInfo,
    ram: bool,
    kept: bool,
    n: TokenStream,
    then: TokenStream,
) -> TokenStream {
    let tick_fn = match (ram, p.clock()) {
        (true, Clock::Exact) => quote!(rt::tick_ram),
        (false, Clock::Exact) => quote!(rt::tick),
        (true, _) => quote!(rt::tick_ram_flat),
        (false, _) => quote!(rt::tick_flat),
    };
    // The flags whose writes liveness deleted, which an IRQ taken at the
    // tick could see (`docs/structured-codegen-design.md`, step 2).
    let yields: Vec<usize> = (0..x.index)
        .rev()
        .take_while(|&i| matches!(gb.block.insts[i].op, Op::YieldFlag(..)))
        .collect();
    let flags = p.yield_flags(&yields);
    let loc = p.locals();
    // An IRQ or halt is taken here, nested (`rt::yield_at`), and compiled
    // code goes on; the frame end or a fault unwinds.
    let y = loc.sync(Point::Yield);
    let (wb, reload) = (y.before, y.after);
    let take = quote! {
        #wb
        if let Some(k) = yield_at(m, pc) {
            return Flow::Jump(k);
        }
        #reload
    };
    // Around a call into guest code.
    let c = loc.sync(Point::Call);
    let (call_wb, call_reload) = (c.before, c.after);
    let ret_stale = loc.ret(Point::ReturnStale, quote!(pc));
    let ret = loc.ret(Point::Return, quote!(pc));
    let sp = loc.reg(13);
    // Native timing ticks nothing, but runs the clock on an idle loop's
    // back edge.
    let tick_call = match p.clock() {
        Clock::Native if x.idle == Idle::Edge => quote!(rt::idle_edge(m)),
        Clock::Native if x.idle == Idle::Exit => quote!(rt::idle_exit(m)),
        Clock::Native if x.idle == Idle::Spin => {
            let at = hex(x.at);
            quote!(rt::spin_edge(m, #at))
        }
        Clock::Native => quote!(rt::tick_native(m)),
        _ => quote!(#tick_fn(m, #n)),
    };
    let tick = quote! {
        if #tick_call {
            #flags
            #take
        }
    };
    let (at, thumb) = (hex(x.at), gb.block.thumb);
    let Op::Exit(e) = gb.block.insts[x.index].op else {
        unreachable!()
    };
    let set = match (x.plan, e) {
        (Plan::Static(k), _) => {
            let k = hex(k);
            quote!(pc = #k;)
        }
        (Plan::Call { target, ret }, _) => {
            let (f, t, r) = (p.fn_ident(target), hex(target), hex(ret));
            // Back at its return site from a callee that keeps r4-r11 and
            // sp, they are as the locals hold them; anywhere else (an
            // unwind past it), every local reloads.
            let reload_kept = loc.reload_kept();
            let call_reload = if kept && loc.kept && !call_reload.is_empty() {
                quote! {
                    if pc == #r {
                        #reload_kept
                    } else {
                        #call_reload
                    }
                }
            } else {
                call_reload.clone()
            };
            return quote! {
                pc = #t;
                #tick
                #call_wb
                let sp0 = #sp;
                let Flow::Jump(a) = #f(m, #t);
                // Came back elsewhere: run it to the return site here.
                pc = settle(m, a, #r, sp0);
                // The callee's tick may have asked to yield; `tick`
                // doesn't say so twice.
                if rt::yielding(m) {
                    #ret_stale;
                }
                #call_reload
                #then
            };
        }
        (
            Plan::CallVia {
                stub, reg: r, ret, ..
            },
            _,
        ) => {
            let (s, t) = (hex(stub & !1), loc.reg(r.index()));
            let (st, r) = (stub & 1 != 0, hex(ret));
            return quote! {
                // The stub's `bx`, then a call to wherever it goes.
                let t = #t;
                pc = rt::exit(m, #s, #st, Exit::Exchange(t));
                #tick
                #call_wb
                let sp0 = #sp;
                let Flow::Jump(a) = call(m, pc);
                pc = settle(m, a, #r, sp0);
                if rt::yielding(m) {
                    #ret_stale;
                }
                #call_reload
                #then
            };
        }
        (Plan::Rekey(k), _) => {
            let k = hex(k);
            quote!(pc = rt::rekey(m, #k);)
        }
        (Plan::Swi { .. }, Exit::Swi { comment, .. }) => {
            let c = hex(comment);
            quote! {
                #call_wb
                pc = rt::swi(m, #at, #thumb, #c);
                #call_reload
            }
        }
        (_, Exit::Jump(t)) => {
            let t = p.arg(t);
            quote!(pc = rt::exit(m, #at, #thumb, Exit::Jump(#t));)
        }
        (_, Exit::JumpExchange(t)) => {
            let t = p.arg(t);
            quote!(pc = rt::exit(m, #at, #thumb, Exit::Exchange(#t));)
        }
        // These two switch modes (the banks, the CPSR from the SPSR).
        (_, Exit::ExceptionReturn(t)) => {
            let t = p.arg(t);
            quote! {
                #call_wb
                pc = rt::exit(m, #at, #thumb, Exit::ExceptionReturn(#t));
                #call_reload
            }
        }
        (_, Exit::Undefined) => quote! {
            #call_wb
            pc = rt::exit(m, #at, #thumb, Exit::Undefined);
            #call_reload
        },
        (p, e) => unreachable!("{p:?} for {e:?}"),
    };
    if let Plan::Swi { .. } = x.plan {
        // A wait (`Halt`, `IntrWait`) halts, an unimplemented call faults:
        // either way the dispatcher has to take over.
        return quote! {
            #set
            if #tick_call || rt::yielding(m) {
                #take
            }
            #then
        };
    }
    if let Plan::Dynamic {
        local: false,
        ret: Some(ret),
        ..
    } = x.plan
    {
        // An unresolved indirect call: run it to the return site here,
        // through the dispatcher.
        let r = hex(ret);
        return quote! {
            let sp0 = #sp;
            #set
            #tick
            #call_wb
            pc = settle(m, pc, #r, sp0);
            if rt::yielding(m) {
                #ret_stale;
            }
            #call_reload
            #then
        };
    }
    if let Plan::Dynamic { local: false, .. } = x.plan {
        // A return, or a jump the dispatcher finds: an IRQ at its tick
        // is still taken here, so the caller doesn't unwind.
        return quote! {
            #set
            #tick
            #ret;
        };
    }
    quote! {
        #set
        #tick
        #then
    }
}

/// The `insn!` text of `i` at `a`: its assembly, with a Thumb `bl` whose
/// halves are both in the block printed whole at the first half.
fn asm_text(instrs: &BTreeMap<u32, &Instr>, a: u32, i: &Instr, thumb: bool) -> String {
    use armv4t::text::Bl;
    let half = |at: u32| instrs.get(&at).copied().copied();
    match (*i, half(a.wrapping_sub(2)), half(a.wrapping_add(2))) {
        (Instr::ThumbBlHi { offset: hi }, _, Some(Instr::ThumbBlLo { offset: lo })) => {
            Bl(a.wrapping_add(4).wrapping_add_signed(hi).wrapping_add(lo)).to_string()
        }
        (Instr::ThumbBlLo { .. }, Some(Instr::ThumbBlHi { .. }), _) => "(bl, second half)".into(),
        (i, ..) => i.text(thumb).to_string(),
    }
}

/// Where each stall of `gb` is printed (the pc and whether it follows a
/// load), by inst: run from ROM, a memory instruction or multiply ends with
/// `rt::stall` (mGBA's prefetch stall, `runtime::waits`), as in the
/// interpreter: this one's, until the next instruction starts or the block
/// exits. The printer moves nothing past one.
fn stalls_of(gb: &GBlock, instrs: &BTreeMap<u32, &Instr>, ram: bool) -> Vec<Option<(u32, bool)>> {
    let rom = !ram && region(gb.key) >= 8;
    let size = if gb.block.thumb { 2 } else { 4 };
    let insts = &gb.block.insts;
    let mut stalls = vec![None; insts.len()];
    let mut stall = None;
    for (i, inst) in insts.iter().enumerate() {
        if matches!(inst.op, Op::Insn(_) | Op::Exit(_)) {
            stalls[i] = stall.take();
        }
        if let Op::Insn(a) = inst.op
            && rom
            && let Some(load) = instrs.get(&a).and_then(|i| armv4t::cycles::stalls(i))
        {
            stall = Some((a.wrapping_add(2 * size), load));
        }
    }
    stalls
}

/// How many of `f`'s blocks the readable form prints with their registers
/// out of `Machine` until their exits, and how many blocks it has: the
/// rest keep the play view's statements ([`Readable::of`]).
pub fn readable_blocks(f: &Func) -> (usize, usize) {
    let ram = f.overlay.is_some();
    let n = f
        .blocks
        .values()
        .filter(|gb| {
            let instrs: BTreeMap<u32, &Instr> = gb.insns.iter().map(|(a, i)| (*a, i)).collect();
            !ram && Readable::of(gb, &instrs, &stalls_of(gb, &instrs, ram), &|_| None).is_some()
        })
        .count();
    (n, f.blocks.len())
}

/// One block as a `match` arm's body. `ram`: it is an overlay's.
/// `cfg`: its function's ([`Ctx::cfg_for`]).
pub fn lower_block(ctx: &Ctx, cfg: &Config, gb: &GBlock, ram: bool, loc: &Locals) -> TokenStream {
    lower_block_then(
        ctx,
        cfg,
        gb,
        ram,
        loc,
        &Carry::default(),
        None,
        &mut |_, _| (quote!(continue;), false),
    )
}

/// The readable form's text comments ([`crate::text`]): for each pool
/// word of a block run from ROM that points into the cartridge at text the
/// map's `[text]` (or the game's own table, `crate::text::Table`) decodes,
/// `gText_MainMenuNewGame: "NEW GAME"` before the instruction that loads
/// it, by that instruction's inst, or at a table of pointers that leads to
/// text, its strings ([`crate::text::Charmap::table_note`]). Once per block.
/// A map with no `[text]` (no one knows the game's encoding) gets each
/// such word's bytes instead, `sData: [u8; 32] = [..]; encoding unknown`,
/// up to the next address other code points at ([`Ctx::pointed`]) or the
/// map names, capped by `crates/codegen/data-preview.toml`
/// ([`crate::text::Preview`]).
fn text_notes(
    ctx: &Ctx,
    names: &crate::names::SourceMap,
    insts: &[analysis::ir::Inst],
) -> BTreeMap<usize, Vec<String>> {
    use analysis::program::ROM_BASE;
    let mut out: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    if ctx.base != ROM_BASE {
        return out;
    }
    let cm = names.text.as_ref();
    let mut said = Vec::new();
    let mut insn = None;
    for (i, inst) in insts.iter().enumerate() {
        let x = match inst.op {
            Op::Insn(_) => {
                insn = Some(i);
                continue;
            }
            Op::Pool(x) => x,
            _ => continue,
        };
        let Some(bytes) = x
            .checked_sub(ROM_BASE)
            .and_then(|o| ctx.rom.get(o as usize..))
            .filter(|b| !b.is_empty())
        else {
            continue;
        };
        // A callback's word is a function's key, not text, nor is code
        // data. Code only the pointer scan found may be a table of
        // pointers it took for code (Metroid's text tables): a table note
        // may still say what it leads to.
        let code = ctx.disc.code.get(&(x & !1)).map(|c| c.evidence);
        if said.contains(&x)
            || names.functions.contains_key(&x)
            || code == Some(analysis::discovery::Evidence::Confirmed)
        {
            continue;
        }
        let scanned = code.is_some();
        // The datum it lands in: its DWARF type, and the bytes left in it.
        let name = names.data_at(x);
        let item = name.and_then(|_| names.data.range(..=x).next_back());
        let note = match cm {
            Some(cm) => {
                let ty = item.and_then(|(_, d)| d.proto.as_deref());
                let size = item.and_then(|(&k, d)| Some(d.size? as u32 - (x - k)));
                let direct = if scanned {
                    None
                } else {
                    cm.note(x, bytes, name, ty, size)
                };
                direct.or_else(|| {
                    // Text reached through a table of pointers.
                    let at = match name {
                        Some((n, 0)) => n.to_string(),
                        Some((n, off)) => format!("{n}+{off}"),
                        None => format!("0x{x:08x}"),
                    };
                    let size = |a: u32| names.data.get(&a).and_then(|d| d.size).map(|n| n as u32);
                    let label = |a: u32| match names.data_at(a) {
                        Some((n, 0)) => Some(n.to_string()),
                        _ => None,
                    };
                    cm.table_note(x, ctx.rom, &at, &size, &label)
                })
            }
            None if scanned => None,
            None => {
                let after = x.saturating_add(1);
                let next = [
                    ctx.pointed().range(after..).next().copied(),
                    names.data.range(after..).next().map(|(&k, _)| k),
                ]
                .into_iter()
                .flatten()
                .min();
                crate::text::Preview::get().note(x, bytes, next, name)
            }
        };
        if let Some(n) = note {
            said.push(x);
            out.entry(insn.unwrap_or(i)).or_default().push(n);
        }
    }
    out
}

/// The readable form's comment at a call to `target` in `gb` that passes
/// a message number of a game that reaches its text by one
/// ([`crate::text::Messages`]; `crates/codegen/text-tables.toml`'s
/// `[game.index]`): `message 0x0f07: "…"`, when `target` is one of its
/// `calls` and `r0` (in `regs`, the exit's synced registers) folds to a
/// constant that is a message. Only from ROM, as [`text_notes`].
fn message_note(ctx: &Ctx, gb: &GBlock, target: u32, regs: &[(usize, ValueId)]) -> Option<String> {
    let names = ctx.names?;
    let ms = names.messages.as_ref()?;
    if ctx.base != analysis::program::ROM_BASE {
        return None;
    }
    let callee = &names.functions.get(&target)?.name;
    if !ms.index.calls.contains(callee) {
        return None;
    }
    let &(_, v) = regs.iter().find(|&&(r, _)| r == 0)?;
    let n = crate::func::fold(&gb.block, v)?;
    ms.note(ctx.rom, n)
}

/// What follows exit `j` of a block: given the return site of a readable
/// call (which may go on only there), the code, and whether it took that
/// (the code assumes `pc` is the return site).
type Then<'t> = dyn FnMut(usize, Option<u32>) -> (TokenStream, bool) + 't;

/// One block, each exit that goes on in the function ending in `then` of
/// its number (in order) where v1's ends in `continue`. `carry`: the
/// registers a readable function holds in Rust locals across its blocks.
/// `dw`: its function's DWARF, which a readable block prints from, and
/// whether this is the function's entry block.
#[allow(clippy::too_many_arguments)]
fn lower_block_then(
    ctx: &Ctx,
    cfg: &Config,
    gb: &GBlock,
    ram: bool,
    loc: &Locals,
    carry: &Carry,
    dw: Option<(&FnDwarf, bool)>,
    then: &mut Then,
) -> TokenStream {
    let xs: BTreeMap<usize, ExitInfo> = exits(ctx, gb).into_iter().map(|x| (x.index, x)).collect();
    let instrs: BTreeMap<u32, &Instr> = gb.insns.iter().map(|(a, i)| (*a, i)).collect();
    let mut out = TokenStream::new();
    let r = region(gb.key);
    let insts = &gb.block.insts;
    let stalls = stalls_of(gb, &instrs, ram);
    let word = |a: u32| {
        let o = a.wrapping_sub(ctx.base) as usize;
        let b = ctx.rom.get(o..o.checked_add(4)?)?;
        Some(u32::from_le_bytes(b.try_into().ok()?))
    };
    let rd = if cfg.readable && !ram {
        Readable::of(gb, &instrs, &stalls, &word)
    } else {
        None
    };
    let mut hints = rd.as_ref().map(|r| r.hints.clone()).unwrap_or_default();
    if let Some(rd) = &rd {
        hints.carried = carry.at.get(&gb.key).copied().unwrap_or(0);
        // A value a sync gives a carried local too is named once.
        for (&i, (regs, _)) in &rd.syncs {
            let set = carry.named(gb.key, i);
            for &(r, x) in regs {
                let local = match insts[x.index()].op {
                    Op::Const(_) | Op::Pool(_) => true,
                    Op::GetReg(q) => hints.carried & 1 << q.index() != 0,
                    _ => false,
                };
                if set & 1 << r != 0 && !local {
                    hints.bound[x.index()] = true;
                }
            }
        }
        // Accesses through an element of a typed array name its index; a
        // register holding a datum's address names the datum.
        if let Some(names) = ctx.names {
            let known = carry.known.get(&gb.key);
            let known = |r: usize| known.and_then(|k| k.get(&r)).copied();
            hints.index = vec![None; insts.len()];
            for (i, inst) in insts.iter().enumerate() {
                let (Op::Load { addr, .. } | Op::Store { addr, .. }) = inst.op else {
                    continue;
                };
                if hints.fused[i].is_none() || hints.operands[i].is_some() {
                    continue;
                }
                let Some(ix) = crate::print::index_of(insts, names, &known, addr) else {
                    continue;
                };
                let mut o: Vec<ValueId> = ix.i.into_iter().collect();
                if let Op::Store { value, .. } = inst.op {
                    o.push(value);
                }
                hints.operands[i] = Some(o);
                hints.index[i] = Some(ix);
            }
        }
    }
    let mut dn = match (&rd, dw) {
        (Some(_), Some((d, entry))) => Some((d, d.block(gb, entry))),
        _ => None,
    };
    let tn = match (&rd, ctx.names) {
        (Some(_), Some(names)) => text_notes(ctx, names, insts),
        _ => BTreeMap::new(),
    };
    let mut p = Printer::hinted(
        insts,
        &stalls.iter().map(Option::is_some).collect::<Vec<_>>(),
        loc,
        &hints,
    )
    .named(ctx.names)
    .clocked(cfg.clock);
    if let Some((_, b)) = &dn {
        p = p.values_named(b.values.clone());
    }
    // The entry block's parameters, each read of one by its name.
    let mut params = TokenStream::new();
    if let (Some((d, b)), Some((_, true))) = (&dn, dw) {
        // A parameter whose name a value of the block takes too (its copy
        // in a saved register, its narrowed value) is `name_arg`, so that
        // value's `let` doesn't shadow it.
        let mut args = d.args();
        let taken = |id: &Ident, args: &[Option<(Ident, Option<crate::types::Ty>)>]| {
            b.values
                .iter()
                .enumerate()
                .any(|(i, v)| v.as_ref() == Some(id) && p.binds(i))
                || args.iter().flatten().filter(|(a, _)| a == id).count() > 1
        };
        for j in 0..args.len() {
            while let Some((id, _)) = &args[j]
                && taken(id, &args)
            {
                let id = format_ident!("{id}_arg");
                args[j].as_mut().unwrap().0 = id;
            }
        }
        p = p.args(args);
        let mut said = Vec::new();
        for (i, inst) in insts.iter().enumerate() {
            if let Some((id, ty)) = p.arg_of(i)
                && !said.contains(id)
            {
                let Op::GetReg(r) = inst.op else {
                    unreachable!()
                };
                let r = Literal::usize_unsuffixed(r.index());
                let ty = match ty.as_ref().zip(ctx.names) {
                    Some((t, n)) => {
                        let t = n.typed.rust(t);
                        quote!(Ptr<#t>)
                    }
                    None => quote!(u32),
                };
                params.extend(quote!(let #id: #ty = arg!(m, #r);));
                said.push(id.clone());
            }
        }
    }
    out.extend(params);
    if let Some((_, b)) = &mut dn {
        // A register read whose name doesn't print gets its comment.
        let mut said = Vec::new();
        for (r, at, text) in std::mem::take(&mut b.reads) {
            let shown = b.values[r].is_some() && p.shows(r) || p.arg_of(r).is_some();
            if !shown && !said.contains(&text) {
                b.notes.entry(at).or_default().push(text.clone());
                said.push(text);
            }
        }
    }
    // A run of effects under one guard shares its `if`, until a statement
    // under another guard (or none) or an exit.
    let mut run: Option<(ValueId, TokenStream)> = None;
    let flush = |run: &mut Option<(ValueId, TokenStream)>, out: &mut TokenStream| {
        // (A run may be only the empty heads of a flag-write run whose
        // last write comes after a tuple.)
        if let Some((g, body)) = run.take()
            && !body.is_empty()
        {
            let g = p.guard(g);
            out.extend(quote!(if #g { #body }));
        }
    };
    let thumb = gb.block.thumb;
    let mut at = gb.key & !1;
    let order: BTreeMap<usize, usize> = xs.keys().enumerate().map(|(j, &i)| (i, j)).collect();
    for (i, inst) in insts.iter().enumerate() {
        if let Some(ns) = dn.as_ref().and_then(|(_, b)| b.notes.get(&i)) {
            flush(&mut run, &mut out);
            out.extend(ns.iter().map(|s| crate::dwarf::note(s)));
        }
        if let Some(ns) = tn.get(&i) {
            flush(&mut run, &mut out);
            out.extend(ns.iter().map(|s| crate::dwarf::note(s)));
        }
        let mut before = TokenStream::new();
        if let Some(rd) = &rd {
            if let Some(mm) = rd.multi.get(&i) {
                out.extend(mm.print(&p));
            }
            if let Some(b) = rd.bit_writes.get(&i) {
                out.extend(b.print(&p, i));
            }
            if rd.skip[i] {
                continue;
            }
        }
        if let Some((pc, load)) = stalls[i]
            && cfg.clock == Clock::Exact
            && !rd.as_ref().is_some_and(|rd| rd.in_access[i])
        {
            let (r, pc) = (Literal::usize_unsuffixed(r), hex(pc));
            before.extend(quote!(rt::stall(m, #r, #pc, #load);));
        }
        before.extend(p.tuple_before(i));
        let stmt = match &inst.op {
            Op::Insn(a) => {
                at = *a;
                let i = instrs.get(a);
                match cfg.insn {
                    Insn::Off => TokenStream::new(),
                    // A `bl`'s second half is in its first's line.
                    Insn::Comment => match i {
                        Some(Instr::ThumbBlLo { .. })
                            if matches!(
                                instrs.get(&a.wrapping_sub(2)),
                                Some(Instr::ThumbBlHi { .. })
                            ) =>
                        {
                            TokenStream::new()
                        }
                        _ => {
                            let s = i.map_or(String::new(), |i| asm_text(&instrs, *a, i, thumb));
                            let pc = format!("0x{:04x}_{:04x}", a >> 16, a & 0xFFFF);
                            crate::dwarf::note(format!("{pc} {s}").trim_end())
                        }
                    },
                    insn => {
                        let s = match i {
                            Some(i) if insn == Insn::Asm => asm_text(&instrs, *a, i, thumb),
                            Some(i) => format!("{i:?}"),
                            None => String::new(),
                        };
                        let a = hex(*a);
                        quote!(insn!(#a, #s);)
                    }
                }
            }
            // Without mGBA's model a block's cost is its constant alone.
            Op::Timing(_) if cfg.clock != Clock::Exact => TokenStream::new(),
            Op::Timing(t) => {
                let r = Literal::usize_unsuffixed(r);
                match *t {
                    Timing::Skipped => {
                        let c = cost(instrs[&at]);
                        let (base, s, n) = (lit(c.base), lit(c.seq), lit(c.nonseq));
                        quote!(rt::skipped(m, #r, #thumb, #base, #s, #n);)
                    }
                    Timing::Multiply(v) => {
                        let (signed, extra) = armv4t::cycles::mul_form(instrs[&at]).unwrap();
                        let (v, extra) = (p.arg(v), lit(extra));
                        quote!(rt::multiply(m, #v, #signed, #extra);)
                    }
                    Timing::Block(n) => {
                        let n = lit(n);
                        quote!(rt::block(m, #n);)
                    }
                }
            }
            Op::Exit(_) => {
                let x = &xs[&i];
                let kept =
                    matches!(x.plan, Plan::Call { target, .. } if ctx.kept.contains(&target));
                // A readable function's BIOS calls say which call.
                let bios = match x.plan {
                    Plan::Swi { .. } if cfg.readable => {
                        bios_note(gb, x).map(|s| crate::dwarf::note(&s))
                    }
                    _ => None,
                };
                let exit = match &rd {
                    Some(rd) => {
                        let (regs, flags) = &rd.syncs[&i];
                        // A return's r0 is the value it returns, if the
                        // prototype says it has one: written first, as
                        // the sync block would.
                        let value = match (&dn, regs.first()) {
                            (Some((d, _)), Some(&(0, v))) if d.returns() && returns(gb, x) => {
                                Some(p.arg(v))
                            }
                            _ => None,
                        };
                        let synced = if value.is_some() {
                            &regs[1..]
                        } else {
                            &regs[..]
                        };
                        // A call's lr is its return site, which `call!`
                        // writes itself.
                        let synced: Vec<(usize, ValueId)> = match x.plan {
                            Plan::Call { ret, .. } => synced
                                .iter()
                                .copied()
                                .filter(|&(r, v)| {
                                    r != 14 || crate::func::fold(&gb.block, v) != Some(ret)
                                })
                                .collect(),
                            _ => synced.to_vec(),
                        };
                        let mut sync = p.sync(&synced, flags);
                        sync.extend(carry.assign(&p, gb.key, i, regs));
                        let keep = carry.keep(&p, gb.key, i, regs);
                        let j = order[&i];
                        let note = match (x.plan, &dn) {
                            (Plan::Call { target, .. }, Some((d, _))) => d.call(target),
                            _ => None,
                        };
                        let mut note = note.map(|s| crate::dwarf::note(&s));
                        // A call that takes a message number, passed as a
                        // constant: the message.
                        if let Plan::Call { target, .. } = x.plan
                            && let Some(s) = message_note(ctx, gb, target, regs)
                        {
                            note.get_or_insert_default().extend(crate::dwarf::note(&s));
                        }
                        let ex = Exited {
                            sync,
                            keep,
                            note,
                            value,
                        };
                        exit_readable(&p, gb, x, ex, &mut |ret| then(j, ret))
                    }
                    None => lower_exit(&p, gb, x, ram, kept, then(order[&i], None).0),
                };
                quote!(#bios #exit)
            }
            _ => match p.value(i) {
                Some(s) => s,
                None => p.effect(i),
            },
        };
        let guard = match inst.op {
            Op::Exit(_) | Op::SetReg(..) | Op::SetUserReg(..) | Op::SetFlag(..) => inst.guard,
            Op::SetCpsr { .. } | Op::SetSpsr { .. } | Op::Store { .. } | Op::Timing(_) => {
                inst.guard
            }
            _ => None,
        };
        if !before.is_empty() || !stmt.is_empty() && run.as_ref().map(|r| r.0) != guard {
            flush(&mut run, &mut out);
        }
        out.extend(before);
        match guard {
            Some(g) => run
                .get_or_insert_with(|| (g, TokenStream::new()))
                .1
                .extend(stmt),
            None => out.extend(stmt),
        }
        if matches!(inst.op, Op::Exit(_)) {
            flush(&mut run, &mut out);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// How the readable form (`docs/mod-view-design.md`, "What the
/// readable-form units should become") prints one block: the guest
/// registers and flags stay in `Machine`, untouched until an exit, which
/// writes what the block set in its sync block; the block's values are its
/// locals; accesses carry their instruction's stall.
struct Readable {
    hints: Hints,
    /// Each exit's sync block: the last write of each register and flag
    /// before it.
    syncs: BTreeMap<usize, Sync>,
    /// Insts printed elsewhere (a moved write, a `push!`'s access) or not
    /// at all.
    skip: Vec<bool>,
    /// Where an instruction's stall would be printed that an access (or a
    /// `push!` or `pop!`) already carries.
    in_access: Vec<bool>,
    /// `push!` and `pop!`, by the index of their last access.
    multi: BTreeMap<usize, Multi>,
    /// Bitfield writes, by the index of their store.
    bit_writes: BTreeMap<usize, BitWrite>,
}

/// A load of a byte, its bits `bits` replaced, and a store of it to the
/// same address (`x.f = v;` of a bitfield `f`, `docs/mod-view-design.md`,
/// "The readable form"): `m.set_bits(..)` where the store's address names
/// that bitfield. Nothing that could see the order runs between the load
/// and the store, so all of it prints at the store.
struct BitWrite {
    load: usize,
    /// The insts between that compute the value stored, the last `value`.
    chain: Vec<usize>,
    value: usize,
    bits: (u32, u32),
    v: BitValue,
}

/// The value a [`BitWrite`] puts in its field.
#[derive(Clone, Copy)]
pub enum BitValue {
    Const(u32),
    /// Inst `i`'s value, of which the field takes the low bits.
    Of(usize),
}

impl BitWrite {
    /// The stores of `insts` that set one run of bits of what a byte load
    /// from their address read: `(x & k) | c`, `x & k` or `x | c`, where
    /// `k` keeps every bit of the byte but the run and `c` lies in it
    /// (for `x | c`, is all of it); or `(x & k) | e` of a computed `y`
    /// ([`BitWrite::computed`]). `quiet(i)`: no stall is printed at
    /// inst `i`; `ok(i)`: nor is it printed elsewhere already.
    fn find(
        insts: &[analysis::ir::Inst],
        fused: &[Option<u32>],
        via: &[Option<(u32, u32)>],
        quiet: &dyn Fn(usize) -> bool,
        ok: &dyn Fn(usize) -> bool,
    ) -> BTreeMap<usize, BitWrite> {
        use armv4t::sem::Width;
        let n = insts.len();
        let mut users: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, inst) in insts.iter().enumerate() {
            for x in inst.op.operands().into_iter().chain(inst.guard) {
                users[x.index()].push(i);
            }
        }
        let konst = |x: ValueId| match insts[x.index()].op {
            Op::Const(c) | Op::Pool(c) => Some(c),
            _ => None,
        };
        // `x op c`, either way round.
        let with_const = |i: usize, op: BinOp| match insts[i].op {
            Op::Bin(o, a, b) if o == op => match (konst(a), konst(b)) {
                (_, Some(c)) => Some((a.index(), c)),
                (Some(c), _) => Some((b.index(), c)),
                _ => None,
            },
            _ => None,
        };
        // A contiguous run of the byte's bits, as (lowest, count).
        let run = |m: u32| {
            let lo = m.trailing_zeros();
            let w = (m >> lo).trailing_ones();
            (m != 0 && m <= 0xff && (m >> lo) >> w == 0 && w < 8).then_some((lo, w))
        };
        let mut out = BTreeMap::new();
        for s in 0..n {
            let Op::Store {
                width: Width::Byte,
                addr,
                value,
            } = insts[s].op
            else {
                continue;
            };
            let x = value.index();
            let (mut chain, mut c) = (vec![x], 0);
            let mut at = x;
            // The value `y` of `(x & k) | e`, and whether `e` keeps bits
            // above the byte (which the store drops).
            let mut of = None;
            if let Some((a, k)) = with_const(x, BinOp::Or) {
                (at, c) = (a, k);
                if with_const(a, BinOp::And).is_some() {
                    chain.insert(0, a);
                }
            } else if let Op::Bin(BinOp::Or, a, b) = insts[x].op {
                let Some((a, e)) = [(a, b), (b, a)].into_iter().find_map(|(a, e)| {
                    let (_, k) = with_const(a.index(), BinOp::And)?;
                    let (lo, w) = run(!k & 0xff)?;
                    Some((a.index(), Self::computed(insts, e.index(), lo, w)?))
                }) else {
                    continue;
                };
                let (y, mut ins, wide) = e;
                // Each of `e`'s insts used only by the next, the last by
                // `x`; `x`, if wider than the byte, only by the store.
                let next = ins.iter().skip(1).copied().chain([x]);
                if ins
                    .iter()
                    .zip(next)
                    .any(|(&i, u)| users[i] != [u] || !ok(i))
                    || wide && users[x] != [s]
                    || ins.contains(&y)
                {
                    continue;
                }
                at = a;
                of = Some(y);
                chain.push(a);
                chain.append(&mut ins);
                chain.sort_unstable();
            }
            let (l, mask) = match with_const(at, BinOp::And) {
                Some((l, k)) => (l, !k & 0xff),
                // `x | c` alone: all of the run set.
                None if at != x => (at, c),
                None => continue,
            };
            let Some((lo, w)) = run(mask) else { continue };
            if c & !mask != 0 || of == Some(l) {
                continue;
            }
            let Op::Load {
                width: Width::Byte,
                addr: la,
            } = insts[l].op
            else {
                continue;
            };
            let quiet = (l + 1..s).all(|i| {
                chain.contains(&i)
                    || !matches!(
                        insts[i].op,
                        Op::Load { .. }
                            | Op::Store { .. }
                            | Op::Timing(_)
                            | Op::Exit(_)
                            | Op::YieldFlag(..)
                    ) && quiet(i)
            });
            // The chain's values used only by it, the store, insts after
            // the store and writes moved to the exits, directly or through
            // values each used once (printed inside their use).
            fn late(
                insts: &[analysis::ir::Inst],
                users: &[Vec<usize>],
                v: usize,
                s: usize,
            ) -> bool {
                users[v].iter().all(|&u| {
                    u > s
                        || matches!(insts[u].op, Op::SetReg(..) | Op::SetFlag(..))
                        || !insts[u].op.is_root() && users[u].len() == 1 && late(insts, users, u, s)
                })
            }
            let used_late = |v: usize, by: usize| {
                users[v].iter().all(|&u| {
                    u == by
                        || u > s
                        || matches!(insts[u].op, Op::SetReg(..) | Op::SetFlag(..))
                        || !insts[u].op.is_root()
                            && users[u].len() == 1
                            && late(insts, &users, u, s)
                })
            };
            // The `x & k`, used only by the `|`.
            let and = if of.is_some() { at } else { chain[0] };
            let inner = and != x && users[and] != [x];
            if !same_value(insts, la, addr)
                || !quiet
                || inner
                || fused[l].is_none()
                || fused[s].is_none()
                || via[s].is_some()
                || !ok(l)
                || !ok(s)
                || chain
                    .iter()
                    .chain([&l, &s])
                    .any(|&i| insts[i].guard.is_some())
                || !used_late(l, and)
                || !used_late(x, s)
            {
                continue;
            }
            out.insert(
                s,
                BitWrite {
                    load: l,
                    chain,
                    value: x,
                    bits: (lo, w),
                    v: of.map_or(BitValue::Const(c >> lo), BitValue::Of),
                },
            );
        }
        out
    }

    /// `e`, the bits `lo..lo + w` of a byte set from a value `y`, as `y`,
    /// the insts of `e` in order and whether `e` keeps `y`'s bits above
    /// the byte: `(y & m) << lo`, `(y << lo) & (m << lo)`, `y & m` (`lo`
    /// 0), or `y << lo` for bits that reach the byte's top, where `m` is
    /// `w` ones.
    fn computed(
        insts: &[analysis::ir::Inst],
        e: usize,
        lo: u32,
        w: u32,
    ) -> Option<(usize, Vec<usize>, bool)> {
        use armv4t::decode::ShiftType;
        let m = (1u32 << w) - 1;
        let konst = |x: ValueId| match insts[x.index()].op {
            Op::Const(c) | Op::Pool(c) => Some(c),
            _ => None,
        };
        // `y & k`, either way round.
        let and = |i: usize, k: u32| match insts[i].op {
            Op::Bin(BinOp::And, a, b) if konst(b) == Some(k) => Some(a.index()),
            Op::Bin(BinOp::And, a, b) if konst(a) == Some(k) => Some(b.index()),
            _ => None,
        };
        let shl = |i: usize| match insts[i].op {
            Op::Shift {
                kind: ShiftType::Lsl,
                v,
                amount,
            } if konst(amount) == Some(lo) => Some(v.index()),
            _ => None,
        };
        let y = |i: usize| konst(ValueId(i as u32)).is_none().then_some(i);
        if lo == 0 {
            let y = and(e, m).and_then(y)?;
            return Some((y, vec![e], false));
        }
        if let Some(a) = shl(e) {
            if let Some(v) = and(a, m).and_then(y) {
                return Some((v, vec![a, e], false));
            }
            return (lo + w == 8)
                .then(|| y(a))
                .flatten()
                .map(|v| (v, vec![e], true));
        }
        let a = and(e, m << lo)?;
        let v = shl(a).and_then(y)?;
        Some((v, vec![a, e], false))
    }

    /// The write as `m.set_bits(..)`, else its insts' own statements, in
    /// order.
    fn print(&self, p: &Printer, store: usize) -> TokenStream {
        if let Some(t) = p.set_bits(self.load, self.value, store, self.bits, self.v) {
            return t;
        }
        let mut out = TokenStream::new();
        for &i in std::iter::once(&self.load).chain(&self.chain) {
            out.extend(p.value(i));
        }
        out.extend(p.effect(store));
        out
    }
}

/// The registers and flags an exit's sync block writes, with their values.
type Sync = (Vec<(usize, ValueId)>, Vec<(Flag, ValueId)>);

/// A Thumb `push` or `pop` (an `stmdb`/`ldmia` of sp without pc) as one
/// macro.
struct Multi {
    at: u32,
    load: bool,
    /// The first access's address: the rest are at 4, 8, ... past it.
    base: ValueId,
    accesses: Vec<usize>,
}

impl Multi {
    fn print(&self, p: &Printer) -> TokenStream {
        let (at, base) = (p.at(self.at), p.arg(self.base));
        if self.load {
            let n = Literal::usize_unsuffixed(self.accesses.len());
            let names = self.accesses.iter().map(|&i| {
                if p.used(i) {
                    let id = p.id(ValueId(i as u32));
                    quote!(#id)
                } else {
                    quote!(_)
                }
            });
            quote!(let [#(#names),*] = pop!(m, #at #base, #n);)
        } else {
            let vs = self.accesses.iter().map(|&i| match p.op(i) {
                Op::Store { value, .. } => p.arg(value),
                _ => unreachable!(),
            });
            quote!(push!(m, #at #base, [#(#vs),*]);)
        }
    }
}

impl Readable {
    /// The plan for `gb` (`stalls`: where the play view prints each
    /// stall), or `None` where the block can't keep its registers out of
    /// `Machine` until its exits (ARM code, a mode or PSR access, a guarded
    /// write, a read of a register the block wrote), which the play view's
    /// form prints. `word`: the code's word at an address, for the pool
    /// loads folded into the access after them.
    fn of(
        gb: &GBlock,
        instrs: &BTreeMap<u32, &Instr>,
        stalls: &[Option<(u32, bool)>],
        word: &dyn Fn(u32) -> Option<u32>,
    ) -> Option<Readable> {
        let insts = &gb.block.insts;
        let n = insts.len();
        if !gb.block.thumb {
            return None;
        }
        let (mut regs, mut flags) = (0u16, 0u8);
        for inst in insts {
            match inst.op {
                Op::SetCpsr { .. }
                | Op::SetSpsr { .. }
                | Op::GetSpsr
                | Op::GetCpsr
                | Op::GetUserReg(_)
                | Op::SetUserReg(..) => return None,
                Op::SetReg(..) | Op::SetFlag(..) if inst.guard.is_some() => return None,
                Op::SetReg(r, _) => regs |= 1 << r.index(),
                Op::SetFlag(f, _) => flags |= 1 << f.index(),
                Op::GetReg(r) if regs & 1 << r.index() != 0 => return None,
                Op::GetFlag(f) if flags & 1 << f.index() != 0 => return None,
                _ => {}
            }
        }
        // The writes, moved to the exits after them.
        let mut moved: Vec<Option<Vec<(usize, bool)>>> = vec![None; n];
        let (mut regs, mut flags) = (BTreeMap::new(), BTreeMap::new());
        let mut syncs = BTreeMap::new();
        let mut skip = vec![false; n];
        for (i, inst) in insts.iter().enumerate() {
            match inst.op {
                Op::SetReg(r, x) => {
                    regs.insert(r.index(), (i, x));
                }
                Op::SetFlag(f, x) => {
                    flags.insert(f.index(), (i, x));
                }
                Op::Exit(_) => {
                    let cond = inst.guard.is_some();
                    for &(j, _) in regs.values().chain(flags.values()) {
                        moved[j].get_or_insert_with(Vec::new).push((i, cond));
                    }
                    let r = regs.iter().map(|(&r, &(_, x))| (r, x)).collect();
                    let f = flags.iter().map(|(&f, &(_, x))| (Flag::ALL[f], x));
                    syncs.insert(i, (r, f.collect()));
                    continue;
                }
                _ => continue,
            }
            moved[i].get_or_insert_with(Vec::new);
            skip[i] = true;
        }
        // Accesses with their stall: an instruction's span runs to the next
        // instruction or exit, where the play view prints its stall.
        let mut fused = vec![None; n];
        let mut bound = vec![false; n];
        let mut in_access = vec![false; n];
        let mut rotated = vec![None; n];
        let mut operands = vec![None; n];
        let mut multi = BTreeMap::new();
        let marks: Vec<usize> = (0..n)
            .filter(|&i| matches!(insts[i].op, Op::Insn(_) | Op::Exit(_)))
            .collect();
        for w in marks.windows(2) {
            let (s, e) = (w[0], w[1]);
            let Op::Insn(at) = insts[s].op else { continue };
            let Some((pc, load)) = stalls[e] else {
                continue;
            };
            if pc != at.wrapping_add(4) {
                continue;
            }
            let span = s + 1..e;
            let accesses: Vec<usize> = span
                .clone()
                .filter(|&i| matches!(insts[i].op, Op::Load { .. } | Op::Store { .. }))
                .collect();
            let timing: Vec<usize> = span
                .filter(|&i| matches!(insts[i].op, Op::Timing(_)))
                .collect();
            let kind_ok = |i: usize| {
                insts[i].guard.is_none()
                    && match insts[i].op {
                        Op::Load { .. } => load,
                        Op::Store { .. } => !load,
                        _ => false,
                    }
            };
            match (&accesses[..], &timing[..]) {
                (&[a], []) if kind_ok(a) => {
                    fused[a] = Some(at);
                    in_access[e] = true;
                    if let Some(j) = rotate_of(insts, a, e) {
                        rotated[j] = Some(a);
                        bound[j] = true;
                    }
                }
                (_, &[t]) => {
                    let Some(m) = Multi::of(gb, instrs, at, t, &accesses, load) else {
                        continue;
                    };
                    if !accesses.iter().all(|&a| kind_ok(a)) {
                        continue;
                    }
                    skip[t] = true;
                    for (j, &a) in accesses.iter().enumerate() {
                        skip[a] = true;
                        bound[a] = load;
                        // What the macro prints of each: the base, and a
                        // store's value.
                        let mut o: Vec<ValueId> = (j == 0).then_some(m.base).into_iter().collect();
                        if let Op::Store { value, .. } = insts[a].op {
                            o.push(value);
                        }
                        operands[a] = Some(o);
                    }
                    in_access[e] = true;
                    // Printed where its last access was: its base and
                    // values are bound by then.
                    multi.insert(*accesses.last().unwrap(), m);
                }
                _ => {}
            }
        }
        let via = fold_pools(insts, &fused, stalls, &in_access, &mut skip, word);
        let quiet = |i: usize| stalls[i].is_none() || in_access[i];
        let ok = |i: usize| !skip[i] && quiet(i);
        let bit_writes = BitWrite::find(insts, &fused, &via, &quiet, &ok);
        for (&s, b) in &bit_writes {
            for &i in [b.load, s].iter().chain(&b.chain) {
                skip[i] = true;
            }
        }
        Some(Readable {
            hints: Hints {
                moved,
                fused,
                bound,
                rotated,
                operands,
                via,
                carried: 0,
                index: Vec::new(),
            },
            syncs,
            skip,
            in_access,
            multi,
            bit_writes,
        })
    }
}

/// Whether `a` and `b` are the same value: one inst, equal constants, or
/// the same operator of the same values. (A readable block reads each
/// register before it writes it, so its reads of one register are one
/// value.)
fn same_value(insts: &[analysis::ir::Inst], a: ValueId, b: ValueId) -> bool {
    a == b
        || match (insts[a.index()].op, insts[b.index()].op) {
            (Op::Const(x) | Op::Pool(x), Op::Const(y) | Op::Pool(y)) => x == y,
            (Op::GetReg(x), Op::GetReg(y)) => x == y,
            (Op::Bin(o, a1, b1), Op::Bin(p, a2, b2)) => {
                o == p && same_value(insts, a1, a2) && same_value(insts, b1, b2)
            }
            _ => false,
        }
}

/// The pool loads (`ldr rN, =X`) folded into the access after them,
/// `m.get_via`: for each access, its pool load's instruction address and
/// literal. A pool load folds if const-prop has taken its value (nothing
/// names it), the next access uses it as its address (within an
/// immediate offset), and nothing that could see the order runs between
/// them: no other access, timing, stall or exit. Folded loads are added to
/// `skip`.
fn fold_pools(
    insts: &[analysis::ir::Inst],
    fused: &[Option<u32>],
    stalls: &[Option<(u32, bool)>],
    in_access: &[bool],
    skip: &mut [bool],
    word: &dyn Fn(u32) -> Option<u32>,
) -> Vec<Option<(u32, u32)>> {
    let n = insts.len();
    let mut used = vec![false; n];
    for inst in insts {
        for x in inst.op.operands().into_iter().chain(inst.guard) {
            used[x.index()] = true;
        }
    }
    let konst = |x: ValueId| match insts[x.index()].op {
        Op::Const(c) | Op::Pool(c) => Some(c),
        _ => None,
    };
    // A word load from the code: the printer prints it as `m.pool` if
    // nothing names its value.
    let code_word = |i: usize| match insts[i].op {
        Op::Load {
            width: Width::Word,
            addr,
        } if fused[i].is_some() => konst(addr).filter(|l| (0x0800_0000..0x0e00_0000).contains(l)),
        _ => None,
    };
    let pool = |i: usize| code_word(i).filter(|_| !used[i]);
    let mut via = vec![None; n];
    for a in 0..n {
        let (Some(lit), Some(at)) = (pool(a), fused[a]) else {
            continue;
        };
        if skip[a] {
            continue;
        }
        let Some(w) = word(lit & !3) else { continue };
        let Some(b) = (a + 1..n).find(|&i| {
            matches!(
                insts[i].op,
                Op::Load { .. }
                    | Op::Store { .. }
                    | Op::Timing(_)
                    | Op::Exit(_)
                    | Op::YieldFlag(..)
            ) || stalls[i].is_some() && !in_access[i]
        }) else {
            continue;
        };
        let target = match insts[b].op {
            Op::Load { addr, .. } | Op::Store { addr, .. } => konst(addr),
            _ => None,
        };
        // (A load from a ROM word after it, `ldr r0, [r0]`, may print as
        // its own `m.pool`.)
        if fused[b].is_none()
            || code_word(b).is_some()
            || skip[b]
            || insts[b].guard.is_some()
            || (a + 1..b).any(|i| stalls[i].is_some() && !in_access[i])
            || !target.is_some_and(|t| t.wrapping_sub(w) < 0x80)
        {
            continue;
        }
        skip[a] = true;
        via[b] = Some((at, lit));
    }
    via
}

/// The rotate of load `l`'s value by its address's low bits (a misaligned
/// `ldr` or `ldrh`, `armv4t::sem::arm::load_word_rotated`), before `e`, if
/// it is the load's one use.
fn rotate_of(insts: &[analysis::ir::Inst], l: usize, e: usize) -> Option<usize> {
    let Op::Load { width, addr } = insts[l].op else {
        return None;
    };
    let mask = match width {
        armv4t::sem::Width::Half => 1,
        armv4t::sem::Width::Word => 3,
        armv4t::sem::Width::Byte => return None,
    };
    let lv = ValueId(l as u32);
    let users: Vec<usize> = (l + 1..insts.len())
        .filter(|&i| insts[i].op.operands().contains(&lv))
        .collect();
    let &[j] = &users[..] else { return None };
    let konst = |x: ValueId| match insts[x.index()].op {
        Op::Const(c) | Op::Pool(c) => Some(c),
        _ => None,
    };
    let Op::Shift {
        kind: armv4t::sem::ShiftType::Ror,
        v,
        amount,
    } = insts[j].op
    else {
        return None;
    };
    let Op::Shift {
        kind: armv4t::sem::ShiftType::Lsl,
        v: low,
        amount: three,
    } = insts[amount.index()].op
    else {
        return None;
    };
    let ok = v == lv
        && j < e
        && insts[j].guard.is_none()
        && konst(three) == Some(3)
        && matches!(insts[low.index()].op, Op::Bin(BinOp::And, a, k) if a == addr && konst(k) == Some(mask));
    ok.then_some(j)
}

impl Multi {
    /// The `push` or `pop` at `at` whose `Timing::Block` is inst `t` and
    /// whose accesses are `accesses`, if it is one.
    fn of(
        gb: &GBlock,
        instrs: &BTreeMap<u32, &Instr>,
        at: u32,
        t: usize,
        accesses: &[usize],
        load: bool,
    ) -> Option<Multi> {
        let insts = &gb.block.insts;
        let sp_block = matches!(
            instrs.get(&at),
            Some(Instr::Block { rn, writeback: true, psr: false, cond: armv4t::decode::Cond::Al, load: l, .. })
                if *rn == armv4t::decode::Reg::Sp && *l == load
        );
        let k = accesses.len();
        if !sp_block
            || k == 0
            || insts[t].guard.is_some()
            || insts[t].op != Op::Timing(Timing::Block(k as u32))
            || accesses[0] < t
        {
            return None;
        }
        let addr = |i: usize| match insts[i].op {
            Op::Load {
                width: armv4t::sem::Width::Word,
                addr,
            }
            | Op::Store {
                width: armv4t::sem::Width::Word,
                addr,
                ..
            } => Some(addr),
            _ => None,
        };
        // Each address 4 past the one before, or 4 × j past the first.
        let base = addr(accesses[0])?;
        let mut prev = base;
        for (j, &a) in accesses.iter().enumerate().skip(1) {
            let x = addr(a)?;
            let ok = match insts[x.index()].op {
                Op::Bin(BinOp::Add, b, c) => {
                    let off = crate::func::fold(&gb.block, c);
                    (b == prev && off == Some(4)) || (b == base && off == Some(4 * j as u32))
                }
                _ => false,
            };
            if !ok {
                return None;
            }
            prev = x;
        }
        Some(Multi {
            at,
            load,
            base,
            accesses: accesses.to_vec(),
        })
    }
}

/// The registers a readable structured function holds in Rust locals
/// across its blocks (`docs/mod-view-design.md`, gap 4): a loop's variable
/// that the guest keeps in a register over the back edge's tick. A block
/// reads such a register as the local `rN`, which holds what `Machine` does
/// at the block's start; each tick into a block that reads it (or passes it
/// on) sets the local where its sync block writes the register, and checks
/// it after a yield taken in place (`tick!`'s `; 0 = r0`), unwinding if
/// the IRQ left the register changed. Only ticks carry: a call, a return or
/// a block in the play view's form ends a local, as does the function's
/// entry and a collapsed region (whose `match pc` the compiler can't see
/// through).
#[derive(Default)]
struct Carry {
    /// Each block's carried registers at its start, a bit each.
    at: BTreeMap<u32, u16>,
    /// For each tick (block, exit inst) into a block with some: the
    /// registers whose locals its sync block sets, and those the block
    /// after it carries.
    exits: BTreeMap<Tick, (u16, u16)>,
    /// Every register carried somewhere: the locals declared.
    all: u16,
    /// The loops printed as `for` (see [`For`]), by label.
    fors: BTreeMap<usize, For>,
    /// The ticks into a `for`'s block, with its variable's register: its
    /// local isn't set there, the `for` sets it.
    ivs: BTreeMap<Tick, usize>,
    /// The registers each block starts with a known constant in (see
    /// [`known`]).
    known: BTreeMap<u32, BTreeMap<usize, u32>>,
}

/// Forwards, the registers that hold the same constant at a block's start
/// whichever way it is entered: set to it, or carried on, by the sync
/// block of every edge in, each a readable block's tick or its call to a
/// function that keeps r4-r11 and sp (which carries only those). Nothing
/// is known at the entry or after any other edge. A resume lands at a
/// block's start with the state an edge in left.
fn known(entry: u32, steps: &[Step]) -> BTreeMap<u32, BTreeMap<usize, u32>> {
    type Regs = BTreeMap<usize, u32>;
    // `None`: no edge in seen yet.
    let mut at: BTreeMap<u32, Option<Regs>> = BTreeMap::new();
    loop {
        let mut next: BTreeMap<u32, Option<Regs>> = BTreeMap::new();
        next.insert(entry, Some(Regs::new()));
        for s in steps {
            let into = match (&s.sets, at.get(&s.from)) {
                (None, _) => Regs::new(),
                // Its block not reached yet.
                (Some(_), None | Some(None)) => continue,
                (Some((sets, call)), Some(Some(k))) => {
                    let mut out = k.clone();
                    for &(r, v) in sets {
                        let v = match v {
                            Val::Const(c) => Some(c),
                            Val::Reg(q) => k.get(&q).copied(),
                            Val::Other => None,
                        };
                        match v {
                            Some(c) => out.insert(r, c),
                            None => out.remove(&r),
                        };
                    }
                    if *call {
                        out.retain(|&r, _| crate::saves::KEPT & 1 << r != 0);
                    }
                    out
                }
            };
            let slot = next.entry(s.to).or_insert(None);
            *slot = Some(match slot.take() {
                None => into,
                Some(a) => a
                    .into_iter()
                    .filter(|(r, c)| into.get(r) == Some(c))
                    .collect(),
            });
        }
        if next == at {
            break;
        }
        at = next;
    }
    at.into_iter()
        .filter_map(|(k, v)| Some((k, v?)))
        .filter(|(_, v)| !v.is_empty())
        .collect()
}

/// A block's exit: its key and the exit's inst.
type Tick = (u32, usize);

/// A loop of one block whose tick back to itself is taken while a
/// register, its variable, steps through a range: `for rN in lo..=hi`
/// (`.rev()` down, `.step_by(k)`), entered from one tick that sets the
/// variable to a constant. The branch back stays as the block has it, and
/// the block's exit after it leaves the loop at the range's last value, so
/// the range is proven by running the block's IR over it, with what else
/// its branch reads constant from the entry: at most [`For::MAX`] times.
struct For {
    reg: usize,
    range: TokenStream,
}

impl For {
    const MAX: usize = 4096;
}

/// One of the plan's edges, for [`Carry`]: its block and exit inst, its
/// target, whether it is a readable block's tick, and the registers that
/// exit's sync writes.
struct Step {
    from: u32,
    exit: usize,
    to: u32,
    tick: bool,
    writes: u16,
    /// For a tick, or a call to a function that keeps r4-r11 and sp
    /// ([`crate::saves`]) back to the block after it: what the sync block
    /// writes each register, and whether it is the call.
    sets: Option<(Vec<(usize, Val)>, bool)>,
}

/// What a sync block writes a register: a constant, another register's
/// value at the block's start, or something else.
#[derive(Clone, Copy)]
enum Val {
    Const(u32),
    Reg(usize),
    Other,
}

fn bits(m: u16) -> impl Iterator<Item = usize> {
    (0..16).filter(move |&i| m & 1 << i != 0)
}

impl Carry {
    fn of(ctx: &Ctx, cfg: &Config, f: &Func, g: &Cfg, p: &Planned) -> Carry {
        if !cfg.readable {
            return Carry::default();
        }
        let mut steps: Vec<Step> = Vec::new();
        let mut reads: BTreeMap<u32, u16> = BTreeMap::new();
        let mut seq: Vec<(&[Node], bool)> = vec![(&p.tree, false)];
        while let Some((nodes, region)) = seq.pop() {
            for n in nodes {
                let (node, outs) = match n {
                    Node::Block { node, exits } => (*node, exits),
                    Node::Loop { body, .. } | Node::Labeled { body, .. } => {
                        seq.push((body, region));
                        continue;
                    }
                    Node::Region { blocks, exits, .. } => {
                        seq.push((blocks, true));
                        for e in exits {
                            steps.push(Step {
                                from: 0,
                                exit: 0,
                                to: g.keys[e.to],
                                tick: false,
                                writes: 0,
                                sets: None,
                            });
                            if let How::Inline(s) = &e.how {
                                seq.push((s, region));
                            }
                        }
                        continue;
                    }
                };
                let key = g.keys[node];
                let gb = &f.blocks[&key];
                let rd = if region { None } else { readable_plan(ctx, gb) };
                if rd.is_some() {
                    let r = gb.block.insts.iter().fold(0u16, |a, i| match i.op {
                        Op::GetReg(r) if r.index() < 15 => a | 1 << r.index(),
                        _ => a,
                    });
                    reads.insert(key, r);
                }
                let xs = exits(ctx, gb);
                for (x, o) in xs.iter().zip(outs) {
                    let tick = rd.is_some()
                        && matches!(x.plan, Plan::Static(_))
                        && o.edges.len() == 1
                        && !o.fallback;
                    let writes = rd.as_ref().map_or(0, |rd| {
                        rd.syncs[&x.index]
                            .0
                            .iter()
                            .fold(0u16, |a, &(r, _)| a | 1 << r)
                    });
                    let kept =
                        matches!(x.plan, Plan::Call { target, .. } if ctx.kept.contains(&target));
                    let sets = rd.as_ref().filter(|_| tick || kept).map(|rd| {
                        let val = |v: ValueId| match gb.block.insts[v.index()].op {
                            Op::GetReg(q) if q.index() < 15 => Val::Reg(q.index()),
                            _ => crate::func::fold(&gb.block, v).map_or(Val::Other, Val::Const),
                        };
                        let r = rd.syncs[&x.index].0.iter().map(|&(r, v)| (r, val(v)));
                        (r.collect(), !tick)
                    });
                    for e in &o.edges {
                        steps.push(Step {
                            from: key,
                            exit: x.index,
                            to: g.keys[e.to],
                            tick,
                            writes,
                            sets: sets.clone(),
                        });
                        if let How::Inline(s) = &e.how {
                            seq.push((s, region));
                        }
                    }
                }
            }
        }
        // Forwards, the registers whose local holds `Machine`'s value at
        // each block's start: carried on or set by the tick of every edge
        // in. Nothing at the entry or after any other edge.
        let mut valid: BTreeMap<u32, u16> = f.blocks.keys().map(|&k| (k, !0)).collect();
        valid.insert(f.entry, 0);
        loop {
            let mut next: BTreeMap<u32, u16> = f.blocks.keys().map(|&k| (k, !0)).collect();
            next.insert(f.entry, 0);
            for s in &steps {
                let v = if s.tick { valid[&s.from] | s.writes } else { 0 };
                *next.get_mut(&s.to).unwrap() &= v;
            }
            if next == valid {
                break;
            }
            valid = next;
        }
        // Backwards, the ones carried: valid there, and read by the
        // block or carried on by one of its ticks without its sync
        // writing them.
        let mut at: BTreeMap<u32, u16> = f.blocks.keys().map(|&k| (k, 0)).collect();
        loop {
            let mut next: BTreeMap<u32, u16> = reads.clone();
            for k in f.blocks.keys() {
                next.entry(*k).or_insert(0);
            }
            for s in steps.iter().filter(|s| s.tick) {
                *next.get_mut(&s.from).unwrap() |= at[&s.to] & !s.writes;
            }
            for (k, v) in next.iter_mut() {
                *v &= valid[k];
            }
            if next == at {
                break;
            }
            at = next;
        }
        let mut c = Carry::default();
        for s in steps.iter().filter(|s| s.tick && at[&s.to] != 0) {
            c.exits
                .insert((s.from, s.exit), (at[&s.to] & s.writes, at[&s.to]));
        }
        // `for` loops.
        let mut loops: Vec<&[Node]> = vec![&p.tree];
        while let Some(nodes) = loops.pop() {
            for n in nodes {
                match n {
                    Node::Loop { label, body } => {
                        if let Some((f, back, entry)) =
                            For::of(ctx, f, g, *label, body, &steps, &at)
                        {
                            c.ivs.insert(back, f.reg);
                            c.ivs.insert(entry, f.reg);
                            c.fors.insert(*label, f);
                        }
                        loops.push(body);
                    }
                    Node::Labeled { body, .. } => loops.push(body),
                    Node::Block { exits, .. } => {
                        for e in exits.iter().flat_map(|o| &o.edges) {
                            if let How::Inline(s) = &e.how {
                                loops.push(s);
                            }
                        }
                    }
                    Node::Region { .. } => {}
                }
            }
        }
        c.known = known(f.entry, &steps);
        at.retain(|_, v| *v != 0);
        // The locals some tick sets: a `for`'s variable is its binding.
        c.all = c.exits.keys().fold(0, |a, &(k, x)| a | c.set(k, x));
        c.at = at;
        c
    }

    /// The registers whose locals tick `exit` of block `key` sets.
    fn set(&self, key: u32, exit: usize) -> u16 {
        let iv = self.ivs.get(&(key, exit)).map_or(0, |&r| 1 << r);
        self.exits.get(&(key, exit)).map_or(0, |e| e.0 & !iv)
    }

    /// The registers whose value tick `exit` of block `key` names: the
    /// ones it sets and a `for`'s variable.
    fn named(&self, key: u32, exit: usize) -> u16 {
        self.exits.get(&(key, exit)).map_or(0, |e| e.0)
    }

    /// The statement that sets them, from the sync's `regs`.
    fn assign(&self, p: &Printer, key: u32, exit: usize, regs: &[(usize, ValueId)]) -> TokenStream {
        let set = self.set(key, exit);
        let (ls, vs): (Vec<_>, Vec<_>) = regs
            .iter()
            .filter(|&&(r, _)| set & 1 << r != 0)
            .map(|&(r, x)| (format_ident!("r{r}"), p.arg(x)))
            .unzip();
        match (&ls[..], &vs[..]) {
            ([], _) => quote!(),
            ([l], [v]) => quote!(#l = #v;),
            _ => quote!((#(#ls),*) = (#(#vs),*);),
        }
    }

    /// `tick!`'s list of the registers carried on after `exit`, each with
    /// its local (a `for`'s variable with the value it steps to).
    fn keep(&self, p: &Printer, key: u32, exit: usize, regs: &[(usize, ValueId)]) -> TokenStream {
        let keep = self.exits.get(&(key, exit)).map_or(0, |e| e.1);
        if keep == 0 {
            return quote!();
        }
        let iv = self.ivs.get(&(key, exit)).copied();
        let rs = bits(keep).map(|r| {
            let n = Literal::usize_unsuffixed(r);
            match regs.iter().find(|&&(q, _)| Some(q) == iv && q == r) {
                Some(&(_, x)) => {
                    let x = p.arg(x);
                    quote!(#n = #x)
                }
                None => {
                    let l = format_ident!("r{r}");
                    quote!(#n = #l)
                }
            }
        });
        quote!(; #(#rs),*)
    }

    /// The locals' declarations.
    fn declare(&self) -> TokenStream {
        let ls = bits(self.all).map(|r| format_ident!("r{r}"));
        quote!(#(let mut #ls: u32;)*)
    }
}

impl For {
    /// Loop `label` (`body`) as a `for`, if it is one, with its back
    /// edge's and its entry's ticks.
    fn of(
        ctx: &Ctx,
        f: &Func,
        g: &Cfg,
        label: usize,
        body: &[Node],
        steps: &[Step],
        at: &BTreeMap<u32, u16>,
    ) -> Option<(For, Tick, Tick)> {
        let [Node::Block { node, exits: outs }] = body else {
            return None;
        };
        let h = g.keys[*node];
        let gb = &f.blocks[&h];
        let insts = &gb.block.insts;
        let rd = readable_plan(ctx, gb)?;
        let xs = exits(ctx, gb);
        // Its first exit the branch back, its others leaving.
        let back = |o: &Out| o.edges.iter().any(|e| e.how == How::Continue(label));
        let (x0, o0) = (xs.first()?, outs.first()?);
        if !matches!(o0.edges[..], [Edge { to, how: How::Continue(l) }] if to == *node && l == label)
            || outs[1..].iter().any(back)
        {
            return None;
        }
        let guard = insts[x0.index].guard?;
        // One tick in from outside.
        let ins: Vec<&Step> = steps.iter().filter(|s| s.to == h && s.from != h).collect();
        let [entry] = ins[..] else { return None };
        let back_step = steps.iter().find(|s| s.from == h && s.exit == x0.index)?;
        if !entry.tick || !back_step.tick {
            return None;
        }
        let carried = at[&h];
        // The blocks printed inside the loop: its block, and those its
        // exits inline.
        let mut inside = vec![h];
        let mut todo: Vec<&[Out]> = vec![outs];
        while let Some(outs) = todo.pop() {
            for e in outs.iter().flat_map(|o| &o.edges) {
                let How::Inline(seq) = &e.how else { continue };
                let mut nodes: Vec<&Node> = seq.iter().collect();
                while let Some(n) = nodes.pop() {
                    match n {
                        Node::Block { node, exits } => {
                            inside.push(g.keys[*node]);
                            todo.push(exits);
                        }
                        Node::Loop { body, .. } | Node::Labeled { body, .. } => nodes.extend(body),
                        Node::Region { blocks, exits, .. } => {
                            nodes.extend(blocks);
                            for e in exits {
                                inside.push(g.keys[e.to]);
                                if let How::Inline(s) = &e.how {
                                    nodes.extend(s);
                                }
                            }
                        }
                    }
                }
            }
        }
        // The variable: carried, and stepped by the branch back's sync.
        let (back_regs, _) = &rd.syncs[&x0.index];
        let pg = &f.blocks[&entry.from];
        let pd = readable_plan(ctx, pg)?;
        let (entry_regs, _) = &pd.syncs[&entry.exit];
        let konst = |x: ValueId| crate::func::fold(&pg.block, x);
        // What the entry sets the block's other carried registers to.
        let start: BTreeMap<usize, u32> = entry_regs
            .iter()
            .filter_map(|&(r, x)| Some((r, konst(x)?)))
            .collect();
        for &(r, next) in back_regs {
            // Nothing else printed in the loop names its local: a tick
            // that carries it would set the `for`'s binding.
            let out = steps.iter().any(|s| {
                inside.contains(&s.from)
                    && (s.from, s.exit) != (h, x0.index)
                    && at[&s.to] & 1 << r != 0
            }) || inside.iter().any(|&k| k != h && at[&k] & 1 << r != 0);
            if carried & 1 << r == 0 || out {
                continue;
            }
            let Some(&init) = start.get(&r) else { continue };
            // Every other register the block reads is carried round
            // unchanged (and the run below needs the ones its branch
            // reads to start constant).
            let ok = insts.iter().all(|i| match i.op {
                Op::GetReg(q) if q.index() != r => {
                    carried & 1 << q.index() != 0 && back_regs.iter().all(|&(w, _)| w != q.index())
                }
                Op::GetFlag(_) => false,
                _ => true,
            });
            if !ok {
                continue;
            }
            let run = |v: u32| -> Option<(u32, bool)> {
                let mut vals: Vec<Option<u32>> = vec![None; insts.len()];
                for (i, inst) in insts.iter().enumerate() {
                    vals[i] = match inst.op {
                        Op::GetReg(q) if q.index() == r => Some(v),
                        Op::GetReg(q) => start.get(&q.index()).copied(),
                        op => {
                            let known = op.operands().iter().all(|o| vals[o.index()].is_some());
                            if known {
                                op.eval_pure(
                                    |o| vals[o.index()].unwrap(),
                                    |o| vals[o.index()].unwrap() != 0,
                                )
                            } else {
                                None
                            }
                        }
                    };
                }
                Some((vals[next.index()]?, vals[guard.index()]? != 0))
            };
            // The values it takes, to the one whose branch isn't taken.
            let mut vs = vec![init];
            let range = loop {
                if vs.len() > For::MAX {
                    break None;
                }
                let cur = *vs.last().unwrap();
                let Some((nx, again)) = run(cur) else {
                    break None;
                };
                if !again {
                    break Some(());
                }
                vs.push(nx);
            };
            if range.is_none() || vs.len() < 2 {
                continue;
            }
            let step = vs[1].wrapping_sub(vs[0]) as i32;
            let even = vs
                .windows(2)
                .all(|w| w[1].wrapping_sub(w[0]) as i32 == step);
            let (first, last) = (vs[0], *vs.last().unwrap());
            let k = vs.len() as i64 - 1;
            let span = last as i64 - first as i64;
            if step == 0 || !even || span != k * step as i64 {
                continue;
            }
            let (lo, hi) = (
                crate::print::word(first.min(last), true),
                crate::print::word(first.max(last), false),
            );
            let by = step.unsigned_abs();
            let by = (by != 1).then(|| {
                let by = Literal::u32_unsuffixed(by);
                quote!(.step_by(#by))
            });
            let range = if step > 0 {
                quote!((#lo..=#hi) #by)
            } else {
                quote!((#lo..=#hi).rev() #by)
            };
            return Some((
                For { reg: r, range },
                (h, x0.index),
                (entry.from, entry.exit),
            ));
        }
        None
    }
}

/// [`Readable::of`] for `gb`, a ROM block.
fn readable_plan(ctx: &Ctx, gb: &GBlock) -> Option<Readable> {
    let instrs: BTreeMap<u32, &Instr> = gb.insns.iter().map(|(a, i)| (*a, i)).collect();
    let stalls = stalls_of(gb, &instrs, false);
    let word = |a: u32| {
        let o = a.wrapping_sub(ctx.base) as usize;
        let b = ctx.rom.get(o..o.checked_add(4)?)?;
        Some(u32::from_le_bytes(b.try_into().ok()?))
    };
    Readable::of(gb, &instrs, &stalls, &word)
}

/// What a readable exit prints besides its code ([`exit_readable`]).
struct Exited {
    /// Its sync block.
    sync: TokenStream,
    /// A tick's carried registers (see [`Carry`]).
    keep: TokenStream,
    /// A call's comment (`crate::dwarf`).
    note: Option<TokenStream>,
    /// A return's value, r0, which `sync` then leaves out ([`returns`]).
    value: Option<TokenStream>,
}

/// Whether exit `x` of `gb` is a return that `ret!` prints: out of the
/// function to the address the caller left in `lr` or on the stack (`bx
/// lr`, `pop {r0}; bx r0`, `pop {pc}`), not a jump through a register.
fn returns(gb: &GBlock, x: &ExitInfo) -> bool {
    let insts = &gb.block.insts;
    match (x.plan, insts[x.index].op) {
        (
            Plan::Dynamic {
                local: false,
                ret: None,
                ..
            },
            Op::Exit(Exit::Jump(t) | Exit::JumpExchange(t)),
        ) => matches!(insts[t.index()].op, Op::GetReg(Reg::LR) | Op::Load { .. }),
        _ => false,
    }
}

/// Whether nothing in `f` jumps back to its entry block, so the block
/// starts with the registers the function was called with.
fn entered_once(ctx: &Ctx, f: &Func) -> bool {
    f.blocks.values().all(|gb| {
        exits(ctx, gb).iter().all(|x| match x.plan {
            Plan::Static(k) | Plan::Rekey(k) => k != f.entry,
            Plan::Dynamic {
                local: true,
                target,
                ..
            } => match target {
                Some(k) => k != f.entry,
                None => ctx
                    .table_targets(x.at, x.thumb)
                    .is_some_and(|t| !t.contains(&f.entry)),
            },
            _ => true,
        })
    })
}

/// A readable block's exit (see [`Readable`]): its sync block first, then
/// the play view's statements, through `tick!`, `call!` and `ret!` where
/// it is one of those ([`Exited`]). `then`: what follows (given the return
/// site of a call that may go on only there).
fn exit_readable(
    p: &Printer,
    gb: &GBlock,
    x: &ExitInfo,
    ex: Exited,
    then: &mut dyn FnMut(Option<u32>) -> (TokenStream, bool),
) -> TokenStream {
    let Exited {
        sync,
        keep,
        note,
        value,
    } = ex;
    let n = exit_cost(p.clock(), gb, x);
    let yields: Vec<usize> = (0..x.index)
        .rev()
        .take_while(|&i| matches!(gb.block.insts[i].op, Op::YieldFlag(..)))
        .collect();
    let flags = p.yield_flags(&yields);
    // The macros' optional blocks: the sync, and the flags only a yield
    // writes.
    let tail = match (sync.is_empty(), flags.is_empty()) {
        (true, true) => quote!(),
        (_, true) => quote!(, { #sync }),
        _ => quote!(, { #sync }, { #flags }),
    };
    let Op::Exit(e) = gb.block.insts[x.index].op else {
        unreachable!()
    };
    let at = hex(x.at);
    match (x.plan, e) {
        // An idle back edge (native timing) takes the play view's exit.
        (Plan::Static(k), _) if x.idle == Idle::No || p.clock() != Clock::Native => {
            let (k, (t, _)) = (hex(k), then(None));
            quote! {
                tick!(m, pc = #k, #n #tail #keep);
                #t
            }
        }
        (Plan::Call { target, ret }, _) => {
            let (f, r) = (p.fn_ident(target), hex(ret));
            let (t, only) = then(Some(ret));
            let mac = if only { quote!(call) } else { quote!(call_any) };
            quote! {
                #note
                #mac!(m, pc, #f -> #r, #n #tail);
                #t
            }
        }
        (
            Plan::Dynamic {
                local: false,
                ret: None,
                ..
            },
            Exit::Jump(_) | Exit::JumpExchange(_),
        ) => {
            let _ = then(None);
            let t = match e {
                Exit::Jump(t) => {
                    let t = p.arg(t);
                    quote!(Exit::Jump(#t))
                }
                Exit::JumpExchange(t) => {
                    let t = p.arg(t);
                    quote!(Exit::Exchange(#t))
                }
                _ => unreachable!(),
            };
            let value = value.map(|v| quote!(, return #v));
            quote!(ret!(m, pc @ #at, #t, #n #value #tail);)
        }
        _ => {
            let (t, _) = then(None);
            let body = exit_body(p, gb, x, false, false, quote!(cycles), t);
            quote! {
                #sync
                let cycles: u32 = #n;
                #body
            }
        }
    }
}

/// A whole function, named for its entry.
pub fn lower_func(ctx: &Ctx, f: &Func) -> TokenStream {
    lower_func_named(ctx, f, &fn_name(f.entry))
}

/// A whole function, named `name`: structured if it has a plan
/// ([`structured`]), else v1's `loop { match pc }`.
pub fn lower_func_named(ctx: &Ctx, f: &Func, name: &Ident) -> TokenStream {
    lower_func_checked(ctx, f, name).0
}

/// [`lower_func_named`], and why its registers aren't in locals if
/// [`crate::locals::check`] failed it: then it is printed with them in
/// `Machine`.
pub fn lower_func_checked(ctx: &Ctx, f: &Func, name: &Ident) -> (TokenStream, Option<String>) {
    let plan = structured(ctx, f);
    let cfg = ctx.cfg_for(f.entry);
    let print = |loc: &Locals| match &plan {
        Some((g, p)) => lower_structured(ctx, &cfg, f, g, p, name, loc),
        None => lower_v1(ctx, &cfg, f, name, loc),
    };
    let loc = Locals::of(&cfg, f);
    let t = print(&loc);
    if !cfg.locals {
        return (t, None);
    }
    let why = match syn::parse2::<syn::ItemFn>(t.clone()) {
        Ok(item) => crate::locals::check(&loc, &item, ctx.names, &ctx.kept).err(),
        Err(e) => Some(format!("doesn't parse: {e}")),
    };
    match why {
        None => (t, None),
        Some(w) => (print(&Locals::OFF), Some(w)),
    }
}

/// The deepest a structured function's printed Rust may nest
/// ([`plan::Depth::indent`]); a deeper one stays v1.
pub const MAX_INDENT: usize = 64;

/// Why a function is printed in v1 form rather than structured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum V1 {
    /// Structuring is off ([`crate::func::Config::structured`]).
    Off,
    /// It runs from IWRAM: an overlay, whose blocks keep their guards.
    Overlay,
    /// It has no plan, or the plan failed the structure check.
    NoPlan,
    /// A block its edges don't reach from the entry.
    Unreachable,
    /// A block whose last exit is conditional or isn't its last inst.
    LastExit,
    /// Nested deeper than [`MAX_INDENT`].
    Deep,
    /// No exit leaves it ([`never_returns`]): a main loop. A rollback's
    /// restore ends netplay's guest stacks, and a structured function
    /// resumed past its entry runs block-timed until it returns, so one
    /// that never returns would stay out of compiled code for good; v1's
    /// keys resume it compiled.
    NeverReturns,
}

/// The plan `f` is printed from, if it is printed structured
/// (`docs/structured-codegen-design.md`, step 8): a ROM function whose
/// plan passed the structure check, every block reached from the entry,
/// each block ending in its one unconditional exit (which the plan's
/// fall-throughs rely on), nested at most [`MAX_INDENT`] deep.
pub fn structured(ctx: &Ctx, f: &Func) -> Option<(Cfg, Planned)> {
    plan_or_why(ctx, f).ok()
}

/// [`structured`], or why not.
pub fn plan_or_why(ctx: &Ctx, f: &Func) -> Result<(Cfg, Planned), V1> {
    if !ctx.cfg.structured {
        return Err(V1::Off);
    }
    if f.overlay.is_some() || ctx.base != analysis::program::ROM_BASE {
        return Err(V1::Overlay);
    }
    let last_exit = |gb: &GBlock| {
        let insts = &gb.block.insts;
        matches!(insts.last(), Some(i) if matches!(i.op, Op::Exit(_)) && i.guard.is_none())
    };
    if !f.blocks.values().all(last_exit) {
        return Err(V1::LastExit);
    }
    let (g, p) = plan::of(ctx, f);
    let p = p.map_err(|_| V1::NoPlan)?;
    if !g.unreachable.is_empty() {
        return Err(V1::Unreachable);
    }
    if p.depth().indent > MAX_INDENT {
        return Err(V1::Deep);
    }
    if never_returns(ctx, f) {
        return Err(V1::NeverReturns);
    }
    Ok((g, p))
}

/// Whether every exit of `f` goes on inside it: to one of its blocks, a
/// call (which comes back to its return site, or never does) or a `swi`
/// back to one. Such a function
/// (a game's main loop) is only ever left by an IRQ or the frame's end.
pub fn never_returns(ctx: &Ctx, f: &Func) -> bool {
    let mine = |k: u32| f.blocks.contains_key(&k);
    f.blocks.values().flat_map(|gb| exits(ctx, gb)).all(|x| {
        let table = ctx.table_targets(x.at, x.thumb);
        match x.plan {
            Plan::Static(k) | Plan::Rekey(k) => mine(k),
            // A call comes back to its return site or not at all (its
            // callee never returns, so the site isn't code).
            Plan::Call { .. } | Plan::CallVia { .. } | Plan::Dynamic { ret: Some(_), .. } => true,
            Plan::Dynamic {
                ret: None, target, ..
            } => match (target, table) {
                (_, Some(ts)) => !ts.is_empty() && ts.into_iter().all(mine),
                (Some(t), None) => mine(t),
                (None, None) => false,
            },
            Plan::Swi { next } => mine(next),
            Plan::Undefined => false,
        }
    })
}

/// A function printed from its plan: `loop`s, labeled blocks, `if`s and
/// `match`es over its blocks, each block's code v1's but for what follows
/// each edge's tick (`pc = K; continue` becomes a `break`, a `continue`, a
/// fall-through or the target's code). It is only ever called at its
/// entry; a key it doesn't take is returned to the dispatcher, as v1's `_`
/// arm does.
fn lower_structured(
    ctx: &Ctx,
    cfg: &Config,
    f: &Func,
    g: &Cfg,
    p: &Planned,
    name: &Ident,
    loc: &Locals,
) -> TokenStream {
    let s = Structured {
        ctx,
        cfg,
        f,
        g,
        loc,
        carry: Carry::of(ctx, cfg, f, g, p),
        dw: FnDwarf::of(ctx.names, f.entry)
            .filter(|_| cfg.readable)
            .map(|d| d.entered_once(entered_once(ctx, f))),
    };
    let body = s.seq(&p.tree);
    let (load, carried) = (loc.load(), s.carry.declare());
    quote! {
        pub fn #name(m: &mut M, mut pc: u32) -> Flow {
            #load
            #carried
            #body
        }
    }
}

struct Structured<'a> {
    ctx: &'a Ctx<'a>,
    cfg: &'a Config,
    f: &'a Func,
    g: &'a Cfg,
    loc: &'a Locals,
    carry: Carry,
    dw: Option<FnDwarf<'a>>,
}

fn label(kind: &str, node: usize) -> syn::Lifetime {
    syn::Lifetime::new(&format!("'{kind}{node}"), proc_macro2::Span::call_site())
}

impl Structured<'_> {
    fn seq(&self, seq: &[Node]) -> TokenStream {
        seq.iter().map(|n| self.node(n)).collect()
    }

    /// A block's code, each exit going on as its planned edges say.
    /// `region`: the label of the region it is in.
    fn block(&self, node: usize, exits: &[Out], region: Option<&syn::Lifetime>) -> TokenStream {
        let gb = &self.f.blocks[&self.g.keys[node]];
        let mut used = vec![false; exits.len()];
        let (ctx, cfg, loc, carry) = (self.ctx, self.cfg, self.loc, &self.carry);
        let dw = self.dw.as_ref().map(|d| (d, gb.key == self.f.entry));
        let code = lower_block_then(ctx, cfg, gb, false, loc, carry, dw, &mut |j, ret| {
            used[j] = true;
            let out = &exits[j];
            // A readable call that goes on only at its return site.
            if let (Some(r), [e], true) = (ret, &out.edges[..], out.fallback)
                && self.g.keys[e.to] == r
            {
                return (self.edge(e, region), true);
            }
            (self.edges(&out.edges, out.fallback, region), false)
        });
        // An exit whose code returns (a return, a jump the dispatcher
        // finds) has no edges in the plan.
        for (j, u) in used.iter().enumerate() {
            assert!(
                *u || exits[j].edges.is_empty(),
                "{:#x}: exit {j} of block {:#x} drops its edges",
                self.f.entry,
                gb.key
            );
        }
        code
    }

    fn node(&self, n: &Node) -> TokenStream {
        match n {
            Node::Block { node, exits } => self.block(*node, exits, None),
            Node::Loop { label: n, body } => {
                let (l, body) = (label("l", *n), self.seq(body));
                match self.carry.fors.get(n) {
                    // The block's exit leaves at the range's last value.
                    Some(For { reg, range }) => {
                        let r = format_ident!("r{reg}");
                        quote! {
                            #l: for #r in #range { #body }
                            unreachable!();
                        }
                    }
                    None => quote!(#l: loop { #body }),
                }
            }
            Node::Labeled { label: l, body } => {
                let (l, body) = (label("b", *l), self.seq(body));
                quote!(#l: { #body })
            }
            // v1's `loop { match pc }` over the region's blocks, entered
            // at the key the edge into it set; leaving it, `match pc`
            // picks the edge on.
            Node::Region {
                label: l,
                blocks,
                exits,
                ..
            } => {
                let l = label("r", *l);
                let arms = blocks.iter().map(|b| {
                    let Node::Block { node, exits } = b else {
                        unreachable!("a region holds blocks")
                    };
                    let (k, body) = (hex(self.g.keys[*node]), self.block(*node, exits, Some(&l)));
                    quote!(#k => { #body })
                });
                let out = self.edges(exits, true, None);
                let ret = self.loc.ret(Point::Return, quote!(pc));
                quote! {
                    #l: loop {
                        match pc {
                            #(#arms)*
                            _ => #ret,
                        }
                    }
                    #out
                }
            }
        }
    }

    /// What follows an exit's tick, with `pc` its key: its one edge, or
    /// `match pc` over its edges and, with `fallback`, the dispatcher.
    fn edges(&self, edges: &[Edge], fallback: bool, region: Option<&syn::Lifetime>) -> TokenStream {
        let ret = self.loc.ret(Point::Return, quote!(pc));
        match edges {
            [] => quote!(#ret;),
            [e] if !fallback => self.edge(e, region),
            _ => {
                let arms = edges.iter().map(|e| {
                    let (k, body) = (hex(self.g.keys[e.to]), self.edge(e, region));
                    quote!(#k => { #body })
                });
                quote! {
                    match pc {
                        #(#arms)*
                        _ => #ret,
                    }
                }
            }
        }
    }

    fn edge(&self, e: &Edge, region: Option<&syn::Lifetime>) -> TokenStream {
        match &e.how {
            How::Break(l) => {
                let l = label("b", *l);
                quote!(break #l;)
            }
            How::Continue(l) => {
                let l = label("l", *l);
                quote!(continue #l;)
            }
            How::Next => TokenStream::new(),
            How::Inline(seq) => self.seq(seq),
            How::Goto => {
                let l = region.expect("a goto inside a region");
                quote!(continue #l;)
            }
            How::Leave => {
                let l = region.expect("a leave inside a region");
                quote!(break #l;)
            }
        }
    }
}

/// A whole function as v1's `loop { match pc }` over its blocks.
pub fn lower_v1(ctx: &Ctx, cfg: &Config, f: &Func, name: &Ident, loc: &Locals) -> TokenStream {
    let ret = loc.ret(Point::Return, quote!(pc));
    let dw = FnDwarf::of(ctx.names, f.entry)
        .filter(|_| cfg.readable)
        .map(|d| d.entered_once(entered_once(ctx, f)));
    let arms = f.blocks.values().map(|gb| {
        let k = hex(gb.key);
        let ram = f.overlay.is_some();
        let dw = dw.as_ref().map(|d| (d, gb.key == f.entry));
        let body = lower_block_then(
            ctx,
            cfg,
            gb,
            ram,
            loc,
            &Carry::default(),
            dw,
            &mut |_, _| (quote!(continue;), false),
        );
        // An overlay block runs only while IWRAM holds the bytes it was
        // compiled from; a store (its function's own, even) may change
        // them between blocks. Only that block goes to the fallback,
        // unless IWRAM holds one of its variants'.
        let Some(o) = f.block_overlay(gb.key) else {
            return quote!(#k => { #body });
        };
        let variants = f.variants.get(&gb.key).into_iter().flatten();
        let versions = std::iter::once(body)
            .chain(variants.map(|v| lower_block(ctx, cfg, &v.block, true, loc)));
        let versions = versions.enumerate().map(|(i, body)| {
            let o = Literal::usize_unsuffixed(o as usize + i);
            quote! {
                if rt::overlay_valid(m, #o) {
                    #body
                }
            }
        });
        quote!(#k => {
            #(#versions)*
            #ret;
        })
    });
    let load = loc.load();
    quote! {
        pub fn #name(m: &mut M, mut pc: u32) -> Flow {
            #load
            loop {
                match pc {
                    #(#arms)*
                    _ => #ret,
                }
            }
        }
    }
}
