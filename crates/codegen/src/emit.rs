//! Writing the generated crate (`docs/codegen-design.md`, "`just recompile`
//! and running it"): `out/<name>/` with its own `[workspace]`, the code as a
//! library, the dispatch table, a `main` (the desktop) and a report.
//!
//! Code the ROM copies into IWRAM is recompiled too (decision 7): each
//! block of its functions, and each version of a block the code patches
//! ([`crate::variants`]), is an overlay, with its own guard in `OVERLAYS`;
//! its blocks are in `RAM_BLOCKS`.

use crate::func::{Clock, Config, Ctx, Func, Plan, build_func, exits};
use crate::lower::{V1, fn_name, hex, lower_func_checked, plan_or_why};
use crate::names::SourceMap;
use analysis::discovery::{Discovery, Overlay, overlays};
use analysis::ir::Op;
use proc_macro2::TokenStream;
use quote::quote;
use runtime::dispatch::Extent;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

/// Functions per generated module, so rustc can spread them over codegen
/// units.
pub const PER_MODULE: usize = 256;

/// Format a token stream as a Rust file. Each `note!("…");` statement (a
/// readable form's comment, `crate::dwarf`) prints as `// …`.
pub fn pretty(tokens: TokenStream) -> String {
    let file: syn::File = syn::parse2(tokens.clone())
        .unwrap_or_else(|e| panic!("generated code doesn't parse: {e}\n{tokens}"));
    comments(prettyplease::unparse(&file))
}

/// `text` with each `note!("…");` statement, alone on its line or
/// wrapped over several, as the comment `// …`, indented the same.
fn comments(text: String) -> String {
    if !text.contains("note!(") {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    let mut lines = text.split_inclusive('\n');
    while let Some(line) = lines.next() {
        let t = line.trim();
        let indent = &line[..line.len() - line.trim_start().len()];
        let lit = match t.strip_prefix("note!(") {
            Some("") => {
                // Wrapped: the literal, then `);`.
                let (lit, close) = (lines.next().unwrap_or(""), lines.next().unwrap_or(""));
                assert_eq!(close.trim(), ");", "a wrapped note! in {text}");
                lit.trim().to_string()
            }
            Some(r) => match r.strip_suffix(");") {
                Some(lit) => lit.to_string(),
                None => {
                    out.push_str(line);
                    continue;
                }
            },
            None => {
                out.push_str(line);
                continue;
            }
        };
        let lit: syn::LitStr = syn::parse_str(&lit).expect("a note!'s literal");
        out.push_str(indent);
        out.push_str("// ");
        out.push_str(&lit.value());
        out.push('\n');
    }
    out
}

/// Write `bytes` to `path` unless it already holds exactly them, so an
/// unchanged file keeps its mtime and cargo doesn't rebuild its crate.
pub fn write_if_changed(path: &Path, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    let bytes = bytes.as_ref();
    if std::fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(());
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(path, bytes)
}

/// The files one recompile writes, each only if its bytes changed
/// ([`write_if_changed`]), so that afterwards [`Files::prune`] deletes the
/// files an earlier run wrote that this one didn't.
#[derive(Default)]
pub struct Files(std::collections::HashSet<std::path::PathBuf>);

impl Files {
    /// Write `bytes` to `path` if they differ from what it holds.
    pub fn write(&mut self, path: &Path, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
        write_if_changed(path, bytes)?;
        self.0.insert(path.to_path_buf());
        Ok(())
    }

    /// Delete every file under `dir` this didn't write, and the
    /// directories that leaves empty.
    pub fn prune(&self, dir: &Path) -> std::io::Result<()> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Ok(());
        };
        for e in entries {
            let e = e?;
            let p = e.path();
            // A link is a file here: pruning never follows it.
            if e.file_type()?.is_dir() {
                self.prune(&p)?;
                if std::fs::read_dir(&p)?.next().is_none() {
                    std::fs::remove_dir(&p)?;
                }
            } else if !self.0.contains(&p) {
                std::fs::remove_file(&p)?;
            }
        }
        Ok(())
    }
}

/// What a recompile made.
pub struct Recompiled {
    pub funcs: Vec<Func>,
    /// Every dispatch key, with the function that owns it: each function's
    /// entry, and every block start of the functions printed in v1 form.
    pub table: BTreeMap<u32, u32>,
    /// The block starts of the structured functions that aren't keys,
    /// with where each ticks: the dispatcher doesn't call them, but counts
    /// the interpreter reaching one as a resume rather than a seed.
    pub inner: BTreeMap<u32, Extent>,
    /// Why each function printed in v1 form is, by entry.
    pub v1: BTreeMap<u32, V1>,
    /// Discovered instructions no block covers, as (address, thumb).
    pub uncovered: Vec<(u32, bool)>,
    /// How far each block of `table` runs to its tick, in its owner, for
    /// the block-timed interpreter (`runtime::dispatch::run_blocks`).
    pub extents: BTreeMap<u32, Extent>,
}

/// Build every function discovery found.
pub fn build_all(ctx: &Ctx) -> Recompiled {
    let funcs: Vec<Func> = ctx.entries.iter().map(|&e| build_func(ctx, e)).collect();
    let v1: BTreeMap<u32, V1> = funcs
        .iter()
        .filter_map(|f| Some((f.entry, plan_or_why(ctx, f).err()?)))
        .collect();
    let (table, inner) = keys(&funcs, |f| !v1.contains_key(&f.entry));
    let mut covered = std::collections::HashSet::new();
    for f in &funcs {
        for gb in f.blocks.values() {
            covered.extend(gb.insns.iter().map(|(a, _)| (*a, gb.block.thumb)));
        }
    }
    let uncovered = ctx
        .disc
        .code
        .iter()
        .map(|(&a, c)| (a, c.thumb))
        .filter(|x| !covered.contains(x))
        .collect();
    let owners: BTreeMap<u32, &Func> = funcs.iter().map(|f| (f.entry, f)).collect();
    let extents = table
        .iter()
        .map(|(&k, f)| (k, extent(ctx, &owners[f].blocks[&k])))
        .collect();
    let inner = inner
        .iter()
        .map(|(&k, f)| (k, extent(ctx, &owners[f].blocks[&k])))
        .collect();
    Recompiled {
        funcs,
        table,
        inner,
        v1,
        uncovered,
        extents,
    }
}

