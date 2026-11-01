// The few calls mgba-dump makes into libmgba, behind a flat C API so the
// Rust side needn't mirror mCore's struct of function pointers.

#include <mgba/core/core.h>
#include <mgba/core/log.h>
#include <mgba/gba/core.h>
#include <mgba/internal/gba/audio.h>
#include <mgba/internal/gba/gba.h>
#include <mgba-util/vfs.h>

#include <stdio.h>
#include <stdlib.h>
#include <stddef.h>
#include <string.h>
#include <time.h>

static void quiet(struct mLogger* logger, int category, enum mLogLevel level, const char* format, va_list args) {
	(void) logger;
	(void) category;
	(void) level;
	(void) format;
	(void) args;
}

static struct mLogger logger = { .log = quiet };

// BizHawk's movie clock: its sync settings' start, plus `frame * 4389 >>
// 18` seconds (about 59.73 frames a second) at the movie's frame, which
// counts every frame run, resets included (MGBAHawk.RTCTime).
static struct {
	struct mRTCSource d;
	int64_t start;
	uint64_t frame;
} bizhawk_clock;

static time_t bizhawk_time(struct mRTCSource* source) {
	(void) source;
	return bizhawk_clock.start + (int64_t) ((bizhawk_clock.frame * 4389) >> 18);
}

// A GBA core running `rom` (copied) from reset, drawing into `frame`
// (240x160 XBGR8). With no `bios` (NULL) no BIOS file is loaded and the
// BIOS isn't skipped, so mGBA's built-in HLE BIOS runs its reset code;
// with one (`bios_len` bytes, copied), that BIOS runs, its intro
// included, as BizHawk runs a movie (its SkipBios applies only outside
// one). The cartridge clock reads `clock` (Unix seconds), taken as UTC:
// forever, as runtime::gpio's does; or, if `bizhawk`, as a BizHawk movie's
// clock from `clock`, and idle loops aren't skipped, as BizHawk has it.
// NULL on failure.
struct mCore* gbaa_mgba_open(const void* rom, size_t len, uint32_t* frame, int64_t clock, bool bizhawk,
                             const void* bios, size_t bios_len) {
	mLogSetDefaultLogger(&logger);
	// mGBA turns the clock's time into a date with localtime_r.
	setenv("TZ", "UTC0", 1);
	tzset();
	struct mCore* core = GBACoreCreate();
	if (!core) {
		return NULL;
	}
	if (!core->init(core)) {
		free(core);
		return NULL;
	}
	// Defaults only: no config file is read, so a gba_bios.bin in the
	// user's mGBA config can't be picked up.
	mCoreInitConfig(core, NULL);
	core->opts.useBios = bios != NULL;
	core->opts.skipBios = false;
	if (bizhawk) {
		bizhawk_clock.d.sample = NULL;
		bizhawk_clock.d.unixTime = bizhawk_time;
		bizhawk_clock.start = clock;
		bizhawk_clock.frame = 0;
		mCoreSetRTC(core, &bizhawk_clock.d);
	} else {
		core->rtc.override = RTC_FIXED;
		core->rtc.value = clock * 1000;
	}
	core->setVideoBuffer(core, frame, 240);
	struct VFile* vf = VFileMemChunk(rom, len);
	// On failure loadROM may or may not have closed vf, so it's left.
	if (!vf || !core->loadROM(core, vf)) {
		core->deinit(core);
		return NULL;
	}
	if (bios) {
		struct VFile* bf = VFileMemChunk(bios, bios_len);
		if (!bf || !core->loadBIOS(core, bf, 0)) {
			core->deinit(core);
			return NULL;
		}
	}
	if (bizhawk) {
		((struct GBA*) core->board)->idleOptimization = IDLE_LOOP_IGNORE;
	}
	core->reset(core);
	return core;
}

void gbaa_mgba_run_frame(struct mCore* core, uint32_t keys) {
	core->setKeys(core, keys);
	core->runFrame(core);
	++bizhawk_clock.frame;
}

// The power button: a reset, keeping the ROM, the save data and the clock.
void gbaa_mgba_reset(struct mCore* core) {
	core->reset(core);
}

