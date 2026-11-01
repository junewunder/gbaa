# DMA sound design

Issue #52. How the runtime plays the GBA's two
direct-sound channels (FIFO A and B), fed by timer-clocked DMA, and hands
the result to `Platform::audio`. Decisions 11 (timing) and 13 (runtime)
apply; generated code doesn't change. The PSG's four channels are in "The
PSG" below (factory units `sound/psg-square` and `sound/psg-wave-noise`).

The target is agb 0.13's mixer, as an agb ROM uses it
(`out/music-probe/NOTES.md` in the main checkout has the sizing probe):

- `mixer.enable()` sets timer 0's reload to `65536 − 16777216/f`, so it
  overflows once a sample; timer 1 cascades on it with reload
  `65536 − buffer_size` and its IRQ on, so it overflows once a buffer.
- `SOUNDCNT_H = 0x9A0C`: A and B at 100%, A to the left speaker, B to the
  right, both clocked by timer 0, both FIFOs reset. `SOUNDCNT_X` bit 7
  (master enable). `SOUNDBIAS |= 0x4000` (resolution only; the bias stays
  0x200).
- The timer-1 IRQ handler (`MixerBuffer::swap`) writes DMA1/DMA2's control
  0, the next buffer's halves as their sources, `FIFO_A`/`FIFO_B` as their
  destinations, then control `0xB640`: fixed destination, repeat, words,
  start timing 3 (special), enable. It never writes `DMAxCNT_L`.

| ROM                | f (Hz) | cycles/sample | buffer | cycles/buffer |
|--------------------|--------|---------------|--------|---------------|
| 18157 Hz ROM         | 18157  | 924           | 304    | 280,896       |
| 10512 Hz ROM             | 10512  | 1596          | 176    | 280,896       |

Both buffers are exactly one frame (228 × 1232 = 280,896 cycles), which is
what `just audio-check` asserts.

## The model

### FIFOs

`runtime::sound::Fifo` is a 32-byte ring with a byte count. A store to
`FIFO_A`/`FIFO_B` (`0x0A0`–`0x0A7`) pushes its bytes, low byte first, in
`Bus::io_write` at the moment of the store: a word store pushes 4, a
halfword store 2, a byte store 1. The deferred `event::FIFO_A`/`FIFO_B`
bits go away. (mGBA pushes a whole word on a halfword store, combining it
with the register's other half; GBATEK says the FIFOs take 8, 16 or 32-bit
writes. Nothing we target writes them narrower than a word.)

A pop takes the oldest byte as the channel's new output sample.

### Who pops, and when

A timer overflow pops FIFO A if `SOUNDCNT_X` bit 7 is set, `SOUNDCNT_H`
bit 10 selects that timer (0 or 1), and bit 8 or 9 routes A to a speaker;
the same for B with bits 14, 12 and 13. That is mGBA's condition
(`GBATimerUpdate`, `timer.c`); timers 2 and 3 never pop.

