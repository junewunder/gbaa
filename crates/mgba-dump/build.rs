//! Compiles the C shim against the libmgba that `scripts/fetch-mgba.sh`
//! builds under `$GBAA_DATA/tools/mgba/`. Without it the crate still builds
//! (so `just check` doesn't need mGBA) and the binary says how to get it.
//! `GBAA_MGBA=mgba-tas` links BizHawk 2.9's mGBA (`fetch-mgba.sh tas`)
//! instead, for replaying its movies; build that into its own target dir
//! (`just mgba-dump-tas`), so the oracle's build keeps 0.10.5.

use std::path::PathBuf;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(mgba)");
    println!("cargo::rerun-if-env-changed=GBAA_DATA");
    println!("cargo::rerun-if-env-changed=GBAA_MGBA");
    let which = std::env::var("GBAA_MGBA").unwrap_or_else(|_| "mgba".into());
    println!("cargo::rerun-if-changed=src/shim.c");
    let data = std::env::var_os("GBAA_DATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/gbaa")));
    let Some(mgba) = data.map(|d| d.join("tools").join(&which)) else {
        return;
    };
    let lib = mgba.join("build/libmgba.a");
    println!("cargo::rerun-if-changed={}", lib.display());
    if !lib.exists() {
        return;
    }
    // The macros libmgba was compiled with (fetch-mgba.sh lists them):
    // mCore's layout depends on some (ENABLE_DEBUGGERS, ENABLE_DIRECTORIES)
    // that build/include/mgba/flags.h leaves out, and a shim built without
    // them calls through the wrong fields.
    let defines = mgba.join("build/gbaa-defines");
    println!("cargo::rerun-if-changed={}", defines.display());
    let mut shim = cc::Build::new();
    shim.file("src/shim.c")
        .include(mgba.join("src/include"))
        .include(mgba.join("build/include"))
        .define("M_CORE_GBA", None)
        .warnings(false);
    for define in std::fs::read_to_string(&defines)
        .unwrap_or_default()
        .split_whitespace()
    {
        let (name, value) = define
            .split_once('=')
            .map_or((define, None), |(n, v)| (n, Some(v)));
        shim.define(name, value);
    }
    shim.compile("gbaa_mgba_shim");
    println!(
        "cargo::rustc-link-search=native={}",
        mgba.join("build").display()
    );
    println!("cargo::rustc-link-lib=static=mgba");
    // The shim's __wrap_GBAAudioSampleFIFO sees every FIFO pop (--fifo-dump).
    println!("cargo::rustc-link-arg=-Wl,--wrap=GBAAudioSampleFIFO");
    // And __wrap_GBAudioSamplePSG every PSG sample (--psg-dump).
    println!("cargo::rustc-link-arg=-Wl,--wrap=GBAudioSamplePSG");
    println!("cargo::rustc-link-lib=pthread");
    println!("cargo::rustc-link-lib=m");
    println!("cargo::rustc-cfg=mgba");
}
