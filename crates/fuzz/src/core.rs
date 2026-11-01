//! What every fuzzing worker shares, whatever the game
//! (assets/kirby-fuzz split it out of the Emerald worker): the machine
//! run frame by frame with each ROM instruction marked, the coverage
//! (instructions, ROM loads, and the provenance bitmaps of ROM bytes that
//! reached palette, VRAM or OAM), the corpus of kept states and how one is
//! picked, the shared corpus in `OUT/shared/`, random presses, and the
//! worker's logs (`events.tsv`, `kept.tsv`, `stats.tsv`). A game gives
//! two readers: its screen (Emerald's `gMain.callback2`) and its place
//! (Emerald's map, Kirby's room), logged with each sighting and kept state.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use runtime::Machine;
use runtime::io::key;
use runtime::platform::{Frame, Platform};
use runtime::prov;

use crate::rng::Rng;

pub const ROM: u32 = 0x0800_0000;
/// ROM halfwords, one bit each.
pub const PC_WORDS: usize = (1 << 25) / 2 / 64;
/// Frames between coverage checks, and a run's length.
pub const WINDOW: u64 = 30;
pub const RUN_MIN: u64 = 300;
pub const RUN_MAX: u64 = 900;
/// States kept at most.
pub const CORPUS_MAX: usize = 300;
/// A kept state that found at least this many things is shared (at most
/// [`SHARE_MAX`] a worker: a state is about 640 KB); workers trade states
/// and coverage this often (seconds), taking at most [`IMPORT_MAX`] states
/// each time.
pub const SHARE_MIN: u64 = 100;
pub const SHARE_MAX: u32 = 150;
pub const SHARE_SECS: u64 = 120;
pub const IMPORT_MAX: usize = 24;
/// One pick in this many (percent) is a story seed's, while there are any.
pub const STORY_PICK: u32 = 30;
/// A first sighting of graphics counts this many of an instruction's.
pub const GFX_WEIGHT: u64 = 8;

pub struct Pad(pub u16);

impl Platform for Pad {
    fn present(&mut self, _frame: &Frame) {}

    fn keys(&mut self) -> u16 {
        self.0
    }
}

/// What a worker has seen, and the log of each first sighting.
pub struct Cov {
    pub pcs: Vec<u64>,
    pub loads: HashSet<(u32, u32)>,
    pub packed: Vec<u32>,
    pub copied: Vec<u32>,
    pub tables: Vec<u32>,
    /// Things first seen since the last check.
    pub fresh: u64,
    pub counts: [u64; 5],
    pub events: std::io::BufWriter<std::fs::File>,
    pub start: Instant,
    pub phase: &'static str,
    pub cb2: u32,
    pub rom_words: usize,
}

impl Cov {
    /// The bitmaps, for other workers: ROM instructions, then blobs',
    /// copied and computed bytes.
    pub fn dump(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(8 * self.pcs.len() + 12 * self.packed.len());
        v.extend(self.pcs.iter().flat_map(|w| w.to_le_bytes()));
        v.extend(self.packed.iter().flat_map(|w| w.to_le_bytes()));
        v.extend(self.copied.iter().flat_map(|w| w.to_le_bytes()));
        v.extend(self.tables.iter().flat_map(|w| w.to_le_bytes()));
        v
    }

    /// Another worker's bitmaps, taken as seen (no events: it logged them).
    pub fn merge(&mut self, b: &[u8]) {
        if b.len() != 8 * self.pcs.len() + 12 * self.packed.len() {
            return;
        }
        let (p, rest) = b.split_at(8 * self.pcs.len());
        let (k, rest) = rest.split_at(4 * self.packed.len());
        let (c, t) = rest.split_at(4 * self.packed.len());
        for (w, x) in self.pcs.iter_mut().zip(p.as_chunks::<8>().0) {
            *w |= u64::from_le_bytes(*x);
        }
        for (w, x) in self.packed.iter_mut().zip(k.as_chunks::<4>().0) {
            *w |= u32::from_le_bytes(*x);
        }
        for (w, x) in self.copied.iter_mut().zip(c.as_chunks::<4>().0) {
            *w |= u32::from_le_bytes(*x);
        }
        for (w, x) in self.tables.iter_mut().zip(t.as_chunks::<4>().0) {
            *w |= u32::from_le_bytes(*x);
        }
    }

