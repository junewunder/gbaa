//! Guest registers and NZCV as Rust locals inside a function
//! (`docs/structured-codegen-design.md`, step 9): each function holds the
//! registers it reads or writes, and the four flags, in locals that LLVM
//! promotes. They are loaded at the function's entry and kept in step with
//! `Machine` at every point where something else may read or write the
//! CPU: [`Locals::sync`], the registers review's table ("Where state must
//! be in `Machine`", `docs/research/registers-and-types.md`) as one
//! function. [`check`] proves a printed function keeps to it.

use crate::func::{Config, Func};
use crate::names::{Kind, SourceMap};
use analysis::ir::Op;
use armv4t::sem::Flag;
use proc_macro2::{Ident, Literal, TokenStream};
use quote::{format_ident, quote};
use std::collections::BTreeSet;

/// Which CPU state one function holds in locals.
#[derive(Clone, Copy, Debug, Default)]
pub struct Locals {
    /// Registers held (read or written anywhere in the function), a bit
    /// each.
    pub held: u16,
    /// Registers the function writes: the ones written back.
    pub written: u16,
    /// Whether the flags are held (the function reads or writes one).
    pub flags: bool,
    /// Whether it writes one, so they are written back.
    pub flags_written: bool,
    /// Whether a call to a function that keeps r4-r11 and sp reloads only
    /// the rest ([`crate::func::Config::callee_saves`]).
    pub kept: bool,
}

/// Where generated code hands the CPU to something else: a row of the
/// registers review's table. Every runtime call that reads or writes CPU
/// state is printed through [`Locals::sync`] with its row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Point {
    /// A tick's yield path: `yield_at` takes an IRQ or a halt here, or the
    /// frame end, and compiled code may go on after it.
    Yield,
    /// A call into guest code that may read and write any register: a
    /// generated function, `call`, `settle`, `rt::swi`, an exception
    /// return or entry through `rt::exit`.
    Call,
    /// Something that reads or writes the CPSR's control bits, the SPSRs
    /// or the banks (`msr`, a user-bank op, `mrs spsr`).
    Psr,
    /// A read of CPU state outside the locals (a user-bank read, `mrs
    /// spsr`, which in user mode is the CPSR): written back first.
    Read,
    /// A return to the dispatcher (or the caller).
    Return,
    /// A return right after a call came back yielding: `Machine` holds the
    /// callee's state, and the caller's locals are stale.
    ReturnStale,
}

/// The statements before and after a [`Point`].
pub struct Around {
    pub before: TokenStream,
    pub after: TokenStream,
}

fn reg_ident(i: usize) -> Ident {
    format_ident!("r{}", i)
}

fn flag_ident(f: Flag) -> Ident {
    match f {
        Flag::N => format_ident!("nf"),
        Flag::Z => format_ident!("zf"),
        Flag::C => format_ident!("cf"),
        Flag::V => format_ident!("vf"),
    }
}

impl Locals {
    /// Nothing held: every access goes to `Machine` (v1's form).
    pub const OFF: Locals = Locals {
        held: 0,
        written: 0,
        flags: false,
        flags_written: false,
        kept: false,
    };

    /// What `f` holds, if `cfg` puts registers in locals.
    pub fn of(cfg: &Config, f: &Func) -> Locals {
        if !cfg.locals {
            return Locals::OFF;
        }
        let mut l = Locals {
            kept: cfg.callee_saves,
            ..Locals::OFF
        };
        let blocks = f
            .blocks
            .values()
            .chain(f.variants.values().flatten().map(|v| &v.block));
        for gb in blocks {
            for inst in &gb.block.insts {
                match inst.op {
                    Op::GetReg(r) => l.held |= 1 << r.index(),
                    Op::SetReg(r, _) => {
                        l.held |= 1 << r.index();
                        l.written |= 1 << r.index();
                    }
                    Op::GetFlag(_) => l.flags = true,
                    Op::SetFlag(..) | Op::YieldFlag(..) => {
                        l.flags = true;
                        l.flags_written = true;
                    }
                    _ => {}
                }
            }
        }
        l
    }

