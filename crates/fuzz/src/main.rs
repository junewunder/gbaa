//! `gbaa-fuzz`: does fuzzing reveal graphics and code routes miss?
//! (assets/fuzz-coverage-spike, docs/research/fuzz-coverage.md)
//!
//! - `gbaa-fuzz run ROM SAVE OUT [STORY] --workers N --minutes M [--seed S]`:
//!   N worker processes (this binary's `worker`) for M minutes, then
//!   `report`. STORY is a folder of story-point saves (`emerald-story`)
//!   every worker seeds from (assets/fuzz-story-seeds), and of a TAS's
//!   mGBA states (`state-N.ss`, assets/fuzz-tas-bios).
//! - `gbaa-fuzz worker ROM SAVE DIR [STORY] --secs S --seed S [--index I --of N] [--pc-only 1]`:
//!   one worker (with `--pc-only 1`, stopping after its PC passes,
//!   assets/fuzz-pc-box-sweep), the I-th of N (its share of the sweeps, and its name in
//!   `OUT/shared/`, the corpus the workers trade)
//!   (src/worker.rs), writing `events.tsv` (each first sighting: a ROM
//!   instruction run, a ROM load, a compressed blob, copied run or the
//!   guessed table of computed bytes reaching the screen's memory),
//!   `kept.tsv` (the states kept), `scenes.tsv` (where each contest and
//!   Frontier challenge got to) and `stats.tsv`.
//! - `gbaa-fuzz report OUT NAMES TRUTH STARTS`: the workers joined
//!   (src/report.rs): `OUT/report.txt` and `OUT/curve.tsv`.
//! - `gbaa-fuzz story DIR`: where in the story each of a TAS's states is
//!   (src/story.rs).
//!
//! SAVE is a crafted save (`emerald-save`'s `alice.sav`), the one mutated.
//! The worker needs the `provenance` feature.
//!
//! Given Kirby & The Amazing Mirror (game code `B8KE`), the worker is
//! Kirby's (src/kirby.rs, assets/kirby-fuzz): SAVE and STORY are ignored
//! (pass `-`). What every game's worker shares is src/core.rs.

use std::path::PathBuf;
use std::process::ExitCode;

#[cfg(feature = "provenance")]
mod core;
#[cfg(feature = "provenance")]
mod kirby;
mod report;
#[cfg(feature = "provenance")]
mod rng;
mod story;
#[cfg(feature = "provenance")]
mod worker;

const USAGE: &str = "usage: gbaa-fuzz run ROM SAVE OUT [STORY] --workers N --minutes M [--seed S]\n\
       gbaa-fuzz worker ROM SAVE DIR [STORY] --secs S --seed S [--index I --of N] [--pc-only 1]\n\
       gbaa-fuzz report OUT NAMES TRUTH STARTS\n\
       gbaa-fuzz story DIR";

/// `--name value` options, every value a number.
type Opts = Vec<(String, u64)>;

/// The positional arguments and the options.
fn split(args: &[String]) -> Result<(Vec<PathBuf>, Opts), String> {
    let (mut pos, mut opts) = (Vec::new(), Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let Some(name) = a.strip_prefix("--") {
            let v = it.next().ok_or(format!("{a} needs a value"))?;
            let v = v.parse().map_err(|_| format!("{a} {v}: not a number"))?;
            opts.push((name.to_string(), v));
        } else {
            pos.push(PathBuf::from(a));
        }
    }
    Ok((pos, opts))
}

fn opt(opts: &[(String, u64)], name: &str, default: Option<u64>) -> Result<u64, String> {
    opts.iter()
        .find(|o| o.0 == name)
        .map(|o| o.1)
        .or(default)
        .ok_or(format!("--{name} is needed"))
}

/// Whether the ROM is Kirby & The Amazing Mirror (US): game code `B8KE`.
#[cfg(feature = "provenance")]
fn is_kirby(rom: &std::path::Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let mut code = [0u8; 4];
    std::fs::File::open(rom)
        .and_then(|mut f| {
            f.seek(SeekFrom::Start(0xAC))?;
            f.read_exact(&mut code)
        })
        .is_ok()
        && &code == b"B8KE"
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = (|| -> Result<(), String> {
        let (cmd, rest) = args.split_first().ok_or(USAGE)?;
        let (pos, opts) = split(rest)?;
        match (cmd.as_str(), &pos[..]) {
            #[cfg(feature = "provenance")]
            ("worker", [rom, _, dir, ..]) if is_kirby(rom) => kirby::run(
                rom,
                dir,
                opt(&opts, "secs", None)?,
                opt(&opts, "seed", None)?,
                opt(&opts, "index", Some(0))? as u32,
                opt(&opts, "of", Some(1))? as u32,
            ),
            #[cfg(feature = "provenance")]
            ("worker", [rom, save, dir, story @ ..]) if story.len() <= 1 => worker::run(
                rom,
                save,
                story.first().map(PathBuf::as_path),
                dir,
                opt(&opts, "secs", None)?,
                opt(&opts, "seed", None)?,
                opt(&opts, "index", Some(0))? as u32,
                opt(&opts, "of", Some(1))? as u32,
                opt(&opts, "pc-only", Some(0))? != 0,
            ),
            #[cfg(not(feature = "provenance"))]
            ("worker", _) => Err("the worker needs a build with --features provenance".into()),
            ("run", [rom, save, out, story @ ..]) if story.len() <= 1 => {
                let n = opt(&opts, "workers", None)?;
                let secs = 60 * opt(&opts, "minutes", None)?;
                let seed = opt(&opts, "seed", Some(1))?;
                std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
                let me = std::env::current_exe().map_err(|e| e.to_string())?;
                let kids: Vec<_> = (0..n)
                    .map(|i| {
                        std::process::Command::new(&me)
                            .arg("worker")
                            .args([rom, save, &out.join(format!("w{i:02}"))])
                            .args(story)
                            .args(["--secs", &secs.to_string()])
                            .args(["--seed", &(seed * 1000 + i).to_string()])
                            .args(["--index", &i.to_string(), "--of", &n.to_string()])
                            .spawn()
                            .map_err(|e| e.to_string())
                    })
                    .collect::<Result<_, _>>()?;
                let mut failed = 0;
                for mut k in kids {
                    if !k.wait().map_err(|e| e.to_string())?.success() {
                        failed += 1;
                    }
                }
                // The traded states are big and only for the run.
                if let Ok(dir) = std::fs::read_dir(out.join("shared")) {
                    for e in dir.flatten() {
                        if e.file_name().to_string_lossy().ends_with(".st") {
                            let _ = std::fs::remove_file(e.path());
                        }
                    }
                }
                if failed > 0 {
                    return Err(format!("{failed} of {n} workers failed"));
                }
                Ok(())
            }
            ("report", [out, names, truth, disc]) => report::run(out, names, truth, disc),
            ("story", [dir]) => story::run(dir),
            _ => Err(USAGE.into()),
        }
    })();
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("gbaa-fuzz: {e}");
            ExitCode::FAILURE
        }
    }
}
