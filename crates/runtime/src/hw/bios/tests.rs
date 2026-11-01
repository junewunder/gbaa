extern crate std;

use super::*;
use crate::bios::decomp;
use crate::bus::Bus;
use std::boxed::Box;
use std::vec::Vec;

const EWRAM: u32 = 0x0200_0000;
const DST: u32 = 0x0200_8000;
const VRAM: u32 = 0x0600_0000;

/// A small xorshift: the same data every run.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
}

/// A valid LZ77 stream of `len` bytes: literals and back-references no
/// further back than what's written.
fn lz77_stream(rng: &mut Rng, len: usize) -> Vec<u8> {
    let mut s = std::vec![0x10, len as u8, (len >> 8) as u8, (len >> 16) as u8];
    let mut out = 0;
    while out < len {
        let flags_at = s.len();
        s.push(0);
        for bit in (0..8).rev() {
            if out >= len {
                break;
            }
            if out >= 2 && rng.next().is_multiple_of(3) {
                let dist = 1 + rng.next() as usize % out.min(0x1000);
                let n = 3 + rng.next() as usize % 16;
                let block = (n - 3) << 12 | (dist - 1);
                s[flags_at] |= 1 << bit;
                s.push((block >> 8) as u8);
                s.push(block as u8);
                out += n;
            } else {
                s.push(rng.next() as u8);
                out += 1;
            }
        }
    }
    s
}

fn rl_stream(rng: &mut Rng, len: usize) -> Vec<u8> {
    let mut s = std::vec![0x30, len as u8, (len >> 8) as u8, (len >> 16) as u8];
    let mut out = 0;
    while out < len {
        if rng.next().is_multiple_of(2) {
            let n = 3 + rng.next() as usize % 128;
            s.push(0x80 | (n - 3) as u8);
            s.push(rng.next() as u8);
            out += n;
        } else {
            let n = 1 + rng.next() as usize % 128;
            s.push((n - 1) as u8);
            for _ in 0..n {
                s.push(rng.next() as u8);
            }
            out += n;
        }
    }
    s
}

/// The play build's call on `src` (at EWRAM) into `dst`; the bytes from
/// `dst` on, `n` of them, and where its output ended.
fn play(f: impl Fn(&mut Bus, &mut [u32; 15]), src: &[u8], dst: u32, n: usize) -> (Vec<u8>, usize) {
    let mut bus = Box::new(Bus::new(&[]));
    for (i, &b) in src.iter().enumerate() {
        bus.write8(EWRAM + i as u32, b);
    }
    // A pattern under the output, so writes that don't happen show.
    for i in 0..n as u32 / 2 {
        bus.write16(dst + 2 * i, 0xA55A);
    }
    let mut r = [0; 15];
    r[0] = EWRAM;
    r[1] = dst;
    f(&mut bus, &mut r);
    let bytes = (0..n as u32).map(|i| bus.read8(dst + i)).collect();
    // VRAM reads as halfwords; read8 is fine for its bytes.
    (bytes, (r[1] - dst) as usize)
}

fn native(f: impl Fn(&[u8], &mut [u8]) -> usize, src: &[u8], n: usize) -> (Vec<u8>, usize) {
    let mut dst: Vec<u8> = (0..n)
        .map(|i| if i % 2 == 0 { 0x5A } else { 0xA5 })
        .collect();
    let end = f(src, &mut dst);
    (dst, end)
}

#[test]
fn lz77_decodes_as_the_play_build() {
    let mut rng = Rng(0x1234_5678);
    for len in [1, 2, 3, 7, 64, 255, 1000, 4097] {
        let src = lz77_stream(&mut rng, len);
        let room = (len + 33) & !1;
        for (vram, dst) in [(false, DST), (true, VRAM)] {
            let want = play(|b, r| _ = decomp::lz77(b, r, vram, 0), &src, dst, room);
            let got = native(|s, d| lz77(s, d, vram), &src, room);
            assert_eq!(got, want, "length {len}, vram {vram}");
        }
    }
}

#[test]
fn rl_decodes_as_the_play_build() {
    let mut rng = Rng(0x9E37_79B9);
    for len in [1, 2, 3, 5, 6, 7, 64, 255, 1001] {
        let src = rl_stream(&mut rng, len);
        let room = (len + 33) & !1;
        for (vram, dst) in [(false, DST), (true, VRAM)] {
            let want = play(|b, r| decomp::rl(b, r, vram), &src, dst, room);
            let got = native(|s, d| rl(s, d, vram), &src, room);
            assert_eq!(got, want, "length {len}, vram {vram}");
        }
    }
}

#[test]
fn arithmetic_is_the_play_builds() {
    assert_eq!(div(-7, 2), (-3, -1, 3));
    assert_eq!(div(5, 0), (1, 5, 1));
    assert_eq!(sqrt(1 << 20), 1 << 10);
    assert_eq!(arctan2(1, 0), 0);
    assert_eq!(arctan2(0, 1), 0x4000);
    assert_eq!(arctan2(-1, 0), 0x8000);
    let mut r = [0u32; 15];
    for (x, y) in [(3, 4), (-100, 7), (5, -9), (-2, -2), (1 << 20, 3)] {
        r[0] = x as u32;
        r[1] = y as u32;
        crate::bios::math::arc_tan2(&mut r);
        assert_eq!(arctan2(x, y) as u32, r[0], "({x}, {y})");
    }
    assert_eq!(obj_affine(0x100, 0x100, 0), [0x100, 0, 0, 0x100]);
    let (m, x, y) = bg_affine(10 << 8, 20 << 8, 120, 80, 0x100, 0x100, 0);
    assert_eq!(m, [0x100, 0, 0, 0x100]);
    assert_eq!((x, y), ((10 << 8) - 120 * 0x100, (20 << 8) - 80 * 0x100));
}
