-- Kirby & the Amazing Mirror (USA): one console line a frame for `just timing-lab`
-- (docs/research/why-timing-is-runtime.md), to compare runs whose timing differs.
-- Fields: frame; fc gFrameCount (0x03002E64, counted in the V-blank handler); rng gRngVal
-- (0x030068D8); wait, 1 when the main loop is parked in VBlankIntrWait as the frame ends
-- (gMainFlags 0x03002440 bit 24, set just before the wait and cleared at the loop's top; the frame
-- ends as V-blank starts), so 0 is a lag frame, the game's work not done in time; x y of gKirbys[0]
-- (0x02020EE0 +0x40, +0x44; 1/256 px) and its stateFn (+0x78). Addresses from the katam decomp.

local K = 0x02020EE0
local f = 0

callbacks:add("frame", function()
  f = f + 1
  console:log(string.format("%d fc=%d rng=%08x wait=%d x=%d y=%d st=%08x",
    f, emu:read32(0x03002E64), emu:read32(0x030068D8), (emu:read32(0x03002440) >> 24) & 1,
    emu:read32(K + 0x40), emu:read32(K + 0x44), emu:read32(K + 0x78)))
end)