    pub fn event(&mut self, kind: usize, addr: u32, len: u32) {
        const KIND: [&str; 5] = ["pc", "load", "packed", "copied", "table"];
        self.fresh += if kind >= 2 { GFX_WEIGHT } else { 1 };
        self.counts[kind] += 1;
        let _ = writeln!(
            self.events,
            "{:.1}\t{}\t{}\t{addr:#010x}\t{len}\t{:#010x}",
            self.start.elapsed().as_secs_f64(),
            self.phase,
            KIND[kind],
            self.cb2
        );
    }

    /// New bits in the provenance spike's bitmaps of ROM bytes that
    /// reached palette, VRAM or OAM: blobs' first bytes, copied runs, and
    /// runs of the tables computed bytes were guessed to come from.
    pub fn check_reached(&mut self) {
        for i in 0..self.rom_words {
            let [copied, packed, tables] = prov::reached(i);
            let np = packed & !self.packed[i];
            let nc = copied & !self.copied[i];
            let nt = tables & !self.tables[i];
            if np | nc | nt == 0 {
                continue;
            }
            self.packed[i] |= np;
            self.copied[i] |= nc;
            self.tables[i] |= nt;
            for b in 0..32 {
                if np >> b & 1 != 0 {
                    self.event(2, ROM + (32 * i + b) as u32, 0);
                }
            }
            // Runs of new bytes within the word.
            for (kind, new) in [(3, nc), (4, nt)] {
                let mut b = 0;
                while b < 32 {
                    if new >> b & 1 == 0 {
                        b += 1;
                        continue;
                    }
                    let s = b;
                    while b < 32 && new >> b & 1 != 0 {
                        b += 1;
                    }
                    self.event(kind, ROM + (32 * i + s) as u32, (b - s) as u32);
                }
            }
        }
    }
}

/// A kept state: where it came from and how it has done.
pub struct Entry {
    pub id: u32,
    pub state: Vec<u8>,
    pub picks: u32,
    pub found: u64,
    /// A story seed: never dropped.
    pub story: bool,
}

/// Run frame by frame as `Machine::run_frame` does, marking each ROM
/// instruction run.
pub fn frame(m: &mut Machine<Pad>, keys: u16, cov: &mut Cov) -> bool {
    m.platform.0 = keys;
    m.bus.io.set_keys(keys);
    m.sched.frame_ended = false;
    while !m.sched.frame_ended && m.fault.is_none() {
        if m.sched.halted {
            m.idle();
            continue;
        }
        m.take_irq();
        let pc = m.cpu.pc;
        if (ROM..ROM + (1 << 25)).contains(&pc) {
            let i = ((pc - ROM) >> 1) as usize;
            let (w, b) = (i / 64, i % 64);
            if cov.pcs[w] >> b & 1 == 0 {
                cov.pcs[w] |= 1 << b;
                cov.event(0, pc & !1, 0);
            }
        }
        m.step();
    }
    let mut loads = Vec::new();
    prov::take_loads(|l| {
        if (0x08..=0x0D).contains(&(l.src >> 24)) {
            loads.push((l.op, l.src, l.src_len));
        }
    });
    for (op, src, len) in loads {
        if cov.loads.insert((op, src)) {
            cov.event(1, src, op << 24 | len);
        }
    }
    m.fault.is_none()
}

/// Where [`call`] returns to: never code, so reaching it ends the call.
const CALL_RETURN: u32 = 0x0F00_0000;
/// Instructions a [`call`] may run before it's given up.
const CALL_STEPS: u32 = 4_000_000;

/// Call the game's function at `addr` (bit 0 set for Thumb) with `args`
/// in r0 to r3, between frames, as its own code would: in System mode on
/// the main loop's stack, IRQs masked, returning to [`CALL_RETURN`]. The
/// CPU is put back as it was after, the halt too. Whether it returned
/// within [`CALL_STEPS`] instructions without a fault (the fault is
/// cleared either way). Generic: a decomp's (or discovery's) function
/// address is all a game needs to give.
pub fn call(m: &mut Machine<Pad>, addr: u32, args: &[u32]) -> bool {
    let cpu = m.cpu.clone();
    let halted = m.sched.halted;
    m.sched.halted = false;
    m.cpu.switch_mode(0x1F);
    for (i, &a) in args.iter().take(4).enumerate() {
        m.cpu.r[i] = a;
    }
    m.cpu.r[14] = CALL_RETURN;
    m.cpu.cpsr = (m.cpu.cpsr & !(1 << 5)) | 1 << 7 | (addr & 1) << 5;
    m.cpu.pc = addr & !1;
    let mut ok = false;
    for _ in 0..CALL_STEPS {
        if m.cpu.pc & !1 == CALL_RETURN {
            ok = true;
            break;
        }
        if m.fault.is_some() {
            break;
        }
        m.step();
    }
    m.fault = None;
    m.cpu = cpu;
    m.sched.halted = halted;
    ok
}

