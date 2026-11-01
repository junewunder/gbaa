//! `gbaa-desktop ROM`: runs a ROM through the interpreter. See
//! [`desktop::USAGE`].

fn main() -> std::process::ExitCode {
    desktop::main(None, None)
}
