//! The PSG: square channels 1 (with sweep) and 2, wave channel 3, noise
//! channel 4, their frame sequencer, and the sample grid mGBA 0.10.5 mixes
//! them on.
//!
//! The model is mGBA's (`src/gb/audio.c` and `src/gba/audio.c`), ported
//! as it is, since sound is judged against mGBA (`docs/sound-design.md`,
//! "The PSG"):
//!
//! - A channel's duty step advances lazily: [`Psg::run`] moves it on by
//!   whole periods (`16·(2048 − f)` cycles) from where it last stopped,
//!   and only while the channel plays with a live envelope, or when its own
//!   registers are written. Its output is `duty bit · volume`, and it keeps
//!   its last output after it stops.
//! - Channel 3 steps every `8·(2048 − rate)` cycles from `6·4` cycles
//!   after each write to its `SOUND3CNT_X` while it plays, rotating the
//!   played bank of wave RAM (both in 64-sample mode) a nibble at a time;
//!   its output is the nibble rotated out, scaled by its volume. Channel 4
//!   clocks its LFSR every `32·r·2^s` cycles (`r` the ratio, ½ for 0) from
//!   its restart; its output is the last bit out times its volume. Both
//!   run only while they play, and keep their last output after they stop.
//! - The frame sequencer steps every 32768 cycles from reset (512 Hz):
//!   length on steps 0, 2, 4, 6, sweep on 2 and 6, envelope on 7.
//! - Samples lie on a grid of `0x200 >> resolution` cycles (`SOUNDBIAS`
//!   bits 14–15). Sample `k`, at `g`, is computed by the first sampling call
//!   at or after `g + interval`: every 1024 cycles, every sequencer step and
//!   every write to a PSG register, `SOUNDCNT_L`, `SOUNDCNT_X` or
//!   `SOUNDBIAS`, before the write lands. Its duty steps are run to `g`.

use crate::io::{Io, reg};

/// Samples kept between hand-offs; more are dropped and counted.
pub const PSG_CHUNK: usize = 4096;
/// Cycles between mGBA's sampling calls.
const SAMPLE_EVENT: i64 = 1024;
/// Cycles between frame sequencer steps.
const FRAME_STEP: i64 = 32768;

const DUTY: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1],
    [1, 0, 0, 0, 0, 0, 0, 1],
    [1, 0, 0, 0, 0, 1, 1, 1],
    [0, 1, 1, 1, 1, 1, 1, 0],
];

/// Envelope, duty and length register state (mGBA's `GBAudioEnvelope`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Envelope {
    length: u8,
    duty: u8,
    step_time: u8,
    direction: bool,
    initial_volume: u8,
    current_volume: i8,
    /// 0 live, 1 stuck at 15 (or constant), 2 silent.
    dead: u8,
    next_step: u8,
}

impl Envelope {
    fn write_duty(&mut self, v: u8) {
        self.length = v & 0x3F;
        self.duty = v >> 6;
    }

    /// NRx2. Whether the DAC stays on.
    fn write(&mut self, v: u8) -> bool {
        self.step_time = v & 7;
        self.direction = v & 8 != 0;
        self.initial_volume = v >> 4;
        self.update_dead();
        self.initial_volume != 0 || self.direction
    }

    fn reset(&mut self) -> bool {
        self.current_volume = self.initial_volume as i8;
        self.next_step = self.step_time;
        self.update_dead();
        self.initial_volume != 0 || self.direction
    }

    fn update_dead(&mut self) {
        if self.step_time == 0 {
            self.dead = if self.current_volume != 0 { 1 } else { 2 };
        } else if !self.direction && self.current_volume == 0 {
            self.dead = 2;
        } else if self.direction && self.current_volume == 0xF {
            self.dead = 1;
        } else if self.dead != 0 {
            self.next_step = self.step_time;
            self.dead = 0;
        }
    }

    fn step(&mut self) {
        if self.direction {
            self.current_volume += 1;
        } else {
            self.current_volume -= 1;
        }
        if self.current_volume >= 15 {
            self.current_volume = 15;
            self.dead = 1;
        } else if self.current_volume <= 0 {
            self.current_volume = 0;
            self.dead = 2;
        } else {
            self.next_step = self.step_time;
        }
    }
}

/// Channel 1's sweep (mGBA's `GBAudioSweep`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sweep {
    shift: u8,
    direction: bool,
    time: u8,
    step: u8,
    enable: bool,
    occurred: bool,
    real_frequency: i32,
}

