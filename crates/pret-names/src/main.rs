//! `pret-names ELF ROM OUT [SHOW]`: names the functions codegen compiles
//! from ROM (and the data its pools point at) with the symbols of ELF, a
//! pret build of the same bytes; writes the names file OUT and prints the
//! report, with up to SHOW (default 10) examples of each kind.

use analysis::discovery::{discover, overlays};
use codegen::func::Ctx;
use pret_names::{Copy, Ours, match_names};
use std::process::ExitCode;

fn ours(rom: &[u8]) -> Ours {
    let disc = discover(rom.to_vec());
    let ctx = Ctx::new(rom, &disc);
    // Every BIOS copy out of ROM, with the entries of those discovery
    // follows into IWRAM ([`overlays`]).
    let entered: Vec<(u32, u32, Copy)> = overlays(rom, &disc)
        .iter()
        .map(|o| {
            let at = (o.src - analysis::program::ROM_BASE) as usize;
            let c = Ctx::at(o.dst, &rom[at..at + o.len as usize], &o.disc);
            let copy = Copy {
                src: o.src,
                dst: o.dst,
                len: o.len,
                entries: c.entries.clone(),
            };
            (o.src, o.dst, copy)
        })
        .collect();
    let copies = disc
        .copies
        .iter()
        .map(|&(src, dst, len)| {
            entered
                .iter()
                .find(|(s, d, _)| (*s, *d) == (src, dst))
                .map_or_else(
                    || Copy {
                        src,
                        dst,
                        len,
                        entries: Default::default(),
                    },
                    |(_, _, c)| Copy {
                        entries: c.entries.clone(),
                        ..*c
                    },
                )
        })
        .collect();
    Ours {
        entries: ctx.entries.clone(),
        discovered: disc.funcs.keys().copied().collect(),
        code: disc.code.iter().map(|(&a, c)| (a, c.thumb)).collect(),
        data: disc.data.clone(),
        copies,
    }
}

/// `pret-names --dwarf ELF [NAME…]`: what the ELF's DWARF gives, counted,
/// and each named function's prototype, locals and lines.
fn dump(elf: &str, names: &[String]) -> Result<(), String> {
    let bytes = std::fs::read(elf).map_err(|e| format!("{elf}: {e}"))?;
    let d = pret_names::dwarf::load(&bytes).map_err(|e| format!("{elf}: {e}"))?;
    println!(
        "functions {}, lines {}, globals {}, types {}, type clashes {}, bad units {}, bad line programs {}",
        d.funcs.len(),
        d.lines.len(),
        d.globals.len(),
        d.types.len(),
        d.clashes.len(),
        d.bad_units.len(),
        d.bad_lines.len()
    );
    for b in d.bad_units.iter().chain(&d.bad_lines).take(8) {
        println!("  {b}");
    }
    for (a, f) in d.funcs.iter().filter(|(_, f)| names.contains(&f.name)) {
        println!("0x{a:08x} {} ({})", f.proto, f.src);
        for l in &f.locals {
            println!(
                "  {:?} {} {:?} 0x{:08x}..0x{:08x}{}",
                l.ty,
                l.name,
                l.home,
                l.from,
                l.to,
                if l.param { " param" } else { "" }
            );
        }
        println!("  homeless: {:?}", f.homeless);
        for (la, (file, line)) in d.lines.range(*a..*a + 0x40) {
            println!("  line 0x{la:08x} {file}:{line}");
        }
    }
    for n in names {
        if let Some(t) = d.types.get(n) {
            println!("{n}: {t:?}");
        }
    }
    Ok(())
}

