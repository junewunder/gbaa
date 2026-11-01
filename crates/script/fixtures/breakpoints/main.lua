-- The breakpoints fixture (docs/tools/commands.md, `just script-check`):
-- mGBA 0.11-dev's setBreakpoint, setWatchpoint, setRangeWatchpoint and
-- clearBreakpoint on Emerald (USA), headless from power-on with no save.
-- Each point's frames are checked against what a once-a-frame look at the
-- same memory sees. Addresses from pokeemerald.sym.
local CB2_InitTitleScreen = 0x080AA7A4
local gMain = 0x030022C0      -- callback2 at +4
local gRngValue = 0x03005D80
local Random = 0x0806F5CC

-- Through the intro to the title screen (Start).
callbacks:add("keysRead", function()
  local f = emu:currentFrame()
  emu:setKeys((f % 60 >= 58 and f < 400) and (1 << C.GBA_KEY.START) or 0)
end)

-- Which frame a callback fires in: the one running, one after the last
-- `frame` callback's.
local function now() return emu:currentFrame() + 1 end

-- The title screen's set-up runs as gMain.callback2 over a few frames,
-- one state a frame: each call stops at its first instruction.
local inits = {}
local bp = emu:setBreakpoint(function(info)
  inits[#inits + 1] = now()
  if #inits == 1 then
    console:log(string.format("break at %08X in frame %d, lr %08X, segment %d",
      info.address, now(), emu:readRegister("lr"), info.segment))
  end
end, CB2_InitTitleScreen)
-- Two callbacks on one point fire in the order they were set.
local second = 0
local bp2 = emu:setBreakpoint(function() second = second + 1 end, CB2_InitTitleScreen)

-- The RNG's seed is stored by each Random(); a write watchpoint on it.
local rngWrites, rngFrames, firstRng = 0, {}, nil
local wp = emu:setWatchpoint(function(info)
  rngWrites = rngWrites + 1
  rngFrames[now()] = true
  if not firstRng then
    firstRng = info
    console:log(string.format(
      "write %08X width %d in frame %d: %08X -> %08X, accessType %d",
      info.address, info.width, now(), info.oldValue & 0xFFFFFFFF,
      info.newValue & 0xFFFFFFFF, info.accessType))
  end
end, gRngValue, C.WATCHPOINT_TYPE.WRITE)

-- Inside a function, as PokeLua's held-item scripts break: Random+4, a
-- Thumb instruction after the seed's load.
local randoms = 0
local rb = emu:setBreakpoint(function() randoms = randoms + 1 end, Random + 4)

-- Reads of gMain's callbacks (callback1, callback2), each main-loop pass
-- from frame 13 on, when the game's set-up is done.
local reads, readFrames = 0, {}
local rp = emu:setRangeWatchpoint(function(info)
  reads = reads + 1
  readFrames[now()] = (readFrames[now()] or 0) + 1
  if reads <= 2 then
    console:log(string.format("read %08X width %d in frame %d, accessType %d",
      info.address, info.width, now(), info.accessType))
  end
end, gMain, gMain + 8, C.WATCHPOINT_TYPE.READ)

-- What a once-a-frame look sees: the frames callback2 is the title's
-- set-up at their end, and those whose end saw a new seed.
local seen, changed, lastRng = {}, {}, emu:read32(gRngValue)
callbacks:add("frame", function()
  local f = emu:currentFrame()
  if emu:read32(gMain + 4) == CB2_InitTitleScreen | 1 then seen[#seen + 1] = f end
  local r = emu:read32(gRngValue)
  if r ~= lastRng then changed[f] = true end
  lastRng = r
  if f == 20 then
    local t = {}
    for g = 1, 20 do t[#t + 1] = readFrames[g] or 0 end
    console:log("reads a frame, frames 1-20: " .. table.concat(t, ","))
    console:log("cleared range: " .. tostring(emu:clearBreakpoint(rp)))
  elseif f == 300 then
    -- Every frame whose end saw a new seed had a write in it, and no other.
    local same = true
    for g = 2, 300 do
      if (changed[g] or false) ~= (rngFrames[g] or false) then
        same = false
        console:log("frame " .. g .. ": watch " .. tostring(rngFrames[g]) .. ", look " .. tostring(changed[g]))
      end
    end
    local n = 0
    for _ in pairs(rngFrames) do n = n + 1 end
    console:log(string.format("by frame 300: %d writes to gRngValue in %d frames; the once-a-frame look agrees: %s",
      rngWrites, n, tostring(same)))
    console:log(string.format("by frame 300: Random+4 reached %d times", randoms))
    console:log("cleared watch: " .. tostring(emu:clearBreakpoint(wp)) .. ", again: " .. tostring(emu:clearBreakpoint(wp)))
    emu:clearBreakpoint(rb)
  elseif f == 500 then
    console:log("title set-up broke in frames " .. table.concat(inits, ","))
    console:log("callback2 was the set-up at the end of frames " .. table.concat(seen, ","))
    console:log("second callback fired " .. second .. " times")
    -- The last: a recompiled build runs compiled again from here.
    console:log("cleared breaks: " .. tostring(emu:clearBreakpoint(bp)) .. " " .. tostring(emu:clearBreakpoint(bp2)))
  end
end)
