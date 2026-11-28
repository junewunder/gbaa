use super::*;

/// An agb program's names, as `pret-names` gives them from its ELF
/// (agb/native-spike's: Rust's v0-mangled symbols, `main`, a datum's
/// label with no size).
const MAP: &str = r#"
[map]
rom_sha1 = "00"

[functions]
0x0800017d = { name = "_RNCNvNvCsgHtZ1jaOG4l_12native_spike35__agb_main_func_167954975065260420295inner0B5_", size = 24, file = "native_spike.cgu.0" }
0x0800061b = { name = "main", size = 12, file = "native_spike.cgu.0" }
0x08001df9 = { name = "_RNvMNtCsik9bTz99YjW_3agb4mgbaNtB2_4Mgba5print", size = 80, file = "native_spike.cgu.0" }
0x08002201 = { name = "Game", size = 56 }

[data]
0x02000010 = { name = "_RNvNtCsik9bTz99YjW_3agb9interrupt15INTERRUPT_TABLE", size = 168, file = "native_spike.cgu.0" }
0x020000c0 = { name = "_RNvCsgHtZ1jaOG4l_12native_spike6FRAMES", size = 4, file = "native_spike.cgu.0" }
0x03000090 = { name = "agb_rs__program_counter", size = 0 }
"#;

fn names() -> (BTreeMap<u32, String>, BTreeMap<u32, (String, u32)>) {
    let map = crate::names::SourceMap::from_texts("00", &[MAP]).unwrap();
    let fns = map
        .functions
        .keys()
        .map(|&k| {
            (
                k,
                map.ident(crate::names::Kind::Func, k).unwrap().to_string(),
            )
        })
        .collect();
    let data = map
        .data
        .iter()
        .map(|(&k, i)| {
            let id = map.ident(crate::names::Kind::Data, k).unwrap().to_string();
            (k, (id, i.size.unwrap_or(0) as u32))
        })
        .collect();
    (fns, data)
}

const MAIN: &str = "pub struct Game { pub ram: Ram }\npub struct Ram { pub _RNvCsgHtZ1jaOG4l_12native_spike6FRAMES: u32 }\npub const ENTRY: Fn = f_0800061b;\n";
const RAM: &str = "pub struct Ram { pub _RNvCsgHtZ1jaOG4l_12native_spike6FRAMES: u32 }\n";

fn lift() -> (String, String, Vec<(u32, String)>, Named) {
    let (fns, data) = names();
    let mut head = MAIN.to_string();
    let mut table = "pub static FNS: [(u32, Fn); 3] = [\n    (0x0800017d, f_0800017d),\n    (0x0800061b, a_0800061b),\n    (0x08002201, f_08002201),\n];\n".to_string();
    let mut funcs = vec![
        (
            0x0800_017d,
            "pub fn f_0800017d(g: &mut Game, sp: u32, a: [u32; 4]) -> Result<[u32; 4], Exit> { x_0800017d(g, Regs::call(sp, a)) }\n\
             pub fn x_0800017d(g: &mut Game, s: Regs) -> Result<[u32; 4], Exit> {\n\
             \x20   let v1: u32 = 0x20000c0u32;\n\
             \x20   let v2: u32 = 0x2000018u32;\n\
             \x20   let v3: u32 = 0x3000090u32;\n\
             \x20   let v4: u32 = 0x3000094u32;\n\
             \x20   let v5: u32 = 0x20000c0u32;\n\
             \x20   let v6: u32 = 0x1cu32;\n\
             \x20   g.ram._RNvCsgHtZ1jaOG4l_12native_spike6FRAMES = v1;\n\
             \x20   f_0800061b(g, sp)?; f_08001df9(g, sp, a)?; f_0800017d0(g);\n\
             \x20   panic!(\"in f_0800017d\")\n}\n"
                .to_string(),
        ),
        (
            0x0800_061b,
            "pub fn f_0800061b(g: &mut Game, sp: u32) -> Result<(), Exit> { x_0800061b(g, Regs::call(sp, [0; 4])) }\n".to_string(),
        ),
        (
            0x0800_2201,
            "pub fn f_08002201(g: &mut Game, sp: u32) -> Result<(), Exit> { x_08002201(g, Regs::call(sp, [0; 4])) }\n".to_string(),
        ),
    ];
    let lifted = [0x0800_017d, 0x0800_061b, 0x0800_2201];
    let n = apply(&mut head, RAM, &mut table, &mut funcs, &lifted, &fns, &data);
    (head, table, funcs, n)
}