/// A worker's machine, coverage and corpus.
pub struct Core {
    pub m: Box<Machine<Pad>>,
    pub cov: Cov,
    pub rng: Rng,
    pub corpus: Vec<Entry>,
    pub next_id: u32,
    pub kept: std::io::BufWriter<std::fs::File>,
    pub stats: std::io::BufWriter<std::fs::File>,
    pub frames: u64,
    pub runs: u64,
    pub boots: u64,
    pub pokes: u64,
    pub faults: u64,
    pub sweeps: u64,
    /// This worker's place among `of`.
    pub index: u32,
    pub of: u32,
    /// `OUT/shared/`, and the states taken from it.
    pub shared: PathBuf,
    pub shared_out: u32,
    pub imported: HashSet<String>,
    /// The game's screen and place, read from its RAM.
    pub screen: fn(&Machine<Pad>) -> u32,
    pub place: fn(&Machine<Pad>) -> String,
    pub last_stat: u64,
    pub last_trade: u64,
}

impl Core {
    /// A worker writing into `out` (`OUT/wNN`), the `index`-th of `of`.
    pub fn new(
        rom: &'static [u8],
        out: &Path,
        seed: u64,
        index: u32,
        of: u32,
        screen: fn(&Machine<Pad>) -> u32,
        place: fn(&Machine<Pad>) -> String,
    ) -> Result<Core, String> {
        std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
        let create = |name: &str| {
            std::fs::File::create(out.join(name))
                .map(std::io::BufWriter::new)
                .map_err(|e| format!("{name}: {e}"))
        };
        let mut events = create("events.tsv")?;
        let _ = writeln!(events, "secs\tphase\tkind\taddr\tlen\tcallback2");
        let mut kept = create("kept.tsv")?;
        let _ = writeln!(kept, "secs\tid\tparent\twhy\tfound\tcallback2\tmap");
        let mut stats = create("stats.tsv")?;
        let _ = writeln!(
            stats,
            "secs\tframes\truns\tboots\tpokes\tfaults\tcorpus\tpcs\tloads\tpacked\tcopied_runs\tsweeps\ttable_runs"
        );
        let shared = out.parent().unwrap_or(Path::new(".")).join("shared");
        std::fs::create_dir_all(&shared).map_err(|e| format!("{}: {e}", shared.display()))?;
        Ok(Core {
            m: Box::new(Machine::new(rom, Pad(0))),
            cov: Cov {
                pcs: vec![0; PC_WORDS],
                loads: HashSet::new(),
                packed: vec![0; prov::REACHED_WORDS],
                copied: vec![0; prov::REACHED_WORDS],
                tables: vec![0; prov::REACHED_WORDS],
                fresh: 0,
                counts: [0; 5],
                events,
                start: Instant::now(),
                phase: "route",
                cb2: 0,
                rom_words: rom.len().div_ceil(32).min(prov::REACHED_WORDS),
            },
            rng: Rng::new(seed),
            corpus: Vec::new(),
            next_id: 1,
            kept,
            stats,
            frames: 0,
            runs: 0,
            boots: 0,
            pokes: 0,
            faults: 0,
            sweeps: 0,
            index,
            of: of.max(1),
            shared,
            shared_out: 0,
            imported: HashSet::new(),
            screen,
            place,
            last_stat: 0,
            last_trade: 0,
        })
    }

    pub fn snapshot(&mut self) -> Vec<u8> {
        let mut v = Vec::new();
        self.m.save_state(&mut |s| v.extend_from_slice(s));
        v
    }

