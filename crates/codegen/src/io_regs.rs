//! The GBA's I/O registers by name, for the readable form
//! (`docs/mod-view-design.md`, "The readable form"): an access at a
//! constant address in I/O space prints as pret's `REG_*` name of its
//! width (`REG_TM1CNT_L`, a `Ptr<u16>`), not `Ptr::<u16>::at(0x400_0104)`.
//!
//! Names and widths are pret's `include/gba/io_reg.h` (its `REG_*`, each
//! `vu8`, `vu16` or `vu32`); where pret gives two names to one address
//! and width, the one its code uses (`SIOMLT_SEND`, not `SIODATA8`). The
//! JOY bus registers, `POSTFLG` and `HALTCNT`, which pret leaves out, are
//! GBATEK's. An access no register of its width starts at prints from the
//! narrowest register holding it: `REG_VCOUNT.field::<u8>(0)`.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

/// Each register: its name without `REG_`, address and width in bytes,
/// by address.
pub const REGS: &[(&str, u32, u32)] = &[
    ("DISPCNT", 0x0400_0000, 2),
    ("DISPSTAT", 0x0400_0004, 2),
    ("VCOUNT", 0x0400_0006, 2),
    ("BG0CNT", 0x0400_0008, 2),
    ("BG1CNT", 0x0400_000a, 2),
    ("BG2CNT", 0x0400_000c, 2),
    ("BG3CNT", 0x0400_000e, 2),
    ("BG0HOFS", 0x0400_0010, 2),
    ("BG0VOFS", 0x0400_0012, 2),
    ("BG1HOFS", 0x0400_0014, 2),
    ("BG1VOFS", 0x0400_0016, 2),
    ("BG2HOFS", 0x0400_0018, 2),
    ("BG2VOFS", 0x0400_001a, 2),
    ("BG3HOFS", 0x0400_001c, 2),
    ("BG3VOFS", 0x0400_001e, 2),
    ("BG2PA", 0x0400_0020, 2),
    ("BG2PB", 0x0400_0022, 2),
    ("BG2PC", 0x0400_0024, 2),
    ("BG2PD", 0x0400_0026, 2),
    ("BG2X", 0x0400_0028, 4),
    ("BG2X_L", 0x0400_0028, 2),
    ("BG2X_H", 0x0400_002a, 2),
    ("BG2Y", 0x0400_002c, 4),
    ("BG2Y_L", 0x0400_002c, 2),
    ("BG2Y_H", 0x0400_002e, 2),
    ("BG3PA", 0x0400_0030, 2),
    ("BG3PB", 0x0400_0032, 2),
    ("BG3PC", 0x0400_0034, 2),
    ("BG3PD", 0x0400_0036, 2),
    ("BG3X", 0x0400_0038, 4),
    ("BG3X_L", 0x0400_0038, 2),
    ("BG3X_H", 0x0400_003a, 2),
    ("BG3Y", 0x0400_003c, 4),
    ("BG3Y_L", 0x0400_003c, 2),
    ("BG3Y_H", 0x0400_003e, 2),
    ("WIN0H", 0x0400_0040, 2),
    ("WIN1H", 0x0400_0042, 2),
    ("WIN0V", 0x0400_0044, 2),
    ("WIN1V", 0x0400_0046, 2),
    ("WININ", 0x0400_0048, 2),
    ("WINOUT", 0x0400_004a, 2),
    ("MOSAIC", 0x0400_004c, 2),
    ("BLDCNT", 0x0400_0050, 2),
    ("BLDALPHA", 0x0400_0052, 2),
    ("BLDY", 0x0400_0054, 2),
    ("SOUND1CNT_L", 0x0400_0060, 2),
    ("NR10", 0x0400_0060, 1),
    ("SOUND1CNT_H", 0x0400_0062, 2),
    ("NR11", 0x0400_0062, 1),
    ("NR12", 0x0400_0063, 1),
    ("SOUND1CNT_X", 0x0400_0064, 2),
    ("NR13", 0x0400_0064, 1),
    ("NR14", 0x0400_0065, 1),
    ("SOUND2CNT_L", 0x0400_0068, 2),
    ("NR21", 0x0400_0068, 1),
    ("NR22", 0x0400_0069, 1),
    ("SOUND2CNT_H", 0x0400_006c, 2),
    ("NR23", 0x0400_006c, 1),
    ("NR24", 0x0400_006d, 1),
    ("SOUND3CNT_L", 0x0400_0070, 2),
    ("NR30", 0x0400_0070, 1),
    ("SOUND3CNT_H", 0x0400_0072, 2),
    ("NR31", 0x0400_0072, 1),
    ("NR32", 0x0400_0073, 1),
    ("SOUND3CNT_X", 0x0400_0074, 2),
    ("NR33", 0x0400_0074, 1),
    ("NR34", 0x0400_0075, 1),
    ("SOUND4CNT_L", 0x0400_0078, 2),
    ("NR41", 0x0400_0078, 1),
    ("NR42", 0x0400_0079, 1),
    ("SOUND4CNT_H", 0x0400_007c, 2),
    ("NR43", 0x0400_007c, 1),
    ("NR44", 0x0400_007d, 1),
    ("SOUNDCNT_L", 0x0400_0080, 2),
    ("NR50", 0x0400_0080, 1),
    ("NR51", 0x0400_0081, 1),
    ("SOUNDCNT_H", 0x0400_0082, 2),
    ("SOUNDCNT_X", 0x0400_0084, 2),
    ("NR52", 0x0400_0084, 1),
    ("SOUNDBIAS", 0x0400_0088, 2),
    ("SOUNDBIAS_L", 0x0400_0088, 1),
    ("SOUNDBIAS_H", 0x0400_0089, 1),
    ("WAVE_RAM0", 0x0400_0090, 4),
    ("WAVE_RAM1", 0x0400_0094, 4),
    ("WAVE_RAM2", 0x0400_0098, 4),
    ("WAVE_RAM3", 0x0400_009c, 4),
    ("FIFO_A", 0x0400_00a0, 4),
    ("FIFO_B", 0x0400_00a4, 4),
    ("DMA0SAD", 0x0400_00b0, 4),
    ("DMA0DAD", 0x0400_00b4, 4),
    ("DMA0CNT", 0x0400_00b8, 4),
    ("DMA0CNT_L", 0x0400_00b8, 2),
    ("DMA0CNT_H", 0x0400_00ba, 2),
    ("DMA1SAD", 0x0400_00bc, 4),
    ("DMA1DAD", 0x0400_00c0, 4),
    ("DMA1CNT", 0x0400_00c4, 4),
    ("DMA1CNT_L", 0x0400_00c4, 2),
    ("DMA1CNT_H", 0x0400_00c6, 2),
    ("DMA2SAD", 0x0400_00c8, 4),
    ("DMA2DAD", 0x0400_00cc, 4),
    ("DMA2CNT", 0x0400_00d0, 4),
    ("DMA2CNT_L", 0x0400_00d0, 2),
    ("DMA2CNT_H", 0x0400_00d2, 2),
    ("DMA3SAD", 0x0400_00d4, 4),
    ("DMA3DAD", 0x0400_00d8, 4),
    ("DMA3CNT", 0x0400_00dc, 4),
    ("DMA3CNT_L", 0x0400_00dc, 2),
    ("DMA3CNT_H", 0x0400_00de, 2),
    ("TM0CNT", 0x0400_0100, 4),
    ("TM0CNT_L", 0x0400_0100, 2),
    ("TM0CNT_H", 0x0400_0102, 2),
    ("TM1CNT", 0x0400_0104, 4),
    ("TM1CNT_L", 0x0400_0104, 2),
    ("TM1CNT_H", 0x0400_0106, 2),
    ("TM2CNT", 0x0400_0108, 4),
    ("TM2CNT_L", 0x0400_0108, 2),
    ("TM2CNT_H", 0x0400_010a, 2),
    ("TM3CNT", 0x0400_010c, 4),
    ("TM3CNT_L", 0x0400_010c, 2),
    ("TM3CNT_H", 0x0400_010e, 2),
    ("SIODATA32", 0x0400_0120, 4),
    ("SIOMULTI0", 0x0400_0120, 2),
    ("SIOMULTI1", 0x0400_0122, 2),
    ("SIOMULTI2", 0x0400_0124, 2),
    ("SIOMULTI3", 0x0400_0126, 2),
    ("SIOCNT", 0x0400_0128, 2),
    ("SIOMLT_SEND", 0x0400_012a, 2),
    ("KEYINPUT", 0x0400_0130, 2),
    ("KEYCNT", 0x0400_0132, 2),
    ("RCNT", 0x0400_0134, 2),
    ("JOYCNT", 0x0400_0140, 2),
    ("JOY_RECV", 0x0400_0150, 4),
    ("JOY_TRANS", 0x0400_0154, 4),
    ("JOYSTAT", 0x0400_0158, 2),
    ("IE", 0x0400_0200, 2),
    ("IF", 0x0400_0202, 2),
    ("WAITCNT", 0x0400_0204, 2),
    ("IME", 0x0400_0208, 2),
    ("POSTFLG", 0x0400_0300, 1),
    ("HALTCNT", 0x0400_0301, 1),
];

