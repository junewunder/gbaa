//! Reads an ELF's sections and symbols for [`crate::Elf`].

use object::elf::{SHF_ALLOC, SHF_EXECINSTR, SHT_NOBITS, STB_GLOBAL, STT_FUNC, STT_NOTYPE};
use object::read::elf::{ElfFile32, FileHeader, SectionHeader, Sym as _};
use object::{Endianness, Object, ObjectSymbol};

use crate::{Elf, Section, Sym};

pub fn read(data: &[u8]) -> Result<Elf, object::Error> {
    let file = ElfFile32::<Endianness>::parse(data)?;
    let endian = file.endian();
    let table = file.elf_header().sections(endian, data)?;
    let mut sections = Vec::new();
    for (i, s) in table.iter().enumerate() {
        let flags = s.sh_flags(endian);
        sections.push(Section {
            index: i,
            addr: s.sh_addr(endian),
            size: s.sh_size(endian),
            exec: flags & SHF_EXECINSTR != 0,
            data: flags & SHF_ALLOC != 0 && s.sh_type(endian) != SHT_NOBITS,
        });
    }
    let mut syms = Vec::new();
    for sym in file.symbols() {
        let raw = sym.elf_symbol();
        let Some(section) = sym.section_index() else {
            continue;
        };
        syms.push(Sym {
            name: sym.name()?.to_string(),
            value: raw.st_value(endian),
            size: raw.st_size(endian),
            func: raw.st_type() == STT_FUNC,
            global_notype: raw.st_type() == STT_NOTYPE && raw.st_bind() == STB_GLOBAL,
            section: section.0,
        });
    }
    Ok(Elf { sections, syms })
}
