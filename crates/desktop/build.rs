//! Keeps the binary off float functions newer than SteamOS's glibc
//! (sdl-build/glibc-compat.c). Linked whole-archive: nothing in the crate
//! refers to those definitions, and they must beat libm's all the same.

fn main() {
    let src = "sdl-build/glibc-compat.c";
    println!("cargo:rerun-if-changed={src}");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if os != "linux" || env != "gnu" || arch != "x86_64" {
        return;
    }
    cc::Build::new()
        .file(src)
        .link_lib_modifier("+whole-archive")
        .compile("gbaa_glibc_compat");
}
