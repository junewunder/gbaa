//! gbaa's addition to the cmake crate (vendor/README.md): a build that
//! depends on more than its sources, a toolchain file or system headers,
//! names them in a list file given by `CMAKE_INPUTS`. Cargo reruns the
//! build script when one changes, and the build directory is cleared when
//! their contents differ from the last configure's, so no cached result
//! (a library not found, an option turned off) outlives what it came from.
//! A build directory from before the list (no stamp) is cleared as well.
//! The environment variables the crate reads (`CMAKE_TOOLCHAIN_FILE` and the
//! rest) count too, and with a list each gets a `rerun-if-env-changed`.
//!
//! The list: one path a line, relative to the list's own directory or
//! absolute; blank lines and `#` comments are skipped. A directory counts
//! with everything in it. A path that doesn't exist is left to the hash
//! (Cargo would rerun every build over it), so creating it is only noticed
//! with the next change to something that does.

use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const VAR: &str = "CMAKE_INPUTS";

/// Whether `CMAKE_INPUTS` is set, so env reads are tracked.
static TRACKING: AtomicBool = AtomicBool::new(false);

/// At the start of a build, before any other variable is read.
pub(crate) fn track() {
    println!("cargo:rerun-if-env-changed={VAR}");
    TRACKING.store(env::var_os(VAR).is_some(), Ordering::Relaxed);
}

/// Each variable the crate reads, the first time.
pub(crate) fn env_read(var: &str) {
    if TRACKING.load(Ordering::Relaxed) {
        println!("cargo:rerun-if-env-changed={var}");
    }
}

/// Before configuring: `read` is what the crate has read of the environment.
pub(crate) fn clear_if_changed(build_dir: &Path, read: &HashMap<String, Option<OsString>>) {
    let list = match env::var_os(VAR) {
        Some(list) => PathBuf::from(list),
        None => return,
    };
    let text = fs::read_to_string(&list)
        .unwrap_or_else(|e| panic!("{VAR}: reading {}: {e}", list.display()));
    let base = list.parent().unwrap_or(Path::new("."));
    let mut paths = vec![list.clone()];
    for line in text.lines() {
        let line = line.trim();
        if !line.is_empty() && !line.starts_with('#') {
            paths.push(base.join(line));
        }
    }

    let mut hash = Fnv::new();
    for path in &paths {
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
        hash.bytes(path.as_os_str().as_encoded_bytes());
        hash.tree(path, true);
    }
    let mut vars: Vec<_> = read.iter().collect();
    vars.sort();
    for (var, value) in vars {
        hash.bytes(var.as_bytes());
        match value {
            Some(value) => hash.bytes(value.as_encoded_bytes()),
            None => hash.bytes(b"unset"),
        }
    }

    let stamp_file = build_dir.with_file_name("cmake-inputs.stamp");
    let stamp = format!("{:016x}\n", hash.0);
    if fs::read_to_string(&stamp_file).ok().as_deref() != Some(stamp.as_str()) {
        if build_dir.exists() {
            println!("the inputs in {VAR} changed: cleaning out the build directory");
            fs::remove_dir_all(build_dir).unwrap();
        }
        if let Some(parent) = stamp_file.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&stamp_file, stamp).unwrap();
    }
}

/// FNV-1a 64, so the stamp doesn't change with the Rust release.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        // Ends each field, so ("ab", "c") and ("a", "bc") differ.
        self.0 = (self.0 ^ 0xff).wrapping_mul(0x0100_0000_01b3);
    }

    /// A path's kind and contents: a file's bytes, a directory's entries
    /// in name order, a link's target. `follow` follows a link at the top.
    fn tree(&mut self, path: &Path, follow: bool) {
        let meta = if follow {
            fs::metadata(path)
        } else {
            fs::symlink_metadata(path)
        };
        let meta = match meta {
            Ok(meta) => meta,
            Err(_) => return self.bytes(b"missing"),
        };
        if meta.file_type().is_symlink() {
            self.bytes(b"link");
            let target = fs::read_link(path).unwrap_or_default();
            self.bytes(target.as_os_str().as_encoded_bytes());
        } else if meta.is_dir() {
            self.bytes(b"dir");
            let mut names: Vec<_> = fs::read_dir(path)
                .map(|dir| dir.filter_map(|e| e.ok()).map(|e| e.file_name()).collect())
                .unwrap_or_default();
            names.sort();
            for name in names {
                self.bytes(name.as_encoded_bytes());
                self.tree(&path.join(name), false);
            }
        } else {
            self.bytes(b"file");
            self.bytes(&fs::read(path).unwrap_or_default());
        }
    }
}