impl Default for Sweep {
    fn default() -> Sweep {
        Sweep {
            shift: 0,
            direction: false,
            time: 8,
            step: 0,
            enable: false,
            occurred: false,
            real_frequency: 0,
        }
    }
}

impl Sweep {
    /// NR10. Whether the channel stays on.
    fn write(&mut self, v: u8) -> bool {
        self.shift = v & 7;
        let old = self.direction;
        self.direction = v & 8 != 0;
        let on = !(self.occurred && old && !self.direction);
        self.occurred = false;
        self.time = v >> 4 & 7;
        if self.time == 0 {
            self.time = 8;
        }
        on
    }

    fn reset(&mut self) {
        self.step = self.time;
        self.enable = self.step != 8 || self.shift != 0;
        self.occurred = false;
    }
}

/// One square channel (mGBA's `GBAudioSquareChannel` and its `playingChN`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Square {
    env: Envelope,
    sweep: Sweep,
    frequency: u16,
    stop: bool,
    length: u8,
    index: u8,
    last_update: i64,
    /// The current output, 0–15.
    pub sample: i8,
    pub playing: bool,
}

impl Default for Square {
    fn default() -> Square {
        Square {
            env: Envelope {
                dead: 2,
                ..Envelope::default()
            },
            sweep: Sweep::default(),
            frequency: 0,
            stop: false,
            length: 0,
            index: 0,
            last_update: 0,
            sample: 0,
            playing: false,
        }
    }
}

impl Square {
    fn update_sample(&mut self) {
        self.sample =
            DUTY[self.env.duty as usize][self.index as usize] as i8 * self.env.current_volume;
    }

    /// Advance the duty step to `t` (mGBA's `GBAudioRun` for one channel);
    /// `forced` for a write to this channel's own registers.
    fn run(&mut self, t: i64, forced: bool) {
        let diff = t - self.last_update;
        if !((self.playing && self.env.dead != 2) || diff > 0x4000_0000 || forced) {
            return;
        }
        let period = 16 * (2048 - self.frequency as i64);
        if diff >= period {
            let n = diff / period;
            self.index = ((self.index as i64 + n) & 7) as u8;
            self.last_update += n * period;
            self.update_sample();
        }
    }

    /// The output [`Square::run`] to `t` would leave, without moving the
    /// channel's clock.
    fn sample_at(&self, t: i64) -> i8 {
        let mut c = *self;
        c.run(t, false);
        c.sample
    }

    /// The sweep's next frequency, applied unless `initial` (mGBA's
    /// `_updateSweep`). Whether the channel stays on.
    fn update_sweep(&mut self, initial: bool) -> bool {
        if initial || self.sweep.time != 8 {
            let mut f = self.sweep.real_frequency;
            if self.sweep.direction {
                f -= f >> self.sweep.shift;
                if !initial && f >= 0 {
                    self.frequency = f as u16;
                    self.sweep.real_frequency = f;
                }
            } else {
                f += f >> self.sweep.shift;
                if f < 2048 {
                    if !initial && self.sweep.shift != 0 {
                        self.frequency = f as u16;
                        self.sweep.real_frequency = f;
                        if !self.update_sweep(true) {
                            return false;
                        }
                    }
                } else {
                    return false;
                }
            }
            self.sweep.occurred = true;
        }
        self.sweep.step = self.sweep.time;
        true
    }

    /// NRx1: duty and length.
    fn write_duty(&mut self, v: u8) {
        self.env.write_duty(v);
        self.length = 64 - self.env.length;
    }

    /// NRx2: envelope.
    fn write_envelope(&mut self, v: u8) {
        if !self.env.write(v) {
            self.playing = false;
        }
    }

    /// NRx3: frequency, low 8 bits.
    fn write_freq_lo(&mut self, v: u8) {
        self.frequency = self.frequency & 0x700 | v as u16;
    }

    /// NRx4: frequency high bits, length enable, restart. `frame` is the
    /// sequencer's last step; `sweep` for channel 1.
    fn write_control(&mut self, v: u8, frame: u8, sweep: bool) {
        self.frequency = self.frequency & 0xFF | (v as u16 & 7) << 8;
        let was_stop = self.stop;
        self.stop = v & 0x40 != 0;
        if !was_stop && self.stop && self.length != 0 && frame & 1 == 0 {
            self.length -= 1;
            if self.length == 0 {
                self.playing = false;
            }
        }
        if v & 0x80 != 0 {
            self.playing = self.env.reset();
            if sweep {
                self.sweep.real_frequency = self.frequency as i32;
                self.sweep.reset();
                if self.playing && self.sweep.shift != 0 {
                    self.playing = self.update_sweep(true);
                }
            }
            if self.length == 0 {
                self.length = 64;
                if self.stop && frame & 1 == 0 {
                    self.length -= 1;
                }
            }
            self.update_sample();
        }
    }

