//! Reads an ELF's symbol table into [`Sym`]s for [`crate::build_truth`],
//! and its ROM image.

use object::elf::{PT_LOAD, SHF_EXECINSTR, STB_GLOBAL, STT_FILE, STT_FUNC, STT_NOTYPE, STT_OBJECT};
use object::read::elf::{ElfFile32, FileHeader, ProgramHeader, SectionHeader, Sym as _};
use object::{Endianness, Object, ObjectSymbol, SectionIndex};

use crate::{Sym, SymKind};

/// The symbols of a 32-bit little-endian ELF, in symbol-table order (the
/// order `readelf -sW` prints, which rule 3 depends on). The raw `st_value`
/// is kept, so Thumb functions keep bit 0.
pub fn read_syms(data: &[u8]) -> Result<Vec<Sym>, object::Error> {
    let file = ElfFile32::<Endianness>::parse(data)?;
    let endian = file.endian();
    let sections = file.elf_header().sections(endian, data)?;
    let exec = |idx: SectionIndex| {
        sections
            .section(idx)
            .is_ok_and(|s| s.sh_flags(endian) & SHF_EXECINSTR != 0)
    };
    let mut out = Vec::new();
    for sym in file.symbols() {
        let raw = sym.elf_symbol();
        let section = sym.section_index();
        out.push(Sym {
            name: sym.name()?.to_string(),
            value: raw.st_value(endian),
            size: raw.st_size(endian).into(),
            kind: match raw.st_type() {
                STT_FUNC => SymKind::Func,
                STT_NOTYPE => SymKind::NoType,
                STT_OBJECT => SymKind::Object,
                STT_FILE => SymKind::File,
                _ => SymKind::Other,
            },
            global: raw.st_bind() == STB_GLOBAL,
            section: section.map(|s| s.0),
            exec: section.is_some_and(exec),
        });
    }
    Ok(out)
}

/// The segments an ELF loads into ROM: each `PT_LOAD` segment whose load
/// address is in ROM, as its offset from [`crate::ROM_LO`] and its bytes.
/// Sections that run in IWRAM or EWRAM are in them at their load addresses.
pub fn rom_segments(data: &[u8]) -> Result<Vec<(usize, &[u8])>, String> {
    let file = ElfFile32::<Endianness>::parse(data).map_err(|e| e.to_string())?;
    let endian = file.endian();
    let phs = file
        .elf_header()
        .program_headers(endian, data)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for ph in phs {
        let paddr = ph.p_paddr(endian);
        if ph.p_type(endian) != PT_LOAD || ph.p_filesz(endian) == 0 || !crate::in_rom(paddr) {
            continue;
        }
        let bytes = ph
            .data(endian, data)
            .map_err(|()| format!("segment at 0x{paddr:08x} runs past the file"))?;
        out.push(((paddr - crate::ROM_LO) as usize, bytes));
    }
    Ok(out)
}

/// The cartridge ROM an ELF loads: its [`rom_segments`] at their offsets
/// (what `objcopy -O binary` writes for a GBA ELF). Gaps between segments
/// are 0xFF, as pret's `objcopy --gap-fill 0xFF` writes FireRed's (its
/// gfx_data sits apart at 0x08d00000); Emerald's are 0, so a ROM is
/// checked against an ELF with [`builds`], not this.
pub fn rom_image(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut rom: Vec<u8> = Vec::new();
    for (at, bytes) in rom_segments(data)? {
        if rom.len() < at + bytes.len() {
            rom.resize(at + bytes.len(), 0xff);
        }
        rom[at..at + bytes.len()].copy_from_slice(bytes);
    }
    Ok(rom)
}

/// The cartridge header's end: the Nintendo logo, title, codes and
/// checksum before it are written by `gbafix` after linking (metroidret's
/// mzm leaves them 0 in its ELF), so [`builds`] doesn't compare them.
pub const HEADER_END: usize = 0xc0;

/// Whether `rom` holds every byte the ELF loads into ROM, whatever fills
/// the gaps between its segments and whatever header `gbafix` wrote.
pub fn builds(data: &[u8], rom: &[u8]) -> Result<bool, String> {
    let segs = rom_segments(data)?;
    Ok(!segs.is_empty()
        && segs.iter().all(|&(at, bytes)| {
            let skip = HEADER_END.saturating_sub(at).min(bytes.len());
            rom.get(at + skip..at + bytes.len()) == Some(&bytes[skip..])
        }))
}