    /// Keep the machine's state, from `parent`, for having found `found`.
    pub fn keep(&mut self, parent: u32, found: u64, why: &str) {
        let state = self.snapshot();
        let id = self.next_id;
        self.next_id += 1;
        let place = (self.place)(&self.m);
        let _ = writeln!(
            self.kept,
            "{:.1}\t{id}\t{parent}\t{why}\t{found}\t{:#010x}\t{place}",
            self.cov.start.elapsed().as_secs_f64(),
            self.cov.cb2
        );
        if self.corpus.len() >= CORPUS_MAX {
            // Drop the one that has done worst for its picks.
            let worst = (0..self.corpus.len())
                .filter(|&i| !self.corpus[i].story)
                .min_by_key(|&i| {
                    let e = &self.corpus[i];
                    (e.found + 1) * 1000 / (e.picks as u64 + 1)
                })
                .unwrap();
            self.corpus.swap_remove(worst);
        }
        if found >= SHARE_MIN && self.shared_out < SHARE_MAX {
            self.shared_out += 1;
            let name = format!("w{:02}-{id}-{found}.st", self.index);
            let tmp = self.shared.join(format!(".{name}"));
            if std::fs::write(&tmp, &state).is_ok() {
                let _ = std::fs::rename(&tmp, self.shared.join(&name));
            }
        }
        self.corpus.push(Entry {
            id,
            state,
            picks: 0,
            found,
            story: why == "story",
        });
    }

    /// Trade with the other workers: write this one's coverage, take
    /// theirs, and take states they shared.
    pub fn trade(&mut self) {
        let me = format!("w{:02}", self.index);
        let cov = self.shared.join(format!("cov-{me}.bin"));
        let tmp = self.shared.join(format!(".cov-{me}.bin"));
        if std::fs::write(&tmp, self.cov.dump()).is_ok() {
            let _ = std::fs::rename(&tmp, &cov);
        }
        let Ok(dir) = std::fs::read_dir(&self.shared) else {
            return;
        };
        let mut states = Vec::new();
        for e in dir.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name.contains(&me) {
                continue;
            }
            if name.starts_with("cov-") {
                if let Ok(b) = std::fs::read(e.path()) {
                    self.cov.merge(&b);
                }
            } else if name.ends_with(".st") && !self.imported.contains(&name) {
                states.push(name);
            }
        }
        // The ones that found most first.
        let found = |n: &str| -> u64 {
            n.trim_end_matches(".st")
                .rsplit('-')
                .next()
                .and_then(|f| f.parse().ok())
                .unwrap_or(0)
        };
        states.sort_by_key(|n| std::cmp::Reverse(found(n)));
        for name in states.into_iter().take(IMPORT_MAX) {
            if let Ok(state) = std::fs::read(self.shared.join(&name)) {
                let id = self.next_id;
                self.next_id += 1;
                let _ = writeln!(
                    self.kept,
                    "{:.1}\t{id}\t0\tshared {name}\t{}\t0x00000000\t?",
                    self.cov.start.elapsed().as_secs_f64(),
                    found(&name)
                );
                if self.corpus.len() >= CORPUS_MAX {
                    let worst = (0..self.corpus.len())
                        .filter(|&i| !self.corpus[i].story)
                        .min_by_key(|&i| {
                            let e = &self.corpus[i];
                            (e.found + 1) * 1000 / (e.picks as u64 + 1)
                        })
                        .unwrap();
                    self.corpus.swap_remove(worst);
                }
                self.corpus.push(Entry {
                    id,
                    state,
                    picks: 0,
                    found: found(&name),
                    story: false,
                });
            }
            self.imported.insert(name);
        }
    }

    /// Play `keys(f)` for frames `0..frames`; check coverage every
    /// [`WINDOW`] and keep the state after the frames `keep_at` (the first
    /// is frame 1) or, if `keep_at` is empty, where it found something. Returns
    /// what it found, or `None` on a fault.
    pub fn play(
        &mut self,
        frames: u64,
        parent: u32,
        why: &str,
        mut keys: impl FnMut(u64, &mut Rng) -> u16,
        keep_at: &[u64],
    ) -> Option<u64> {
        let mut total = 0;
        for f in 0..frames {
            let k = keys(f, &mut self.rng);
            self.cov.cb2 = (self.screen)(&self.m);
            let ok = frame(&mut self.m, k, &mut self.cov);
            self.frames += 1;
            if !ok {
                self.faults += 1;
                self.m.fault = None;
                return None;
            }
            if (f + 1) % WINDOW == 0 || f + 1 == frames {
                self.cov.check_reached();
                let fresh = std::mem::take(&mut self.cov.fresh);
                total += fresh;
                if (fresh > 0 && keep_at.is_empty()) || keep_at.contains(&(f + 1)) {
                    self.keep(parent, fresh, why);
                }
            } else if keep_at.contains(&(f + 1)) {
                self.keep(parent, 0, why);
            }
        }
        Some(total)
    }

    /// A tournament of four: the entry that has found most for its picks.
    /// Or, one time in [`STORY_PICK`], a story seed.
    pub fn pick(&mut self) -> usize {
        let seeds: Vec<usize> = (0..self.corpus.len())
            .filter(|&i| self.corpus[i].story)
            .collect();
        if !seeds.is_empty() && self.rng.chance(STORY_PICK) {
            return seeds[self.rng.below(seeds.len() as u32) as usize];
        }
        let n = self.corpus.len();
        (0..4)
            .map(|_| self.rng.below(n as u32) as usize)
            .max_by_key(|&i| {
                let e = &self.corpus[i];
                (e.found + 1) * 1000 / (e.picks as u64 + 1)
            })
            .unwrap()
    }

    /// A line of `stats.tsv`, and the logs flushed.
    pub fn stat(&mut self) {
        let c = self.cov.counts;
        let mut line = String::new();
        let _ = write!(
            line,
            "{:.0}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.cov.start.elapsed().as_secs_f64(),
            self.frames,
            self.runs,
            self.boots,
            self.pokes,
            self.faults,
            self.corpus.len()
        );
        let _ = writeln!(
            self.stats,
            "{line}\t{}\t{}\t{}\t{}\t{}\t{}",
            c[0], c[1], c[2], c[3], self.sweeps, c[4]
        );
        let _ = self.stats.flush();
        let _ = self.cov.events.flush();
        let _ = self.kept.flush();
    }

    /// The stats line every 30 seconds and a trade every [`SHARE_SECS`],
    /// when due; called once an iteration of the fuzzing loop.
    pub fn tick(&mut self) {
        let t = self.cov.start.elapsed().as_secs();
        if t >= self.last_stat + 30 {
            self.last_stat = t;
            self.stat();
        }
        if t >= self.last_trade + SHARE_SECS {
            self.last_trade = t;
            self.trade();
        }
    }

    /// Load a picked state, counting the pick; its id.
    pub fn load_pick(&mut self) -> u32 {
        let i = self.pick();
        self.corpus[i].picks += 1;
        let id = self.corpus[i].id;
        let state = std::mem::take(&mut self.corpus[i].state);
        self.m.load_state(&state).expect("our own state loads");
        self.corpus[i].state = state;
        id
    }

    /// A run of [`RUN_MIN`] to [`RUN_MAX`] frames of random presses (a
    /// walker or a masher) from the state just loaded, `parent`, credited
    /// with what it finds.
    pub fn random_run(&mut self, parent: u32, why: &str) {
        let mut p = Presses {
            walker: self.rng.chance(50),
            held: 0,
            left: 0,
            gap: 0,
        };
        let len = RUN_MIN + self.rng.below((RUN_MAX - RUN_MIN) as u32) as u64;
        self.runs += 1;
        if let Some(found) = self.play(len, parent, why, |_, r| p.keys(r), &[]) {
            // Credit the parent: its children found this.
            if let Some(e) = self.corpus.iter_mut().find(|e| e.id == parent) {
                e.found += found;
            }
        }
    }
}