/// The ROM ranges (start, end) const-prop folded loads from in `funcs`
/// (`runtime::dispatch::Code::folded`): every load whose address is a
/// constant the cartridge holds ([`Ctx::cart`]; const-prop folds exactly
/// those), at its `0x08` address, sorted, touching ranges merged.
pub fn folded(ctx: &Ctx, funcs: &[Func]) -> Vec<(u32, u32)> {
    let rom = ctx.cart();
    let mut loads = Vec::new();
    for gb in funcs.iter().flat_map(|f| f.blocks.values()) {
        let insts = &gb.block.insts;
        for i in insts {
            let Op::Load { width, addr } = i.op else {
                continue;
            };
            let (Op::Const(a) | Op::Pool(a)) = insts[addr.index()].op else {
                continue;
            };
            if rom.load(width, a).is_some() {
                let a = 0x0800_0000 | a & 0x01FF_FFFF & !(width.bytes() - 1);
                loads.push((a, a + width.bytes()));
            }
        }
    }
    loads.sort_unstable();
    let mut out: Vec<(u32, u32)> = Vec::new();
    for (a, b) in loads {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// The dispatch keys of `funcs`, each with the function it runs in, and
/// the block starts of the structured ones that aren't keys, each with a
/// function that has it: every function's entry, then the first v1
/// function to claim a shared block. A structured function is only ever
/// called at its entry.
pub fn keys(
    funcs: &[Func],
    structured: impl Fn(&Func) -> bool,
) -> (BTreeMap<u32, u32>, BTreeMap<u32, u32>) {
    let mut table = BTreeMap::new();
    for f in funcs {
        table.insert(f.entry, f.entry);
    }
    let mut inner = BTreeMap::new();
    for f in funcs {
        let s = structured(f);
        for &k in f.blocks.keys() {
            if s {
                inner.entry(k).or_insert(f.entry);
            } else {
                table.entry(k).or_insert(f.entry);
            }
        }
    }
    inner.retain(|k, _| !table.contains_key(k));
    (table, inner)
}

/// Where `gb` ticks: after its instructions, or the first that leaves the
/// straight line, and a `bx rN` stub its call runs on the same tick.
fn extent(ctx: &Ctx, gb: &crate::func::GBlock) -> Extent {
    let stub = exits(ctx, gb)
        .iter()
        .find_map(|x| match x.plan {
            Plan::CallVia { stub, .. } => Some(stub),
            _ => None,
        })
        .unwrap_or(0);
    Extent {
        len: gb.insns.len() as u16,
        stub,
    }
}

/// The readable form's prelude (`docs/mod-view-design.md`, "What the
/// readable-form units should become"; the hand-written spike's, in
/// `mods/emerald/decomp-spike/prelude.rs`, moved here): typed guest
/// addresses and the machine's accesses through them. In every generated
/// crate's `prelude`; [`readable_impl`] implements [`Game`] for its `M`.
pub fn readable_items(clock: Clock) -> TokenStream {
    let game = game_trait(clock);
    quote! {
        /// A guest object's size, from pret's DWARF.
        pub trait Guest {
            const SIZE: u32;
        }
        impl Guest for u8 {
            const SIZE: u32 = 1;
        }
        impl Guest for u16 {
            const SIZE: u32 = 2;
        }
        impl Guest for u32 {
            const SIZE: u32 = 4;
        }
        impl<T: Guest, const N: usize> Guest for [T; N] {
            const SIZE: u32 = T::SIZE * N as u32;
        }

        /// A typed guest address (`T *` in pret's C).
        pub struct Ptr<T>(pub u32, pub core::marker::PhantomData<T>);
        impl<T> Clone for Ptr<T> {
            fn clone(&self) -> Self {
                *self
            }
        }
        impl<T> Copy for Ptr<T> {}
        impl<T> Ptr<T> {
            pub const fn at(a: u32) -> Ptr<T> {
                Ptr(a, core::marker::PhantomData)
            }
            /// A field `off` bytes in.
            pub const fn field<F>(self, off: u32) -> Ptr<F> {
                Ptr(self.0.wrapping_add(off), core::marker::PhantomData)
            }
        }
        impl<T> Guest for Ptr<T> {
            const SIZE: u32 = 4;
        }
        impl<T: Guest, const N: usize> Ptr<[T; N]> {
            /// `&a[i]`.
            pub fn idx(self, i: u32) -> Ptr<T> {
                Ptr(self.0.wrapping_add(i.wrapping_mul(T::SIZE)), core::marker::PhantomData)
            }
        }

        /// A value the CPU can load or store at its own width.
        pub trait Word: Copy {
            const W: Width;
            fn from_bus(v: u32) -> Self;
            fn to_bus(self) -> u32;
        }
        impl Word for u8 {
            const W: Width = Width::Byte;
            fn from_bus(v: u32) -> u8 {
                v as u8
            }
            fn to_bus(self) -> u32 {
                self as u32
            }
        }
        impl Word for u16 {
            const W: Width = Width::Half;
            fn from_bus(v: u32) -> u16 {
                v as u16
            }
            fn to_bus(self) -> u32 {
                self as u32
            }
        }
        impl Word for u32 {
            const W: Width = Width::Word;
            fn from_bus(v: u32) -> u32 {
                v
            }
            fn to_bus(self) -> u32 {
                self
            }
        }
        /// A bitfield (`T x : N` in pret's C): `N` bits from bit `LO` of
        /// the `T` its accessor points at, which loads and stores as `T`.
        #[derive(Clone, Copy)]
        pub struct Bits<T, const LO: u32, const N: u32>(pub T);
        impl<T: Word, const LO: u32, const N: u32> Word for Bits<T, LO, N> {
            const W: Width = T::W;
            fn from_bus(v: u32) -> Self {
                Bits(T::from_bus(v))
            }
            fn to_bus(self) -> u32 {
                self.0.to_bus()
            }
        }
        /// A pointer the guest holds (`T **` in pret's C points at one).
        impl<T> Word for Ptr<T> {
            const W: Width = Width::Word;
            fn from_bus(v: u32) -> Ptr<T> {
                Ptr::at(v)
            }
            fn to_bus(self) -> u32 {
                self.0
            }
        }

        #game
    }
}

/// The `Game` trait: [`readable_items`]' accesses, for `clock`.
fn game_trait(clock: Clock) -> TokenStream {
    if clock != Clock::Exact {
        return quote! {
            /// The machine as the readable form reads and writes it,
            /// without mGBA's timing (`gbaa-recompile --timing instant|static`):
            /// an access is the bus's and nothing more.
            pub trait Game {
                /// `ldr rN, =X`: the literal-pool word at `lit`.
                fn pool(&mut self, lit: u32) -> u32;
                /// `*p`, as the CPU loads it: widened to the register it
                /// lands in, a misaligned halfword or word rotated.
                fn get<T: Word>(&mut self, p: Ptr<T>) -> u32;
                /// The bus's aligned read at `p`, before the CPU's rotate
                /// (which the code then does itself).
                fn load<T: Word>(&mut self, p: Ptr<T>) -> u32;
                /// `*p = v`: `v` is the register stored, which the bus
                /// narrows to `T`'s width.
                fn set<T: Word>(&mut self, p: Ptr<T>, v: u32);
                /// `p`'s bitfield set to `v` (`x.f = v;`) as the code sets
                /// it: a load, then a store of what it read with the
                /// field's bits replaced. The value read and the value
                /// stored, as the registers hold them.
                fn set_bits<T: Word, const LO: u32, const N: u32>(
                    &mut self,
                    p: Ptr<Bits<T, LO, N>>,
                    v: u32,
                ) -> (u32, u32) {
                    let old = self.get(p);
                    let mask = (u32::MAX >> (32 - N)) << LO;
                    let new = (old & !mask) | ((v << LO) & mask);
                    self.set(p, new);
                    (old, new)
                }
            }
        };
    }
    quote! {
        /// The machine as the readable form reads and writes it. Each
        /// access names its Thumb instruction's address `at`: the prefetch
        /// stall after it depends on where it runs.
        pub trait Game {
            /// `ldr rN, =X` at `at`: the literal-pool word at `lit`.
            fn pool(&mut self, at: u32, lit: u32) -> u32;
            /// `*p`, the load at `at`, as the CPU loads it: widened to the
            /// register it lands in, a misaligned halfword or word rotated.
            fn get<T: Word>(&mut self, at: u32, p: Ptr<T>) -> u32;
            /// The bus's aligned read at `p`, before the CPU's rotate (which
            /// the code then does itself).
            fn load<T: Word>(&mut self, at: u32, p: Ptr<T>) -> u32;
            /// `*p = v`, the store at `at`: `v` is the register stored,
            /// which the bus narrows to `T`'s width.
            fn set<T: Word>(&mut self, at: u32, p: Ptr<T>, v: u32);
            /// `p`'s bitfield set to `v` (`x.f = v;`) as the code sets it:
            /// the load at `lat`, then the store at `sat` of what it read
            /// with the field's bits replaced. The value read and the
            /// value stored, as the registers hold them.
            fn set_bits<T: Word, const LO: u32, const N: u32>(
                &mut self,
                lat: u32,
                sat: u32,
                p: Ptr<Bits<T, LO, N>>,
                v: u32,
            ) -> (u32, u32) {
                let old = self.get(lat, p);
                let mask = (u32::MAX >> (32 - N)) << LO;
                let new = (old & !mask) | ((v << LO) & mask);
                self.set(sat, p, new);
                (old, new)
            }
            /// [`Game::get`] with the pool load of its address (`ldr rN,
            /// =X` at `pat`, the word at `lit`) before it.
            fn get_via<T: Word>(&mut self, pat: u32, lit: u32, at: u32, p: Ptr<T>) -> u32 {
                self.pool(pat, lit);
                self.get(at, p)
            }
            /// [`Game::set_bits`] after the pool load of its address.
            fn set_bits_via<T: Word, const LO: u32, const N: u32>(
                &mut self,
                pat: u32,
                lit: u32,
                lat: u32,
                sat: u32,
                p: Ptr<Bits<T, LO, N>>,
                v: u32,
            ) -> (u32, u32) {
                self.pool(pat, lit);
                self.set_bits(lat, sat, p, v)
            }
            /// [`Game::load`] after the pool load of its address.
            fn load_via<T: Word>(&mut self, pat: u32, lit: u32, at: u32, p: Ptr<T>) -> u32 {
                self.pool(pat, lit);
                self.load(at, p)
            }
            /// [`Game::set`] after the pool load of its address.
            fn set_via<T: Word>(&mut self, pat: u32, lit: u32, at: u32, p: Ptr<T>, v: u32) {
                self.pool(pat, lit);
                self.set(at, p, v)
            }
        }
    }
}

/// `impl Game for #ty`: [`readable_items`]' accesses on a crate's machine.
pub fn readable_impl(ty: TokenStream, clock: Clock) -> TokenStream {
    if clock != Clock::Exact {
        return quote! {
            impl Game for #ty {
                #[inline(always)]
                fn pool(&mut self, lit: u32) -> u32 {
                    self.bus.load(Width::Word, lit & !3)
                }
                #[inline(always)]
                fn get<T: Word>(&mut self, p: Ptr<T>) -> u32 {
                    let a = p.0;
                    match T::W {
                        Width::Byte => self.bus.load(Width::Byte, a),
                        Width::Half => self.bus.load(Width::Half, a & !1).rotate_right((a & 1) << 3),
                        Width::Word => self.bus.load(Width::Word, a & !3).rotate_right((a & 3) << 3),
                    }
                }
                #[inline(always)]
                fn load<T: Word>(&mut self, p: Ptr<T>) -> u32 {
                    let a = match T::W {
                        Width::Byte => p.0,
                        Width::Half => p.0 & !1,
                        Width::Word => p.0 & !3,
                    };
                    self.bus.load(T::W, a)
                }
                #[inline(always)]
                fn set<T: Word>(&mut self, p: Ptr<T>, v: u32) {
                    let a = match T::W {
                        Width::Byte => p.0,
                        Width::Half => p.0 & !1,
                        Width::Word => p.0 & !3,
                    };
                    self.bus.store(T::W, a, v);
                }
            }
        };
    }
    quote! {
        impl Game for #ty {
            #[inline(always)]
            fn pool(&mut self, at: u32, lit: u32) -> u32 {
                let v = self.bus.load(Width::Word, lit & !3);
                rt::stall(self, (at >> 24) as usize, at.wrapping_add(4), true);
                v
            }
            #[inline(always)]
            fn get<T: Word>(&mut self, at: u32, p: Ptr<T>) -> u32 {
                let a = p.0;
                let v = match T::W {
                    Width::Byte => self.bus.load(Width::Byte, a),
                    Width::Half => self.bus.load(Width::Half, a & !1).rotate_right((a & 1) << 3),
                    Width::Word => self.bus.load(Width::Word, a & !3).rotate_right((a & 3) << 3),
                };
                rt::stall(self, (at >> 24) as usize, at.wrapping_add(4), true);
                v
            }
            #[inline(always)]
            fn load<T: Word>(&mut self, at: u32, p: Ptr<T>) -> u32 {
                let a = match T::W {
                    Width::Byte => p.0,
                    Width::Half => p.0 & !1,
                    Width::Word => p.0 & !3,
                };
                let v = self.bus.load(T::W, a);
                rt::stall(self, (at >> 24) as usize, at.wrapping_add(4), true);
                v
            }
            #[inline(always)]
            fn set<T: Word>(&mut self, at: u32, p: Ptr<T>, v: u32) {
                let a = match T::W {
                    Width::Byte => p.0,
                    Width::Half => p.0 & !1,
                    Width::Word => p.0 & !3,
                };
                self.bus.store(T::W, a, v);
                rt::stall(self, (at >> 24) as usize, at.wrapping_add(4), false);
            }
        }
    }
}