    fn step_length(&mut self) {
        if self.length != 0 && self.stop {
            self.length -= 1;
            if self.length == 0 {
                self.playing = false;
            }
        }
    }

    fn step_envelope(&mut self) {
        if self.playing && self.env.dead == 0 {
            self.env.next_step = self.env.next_step.wrapping_sub(1);
            if self.env.next_step == 0 {
                self.env.step();
                self.update_sample();
            }
        }
    }
}

/// Channel 3 (mGBA's `GBAudioWaveChannel`, GBA style).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Wave {
    /// 64-sample mode: both banks play.
    size: bool,
    /// The bank played; the other is the one `0x04000090` reaches.
    bank: bool,
    /// The DAC (`SOUND3CNT_L` bit 7).
    enable: bool,
    length: u16,
    /// `SOUND3CNT_H` bits 13–15: 0 mute, 1 100%, 2 50%, 3 25%, 4–7 75%.
    volume: u8,
    rate: u16,
    stop: bool,
    next_update: i64,
    /// Both banks, as words of eight samples, high nibble first.
    data: [u32; 8],
    /// The current output, 0–15 scaled by the volume.
    pub sample: i8,
    pub playing: bool,
}

impl Wave {
    /// Step the wave to `t` (mGBA's `GBAudioRun` for channel 3). As in
    /// mGBA, the volume scales the sample again on every call that steps,
    /// even one that rotates by a whole bank and so doesn't reload it.
    fn run(&mut self, t: i64) {
        if !self.playing {
            return;
        }
        let cycles = 8 * (2048 - self.rate as i64);
        let diff = t - self.next_update;
        if diff < 0 {
            return;
        }
        let n = diff / cycles + 1;
        let (start, end, mask) = if self.size {
            (7, 0, 0x3F)
        } else if self.bank {
            (7, 4, 0x1F)
        } else {
            (3, 0, 0x1F)
        };
        for _ in 0..(n & mask) {
            let mut carry = self.data[end] & 0xF0;
            for i in (end..=start).rev() {
                let w = self.data[i];
                let bits = w & 0xF0;
                self.data[i] = (w & 0x0F0F_0F0F) << 4 | (w & 0xF0F0_F000) >> 12 | carry << 20;
                carry = bits;
            }
            self.sample = (carry >> 4) as i8;
        }
        if self.volume > 3 {
            self.sample = self.sample.wrapping_add(self.sample.wrapping_shl(1));
        }
        self.sample >>= match self.volume {
            0 => 4,
            1 => 0,
            2 => 1,
            _ => 2,
        };
        self.next_update += n * cycles;
    }

    /// NR34: rate high bits, length enable, restart. `frame` is the
    /// sequencer's last step.
    fn write_control(&mut self, v: u8, frame: u8, now: i64) {
        self.rate = self.rate & 0xFF | (v as u16 & 7) << 8;
        let was_stop = self.stop;
        self.stop = v & 0x40 != 0;
        if !was_stop && self.stop && self.length != 0 && frame & 1 == 0 {
            self.length -= 1;
            if self.length == 0 {
                self.playing = false;
            }
        }
        if v & 0x80 != 0 {
            self.playing = self.enable;
            if self.length == 0 {
                self.length = 256;
                if self.stop && frame & 1 == 0 {
                    self.length -= 1;
                }
            }
        }
        // mGBA's "TODO: Where does this cycle delay come from?": every
        // write while it plays, restart or not, sets the next step.
        if self.playing {
            self.next_update = now + 4 * (6 + 2 * (2048 - self.rate as i64));
        }
    }

    fn step_length(&mut self) {
        if self.length != 0 && self.stop {
            self.length -= 1;
            if self.length == 0 {
                self.playing = false;
            }
        }
    }
}

/// Channel 4 (mGBA's `GBAudioNoiseChannel`, GBA style).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Noise {
    env: Envelope,
    ratio: u8,
    frequency: u8,
    /// 7-bit LFSR.
    power: bool,
    stop: bool,
    length: u8,
    lfsr: u32,
    last_event: i64,
    /// The current output, 0–15.
    pub sample: i8,
    pub playing: bool,
}

