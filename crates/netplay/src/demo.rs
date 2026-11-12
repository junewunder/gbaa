//! What the tests and `gbaa-pingsim` play: a toy console, a tiny ROM that
//! reads the keys, a scripted player, and the same game run straight,
//! without a network, to compare a session's hashes with.

use crate::gba::Gba;
use crate::session::HASH_EVERY;
use crate::{Console, Fnv, Snapshot};

/// A console whose seats are linked: each seat's state mixes in its
/// neighbour's every frame, so a wrong input or state anywhere spreads.
#[derive(Clone)]
pub struct Toy {
    pub st: Vec<u64>,
    pub build: u8,
}

impl Toy {
    /// The true start: seat `s` at `1000 + s`.
    pub fn new(seats: usize) -> Toy {
        Toy {
            st: (0..seats as u64).map(|s| 1000 + s).collect(),
            build: 1,
        }
    }

    /// What seat `seat`'s peer holds before the start: its own seat right,
    /// the others garbage.
    pub fn peer(seats: usize, seat: usize) -> Toy {
        let mut t = Toy::new(seats);
        for (s, v) in t.st.iter_mut().enumerate() {
            if s != seat {
                *v = 0xBAD0 + s as u64;
            }
        }
        t
    }
}

impl Console for Toy {
    type Input = u16;

    fn seats(&self) -> usize {
        self.st.len()
    }

    fn step(&mut self, inputs: &[u16]) -> Result<(), String> {
        let prev = self.st.clone();
        let n = prev.len();
        for (s, v) in self.st.iter_mut().enumerate() {
            let x = prev[s]
                ^ (inputs[s] as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ prev[(s + 1) % n].rotate_left(7);
            *v = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD) ^ x >> 29;
        }
        Ok(())
    }

    fn save(&mut self, seat: usize, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.st[seat].to_le_bytes());
    }

    fn load(&mut self, states: &[Vec<u8>]) -> Result<(), String> {
        for (v, s) in self.st.iter_mut().zip(states) {
            *v = u64::from_le_bytes(s[..].try_into().map_err(|_| "toy: bad state")?);
        }
        Ok(())
    }

    fn hash(&mut self) -> u64 {
        let mut h = Fnv::default();
        for v in &self.st {
            h.bytes(&v.to_le_bytes());
        }
        h.0
    }

    fn build_id(&self) -> [u8; 32] {
        [self.build; 32]
    }
}

impl Snapshot for Toy {
    type Snap = Vec<u64>;

    fn snapshot(&mut self, into: &mut Vec<u64>) {
        into.clone_from(&self.st);
    }

    fn restore(&mut self, from: &Vec<u64>) -> Result<(), String> {
        self.st.clone_from(from);
        Ok(())
    }

    fn snap_hash(&self, s: &Vec<u64>) -> u64 {
        let mut h = Fnv::default();
        for v in s {
            h.bytes(&v.to_le_bytes());
        }
        h.0
    }
}

/// A player's presses: changing every few frames, different per seat.
pub fn script(seat: usize, frame: u32) -> u16 {
    let x = (seat as u32 * 7919 + frame / 5).wrapping_mul(2_654_435_761);
    (x >> 16) as u16 & 0x3FF
}

/// The same game without a network: frame `f` runs on what each player
/// held `delay` frames earlier. Its hash every [`HASH_EVERY`] frames and
/// after the last.
pub fn straight<C: Console<Input = u16>>(mut c: C, delay: u32, frames: u32) -> Vec<(u32, u64)> {
    let mut hs = Vec::new();
    for f in 0..=frames {
        if f.is_multiple_of(HASH_EVERY) || f == frames {
            hs.push((f, c.hash()));
        }
        if f == frames {
            break;
        }
        let inputs: Vec<u16> = (0..c.seats())
            .map(|s| if f < delay { 0 } else { script(s, f - delay) })
            .collect();
        c.step(&inputs).unwrap();
    }
    hs
}

/// A cartridge that folds `KEYINPUT` into a word of EWRAM as fast as it
/// can, so every frame's state depends on every frame's keys:
///
/// ```text
///     mov r0, #0x04000000 ; add r0, r0, #0x130   @ KEYINPUT
///     mov r1, #0x02000000
/// 1:  ldrh r2, [r0] ; ldr r3, [r1]
///     add r3, r2, r3, ror #7 ; str r3, [r1]
///     b 1b
/// ```
pub fn key_folder() -> &'static [u8] {
    let code: [u32; 8] = [
        0xE3A0_0301,
        0xE280_0E13,
        0xE3A0_1402,
        0xE1D0_20B0,
        0xE591_3000,
        0xE082_33E3,
        0xE581_3000,
        0xEAFF_FFFA,
    ];
    let bytes: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
    Box::leak(bytes.into_boxed_slice())
}