/// The readable form's macros, at each generated crate's root (before the
/// modules that use them). Each expands to the play view's statements for
/// the same exit, in the same order; `{ sync }` is the registers and flags
/// the block set (`regs!`), written before anything can see them, and a
/// second block the flags only a yield writes.
pub fn readable_macros(clock: Clock) -> TokenStream {
    let (tick, frame) = match clock {
        Clock::Exact => (quote!(rt::tick), frame_macros()),
        _ => (quote!(rt::tick_flat), frame_macros_untimed()),
    };
    quote! {
        /// A comment the printer turns into `// …` (`crate::dwarf`).
        /// Expands to nothing.
        macro_rules! note {
            ($($t:tt)*) => {};
        }
        /// The guest's registers and flags a block set, written into
        /// `Machine`: `regs!(m; 0 = x, 4 = y; n = a, z = b)`.
        macro_rules! regs {
            ($m:ident; $($r:literal = $v:expr),* $(; $($f:ident = $fv:expr),*)?) => {{
                $( $m.cpu.r[$r] = $v; )*
                $( $( regs!(@flag $m, $f, $fv); )* )?
            }};
            (@flag $m:ident, n, $v:expr) => { $m.cpu.cpsr = $m.cpu.cpsr & !(1 << 31) | (($v) as u32) << 31 };
            (@flag $m:ident, z, $v:expr) => { $m.cpu.cpsr = $m.cpu.cpsr & !(1 << 30) | (($v) as u32) << 30 };
            (@flag $m:ident, c, $v:expr) => { $m.cpu.cpsr = $m.cpu.cpsr & !(1 << 29) | (($v) as u32) << 29 };
            (@flag $m:ident, v, $v:expr) => { $m.cpu.cpsr = $m.cpu.cpsr & !(1 << 28) | (($v) as u32) << 28 };
        }
        #frame
        /// The end of a block that goes on in the function at `k`: its
        /// cycles, and a yield if its tick takes one. After `;`, the
        /// registers Rust locals carry on, with the value each holds (`0 =
        /// r0`): a yield taken in place that left one changed unwinds to
        /// the dispatcher at `k`.
        macro_rules! tick {
            ($m:ident, $pc:ident = $k:expr, $n:expr $(, { $($s:tt)* } $(, { $($y:tt)* })?)? $(; $($r:literal = $l:expr),+)?) => {{
                $( $($s)* )?
                let cycles: u32 = $n;
                $pc = $k;
                if #tick($m, cycles) {
                    $( $( $($y)* )? )?
                    if let Some(k) = yield_at($m, $pc) {
                        return Flow::Jump(k);
                    }
                    $(
                        if false $(|| $m.cpu.r[$r] != $l)+ {
                            return Flow::Jump($pc);
                        }
                    )?
                }
            }};
        }
        /// A parameter, as the function was called with it: argument
        /// register `r` read as the `let`'s type (`u32`, or a `Ptr`).
        macro_rules! arg {
            ($m:ident, $r:literal) => {
                Word::from_bus($m.cpu.r[$r])
            };
        }
        /// A Thumb exit at `at` the dispatcher finds the target of (a `bx`
        /// return): the block's cycles, then out. `return v`: the value it
        /// returns, into r0 before the rest of its sync.
        macro_rules! ret {
            ($m:ident, $pc:ident @ $at:literal, $e:expr, $n:expr $(, return $v:expr)? $(, { $($s:tt)* } $(, { $($y:tt)* })?)?) => {{
                $( $m.cpu.r[0] = $v; )?
                $( $($s)* )?
                let cycles: u32 = $n;
                $pc = rt::exit($m, $at, true, $e);
                if #tick($m, cycles) {
                    $( $( $($y)* )? )?
                    if let Some(k) = yield_at($m, $pc) {
                        return Flow::Jump(k);
                    }
                }
                return Flow::Jump($pc);
            }};
        }
        /// `bl f` returning to `ret`: the block's cycles, lr set to `ret`,
        /// the call to `f` at its entry key (`keys::f`), and on with the
        /// caller only if it came back to `ret` (else out to the
        /// dispatcher).
        ///
        /// `call!(m, pc, f, { … })`, with no return site: a call a mod
        /// adds (`rt::modcall`), whose value is the callee's r0. The sync
        /// runs, then lr gets the site's reserved key and a record below
        /// sp keeps the registers as the block found them and `pc`, the key
        /// it began at; the call is free. Back at the key, every register
        /// and flag is put back as the block had it and the mod goes on,
        /// with `pc` as it was. If a state load, a rewind or an unwind
        /// inside the call lost this frame, the guest goes on at `pc` with
        /// the record's registers instead: the block runs again from its
        /// start (the stock code, or the mod's from its top at its entry).
        macro_rules! call {
            ($m:ident, $pc:ident, $f:ident -> $ret:literal, $n:expr $(, { $($s:tt)* } $(, { $($y:tt)* })?)?) => {{
                call_any!($m, $pc, $f -> $ret, $n $(, { $($s)* } $(, { $($y)* })?)?);
                if $pc != $ret {
                    return Flow::Jump($pc);
                }
            }};
            ($m:ident, $pc:ident, $f:ident $(, { $($s:tt)* })?) => {{
                const KEY: u32 = rt::modcall::call_key(file!(), line!(), column!());
                let (at, regs) = ($pc, rt::modcall::regs($m));
                $( $($s)* )?
                let sp0 = rt::modcall::enter($m, &regs, at, KEY);
                let Flow::Jump(a) = $f($m, keys::$f);
                $pc = settle($m, a, KEY, sp0);
                if $pc != KEY {
                    return Flow::Jump($pc);
                }
                if rt::yielding($m) {
                    if let Some(k) = yield_at($m, KEY) {
                        return Flow::Jump(k);
                    }
                }
                let r0 = $m.cpu.r[0];
                rt::modcall::leave($m, &regs);
                $pc = at;
                r0
            }};
        }
        /// [`call!`], going on wherever the callee came back to (a `match
        /// pc` follows).
        macro_rules! call_any {
            ($m:ident, $pc:ident, $f:ident -> $ret:literal, $n:expr $(, { $($s:tt)* } $(, { $($y:tt)* })?)?) => {{
                $( $($s)* )?
                $m.cpu.r[14] = $ret;
                let cycles: u32 = $n;
                $pc = keys::$f;
                if #tick($m, cycles) {
                    $( $( $($y)* )? )?
                    if let Some(k) = yield_at($m, $pc) {
                        return Flow::Jump(k);
                    }
                }
                let sp0 = $m.cpu.r[13];
                let Flow::Jump(a) = $f($m, keys::$f);
                $pc = settle($m, a, $ret, sp0);
                if rt::yielding($m) {
                    return Flow::Jump($pc);
                }
            }};
        }
    }
}

/// `push!` and `pop!` in mGBA's model: an `stm`/`ldm`'s accesses
/// ([`armv4t::cycles::block_accesses`]) and the stall after it, from the
/// instruction's address `at`.
fn frame_macros() -> TokenStream {
    quote! {
        /// Thumb `push {…}` at `at`, the lowest slot at `base`: the
        /// registers' values, lowest first.
        macro_rules! push {
            ($m:ident, $at:literal, $base:expr, [$($v:expr),+]) => {{
                let vs: [u32; _] = [$($v),+];
                let base: u32 = $base;
                rt::block($m, vs.len() as u32);
                for (i, v) in vs.into_iter().enumerate() {
                    $m.bus.store(Width::Word, base.wrapping_add(4 * i as u32) & !3, v);
                }
                rt::stall($m, ($at as u32 >> 24) as usize, ($at as u32).wrapping_add(4), false);
            }};
        }
        /// Thumb `pop {…}` at `at`: `n` words from `base`, lowest first.
        macro_rules! pop {
            ($m:ident, $at:literal, $base:expr, $n:literal) => {{
                let base: u32 = $base;
                rt::block($m, $n);
                let mut vs = [0u32; $n];
                for (i, v) in vs.iter_mut().enumerate() {
                    *v = $m.bus.load(Width::Word, base.wrapping_add(4 * i as u32) & !3);
                }
                rt::stall($m, ($at as u32 >> 24) as usize, ($at as u32).wrapping_add(4), true);
                vs
            }};
        }
    }
}