impl Default for Noise {
    fn default() -> Noise {
        Noise {
            env: Envelope {
                dead: 2,
                ..Envelope::default()
            },
            ratio: 0,
            frequency: 0,
            power: false,
            stop: false,
            length: 0,
            lfsr: 0,
            last_event: 0,
            sample: 0,
            playing: false,
        }
    }
}

impl Noise {
    /// Cycles between LFSR clocks.
    fn cycles(&self) -> i64 {
        let r = if self.ratio != 0 {
            2 * self.ratio as i64
        } else {
            1
        };
        32 * (r << self.frequency)
    }

    /// Clock the LFSR to `t` (mGBA's `GBAudioRun` for channel 4).
    fn run(&mut self, t: i64) {
        if !self.playing {
            return;
        }
        let cycles = self.cycles();
        let diff = t - self.last_event;
        if diff < cycles {
            return;
        }
        let coeff = if self.power { 0x60 } else { 0x6000 };
        let mut last = 0;
        let mut lsb = 0;
        while last + cycles <= diff {
            lsb = self.lfsr & 1;
            self.lfsr >>= 1;
            self.lfsr ^= lsb * coeff;
            last += cycles;
        }
        self.sample = lsb as i8 * self.env.current_volume;
        self.last_event += last;
    }

    /// NR41: length.
    fn write_length(&mut self, v: u8) {
        self.env.write_duty(v);
        self.length = 64 - self.env.length;
    }

    /// NR42: envelope.
    fn write_envelope(&mut self, v: u8) {
        if !self.env.write(v) {
            self.playing = false;
        }
    }

    /// NR43: clock and LFSR width.
    fn write_feedback(&mut self, v: u8) {
        self.ratio = v & 7;
        self.power = v & 8 != 0;
        self.frequency = v >> 4;
    }

    /// NR44: length enable, restart. `frame` is the sequencer's last step.
    fn write_control(&mut self, v: u8, frame: u8, now: i64) {
        let was_stop = self.stop;
        self.stop = v & 0x40 != 0;
        if !was_stop && self.stop && self.length != 0 && frame & 1 == 0 {
            self.length -= 1;
            if self.length == 0 {
                self.playing = false;
            }
        }
        if v & 0x80 != 0 {
            self.playing = self.env.reset();
            self.lfsr = if self.power { 0x7F } else { 0x7FFF };
            if self.length == 0 {
                self.length = 64;
                if self.stop && frame & 1 == 0 {
                    self.length -= 1;
                }
            }
            if self.playing {
                self.last_event = now;
            }
        }
    }

    fn step_length(&mut self) {
        if self.length != 0 && self.stop {
            self.length -= 1;
            if self.length == 0 {
                self.playing = false;
            }
        }
    }

    fn step_envelope(&mut self) {
        if self.playing && self.env.dead == 0 {
            self.env.next_step = self.env.next_step.wrapping_sub(1);
            if self.env.next_step == 0 {
                let s = self.sample;
                self.env.step();
                self.sample = (s > 0) as i8 * self.env.current_volume;
            }
        }
    }
}

/// The PSG and its sample stream.
#[derive(Clone, Debug)]
pub struct Psg {
    pub enable: bool,
    /// The sequencer's last step, 0–7.
    frame: u8,
    pub ch: [Square; 2],
    pub wave: Wave,
    pub noise: Noise,
    /// `SOUNDCNT_L`: master volumes (0–7) and routing bits (8–15).
    volume_right: u8,
    volume_left: u8,
    route: u8,
    /// The next sampling event (every 1024 cycles) and sequencer step
    /// (every 32768), both from reset unless a state moved them.
    next_sample: i64,
    next_frame: i64,
    /// mGBA's `lastSample` and `sampleIndex`: the next sample to compute
    /// is at `last_sample + index · interval`.
    last_sample: i64,
    index: u32,
    /// Samples since the last hand-off, after the `SOUNDCNT_H` PSG volume
    /// shift: what mGBA adds to the FIFOs' share.
    pub out: [(i16, i16); PSG_CHUNK],
    pub len: usize,
    pub dropped: u32,
}

impl Default for Psg {
    fn default() -> Psg {
        Psg {
            enable: false,
            frame: 0,
            ch: [Square::default(); 2],
            wave: Wave::default(),
            noise: Noise::default(),
            volume_right: 0,
            volume_left: 0,
            route: 0,
            next_sample: 0,
            next_frame: 0,
            last_sample: 0,
            index: 0,
            out: [(0, 0); PSG_CHUNK],
            len: 0,
            dropped: 0,
        }
    }
}

/// The resolution field of `SOUNDBIAS`.
fn resolution(io: &Io) -> u32 {
    (io.raw16(reg::SOUNDBIAS) >> 14) as u32
}

