extern crate std;

use crate::io::reg;
use crate::machine::Machine;
use crate::platform::Headless;
use crate::sound::{self, mix};
use crate::test_util::{IO, machine};

/// Frequency for a period of `n` · 512 cycles (`4096 / n` Hz).
const fn note(n: u16) -> u16 {
    2048 - 32 * n
}

/// Master on, both PSG volumes 7, `route` (`SOUNDCNT_L` bits 8–15), PSG at
/// 100%, at cycle 0.
fn psg_on(m: &mut Machine<Headless>, route: u16) {
    m.bus.write16(IO + reg::SOUNDCNT_X, 0x80);
    m.bus.write16(IO + 0x080, 0x77 | route);
    m.bus.write16(IO + reg::SOUNDCNT_H, 2);
}

/// Bring the sound up to cycle `t`.
fn run_to(m: &mut Machine<Headless>, t: u64) {
    m.bus.io.now = t;
    sound::run_timers(&mut m.bus);
}

#[test]
fn channel_1_plays_its_duty_on_the_sample_grid() {
    let mut m = machine();
    psg_on(&mut m, 1 << 8 | 1 << 12);
    // 50% duty, volume 15 held, a duty step every 4096 cycles (8 samples).
    m.bus.write16(IO + 0x062, 2 << 6 | 0xF000);
    m.bus.write16(IO + 0x064, note(8) | 0x8000);
    run_to(&mut m, 64 * 512 + 512);
    let psg = &m.bus.sound.psg;
    assert_eq!(psg.len, 64);
    // 15 · 8 · (1 + 7) >> (4 − 2) on both sides where the duty bit is set.
    let duty = [1, 0, 0, 0, 0, 1, 1, 1];
    for k in 0..64 {
        let v = duty[k / 8] * 240;
        assert_eq!(psg.out[k], (v, v), "sample {k}");
    }
    assert_eq!(m.bus.io.raw16(reg::SOUNDCNT_X) & 3, 1);
}

#[test]
fn routing_and_master_volume_per_side() {
    let mut m = machine();
    // Channel 2 to the right only; left volume 3, right 7.
    m.bus.write16(IO + reg::SOUNDCNT_X, 0x80);
    m.bus.write16(IO + 0x080, 0x37 | 1 << 9);
    m.bus.write16(IO + reg::SOUNDCNT_H, 1);
    m.bus.write16(IO + 0x068, 3 << 6 | 0xA000);
    m.bus.write16(IO + 0x06C, note(8) | 0x8000);
    run_to(&mut m, 1024);
    // 75% duty starts low: index 0 is 0, so sample 0 is silent; step 1
    // is 4096 cycles on.
    assert_eq!(m.bus.sound.psg.out[0], (0, 0));
    // Samples are taken every 1024 cycles, up to the one ending there.
    run_to(&mut m, 10 * 512);
    // 10 · 8 · (1 + 7) >> (4 − 1), right only.
    assert_eq!(m.bus.sound.psg.out[8], (0, 80));
}

#[test]
fn length_expiry_stops_the_channel() {
    let mut m = machine();
    psg_on(&mut m, 1 << 8);
    // Length 63: one step left. Enabling set the sequencer to step 7, so
    // the next step, at 32768, clocks length.
    m.bus.write16(IO + 0x062, 63 | 0xF000);
    m.bus.write16(IO + 0x064, note(8) | 0xC000);
    assert_eq!(m.bus.io.raw16(reg::SOUNDCNT_X) & 1, 1);
    run_to(&mut m, 32767);
    assert_eq!(m.bus.io.raw16(reg::SOUNDCNT_X) & 1, 1);
    run_to(&mut m, 32768);
    assert_eq!(m.bus.io.raw16(reg::SOUNDCNT_X) & 1, 0);
    assert!(!m.bus.sound.psg.ch[0].playing);
}

