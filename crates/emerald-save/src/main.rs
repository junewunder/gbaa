//! `emerald-save ROM OUT_DIR`: writes `alice.sav` and `bob.sav` to OUT_DIR,
//! two Emerald saves standing below Oldale Town's cable club attendant
//! (see the crate's docs and `trainers`).
//!
//! The base is the game's own new game: Emerald (US, the pokeemerald
//! build, sha1 f3ae0881…) runs in the interpreter from reset with the
//! new-game route's presses (`units/emerald-newgame.toml`: Start, A, then
//! A every 20 frames through Birch's speech) to the truck, and the three
//! save blocks are read where its pointers say. Deterministic: the same
//! ROM gives the same files.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use emerald_save::{Blocks, INSIDE_OF_TRUCK, SB1_SIZE, SB2_SIZE, STORAGE_SIZE};
use runtime::Machine;
use runtime::io::key;
use runtime::platform::{Frame, Platform};

const USAGE: &str = "usage: emerald-save ROM OUT_DIR";

/// `gSaveBlock2Ptr`, `gSaveBlock1Ptr`, `gPokemonStoragePtr` (the ELF's
/// symbols; `MoveSaveBlocks_ResetHeap` moves the blocks, so read these).
const SB2_PTR: u32 = 0x0300_5D90;
const SB1_PTR: u32 = 0x0300_5D8C;
const STORAGE_PTR: u32 = 0x0300_5D94;
/// The truck's first frame on the new-game route.
const TRUCK_FRAME: u64 = 5400;
const HOLD: u64 = 6;

/// The new-game route's presses: the buttons held during frame `f`.
fn keys(f: u64) -> u16 {
    let held = |at: u64| (at..at + HOLD).contains(&f);
    let mut k = 0;
    if held(300) {
        k |= key::START;
    }
    if held(900) || (1100..=5380).step_by(20).any(held) {
        k |= key::A;
    }
    k
}

/// The keys [`Machine::present_frame`] latches for the next frame.
struct Pad(u16);

impl Platform for Pad {
    fn present(&mut self, _frame: &Frame) {}

    fn keys(&mut self) -> u16 {
        self.0
    }
}

fn base(rom: &'static [u8]) -> Result<Blocks, String> {
    if rom.get(0xAC..0xB0) != Some(b"BPEE") {
        return Err("not Pokémon Emerald (US): its game code isn't BPEE".into());
    }
    let mut m = Box::new(Machine::new(rom, Pad(0)));
    for f in 1..=TRUCK_FRAME {
        // Held from the frame's start, as `gbaa-desktop --input` holds them.
        m.platform.0 = keys(f);
        m.bus.io.set_keys(keys(f));
        m.run_frame();
        if let Some(fault) = &m.fault {
            return Err(format!("frame {f}: {fault:?}"));
        }
    }
    let read = |ptr: u32, len: usize| -> Vec<u8> {
        let at = m.bus.read32(ptr);
        (0..len as u32).map(|i| m.bus.read8(at + i)).collect()
    };
    let b = Blocks {
        sb2: read(SB2_PTR, SB2_SIZE),
        sb1: read(SB1_PTR, SB1_SIZE),
        storage: read(STORAGE_PTR, STORAGE_SIZE),
    };
    match emerald_save::location(&b) {
        l if l == INSIDE_OF_TRUCK => Ok(b),
        l => Err(format!(
            "frame {TRUCK_FRAME} isn't in the truck (map {}.{}): not the pokeemerald build?",
            l.group, l.num
        )),
    }
}

fn run(rom: &Path, out: &Path) -> Result<(), String> {
    let rom = std::fs::read(rom).map_err(|e| format!("{}: {e}", rom.display()))?;
    let base = base(Box::leak(rom.into_boxed_slice()))?;
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    for t in emerald_save::trainers() {
        let path = out.join(format!("{}.sav", t.file));
        let save = emerald_save::flash(&emerald_save::patch(&base, &t));
        std::fs::write(&path, save).map_err(|e| format!("{}: {e}", path.display()))?;
        let party: Vec<String> = t
            .party
            .iter()
            .map(|m| format!("{} L{}", m.species.name, m.level))
            .collect();
        println!(
            "{}: {} ({:05}), {}",
            path.display(),
            t.name,
            t.id & 0xFFFF,
            party.join(", ")
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [rom, out] = &args[..] else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match run(rom, out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("emerald-save: {e}");
            ExitCode::FAILURE
        }
    }
}