#[test]
fn an_agb_programs_functions_print_by_their_names() {
    let (head, table, funcs, n) = lift();
    let inner =
        "_RNCNvNvCsgHtZ1jaOG4l_12native_spike35__agb_main_func_167954975065260420295inner0B5_";
    let f = &funcs[0].1;
    assert!(
        f.starts_with(&format!("pub fn {inner}(g: &mut Game")),
        "{f}"
    );
    // The body keeps its key, for a stop's backtrace.
    assert!(f.contains("x_0800017d(g, Regs::call"), "{f}");
    // `main` is Rust's and the printer's: the map makes it `main_`.
    assert!(f.contains("main_(g, sp)?;"), "{f}");
    // Not a lifted function: its call is left as it is; nor a longer word.
    assert!(f.contains("f_08001df9(g, sp, a)?; f_0800017d0(g);"), "{f}");
    assert!(f.contains(&format!("panic!(\"in {inner}\")")), "{f}");
    // `Game` is the prelude's: a `_` makes it the function's own.
    assert!(
        funcs[2].1.starts_with("pub fn Game_(g: &mut Game"),
        "{}",
        funcs[2].1
    );
    assert!(
        table.contains(&format!("(0x0800017d, {inner}),")),
        "{table}"
    );
    // The adapter keeps its key too.
    assert!(table.contains("(0x0800061b, a_0800061b),"), "{table}");
    assert!(table.contains("(0x08002201, Game_),"), "{table}");
    assert!(head.contains("pub const ENTRY: Fn = main_;"), "{head}");
    assert_eq!((n.functions, n.of_functions), (3, 3));
}

#[test]
fn an_address_in_a_named_datum_prints_as_its_name() {
    let (head, _, funcs, n) = lift();
    let f = &funcs[0].1;
    let frames = "_RNvCsgHtZ1jaOG4l_12native_spike6FRAMES";
    let table = "_RNvNtCsik9bTz99YjW_3agb9interrupt15INTERRUPT_TABLE";
    assert!(
        f.contains(&format!("let v1: u32 = {frames} /* 0x020000c0 */;")),
        "{f}"
    );
    assert!(
        f.contains(&format!("let v2: u32 = ({table} + 0x8) /* 0x02000018 */;")),
        "{f}"
    );
    // A label with no size holds its own address only.
    assert!(
        f.contains("let v3: u32 = agb_rs__program_counter /* 0x03000090 */;"),
        "{f}"
    );
    assert!(f.contains("let v4: u32 = 0x3000094u32;"), "{f}");
    assert!(f.contains("let v6: u32 = 0x1cu32;"), "{f}");
    // The field of the same name is a field, not a clash.
    assert!(f.contains(&format!("g.ram.{frames} = v1;")), "{f}");
    assert!(head.contains(&format!("pub const {frames}: u32 = 0x020000c0;")));
    assert!(head.contains(&format!("pub const {table}: u32 = 0x02000010;")));
    assert!(head.contains("pub const agb_rs__program_counter: u32 = 0x03000090;"));
    assert_eq!((n.data, n.constants, n.const_lines), (3, 4, 5));
}

#[test]
fn a_name_the_crate_has_takes_a_suffix() {
    let mut taken = BTreeSet::new();
    words("let mut b = s.r0; Regs::call(f_08000001)", &mut taken);
    assert!(taken.contains("b") && taken.contains("Regs") && taken.contains("s"));
    // A field or method after a `.` names no item.
    assert!(!taken.contains("r0") && taken.contains("call"));
    assert_eq!(numbered("f_08000001"), Some(0x0800_0001));
    assert_eq!(numbered("g_08000001"), None);
}