#[test]
fn envelope_steps_on_sequencer_step_7() {
    let mut m = machine();
    psg_on(&mut m, 1 << 8);
    // 12.5% duty is high at duty step 7 only; volume 2, falling a step
    // every sequencer step 7.
    m.bus.write16(IO + 0x062, 0x2100);
    m.bus.write16(IO + 0x064, note(8) | 0x8000);
    // The sample taken at 7 · 4096 ran the channel to its duty step 7.
    run_to(&mut m, 7 * 4096 + 1024);
    assert_eq!(m.bus.sound.psg.ch[0].sample, 2);
    // Steps at 32768 (0) ... 8 · 32768 (7): the volume drops to 1.
    let t = 8 * 32768 + 7 * 4096;
    run_to(&mut m, t + 1024);
    assert_eq!(m.bus.sound.psg.ch[0].sample, 1);
    // Routed right only: 1 · 8 · (1 + 7) >> 2.
    assert_eq!(m.bus.sound.psg.out[(t / 512) as usize], (0, 16));
}

#[test]
fn a_sweep_that_would_overflow_stops_channel_1_at_restart() {
    let mut m = machine();
    psg_on(&mut m, 1 << 8);
    // Up by 1500 >> 1: past 2047 at once.
    m.bus.write16(IO + 0x060, 1 << 4 | 1);
    m.bus.write16(IO + 0x062, 0xF000);
    m.bus.write16(IO + 0x064, 1500 | 0x8000);
    assert!(!m.bus.sound.psg.ch[0].playing);
    // Shift 3 fits: 1500 + 187 = 1687.
    m.bus.write16(IO + 0x060, 1 << 4 | 3);
    m.bus.write16(IO + 0x064, 1500 | 0x8000);
    assert!(m.bus.sound.psg.ch[0].playing);
}

#[test]
fn master_off_clears_the_psg_registers() {
    let mut m = machine();
    psg_on(&mut m, 1 << 8);
    m.bus.write16(IO + 0x062, 0xF080);
    m.bus.write16(IO + 0x064, note(8) | 0x8000);
    m.bus.write16(IO + reg::SOUNDCNT_H, 0x0306);
    m.bus.write16(IO + reg::SOUNDCNT_X, 0);
    for o in (0x060..0x082).step_by(2) {
        assert_eq!(m.bus.io.raw16(o), 0, "{o:#x}");
    }
    assert_eq!(m.bus.io.raw16(reg::SOUNDCNT_H), 0x0300);
    assert_eq!(m.bus.io.raw16(reg::SOUNDCNT_X), 0);
}

#[test]
fn the_psg_adds_to_the_fifos_in_the_mix() {
    let mut m = machine();
    m.bus.write16(IO + reg::SOUNDCNT_X, 0x80);
    m.bus.write16(IO + reg::SOUNDCNT_H, 0x0304);
    // FIFO A at 100% (4 · 10) plus the PSG's 240 left and 0 right, on the
    // 0x200 bias, scaled by 64.
    assert_eq!(mix(&m.bus.io, 10, 0, (240, 0)), (280 * 64, 40 * 64));
    // The DAC clips at 0x3FF.
    assert_eq!(mix(&m.bus.io, 127, 0, (720, 0)).0, 0x1FF * 64);
}

#[test]
fn channel_3_plays_the_selected_bank_and_writes_reach_the_other() {
    let mut m = machine();
    psg_on(&mut m, 1 << 10 | 1 << 14);
    // Bank 0 selected: the sawtooth 0-15, twice, goes to bank 1.
    let saw = [0x6745_2301, 0xEFCD_AB89, 0x6745_2301, 0xEFCD_AB89];
    for (i, w) in saw.into_iter().enumerate() {
        m.bus.write32(IO + reg::WAVE_RAM + 4 * i as u32, w);
    }
    // Bank 1 selected and on: this goes to bank 0, not played.
    m.bus.write16(IO + 0x070, 0xC0);
    for i in 0..4 {
        m.bus.write32(IO + reg::WAVE_RAM + 4 * i, 0xFFFF_FFFF);
    }
    // 100%, a step every 8 · 128 cycles, the first 4 · (6 + 256) after
    // the restart.
    m.bus.write16(IO + 0x072, 1 << 13);
    m.bus.write16(IO + 0x074, (2048 - 128) | 0x8000);
    assert_eq!(m.bus.io.raw16(reg::SOUNDCNT_X) & 4, 4);
    run_to(&mut m, 80 * 512 + 512);
    let psg = &m.bus.sound.psg;
    for (k, &o) in psg.out[..80].iter().enumerate() {
        let t = 512 * k as i64;
        let v = if t < 1048 {
            0
        } else {
            ((t - 1048) / 1024 % 16) as i16 * 16
        };
        assert_eq!(o, (v, v), "sample {k}");
    }
    // The DAC off stops it.
    m.bus.write16(IO + 0x070, 0x40);
    assert_eq!(m.bus.io.raw16(reg::SOUNDCNT_X) & 4, 0);
}