    pub fn holds(&self, i: usize) -> bool {
        self.held & 1 << i != 0
    }

    /// Register `i`'s value.
    pub fn reg(&self, i: usize) -> TokenStream {
        if self.holds(i) {
            let r = reg_ident(i);
            quote!(#r)
        } else {
            let i = Literal::usize_unsuffixed(i);
            quote!(m.cpu.r[#i])
        }
    }

    /// Set register `i` to `x`.
    pub fn set_reg(&self, i: usize, x: TokenStream) -> TokenStream {
        if self.holds(i) {
            let r = reg_ident(i);
            quote!(#r = #x;)
        } else {
            let i = Literal::usize_unsuffixed(i);
            quote!(m.cpu.r[#i] = #x;)
        }
    }

    /// Flag `f`'s value, if held.
    pub fn flag(&self, f: Flag) -> Option<TokenStream> {
        self.flags.then(|| {
            let id = flag_ident(f);
            quote!(#id)
        })
    }

    /// The CPSR with the held flags in it.
    pub fn cpsr(&self) -> TokenStream {
        if !self.flags {
            return quote!(m.cpu.cpsr);
        }
        let [n, z, c, v] = Flag::ALL.map(flag_ident);
        quote! {
            (m.cpu.cpsr & 0x0fff_ffff | (#n as u32) << 31 | (#z as u32) << 30
                | (#c as u32) << 29 | (#v as u32) << 28)
        }
    }

    /// The function's first statements: every held register and flag
    /// loaded from `Machine`, and the `wb!`, `reload!` and `reload_kept!`
    /// macros the write-backs and reloads print as.
    pub fn load(&self) -> TokenStream {
        let loads = self.loads();
        let (wb, reload) = (self.write_back_body(), self.reload_body());
        let kept = self.reload_kept_body();
        let kept = (self.kept && !kept.is_empty()).then(|| {
            quote!(
                macro_rules! reload_kept { () => { #kept }; }
            )
        });
        let wb = (!wb.is_empty()).then(|| {
            quote!(
                macro_rules! wb { () => { #wb }; }
            )
        });
        let reload = (!reload.is_empty()).then(|| {
            quote!(
                macro_rules! reload { () => { #reload }; }
            )
        });
        quote!(#loads #wb #reload #kept)
    }

    fn loads(&self) -> TokenStream {
        let regs = (0..15).filter(|&i| self.holds(i)).map(|i| {
            let (r, k) = (reg_ident(i), Literal::usize_unsuffixed(i));
            quote!(let mut #r: u32 = m.cpu.r[#k];)
        });
        let flags = Flag::ALL.into_iter().filter(|_| self.flags).map(|f| {
            let (id, b) = (flag_ident(f), Literal::u32_unsuffixed(f.cpsr_bit()));
            quote!(let mut #id: bool = (m.cpu.cpsr >> #b) & 1 != 0;)
        });
        quote!(#(#regs)* #(#flags)*)
    }

    /// The written registers and flags, stored into `Machine`.
    pub fn write_back(&self) -> TokenStream {
        if self.written == 0 && !self.flags_written {
            return quote!();
        }
        quote!(wb!();)
    }

    /// Every held register and flag, reloaded from `Machine`.
    pub fn reload(&self) -> TokenStream {
        if self.held == 0 && !self.flags {
            return quote!();
        }
        quote!(reload!();)
    }

    /// What `Machine` may have changed after a call to a function that
    /// keeps r4-r11 and sp, reloaded: the other held registers and the
    /// flags. Nothing if this doesn't take the proof.
    pub fn reload_kept(&self) -> TokenStream {
        if !self.kept || self.reload_kept_body().is_empty() {
            return quote!();
        }
        quote!(reload_kept!();)
    }

    /// What `wb!()` does.
    fn write_back_body(&self) -> TokenStream {
        let regs = (0..15).filter(|&i| self.written & 1 << i != 0).map(|i| {
            let (r, k) = (reg_ident(i), Literal::usize_unsuffixed(i));
            quote!(m.cpu.r[#k] = #r;)
        });
        let flags = self.flags_written.then(|| {
            let c = self.cpsr();
            quote!(m.cpu.cpsr = #c;)
        });
        quote!(#(#regs)* #flags)
    }

    /// What `reload!()` does.
    fn reload_body(&self) -> TokenStream {
        self.reload_of(self.held)
    }

    /// What `reload_kept!()` does.
    fn reload_kept_body(&self) -> TokenStream {
        self.reload_of(self.held & !crate::saves::KEPT)
    }

    /// The registers `regs` and the held flags, reloaded.
    fn reload_of(&self, regs: u16) -> TokenStream {
        let regs = (0..15).filter(|&i| regs & 1 << i != 0).map(|i| {
            let (r, k) = (reg_ident(i), Literal::usize_unsuffixed(i));
            quote!(#r = m.cpu.r[#k];)
        });
        let flags = Flag::ALL.into_iter().filter(|_| self.flags).map(|f| {
            let (id, b) = (flag_ident(f), Literal::u32_unsuffixed(f.cpsr_bit()));
            quote!(#id = (m.cpu.cpsr >> #b) & 1 != 0;)
        });
        quote!(#(#regs)* #(#flags)*)
    }

    /// What `Machine` must hold before point `p`, and what the locals
    /// reload after it.
    pub fn sync(&self, p: Point) -> Around {
        let (wb, reload) = match p {
            Point::Yield | Point::Call | Point::Psr => (true, true),
            Point::Read | Point::Return => (true, false),
            Point::ReturnStale => (false, false),
        };
        Around {
            before: if wb { self.write_back() } else { quote!() },
            after: if reload { self.reload() } else { quote!() },
        }
    }

    /// `return Flow::Jump(key)` at point `p`, as an expression.
    pub fn ret(&self, p: Point, key: TokenStream) -> TokenStream {
        let before = self.sync(p).before;
        if before.is_empty() {
            return quote!(return Flow::Jump(#key));
        }
        quote!({
            #before
            return Flow::Jump(#key)
        })
    }
}

/// Where each local stands against `Machine` at one point of a printed
/// function, a bit per local (registers 0 to 14, then N, Z, C and V);
/// `None` where control can't reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
struct State {
    /// Written since `Machine` last got it.
    dirty: u32,
    /// `Machine` changed since the local was last loaded.
    stale: u32,
}

const FLAG_BITS: u32 = 0xf << 15;

fn join(a: Option<State>, b: Option<State>) -> Option<State> {
    match (a, b) {
        (Some(a), Some(b)) => Some(State {
            dirty: a.dirty | b.dirty,
            stale: a.stale | b.stale,
        }),
        (a, None) => a,
        (None, b) => b,
    }
}

/// The local `name` is, as a bit.
fn local_bit(name: &str) -> Option<u32> {
    match name {
        "nf" => Some(1 << 15),
        "zf" => Some(1 << 16),
        "cf" => Some(1 << 17),
        "vf" => Some(1 << 18),
        _ => {
            let i: u32 = name.strip_prefix('r')?.parse().ok()?;
            (i < 15).then_some(1 << i)
        }
    }
}

/// The printed path `e` names, as `a::b`, if it is a path.
fn path(e: &syn::Expr) -> Option<String> {
    match e {
        syn::Expr::Path(p) => Some(
            p.path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>()
                .join("::"),
        ),
        syn::Expr::Field(f) => {
            let syn::Member::Named(n) = &f.member else {
                return None;
            };
            Some(format!("{}.{n}", path(&f.base)?))
        }
        _ => None,
    }
}

/// `m.cpu.r[i]`'s `i`.
fn machine_reg(e: &syn::Expr) -> Option<u32> {
    let syn::Expr::Index(ix) = e else { return None };
    if path(&ix.expr).as_deref() != Some("m.cpu.r") {
        return None;
    }
    let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Int(i),
        ..
    }) = &*ix.index
    else {
        return None;
    };
    i.base10_parse().ok()
}

/// The runtime calls that touch nothing the locals hold: time, the bus,
/// pure helpers (rows "nothing" of the table).
const QUIET: [&str; 23] = [
    "rt::tick",
    "rt::spin_edge",
    "rt::tick_native",
    "rt::idle_edge",
    "rt::idle_exit",
    "rt::tick_ram",
    "rt::tick_flat",
    "rt::tick_ram_flat",
    "rt::fetch",
    "rt::stall",
    "rt::skipped",
    "rt::multiply",
    "rt::block",
    "rt::overlay_valid",
    "rt::yielding",
    "rt::rekey",
    "Flow::Jump",
    "Some",
    "Reg",
    "PsrMask",
    "Exit::Jump",
    "Exit::Exchange",
    "Exit::ExceptionReturn",
];

/// Checks one printed function against [`Locals::sync`]'s table, as
/// Rideau and Leroy's register-allocation validator does, backwards:
/// before every call that may read the CPU, `Machine` holds every written
/// local; after every call that may write it, no local is used before it
/// is reloaded; every return leaves `Machine` complete; no held register
/// or flag is touched in `Machine` but by a write-back or a reload; and
/// no call is one the table has no row for. A `reload_kept!()`, which
/// leaves r4-r11 and sp as they were, must follow a call to one of `kept`
/// (the functions [`crate::saves`] proved keep them) with no other call
/// between. `Err` names the first violation.
pub fn check(
    loc: &Locals,
    f: &syn::ItemFn,
    names: Option<&SourceMap>,
    kept: &BTreeSet<u32>,
) -> Result<(), String> {
    let mut c = Checker {
        loc,
        names,
        kept,
        kept_call: false,
        held: loc.held as u32 | if loc.flags { FLAG_BITS } else { 0 },
        written: loc.written as u32 | if loc.flags_written { FLAG_BITS } else { 0 },
        labels: Vec::new(),
        err: None,
    };
    c.block(&f.block, Some(State::default()));
    match c.err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

struct Checker<'a> {
    loc: &'a Locals,
    /// A named print's source map: its function names are guest calls.
    names: Option<&'a SourceMap>,
    /// The functions that keep r4-r11 and sp.
    kept: &'a BTreeSet<u32>,
    /// Whether the last call that may write the CPU was to one of `kept`
    /// (and `settle`s after it).
    kept_call: bool,
    held: u32,
    written: u32,
    /// The enclosing loops and labeled blocks, innermost last: the label,
    /// whether it is a loop, and the states its `continue`s and `break`s
    /// carry.
    labels: Vec<Label>,
    err: Option<String>,
}

struct Label {
    name: Option<String>,
    is_loop: bool,
    cont: Option<State>,
    brk: Option<State>,
}

type S = Option<State>;

impl Checker<'_> {
    fn fail(&mut self, why: impl FnOnce() -> String) {
        if self.err.is_none() {
            self.err = Some(why());
        }
    }

    fn block(&mut self, b: &syn::Block, mut s: S) -> S {
        for st in &b.stmts {
            s = self.stmt(st, s);
        }
        s
    }

    fn stmt(&mut self, st: &syn::Stmt, s: S) -> S {
        match st {
            // The entry's loads.
            syn::Stmt::Local(syn::Local {
                pat: syn::Pat::Type(syn::PatType { pat, .. }),
                init: Some(init),
                ..
            }) if matches!(&**pat, syn::Pat::Ident(p)
                if local_bit(&p.ident.to_string()).is_some_and(|b| self.held & b != 0)) =>
            {
                let syn::Pat::Ident(p) = &**pat else {
                    unreachable!()
                };
                let id = &p.ident;
                self.assign(&syn::parse_quote!(#id), &init.expr, s)
            }
            syn::Stmt::Local(l) => match &l.init {
                Some(init) if init.diverge.is_none() => self.expr(&init.expr, s),
                None => s,
                Some(_) => {
                    self.fail(|| "a `let ... else`".into());
                    s
                }
            },
            syn::Stmt::Expr(e, _) => self.expr(e, s),
            syn::Stmt::Macro(m) => self.mac(&m.mac, s),
            syn::Stmt::Item(syn::Item::Macro(m)) => {
                self.def(m);
                s
            }
            syn::Stmt::Item(_) => {
                self.fail(|| "an item inside a function".into());
                s
            }
        }
    }

    fn mac(&mut self, m: &syn::Macro, s: S) -> S {
        if m.path.is_ident("wb") {
            // Checked against the written set where it is defined.
            self.read(self.written, s, "a write-back");
            return s.map(|st| State { dirty: 0, ..st });
        }
        if m.path.is_ident("reload") {
            return s.map(|_| State::default());
        }
        if m.path.is_ident("reload_kept") {
            if !self.kept_call {
                self.fail(|| "reload_kept! after a call not proven to keep r4-r11".into());
            }
            return s.map(|_| State::default());
        }
        // `addr!(name + off)`, a named print's constant, reads nothing, nor
        // does `--asm-comments`' `note!`.
        if !m.path.is_ident("insn")
            && !m.path.is_ident("note")
            && !(self.names.is_some() && m.path.is_ident("addr"))
        {
            self.fail(|| format!("macro {:?}", m.path.get_ident()));
        }
        s
    }

    /// `macro_rules! wb` or `reload`: exactly the write-back or reload of
    /// what the function holds.
    fn def(&mut self, m: &syn::ItemMacro) {
        let name = m.ident.as_ref().map(|i| i.to_string()).unwrap_or_default();
        let body = match name.as_str() {
            "wb" => self.loc.write_back_body(),
            "reload" => self.loc.reload_body(),
            "reload_kept" if self.loc.kept => self.loc.reload_kept_body(),
            _ => {
                self.fail(|| format!("macro_rules! {name}"));
                return;
            }
        };
        let want = quote!(() => { #body };).to_string();
        if m.mac.tokens.to_string() != want {
            self.fail(|| {
                format!("macro_rules! {name} isn't the {name} of what the function holds")
            });
        }
    }

    /// A read of locals `bits`.
    fn read(&mut self, bits: u32, s: S, what: &str) {
        if let Some(st) = s
            && st.stale & bits != 0
        {
            self.fail(|| format!("{what} read stale"));
        }
    }

    /// `Machine` must hold every written local here.
    fn synced(&mut self, s: S, what: &str) {
        if let Some(st) = s
            && st.dirty != 0
        {
            self.fail(|| format!("{what} with locals {:#x} not written back", st.dirty));
        }
    }

    fn exprs<'e>(&mut self, es: impl IntoIterator<Item = &'e syn::Expr>, mut s: S) -> S {
        for e in es {
            s = self.expr(e, s);
        }
        s
    }

    fn expr(&mut self, e: &syn::Expr, s: S) -> S {
        use syn::Expr::*;
        s?;
        match e {
            Lit(_) => s,
            Paren(p) => self.expr(&p.expr, s),
            Group(g) => self.expr(&g.expr, s),
            Cast(c) => self.expr(&c.expr, s),
            Unary(u) => self.expr(&u.expr, s),
            Tuple(t) => self.exprs(&t.elems, s),
            Let(l) => self.expr(&l.expr, s),
            Macro(m) => self.mac(&m.mac, s),
            Binary(b) => {
                // The CPSR's other bits, beside the flags in locals.
                if let (Some(p), syn::BinOp::BitAnd(_)) = (path(&b.left), &b.op)
                    && p == "m.cpu.cpsr"
                    && matches!(&*b.right, Lit(syn::ExprLit { lit: syn::Lit::Int(i), .. })
                        if i.base10_parse::<u32>().ok() == Some(0x0fff_ffff))
                {
                    return s;
                }
                let s = self.expr(&b.left, s);
                self.expr(&b.right, s)
            }
            Path(_) | Field(_) => {
                let p = path(e).unwrap_or_default();
                if let Some(b) = local_bit(&p).filter(|b| self.held & b != 0) {
                    self.read(b, s, &p);
                } else if p == "m.cpu.cpsr" && self.loc.flags {
                    // The flags as `Machine` has them.
                    self.synced(
                        s.map(|st| State {
                            dirty: st.dirty & FLAG_BITS,
                            ..st
                        }),
                        "a CPSR read",
                    );
                } else if p.starts_with("m.") && p != "m.bus" && p != "m.cpu.cpsr" {
                    self.fail(|| format!("{p} read"));
                }
                s
            }
            Index(ix) => {
                if let Some(i) = machine_reg(e) {
                    if self.held & 1 << i != 0 {
                        self.fail(|| format!("r{i} read from `Machine` outside a reload"));
                    }
                    return s;
                }
                let s = self.expr(&ix.expr, s);
                self.expr(&ix.index, s)
            }
            Assign(a) => self.assign(&a.left, &a.right, s),
            Block(b) => {
                if b.label.is_some() {
                    self.labeled(
                        b.label.as_ref().map(|l| l.name.ident.to_string()),
                        &b.block,
                        s,
                    )
                } else {
                    self.block(&b.block, s)
                }
            }
            If(i) => {
                let s = self.expr(&i.cond, s);
                let t = self.block(&i.then_branch, s);
                let f = match &i.else_branch {
                    Some((_, e)) => self.expr(e, s),
                    None => s,
                };
                join(t, f)
            }
            Match(mt) => {
                let s = self.expr(&mt.expr, s);
                let mut out = None;
                for arm in &mt.arms {
                    if arm.guard.is_some() {
                        self.fail(|| "a match guard".into());
                    }
                    out = join(out, self.expr(&arm.body, s));
                }
                out
            }
            Loop(l) => {
                let name = l.label.as_ref().map(|l| l.name.ident.to_string());
                let mut entry = s;
                loop {
                    self.labels.push(Label {
                        name: name.clone(),
                        is_loop: true,
                        cont: None,
                        brk: None,
                    });
                    let end = self.block(&l.body, entry);
                    let lb = self.labels.pop().unwrap();
                    let next = join(join(entry, end), lb.cont);
                    if next == entry || self.err.is_some() {
                        return lb.brk;
                    }
                    entry = next;
                }
            }
            Break(b) => {
                let s = match &b.expr {
                    Some(x) => self.expr(x, s),
                    None => s,
                };
                self.jump(b.label.as_ref(), false, s);
                None
            }
            Continue(c) => {
                self.jump(c.label.as_ref(), true, s);
                None
            }
            Return(r) => {
                let s = match &r.expr {
                    Some(x) => self.expr(x, s),
                    None => s,
                };
                self.synced(s, "a return");
                None
            }
            Call(c) => {
                let s = self.exprs(&c.args, s);
                let name = path(&c.func).unwrap_or_default();
                self.call(&name, c.args.iter().nth(3), s)
            }
            MethodCall(mc) => {
                let s = self.exprs(&mc.args, s);
                match path(&mc.receiver).as_deref() {
                    Some("m.bus") => s,
                    Some("m.cpu") => {
                        let name = format!("m.cpu.{}", mc.method);
                        self.call(&name, None, s)
                    }
                    Some(p) if p.starts_with("m.") || p == "m" => {
                        self.fail(|| format!("{p}.{}", mc.method));
                        s
                    }
                    // A method of a value: pure.
                    _ => self.expr(&mc.receiver, s),
                }
            }
            _ => {
                self.fail(|| format!("an expression the check doesn't know: {}", quote!(#e)));
                s
            }
        }
    }

    fn labeled(&mut self, name: Option<String>, b: &syn::Block, s: S) -> S {
        self.labels.push(Label {
            name,
            is_loop: false,
            cont: None,
            brk: None,
        });
        let end = self.block(b, s);
        let lb = self.labels.pop().unwrap();
        join(end, lb.brk)
    }

    fn jump(&mut self, label: Option<&syn::Lifetime>, cont: bool, s: S) {
        let name = label.map(|l| l.ident.to_string());
        let target = self.labels.iter_mut().rev().find(|l| match &name {
            Some(n) => l.name.as_ref() == Some(n),
            None => l.is_loop,
        });
        match target {
            Some(l) if cont => l.cont = join(l.cont, s),
            Some(l) => l.brk = join(l.brk, s),
            None => self.fail(|| "a jump to no label".into()),
        }
    }

    /// `left = right`.
    fn assign(&mut self, left: &syn::Expr, right: &syn::Expr, s: S) -> S {
        let lp = path(left).unwrap_or_default();
        // A reload: the local from `Machine`.
        if let Some(b) = local_bit(&lp).filter(|b| self.held & b != 0) {
            let reload = if b < 1 << 15 {
                machine_reg(right) == Some(b.trailing_zeros())
            } else {
                let bit = Literal::u32_unsuffixed(31 - (b.trailing_zeros() - 15));
                quote!(#right).to_string() == quote!((m.cpu.cpsr >> #bit) & 1 != 0).to_string()
            };
            if reload {
                return s.map(|st| State {
                    dirty: st.dirty & !b,
                    stale: st.stale & !b,
                });
            }
            let s = self.expr(right, s);
            return s.map(|st| State {
                dirty: st.dirty | b,
                stale: st.stale & !b,
            });
        }
        // A write-back: `Machine` from the local.
        if let Some(i) = machine_reg(left) {
            if self.held & 1 << i == 0 {
                return self.expr(right, s);
            }
            if path(right) != Some(format!("r{i}")) {
                self.fail(|| format!("r{i} written to `Machine` outside a write-back"));
            }
            self.read(1 << i, s, &format!("r{i}"));
            return s.map(|st| State {
                dirty: st.dirty & !(1 << i),
                ..st
            });
        }
        if lp == "m.cpu.cpsr" && self.loc.flags {
            if quote!(#right).to_string() != self.loc.cpsr().to_string() {
                // Not the write-back: what `Machine` holds stays.
                self.fail(|| "the CPSR written outside a write-back".into());
            }
            let s = self.expr(right, s);
            return s.map(|st| State {
                dirty: st.dirty & !FLAG_BITS,
                ..st
            });
        }
        if lp.starts_with("m.") && lp != "m.cpu.cpsr" && lp != "m.cpu.pc" {
            self.fail(|| format!("{lp} written"));
        }
        // `pc`, or the CPSR with no flags held.
        self.expr(right, s)
    }

    /// A call to `name` (a path, or `m.cpu.method`); `first`: its fourth
    /// argument (an exit's kind).
    fn call(&mut self, name: &str, first: Option<&syn::Expr>, s: S) -> S {
        if QUIET.contains(&name) || name.starts_with("prim::") {
            return s;
        }
        // An exit that only sets pc and the T bit; the other two switch
        // modes.
        let mode = |e: &syn::Expr| match e {
            syn::Expr::Call(c) => path(&c.func).is_some_and(|p| p == "Exit::ExceptionReturn"),
            e => path(e).is_some_and(|p| p == "Exit::Undefined"),
        };
        let first = match first {
            Some(_) if name == "rt::exit" => first,
            _ => None,
        };
        if name == "rt::exit" && !first.is_some_and(mode) {
            return s;
        }
        let reads = |c: &mut Self, s: S| {
            c.synced(s, name);
            s
        };
        let writes = |c: &mut Self, s: S| {
            c.synced(s, name);
            let held = c.held;
            s.map(|st| State {
                stale: st.stale | held,
                ..st
            })
        };
        // A guest call's callee, by its fn's name.
        let callee = match self.names.and_then(|m| m.key_of(name)) {
            Some((Kind::Func, k)) => Some(k),
            _ => name
                .strip_prefix("f_")
                .and_then(|k| u32::from_str_radix(k, 16).ok()),
        };
        match name {
            "m.cpu.user_reg" | "m.cpu.spsr" => reads(self, s),
            // Runs the call on to its return site: the same call.
            "settle" => writes(self, s),
            "yield_at" | "call" | "rt::swi" | "rt::exit" | "m.cpu.msr_cpsr" | "m.cpu.msr_spsr"
            | "m.cpu.set_user_reg" => {
                self.kept_call = false;
                writes(self, s)
            }
            _ if let Some(k) = callee => {
                self.kept_call = self.kept.contains(&k);
                writes(self, s)
            }
            // Machine form only.
            "m.cpu.flag" | "m.cpu.set_flag" if !self.loc.flags => s,
            _ => {
                self.fail(|| format!("a call the table has no row for: {name}"));
                s
            }
        }
    }
}