/// The register an access of `w` bytes at `x` prints through, and the
/// offset into it, `None` if it is the register itself: the one of width
/// `w` at `x`, else the narrowest holding all of it.
pub fn at(x: u32, w: u32) -> Option<(&'static str, Option<u32>)> {
    let holds = |&&(_, a, s): &&(&str, u32, u32)| a <= x && x - a + w <= s;
    REGS.iter()
        .find(|&&(_, a, s)| a == x && s == w)
        .or_else(|| REGS.iter().filter(holds).min_by_key(|r| r.2))
        .map(|&(n, a, s)| (n, (a != x || s != w).then_some(x - a)))
}

/// A register's identifier: `REG_` and its name.
pub fn ident(name: &str) -> String {
    format!("REG_{name}")
}

/// Each register as a `Ptr` constant, for the `globals` module.
pub fn consts() -> TokenStream {
    let cs = REGS.iter().map(|&(n, a, w)| {
        let id = format_ident!("{}", ident(n));
        let t = match w {
            1 => quote!(u8),
            2 => quote!(u16),
            _ => quote!(u32),
        };
        let a = crate::lower::hex(a);
        let doc = format!(" The GBA's I/O register `REG_{n}`.");
        quote!(#[doc = #doc] pub const #id: Ptr<#t> = Ptr::at(#a);)
    });
    quote!(#(#cs)*)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_name_per_address_and_width() {
        let mut seen = std::collections::BTreeSet::new();
        for &(n, a, w) in REGS {
            assert!(
                seen.insert((a, w)),
                "{n}: a second name at 0x{a:x}, {w} bytes"
            );
            assert_eq!(a % w, 0, "{n} is unaligned");
        }
    }

    #[test]
    fn an_access_finds_its_register() {
        assert_eq!(at(0x0400_0104, 2), Some(("TM1CNT_L", None)));
        assert_eq!(at(0x0400_0104, 4), Some(("TM1CNT", None)));
        assert_eq!(at(0x0400_0106, 2), Some(("TM1CNT_H", None)));
        assert_eq!(at(0x0400_0007, 1), Some(("VCOUNT", Some(1))));
        assert_eq!(at(0x0400_00d6, 2), Some(("DMA3SAD", Some(2))));
        assert_eq!(at(0x0400_0200, 4), None);
        assert_eq!(at(0x0400_0000, 1), Some(("DISPCNT", Some(0))));
        assert_eq!(at(0x0400_0060, 1), Some(("NR10", None)));
    }
}