#[test]
fn channel_4_outputs_the_bits_its_lfsr_shifts_out() {
    let mut m = machine();
    psg_on(&mut m, 1 << 11 | 1 << 15);
    // Volume 15 held; 15-bit, a clock every 32 · 1 · 2^4 cycles.
    m.bus.write16(IO + 0x078, 0xF000);
    m.bus.write16(IO + 0x07C, 4 << 4 | 0x8000);
    assert_eq!(m.bus.io.raw16(reg::SOUNDCNT_X) & 8, 8);
    run_to(&mut m, 200 * 512 + 512);
    let psg = &m.bus.sound.psg;
    let (mut lfsr, mut seen) = (0x7FFFu32, [0; 2]);
    assert_eq!(psg.out[0], (0, 0));
    for (k, &o) in psg.out[..200].iter().enumerate().skip(1) {
        let lsb = lfsr & 1;
        lfsr = (lfsr >> 1) ^ (lsb * 0x6000);
        let v = lsb as i16 * 240;
        assert_eq!(o, (v, v), "sample {k}");
        seen[lsb as usize] += 1;
    }
    assert!(seen[0] > 0 && seen[1] > 0);
}

#[test]
fn without_a_fifo_timer_the_grid_makes_the_stream() {
    let mut m = machine();
    psg_on(&mut m, 1 << 8 | 1 << 12);
    m.bus.write16(IO + 0x062, 2 << 6 | 0xF000);
    m.bus.write16(IO + 0x064, note(8) | 0x8000);
    run_to(&mut m, 16 * 512 + 512);
    let s = &m.bus.sound;
    // No FIFO records, one stream sample per grid sample, at 512 cycles.
    assert_eq!((s.len, s.samples_len, s.psg.len), (0, 16, 16));
    assert_eq!((s.period, &s.spans[..s.spans_len]), (512, &[(0, 512)][..]));
    for k in 0..16 {
        assert_eq!(s.samples[k], mix(&m.bus.io, 0, 0, s.psg.out[k]), "{k}");
    }
    assert_eq!(s.samples[0], (240 * 64, 240 * 64));
    // With the master off there's no stream.
    let mut m = machine();
    run_to(&mut m, 16 * 512 + 512);
    assert_eq!(m.bus.sound.samples_len, 0);
}

#[test]
fn a_fifo_timer_takes_the_stream_over_from_the_grid() {
    let mut m = machine();
    psg_on(&mut m, 1 << 8 | 1 << 12);
    run_to(&mut m, 4 * 512 + 512);
    // FIFO A on timer 0, every 924 cycles.
    m.bus.write16(IO + reg::TM0CNT_L, (0x1_0000 - 924) as u16);
    m.bus
        .write16(IO + reg::TM0CNT_L + 2, crate::timer::ctl::ENABLE);
    m.bus.write16(IO + reg::SOUNDCNT_H, 0x0B06);
    run_to(&mut m, 4 * 512 + 512 + 3 * 924 + 100);
    let s = &m.bus.sound;
    // The grid's samples until SOUNDCNT_H's store, then one per record.
    assert_eq!(s.len, 3);
    let grid = s.samples_len - 3;
    assert_eq!(&s.spans[..s.spans_len], &[(0, 512), (grid as u16, 924)]);
    assert_eq!(s.period, 924);
}
