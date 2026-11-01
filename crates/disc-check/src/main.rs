//! `disc-check truth ELF OUT` writes the truth file and prints the names
//! rule 2 added. `disc-check score TRUTH DISCOVERED [--show N]
//! [--min-precision P] [--min-recall R]` prints precision, recall and
//! diagnostics; it exits 2 on a misaligned ARM start, and 1 if precision
//! or recall is below the minimum given.
//! `disc-check discover ROM OUT` runs code discovery
//! ([`analysis::discovery`]) on a ROM, writes the discovered-starts file
//! and prints what it found. `disc-check rom ELF OUT` writes the ROM image
//! an ELF loads (its `PT_LOAD` segments in ROM). `disc-check swis ROM...`
//! lists the BIOS calls each ROM's discovered code makes and which of them
//! the runtime doesn't implement ([`disc_check::swis`]).

use std::path::Path;
use std::process::ExitCode;

use disc_check::{build_truth, elf, format_truth, parse_discovered, parse_truth, score};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const USAGE: &str = "usage: disc-check truth ELF OUT\n       disc-check rom ELF OUT\n       disc-check score TRUTH DISCOVERED [--show N] [--min-precision P] [--min-recall R]\n       disc-check discover ROM OUT\n       disc-check swis ROM...";

fn read(path: &str) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| format!("{path}: {e}").into())
}

fn text(path: &str) -> Result<String> {
    String::from_utf8(read(path)?).map_err(|e| format!("{path}: {e}").into())
}

fn truth(elf_path: &str, out: &str) -> Result<()> {
    let syms = elf::read_syms(&read(elf_path)?).map_err(|e| format!("{elf_path}: {e}"))?;
    let t = build_truth(&syms);
    if let Some(dir) = Path::new(out).parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(out, format_truth(&t.rows)).map_err(|e| format!("{out}: {e}"))?;
    let thumb = t.rows.iter().filter(|r| r.addr & 1 != 0).count();
    println!(
        "wrote {out}: {} rows ({thumb} Thumb, {} ARM)",
        t.rows.len(),
        t.rows.len() - thumb
    );
    println!(
        "rule 2 added {}: {}",
        t.rule2_added.len(),
        t.rule2_added.join(" ")
    );
    Ok(())
}

fn rom(elf_path: &str, out: &str) -> Result<()> {
    let rom = elf::rom_image(&read(elf_path)?).map_err(|e| format!("{elf_path}: {e}"))?;
    if rom.is_empty() {
        return Err(format!("{elf_path}: no segment loads into ROM").into());
    }
    std::fs::write(out, &rom).map_err(|e| format!("{out}: {e}"))?;
    println!("wrote {out}: {} bytes", rom.len());
    Ok(())
}

fn discover(rom_path: &str, out: &str) -> Result<()> {
    let rom = read(rom_path)?;
    let t0 = std::time::Instant::now();
    let d = analysis::discovery::discover(rom);
    let secs = t0.elapsed().as_secs_f64();
    if let Some(dir) = Path::new(out).parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(out, d.starts_file()).map_err(|e| format!("{out}: {e}"))?;
    print!("{}", disc_check::discovery_report(&d));
    println!("  wrote {out} in {secs:.1} s");
    Ok(())
}

fn swis(roms: &[String]) -> Result<()> {
    for path in roms {
        let rom = read(path)?;
        let name = Path::new(path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        println!("{name}");
        print!(
            "{}",
            disc_check::swis::report(&disc_check::swis::survey(&rom))
        );
    }
    Ok(())
}

fn run(args: &[String]) -> Result<ExitCode> {
    match args {
        [cmd, elf_path, out] if cmd == "rom" => rom(elf_path, out).map(|()| ExitCode::SUCCESS),
        [cmd, rom, out] if cmd == "discover" => discover(rom, out).map(|()| ExitCode::SUCCESS),
        [cmd, roms @ ..] if cmd == "swis" && !roms.is_empty() => {
            swis(roms).map(|()| ExitCode::SUCCESS)
        }
        [cmd, elf_path, out] if cmd == "truth" => truth(elf_path, out).map(|()| ExitCode::SUCCESS),
        [cmd, truth_path, disc_path, rest @ ..] if cmd == "score" => {
            let (mut show, mut min_p, mut min_r) = (20, 0.0, 0.0);
            let mut it = rest.iter();
            while let Some(flag) = it.next() {
                let arg = it.next().ok_or(USAGE)?;
                match flag.as_str() {
                    "--show" => show = arg.parse()?,
                    "--min-precision" => min_p = arg.parse()?,
                    "--min-recall" => min_r = arg.parse()?,
                    _ => return Err(USAGE.into()),
                }
            }
            let t = parse_truth(&text(truth_path)?).map_err(|e| format!("{truth_path}: {e}"))?;
            let d = parse_discovered(&text(disc_path)?).map_err(|e| format!("{disc_path}: {e}"))?;
            match score(&t, &d) {
                Ok(s) => {
                    print!("{}", s.report(show));
                    if s.precision() < min_p || s.recall() < min_r {
                        println!("FAIL: want precision >= {min_p} and recall >= {min_r}");
                        return Ok(ExitCode::FAILURE);
                    }
                    Ok(ExitCode::SUCCESS)
                }
                Err(e) => {
                    eprintln!("{disc_path}: {e}");
                    Ok(ExitCode::from(2))
                }
            }
        }
        _ => Err(USAGE.into()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run(&args).unwrap_or_else(|e| {
        eprintln!("disc-check: {e}");
        ExitCode::FAILURE
    })
}