fn run(args: &[String]) -> Result<(), String> {
    if let [flag, elf, names @ ..] = args
        && flag == "--dwarf"
    {
        return dump(elf, names);
    }
    let [elf, rom, out, rest @ ..] = args else {
        return Err("usage: pret-names ELF ROM OUT [SHOW]".into());
    };
    let show = match rest {
        [] => 10,
        [n] => n.parse().map_err(|e| format!("SHOW {n}: {e}"))?,
        _ => return Err("usage: pret-names ELF ROM OUT [SHOW]".into()),
    };
    let elf_bytes = std::fs::read(elf).map_err(|e| format!("{elf}: {e}"))?;
    let rom_bytes = std::fs::read(rom).map_err(|e| format!("{rom}: {e}"))?;
    if !disc_check::elf::builds(&elf_bytes, &rom_bytes).map_err(|e| format!("{elf}: {e}"))? {
        return Err(format!("{rom} isn't the ROM {elf} builds"));
    }
    let mut syms = disc_check::elf::read_syms(&elf_bytes).map_err(|e| format!("{elf}: {e}"))?;
    // A modern-GCC build names no object (`<stdin>`); its map beside it does.
    let map = std::path::Path::new(elf).with_extension("map");
    if syms
        .iter()
        .any(|s| s.kind == disc_check::SymKind::File && s.name == "<stdin>")
        && let Ok(text) = std::fs::read_to_string(&map)
    {
        let n = pret_names::name_stdin_files(&mut syms, &pret_names::map_sections(&text));
        println!("{n} <stdin> objects named from {}", map.display());
    }
    let names = match_names(&syms, &rom_bytes, &ours(&rom_bytes));
    let dbg = pret_names::dwarf::load(&elf_bytes).map_err(|e| format!("{elf}: {e}"))?;
    let header = [
        format!("Names for {rom}, from pret's symbols in {elf}."),
        "Written by `just pret-names` (crates/pret-names). Don't edit.".to_string(),
    ];
    if let Some(dir) = std::path::Path::new(out).parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let sha = codegen::names::sha1_hex(&rom_bytes);
    let mut text = names.toml(&header, &sha, &source(elf), Some(&dbg));
    // pret's constants headers and source beside the ELF, for the fields'
    // value names.
    let src = std::path::Path::new(elf)
        .parent()
        .unwrap_or(std::path::Path::new("."));
    let fams = {
        use pret_names::values::{families, read_defines, read_uses};
        families(&read_defines(src), &read_uses(src), &dbg.types)
    };
    text.push_str(&pret_names::values::toml(&fams));
    // The character table at the tree's root, for the readable form's
    // text comments. A tree without one (a non-pret decomp) gives none,
    // nor does a game crates/codegen/text-tables.toml gives a table of its
    // own layout (tmc, mzm: codegen::text::Table, loaded at print time).
    let charmap = src.join("charmap.txt");
    let own = codegen::text::Table::all()
        .iter()
        .any(|t| t.rom_sha1 == sha);
    let chars = match std::fs::read_to_string(&charmap) {
        Ok(_) if own => Vec::new(),
        Ok(t) => pret_names::text::parse(&t).map_err(|e| format!("{}: {e}", charmap.display()))?,
        Err(_) => Vec::new(),
    };
    if !chars.is_empty() {
        text.push_str(&pret_names::text::toml(&chars));
    }
    // Written beside and renamed, so a build reading the map (another lane's)
    // never sees half of one.
    let tmp = format!("{out}.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{tmp}: {e}"))?;
    std::fs::rename(&tmp, out).map_err(|e| format!("{out}: {e}"))?;
    println!("wrote {out}");
    print!("{}", names.report(show));
    print!("{}", dwarf_report(&names, &dbg));
    println!("text: {} charmap.txt entries", chars.len());
    println!(
        "values: {} families of #define names, taken by {} fields",
        fams.len(),
        fams.values().map(|f| f.fields.len()).sum::<usize>()
    );
    loads(out, &sha)
}

/// What the DWARF added to the map, counted.
fn dwarf_report(names: &pret_names::Names, d: &pret_names::dwarf::Debug) -> String {
    use pret_names::dwarf::Home;
    let roms: Vec<u32> = names
        .entries
        .iter()
        .filter(|e| matches!(e.name, pret_names::Name::Func { .. }))
        .map(|e| e.rom.unwrap_or(e.key) & !1)
        .collect();
    let fs: Vec<_> = roms.iter().filter_map(|a| d.funcs.get(a)).collect();
    let (mut reg, mut slot, mut homeless) = (0, 0, 0);
    for f in &fs {
        reg += f
            .locals
            .iter()
            .filter(|l| matches!(l.home, Home::Reg(_)))
            .count();
        slot += f
            .locals
            .iter()
            .filter(|l| matches!(l.home, Home::Slot(_)))
            .count();
        homeless += f.homeless.len();
    }
    let typed = names
        .data
        .keys()
        .filter(|a| d.globals.contains_key(a))
        .count();
    format!(
        "dwarf: {} of {} named functions have a prototype; locals: {reg} in a register, \
         {slot} in a stack slot, {homeless} with no location; lines {}; data typed {typed} of {}; \
         types {} ({} names with two layouts, the first kept); units unread {}, line programs cut short {}\n",
        fs.len(),
        roms.len(),
        d.lines.len(),
        names.data.len(),
        d.types.len(),
        d.clashes.len(),
        d.bad_units.len(),
        d.bad_lines.len()
    )
}

/// Where the names come from: `pret/<checkout>@<commit>` when the ELF is
/// in a git checkout, else its path.
fn source(elf: &str) -> String {
    let dir = std::path::Path::new(elf)
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let rev = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--short=8", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let checkout = dir
        .canonicalize()
        .ok()
        .and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()));
    match (checkout, rev) {
        (Some(c), Some(r)) => format!("pret/{c}@{r}"),
        _ => elf.to_string(),
    }
}

/// Loads the file just written as codegen will (`codegen::names`), and
/// checks every function in it has one identifier of its own.
fn loads(out: &str, sha: &str) -> Result<(), String> {
    use codegen::names::{Kind, SourceMap};
    let m = SourceMap::load(sha, &[out]).map_err(|e| format!("{out} doesn't load: {e}"))?;
    let unnamed: Vec<String> = m
        .functions
        .keys()
        .filter(|&&k| m.ident(Kind::Func, k).is_none())
        .map(|k| format!("0x{k:08x}"))
        .collect();
    if !unnamed.is_empty() {
        return Err(format!(
            "{} functions have no identifier: {}",
            unnamed.len(),
            unnamed.join(" ")
        ));
    }
    let count = |kind: Kind, f: &dyn Fn(&str, &str) -> bool| {
        m.idents()
            .filter(|&(id, k, key)| {
                let items = if k == Kind::Func {
                    &m.functions
                } else {
                    &m.data
                };
                k == kind && f(id, &items[&key].name)
            })
            .count()
    };
    for (kind, what, n) in [
        (Kind::Func, "functions", m.functions.len()),
        (Kind::Data, "data", m.data.len()),
    ] {
        println!(
            "map loads: {what} {n}, identifiers {}: {} qualified by file or address, {} with a trailing _",
            count(kind, &|_, _| true),
            count(kind, &|id, name| id.starts_with(&format!("{name}__"))),
            count(kind, &|id, name| id == format!("{name}_")),
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pret-names: {e}");
            ExitCode::FAILURE
        }
    }
}
