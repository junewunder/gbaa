//! The load and use logs (assets/load-log, docs/asset-extraction-design.md,
//! Recording): `--assets-log DIR` in a `provenance` build writes, along a
//! headless run (and `--extract DIR`, windowed or not, into a session
//! folder under DIR, docs/asset-extraction-design.md, Extract while
//! playing),
//!
//! - `loads.tsv`: each load [`runtime::prov`] logged (a BIOS decompression,
//!   `CpuSet`, `CpuFastSet` or DMA whose source traces to ROM), with its
//!   frame; a RAM source's ROM runs in `runs`. A load the same as the last
//!   one from its source to its destination (a buffer copied every frame)
//!   isn't written again.
//! - `uses.tsv`: each BG and OBJ tile shown, by what it is (layer, bpp, the
//!   ROM bytes its first byte came from, the blob and offset that gives, its
//!   palette bank and the ROM bytes that bank came from), with the first and
//!   last frame it was shown and on how many frames: one line a distinct
//!   use, not a line a frame.
//! - `oam.tsv`: each OAM entry shown, a line whenever it changes (where it
//!   is, its size, tile, bank and flips, and the ROM bytes its first tile
//!   and its bank came from) and when it stops showing: the sequence and
//!   timing of each entry's frames, for sprites and their animations.
//! - `reached.tsv` and `shown.tsv`, the check's inputs (`gbaa-assets
//!   check-load-log`): the ROM bytes that reached palette, VRAM and OAM over
//!   the run (the provenance spike's blobs), and every tile shown on the
//!   last frame.
//!
//! A palette bank faded in is computed, so its colours are matched by value
//! against a RAM buffer of ROM-copied bytes (the spike's unfaded-buffer
//! match), found once and searched for again when it stops matching.

use runtime::Machine;
use runtime::platform::Platform;
use runtime::prov::{self, Load, op, via};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

const ROM: u32 = 0x0800_0000;
/// Frames between searches for the palette buffer when it stops matching.
const SEARCH_EVERY: u64 = 30;

/// Where a byte came from: the ROM address of its origin (0 for none), its
/// steps ([`via`] bits) and, decompressed, its offset in the unpacked blob.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Src {
    pub rom: u32,
    pub via: u32,
    pub off: u32,
}

impl Src {
    fn at(addr: u32) -> Src {
        let t = prov::tag_at(addr);
        Src {
            rom: prov::origin(t).map_or(0, |o| ROM + o),
            via: prov::steps(t),
            off: prov::offset_at(addr),
        }
    }

    fn packed(&self) -> bool {
        self.via & (via::LZ77 | via::UNPACK) != 0
    }

    /// Copied or decompressed from ROM, not computed.
    fn copied(&self) -> bool {
        self.rom != 0 && self.via & via::DERIVED == 0
    }

    /// The source of the byte `back` bytes before this one, in the same
    /// copy.
    fn minus(self, back: u32) -> Src {
        if self.packed() {
            Src {
                off: self.off.wrapping_sub(back),
                ..self
            }
        } else {
            Src {
                rom: self.rom - back,
                ..self
            }
        }
    }
}

fn via_name(v: u32) -> String {
    let s: Vec<&str> = (0..7)
        .filter(|b| v >> b & 1 != 0)
        .map(|b| via::NAMES[b])
        .collect();
    if s.is_empty() {
        "-".into()
    } else {
        s.join("+")
    }
}

/// A tile on screen: BG `layer` 0 to 3 or OBJ (4), its bytes at `vram`,
/// its palette bank (0 for 8 bpp), and for an OBJ its size in tiles and
/// which of its tiles this is (row by row).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shown {
    pub layer: u8,
    pub bpp8: bool,
    pub vram: u32,
    pub bank: u8,
    pub size: (u8, u8),
    pub part: u16,
}

fn layer_name(l: u8) -> String {
    if l < 4 {
        format!("bg{l}")
    } else {
        "obj".into()
    }
}

/// OBJ sizes in tiles, by shape and size.
const OBJ_SIZES: [[(u8, u8); 4]; 3] = [
    [(1, 1), (2, 2), (4, 4), (8, 8)],
    [(2, 1), (4, 1), (4, 2), (8, 4)],
    [(1, 2), (1, 4), (2, 4), (4, 8)],
];