/// Random presses: a walker (long holds of a direction, A now and then)
/// or a masher (short presses of everything, A most).
pub struct Presses {
    pub walker: bool,
    pub held: u16,
    pub left: u32,
    pub gap: u32,
}

impl Presses {
    pub fn keys(&mut self, r: &mut Rng) -> u16 {
        if self.left > 0 {
            self.left -= 1;
            return self.held;
        }
        if self.gap > 0 {
            self.gap -= 1;
            return 0;
        }
        const DIRS: [u16; 4] = [key::UP, key::DOWN, key::LEFT, key::RIGHT];
        let dir = DIRS[r.below(4) as usize];
        let roll = r.below(100);
        let (k, len) = if self.walker {
            match roll {
                0..60 => (dir, 8 + r.below(40)),
                60..85 => (key::A, 2 + r.below(4)),
                85..93 => (key::B, 2 + r.below(4)),
                93..97 => (key::START, 2 + r.below(4)),
                97..99 => (key::SELECT, 2 + r.below(4)),
                _ => ([key::L, key::R][r.below(2) as usize], 2 + r.below(4)),
            }
        } else {
            match roll {
                0..35 => (key::A, 2 + r.below(6)),
                35..50 => (key::B, 2 + r.below(6)),
                50..60 => (key::START, 2 + r.below(6)),
                60..95 => (dir, 2 + r.below(10)),
                _ => (
                    [key::SELECT, key::L, key::R][r.below(3) as usize],
                    2 + r.below(6),
                ),
            }
        };
        self.held = k;
        self.left = len - 1;
        self.gap = 2 + r.below(10);
        k
    }
}