/// Cycles between grid samples: `0x200 >> resolution`.
pub fn interval(io: &Io) -> u32 {
    0x200 >> resolution(io)
}

impl Psg {
    /// mGBA's `GBAudioRun` with channel mask `channels`.
    fn run(&mut self, t: i64, channels: u8) {
        if !self.enable {
            return;
        }
        for (n, ch) in self.ch.iter_mut().enumerate() {
            if channels & (1 << n) != 0 {
                ch.run(t, channels == 1 << n);
            }
        }
        if channels & 4 != 0 {
            self.wave.run(t);
        }
        if channels & 8 != 0 {
            self.noise.run(t);
        }
    }

    /// The four channels' outputs.
    fn outputs(&self) -> [i8; 4] {
        [
            self.ch[0].sample,
            self.ch[1].sample,
            self.wave.sample,
            self.noise.sample,
        ]
    }

    /// The PSG's output from channel outputs `s`, before the `SOUNDCNT_H`
    /// shift (mGBA's `GBAudioSamplePSG`).
    fn raw(&self, s: [i8; 4]) -> (i16, i16) {
        let (mut l, mut r) = (0i32, 0i32);
        for (n, &s) in s.iter().enumerate() {
            if self.route & (0x10 << n) != 0 {
                l += s as i32;
            }
            if self.route & (1 << n) != 0 {
                r += s as i32;
            }
        }
        (
            (l * 8 * (1 + self.volume_left as i32)) as i16,
            (r * 8 * (1 + self.volume_right as i32)) as i16,
        )
    }

    /// The PSG's share of the mix from channel outputs `s`, with the
    /// `SOUNDCNT_H` volume shift.
    fn shifted(&self, io: &Io, s: [i8; 4]) -> (i16, i16) {
        let shift = 4 - (io.raw16(reg::SOUNDCNT_H) & 3);
        let (l, r) = self.raw(s);
        (l >> shift, r >> shift)
    }

    /// The PSG's share of the mix at `t` (not before the last sample
    /// computed), leaving the channels' clocks alone, so the grid's
    /// samples are as mGBA computes them.
    pub fn level_at(&self, io: &Io, t: i64) -> (i16, i16) {
        let s = if self.enable {
            let (mut w, mut n) = (self.wave, self.noise);
            w.run(t);
            n.run(t);
            [
                self.ch[0].sample_at(t),
                self.ch[1].sample_at(t),
                w.sample,
                n.sample,
            ]
        } else {
            self.outputs()
        };
        self.shifted(io, s)
    }

    /// Compute the samples due by `t` (mGBA's `GBAAudioSample`).
    fn sample(&mut self, io: &Io, t: i64) {
        let res = resolution(io);
        let interval = interval(io) as i64;
        let max = 2u32 << res;
        let mut ts = t - self.last_sample - self.index as i64 * interval;
        let mut k = self.index;
        while ts >= interval && k < max {
            self.run(self.last_sample + k as i64 * interval, 0xF);
            let s = self.shifted(io, self.outputs());
            if self.len < PSG_CHUNK {
                self.out[self.len] = s;
                self.len += 1;
            } else {
                self.dropped += 1;
            }
            k += 1;
            ts -= interval;
        }
        self.index = k;
        if k == max {
            self.last_sample += SAMPLE_EVENT;
            self.index = 0;
        }
    }

    /// Run the sampling events and sequencer steps due by `t`.
    pub fn advance(&mut self, io: &mut Io, t: i64) {
        loop {
            // A step and a sampling event at once: the step goes first
            // (mGBA's priorities); both sample first, so it makes no odds.
            let e = self.next_frame.min(self.next_sample);
            if e > t {
                break;
            }
            self.sample(io, e);
            if e == self.next_frame {
                self.frame_step(io, e);
                self.next_frame += FRAME_STEP;
            } else {
                self.next_sample += SAMPLE_EVENT;
            }
        }
    }

    /// One sequencer step at `t` (mGBA's `GBAudioUpdateFrame`).
    fn frame_step(&mut self, io: &mut Io, t: i64) {
        if !self.enable {
            return;
        }
        self.run(t, 0x7);
        self.frame = (self.frame + 1) & 7;
        match self.frame {
            2 | 6 | 0 | 4 => {
                if matches!(self.frame, 2 | 6) && self.ch[0].sweep.enable {
                    let c = &mut self.ch[0];
                    c.sweep.step = c.sweep.step.wrapping_sub(1);
                    if c.sweep.step == 0 && !c.update_sweep(false) {
                        c.playing = false;
                    }
                }
                for c in &mut self.ch {
                    c.step_length();
                }
                self.wave.step_length();
                self.noise.step_length();
            }
            7 => {
                for c in &mut self.ch {
                    c.step_envelope();
                }
                self.noise.step_envelope();
            }
            _ => {}
        }
        self.status(io);
    }