/// Every BG and OBJ tile the PPU's state shows now: nothing in a forced
/// blank or a bitmap mode's BGs.
pub fn shown<P: Platform>(m: &Machine<P>) -> Vec<Shown> {
    let io = |o: u32| m.bus.io.raw16(o);
    let vram16 = |a: usize| u16::from_le_bytes([m.bus.video.vram[a], m.bus.video.vram[a + 1]]);
    let dispcnt = io(0);
    let mut out = Vec::new();
    if dispcnt & 0x80 != 0 {
        return out;
    }
    let mode = dispcnt & 7;
    for n in 0..4u8 {
        if dispcnt >> (8 + n) & 1 == 0 {
            continue;
        }
        let affine = match (mode, n) {
            (0, _) | (1, 0 | 1) => false,
            (1, 2) | (2, 2 | 3) => true,
            _ => continue,
        };
        let cnt = io(0x08 + 2 * n as u32);
        let chars = (cnt >> 2 & 3) as usize * 0x4000;
        let screen = (cnt >> 8 & 0x1F) as usize * 0x800;
        let size = cnt >> 14;
        if affine {
            let side = 16usize << size;
            let mut seen = [false; 256];
            for i in 0..side * side {
                let t = m.bus.video.vram[(screen + i) & 0xFFFF] as usize;
                if !seen[t] && chars + t * 64 < 0x1_0000 {
                    seen[t] = true;
                    out.push(Shown {
                        layer: n,
                        bpp8: true,
                        vram: 0x0600_0000 + (chars + t * 64) as u32,
                        bank: 0,
                        size: (1, 1),
                        part: 0,
                    });
                }
            }
            continue;
        }
        let bpp8 = cnt & 0x80 != 0;
        let (wt, ht) = (
            if size & 1 != 0 { 64 } else { 32 },
            if size & 2 != 0 { 64 } else { 32 },
        );
        let (hofs, vofs) = (
            (io(0x10 + 4 * n as u32) & 0x1FF) as usize,
            (io(0x12 + 4 * n as u32) & 0x1FF) as usize,
        );
        for ty in 0..21 {
            for tx in 0..31 {
                let (mx, my) = ((hofs / 8 + tx) % wt, (vofs / 8 + ty) % ht);
                let block = match size {
                    0 => 0,
                    1 => mx / 32,
                    2 => my / 32,
                    _ => my / 32 * 2 + mx / 32,
                };
                let e = vram16((screen + block * 0x800 + ((my % 32) * 32 + mx % 32) * 2) & 0xFFFF);
                let at = chars + (e & 0x3FF) as usize * if bpp8 { 64 } else { 32 };
                if at >= 0x1_0000 {
                    continue;
                }
                out.push(Shown {
                    layer: n,
                    bpp8,
                    vram: 0x0600_0000 + at as u32,
                    bank: if bpp8 { 0 } else { (e >> 12) as u8 },
                    size: (1, 1),
                    part: 0,
                });
            }
        }
    }
    if dispcnt & 0x1000 == 0 {
        return out;
    }
    let one_d = dispcnt & 0x40 != 0;
    // Bitmap modes keep their frames in the OBJ tiles below 512.
    let first = if mode >= 3 { 512 } else { 0 };
    for i in 0..128 {
        let a = |k: usize| {
            u16::from_le_bytes([
                m.bus.video.oam[8 * i + 2 * k],
                m.bus.video.oam[8 * i + 2 * k + 1],
            ])
        };
        let (a0, a1, a2) = (a(0), a(1), a(2));
        let affine = a0 & 0x100 != 0;
        let obj_mode = a0 >> 10 & 3;
        let shape = (a0 >> 14) as usize;
        if !affine && a0 & 0x200 != 0 || obj_mode >= 2 || shape == 3 {
            continue;
        }
        let (w, h) = OBJ_SIZES[shape][(a1 >> 14) as usize];
        let y = (a0 & 0xFF) as i32;
        let y = if y >= 160 { y - 256 } else { y };
        let x = ((a1 & 0x1FF) as i32) << 23 >> 23;
        if x + 8 * w as i32 <= 0 || x >= 240 || y + 8 * h as i32 <= 0 || y >= 160 {
            continue;
        }
        let bpp8 = a0 & 0x2000 != 0;
        let unit = if bpp8 { 2 } else { 1 };
        let t = (a2 & 0x3FF) as usize;
        for r in 0..h as usize {
            for c in 0..w as usize {
                let k = if one_d {
                    t + (r * w as usize + c) * unit
                } else {
                    t + r * 32 + c * unit
                } & 0x3FF;
                if k < first {
                    continue;
                }
                out.push(Shown {
                    layer: 4,
                    bpp8,
                    vram: 0x0601_0000 + 32 * k as u32,
                    bank: if bpp8 { 0 } else { (a2 >> 12) as u8 },
                    size: (w, h),
                    part: (r * w as usize + c) as u16,
                });
            }
        }
    }
    out
}