/// GBAs on `rom`, seat `s`'s save 64 bytes of `s + 1`: seat `seat`'s
/// own console, its own save given and the others' blank (every save with
/// `None`).
pub fn gba(rom: &'static [u8], seats: usize, seat: Option<usize>) -> Gba {
    let saves: Vec<Vec<u8>> = (0..seats).map(|s| vec![s as u8 + 1; 64]).collect();
    let pick: Vec<Option<&[u8]>> = (0..seats)
        .map(|s| (seat.is_none_or(|own| own == s)).then_some(&saves[s][..]))
        .collect();
    Gba::new(rom, &pick)
}

/// A tiny ARM assembler for the demo ROMs: words appended in order, each
/// branch to a label resolved by [`Asm::finish`].
struct Asm {
    words: Vec<u32>,
    labels: Vec<(&'static str, usize)>,
    /// (word, label): a branch whose offset is filled in at the end.
    fixups: Vec<(usize, &'static str)>,
}

/// Conditions.
const AL: u32 = 0xE;
const NE: u32 = 0x1;

/// An ARM data-processing immediate: 8 bits rotated right by an even
/// amount.
fn imm(v: u32) -> u32 {
    (0..16)
        .find_map(|r| {
            let x = v.rotate_left(2 * r);
            (x < 0x100).then_some(r << 8 | x)
        })
        .unwrap_or_else(|| panic!("{v:#x} isn't an ARM immediate"))
}

impl Asm {
    fn new() -> Asm {
        Asm {
            words: Vec::new(),
            labels: Vec::new(),
            fixups: Vec::new(),
        }
    }

    fn w(&mut self, w: u32) -> &mut Asm {
        self.words.push(w);
        self
    }

    fn label(&mut self, l: &'static str) -> &mut Asm {
        self.labels.push((l, self.words.len()));
        self
    }

    /// `op rd, rn, #v` for a data-processing opcode `op` (its bits 21-24,
    /// with S at bit 20 as `s`).
    fn dp(&mut self, op: u32, s: bool, rd: u32, rn: u32, v: u32) -> &mut Asm {
        self.w(AL << 28 | 1 << 25 | op << 21 | (s as u32) << 20 | rn << 16 | rd << 12 | imm(v))
    }

    fn mov(&mut self, rd: u32, v: u32) -> &mut Asm {
        self.dp(0xD, false, rd, 0, v)
    }

    fn orr(&mut self, rd: u32, rn: u32, v: u32) -> &mut Asm {
        self.dp(0xC, false, rd, rn, v)
    }

    fn add(&mut self, rd: u32, rn: u32, v: u32) -> &mut Asm {
        self.dp(0x4, false, rd, rn, v)
    }

    fn and(&mut self, rd: u32, rn: u32, v: u32) -> &mut Asm {
        self.dp(0x0, false, rd, rn, v)
    }

    fn subs(&mut self, rd: u32, rn: u32, v: u32) -> &mut Asm {
        self.dp(0x2, true, rd, rn, v)
    }

    fn cmp(&mut self, rn: u32, v: u32) -> &mut Asm {
        self.dp(0xA, true, 0, rn, v)
    }

    fn tst(&mut self, rn: u32, v: u32) -> &mut Asm {
        self.dp(0x8, true, 0, rn, v)
    }

    /// `cmp rn, rm`.
    fn cmp_r(&mut self, rn: u32, rm: u32) -> &mut Asm {
        self.w(AL << 28 | 0x0150_0000 | rn << 16 | rm)
    }

    /// `mov rd, rm`.
    fn mov_r(&mut self, rd: u32, rm: u32) -> &mut Asm {
        self.w(AL << 28 | 0x01A0_0000 | rd << 12 | rm)
    }

    /// `ldrh`/`strh rd, [rn, #off]` (`off` < 256).
    fn half(&mut self, load: bool, rd: u32, rn: u32, off: u32) -> &mut Asm {
        assert!(off < 0x100);
        self.w(AL << 28
            | 0x01C0_00B0
            | (load as u32) << 20
            | rn << 16
            | rd << 12
            | (off >> 4) << 8
            | off & 0xF)
    }

    /// `ldrh rd, [rn], #2`.
    fn ldrh_next(&mut self, rd: u32, rn: u32) -> &mut Asm {
        self.w(AL << 28 | 0x00D0_00B2 | rn << 16 | rd << 12)
    }

    /// `ldr`/`str` (`byte`: `ldrb`/`strb`) `rd, [rn, #off]`.
    fn word(&mut self, load: bool, byte: bool, rd: u32, rn: u32, off: u32) -> &mut Asm {
        assert!(off < 0x1000);
        self.w(AL << 28
            | 0x0580_0000
            | (byte as u32) << 22
            | (load as u32) << 20
            | rn << 16
            | rd << 12
            | off)
    }

    /// `b`/`bl` (`link`) to `l` if `cond`.
    fn b(&mut self, cond: u32, link: bool, l: &'static str) -> &mut Asm {
        self.fixups.push((self.words.len(), l));
        self.w(cond << 28 | 0x0A00_0000 | (link as u32) << 24)
    }

    /// The words, branches resolved.
    fn finish(mut self) -> Vec<u32> {
        for &(at, l) in &self.fixups {
            let to = self
                .labels
                .iter()
                .find(|&&(n, _)| n == l)
                .unwrap_or_else(|| panic!("no label {l}"))
                .1;
            let off = (to as i32 - at as i32 - 2) as u32 & 0xFF_FFFF;
            self.words[at] |= off;
        }
        self.words
    }
}

/// `add r3, r2, r3, ror #7`: [`key_folder`]'s fold.
const FOLD: u32 = 0xE082_33E3;
const IO: u32 = 0x0400_0000;
const BX_LR: u32 = 0xE12F_FF1E;
/// The palette byte the sender sends (`63pp`).
pub const MB_PALETTE: u32 = 0xD1;
/// Where the program a single-pak sender sends writes [`MB_MARK`], in the
/// child's IWRAM, beside the word it folds the keys into (0x03000000).
pub const MB_MARK_AT: u32 = 0x0300_0004;
pub const MB_MARK: u16 = 0x4D42;
/// Where the sender keeps `MultiBoot`'s parameters, its result (+0x40)
/// and the word it folds its keys into afterwards (+0x44), in its IWRAM.
pub const MB_PARAMS: u32 = 0x0300_0000;

/// A single-pak sender: a homebrew cartridge that sends a program over the
/// cable to the GBA in seat 1 with no cartridge, the way a single-pak game
/// does (GBATEK's handshake by its own code, then the BIOS's `MultiBoot`,
/// [`runtime::bios::multiboot`]), then folds its keys into a word of IWRAM
/// as [`key_folder`] does. The program, the 0x100 bytes at 0xC0, boots on
/// the child at `0x020000C0`: it writes [`MB_MARK`] to [`MB_MARK_AT`] and
/// folds the child's keys into `0x03000000`, so each seat's state depends
/// on its own player's keys from then on.
///
/// ```text
/// 0x000  b start
/// 0x0C0  the program (padded to 0x100 bytes)
/// 0x1C0  start: RCNT = 0, SIOCNT = 0x2003 (multiplayer, 115,200 bps)
///        send 6200 until seat 1 answers 7202; send 6102
///        send the header's 60h halfwords; send 6200, 6202
///        send 63D1 until it answers 73cc; send 64hh, hh = 11h + cc + FFh + FFh
///        MultiBoot(params, 1): result to MB_PARAMS + 0x40; fold the keys
/// send:  SIOMLT_SEND = r1, start, wait while busy, r0 = SIOMULTI1, pause
/// ```
pub fn single_pak_sender() -> &'static [u8] {
    // The program the child boots.
    let mut p = Asm::new();
    p.mov(0, IO)
        .add(0, 0, 0x130)
        .mov(1, 0x0300_0000)
        .mov(4, MB_MARK as u32 & 0xFF00)
        .orr(4, 4, MB_MARK as u32 & 0xFF)
        .half(false, 4, 1, MB_MARK_AT - 0x0300_0000)
        .label("fold")
        .half(true, 2, 0, 0)
        .word(true, false, 3, 1, 0)
        .w(FOLD)
        .word(false, false, 3, 1, 0)
        .b(AL, false, "fold");
    let program = p.finish();

    // The sender, from 0x1C0. r11 = 0x04000100 (the serial registers'
    // page).
    let mut s = Asm::new();
    s.label("start")
        .mov(11, IO)
        .add(11, 11, 0x100)
        .mov(0, 0)
        .half(false, 0, 11, 0x34) // RCNT
        .mov(0, 0x2000)
        .orr(0, 0, 3)
        .half(false, 0, 11, 0x28) // SIOCNT
        .label("recognise")
        .mov(1, 0x6200)
        .b(AL, true, "send")
        .mov(2, 0x7200)
        .orr(2, 2, 2)
        .cmp_r(0, 2)
        .b(NE, false, "recognise")
        .mov(1, 0x6100)
        .orr(1, 1, 2)
        .b(AL, true, "send")
        .mov(4, 0x0800_0000)
        .mov(5, 0x60)
        .label("header")
        .ldrh_next(1, 4)
        .b(AL, true, "send")
        .subs(5, 5, 1)
        .b(NE, false, "header")
        .mov(1, 0x6200)
        .b(AL, true, "send")
        .mov(1, 0x6200)
        .orr(1, 1, 2)
        .b(AL, true, "send")
        .label("palette")
        .mov(1, 0x6300)
        .orr(1, 1, MB_PALETTE)
        .b(AL, true, "send")
        .and(2, 0, 0xFF00)
        .cmp(2, 0x7300)
        .b(NE, false, "palette")
        .and(6, 0, 0xFF) // cc
        .add(7, 6, 0x0F) // 11h + cc + FFh + FFh
        .and(7, 7, 0xFF)
        .orr(1, 7, 0x6400)
        .b(AL, true, "send")
        .mov(8, MB_PARAMS)
        .word(false, true, 7, 8, 0x14) // handshake_data
        .word(false, true, 6, 8, 0x19) // client_data[1]
        .mov(0, 0xFF)
        .word(false, true, 0, 8, 0x1A)
        .word(false, true, 0, 8, 0x1B)
        .mov(0, MB_PALETTE)
        .word(false, true, 0, 8, 0x1C) // palette_data
        .mov(0, 2)
        .word(false, true, 0, 8, 0x1E) // client_bit: seat 1
        .mov(0, 0x0800_0000)
        .orr(0, 0, 0xC0)
        .word(false, false, 0, 8, 0x20) // boot_srcp
        .add(0, 0, 0x100)
        .word(false, false, 0, 8, 0x24) // boot_endp
        .mov_r(0, 8)
        .mov(1, 1)
        .w(0xEF25_0000) // swi 0x25: MultiBoot
        .word(false, false, 0, 8, 0x40)
        .mov(0, IO)
        .add(0, 0, 0x130)
        .add(1, 8, 0x44)
        .label("fold")
        .half(true, 2, 0, 0)
        .word(true, false, 3, 1, 0)
        .w(FOLD)
        .word(false, false, 3, 1, 0)
        .b(AL, false, "fold")
        .label("send")
        .half(false, 1, 11, 0x2A) // SIOMLT_SEND
        .half(true, 2, 11, 0x28)
        .orr(2, 2, 0x80) // start
        .half(false, 2, 11, 0x28)
        .label("busy")
        .half(true, 2, 11, 0x28)
        .tst(2, 0x80)
        .b(NE, false, "busy")
        .half(true, 0, 11, 0x22) // SIOMULTI1
        .mov(2, 0x400)
        .label("pause")
        .subs(2, 2, 1)
        .b(NE, false, "pause")
        .w(BX_LR);
    let sender = s.finish();

    let mut rom = vec![0u32; 0x1C0 / 4];
    rom[0] = 0xEA00_0000 | (0x1C0 / 4 - 2);
    assert!(program.len() <= 0x100 / 4);
    rom[0xC0 / 4..0xC0 / 4 + program.len()].copy_from_slice(&program);
    rom.extend(sender);
    let bytes: Vec<u8> = rom.iter().flat_map(|w| w.to_le_bytes()).collect();
    Box::leak(bytes.into_boxed_slice())
}

/// Single-pak GBAs: seat 0 running [`single_pak_sender`] with no save,
/// seat 1 with no cartridge, the interpreter exact.
pub fn gba_single_pak() -> Gba {
    Gba::single_pak(single_pak_sender(), 2, None, runtime::Timing::Exact)
}