    /// Reflect the channels' state in `SOUNDCNT_X` bits 0–3.
    fn status(&self, io: &mut Io) {
        let x = io.raw16(reg::SOUNDCNT_X) & !0xF;
        io.set_raw16(
            reg::SOUNDCNT_X,
            x | self.ch[0].playing as u16
                | (self.ch[1].playing as u16) << 1
                | (self.wave.playing as u16) << 2
                | (self.noise.playing as u16) << 3,
        );
    }

    /// Before a store to I/O byte offsets `o..o + bytes` at `now`: run the
    /// events due, and sample now if the store reaches a register mGBA
    /// samples before writing.
    pub fn before_write(&mut self, io: &mut Io, o: u32, bytes: u32, now: i64) {
        self.advance(io, now);
        let samples = |b: u32| (0x060..0x082).contains(&b) || (0x084..0x08A).contains(&b);
        if (o..o + bytes).any(samples) {
            self.sample(io, now);
        }
    }

    /// After a store of `v` to I/O byte offsets `o..o + bytes` at `now`:
    /// each byte's register effect, low byte first. `old_bias` is
    /// `SOUNDBIAS` before the store.
    pub fn written(&mut self, io: &mut Io, o: u32, bytes: u32, v: u32, now: i64, old_bias: u16) {
        for i in 0..bytes {
            self.written_byte(io, o + i, (v >> (8 * i)) as u8, now);
        }
        // Wave RAM: mGBA writes a whole word, the halves it didn't get from
        // what was last stored there (its I/O copy, which is ours).
        for w in (o & !3..o + bytes).step_by(4) {
            if (reg::WAVE_RAM..reg::WAVE_RAM + 16).contains(&w) {
                let word = io.raw16(w) as u32 | (io.raw16(w + 2) as u32) << 16;
                self.write_wave_ram((w - reg::WAVE_RAM) / 4, word, now);
            }
        }
        if (o..o + bytes).any(|b| b == 0x088 || b == 0x089) {
            self.bias_written(io, old_bias, now);
        }
        self.status(io);
    }

    /// One byte `b` stored at `o`.
    fn written_byte(&mut self, io: &mut Io, o: u32, b: u8, now: i64) {
        match o {
            0x060 => {
                self.run(now, 1);
                if !self.ch[0].sweep.write(b) {
                    self.ch[0].playing = false;
                }
            }
            0x062 | 0x068 => {
                let n = (o == 0x068) as usize;
                self.run(now, 1 << n);
                self.ch[n].write_duty(b);
            }
            0x063 | 0x069 => {
                let n = (o == 0x069) as usize;
                self.run(now, 1 << n);
                self.ch[n].write_envelope(b);
            }
            0x064 | 0x06C => {
                let n = (o == 0x06C) as usize;
                self.run(now, 1 << n);
                self.ch[n].write_freq_lo(b);
            }
            0x065 | 0x06D => {
                let n = (o == 0x06D) as usize;
                self.run(now, 1 << n);
                self.ch[n].write_control(b, self.frame, n == 0);
            }
            0x070 => {
                // mGBA sets the bank and size before running the channel.
                self.wave.size = b & 0x20 != 0;
                self.wave.bank = b & 0x40 != 0;
                self.run(now, 4);
                self.wave.enable = b & 0x80 != 0;
                if !self.wave.enable {
                    self.wave.playing = false;
                }
            }
            0x072 => {
                self.run(now, 4);
                self.wave.length = 256 - b as u16;
            }
            0x073 => self.wave.volume = b >> 5,
            0x074 => {
                self.run(now, 4);
                self.wave.rate = self.wave.rate & 0x700 | b as u16;
            }
            0x075 => {
                self.run(now, 4);
                self.wave.write_control(b, self.frame, now);
            }
            0x078 => {
                self.run(now, 8);
                self.noise.write_length(b);
            }
            0x079 => {
                self.run(now, 8);
                self.noise.write_envelope(b);
            }
            0x07C => {
                self.run(now, 8);
                self.noise.write_feedback(b);
            }
            0x07D => {
                self.run(now, 8);
                self.noise.write_control(b, self.frame, now);
            }
            0x080 => {
                self.run(now, 0xF);
                self.volume_right = b & 7;
                self.volume_left = b >> 4 & 7;
            }
            0x081 => {
                self.run(now, 0xF);
                self.route = b;
            }
            0x084 => self.write_enable(io, b),
            _ => {}
        }
    }

