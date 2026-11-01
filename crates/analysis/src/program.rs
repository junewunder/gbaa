//! A ROM image with its code map: which bytes are code, in which instruction
//! set, and which function each address belongs to. The analyses read
//! instructions and constant data through it.
//!
//! The code map is an input: an ELF's (the jump-table check), or none, as
//! in code discovery ([`crate::discovery`]), where every ROM address may be
//! code except the data found so far, and the functions are the starts
//! found so far.

use armv4t::decode::{Instr, decode_arm, decode_thumb, pair_thumb_bl};

/// Where the cartridge ROM is mapped.
pub const ROM_BASE: u32 = 0x0800_0000;

/// A run of code in one instruction set, `start..end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Region {
    pub start: u32,
    pub end: u32,
    pub thumb: bool,
}

/// A function: its entry and its extent `start..end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Func {
    pub start: u32,
    pub end: u32,
    pub thumb: bool,
}

/// One decoded instruction. A Thumb `bl` pair is one 4-byte instruction
/// with [`bl`](Insn::bl) set; so is ARM `bl`.
#[derive(Clone, Copy, Debug)]
pub struct Insn {
    pub addr: u32,
    pub thumb: bool,
    pub size: u32,
    pub instr: Instr,
    /// The call target of a `bl`.
    pub bl: Option<u32>,
}

impl Insn {
    pub fn next(&self) -> u32 {
        self.addr.wrapping_add(self.size)
    }
}

pub struct Program {
    rom: Vec<u8>,
    /// Where the image is mapped: [`ROM_BASE`], or a RAM address for code
    /// copied there ([`crate::discovery::discover_at`]).
    base: u32,
    /// Sorted by start, disjoint. Empty means every ROM address is code in
    /// either instruction set.
    regions: Vec<Region>,
    /// Sorted by start.
    funcs: Vec<Func>,
    /// Known data, `start..end`, sorted and disjoint: never code. Only used
    /// without regions.
    data: Vec<(u32, u32)>,
}

impl Program {
    /// `regions` and `funcs` may come in any order; regions must not overlap.
    pub fn new(rom: Vec<u8>, mut regions: Vec<Region>, mut funcs: Vec<Func>) -> Program {
        regions.sort();
        funcs.sort();
        funcs.dedup_by_key(|f| f.start);
        Program {
            rom,
            base: ROM_BASE,
            regions,
            funcs,
            data: Vec::new(),
        }
    }

    /// The image mapped at `base` instead of [`ROM_BASE`], with no code map.
    pub fn at(base: u32, image: Vec<u8>) -> Program {
        Program {
            base,
            ..Program::new(image, Vec::new(), Vec::new())
        }
    }

    pub fn base(&self) -> u32 {
        self.base
    }

    /// Replace the functions, as [`Program::new`] takes them.
    pub fn set_funcs(&mut self, mut funcs: Vec<Func>) {
        funcs.sort();
        funcs.dedup_by_key(|f| f.start);
        self.funcs = funcs;
    }

    /// Replace the known data ranges; they may overlap and come in any
    /// order.
    pub fn set_data(&mut self, mut data: Vec<(u32, u32)>) {
        data.sort();
        let mut out: Vec<(u32, u32)> = Vec::with_capacity(data.len());
        for (a, b) in data {
            match out.last_mut() {
                Some(l) if a <= l.1 => l.1 = l.1.max(b),
                _ if a < b => out.push((a, b)),
                _ => {}
            }
        }
        self.data = out;
    }

    /// Whether `a` is in a known data range.
    pub fn is_data(&self, a: u32) -> bool {
        let i = self.data.partition_point(|d| d.0 <= a);
        i > 0 && a < self.data[i - 1].1
    }

    pub fn rom(&self) -> &[u8] {
        &self.rom
    }

    pub fn funcs(&self) -> &[Func] {
        &self.funcs
    }

    pub fn regions(&self) -> &[Region] {
        &self.regions
    }

    /// Whether `a` is in the image (the ROM, unless [`Program::at`] put it
    /// elsewhere).
    pub fn in_rom(&self, a: u32) -> bool {
        a >= self.base && ((a - self.base) as usize) < self.rom.len()
    }

    /// Read `bytes` (1, 2 or 4) little-endian bytes at `a`, which must be
    /// aligned and in the image.
    pub fn read(&self, a: u32, bytes: u32) -> Option<u32> {
        if !a.is_multiple_of(bytes) || !self.in_rom(a) || !self.in_rom(a + (bytes - 1)) {
            return None;
        }
        let o = (a - self.base) as usize;
        let b = &self.rom[o..o + bytes as usize];
        Some(b.iter().rev().fold(0, |v, &x| v << 8 | x as u32))
    }

    /// Whether `a` is code in the given instruction set.
    pub fn is_code(&self, a: u32, thumb: bool) -> bool {
        if self.regions.is_empty() {
            return self.in_rom(a) && !self.is_data(a);
        }
        let i = self.regions.partition_point(|r| r.start <= a);
        i > 0 && {
            let r = self.regions[i - 1];
            a < r.end && r.thumb == thumb
        }
    }

    /// The function whose extent contains `a`: the last one starting at or
    /// before it, if `a` is inside its extent.
    pub fn func_at(&self, a: u32) -> Option<&Func> {
        let i = self.funcs.partition_point(|f| f.start <= a);
        let f = self.funcs.get(i.checked_sub(1)?)?;
        (a < f.end).then_some(f)
    }

    /// Decode the instruction at `a`, if it is code in that instruction set.
    pub fn decode(&self, a: u32, thumb: bool) -> Option<Insn> {
        if !self.is_code(a, thumb) {
            return None;
        }
        if !thumb {
            let instr = decode_arm(self.read(a, 4)?, a);
            let bl = match instr {
                Instr::Branch {
                    link: true, target, ..
                } => Some(target),
                _ => None,
            };
            return Some(Insn {
                addr: a,
                thumb,
                size: 4,
                instr,
                bl,
            });
        }
        let h = self.read(a, 2)? as u16;
        let instr = decode_thumb(h, a);
        if let Instr::ThumbBlHi { .. } = instr
            && self.is_code(a + 2, true)
            && let Some(lo) = self.read(a + 2, 2)
            && let Some(t) = pair_thumb_bl(h, lo as u16, a)
        {
            return Some(Insn {
                addr: a,
                thumb,
                size: 4,
                instr,
                bl: Some(t),
            });
        }
        Some(Insn {
            addr: a,
            thumb,
            size: 2,
            instr,
            bl: None,
        })
    }

    /// The instruction ending right before `a`, read linearly (no CFG).
    pub fn prev(&self, a: u32, thumb: bool) -> Option<Insn> {
        if thumb
            && let Some(i) = self.decode(a.wrapping_sub(4), true)
            && i.size == 4
        {
            return Some(i);
        }
        let size = if thumb { 2 } else { 4 };
        self.decode(a.wrapping_sub(size), thumb)
    }
}