// r0-r15, then CPSR.
void gbaa_mgba_regs(struct mCore* core, uint32_t* out) {
	char name[8];
	for (int i = 0; i < 16; ++i) {
		snprintf(name, sizeof(name), "r%d", i);
		core->readRegister(core, name, &out[i]);
	}
	core->readRegister(core, "cpsr", &out[16]);
}

void gbaa_mgba_close(struct mCore* core) {
	core->deinit(core);
}

size_t gbaa_mgba_state_size(struct mCore* core) {
	return core->stateSize(core);
}

// The uncompressed state (GBASerializedState), state_size bytes.
bool gbaa_mgba_save_state(struct mCore* core, void* out) {
	return core->saveState(core, out);
}

static size_t psg_len;

// Loading replays the I/O registers through mGBA's write handlers, which
// sample the PSG on the core's old clock: those records aren't the
// state's, so the PSG dump starts after the load.
bool gbaa_mgba_load_state(struct mCore* core, const void* in) {
	bool ok = core->loadState(core, in);
	psg_len = 0;
	return ok;
}

// Put `save` (copied) in place of the cartridge's save data, as mGBA does
// loading a GUI state that carries it: a mask over the save, which keeps
// the type the state set.
bool gbaa_mgba_restore_save(struct mCore* core, const void* save, size_t len) {
	return core->savedataRestore(core, save, len, false);
}

// The sound FIFOs' output, one record per timer overflow that pops one, as
// gbaa-desktop --fifo-dump writes it: FIFO A's sample then B's, a FIFO this
// timer doesn't clock keeping its last. build.rs links with
// --wrap=GBAAudioSampleFIFO, so timer.c's call (once per FIFO an overflow
// pops, A before B) lands here. One core per process, so the state is global.
static uint8_t* fifo_out;
static size_t fifo_len, fifo_cap;
static int8_t fifo_last[2];

// Whether the FIFO and PSG records are kept: a long replay (a TAS's
// millions of frames) that wants none would fill memory with them.
static bool record_audio = true;

void gbaa_mgba_record_audio(bool on) {
	record_audio = on;
}

void __real_GBAAudioSampleFIFO(struct GBAAudio* audio, int fifoId, int32_t cycles);

void __wrap_GBAAudioSampleFIFO(struct GBAAudio* audio, int fifoId, int32_t cycles) {
	struct GBAAudioFIFO* ch = fifoId ? &audio->chB : &audio->chA;
	// What GBAAudioSampleFIFO outputs: the shift register's low byte, after
	// it loads the next word if it's drained and the FIFO isn't empty. The
	// DMA it may ask for is only scheduled, so can't change that.
	uint32_t sample = ch->internalSample;
	if (!ch->internalRemaining && ch->fifoRead != ch->fifoWrite) {
		sample = ch->fifo[ch->fifoRead];
	}
	__real_GBAAudioSampleFIFO(audio, fifoId, cycles);
	if (fifoId < 0 || fifoId > 1 || !record_audio) {
		return;
	}
	fifo_last[fifoId] = (int8_t) sample;
	// B popped by the overflow that just popped A belongs to A's record.
	bool same = fifoId == 1 && audio->chATimer == audio->chBTimer && (audio->chALeft || audio->chARight);
	if (same && fifo_len) {
		fifo_out[fifo_len - 1] = (uint8_t) sample;
		return;
	}
	if (fifo_len + 2 > fifo_cap) {
		fifo_cap = fifo_cap ? fifo_cap * 2 : 1 << 16;
		fifo_out = realloc(fifo_out, fifo_cap);
		if (!fifo_out) {
			abort();
		}
	}
	fifo_out[fifo_len++] = (uint8_t) fifo_last[0];
	fifo_out[fifo_len++] = (uint8_t) fifo_last[1];
}

// The records so far (2 bytes each), and how many bytes.
const uint8_t* gbaa_mgba_fifo(size_t* len) {
	*len = fifo_len;
	return fifo_out;
}