/// `push!` and `pop!` without mGBA's model: only the accesses.
fn frame_macros_untimed() -> TokenStream {
    quote! {
        /// Thumb `push {…}`, the lowest slot at `base`: the
        /// registers' values, lowest first.
        macro_rules! push {
            ($m:ident, $base:expr, [$($v:expr),+]) => {{
                let vs: [u32; _] = [$($v),+];
                let base: u32 = $base;
                for (i, v) in vs.into_iter().enumerate() {
                    $m.bus.store(Width::Word, base.wrapping_add(4 * i as u32) & !3, v);
                }
            }};
        }
        /// Thumb `pop {…}`: `n` words from `base`, lowest first.
        macro_rules! pop {
            ($m:ident, $base:expr, $n:literal) => {{
                let base: u32 = $base;
                let mut vs = [0u32; $n];
                for (i, v) in vs.iter_mut().enumerate() {
                    *v = $m.bus.load(Width::Word, base.wrapping_add(4 * i as u32) & !3);
                }
                vs
            }};
        }
    }
}

/// The prelude and `insn!`: what every generated crate's `lib.rs` starts
/// with. `rt` is the path of the dispatch helpers; `alias` defines `M`, the
/// machine type generated fns take.
pub fn lib_head(rt: TokenStream, alias: TokenStream, clock: Clock) -> TokenStream {
    let (items, macros) = (readable_items(clock), readable_macros(clock));
    quote! {
        /// Marks where each guest instruction starts. Expands to nothing.
        macro_rules! insn {
            ($($t:tt)*) => {};
        }
        #macros

        pub mod prelude {
            pub use #rt::{self as rt, Flow};
            pub use armv4t::interp::{Bus as _, Exit};
            pub use armv4t::sem::prim::{self, MulKind};
            pub use armv4t::sem::{Flag, PsrMask, Reg, ShiftType, Width};
            #items
            #alias
        }
    }
}

/// `prelude::keys`: each function's entry key, a `u32` constant named as
/// its fn is, which `call!` and `call_any!` call it at. One name per key;
/// a later duplicate of a name is left out.
pub(crate) fn keys_module(fns: impl IntoIterator<Item = (proc_macro2::Ident, u32)>) -> TokenStream {
    let mut by_name: BTreeMap<String, (proc_macro2::Ident, u32)> = BTreeMap::new();
    for (id, k) in fns {
        by_name.entry(id.to_string()).or_insert((id, k));
    }
    let consts = by_name.into_values().map(|(id, k)| {
        let k = hex(k);
        quote!(pub const #id: u32 = #k;)
    });
    quote! {
        /// Each function's entry key, by its fn's name: where `call!`
        /// calls it.
        #[allow(non_upper_case_globals)]
        pub mod keys {
            #(#consts)*
        }
    }
}

/// The ROM crate's prelude additions to [`lib_head`]: `M`, and the calls
/// through its own table.
pub fn rom_alias(clock: Clock) -> TokenStream {
    let game = readable_impl(quote!(M), clock);
    quote! {
        pub type M = runtime::Machine<crate::P>;
        #game
        /// An indirect call through this ROM's table.
        #[inline]
        pub fn call(m: &mut M, key: u32) -> Flow {
            // SAFETY: the main crate defines `gbaa_code` as a `Code<P>`.
            rt::call(m, unsafe { &crate::CODE }, key)
        }
        /// A call that came back at `key`, run on to its return site
        /// `ret` (`rt::settle`).
        #[inline]
        pub fn settle(m: &mut M, key: u32, ret: u32, sp0: u32) -> u32 {
            // SAFETY: as in `call`.
            rt::settle(m, unsafe { &crate::CODE }, key, ret, sp0)
        }
        /// A tick found an IRQ takeable or the CPU halted at `key`: take it
        /// here (`rt::yield_at`); `Some` to unwind with.
        #[inline]
        pub fn yield_at(m: &mut M, key: u32) -> Option<u32> {
            // SAFETY: as in `call`.
            rt::yield_at(m, unsafe { &crate::CODE }, key)
        }
    }
}

/// Guest instructions per part crate, at most (a function is never split).
/// rustc's memory grows with a crate's size and cargo builds parts side by
/// side, so a big ROM builds in several smaller crates: Emerald's 815k
/// instructions in one crate took over 16 GB.
pub const PART_INSNS: usize = 20_000;

/// The functions, each with the context it was built in, cut into parts of
/// at most [`PART_INSNS`] instructions, in order.
pub fn parts<'a>(funcs: &[(&'a Ctx<'a>, &'a Func)]) -> Vec<Vec<(&'a Ctx<'a>, &'a Func)>> {
    let mut out: Vec<Vec<_>> = vec![Vec::new()];
    let mut n = 0;
    for &(c, f) in funcs {
        let k: usize = f.blocks.values().map(|gb| gb.insns.len()).sum();
        if n > 0 && n + k > PART_INSNS {
            out.push(Vec::new());
            n = 0;
        }
        out.last_mut().unwrap().push((c, f));
        n += k;
    }
    out
}

/// The symbol a function is exported as, for the parts that call it.
pub(crate) fn symbol(entry: u32) -> String {
    format!("gbaa_{}", fn_name(entry))
}