    /// A word stored to wave RAM word `i` (0–3): it reaches the bank not
    /// played, or bank 1 with the PSG off (mGBA's `GBAAudioWriteWaveRAM`).
    /// Reads give what was last stored, not the bank (mGBA's give the bank).
    fn write_wave_ram(&mut self, i: u32, word: u32, now: i64) {
        let bank = !self.wave.bank || !self.enable;
        self.run(now, 4);
        self.wave.data[(i | (bank as u32) << 2) as usize] = word;
    }

    /// `SOUNDCNT_X` (mGBA's `GBAAudioWriteSOUNDCNT_X` and NR52).
    fn write_enable(&mut self, io: &mut Io, b: u8) {
        let was = self.enable;
        self.enable = b & 0x80 != 0;
        if !self.enable {
            for c in &mut self.ch {
                c.playing = false;
            }
            self.wave.playing = false;
            self.noise.playing = false;
            // NR10, NR12–NR14, NR22–NR24, NR30, NR32–NR34, NR42–NR44, NR50,
            // NR51, then NR11, NR21, NR31 and NR41 (not DMG), all 0. The PSG
            // is off, so none runs a channel.
            for (n, c) in self.ch.iter_mut().enumerate() {
                if n == 0 {
                    c.sweep.write(0);
                }
                c.write_envelope(0);
                c.write_freq_lo(0);
                c.write_control(0, self.frame, n == 0);
            }
            let w = &mut self.wave;
            w.enable = false;
            // NR32 with the DMG's volume 0 reloads the sample shifted out.
            w.sample = 0;
            w.rate = 0;
            w.write_control(0, self.frame, 0);
            self.noise.write_envelope(0);
            self.noise.write_feedback(0);
            self.noise.write_control(0, self.frame, 0);
            self.volume_left = 0;
            self.volume_right = 0;
            self.route = 0;
            for c in &mut self.ch {
                c.write_duty(0);
            }
            self.wave.length = 256;
            self.noise.write_length(0);
            // GBAAudioWriteSOUNDCNT_X.
            self.wave.size = false;
            self.wave.bank = false;
            self.wave.volume = 0;
            for r in (0x060..0x082).step_by(2) {
                io.set_raw16(r, 0);
            }
            let h = io.raw16(reg::SOUNDCNT_H);
            io.set_raw16(reg::SOUNDCNT_H, h & 0xFF00);
        } else if !was {
            self.frame = 7;
        }
    }

    /// `SOUNDBIAS` was written: a new resolution moves the sample index
    /// (mGBA's `GBAAudioWriteSOUNDBIAS`).
    fn bias_written(&mut self, io: &Io, old: u16, now: i64) {
        let res = resolution(io);
        if res != (old >> 14) as u32 {
            let ts = now - self.last_sample;
            let idx = ts >> (9 - res);
            self.index = if (0..(2i64 << 3)).contains(&idx) {
                idx as u32
            } else {
                0
            };
        }
    }

    /// Start a new chunk.
    pub fn clear(&mut self) {
        self.len = 0;
        self.dropped = 0;
    }

