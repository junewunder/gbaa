// mGBA side of `gbaa-interp --trace`: prints frame and line each time an address runs.
// Build: see memory/emerald-debugging.md. Usage: mgba-trace ROM FRAMES ADDR...
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
int main(int argc, char** argv) {
	mLogSetDefaultLogger(&logger);
	int frames = atoi(argv[2]);
	int n = argc - 3; unsigned addrs[32];
	for (int i = 0; i < n; i++) addrs[i] = strtoul(argv[3 + i], 0, 16) & ~1u;
	struct mCore* core = GBACoreCreate(); core->init(core); mCoreInitConfig(core, NULL);
	core->opts.useBios = false; core->opts.skipBios = false;
	static uint32_t fb[240*160]; core->setVideoBuffer(core, fb, 240);
	struct VFile* vf = VFileOpen(argv[1], O_RDONLY); core->loadROM(core, vf); core->reset(core);
	struct GBA* gba = core->board; struct ARMCore* cpu = gba->cpu;
	unsigned last = 0xffffffff;
	while (gba->video.frameCounter < (unsigned) frames) {
		ARMRun(cpu);
		unsigned pc = cpu->gprs[15] - (cpu->executionMode == MODE_THUMB ? 2 : 4);
		if (pc == last) continue;
		last = pc;
		for (int i = 0; i < n; i++) if (pc == addrs[i])
			printf("trace %08x frame %u line %d r0 %08x lr %08x\n", pc, gba->video.frameCounter, gba->video.vcount, cpu->gprs[0], cpu->gprs[14]);
	}
	return 0;
}