/// Since native/lift-inline-temps a single-use constant prints inside its
/// use, not on a `let` line of its own (native/lift-data-names-inline).
#[test]
fn an_inlined_constant_prints_as_its_name() {
    let (fns, data) = names();
    let frames = "_RNvCsgHtZ1jaOG4l_12native_spike6FRAMES";
    let table = "_RNvNtCsik9bTz99YjW_3agb9interrupt15INTERRUPT_TABLE";
    let mut head = MAIN.to_string();
    let mut fns_table = String::new();
    let mut funcs = vec![(
        0x0800_2201,
        "pub fn f_08002201(g: &mut Game, sp: u32) -> Result<(), Exit> {\n\
         \x20   r0 = 0x20000c0u32;\n\
         \x20   r1 = f_0800061b(g, sp, 0x2000018u32)?.wrapping_add(0x20000c0u32.wrapping_sub(r0));\n\
         \x20   g.set(Ref::<t::S>::new(0x2000014u32 as u32).x(), r1);\n\
         \x20   match r1 { 0x20000c0u32 => {}, 0x2000014u32 => {}, _ => {} }\n\
         \x20   // 0x20000c0u32 in a comment\n\
         \x20   let v9: u32 = r0.wrapping_add(0x4u32); let w = x0x20000c0u32;\n\
         \x20   panic!(\"at 0x20000c0u32\")\n}\n"
            .to_string(),
    )];
    let n = apply(
        &mut head,
        RAM,
        &mut fns_table,
        &mut funcs,
        &[0x0800_2201],
        &fns,
        &data,
    );
    let f = &funcs[0].1;
    assert!(
        f.contains(&format!("r0 = {frames} /* 0x020000c0 */;")),
        "{f}"
    );
    assert!(
        f.contains(&format!(
            "(g, sp, ({table} + 0x8) /* 0x02000018 */)?.wrapping_add({frames} /* 0x020000c0 */.wrapping_sub(r0));"
        )),
        "{f}"
    );
    assert!(
        f.contains(&format!("new(({table} + 0x4) /* 0x02000014 */ as u32)")),
        "{f}"
    );
    // A pattern takes the name, but not a sum.
    assert!(
        f.contains(&format!(
            "{{ {frames} /* 0x020000c0 */ => {{}}, 0x2000014u32 => {{}}"
        )),
        "{f}"
    );
    // Not in a comment, a string, a longer word, or outside a datum.
    assert!(f.contains("// 0x20000c0u32 in a comment"), "{f}");
    assert!(f.contains("panic!(\"at 0x20000c0u32\")"), "{f}");
    assert!(f.contains("let w = x0x20000c0u32;"), "{f}");
    assert!(f.contains("r0.wrapping_add(0x4u32)"), "{f}");
    assert!(head.contains(&format!("pub const {frames}: u32 = 0x020000c0;")));
    assert_eq!((n.data, n.constants, n.unnamed), (2, 5, 1));
}

/// A stand-in that names the lift's function and global (unit
/// native/standin-lift-names): its names are the lift's, and its `why`,
/// a comment, takes none.
#[test]
fn a_stand_ins_names_are_the_lifts() {
    let (fns, data) = names();
    let frames = "_RNvCsgHtZ1jaOG4l_12native_spike6FRAMES";
    let print = fns[&0x0800_1df9].clone();
    let code = format!(
        "pub fn f_08002201(g: &mut Game, sp: u32) -> Result<(), Exit> {{\n\
         \x20   // {print} and {frames}, by name\n\
         \x20   {print}(g, sp)?;\n\
         \x20   let x = g.ram.{frames}.wrapping_add({frames});\n\
         \x20   let k = call_fn(g, {print}, sp, [0; 4]);\n\
         \x20   let p = t::S; let s = \"{print}\";\n\
         \x20   Ok(())\n}}\n"
    );
    // `Game`, a function's name too, is the prelude's type.
    let own: BTreeSet<String> = ["Game".to_string()].into();
    let resolved = resolve(&code, &fns, &data, &own);
    assert!(resolved.contains("    f_08001df9(g, sp)?;"), "{resolved}");
    assert!(resolved.contains("(g: &mut Game, sp"), "{resolved}");
    // Not called: its key, as `call_fn` takes it.
    assert!(
        resolved.contains("call_fn(g, 0x08001df9u32, sp"),
        "{resolved}"
    );
    assert!(
        resolved.contains(&format!("g.ram.{frames}.wrapping_add(0x020000c0u32)")),
        "{resolved}"
    );
    assert!(resolved.contains(&format!("// {print} and {frames}, by name")));
    assert!(
        resolved.contains(&format!("t::S; let s = \"{print}\";")),
        "{resolved}"
    );
    // The lift: the stand-in as printed, its `why` naming the function in
    // the comment above.
    let mut head = MAIN.to_string();
    let mut table = String::new();
    let mut funcs = vec![
        (
            0x0800_2201,
            format!("// f_08002201: a stand-in: calls {print}\n{resolved}"),
        ),
        (
            0x0800_1df9,
            "pub fn f_08001df9(g: &mut Game, sp: u32) -> Result<(), Exit> { x_08001df9(g, Regs::call(sp, [0; 4])) }\n".to_string(),
        ),
    ];
    apply(
        &mut head,
        RAM,
        &mut table,
        &mut funcs,
        &[0x0800_2201, 0x0800_1df9],
        &fns,
        &data,
    );
    let f = &funcs[0].1;
    assert!(f.contains(&format!("    {print}(g, sp)?;")), "{f}");
    assert!(
        f.contains(&format!("wrapping_add({frames} /* 0x020000c0 */)")),
        "{f}"
    );
    assert!(
        funcs[1].1.starts_with(&format!("pub fn {print}(g")),
        "{}",
        funcs[1].1
    );
    assert!(head.contains(&format!("pub const {frames}: u32 = 0x020000c0;")));
}
