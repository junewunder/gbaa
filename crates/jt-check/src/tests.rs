use super::*;
use analysis::vsa::domain::Src;
use analysis::vsa::func::Jump;

const B: u32 = ROM_BASE;

fn sym(name: &str, value: u32, size: u32, func: bool) -> Sym {
    Sym {
        name: name.into(),
        value,
        size,
        func,
        global_notype: false,
        section: 1,
    }
}

/// One Thumb function `f` at 0..0x40 with a data region at 0x10..0x20
/// holding a pool word and an inline three-entry table; a second table
/// out of line in `.rodata` (section 2), its start in the pool.
fn fixture() -> (Elf, Vec<u8>) {
    let elf = Elf {
        sections: vec![
            Section {
                index: 1,
                addr: B,
                size: 0x40,
                exec: true,
                data: true,
            },
            Section {
                index: 2,
                addr: B + 0x40,
                size: 0x10,
                exec: false,
                data: true,
            },
        ],
        syms: vec![
            sym("$t", B, 0, false),
            sym("f", B | 1, 0x40, true),
            sym("$d", B + 0x10, 0, false),
            sym("$t", B + 0x20, 0, false),
        ],
    };
    let mut rom = vec![0u8; 0x50];
    let mut w =
        |at: u32, v: u32| rom[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
    w(0x10, B + 0x40); // pool: the out-of-line table
    w(0x14, B + 0x20); // inline table
    w(0x18, B + 0x24);
    w(0x1c, B + 0x28);
    w(0x40, B + 0x2c); // out-of-line table
    w(0x44, B + 0x30);
    w(0x48, 0x0300_0000); // not code: ends the run
    (elf, rom)
}

#[test]
fn truth_tables() {
    let (elf, rom) = fixture();
    let t = Truth::build(&elf, &rom);
    assert_eq!(t.funcs.len(), 1);
    assert_eq!(t.code.len(), 2);
    let tables: Vec<(u32, usize)> = t.tables.iter().map(|t| (t.addr, t.targets.len())).collect();
    assert_eq!(tables, [(B + 0x40, 2), (B + 0x14, 3)]);
}

#[test]
fn verdicts() {
    let (elf, rom) = fixture();
    let t = Truth::build(&elf, &rom);
    let jump = |targets: &[u32], first: u32| {
        JumpRes::Resolved(Jump {
            targets: targets.iter().map(|a| (B + a) | 1).collect(),
            table: Some(Src {
                first: B + first,
                last: B + first,
                size: 4,
            }),
        })
    };
    let mut rec = Recovery::default();
    rec.jumps.insert(B + 2, jump(&[0x20, 0x24, 0x28], 0x14));
    rec.jumps.insert(B + 4, jump(&[0x2c, 0x34], 0x40));
    rec.jumps.insert(B + 6, JumpRes::Unresolved);
    let s = Score::new(&t, &rec);
    assert_eq!((s.get(Verdict::Exact), s.get(Verdict::Wrong)), (1, 1));
    assert!(s.missed.is_empty() && s.unresolved == 1);

    let mut rec = Recovery::default();
    rec.jumps.insert(B + 2, jump(&[0x20, 0x28], 0x18));
    let s = Score::new(&t, &rec);
    assert_eq!(s.get(Verdict::Subset), 1);
    assert_eq!(s.missed.len(), 1);
}
