//! `native-spike`: agb/native-spike as a native Rust program
//! (native/agb-spike). Takes gbaa-desktop's headless flags, so the same
//! script runs both (src/host.rs):
//!
//! ```text
//! native-spike --headless --frames N [--input PRESSES]
//!                   [--frame-hashes F] [--dump FILE.png] [--native-record F]
//! ```
//!
//! The hardware is `runtime::hw` (native/hw-library): the play build's
//! renderer drawing the program's `Video` line by line. With no flags it
//! opens gbaa-desktop's window (`native_host`).
//! Built with `--features guest-layout`, its RAM is the ROM's EWRAM and
//! IWRAM, each global at its link address (src/game.rs, `Ram`), so the
//! native check (`route-check --native`) compares it byte for byte. The
//! play layout records the same image (`Ram::image`), which the check's
//! `--packed` compares with the check layout's.

mod game;
mod host;

pub use host::AnyHost;

fn main() -> std::process::ExitCode {
    host::main(
        "native-spike",
        true,
        |h| {
            let mut g = Box::new(game::Game {
                ram: game::Ram::new(),
                hw: runtime::hw::Hw::new(h),
            });
            g.hw.on_frame_end(game::record);
            g
        },
        game::main,
    )
}