/// A distinct use: what was shown, where its tile and palette came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Use {
    layer: u8,
    bpp8: bool,
    size: (u8, u8),
    part: u16,
    tile: Src,
    bank: u8,
    pal: Option<Src>,
}

#[derive(Clone, Copy)]
struct Seen {
    first: u64,
    last: u64,
    frames: u64,
}

pub struct Recorder {
    dir: PathBuf,
    loads: std::io::BufWriter<std::fs::File>,
    dropped: u64,
    /// The last line logged for each (op, source, destination), so a load
    /// repeated unchanged (a buffer copied every frame) is logged once.
    logged: HashMap<(u32, u32, u32), String>,
    /// ROM ranges loaded as they are (copied, not decompressed): start to
    /// the longest length seen from it.
    ranges: BTreeMap<u32, u32>,
    uses: HashMap<Use, Seen>,
    /// The RAM buffer palette RAM equals by value, and when it was last
    /// searched for.
    pal_base: Option<u32>,
    searched: Option<u64>,
    last: Vec<(Shown, Use)>,
    frame: u64,
    oam: std::io::BufWriter<std::fs::File>,
    /// Each OAM entry's last line in `oam.tsv`, empty while hidden.
    oam_last: Vec<String>,
}

impl Recorder {
    pub fn new(dir: &Path) -> Result<Recorder, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let p = dir.join("loads.tsv");
        let f = std::fs::File::create(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let mut loads = std::io::BufWriter::new(f);
        let _ = writeln!(
            loads,
            "# each run: rom address/unpacked offset/bytes/steps\nframe\top\tsrc\tsrc_len\tdst\tdst_len\truns"
        );
        let p = dir.join("oam.tsv");
        let f = std::fs::File::create(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let mut oam = std::io::BufWriter::new(f);
        let _ = writeln!(
            oam,
            "# a line each time an OAM entry changes; `hidden` when it stops showing\nframe\tindex\tx\ty\tsize\tbpp\tmapping\ttile\tbank\tflip\ttile_src\ttile_via\ttile_off\tblob\tblob_off\tpal_src\tpal_via\tpal_off"
        );
        Ok(Recorder {
            dir: dir.into(),
            oam,
            oam_last: vec![String::new(); 128],
            loads,
            dropped: 0,
            logged: HashMap::new(),
            ranges: BTreeMap::new(),
            uses: HashMap::new(),
            pal_base: None,
            searched: None,
            last: Vec::new(),
            frame: 0,
        })
    }

    fn log(&mut self, frame: u64, l: &Load) {
        let mut runs = String::new();
        for r in l.runs {
            let s = Src {
                rom: prov::origin(r.tag).map_or(0, |o| ROM + o),
                via: prov::steps(r.tag),
                off: r.offset,
            };
            if !runs.is_empty() {
                runs.push(',');
            }
            let _ = write!(
                runs,
                "{:#010x}/{}/{}/{}",
                s.rom,
                s.off,
                r.len,
                via_name(s.via)
            );
            if !s.packed() {
                self.range(s.rom, r.len);
            }
        }
        if l.truncated {
            runs.push_str(",…");
        }
        if runs.is_empty() {
            runs.push('-');
        }
        let decompressed = matches!(
            l.op,
            op::LZ77 | op::RL | op::HUFF | op::BIT_UNPACK | op::DIFF
        );
        if l.runs.is_empty() && !decompressed {
            self.range(l.src, l.src_len);
        }
        let line = format!(
            "{}\t{:#010x}\t{}\t{:#010x}\t{}\t{runs}",
            op::NAMES[l.op as usize],
            l.src,
            l.src_len,
            l.dst,
            l.dst_len
        );
        let last = self.logged.entry((l.op, l.src, l.dst)).or_default();
        if *last != line {
            let _ = writeln!(self.loads, "{frame}\t{line}");
            *last = line;
        }
    }

    fn range(&mut self, start: u32, len: u32) {
        let e = self.ranges.entry(start).or_default();
        *e = (*e).max(len);
    }

    /// The loaded range a copied ROM byte is in: (its start, the byte's
    /// offset in it). The nearest start before it whose range reaches it.
    fn blob_of(&self, s: &Src) -> Option<(u32, u32)> {
        if s.packed() {
            return Some((s.rom, s.off));
        }
        self.ranges
            .range(..=s.rom)
            .rev()
            .take(256)
            .find(|(st, len)| *st + *len > s.rom)
            .map(|(st, _)| (*st, s.rom - st))
    }

    /// Where palette bank `bank` of half `obj` came from: its own tags if
    /// copied, else the buffer it equals by value.
    fn palette<P: Platform>(&mut self, m: &Machine<P>, obj: bool, bank: u8) -> Option<Src> {
        let at = (obj as u32) * 0x200 + bank as u32 * 32;
        let first_copied = |base: u32| {
            (0..32)
                .map(|i| (i, Src::at(base + at + i)))
                .find(|(_, s)| s.copied())
                .map(|(i, s)| s.minus(i))
        };
        if let Some(s) = first_copied(0x0500_0000) {
            return Some(s);
        }
        let colours = &m.bus.video.palette[at as usize..at as usize + 32];
        if colours.iter().all(|&b| b == 0) {
            return None;
        }
        let matches = |base: u32| ram(m, base + at, 32) == Some(colours);
        if !self.pal_base.is_some_and(matches)
            && self.searched.is_none_or(|f| self.frame >= f + SEARCH_EVERY)
        {
            self.searched = Some(self.frame);
            self.pal_base = search_palette(m, at).or(self.pal_base);
        }
        let base = self.pal_base.filter(|&b| matches(b))?;
        first_copied(base)
    }

    /// The use of each tile shown now.
    fn uses<P: Platform>(&mut self, m: &Machine<P>) -> Vec<(Shown, Use)> {
        let mut pals: HashMap<(bool, u8), Option<Src>> = HashMap::new();
        shown(m)
            .into_iter()
            .map(|s| {
                let obj = s.layer == 4;
                let pal = *pals
                    .entry((obj, s.bank))
                    .or_insert_with(|| self.palette(m, obj, s.bank));
                let u = Use {
                    layer: s.layer,
                    bpp8: s.bpp8,
                    size: s.size,
                    part: s.part,
                    tile: Src::at(s.vram),
                    bank: s.bank,
                    pal,
                };
                (s, u)
            })
            .collect()
    }

    /// Each OAM entry's line now: `None` if it isn't shown.
    fn objs<P: Platform>(&mut self, m: &Machine<P>) -> Vec<Option<String>> {
        let dispcnt = m.bus.io.raw16(0);
        let mut out = vec![None; 128];
        if dispcnt & 0x80 != 0 || dispcnt & 0x1000 == 0 {
            return out;
        }
        let mapping = if dispcnt & 0x40 != 0 { "1d" } else { "2d" };
        let mut pals: HashMap<u8, Option<Src>> = HashMap::new();
        for (i, line) in out.iter_mut().enumerate() {
            let a = |k: usize| {
                u16::from_le_bytes([
                    m.bus.video.oam[8 * i + 2 * k],
                    m.bus.video.oam[8 * i + 2 * k + 1],
                ])
            };
            let (a0, a1, a2) = (a(0), a(1), a(2));
            let affine = a0 & 0x100 != 0;
            let shape = (a0 >> 14) as usize;
            if !affine && a0 & 0x200 != 0 || a0 >> 10 & 3 >= 2 || shape == 3 {
                continue;
            }
            let (w, h) = OBJ_SIZES[shape][(a1 >> 14) as usize];
            let y = (a0 & 0xFF) as i32;
            let y = if y >= 160 { y - 256 } else { y };
            let x = ((a1 & 0x1FF) as i32) << 23 >> 23;
            if x + 8 * w as i32 <= 0 || x >= 240 || y + 8 * h as i32 <= 0 || y >= 160 {
                continue;
            }
            let bpp8 = a0 & 0x2000 != 0;
            let bank = if bpp8 { 0 } else { (a2 >> 12) as u8 };
            let tile = a2 & 0x3FF;
            let flip = if affine {
                "affine"
            } else {
                ["-", "h", "v", "hv"][(a1 >> 12 & 3) as usize]
            };
            let t = Src::at(0x0601_0000 + 32 * tile as u32);
            let pal = *pals
                .entry(bank)
                .or_insert_with(|| self.palette(m, true, bank));
            let src = |s: &Src| {
                if s.rom == 0 {
                    format!("-\t{}\t-", via_name(s.via))
                } else {
                    format!("{:#010x}\t{}\t{}", s.rom, via_name(s.via), s.off)
                }
            };
            let blob = match self.blob_of(&t) {
                Some((b, o)) if t.copied() => format!("{b:#010x}\t{o}"),
                _ => "-\t-".into(),
            };
            *line = Some(format!(
                "{x}\t{y}\t{w}x{h}\t{}\t{mapping}\t{tile}\t{bank}\t{flip}\t{}\t{blob}\t{}",
                if bpp8 { 8 } else { 4 },
                src(&t),
                pal.as_ref().map_or("-\t-\t-".into(), src),
            ));
        }
        out
    }

    /// Frame `frame` ended: takes its loads and records its uses.
    pub fn frame<P: Platform>(&mut self, m: &Machine<P>, frame: u64) {
        self.frame = frame;
        let dropped = prov::take_loads(|l| self.log(frame, l));
        self.dropped += dropped as u64;
        let now = self.uses(m);
        for (_, u) in &now {
            let e = self.uses.entry(*u).or_insert(Seen {
                first: frame,
                last: frame,
                frames: 0,
            });
            if e.frames == 0 || e.last != frame {
                e.frames += 1;
            }
            e.last = frame;
        }
        self.last = now;
        for (i, line) in self.objs(m).into_iter().enumerate() {
            let line = line.unwrap_or_default();
            if line != self.oam_last[i] {
                let text = if line.is_empty() { "hidden" } else { &line };
                let _ = writeln!(self.oam, "{frame}\t{i}\t{text}");
                self.oam_last[i] = line;
            }
        }
    }

    /// The run ended: writes the use log and the check's inputs.
    pub fn finish(mut self) -> Result<(), String> {
        self.write()
    }

    /// Writes the use log and the check's inputs as they stand, and
    /// flushes the load and OAM logs: at the end, or mid-run for an export
    /// (`--extract`'s hotkey). Recording goes on after.
    pub fn write(&mut self) -> Result<(), String> {
        let _ = self.loads.flush();
        let _ = self.oam.flush();
        let write = |name: &str, text: String| {
            let p = self.dir.join(name);
            std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))
        };
        let blob = |s: &Src| match self.blob_of(s) {
            Some((b, o)) if s.copied() => format!("{b:#010x}\t{o}"),
            _ => "-\t-".into(),
        };
        let src = |s: &Src| {
            if s.rom == 0 {
                format!("-\t{}\t-", via_name(s.via))
            } else {
                format!("{:#010x}\t{}\t{}", s.rom, via_name(s.via), s.off)
            }
        };
        let key = |u: &Use| {
            format!(
                "{}\t{}\t{}x{}\t{}\t{}\t{}\t{}\t{}",
                layer_name(u.layer),
                if u.bpp8 { 8 } else { 4 },
                u.size.0,
                u.size.1,
                u.part,
                src(&u.tile),
                blob(&u.tile),
                u.bank,
                u.pal.as_ref().map_or("-\t-\t-".into(), src),
            )
        };
        let head = "layer\tbpp\tsize\tpart\ttile_src\ttile_via\ttile_off\tblob\tblob_off\tbank\tpal_src\tpal_via\tpal_off";
        let mut uses: Vec<(&Use, &Seen)> = self.uses.iter().collect();
        uses.sort_by_key(|(u, s)| (s.first, **u));
        let mut text = format!(
            "# one line a distinct use; tile_src/pal_src: the ROM bytes the tile's first byte and the bank's first colour came from\n{head}\tfirst\tlast\tframes\n"
        );
        for (u, s) in uses {
            let _ = writeln!(text, "{}\t{}\t{}\t{}", key(u), s.first, s.last, s.frames);
        }
        write("uses.tsv", text)?;
        let mut text = format!("# every tile shown on frame {}\nvram\t{head}\n", self.frame);
        for (s, u) in &self.last {
            let _ = writeln!(text, "{:#010x}\t{}", s.vram, key(u));
        }
        write("shown.tsv", text)?;
        let mut text = format!(
            "# rom bytes that reached palette, vram or oam by frame {}: compressed blobs' first bytes and copied runs\nkind\tstart\tbytes\n",
            self.frame
        );
        let mut run: Option<(u32, u32)> = None;
        for i in 0..prov::REACHED_WORDS {
            let [copied, packed, _] = prov::reached(i);
            for b in 0..32 {
                let o = ROM + (i * 32 + b) as u32;
                if packed >> b & 1 != 0 {
                    let _ = writeln!(text, "packed\t{o:#010x}\t");
                }
                if copied >> b & 1 != 0 {
                    run = match run {
                        Some((s, n)) if s + n == o => Some((s, n + 1)),
                        Some((s, n)) => {
                            let _ = writeln!(text, "copied\t{s:#010x}\t{n}");
                            Some((o, 1))
                        }
                        None => Some((o, 1)),
                    };
                }
            }
        }
        if let Some((s, n)) = run {
            let _ = writeln!(text, "copied\t{s:#010x}\t{n}");
        }
        write("reached.tsv", text)?;
        if self.dropped > 0 {
            return Err(format!(
                "the load log dropped {} loads for want of room",
                self.dropped
            ));
        }
        Ok(())
    }
}

