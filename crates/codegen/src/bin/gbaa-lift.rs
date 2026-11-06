//! `gbaa-lift ROM OUT [--name NAME] [--holes FILE] [--bars FILE]
//! [--names DIR] [--layout guest|packed] [--drop-standin KEY] [--plant AT]
//! [--fall-as-tail-call] [--irq-at-waits] [--ime-at-waits] [--plant-slot=AT] [--shadow-locals]
//! [--copies N]`:
//! the stage-A lifter (`codegen::native`; docs/native-port-design.md,
//! stage A). Writes the whole program as a Rust crate to OUT (`Cargo.toml`,
//! `src/`, a copy of the ROM for its data, `report.txt`), with the
//! stand-ins and jump targets the holes file gives. Fails, naming each, if
//! a hard hole is left: the crate then has a `compile_error!` for each and
//! won't build.
//!
//! RAM's globals (`codegen::native::globals`, unit `native/ram-globals`)
//! take their names from the names files in DIR whose `[map]` names this
//! ROM (default the repo's `names/`; none is fine). `--layout guest` (the
//! default) puts them at the guest's offsets, the native check's layout;
//! `--layout packed` makes them fields in the lifter's order, the rest of
//! RAM bytes, the play layout, which must draw the same frames (unit
//! `native/lift-packed-layout`).
//!
//! `--bars FILE` (`crates/codegen/lift-check.toml`, unit
//! `native/lift-bars`) holds the report's metrics to the maxima of
//! `[program.NAME.bars]`: the lift fails naming each metric over its bar,
//! with both numbers, after writing the crate. Metrics under their bars
//! are listed, to be lowered.
//!
//! The check's controls (`just lift-check`): `--drop-standin KEY` leaves
//! the holes file's stand-in for function KEY (hex) out, a planted hard
//! hole the lift must name; `--plant AT` makes the store of the
//! instruction at AT (hex) store its value plus one, a planted wrong store
//! the native check must catch; `--fall-as-tail-call` prints a fall or
//! jump into another function's entry as stage A first did, a tail call
//! with r4-r12 and the flags zeroed, which the native check must catch on
//! `agb/native-fallthrough` (unit `native/lift-fallthrough`);
//! `--irq-at-waits` runs an IRQ at a wait whatever the CPSR's I bit, as
//! stage A first did, where the hardware holds it until a critical
//! section's end, which the native check must catch on
//! `agb/native-irq-hold` (unit `native/lift-irq-hold`); `--ime-at-waits`
//! leaves an IRQ pending when a store sets `IME` again to the next wait,
//! as stage A first did, where the hardware takes it at once, which the
//! native check must catch on `agb/native-ime-release` (unit
//! `native/ime-release`); `--plant-slot=AT`
//! makes the store of the instruction at AT, which must be to a stack slot
//! printed as a Rust local, store its value plus one, which the native
//! check must catch on `agb/native-frame-locals` (unit
//! `native/frame-locals`).
//!
//! `--shadow-locals` keeps every stack local in the frame's bytes as well
//! and prints a `shadow local:` line to stderr wherever a load from a
//! local differs from them: something the escape analysis didn't see
//! wrote the frame. A check by hand, not part of the lift check.
//!
//! `src/` is `main.rs` (the prelude, RAM, `ENTRY`), `fns.rs` (`FNS`) and
//! the functions in modules of about 4,000 lines (`codegen::native::modules`,
//! unit `native/lift-modules`), so rustc builds them in parallel; a `.rs`
//! file there from an earlier lift that this one didn't write is removed.
//! `--copies N` writes the modules N times over under different names: a
//! synthetic large lift, for measuring the build of a game-sized crate.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .canonicalize()
        .expect("crates/ exists")
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = || {
        eprintln!(
            "usage: gbaa-lift ROM OUT [--name NAME] [--holes FILE] [--bars FILE] [--names DIR] [--layout guest|packed] [--drop-standin KEY] [--plant AT] [--fall-as-tail-call] [--irq-at-waits] [--ime-at-waits] [--plant-slot=AT] [--shadow-locals] [--copies N]"
        );
        ExitCode::FAILURE
    };
    let [rom, out, rest @ ..] = &args[..] else {
        return usage();
    };
    let (mut name, mut holes, mut bars, mut names) = (None, None, None, None);
    let (mut drop, mut plant, mut tail, mut irq_at_waits) = (None, None, false, false);
    let mut ime_at_waits = false;
    let (mut plant_slot, mut shadow, mut copies) = (None, false, 1);
    let mut layout = codegen::native::globals::Layout::Guest;
    let hex = |s: &str| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok();
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        if a == "--fall-as-tail-call" {
            tail = true;
            continue;
        }
        if a == "--irq-at-waits" {
            irq_at_waits = true;
            continue;
        }
        if a == "--ime-at-waits" {
            ime_at_waits = true;
            continue;
        }
        if a == "--shadow-locals" {
            shadow = true;
            continue;
        }
        // One word, so the lift check's `planted_flags` can name it.
        if let Some(at) = a.strip_prefix("--plant-slot=") {
            match hex(at) {
                Some(at) => plant_slot = Some(at),
                None => return usage(),
            }
            continue;
        }
        match (a.as_str(), it.next()) {
            ("--name", Some(n)) => name = Some(n.clone()),
            ("--holes", Some(f)) => holes = Some(PathBuf::from(f)),
            ("--bars", Some(f)) => bars = Some(PathBuf::from(f)),
            ("--names", Some(d)) => names = Some(PathBuf::from(d)),
            ("--layout", Some(l)) if l == "guest" => {
                layout = codegen::native::globals::Layout::Guest
            }
            ("--layout", Some(l)) if l == "packed" => {
                layout = codegen::native::globals::Layout::Packed
            }
            ("--drop-standin", Some(k)) if hex(k).is_some() => drop = hex(k),
            ("--plant", Some(a)) if hex(a).is_some() => plant = hex(a),
            ("--copies", Some(n)) if n.parse::<usize>().is_ok_and(|n| n >= 1) => {
                copies = n.parse().unwrap()
            }
            _ => return usage(),
        }
    }
    let bytes = match std::fs::read(rom) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("gbaa-lift: {rom}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let name = name.unwrap_or_else(|| {
        Path::new(rom)
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    let mut holes = match &holes {
        Some(p) => match std::fs::read_to_string(p)
            .map_err(|e| e.to_string())
            .and_then(|t| codegen::native::Holes::parse(&t))
        {
            Ok(h) => h,
            Err(e) => {
                eprintln!("gbaa-lift: {}: {e}", p.display());
                return ExitCode::FAILURE;
            }
        },
        None => codegen::native::Holes::default(),
    };
    if let Some(k) = drop
        && holes.standins.remove(&k).is_none()
    {
        eprintln!("gbaa-lift: --drop-standin {k:#010x}: the holes file has no stand-in for it");
        return ExitCode::FAILURE;
    }
    holes.plant = plant;
    holes.fall_as_tail_call = tail;
    holes.irq_at_waits = irq_at_waits;
    holes.ime_at_waits = ime_at_waits;
    holes.plant_slot = plant_slot;
    holes.shadow_locals = shadow;
    holes.copies = copies;
    holes.layout = layout;
    let names = names.unwrap_or_else(|| crates_dir().parent().expect("the repo").join("names"));
    let sha = codegen::names::sha1_hex(&bytes);
    match codegen::names::files_for(&names, &sha)
        .map_err(|e| e.to_string())
        .and_then(|fs| codegen::names::SourceMap::load(&sha, &fs))
    {
        Ok(map) => {
            holes.names = map
                .data
                .iter()
                .filter_map(|(&a, item)| {
                    let id = map.ident(codegen::names::Kind::Data, a)?;
                    Some((a, (id.to_string(), item.size.unwrap_or(0) as u32)))
                })
                .collect();
        }
        Err(e) => {
            eprintln!("gbaa-lift: {}: {e}", names.display());
            return ExitCode::FAILURE;
        }
    }
    // Read before the lift, so a bad file fails at once.
    let bars = match &bars {
        Some(p) => match std::fs::read_to_string(p)
            .map_err(|e| e.to_string())
            .and_then(|t| codegen::native::metrics::bars(&t, &name))
        {
            Ok(b) => Some((p, b)),
            Err(e) => {
                eprintln!("gbaa-lift: {}: {e}", p.display());
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };
    let lifted = codegen::native::lift(&bytes, &name, &holes);
    let out = Path::new(out);
    let crates = crates_dir();
    let cargo = format!(
        "# Generated by gbaa-lift. Don't edit; rerun `just lift`.\n\
         [package]\nname = \"gbaa-native-{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n\
         # Its own workspace: not a member of gbaa's.\n[workspace]\n\n\
         [[bin]]\nname = \"gbaa-native-{name}\"\npath = \"src/main.rs\"\n\n\
         [dependencies]\n\
         armv4t = {{ path = \"{c}/armv4t\" }}\n\
         runtime = {{ path = \"{c}/runtime\" }}\n\
         native-host = {{ path = \"{c}/native-host\" }}\n\n\
         [profile.release]\ndebug = false\nincremental = false\n",
        c = crates.display()
    );
    let write = |p: &Path, b: &[u8]| -> std::io::Result<()> {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        // Unchanged files keep their times, so cargo doesn't rebuild.
        if std::fs::read(p).is_ok_and(|old| old == b) {
            return Ok(());
        }
        std::fs::write(p, b)
    };
    let src = out.join("src");
    let stale = |src: &Path| -> std::io::Result<()> {
        let Ok(dir) = std::fs::read_dir(src) else {
            return Ok(());
        };
        for e in dir {
            let p = e?.path();
            let mine = p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".rs") && !lifted.files.iter().any(|(f, _)| f == n));
            if mine {
                std::fs::remove_file(&p)?;
            }
        }
        Ok(())
    };
    let r = write(&out.join("Cargo.toml"), cargo.as_bytes())
        .and_then(|_| {
            lifted
                .files
                .iter()
                .try_for_each(|(f, t)| write(&src.join(f), t.as_bytes()))
        })
        .and_then(|_| stale(&src))
        .and_then(|_| write(&out.join("rom.gba"), &bytes))
        .and_then(|_| write(&out.join("report.txt"), lifted.report.as_bytes()));
    if let Err(e) = r {
        eprintln!("gbaa-lift: {}: {e}", out.display());
        return ExitCode::FAILURE;
    }
    print!("{}", lifted.report);
    let mut ok = true;
    if !lifted.holes.is_empty() {
        eprintln!(
            "gbaa-lift: {} hard holes; the crate won't build",
            lifted.holes.len()
        );
        ok = false;
    }
    if let Some((p, bars)) = bars {
        let v = codegen::native::metrics::check(&lifted.metrics, &bars);
        for (n, x, b) in &v.lowerable {
            println!("gbaa-lift: {name}: {n} is {x}, under its bar of {b}: lower it");
        }
        for f in &v.failures {
            eprintln!("gbaa-lift: {name}: {f} ({})", p.display());
        }
        if v.failures.is_empty() {
            println!(
                "gbaa-lift: {name}: every metric within its bar ({})",
                p.display()
            );
        } else {
            ok = false;
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