Such a timer's overflows are **walked one at a time in clock order** by
`sound::run_timers(&mut Bus)`, which replaces `timer::run(&mut Io)` at
every call site (the scheduler's syncs, and the timer-register store). For
each overflow it runs all four timers up to that overflow's cycle with the
existing batch code (`timer::run_to`, which now returns which timers
overflowed), then pops the FIFOs that timer clocks, runs any refill DMA,
and records the sample. Cascades and IRQs come out of `run_to` for the same
step, so the order within one overflow is pop, refill, cascade, IRQ, with
nothing able to observe the difference between those. When no timer clocks
a FIFO, `run_timers` is exactly the old `timer::run`.

A cascading FIFO timer is walked by stepping at each overflow of the
clocked timer at the bottom of its cascade chain; `run_to` reports which
timers overflowed at each step.

A FIFO timer counts as listened to in `timer::next_event`, so the
scheduler syncs at the first tick at or after each sample: after every
instruction in the interpreter, at the next block exit in generated code.

`Bus::io_write` also calls `run_timers` before a store to the sound
registers (`0x080`–`0x0A7`) or to DMA1/DMA2's registers, so overflows due
before the store are walked before it lands: a buffer swap that re-latches
DMA1 sees the refills that came before it, in order.

(The BIOS's RegisterRamReset walks them too before it clears the sound
registers.)

### FIFO-timed DMA

DMA1 or DMA2 with start timing 3 (special) whose latched destination is
`FIFO_A` or `FIFO_B` is that FIFO's feeder. After a pop leaves the FIFO
with 16 bytes or fewer, the walk runs the feeder (the lower channel if both
point at one FIFO): exactly 4 words, read from the channel's source and
pushed straight into the FIFO, the source stepped by its address control,
the destination left alone. `DMAxCNT_L` and destination address control
are ignored (agb never writes the count, and `dma::count()` would read its
0 as 0x4000). Enabling the channel doesn't run it; the first pop after it
does. With the repeat bit clear the channel switches off after one refill
(GBATEK: sound DMA should have repeat set). The end-of-transfer IRQ is
raised if enabled, and the 10 cycles a 4-word transfer costs (`2n + 2`,
as for every other DMA here) are added to the clock.

This is GBATEK's rule ("16 bytes or less") as the issue fixes it. mGBA
differs in phase, not in the bytes: it pops whole words into a 4-byte
shift register and asks for DMA, before the pop, when fewer than 4 words
are queued. The popped stream is the same; when the refills happen
relative to the swaps is not, so mGBA's margin for the race below differs
from ours. Compared with `just mgba-audio-diff` (#63), the popped streams
are byte-identical: 600 frames of the 18157 Hz ROM and 1000 of the 10512 Hz ROM, each
from an mGBA state at frame 100, so GBATEK's rule stays. From reset both
mismatch only by a shift: mGBA's music starts 3 buffers later (912 records
on the 18157 Hz ROM, 528 on the 10512 Hz ROM) and its timer 7 (3) records later, and
then agrees to the end, because without memory wait states agb's boot runs
faster in ours (its frames lead mGBA's by 2 at frame 5 too). So the sound
oracle compares from a state.

Channel 3's special timing (video capture) and a special-timing DMA1/2 not
aimed at a FIFO still never run.

## Timing: can a FIFO run dry between ticks?

No, not from tick granularity. Generated code ticks only at block exits
(decision 11), and a block is at most 256 instructions. The largest
per-instruction estimate is `ldm` of 16 registers including pc, 20 cycles,
so a block is at most 5,120 cycles: 5.5 samples at 924 cycles, 3.2 at
1596. But a sync doesn't "catch up" a FIFO in bulk: it walks the overflows
since the last sync one by one, and each pop's refill runs right after that
pop, in the same step. The sequence of pops and pushes is the one the
hardware would produce whatever the tick spacing; only *when* the host
computes it is late, by at most one block.

That lateness has two visible effects, both bounded by one block:

1. **The DMA reads memory late.** A refill due at cycle T reads its 16
   bytes when the walk reaches it, after any stores the CPU made in the
   rest of that block. agb never writes the buffer that's playing (it
   mixes into a third buffer, `MixerBufferState::should_calculate`), so the
   bytes are the same.
2. **Stores land at the block's start.** Inside a block `Io::now` is the
   block's start time, so `run_timers` before a store walks only the
   overflows due before the block began. A re-latch store is seen *earlier*
   relative to the pops than on hardware, never later.

What can go wrong is the buffer swap. At 16 bytes a refill and one pop a
sample, the FIFO refills every 16 samples, and 304 and 176 are both
multiples of 16, so the refill phase is fixed relative to the swaps. From
the first enable (FIFO empty; the first pop pops nothing, refills to 16;
the second refills to 31; then every 16th pop refills to 32), refills fall
on the first pop after each swap: the swap's own pop leaves 17 bytes, the
next leaves 16 and refills. So **the IRQ handler must re-latch DMA1/2
within one sample period of the timer-1 overflow** (924 cycles on the repo
build, 1596 on the 10512 Hz ROM), or that refill reads 16 bytes past the end of the
old buffer. On hardware agb's handler makes it; in our runs the IRQ is
taken at the next instruction (interpreter) or the next block exit
(generated code), and the handler's store counts from its block's start.
The sound state measures the margin: from the timer-1 overflow's cycle to
each FIFO's re-latch store, with the samples popped and refills run in
between (`gbaa-desktop` prints it on stderr as `audio rate …, swap to
relatch max …`). Measured over 600 frames (1200 for the 18157 Hz ROM's
recompiled and interpreted runs):

| ROM (sample period)  | interpreter            | recompiled             |
|----------------------|------------------------|------------------------|
| 18157 Hz ROM (924)     | 235 cycles, 0 samples  | 247 cycles, 0 samples  |
| 10512 Hz ROM (1596)        | 230 cycles, 0 samples  | 224 cycles, 0 samples  |
| 18157 Hz ROM, IWRAM compiled (#54) | 235 cycles, 0 samples | 247 cycles, 0 samples |
| 10512 Hz ROM, IWRAM compiled (#54)     | 230 cycles, 0 samples | 224 cycles, 0 samples |

The samples and refills between swap and re-latch are the figures to
trust: in generated code the re-latch store's cycle is its block's start,
so the cycle count understates the real delay by up to a block. Neither
ever moved: no sample was popped, and no refill fell, between a swap and
its re-latch. The worst case is a
quarter of the 18157 Hz ROM's period: the handler re-latches well before
the next sample. The last two rows are after merging #54, with agb's IWRAM
code (the mixer and its IRQ dispatcher) compiled: 26.2% (18157 Hz ROM) and
19.0% (10512 Hz ROM) of guest time ran as compiled RAM code, and the margins
didn't move. This is measured, not
proven: a block exit up to 5,120 cycles after the overflow could exceed
the period, and `audio-check`'s hash comparison is what would catch it.

A mismatch traced to this race is the issue's stop condition, reported to
the overseer, not fixed around in codegen.

The first buffer after `enable()` is 320 bytes long in the popped stream,
not 304: the DMA starts with an empty FIFO, refills on the first two pops
instead of one, and so reads 16 bytes past the first buffer's end before
the next swap. After that every buffer contributes exactly its 304 (or
176) bytes in order. Those 16 bytes come in the first few frames, long
before the ground-truth window (frames 120–1200), and they come the same
way in both of our runs.

## The sample stream

**The popped bytes are the primary output.** Each overflow of a timer that
pops at least one FIFO makes one record: FIFO A's output sample and FIFO
B's, as `i8`. A FIFO that this timer doesn't clock (only when A and B use
different timers) keeps its last output in the record. agb clocks both
from timer 0, so a record is one pop of each, and the stream rate is the
timer's overflow rate.

The FIFO dump (`--fifo-dump F`) is the records in order, `A0 B0 A1 B1 …`,
one byte each (`i8` as two's complement), no header. `--audio-hash` prints
FNV-1a 64 over exactly those bytes.

**Mapping to `i16` stereo** is a separate step, done per record with the
registers as they are at the pop (the hardware mixes continuously, so a
register write applies from the next sample):

```
contribution(s, full) = full ? 4·s : 2·s        (100% / 50%, SOUNDCNT_H bit 2 / 3)
left  = Σ contribution of A if bit 9, of B if bit 13
right = Σ contribution of A if bit 8, of B if bit 12
bias  = SOUNDBIAS & 0x3FE                         (bits 1–9; default 0x200)
out   = (clamp(bias + side, 0, 0x3FF) − 0x200) · 64
```

That is GBATEK's 10-bit DAC: direct sound adds `±4·s` (100%) to the bias
and the sum clips to 0–0x3FF; centring on 0x200 and scaling by 64 maps
100%-volume `i8` exactly onto the `i16` range (`s · 256`). With
`SOUNDCNT_X` bit 7 clear both sides are `bias` alone. The PSG's share at
the record's cycle ("The PSG") is added to each side before the clip, as
mGBA adds it. `SOUNDBIAS`'s resolution bits (14–15) set the
hardware's PWM rate and don't change the values.

Note agb 0.13's routing: its mixer puts what it calls the right channel in
FIFO A, and `SOUNDCNT_H` sends A to the **left** speaker (bit 9; GBATEK and
mGBA agree on the bit). So a stereo WAV plays left/right swapped on
hardware. We reproduce the hardware; the ground truth below compares FIFO
bytes and is unaffected.

### The hand-off

`Bus` owns a `Sound` (FIFOs, the current output samples, and a fixed
buffer of records and their `i16` mappings, 2048 of each; `runtime` is
`no_std` without `alloc`). `Machine::present_frame` hands the buffer to the
platform once a frame, and `sync` hands it over early when it is half
full. If a pathological timer setting (prescaler 1, reload 0xFFFF) fills it
inside one block, the excess records are dropped and counted, never
blocking. `Platform::audio` becomes:

```rust
fn audio(&mut self, _chunk: &AudioChunk<'_>) {}

pub struct AudioChunk<'a> {
    pub fifo: &'a [(i8, i8)],       // the records: (A, B)
    pub samples: &'a [(i16, i16)],  // the same, mapped: (left, right)
                                    //   (or the PSG grid's, "The PSG")
    pub period: u32,                // cycles per record (924), 0 if none yet
    pub spans: &'a [(u16, u32)],    // (first sample, period) per run at one rate
    pub restarts: &'a [u16],        // record indices where a FIFO DMA was (re)enabled
    pub swaps: &'a [u16],           // record indices whose overflow raised a timer IRQ
                                    //   in the FIFO clock's cascade chain
    pub dropped: u32,               // records lost since the last chunk (buffer full)
}
```

`period` is the clocking timer's `65536 − reload` times its prescaler; the
rate is `16777216 / period` Hz (#53's playback resamples from it).
`swaps` marks agb's buffer swaps (timer 1's IRQ) and `restarts` its
re-latches; `audio-check` uses them to say where a mismatch falls
relative to a swap. At most 64 of each are kept per chunk (agb makes one
swap and two restarts a frame).

`gbaa-desktop --headless` gains:

- `--wav F.wav`: the `samples`, 16-bit stereo PCM at
  `round(16777216 / period)` Hz (18157 on the 18157 Hz ROM).
- `--fifo-dump F`: the raw records, as above.
- `--audio-hash`: prints `audio N records hash H` (FNV-1a 64 of the dump).
  Any audio flag also prints `audio dropped N`, which the checks require
  to be 0, and on stderr the swap margin (below).
- `--audio-log F`: one line per frame, `frame records`, then `r<i>` per
  restart and `s<i>` per swap (record indices from the start), for the
  checks below.

## Edge cases

From GBATEK ("Sound Channel A and B – DMA Sound") and mGBA
(`src/gba/audio.c`, `GBAAudioWriteFIFO`, `GBAAudioSampleFIFO`):

- **Pop from an empty FIFO:** the output sample becomes 0. mGBA pops whole
  words into a 4-byte shift register and, once that's drained and the FIFO
  is empty, outputs 0; GBATEK doesn't say. The pop still counts as a sample
  and still triggers the refill check (0 ≤ 16).
- **Push to a full FIFO:** the byte is dropped. mGBA's ring overwrites its
  oldest word, which loses everything but the newest word; GBATEK doesn't
  say. With the refill threshold at 16 and 16 bytes a refill, DMA alone can
  never overfill; only a CPU store can.
- **FIFO reset** (`SOUNDCNT_H` bit 11 for A, 15 for B, write-only, not
  stored): empties the FIFO. The current output sample stays until the next
  pop (mGBA likewise keeps its shift register).
- **Reads** of `FIFO_A`/`FIFO_B` return 0 (write-only, as now).
- **Master disable** (`SOUNDCNT_X` bit 7 clear): no pops, no records. The
  FIFOs keep their contents. It also zeroes the PSG registers
  (`0x060`–`0x081`) and `SOUNDCNT_H`'s low byte, as mGBA does, and stops
  channels 1 and 2.

## agb 0.13's mixer arithmetic, and the ground truth

Re-derived from `agb-0.13.0/src/sound/mixer/{mixer.s, sw_mixer.rs, hw.rs}`
and `agb_sound_converter-0.13.0/src/lib.rs`:

1. **Storage.** `include_wav!` reads the WAV with hound. For 8-bit PCM,
   hound's `i32` samples are `wav_u8 − 128` (8-bit WAV is unsigned), and
   `reduction = bits − 8 = 0`, so each stored byte is `wav_u8 − 128` as
   `i8`, interleaved in file order: `data[2i] = L_i`, `data[2i+1] = R_i`.
2. **Mix** (`agb_rs__mixer_add_stereo`, one stereo channel, working buffer
   zeroed, volume `Num<i16,4>` 1.0 = 16). `ldrsh` loads `data[2i] |
   data[2i+1] << 8`, sign-extended. The next three instructions build
   `r6 = sext(data[2i]) << 16 | (sext(data[2i+1]) & 0xFFF)`; `mla` adds
   `r6 · 16`. With one channel there are no carries between the halves, so
   word `i` of the working buffer is: upper halfword `16 · L_i`, lower
   halfword `16 · R_i`. (agb's comments call the lower half "L"; the bytes
   say it holds `data[2i+1]`, the WAV's right channel.)
3. **Collapse** (`agb_rs__mixer_collapse`). For each word, the lower
   halfword `asr 4` (via `lsl 16; asr 20`) gives `R_i`, the upper `asr 20`
   gives `L_i` exactly (the lower half is below 2^20, so it never rounds
   into the upper). Each is `127 + s`, clamped by `clamp_s8` to 0–255
   (only `s = −128`, giving −1, clamps, to 0), then `eor 0x80` and stored as
   a byte: `(127 + s) − 128 = s − 1`, or −128 for `s = −128`. So a byte is
   `max(s − 1, −128)`. The lower-half bytes (`R`) go to the buffer's first
   half (`str r12, [r0], #4`), the upper-half bytes (`L`) to its second half
   (`str r3, [r0, r4]`, `r4 = buffer_size`).
4. **Routing** (`MixerBuffer::swap`, `hw.rs`). The first half is DMA1's
   source, feeding `FIFO_A`; the second half DMA2's, feeding `FIFO_B`.
5. **Playback position.** A stereo channel advances `2 · buffer_size` bytes
   a buffer (`playback_speed = 2`), so consecutive buffers carry
   consecutive WAV frames with no gaps or repeats.

So, with `s = wav_u8 − 128` and the `i8` FIFO byte:

```
FIFO A byte = max(wav_R − 129, −128)
FIFO B byte = max(wav_L − 129, −128)
```

which is the issue's formula; my reading of agb agrees with it.

## The PSG

Channels 1 (square with sweep), 2 (square), 3 (wave) and 4 (noise) live
in `runtime/src/sound/psg.rs`, a port of mGBA 0.10.5's model
(`src/gb/audio.c`, `src/gba/audio.c`), since the oracle is mGBA (decision
"Sound is judged against mGBA from a state"). What mGBA does, and so we:

- **Duty steps advance lazily**: a channel's step moves on by whole
  periods (`16 · (2048 − f)` cycles) from where it last stopped, only while
  it plays with a live envelope, or when its own registers are written. A
  restart doesn't reset the step. A stopped channel keeps its last output
  (`duty bit · volume`), and the mix keeps adding it.
- **Channel 3** steps every `8 · (2048 − rate)` cycles, the first
  `4 · (6 + 2 · (2048 − rate))` after any store to `SOUND3CNT_X` while it
  plays (restart or not: each such store re-times it). Each step rotates
  the played bank of wave RAM (`SOUND3CNT_L` bit 6; both banks in
  64-sample mode, bit 5) by one nibble in place, and the nibble rotated
  out is the output, shifted by the volume (`SOUND3CNT_H` bits 13–15;
  75% is `·3 >> 2`). A restart doesn't reset the rotation. Wave RAM
  stores reach the bank not played (bank 1 with the master off), as
  whole words: a halfword or byte store takes the rest from what was last
  stored there. Reads give that last store (mGBA's give the bank).
- **Channel 4** clocks its LFSR every `32 · r · 2^s` cycles (`r` the
  ratio, ½ for 0; `s` the shift) from its restart, mGBA's form: `lsb =
  x & 1; x = x >> 1 ^ lsb · 0x6000` (`0x60` for 7 bits), from `0x7FFF`
  (`0x7F`). The output is the last `lsb` times the envelope's volume.
- Channels 3 and 4 run only while they play, and keep their last output
  after they stop, as the squares do.
- **The frame sequencer** steps every 32768 cycles from reset: length on
  steps 0, 2, 4, 6, sweep on 2 and 6, envelope on 7. Enabling the master
  sets it to step 7.
- **The sample grid**: samples lie `0x200 >> resolution` cycles apart
  (`SOUNDBIAS` bits 14–15; 512 by default). Sample `k`, at `g`, is
  computed by the first sampling call at or after `g + interval`: one
  every 1024 cycles, every sequencer step, and every store to a PSG
  register, `SOUNDCNT_L`, `SOUNDCNT_X` or `SOUNDBIAS` (before it lands; a
  `SOUNDCNT_H` store doesn't sample, so its PSG volume reaches samples
  already due). The channels are run to `g`.
- **A sample** is `Σ routed channel outputs · 8 · (1 + side volume)`
  (`SOUNDCNT_L`), shifted right by `4 − SOUNDCNT_H & 3` (25%, 50%, 100%).
  That's the PSG's share: `AudioChunk::psg` carries the grid's samples
  (`--psg-dump`, the oracle's stream), and each FIFO record adds the share
  at its own cycle (`Psg::level_at`, which leaves the channels' clocks
  alone so the grid still matches).

**States.** `runtime::state` loads mGBA's PSG state as mGBA's loader does:
it replays the I/O registers through the write handlers, then lays the
saved channel state over them, then rewrites each channel's control
register (restart clear) as mGBA's `GBAAudioDeserialize` does. mGBA's I/O
registers don't keep the frequencies or channel 3's rate (write-only), so
after a load they run at 0 until the game writes them, in mGBA and in
ours; and channel 3, if playing, steps next `4 · 4102` cycles after the
load. The loader also runs a
V-blank or H-blank DMA that mGBA had started but not run (`nextCount`
set: a frame ends just after the blank, before its DMA), and gives the
clock 2^32 cycles of headroom so times before the save stay positive.

**Where ours can still differ.** A DMA runs whole at its start cycle in
ours; in mGBA each unit takes its memory's time, so the ninth word of a
V-blank DMA lands tens of cycles later. A register that DMA writes just
before a sample is taken can then miss that sample in mGBA and not in
ours. The same goes for a CPU store, whose timing we don't model (no wait
states).

**Without a FIFO timer.** With the master on and no timer clocking a
FIFO (`clocks` is 0), there are no records, so the stream's samples
(`AudioChunk::samples`, so `--wav` and playback) come from the grid
instead: each grid sample, mixed with the FIFOs' last outputs as a record
is, `0x200 >> resolution` cycles apart. `fifo`, `--fifo-dump` and
`--audio-hash` stay FIFO records only, and `restarts`/`swaps` index
those. A chunk can hold both kinds (a game starting its FIFO timer
mid-frame), so `AudioChunk::spans` gives each run of samples at one rate,
`(first index, cycles per sample)`, and playback resamples each at its
own; `period` is the last sample's. A WAV has one rate, the last
sample's, so a run that switches plays its other part off-speed there.

**The check** is `just psg-square` (factory unit `sound/psg-square`):
`agb/psg-square` drives both channels from a table that DMA3 copies into
`0x060`–`0x083` each V-blank, so writes land at the same cycle in both
emulators, with FIFO A playing alongside. From mGBA's state after frame 1,
119 frames' FIFO records and PSG samples are byte-identical with mGBA's
(its `SOUND.md` lists the score).

Channels 3 and 4 have `just psg-wave-noise` (factory unit
`sound/psg-wave-noise`): `agb/psg-wave-noise` the same way, DMA3 writing
channel 4 and the mixer registers each V-blank. Channel 3 is written only
in the rows that change it, by a one-shot V-blank DMA2 the CPU arms the
frame before, since each `SOUND3CNT_X` store re-times it and a V-blank
late in a sample period (every eighth frame) would put that in a
different sample in mGBA. The CPU writes wave RAM after a V-blank, to the
bank not played only. 119 frames from mGBA's frame-1 state are
byte-identical (its `SOUND.md`).

## Checks

- `just check`, including `check-nostd`.
- Unit tests in `runtime/src/sound/tests.rs` (and `dma`/`timer` tests):
  FIFO push/pop/full/empty/reset; a FIFO-timed DMA refilling at the
  threshold with exactly 4 words and ignoring `DMAxCNT_L`; a 4-word CPU
  burst (`stm`-style four word stores) pushing 16 bytes; timer-driven
  popping with a cascade and its IRQ; the `i16` mapping at 100% and 50%
  with `SOUNDBIAS`. PSG tests in `runtime/src/sound/psg/tests.rs`: duty on
  the grid, routing and volumes, length, envelope, sweep overflow, master
  off, the mix, channel 3's banks and channel 4's LFSR.
- `just psg-square` and `just psg-wave-noise`: the PSG against mGBA
  (above).
- `just audio-check ROM FRAMES`: interpreted and recompiled headless runs;
  fails unless the `--audio-hash` values match; asserts 304 (repo) or 176
  (10512 Hz ROM) records every frame in steady state; on a mismatch prints the
  first differing record and its offset from the last restart. Passes on
  both agb sound ROMs at 600 frames.
- Nothing that passes now breaks: `just first-frame` (june's ROM,
  jsmolka's `arm`/`thumb`, the sprite example), `just gba-tests`,
  `just armwrestler`.

## Playback (#53)

`gbaa-desktop`'s window plays the stream through the host's default
output device with `cpal` (ALSA on Linux, so through PipeWire's ALSA
plugin; `alsa-lib-devel` is the only build dependency). The code is
`crates/desktop/src/audio.rs` and `audio/`.

**Speed first.** The release recompiled build of the 10512 Hz ROM runs 1200 frames
headless in 0.28 s, about 4,300 frames a second (90% of guest time is
halted), so real time (59.73 fps) leaves a 70× margin.

### Level: how loud, and the default gain

`mix()` maps the DAC's 10 bits to `i16` as `(sum - 0x200) * 64`, so a
full swing is full scale, and the window plays it as it is
(`/ 32768`, no gain). mGBA's `_applyBias` maps the same sum as
`(sum - bias) * masterVolume * 3 >> 4`, which at its default 100%
(`masterVolume` 0x100) is `* 48`: three quarters of ours, **2.50 dB
quieter**, its full swing at -2.5 dBFS. (Its Qt volume slider scales
`masterVolume`; 100% is the default.) `mix()` stays as it is, since the
oracle's audio hashes depend on it; any gain goes in playback.

Measured 2026-10-28 with `just audio-loudness ROM FROM TO [INPUT]`
(`mgba-dump --wav`, mGBA's mixed output before its resampler, against our
interpreter's `--wav`; `loudness`, BS.1770-4 integrated LUFS, RMS over both
channels, peak), over frames FROM to TO:

| Scene | Frames | Ours LUFS | Ours peak | mGBA LUFS | Ours − mGBA (RMS) |
|---|---|---|---|---|---|
| agb sound ROM | 300–1800 | -14.4 | -1.5 dBFS | -17.0 | +2.50 dB |
| 10512 Hz ROM (agb) | 300–1800 | -14.3 | -1.6 | -17.0 | +2.50 |
| Emerald intro (bike to Rayquaza) | 1000–3800 | -15.4 | -1.2 | -17.9 | +2.50 |
| Emerald title screen | 3800–5000 | -15.8 | -4.0 | -18.4 | +2.51 |
| Emerald Littleroot (overworld)¹ | 6300–7500 | -18.7 | -6.7 | -21.3 | +2.51 |
| Kirby & the Amazing Mirror, from reset | 300–2400 | -19.3 | -3.4 | -21.9 | +2.48 |
| Minish Cap, from reset | 300–2400 | -12.5 | -0.0 | -15.1 | +2.27² |
| Metroid Zero Mission, from reset | 300–2400 | **-6.5** | 0.0 | -9.1 | +2.46 |

¹ Input `start@300,a@900,a@1100-5380/20,right@6000+120` (the new-game
route, then out of the truck). No short route reaches an Emerald battle,
so none is measured. ² Mostly silence, where the two differ by
nothing; LUFS (gated) shows +2.53.

So the gap to mGBA is the formula's 2.50 dB everywhere (LUFS +2.5 to
+2.7, the K-weighting reading the two rates' images a little
differently; peaks +2.5, once +3.1). Next to desktop audio (streaming
services normalise to about -14 LUFS, broadcast to -23, games mostly
-16 to -23), ours is at the loud end or past it: median about -15 LUFS,
up to -6.5 with full-scale peaks (Metroid, Minish Cap swing the DAC
fully), and -19 at the quietest.

**Recommended default gain: -6 dB (× 0.5).** It puts the median near
-21 LUFS, the loudest measured (Metroid's title) at -12.5 and the
quietest at -25, inside or at the edges of the desktop range, and leaves
peaks 6 dB under full scale. It is 3.5 dB quieter than mGBA at 100%;
matching mGBA exactly would be -2.5 dB (× 0.75), which still leaves
Metroid at -9 LUFS, louder than anything else on a desktop.

**Applied** (volume/default-gain): `audio::DEFAULT_GAIN`, × 0.5 on each
sample as the `Player` turns it into `f32`, before the resampler (which
is linear, so before or after is the same); an exact halving in `f32`, it
loses nothing. Every window plays through a `Player`, so solo, linked and
netplay seats all get it; headless `--wav`, `--audio-hash` and the dumps
come from `mix()` and stay ungained. `--audio-pace 0 --played-wav F`
writes what the simulated device played, so the gained output can be
measured. On the same frames as the table above, ungained `--wav` (A)
against `--played-wav` (B), 2026-10-28:

| Scene | Frames | Before LUFS / peak | After LUFS / peak | B − A peak, RMS |
|---|---|---|---|---|
| agb sound ROM | 300–1800 | -14.4 / -1.5 | -20.5 / -7.5 | -6.03, -6.04 dB |
| Emerald intro | 1000–3800 | -15.4 / -1.2 | -21.9 / -7.2 | -6.03, -6.20 |
| Kirby & the Amazing Mirror | 300–2400 | -19.3 / -3.4 | -25.9 / -9.4 | -6.02, -6.19 |
| Minish Cap | 300–2400 | -12.5 / -0.0 | -19.0 / -6.1 | -6.05, -6.14 |
| Metroid Zero Mission | 300–2400 | -6.5 / 0.0 | -12.8 / -6.0 | -6.02, -6.06 |

Peaks drop by the gain's 6.02 dB; RMS and LUFS by a little more (to
-6.6 LU), the linear resampler's gentle low-pass at 48 kHz.

**The setting** (volume/setting): `--volume PCT`, else `volume = PCT` in
`settings.conf`, else 100, scales the gain linearly: 100 is
`DEFAULT_GAIN`, 200 the ungained stream, 0 silence (still paced by the
device). On the agb sound ROM, 600 frames of `--played-wav`: peak
-19.32 dBFS at 100, -25.34 at 50, -13.30 at 200 (±6.02 dB).

**Windows, louder still** (volume/windows-loud, read 2026-10-29, not
heard). The report came from a build before the default gain, so both
platforms played at full scale. Nothing in our path scales differently
on Windows: cpal 0.18.2's WASAPI backend opens a shared-mode stream in
the engine's mix format (`GetMixFormat`, F32 on every current Windows),
with `AUTOCONVERTPCM`, and never touches the session volume
(`ISimpleAudioVolume`), so a new app plays at 100% of the master. The
I16/I32/U16 paths use dasp's `s * 2^(n-1)`, saturating, right for our
±1.0; `interleave` puts L/R on the first two channels and silence on the
rest, the WAVEFORMATEXTENSIBLE order (FL, FR, FC, LFE, …), so 5.1/7.1
are not summed or doubled. The difference is the host: PipeWire/Pulse
restore a per-app stream volume and use a cubic volume curve, while a
Windows session starts at 100% and "Loudness Equalization" (a speaker
enhancement) lifts quiet content further. The −6 dB default is the fix
we can make; real-Windows confirmation is june's.

### Exactly one pacer: the device

With sound on, the audio device's consumption paces the emulator.
The window (SDL3, on the main thread) never paces: it draws each frame
the game thread hands it as it arrives. With sound off
(`--mute`, or no device found) a frame clock paces at the GBA's 59.73 Hz
(`desktop/src/fast.rs`). Headless runs never open a device and aren't
paced. Fast-forward sets both pacers aside: frames run unthrottled or on
a clock at `--fast-speed` times 59.73 Hz, each frame's sound is dropped,
and the queue is emptied as it starts and as it stops, so the device
plays silence instead of a starved stream and, on release, the queue
refills to its target before playing at normal speed again.

- The emulator thread resamples each `AudioChunk` as it arrives and, at
  the end of each frame (`Platform::present`), pushes the frame's output
  into a queue the device's callback drains on its own thread.
- It then waits (a condvar the callback signals) until the queue is down
  to its **target**, and only then runs the next frame. The device takes
  the queue a pull at a time but plays it steadily, so up to a pull's
  worth played since the last pull counts as gone (`Queue::due`): frames
  then run a frame apart (presents 16.8 ms apart at p50, 17.8 at p99,
  under gamescope, 2026-10-24) instead of two at once after each 21 ms
  pull, and the queue sits about a pull higher. So the emulator
  runs exactly as fast as the device plays, and the queue never goes over
  the target plus one frame. It dips below the target by about a pull,
  and further when pulls land while the emulator is still running a frame.
- The target is 50 ms, or the device's largest pull so far plus 40 ms if
  that's more (PipeWire's default quantum is 1024 frames, 21 ms, making it
  61 ms; a pull of 2048 makes it 83 ms). The 40 ms covers a late pull
  followed straight away by the next one, with a slow emulator frame.
- The device gets silence until it has made one pull (which says how big
  its pulls are) and the queue has reached the target; after that a short
  queue is an underrun, counted, and filled with silence.
- A frame with no sound (a silent game, or before the mixer starts) still
  lasts a frame: it's queued as a frame's worth of silence (a ramp to 0
  from the last sample), so silence paces at full speed too.
- If the device goes 200 ms without a pull (unplugged, the sound server
  restarted, or slow to start), the pacer paces by the frame clock
  (sleeping to 59.73 frames a second) until the device pulls again, so the
  window keeps running at full speed; it says so on stderr if the device
  had been playing. Meanwhile the queue fills to 500 ms and drops the
  rest. When the device pulls again, that stale queue is dropped and
  refilled to the target before playing. A device that only pulls now and
  then would flip between the two, costing 200 ms each time.

**Why the device, not the frame clock.** Two clocks (a display's 60 Hz and
the device's sample clock) drift apart, and one of them has to give: with
the frame clock in charge the queue drifts until it underruns or must
drop samples. With the device in charge the emulator follows the device's
clock exactly and the frame rate gives instead, by the device's clock
error (typically well under 0.1%; the test allows ±0.5%), which no one
can see. GBA's 59.73 fps against a 60 Hz monitor tears or stutters either
way.

### Resampling and rate control

The resampler works on the machine's clock: each record spans `period`
cycles, an output frame falls every `16777216 / device rate` cycles, and
the output is linearly interpolated between the two records it falls
between. The phase carries across chunks and frames, so the rate is exact
over any length (a unit test runs 1000 frames of the 18157 Hz ROM's stream
and gets a frame's worth of 48 kHz output per frame, to the sample).

**Drift** between the stream's clock and the device's needs no
correction of its own: the pacer makes the emulator follow the device's
clock, so the stream is produced exactly as fast as it's played. On top of
that, rate control smooths the edges (RetroArch's dynamic rate control, in
effect): after each frame the step is scaled by `1 + 0.005 · e`, where `e`
is how far the queue is from its usual level after a push, as a fraction
of that level, clamped to ±1. The usual level is the queue's own running
average over about 64 frames, not a fixed point, because where the queue
sits after a push depends on how the device's pulls beat against the
frames. So `e` averages out to 0 and the pitch has no standing offset.
The average follows only a playing queue under its 500 ms cap, and starts
again after a stall, so filling up doesn't skew it. In practice the pacer
does the work: after a slow frame it refills the queue by running frames
back to back, and what's left for rate control is mostly the beat
between pulls and frames, a few tenths of a percent at most from frame to
frame, which the recording below can't tell from none.

### Checks

- **Pacing test** (`audio::tests::the_device_paces_the_emulator`, no ROM):
  a synthetic 440 Hz stream at 16777216/924 Hz, 304 records a frame, into
  a simulated 48 kHz device whose clock is 0.5% slow, exact or 0.5% fast,
  pulling up to 256, 480, 1024 or 2048 frames at a time on its own clock,
  regularly or irregularly (each pull between half and all of that, up to
  30% of a pull's time late), with the emulator taking 0.5 ms or 12 ms a
  frame. Every case: no underrun after the first second, the queue
  between 20 and 100 ms once playing (25.5–99.4 ms seen; the top only
  with 2048-frame pulls), and the emulator's frame rate equal to the
  device's clock to 0.3%. `a_slow_emulator_underruns` shows the check can
  fail; `silence_paces_too` covers the silent frames.
- The simulation applies the pacer's rule itself, so two more tests run
  the real pacer (`Player::wait`) against a device thread pulling on the
  real clock: `the_pacer_waits_for_the_device` (60 frames take at least
  0.9 s, the queue stays under 80 ms) and
  `a_stalled_device_falls_back_to_the_frame_clock` (one 200 ms wait, then
  30 frames at 59.73 a second, then, when the device pulls again, the stale
  queue dropped and the pacer back on the device).
- **`just audio-pace [ROM] [FRAMES]`** runs the same simulation
  (`audio::sim`, `gbaa-desktop --headless --audio-pace PCT`) on a real
  ROM's stream through the interpreter, each frame costing its real host
  time, at −0.5%, 0 and +0.5% with irregular pulls of up to 1024 frames.
  Both agb sound ROMs: 0 underruns, queue 40.4–78.1 ms.
- **`just audio-window [ROM] [FRAMES]`**: the window, sound on, in a
  private headless KWin session (`cu session`) whose sound goes to the
  `cu-silent` null sink. It checks the session routes sound there before
  starting, then watches where the stream lands and stops the run unless
  every `gbaa-desktop` stream is on `cu-silent`. That second check comes
  after the fact: a stream sent elsewhere would play for up to 0.1 s
  before it's stopped. The first check is what keeps sound off the
  speakers. Passes on both agb sound ROMs (10512 Hz ROM, 600
  frames: 479,744 samples delivered, 0 underruns, queue 40–67 ms). The
  recompiled 10512 Hz ROM, run the same way by hand, delivers the same 479,744
  samples with 0 underruns in 10.15 s, for 10.05 s of guest time.
- **What reached the sink**, recorded from `cu-silent.monitor` (the sink
  is muted by default; unmute it while recording, then mute it again) and
  compared with the same run's headless `--wav` resampled to 48 kHz:
  gain 1.000, correlation 0.996 or better per channel from 2 s on
  (0.9995 from 4 s). The playback runs at the emulated rate: between 2 s
  and 8 s the offset between the two moves by 4 samples at 48 kHz in all,
  and not at all after 4 s.