/// A named function's doc comment: its name and file, and its prototype
/// if the map has one. Nothing for a function the map doesn't name.
pub(crate) fn fn_doc(names: &SourceMap, entry: u32) -> TokenStream {
    let Some(item) = names.functions.get(&entry) else {
        return TokenStream::new();
    };
    let head = match &item.file {
        Some(file) => format!(" {}, {file}", item.name),
        None => format!(" {}", item.name),
    };
    let proto = item.proto.as_ref().map(|p| {
        let p = format!(" `{p}`");
        quote!(#[doc = ""] #[doc = #p])
    });
    quote!(#[doc = #head] #proto)
}

/// `base`'s `names` module and `addr!`, for a named print: each named
/// datum, and each named function's key, as a `u32` constant; and the
/// typed `globals` ([`crate::types`]). Empty for the play view.
/// `exported`: `addr!` is the crate's (`#[macro_export]`, `base`'s), else
/// a macro of the module it is printed into (a `fn-diff` form's).
pub(crate) fn names_module(names: Option<&SourceMap>, exported: bool) -> TokenStream {
    let Some(n) = names else {
        return TokenStream::new();
    };
    let consts = n.idents().map(|(id, _, k)| {
        let (id, v) = (quote::format_ident!("{id}"), hex(k));
        quote!(pub const #id: u32 = #v;)
    });
    let globals = n.typed.module(n);
    let addr = if exported {
        quote! {
            /// A data address by name, `addr!(gHeap)`, or an offset into one,
            /// `addr!(gHeap + 8)`, or a function's key, `addr!(TaskDummy)`:
            /// one `u32` expression.
            #[macro_export]
            macro_rules! addr {
                ($n:ident) => {
                    $crate::names::$n
                };
                ($n:ident + $o:literal) => {
                    ($crate::names::$n + $o)
                };
            }
        }
    } else {
        quote! {
            /// As `base`'s `addr!`, from this module's `names`.
            macro_rules! addr {
                ($n:ident) => {
                    names::$n
                };
                ($n:ident + $o:literal) => {
                    (names::$n + $o)
                };
            }
        }
    };
    quote! {
        /// The source map's data and functions, by name: addresses and keys.
        #[allow(non_upper_case_globals)]
        pub mod names {
            #(#consts)*
        }
        #addr
        #globals
    }
}

/// Write one part crate's source in `src`: its functions as `mNNN.rs`
/// modules, and a `lib.rs` declaring the functions of other parts it
/// calls. `exported`: the functions other parts call, which it exports by
/// symbol. Returns the functions whose registers stay in `Machine`, with
/// why ([`crate::lower::lower_func_checked`]).
pub fn write_part(
    files: &mut Files,
    src: &Path,
    funcs: &[(&Ctx, &Func)],
    exported: &std::collections::HashSet<u32>,
) -> std::io::Result<Vec<(u32, String)>> {
    let mut mods = Vec::new();
    let mut unchecked = Vec::new();
    for (i, chunk) in funcs.chunks(PER_MODULE).enumerate() {
        let name = quote::format_ident!("m{:03}", i);
        let fns = chunk.iter().map(|(ctx, f)| {
            let (tokens, why) = print_fn(ctx, f, exported.contains(&f.entry));
            unchecked.extend(why.map(|w| (f.entry, w)));
            tokens
        });
        let fns: Vec<_> = fns.collect();
        let file = quote! {
            #[allow(unused_imports)]
            use super::*;
            use crate::prelude::*;
            #(#fns)*
        };
        files.write(&src.join(format!("{name}.rs")), pretty(file))?;
        mods.push(name);
    }
    let mods = quote!(#(mod #mods; pub use #mods::*;)*);
    files.write(
        &src.join("lib.rs"),
        pretty(part_lib(funcs, &Default::default(), mods, true)),
    )?;
    Ok(unchecked)
}

/// `f` as its part prints it: its doc comment if `ctx` names it, its
/// symbol if `export`, and its fn; with why its registers stay in
/// `Machine`, if they do ([`crate::lower::lower_func_checked`]).
pub(crate) fn print_fn(ctx: &Ctx, f: &Func, export: bool) -> (TokenStream, Option<String>) {
    let attr = export.then(|| {
        let s = symbol(f.entry);
        quote!(#[unsafe(export_name = #s)])
    });
    let (body, why) = lower_func_checked(ctx, f, &ctx.fn_ident(f.entry));
    let doc = ctx.names.map(|n| fn_doc(n, f.entry));
    (quote!(#doc #attr #body), why)
}

/// A part crate's `lib.rs`: the prelude, the functions of other parts
/// `funcs` and `extra` call (by symbol; `extra`, the entries a mod's own
/// code calls), and `mods`, the modules holding `funcs`.
pub(crate) fn part_lib(
    funcs: &[(&Ctx, &Func)],
    extra: &std::collections::BTreeSet<u32>,
    mods: TokenStream,
    globals: bool,
) -> TokenStream {
    let names = funcs.first().and_then(|(c, _)| c.names);
    let clock = funcs.first().map_or(Clock::Exact, |(c, _)| c.cfg.clock);
    let ident = |e: u32| names.map_or_else(|| fn_name(e), |n| n.fn_ident(e));
    let here: std::collections::HashSet<u32> = funcs.iter().map(|(_, f)| f.entry).collect();
    let outside: std::collections::BTreeSet<u32> = funcs
        .iter()
        .flat_map(|(_, f)| f.calls.iter().copied())
        .chain(extra.iter().copied())
        .filter(|e| !here.contains(e))
        .collect();
    let decls = outside.iter().map(|&e| {
        let (f, s) = (ident(e), symbol(e));
        quote! {
            #[link_name = #s]
            pub fn #f(m: &mut M, pc: u32) -> Flow;
        }
    });
    // prettyplease can't print `safe fn` in an extern block, so each is
    // wrapped in a safe fn of the same name.
    let wraps = outside.iter().map(|&e| {
        let f = ident(e);
        quote! {
            #[inline]
            pub(crate) fn #f(m: &mut M, pc: u32) -> Flow {
                // SAFETY: the part defining this symbol exports it with this signature.
                unsafe { ext::#f(m, pc) }
            }
        }
    });
    // The typed globals and markers, only where the functions are the
    // generator's: a mod's own files may name the same.
    let addr = names.map(|_| {
        let globals = globals.then(|| {
            quote!(
                pub(crate) use base::globals::*;
            )
        });
        quote!(
            pub(crate) use base::addr;
            #globals
        )
    });
    let macros = readable_macros(clock);
    quote! {
        //! Generated by `gbaa-recompile`. Don't edit; rerun `just recompile`.
        #![allow(unused, clippy::all, non_snake_case)]
        /// Marks where each guest instruction starts. Expands to nothing.
        macro_rules! insn {
            ($($t:tt)*) => {};
        }
        #macros
        use base::prelude;
        use prelude::*;
        #addr
        /// The functions of other parts this one calls, by symbol.
        mod ext {
            use crate::prelude::*;
            unsafe extern "Rust" {
                #(#decls)*
            }
        }
        #(#wraps)*
        #mods
    }
}

/// The code copied into IWRAM, recompiled: one copy's discovery and its
/// functions, each block numbered as an overlay ([`Func::block_overlay`]).
pub struct RamCode {
    pub copy: Overlay,
    pub code: Recompiled,
}

/// The guard ranges of the overlay block at `key` of `f`: its instruction
/// bytes and the stubs in the copy its function calls through, merged, as
/// (IWRAM address, length, ROM address).
pub fn guard_ranges(copy: &Overlay, f: &Func, key: u32) -> Vec<(u32, u32, u32)> {
    let in_copy = |a: u32| (copy.dst..copy.dst + copy.len).contains(&a);
    let mut bytes: Vec<(u32, u32)> = f
        .blocks
        .get(&key)
        .into_iter()
        .flat_map(|gb| {
            let n = if gb.block.thumb { 2 } else { 4 };
            gb.insns.iter().map(move |(a, _)| (*a, *a + n))
        })
        .chain(f.stubs.iter().map(|&k| {
            let a = k & !1;
            (a, a + if k & 1 != 0 { 2 } else { 4 })
        }))
        .filter(|&(a, b)| in_copy(a) && b <= copy.dst + copy.len)
        .collect();
    bytes.sort_unstable();
    let mut out: Vec<(u32, u32)> = Vec::new();
    for (a, b) in bytes {
        match out.last_mut() {
            Some(l) if a <= l.1 => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out.into_iter()
        .map(|(a, b)| (a, b - a, copy.rom_addr(a)))
        .collect()
}

/// The code the ROM copies into IWRAM, recompiled with `cfg`, each block
/// numbered as an overlay; a copy past the runtime's overlays is left out.
pub fn ram_code(rom: &[u8], disc: &Discovery, variants: u32, cfg: Config) -> Vec<RamCode> {
    // The IWRAM copies, static and seen running ([`crate::seen`]), each
    // built at its destination, with the versions of their blocks the code
    // patches them to; every version of every block an overlay, numbered
    // across all of them.
    let mut copies = overlays(rom, disc);
    let seen = crate::seen::overlays(rom, &copies, variants);
    copies.extend(seen);
    let mut ram: Vec<RamCode> = copies
        .into_iter()
        .map(|copy| {
            let o = (copy.src - analysis::program::ROM_BASE) as usize;
            let mut c = Ctx::at(copy.dst, &rom[o..o + copy.len as usize], &copy.disc);
            c.cfg = cfg;
            let code = build_all(&c);
            RamCode { copy, code }
        })
        .collect();
    crate::variants::add(rom, &mut ram, variants);
    let mut next = 0u16;
    let mut kept = Vec::new();
    for RamCode { copy, mut code } in ram {
        // The runtime guards at most MAX_OVERLAYS; a copy past that stays
        // interpreted.
        let blocks: usize = code.funcs.iter().map(Func::overlays).sum();
        if next as usize + blocks > runtime::overlay::MAX_OVERLAYS {
            eprintln!(
                "IWRAM copy at 0x{:08x}: over {} overlays, left to the interpreter",
                copy.dst,
                runtime::overlay::MAX_OVERLAYS
            );
            continue;
        }
        for f in &mut code.funcs {
            f.overlay = Some(next);
            next += f.overlays() as u16;
        }
        kept.push(RamCode { copy, code });
    }
    kept
}

/// The context each of `ram`'s copies is printed in.
pub fn ram_ctx<'a>(rom: &'a [u8], ram: &'a [RamCode], cfg: Config) -> Vec<Ctx<'a>> {
    ram.iter()
        .map(|rc| {
            let o = (rc.copy.src - analysis::program::ROM_BASE) as usize;
            let mut c = Ctx::at(
                rc.copy.dst,
                &rom[o..o + rc.copy.len as usize],
                &rc.copy.disc,
            );
            c.cfg = cfg;
            c
        })
        .collect()
}

/// Every function [`write_crate`] prints, as keys: the ROM's and its
/// IWRAM copies' (a readable list's `*`).
pub fn all_entries(
    rom: &[u8],
    disc: &Discovery,
    variants: u32,
    cfg: Config,
) -> std::collections::BTreeSet<u32> {
    let ram = ram_code(rom, disc, variants, cfg);
    let mut keys = Ctx::new(rom, disc).entries;
    for c in ram_ctx(rom, &ram, cfg) {
        keys.extend(c.entries);
    }
    keys
}

/// Recompile `rom` into the crate at `dir`, making the transformations
/// `cfg` turns on, and naming what `names` names (the play view: `None`).
/// The functions in `readable` are printed in the readable form
/// ([`Ctx::cfg_for`]). The package is `gbaa-<name>`; the binary carries
/// `game` as the game's name ([`runtime::name`]). `crates` is the
/// directory holding `runtime` and `armv4t`.
#[allow(clippy::too_many_arguments)]
pub fn write_crate(
    dir: &Path,
    name: &str,
    game: &str,
    crates: &Path,
    rom: &[u8],
    disc: &Discovery,
    variants: u32,
    cfg: Config,
    names: Option<&SourceMap>,
    readable: Option<&std::collections::BTreeSet<u32>>,
    modded: Option<&crate::modcopy::Mod>,
) -> std::io::Result<Recompiled> {
    let name = &crate_name(name);
    // Only ever replace a crate this wrote: OUT could be anything.
    let ours = std::fs::read_to_string(dir.join("Cargo.toml"))
        .is_ok_and(|c| c.starts_with("# Generated by gbaa-recompile"));
    let empty = std::fs::read_dir(dir).map_or(true, |mut d| d.next().is_none());
    if !ours && !empty {
        return Err(std::io::Error::other(format!(
            "{} isn't empty and isn't a crate gbaa-recompile wrote",
            dir.display()
        )));
    }
    let mut ctx = Ctx::new(rom, disc);
    ctx.cfg = cfg;
    ctx.names = names;
    ctx.readable = readable;
    let r = build_all(&ctx);
    ctx.prove_saves(&r.funcs);
    let ram = ram_code(rom, disc, variants, cfg);
    let mut ram_ctx = ram_ctx(rom, &ram, cfg);
    for c in &mut ram_ctx {
        c.names = names;
        c.readable = readable;
    }
    // Marks the crate as ours before writing it, so a run that dies
    // midway leaves a crate the next run may replace. One already ours
    // keeps its Cargo.toml, which is rewritten below only if it changed.
    if !ours {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join("Cargo.toml"), GENERATED)?;
    }
    // Each file is written only if its bytes changed, and only the files
    // no longer generated are deleted, so cargo rebuilds just the crates
    // whose code changed.
    let mut files = Files::default();
    let mut all: Vec<(&Ctx, &Func)> = r.funcs.iter().map(|f| (&ctx, f)).collect();
    for (c, rc) in ram_ctx.iter().zip(&ram) {
        all.extend(rc.code.funcs.iter().map(|f| (c, f)));
    }
    // A mod's functions leave their parts for the `modded` crate; the
    // other functions keep the parts an unmodded build gives them.
    let cut: std::collections::BTreeSet<u32> =
        modded.map_or_else(Default::default, |m| m.funcs.keys().copied().collect());
    let mut mine: Vec<(&Ctx, &Func)> = r
        .funcs
        .iter()
        .filter(|f| cut.contains(&f.entry))
        .map(|f| (&ctx, f))
        .collect();
    if let Some(&k) = cut
        .iter()
        .find(|k| !mine.iter().any(|(_, f)| f.entry == **k))
    {
        return Err(std::io::Error::other(format!(
            "the mod's function 0x{k:08x} isn't a function of this ROM"
        )));
    }
    let mut parts = parts(&all);
    for p in &mut parts {
        p.retain(|(c, f)| !(std::ptr::eq(*c, &ctx) && cut.contains(&f.entry)));
    }
    if modded.is_some() {
        parts.push(std::mem::take(&mut mine));
    }
    // The functions a mod's own code calls, which its cut may never have
    // (a call the mod added).
    let added = match modded {
        Some(m) => crate::modcopy::called(m, names).map_err(std::io::Error::other)?,
        None => Default::default(),
    };
    // A function is exported if a function in another part calls it.
    let mut exported = std::collections::HashSet::new();
    let part_of: std::collections::HashMap<u32, usize> = parts
        .iter()
        .enumerate()
        .flat_map(|(i, p)| p.iter().map(move |(_, f)| (f.entry, i)))
        .collect();
    for (i, p) in parts.iter().enumerate() {
        for (_, f) in p {
            exported.extend(f.calls.iter().filter(|e| part_of.get(e) != Some(&i)));
        }
    }
    if let Some(&k) = added.iter().find(|k| !part_of.contains_key(k)) {
        return Err(std::io::Error::other(format!(
            "the mod calls 0x{k:08x}, which isn't a function of this build"
        )));
    }
    let last = parts.len() - 1;
    exported.extend(added.iter().filter(|e| part_of.get(e) != Some(&last)));
    let path = |c: &str| crates.join(c).display().to_string();
    let deps = format!(
        "armv4t = {{ path = \"{}\" }}\nruntime = {{ path = \"{}\" }}\n",
        path("armv4t"),
        path("runtime"),
    );
    let mut part_deps = String::new();
    let mut part_mods = Vec::new();
    let mut unchecked = Vec::new();
    let mut stale = Vec::new();
    if let Some(m) = modded {
        let mine = parts.pop().expect("the modded part");
        stale = crate::modcopy::write_modded(&mut files, dir, name, m, &mine, &added, &deps)?;
        part_deps +=
            &format!("modded = {{ package = \"gbaa-{name}-modded\", path = \"modded\" }}\n");
    }
    for (i, p) in parts.iter().enumerate() {
        let pdir = dir.join(format!("parts/p{i:02}"));
        unchecked.extend(write_part(&mut files, &pdir.join("src"), p, &exported)?);
        files.write(
            &pdir.join("Cargo.toml"),
            format!(
                "{GENERATED}[package]\nname = \"gbaa-{name}-p{i:02}\"\n{PACKAGE}\n[dependencies]\n{deps}\
                 base = {{ package = \"gbaa-{name}-base\", path = \"../../base\" }}\n"
            ),
        )?;
        let _ = writeln!(
            part_deps,
            "p{i:02} = {{ package = \"gbaa-{name}-p{i:02}\", path = \"parts/p{i:02}\" }}"
        );
        part_mods.push(quote::format_ident!("p{:02}", i));
    }

    let n = r.table.len();
    // The table reaches a modded function by its symbol.
    let via = |f: u32| quote::format_ident!("mod_{}", fn_name(f));
    let entries = r.table.iter().map(|(&k, &f)| {
        let k = hex(k);
        let f = if cut.contains(&f) {
            via(f)
        } else {
            ctx.fn_ident(f)
        };
        quote!((#k, #f))
    });
    let mod_decls = cut.iter().map(|&e| {
        let (f, s) = (via(e), symbol(e));
        quote! {
            #[link_name = #s]
            pub fn #f(m: &mut M, pc: u32) -> Flow;
        }
    });
    let mod_wraps = cut.iter().map(|&e| {
        let f = via(e);
        quote! {
            #[inline]
            fn #f(m: &mut M, pc: u32) -> Flow {
                // SAFETY: the modded crate exports this symbol with this signature.
                unsafe { modded_ext::#f(m, pc) }
            }
        }
    });
    let modded_keys: Vec<_> = r
        .funcs
        .iter()
        .filter(|f| cut.contains(&f.entry))
        .flat_map(|f| f.blocks.keys().copied().filter(move |&k| k != f.entry))
        .collect::<std::collections::BTreeSet<u32>>()
        .into_iter()
        .map(hex)
        .collect();
    let n_modded = modded_keys.len();
    let folded: Vec<_> = folded(&ctx, &r.funcs)
        .into_iter()
        .map(|(a, b)| {
            let (a, b) = (hex(a), hex(b));
            quote!((#a, #b))
        })
        .collect();
    let n_folded = folded.len();
    let extent = |e: &Extent| {
        let (len, stub) = (proc_macro2::Literal::u16_unsuffixed(e.len), hex(e.stub));
        quote!(runtime::dispatch::Extent { len: #len, stub: #stub })
    };
    let extents: Vec<_> = r.extents.values().map(extent).collect();
    let n_inner = r.inner.len();
    let inner = r.inner.keys().map(|&k| hex(k));
    let inner_extents: Vec<_> = r.inner.values().map(extent).collect();
    // A RAM key's overlays are its block's versions, in the function the
    // table names.
    let mut ram_table: BTreeMap<u32, (u32, u16, u16)> = BTreeMap::new();
    let mut ram_extents: BTreeMap<u32, Extent> = BTreeMap::new();
    let mut guards = Vec::new();
    for rc in &ram {
        let funcs: BTreeMap<u32, &Func> = rc.code.funcs.iter().map(|f| (f.entry, f)).collect();
        for (&k, &f) in &rc.code.table {
            let o = funcs[&f].block_overlay(k).expect("an overlay block");
            ram_table
                .entry(k)
                .or_insert((f, o, funcs[&f].versions(k) as u16));
            ram_extents.entry(k).or_insert(rc.code.extents[&k]);
        }
        for f in &rc.code.funcs {
            for &key in f.blocks.keys() {
                let ranges = guard_ranges(&rc.copy, f, key);
                let rom_ranges = ranges.iter().map(|&(a, n, s)| {
                    let (a, n, s) = (hex(a), proc_macro2::Literal::u32_unsuffixed(n), hex(s));
                    quote!((#a, #n, #s))
                });
                guards.push(quote!(runtime::overlay::Overlay {
                    ranges: &[#(#rom_ranges),*],
                    image: None,
                }));
                // A variant's bytes are its own, the ranges' in order.
                for v in f.variants.get(&key).into_iter().flatten() {
                    let mut at = 0u32;
                    let own = ranges.iter().map(|&(a, n, _)| {
                        let (a, l, s) = (hex(a), proc_macro2::Literal::u32_unsuffixed(n), at);
                        at += n;
                        quote!((#a, #l, #s))
                    });
                    let own: Vec<_> = own.collect();
                    let bytes = proc_macro2::Literal::byte_string(&v.bytes);
                    guards.push(quote!(runtime::overlay::Overlay {
                        ranges: &[#(#own),*],
                        image: Some(#bytes),
                    }));
                }
            }
        }
    }
    let (n_ram, n_guards) = (ram_table.len(), guards.len());
    let ram_entries = ram_table.iter().map(|(&k, &(f, o, n))| {
        let (k, f) = (hex(k), ctx.fn_ident(f));
        let (o, n) = (
            proc_macro2::Literal::u16_unsuffixed(o),
            proc_macro2::Literal::u16_unsuffixed(n),
        );
        quote!((#k, #f, #o, #n))
    });
    let ram_extents: Vec<_> = ram_extents.values().map(extent).collect();
    let table = quote! {
        use crate::prelude::*;
        #(use #part_mods::*;)*
        /// The ROM's dispatch keys, sorted: every function's entry, and
        /// every block start of the functions printed in v1 form.
        pub static BLOCKS: [(u32, runtime::dispatch::GuestFn<crate::P>); #n] = [#(#entries),*];
        /// The other block starts of the structured functions, sorted.
        pub static INNER: [u32; #n_inner] = [#(#inner),*];
        /// Where each of `INNER` ticks.
        pub static INNER_EXTENTS: [runtime::dispatch::Extent; #n_inner] = [#(#inner_extents),*];
        /// Every block start of every overlay, sorted, with its first
        /// overlay and its number of versions.
        pub static RAM_BLOCKS: [(u32, runtime::dispatch::GuestFn<crate::P>, u16, u16); #n_ram] =
            [#(#ram_entries),*];
        /// Each overlay block version's guard: the IWRAM bytes it was
        /// compiled from.
        pub static OVERLAYS: [runtime::overlay::Overlay; #n_guards] = [#(#guards),*];
        /// Where each of `BLOCKS` ticks, for the block-timed interpreter.
        pub static EXTENTS: [runtime::dispatch::Extent; #n] = [#(#extents),*];
        /// The same for `RAM_BLOCKS`.
        pub static RAM_EXTENTS: [runtime::dispatch::Extent; #n_ram] = [#(#ram_extents),*];
        /// The block starts past the entry of the functions the mod
        /// replaces, sorted.
        pub static MODDED: [u32; #n_modded] = [#(#modded_keys),*];
        /// The ROM ranges const-prop folded loads from, sorted and merged.
        pub static FOLDED: [(u32, u32); #n_folded] = [#(#folded),*];
        /// The mod's functions, by symbol (`modded`).
        mod modded_ext {
            use crate::prelude::*;
            unsafe extern "Rust" {
                #(#mod_decls)*
            }
        }
        #(#mod_wraps)*
    };
    files.write(&dir.join("src/table.rs"), pretty(table))?;

    let fn_keys = keys_module(all.iter().map(|(c, f)| (c.fn_ident(f.entry), f.entry)));
    let alias = rom_alias(cfg.clock);
    let head = lib_head(
        quote!(runtime::dispatch),
        quote!(#alias #fn_keys),
        cfg.clock,
    );
    let timing = match cfg.clock {
        Clock::Exact => quote!(Exact),
        Clock::Static => quote!(Static),
        Clock::Instant => quote!(Instant),
        Clock::Native => quote!(Native),
    };
    let names_mod = names_module(names, true);
    let base = quote! {
        //! Generated by `gbaa-recompile`. Don't edit; rerun `just recompile`.
        //! What every part of the code shares: the prelude, the platform
        //! and the table, which the main crate defines.
        #![allow(unused, clippy::all, non_snake_case)]
        #head
        #names_mod
        /// The platform this build runs on. An ESP32 build changes this line.
        pub type P = desktop::Desktop;
        unsafe extern "Rust" {
            /// The recompiled ROM: the main crate's `CODE`.
            #[link_name = "gbaa_code"]
            pub static CODE: runtime::dispatch::Code<P>;
        }
    };
    files.write(&dir.join("base/src/lib.rs"), pretty(base))?;
    files.write(
        &dir.join("base/Cargo.toml"),
        format!(
            "{GENERATED}[package]\nname = \"gbaa-{name}-base\"\n{PACKAGE}\n[dependencies]\n{deps}\
             desktop = {{ path = \"{}\" }}\n",
            path("desktop")
        ),
    )?;
    let hash = proc_macro2::Literal::u64_unsuffixed(runtime::dispatch::rom_hash(rom));
    let spins = crate::spins::for_rom(rom)
        .into_iter()
        .map(proc_macro2::Literal::u32_unsuffixed);
    let seeds = std::path::absolute(dir.join("seeds.txt"))?
        .display()
        .to_string();
    // The mod's crate, linked though no path names it.
    let link_mod = modded.map(|_| {
        quote!(
            use modded as _;
        )
    });
    let lib = quote! {
        //! Generated by `gbaa-recompile`. Don't edit; rerun `just recompile`.
        //! The code is in the part crates (`parts/pNN`), at most
        //! `PART_INSNS` guest instructions each, so no one rustc holds it all.
        #![allow(unused, clippy::all, non_snake_case)]
        pub use base::{P, prelude};
        #link_mod
        mod table;
        /// The recompiled ROM.
        #[unsafe(export_name = "gbaa_code")]
        pub static CODE: runtime::dispatch::Code<P> = runtime::dispatch::Code {
            blocks: &table::BLOCKS,
            ram_blocks: &table::RAM_BLOCKS,
            overlays: &table::OVERLAYS,
            extents: &table::EXTENTS,
            ram_extents: &table::RAM_EXTENTS,
            inner: &table::INNER,
            inner_extents: &table::INNER_EXTENTS,
            modded: &table::MODDED,
            folded: &table::FOLDED,
            rom_hash: #hash,
            timing: runtime::Timing::#timing,
            seeds_file: Some(#seeds),
            spins: &[#(#spins),*],
        };
    };
    files.write(&dir.join("src/lib.rs"), pretty(lib))?;
    let krate = crate_ident(name);
    // The ROM the binary carries (MAIN's `include_bytes!`).
    files.write(&dir.join("rom.gba"), rom)?;
    let main = MAIN
        .replace("GEN", &krate)
        .replace("\"GAME\"", &format!("{game:?}"));
    files.write(&dir.join("src/main.rs"), main)?;
    let call = dir.join(format!("src/bin/call-{name}.rs"));
    files.write(&call, CALL.replace("GEN", &krate))?;

    let cargo = format!(
        r#"{GENERATED}[package]
name = "gbaa-{name}"
{PACKAGE}
# Its own workspace: not a member of gbaa's. The base and parts are members.
[workspace]

[dependencies]
{deps}desktop = {{ path = "{}" }}
base = {{ package = "gbaa-{name}-base", path = "base" }}
{part_deps}
[profile.release]
debug = false
incremental = false
"#,
        path("desktop"),
    );
    for d in ["src", "base", "parts", "modded"] {
        files.prune(&dir.join(d))?;
    }
    write_if_changed(&dir.join("Cargo.toml"), cargo)?;
    let mut text = report(name, disc, &r, &ram, &unchecked, cfg.locals, readable);
    if let Some(m) = modded {
        let _ = writeln!(
            text,
            "mod {}: {} functions replaced; {}",
            m.dir.display(),
            m.funcs.len(),
            if stale.is_empty() {
                "each prints today as cut".into()
            } else {
                format!(
                    "printed differently since the cut (re-cut): {}",
                    stale.join(", ")
                )
            }
        );
    }
    write_if_changed(&dir.join("report.txt"), text)?;
    Ok(r)
}

/// Every generated Cargo.toml's first line.
pub(crate) const GENERATED: &str =
    "# Generated by gbaa-recompile. Don't edit; rerun `just recompile`.\n";

/// Every generated package's fields after its name.
pub(crate) const PACKAGE: &str = "version = \"0.1.0\"\nedition = \"2024\"\npublish = false\n";

/// `name` as a Cargo package name: lowercase letters, digits and single
/// dashes (`Pokemon - Emerald Version` is `pokemon-emerald-version`), the
/// game's-name rule ([`runtime::name::GameName`]).
pub fn crate_name(name: &str) -> String {
    runtime::name::GameName(name).to_string()
}

fn crate_ident(name: &str) -> String {
    format!("gbaa_{name}").replace('-', "_")
}

/// `report.txt`: what was recompiled.
/// `unchecked`: the functions whose locals check failed, with why, so
/// their registers stay in `Machine`; listed if `locals` is on everywhere.
/// `readable`: the readable list, and how many of its blocks take the
/// form.
pub fn report(
    name: &str,
    disc: &Discovery,
    r: &Recompiled,
    ram: &[RamCode],
    unchecked: &[(u32, String)],
    locals: bool,
    readable: Option<&std::collections::BTreeSet<u32>>,
) -> String {
    let blocks: usize = r.funcs.iter().map(|f| f.blocks.len()).sum();
    let insns: usize = r
        .funcs
        .iter()
        .flat_map(|f| f.blocks.values())
        .map(|b| b.insns.len())
        .sum();
    let mut s = String::new();
    let _ = writeln!(s, "gbaa-recompile: {name}");
    let _ = writeln!(
        s,
        "functions {} (entries {}), blocks {blocks}, dispatch keys {}, instructions {insns}",
        r.funcs.len(),
        disc.entries.len(),
        r.table.len()
    );
    // Step 8 of docs/structured-codegen-design.md: the functions printed
    // structured, and why the rest stay v1.
    let mut why: BTreeMap<V1, usize> = BTreeMap::new();
    for w in r.v1.values() {
        *why.entry(*w).or_default() += 1;
    }
    let why: Vec<String> = why.iter().map(|(w, n)| format!("{w:?} {n}")).collect();
    let _ = writeln!(
        s,
        "structured functions {} of {}, inner block starts {}; v1: {}",
        r.funcs.len() - r.v1.len(),
        r.funcs.len(),
        r.inner.len(),
        if why.is_empty() {
            "none".into()
        } else {
            why.join(", ")
        }
    );
    for (e, w) in r.v1.iter().filter(|(_, w)| **w != V1::Off).take(50) {
        let _ = writeln!(s, "  v1 0x{e:08x}: {w:?}");
    }
    // Step 9: registers in locals, but where the write-back check failed.
    if locals {
        let _ = writeln!(
            s,
            "registers in locals: {} of {} functions",
            r.funcs.len() - unchecked.len(),
            r.funcs.len()
        );
        for (e, w) in unchecked.iter().take(50) {
            let _ = writeln!(s, "  in Machine 0x{e:08x}: {w}");
        }
    }
    // The readable form (docs/mod-view-design.md): the listed functions,
    // and those whose write-back check failed, printed with registers in
    // `Machine`.
    if let Some(list) = readable {
        // The ROM's functions and its IWRAM copies'.
        let funcs = || {
            r.funcs
                .iter()
                .chain(ram.iter().flat_map(|rc| &rc.code.funcs))
        };
        let have: std::collections::BTreeSet<u32> = funcs().map(|f| f.entry).collect();
        let missing: Vec<String> = list
            .iter()
            .filter(|k| !have.contains(k))
            .map(|k| format!("0x{k:08x}"))
            .collect();
        // Its blocks that keep their writes to the exits' sync blocks
        // (`lower::readable_blocks`); the rest print the play view's
        // statements.
        let (mut held, mut blocks) = (0, 0);
        let mut plain = Vec::new();
        for f in funcs().filter(|f| list.contains(&f.entry)) {
            let (h, n) = crate::lower::readable_blocks(f);
            (held, blocks) = (held + h, blocks + n);
            if h < n {
                plain.push((f.entry, n - h, n));
            }
        }
        let _ = writeln!(
            s,
            "readable functions {} of {} listed, blocks in the readable form {held} of {blocks}; not functions: {}",
            list.len() - missing.len(),
            list.len(),
            if missing.is_empty() {
                "none".into()
            } else {
                missing.join(", ")
            }
        );
        for (e, k, n) in plain.iter().take(50) {
            let _ = writeln!(
                s,
                "  readable 0x{e:08x}: {k} of {n} blocks as the play view"
            );
        }
    }
    let _ = writeln!(
        s,
        "discovered instructions {}, not in any block {}",
        disc.code.len(),
        r.uncovered.len()
    );
    for (a, t) in r.uncovered.iter().take(50) {
        let _ = writeln!(
            s,
            "  uncovered {a:#010x} {}",
            if *t { "thumb" } else { "arm" }
        );
    }
    let unresolved = disc.rec.jumps.len() - disc.rec.resolved().count();
    let _ = writeln!(
        s,
        "indirect jumps {}, unresolved {unresolved}",
        disc.rec.jumps.len()
    );
    // Flag liveness's effect (decision 9): the flag writes kept, and the
    // exits whose yield path writes the ones it deleted (step 2 of
    // docs/structured-codegen-design.md).
    let (mut kept, mut yields, mut exits) = (0, 0, 0);
    for b in r.funcs.iter().flat_map(|f| f.blocks.values()) {
        let insts = &b.block.insts;
        for (i, inst) in insts.iter().enumerate() {
            match inst.op {
                Op::SetFlag(..) => kept += 1,
                Op::YieldFlag(..) => {
                    yields += 1;
                    exits += !matches!(insts[i + 1].op, Op::YieldFlag(..)) as usize;
                }
                _ => {}
            }
        }
    }
    let _ = writeln!(
        s,
        "flag writes {kept}; exits whose yield path writes flags {exits} ({yields} flags)"
    );
    let n: usize = ram
        .iter()
        .flat_map(|rc| &rc.code.funcs)
        .map(Func::overlays)
        .sum();
    let _ = writeln!(
        s,
        "overlays {n} (blocks of code copied into IWRAM, decision 7)"
    );
    for rc in ram {
        let c = &rc.copy;
        let _ = writeln!(
            s,
            "  IWRAM copy 0x{:08x}..0x{:08x} from ROM 0x{:08x}: functions {}, blocks {}, \
             discovered instructions {}, not in any block {}",
            c.dst,
            c.dst + c.len,
            c.src,
            rc.code.funcs.len(),
            rc.code.table.len(),
            c.disc.code.len(),
            rc.code.uncovered.len()
        );
        for f in &rc.code.funcs {
            let bytes: u32 = f
                .blocks
                .keys()
                .flat_map(|&k| guard_ranges(c, f, k))
                .map(|r| r.1)
                .sum();
            let first = f.overlay.unwrap();
            let _ = writeln!(
                s,
                "  overlays {first}..{}: 0x{:08x} from ROM 0x{:08x}, {bytes} bytes of code in {} blocks",
                first as usize + f.overlays(),
                f.entry,
                c.rom_addr(f.entry & !1),
                f.blocks.len()
            );
            for (k, vs) in &f.variants {
                let _ = writeln!(s, "    block 0x{k:08x}: {} patched versions", vs.len());
            }
        }
    }
    s
}

/// The generated `main`: the desktop front end with this ROM's code, and
/// the ROM itself, which it runs when given none, under the game's name
/// (`GAME`, the key its saves are filed under).
const MAIN: &str = r#"//! Generated by gbaa-recompile: `gbaa-desktop`'s command line, running
//! the recompiled code (or only the interpreter, with `--interp`) on the
//! ROM it was recompiled from, `rom.gba`, built in, named "GAME"; a ROM
//! given runs instead.

fn main() -> std::process::ExitCode {
    desktop::main(
        Some(&GEN::CODE),
        Some(desktop::Embedded {
            rom: include_bytes!("../rom.gba"),
            name: "GAME",
        }),
    )
}
"#;

/// `call-<name>`: run one guest function, recompiled or interpreted, and print the
/// CPU state it returns with. For checking code that a ROM never runs on
/// its own (the LLVM jump-table sample).
const CALL: &str = r#"//! Generated by gbaa-recompile.
//!
//! call-NAME ROM KEY R0 [--interp] [--frames N]: call the guest function at KEY
//! (bit 0 set for Thumb) with r0 = R0 and lr pointing at IWRAM, stop when
//! it returns there, and print the CPU state.

use runtime::dispatch::{self, Stats};
use runtime::Machine;

fn num(s: &str) -> u64 {
    let s = s.replace('_', "");
    match s.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16),
        None => s.parse(),
    }
    .unwrap_or_else(|_| panic!("not a number: {s}"))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [rom_path, key, r0, ..] = args.as_slice() else {
        panic!("usage: call ROM KEY R0 [--interp] [--frames N]");
    };
    let rom: &'static [u8] = std::fs::read(rom_path).expect("ROM").leak();
    let code = &GEN::CODE;
    assert_eq!(dispatch::rom_hash(rom), code.rom_hash, "not the ROM this was recompiled from");
    let interp = args.iter().any(|a| a == "--interp");
    let frames = args
        .iter()
        .position(|a| a == "--frames")
        .map_or(10, |i| num(&args[i + 1]));
    let key = num(key) as u32;
    let mut m = Box::new(Machine::new(rom, desktop::Desktop::headless()));
    m.set_timing(code.timing);
    m.spin_list = code.spins;
    m.cpu.r[0] = num(r0) as u32;
    m.cpu.r[14] = 0x0300_0000;
    let t = if key & 1 != 0 { armv4t::interp::T_BIT } else { 0 };
    m.cpu.cpsr = (m.cpu.cpsr & !armv4t::interp::T_BIT) | t;
    m.cpu.pc = key & !1;
    let mut stats = Stats::default();
    let code = (!interp).then_some(code);
    let mut why = dispatch::Stop::FrameEnd;
    for _ in 0..frames {
        why = dispatch::run(&mut m, code, Some(0x0300_0000), &mut stats);
        if why != dispatch::Stop::FrameEnd {
            break;
        }
    }
    let c = &m.cpu;
    println!("stop {why:x?} pc {:08x} cpsr {:08x}", c.pc, c.cpsr);
    for (i, r) in c.r.iter().enumerate() {
        print!("r{i}={r:08x}{}", if i % 5 == 4 { "\n" } else { " " });
    }
    println!("calls {} interpreted {}", stats.calls, stats.interpreted);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_keep_unchanged_mtimes_and_prune_the_rest() {
        let dir = std::env::temp_dir().join(format!("gbaa-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (a, b) = (dir.join("src/a.rs"), dir.join("src/old/b.rs"));
        let mut files = Files::default();
        files.write(&a, "a").unwrap();
        files.write(&b, "b").unwrap();
        let t = std::fs::metadata(&a).unwrap().modified().unwrap();
        // A second run writes `a` with the same bytes and no longer `b`.
        let mut files = Files::default();
        files.write(&a, "a").unwrap();
        files.prune(&dir.join("src")).unwrap();
        assert_eq!(std::fs::metadata(&a).unwrap().modified().unwrap(), t);
        assert!(!dir.join("src/old").exists());
        files.write(&a, "a2").unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a2");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