// The PSG's share of the mix, one record per sample on mGBA's grid, as
// gbaa-desktop --psg-dump writes it: left then right, i16 LE, after
// GBAAudioSample's SOUNDCNT_H shift. build.rs links with
// --wrap=GBAudioSamplePSG, which GBAAudioSample calls once a sample.
static uint8_t* psg_out;
static size_t psg_cap;

void __real_GBAudioSamplePSG(struct GBAudio* audio, int16_t* left, int16_t* right);

void __wrap_GBAudioSamplePSG(struct GBAudio* audio, int16_t* left, int16_t* right) {
	__real_GBAudioSamplePSG(audio, left, right);
	if (!record_audio) {
		return;
	}
	struct GBAAudio* gba = (struct GBAAudio*) ((char*) audio - offsetof(struct GBAAudio, psg));
	int shift = 4 - gba->volume;
	int16_t s[2] = { *left >> shift, *right >> shift };
	if (psg_len + 4 > psg_cap) {
		psg_cap = psg_cap ? psg_cap * 2 : 1 << 16;
		psg_out = realloc(psg_out, psg_cap);
		if (!psg_out) {
			abort();
		}
	}
	for (int i = 0; i < 2; ++i) {
		psg_out[psg_len++] = (uint8_t) s[i];
		psg_out[psg_len++] = (uint8_t) ((uint16_t) s[i] >> 8);
	}
}

// The PSG records so far (4 bytes each), and how many bytes.
const uint8_t* gbaa_mgba_psg(size_t* len) {
	*len = psg_len;
	return psg_out;
}

// mGBA's mixed output (--wav): each sample GBAAudioSample makes, after
// SOUNDBIAS and the master volume (100%, mGBA's default), before the blip
// resampler, through the core's AV stream. `mix_rate` is its rate in Hz
// (16777216 / sampleInterval). When a game changes the rate (SOUNDBIAS's
// resolution), the samples so far are stretched to the new one by
// repeating or dropping samples, so the whole run is at one rate and in time.
static uint8_t* mix_out;
static size_t mix_len, mix_cap;
static unsigned mix_rate;

static void mix_reserve(size_t len) {
	if (len > mix_cap) {
		while (len > mix_cap) {
			mix_cap = mix_cap ? mix_cap * 2 : 1 << 16;
		}
		mix_out = realloc(mix_out, mix_cap);
		if (!mix_out) {
			abort();
		}
	}
}

static void mix_rate_set(struct mAVStream* stream, unsigned rate) {
	(void) stream;
	if (mix_len && mix_rate && rate != mix_rate) {
		size_t n = mix_len / 4, m = (size_t) ((uint64_t) n * rate / mix_rate);
		uint32_t* old = malloc(mix_len);
		if (!old) {
			abort();
		}
		memcpy(old, mix_out, mix_len);
		mix_reserve(m * 4);
		for (size_t i = 0; i < m; ++i) {
			memcpy(mix_out + i * 4, &old[(uint64_t) i * mix_rate / rate], 4);
		}
		free(old);
		mix_len = m * 4;
	}
	mix_rate = rate;
}

static void mix_post(struct mAVStream* stream, int16_t left, int16_t right) {
	(void) stream;
	mix_reserve(mix_len + 4);
	int16_t s[2] = { left, right };
	for (int i = 0; i < 2; ++i) {
		mix_out[mix_len++] = (uint8_t) s[i];
		mix_out[mix_len++] = (uint8_t) ((uint16_t) s[i] >> 8);
	}
}

static struct mAVStream mix_stream = { .audioRateChanged = mix_rate_set, .postAudioFrame = mix_post };

// Start keeping the mixed output, from now on.
void gbaa_mgba_mix_start(struct mCore* core) {
	mix_len = 0;
	mix_rate = 0;
	core->setAVStream(core, &mix_stream);
}

// The mixed samples so far (L R pairs, i16 LE, 4 bytes each), how many
// bytes, and their rate.
const uint8_t* gbaa_mgba_mix(size_t* len, unsigned* rate) {
	*len = mix_len;
	*rate = mix_rate;
	return mix_out;
}