/// `len` bytes of EWRAM or IWRAM at `addr`, if they are all there.
fn ram<P: Platform>(m: &Machine<P>, addr: u32, len: usize) -> Option<&[u8]> {
    let (mem, base): (&[u8], u32) = match addr >> 24 {
        0x02 => (&m.bus.ewram, 0x0200_0000),
        0x03 => (&m.bus.iwram, 0x0300_0000),
        _ => return None,
    };
    let o = addr.checked_sub(base)? as usize;
    mem.get(o..o + len)
}

/// The base of a RAM buffer laid out like palette RAM whose bytes at
/// `at` equal its colours there and are copied from ROM, the one that
/// equals the most of palette RAM's non-zero banks.
fn search_palette<P: Platform>(m: &Machine<P>, at: u32) -> Option<u32> {
    let colours = &m.bus.video.palette[at as usize..at as usize + 32];
    let banks = |base: u32| {
        (0..32u32)
            .filter(|b| {
                let p = &m.bus.video.palette[*b as usize * 32..*b as usize * 32 + 32];
                p.iter().any(|&x| x != 0) && ram(m, base + b * 32, 32) == Some(p)
            })
            .count()
    };
    let mut best: Option<(u32, usize)> = None;
    for (mem, start) in [
        (&m.bus.ewram[..], 0x0200_0000u32),
        (&m.bus.iwram[..], 0x0300_0000),
    ] {
        for o in (at as usize..=mem.len() - 32).step_by(2) {
            if mem[o..o + 32] != *colours {
                continue;
            }
            let base = start + (o - at as usize) as u32;
            if !(0..32).any(|i| Src::at(base + at + i).copied()) {
                continue;
            }
            let n = banks(base);
            if n > best.map_or(0, |b| b.1) {
                best = Some((base, n));
            }
        }
    }
    best.map(|b| b.0)
}
