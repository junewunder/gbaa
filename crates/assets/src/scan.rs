//! `gbaa-assets scan` (docs/asset-extraction-design.md, Graphics coverage,
//! option c): every LZ77 blob in a ROM found statically, with no run and
//! no pret, for the graphics fuzzing and routes didn't reach.
//!
//! - **The rule** (Nintenlord's, as NLZ-GBA Advance and its kin scan): a
//!   word-aligned `0x10` header byte, an unpacked size from [`MIN`] to
//!   [`MAX`] and a multiple of 4, and a stream that decodes to that size
//!   inside the ROM without a back-reference before its first byte. A blob
//!   found is skipped over, so nothing inside its packed bytes is taken
//!   for another.
//! - **Not reached**: a blob whose start a run neither decompressed nor
//!   copied a byte of (a fuzz run's `packed`, `copied` and `load` rows, or
//!   a route's manifest; [`crate::coverage::Found`]).
//! - **Curve** (`--curve STEP`, assets/kirby-fuzz): for a fuzz run, the
//!   blobs reached every STEP seconds of run time, `RUN/scan-curve.tsv`;
//!   the scan is the only denominator for a game with no decomp.
//! - **Export**: each one not reached as a greyscale 4bpp sheet,
//!   `scan/lz_<addr>.png`, its PNG text and `scan/scan.toml` marking it
//!   found by scan, with no palette and no layout (a scan knows neither
//!   which colours nor which screen arrangement the game gives it).

use std::fmt::Write as _;
use std::path::Path;

use crate::coverage::Found;
use crate::graphics;

/// The smallest unpacked size taken: one 4bpp tile, or a 16-colour palette.
pub const MIN: usize = 32;
/// The largest: all of EWRAM, past which nothing a game unpacks fits.
pub const MAX: usize = 0x4_0000;

/// One LZ77 blob: its ROM address, packed length (header included) and
/// unpacked size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blob {
    pub addr: u32,
    pub packed: u32,
    pub size: u32,
}

/// Every LZ77 blob in `rom` (mapped at `0x0800_0000`) by the rule above, in
/// address order.
pub fn lz77_blobs(rom: &[u8]) -> Vec<Blob> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 <= rom.len() {
        if rom[i] == 0x10 {
            let size = u32::from_le_bytes(rom[i..i + 4].try_into().unwrap()) >> 8;
            let s = size as usize;
            if (MIN..=MAX).contains(&s)
                && s.is_multiple_of(4)
                && let Some((data, n)) = graphics::lz77(&rom[i..])
                && data.len() == s
            {
                out.push(Blob {
                    addr: 0x0800_0000 + i as u32,
                    packed: n as u32,
                    size,
                });
                i += n.next_multiple_of(4);
                continue;
            }
        }
        i += 4;
    }
    out
}

/// Whether a run reached `b`: decompressed it, loaded from its start, or
/// copied any of its packed bytes.
pub fn reached(found: &Found, b: &Blob) -> bool {
    found.packed.contains(&b.addr)
        || found.loads.contains_key(&b.addr)
        || found
            .bytes
            .range(b.addr..b.addr + b.packed)
            .next()
            .is_some()
}

/// A fuzz run's blobs reached over time (assets/kirby-fuzz): a TSV line
/// every `step` seconds of run time and at its end, of the seconds, the
/// blobs reached, all of them, and the share.
pub fn curve(blobs: &[Blob], dir: &Path, step: f64) -> Result<String, String> {
    let (_, _, end) = crate::coverage::from_fuzz_until(dir, f64::INFINITY)?;
    let mut out = String::from("secs\treached\tblobs\tshare\n");
    let mut t = step;
    loop {
        let t_now = t.min(end);
        let (_, all, _) = crate::coverage::from_fuzz_until(dir, t_now)?;
        let n = blobs.iter().filter(|b| reached(&all, b)).count();
        let _ = writeln!(
            out,
            "{t_now:.0}\t{n}\t{}\t{:.1}",
            blobs.len(),
            100.0 * n as f64 / blobs.len().max(1) as f64
        );
        if t >= end {
            return Ok(out);
        }
        t += step;
    }
}

/// What the PNG text and the manifest say of a scanned sheet.
pub const MARK: &str = "found by scan, no palette/layout";

/// Writes each blob in `blobs` as a greyscale 4bpp sheet under
/// `dir/scan/`, with `scan.toml` listing them; returns the manifest.
pub fn write(rom: &[u8], blobs: &[Blob], dir: &Path) -> Result<String, String> {
    let sub = dir.join("scan");
    std::fs::create_dir_all(&sub).map_err(|e| format!("{}: {e}", sub.display()))?;
    let mut toml = format!(
        "# gbaa-assets scan: LZ77 blobs no run reached, {MARK}\n\
         # (docs/asset-extraction-design.md, Graphics coverage)\n"
    );
    let grey = graphics::grey(4);
    for b in blobs {
        let off = (b.addr - 0x0800_0000) as usize;
        let (data, _) = graphics::lz77(&rom[off..]).ok_or("blob no longer decodes")?;
        let tiles = data.len() / 32;
        let (w, h, px) = graphics::tile_pixels(&data, 4, graphics::sheet_width(tiles));
        let name = format!("lz_{:08x}.png", b.addr);
        let png = graphics::indexed_png_text(w, h, 4, &px, &grey, &[("Comment", MARK)])?;
        let p = sub.join(&name);
        std::fs::write(&p, png).map_err(|e| format!("{}: {e}", p.display()))?;
        let _ = write!(
            toml,
            "\n[[blob]]\naddr = {:#010x}\npacked = {}\nsize = {}\nfile = \"scan/{name}\"\n\
             found = \"scan\"\npalette = \"none\"\nlayout = \"none\"\n",
            b.addr, b.packed, b.size
        );
    }
    let p = sub.join("scan.toml");
    std::fs::write(&p, &toml).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(toml)
}
