//! Which BIOS calls a ROM makes: every `swi` among the instructions
//! discovery decoded (the ROM's code and the IWRAM copies it runs), not a
//! byte scan, so a `0xDFxx` halfword in data isn't counted.

use std::collections::BTreeMap;

use analysis::discovery::{self, Discovery, Evidence};
use analysis::program::ROM_BASE;
use armv4t::decode::{Instr, decode_arm, decode_thumb};
use runtime::bios;

/// Where one call is made: the `swi`'s address (where it runs, for an
/// IWRAM copy), its instruction set, and whether descent from the entry
/// reached it rather than the pointer scan alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Site {
    pub addr: u32,
    pub thumb: bool,
    pub confirmed: bool,
}

/// The `swi`s of `rom`, by call number.
pub fn survey(rom: &[u8]) -> BTreeMap<u8, Vec<Site>> {
    let d = discovery::discover(rom.to_vec());
    let mut out = BTreeMap::new();
    sites(&d, ROM_BASE, rom, &mut out);
    for o in discovery::overlays(rom, &d) {
        let at = (o.src - ROM_BASE) as usize;
        sites(&o.disc, o.dst, &rom[at..at + o.len as usize], &mut out);
    }
    out
}

/// The `swi`s among `d`'s instructions, `image` being the bytes at `base`.
fn sites(d: &Discovery, base: u32, image: &[u8], out: &mut BTreeMap<u8, Vec<Site>>) {
    for (&addr, c) in &d.code {
        let o = (addr - base) as usize;
        let size = if c.thumb { 2 } else { 4 };
        let Some(b) = image.get(o..o + size) else {
            continue;
        };
        let instr = if c.thumb {
            decode_thumb(u16::from_le_bytes([b[0], b[1]]), addr)
        } else {
            decode_arm(u32::from_le_bytes([b[0], b[1], b[2], b[3]]), addr)
        };
        if let Instr::Swi { comment, .. } = instr {
            out.entry(bios::number(comment, c.thumb))
                .or_default()
                .push(Site {
                    addr,
                    thumb: c.thumb,
                    confirmed: c.evidence == Evidence::Confirmed,
                });
        }
    }
}

/// GBATEK's name for BIOS call `n`, if it has one.
pub fn name(n: u8) -> Option<&'static str> {
    const NAMES: [&str; 0x2B] = [
        "SoftReset",
        "RegisterRamReset",
        "Halt",
        "Stop",
        "IntrWait",
        "VBlankIntrWait",
        "Div",
        "DivArm",
        "Sqrt",
        "ArcTan",
        "ArcTan2",
        "CpuSet",
        "CpuFastSet",
        "GetBiosChecksum",
        "BgAffineSet",
        "ObjAffineSet",
        "BitUnPack",
        "LZ77UnCompWram",
        "LZ77UnCompVram",
        "HuffUnComp",
        "RLUnCompWram",
        "RLUnCompVram",
        "Diff8bitUnFilterWram",
        "Diff8bitUnFilterVram",
        "Diff16bitUnFilter",
        "SoundBias",
        "SoundDriverInit",
        "SoundDriverMode",
        "SoundDriverMain",
        "SoundDriverVSync",
        "SoundChannelClear",
        "MidiKey2Freq",
        "SoundWhatever0",
        "SoundWhatever1",
        "SoundWhatever2",
        "SoundWhatever3",
        "SoundWhatever4",
        "MultiBoot",
        "HardReset",
        "CustomHalt",
        "SoundDriverVSyncOff",
        "SoundDriverVSyncOn",
        "SoundGetJumpList",
    ];
    NAMES.get(n as usize).copied()
}

/// Whether the runtime's BIOS runs call `n`.
pub fn implemented(n: u8) -> bool {
    bios::call::IMPLEMENTED.contains(&n)
}

/// One ROM's survey as text: a line per call number with its sites.
pub fn report(calls: &BTreeMap<u8, Vec<Site>>) -> String {
    let mut out = String::new();
    for (&n, sites) in calls {
        let confirmed = sites.iter().filter(|s| s.confirmed).count();
        let first: Vec<String> = sites
            .iter()
            .take(4)
            .map(|s| format!("{:#010x}{}", s.addr, if s.thumb { "t" } else { "a" }))
            .collect();
        out += &format!(
            "  swi {n:#04x} {:<20} {:>4} sites ({confirmed} confirmed)  {}  {}\n",
            name(n).unwrap_or("?"),
            sites.len(),
            if implemented(n) { "ok" } else { "MISSING" },
            first.join(" "),
        );
    }
    out
}
