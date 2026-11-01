//! `emerald-story ROM PRET STORY BASE OUT_DIR`: a save at each story point
//! (assets/fuzz-story-seeds, `emerald_save::story`). STORY is
//! `crates/emerald-save/story.toml`, BASE the crafted `alice.sav` (`just
//! emerald-saves`) it starts from, PRET pret's pokeemerald tree for the
//! names. Writes `OUT_DIR/NN-name.sav`, point NN with every point before
//! it applied, and prints a line a save. The player is made the story's
//! gender; the party stays alice's.

use std::path::Path;
use std::process::ExitCode;

use emerald_save::{SB2_GENDER, flash, read, story, trainers};

const USAGE: &str = "usage: emerald-story ROM PRET STORY BASE OUT_DIR";

fn run(args: &[String]) -> Result<(), String> {
    let [rom, pret, story_path, base, out] = args else {
        return Err(USAGE.into());
    };
    let load = |p: &str| std::fs::read(p).map_err(|e| format!("{p}: {e}"));
    let rom = load(rom)?;
    if rom.get(0xAC..0xB0) != Some(b"BPEE") {
        return Err("not Pokémon Emerald (US): its game code isn't BPEE".into());
    }
    let consts = story::Consts::load(Path::new(pret))?;
    let text = std::fs::read_to_string(story_path).map_err(|e| format!("{story_path}: {e}"))?;
    let story = story::parse(&text).map_err(|e| format!("{story_path}: {e}"))?;
    let mut b = read(&load(base)?).ok_or(format!("{base}: no whole save slot"))?;
    let [mut alice, _] = trainers();
    alice.female = !story.male;
    b.sb2[SB2_GENDER] = alice.female as u8;
    let points = story.point;
    std::fs::create_dir_all(out).map_err(|e| format!("{out}: {e}"))?;
    for (i, p) in points.iter().enumerate() {
        story::apply(&mut b, p, &consts, &alice, |m| story::layout(&rom, m))?;
        let path = Path::new(out).join(format!("{:02}-{}.sav", i + 1, p.name));
        std::fs::write(&path, flash(&b)).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("{}\t{}", path.display(), p.title);
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("emerald-story: {e}");
            ExitCode::FAILURE
        }
    }
}
