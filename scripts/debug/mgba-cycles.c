#include <mgba/core/core.h>
#include <mgba/core/log.h>
#include <mgba/gba/core.h>
#include <mgba/internal/gba/gba.h>
#include <mgba/internal/arm/arm.h>
#include <mgba-util/vfs.h>
#include <stdio.h>
#include <stdlib.h>
static void quiet(struct mLogger* l, int c, enum mLogLevel v, const char* f, va_list a) {}
static struct mLogger logger = { .log = quiet };
// mGBA per-instruction cycles: ROM FRAME START END prints "pc cycle" for each instruction from
// START's first run in FRAME until END. Build: as mgba-trace.c (memory/emerald-debugging.md).
int main(int argc, char** argv) {
	mLogSetDefaultLogger(&logger);
	unsigned frame = atoi(argv[2]); unsigned start = strtoul(argv[3],0,16)&~1u, end = strtoul(argv[4],0,16)&~1u;
	struct mCore* core = GBACoreCreate(); core->init(core); mCoreInitConfig(core, NULL);
	core->opts.useBios = false; core->opts.skipBios = false;
	static uint32_t fb[240*160]; core->setVideoBuffer(core, fb, 240);
	struct VFile* vf = VFileOpen(argv[1], O_RDONLY); core->loadROM(core, vf); core->reset(core);
	struct GBA* gba = core->board; struct ARMCore* cpu = gba->cpu;
	int on = 0;
	while (gba->video.frameCounter <= frame + 1) {
		unsigned pc = cpu->gprs[15] - (cpu->executionMode == MODE_THUMB ? 2 : 4);
		if (!on && gba->video.frameCounter == frame && pc == start) on = 1;
		if (on) { printf("%08x %lld\n", pc, (long long) mTimingCurrentTime(&gba->timing)); if (pc == end) return 0; }
		ARMRun(cpu);
	}
	return 0;
}