    /// Load mGBA's PSG state at `now`, as its `GBADeserialize` does: the
    /// I/O registers (already restored, raw) written in order, then the
    /// saved channel state over them. `u32_at` reads the state's word at a
    /// byte offset. mGBA's I/O doesn't keep the frequencies (write-only),
    /// so after a load both run at frequency 0 until the game writes them.
    pub fn load(&mut self, io: &mut Io, now: i64, u32_at: impl Fn(usize) -> u32) {
        *self = Psg::default();
        let rel = |o: usize| now + u32_at(o) as i32 as i64;
        // The write side, as GBAIODeserialize replays it: SOUNDCNT_X, then
        // each register up from 0x060.
        let x = io.raw16(reg::SOUNDCNT_X);
        let regs: [u16; 18] = core::array::from_fn(|i| io.raw16(0x060 + 2 * i as u32));
        self.write_enable(io, x as u8);
        for (i, &v) in regs.iter().enumerate() {
            let o = 0x060 + 2 * i as u32;
            if matches!(o, 0x066 | 0x06A | 0x06E | 0x076 | 0x07A | 0x07E) {
                continue;
            }
            for (j, b) in v.to_le_bytes().into_iter().enumerate() {
                self.written_byte(io, o + j as u32, b, now);
            }
        }
        for (i, &v) in regs.iter().enumerate() {
            io.set_raw16(0x060 + 2 * i as u32, v);
        }
        io.set_raw16(reg::SOUNDCNT_X, x);
        // GBAudioPSGDeserialize and GBAAudioDeserialize.
        self.ch[0].playing = x & 1 != 0;
        self.ch[1].playing = x & 2 != 0;
        self.enable = x & 0x80 != 0;
        self.next_frame = rel(0x134);
        let flags = u32_at(0x1DC);
        self.frame = (flags >> 22 & 7) as u8;
        for (n, (env, last)) in [(0x130, 0x140), (0x144, 0x150)].into_iter().enumerate() {
            let c = &mut self.ch[n];
            let e = u32_at(env);
            c.env.current_volume = (flags >> (8 * n) & 0xF) as i8;
            c.env.dead = (flags >> (8 * n + 4) & 3) as u8;
            c.length = (e & 0x7F) as u8;
            c.env.next_step = (e >> 7 & 7) as u8;
            c.index = (e >> 21 & 7) as u8;
            c.last_update = rel(last);
            if n == 0 {
                c.sweep.enable = flags >> 25 & 1 != 0;
                c.sweep.occurred = flags >> 26 & 1 != 0;
                c.sweep.time = (u32_at(0x13C) & 7) as u8;
                if c.sweep.time == 0 {
                    c.sweep.time = 8;
                }
                c.sweep.real_frequency = (e >> 10 & 0x7FF) as i32;
            }
        }
        // lastSample and channel 4's lastEvent are absolute, in mGBA's
        // 32-bit clock: its master cycles plus the CPU's.
        let clock = u32_at(0x00C).wrapping_add(u32_at(0x068));
        let abs = |o: usize| now - clock.wrapping_sub(u32_at(o)) as i32 as i64;
        self.wave.playing = x & 4 != 0;
        self.noise.playing = x & 8 != 0;
        for (i, d) in self.wave.data.iter_mut().enumerate() {
            *d = u32_at(0x154 + 4 * i);
        }
        self.wave.length = u32_at(0x174) as u16;
        self.wave.next_update = rel(0x178);
        let n = &mut self.noise;
        let e = u32_at(0x180);
        n.lfsr = u32_at(0x17C);
        n.env.current_volume = (flags >> 16 & 0xF) as i8;
        n.env.dead = (flags >> 20 & 3) as u8;
        n.length = (e & 0x7F) as u8;
        n.env.next_step = (e >> 7 & 7) as u8;
        n.last_event = abs(0x184);
        if n.env.dead < 2 && n.playing && u32_at(0x184) == 0 {
            // mGBA's back-compat guess from the next event.
            let c = n.cycles();
            n.last_event = now + (u32_at(0x188) as i64 & (c - 1)) - c;
        }
        // GBAAudioDeserialize then rewrites each channel's control
        // register, restart bit clear, from its I/O copy (which keeps no
        // frequency): channel 3's sets its next step.
        for o in [0x064, 0x06C, 0x074, 0x07C] {
            let v = io.raw16(o) & 0x7FFF;
            self.written_byte(io, o, v as u8, now);
            self.written_byte(io, o + 1, (v >> 8) as u8, now);
        }
        self.last_sample = abs(0x1E0);
        self.index = u32_at(0x1E4) & 0xF;
        self.next_sample = rel(0x1D4);
    }
}

crate::snap::snap!(Envelope {
    length,
    duty,
    step_time,
    direction,
    initial_volume,
    current_volume,
    dead,
    next_step
});
crate::snap::snap!(Sweep {
    shift,
    direction,
    time,
    step,
    enable,
    occurred,
    real_frequency
});
crate::snap::snap!(Square {
    env,
    sweep,
    frequency,
    stop,
    length,
    index,
    last_update,
    sample,
    playing
});
crate::snap::snap!(Wave {
    size,
    bank,
    enable,
    length,
    volume,
    rate,
    stop,
    next_update,
    data,
    sample,
    playing
});
crate::snap::snap!(Noise {
    env,
    ratio,
    frequency,
    power,
    stop,
    length,
    lfsr,
    last_event,
    sample,
    playing
});
crate::snap::snap!(Psg {
    enable,
    frame,
    ch,
    wave,
    noise,
    volume_right,
    volume_left,
    route,
    next_sample,
    next_frame,
    last_sample,
    index,
    out,
    len,
    dropped,
});

#[cfg(test)]
mod tests;
