-- The Lua host's fixture (docs/tools/commands.md, `just script-check`):
-- mGBA 0.10.5's API on Emerald (USA), headless from power-on with no save.
-- It presses through the intro and title into a new game and, in Birch's
-- speech, writes a party of one into RAM for mGBA's pokemon.lua, run beside
-- it, to show.
local fx = require("fixture_util")
local here = debug.getinfo(1, "S").source:match("^@(.*[/\\])") or "./"
console:log("dofile: " .. tostring(dofile(here .. "extra.lua")))

console:log(string.format("title %s, code %s, crc32 %s, %d bytes",
  emu:getGameTitle(), emu:getGameCode(), fx.hex(emu:checksum(C.CHECKSUM.CRC32)),
  emu:romSize()))
console:log(string.format("platform %d, %d cycles a frame at %d Hz",
  emu:platform(), emu:frameCycles(), emu:frequency()))
local cart = emu.memory.cart0
console:log(string.format("cart0 %08X-%08X; header byte %02X, logo word %08X",
  cart:base(), cart:bound(), cart:read8(0xB2), cart:read32(4)))
console:log("readRange: " .. fx.hex(emu:readRange(0x080000A0, 12)))

-- Unused EWRAM at its end: a write read back, through `emu` and a domain.
emu:write32(0x0203FFF0, 0x12345678)
emu.memory.wram:write16(0x3FFF4, 0xBEEF)
console:log(string.format("wram %08X %04X %02X", emu.memory.wram:read32(0x3FFF0),
  emu:read16(0x0203FFF4), emu:read8(0x0203FFF3)))
console:log(string.format("bitmask %d, %s", util.makeBitmask({C.GBA_KEY.A, C.GBA_KEY.START}),
  table.concat(util.expandBitmask(9), ",")))

-- A name mGBA has and we don't: warned of once, answers nil.
console:log("missing: " .. tostring(emu:notAnMgbaCall()) .. " " .. tostring(emu:notAnMgbaCall()))

local buf = console:createBuffer("Fixture")

-- Through the intro and title (Start), then New Game (A), as a player would.
local presses = {}
for f = 60, 540, 60 do presses[f] = "START" end
for f = 600, 840, 60 do presses[f] = "A" end
callbacks:add("keysRead", function()
  local f = emu:currentFrame()
  local key
  for at, k in pairs(presses) do
    if f >= at and f < at + 4 then key = k end
  end
  emu:setKeys(key and (1 << C.GBA_KEY[key]) or 0)
end)

-- Loaded into a running game, as from mGBA's scripting window: `start`
-- doesn't fire.
local starts = 0
callbacks:add("start", function() starts = starts + 1 end)

callbacks:add("frame", function()
  local f = emu:currentFrame()
  if f == 1 then
    console:log(string.format("frame 1: start fired %d time(s), pc %08X", starts,
      emu:readRegister("pc")))
    emu:reset()
  elseif f == 2 then
    -- Ten instructions on from frame 2, then a frame more.
    for _ = 1, 10 do emu:step() end
    console:log(string.format("stepped to pc %08X", emu:readRegister("pc")))
    emu:runFrame()
  elseif f == 50 then
    -- Beside the script: a sandboxed script may write only there.
    local png = here .. "screenshot.tmp.png"
    local ok = emu:screenshot(png)
    local fh = io.open(png, "rb")
    console:log("screenshot: " .. tostring(ok) .. ", " .. fh:read(4):sub(2) .. " file")
    fh:close()
    os.remove(png)
  elseif f == 1200 then
    -- Emerald's gPlayerParty[0]: personality and OT ID 0, so the secure
    -- substructures are in order and unencrypted (key 0); the growth one,
    -- first, holds the species. pokemon.lua decodes it.
    local mon = 0x020244EC
    for i = 0, 99 do emu:write8(mon + i, 0) end
    local name = {0xC6, 0xCF, 0xBB, 0xFF} -- "LUA" in the game's charset
    for i, c in ipairs(name) do emu.memory.wram:write8(mon - 0x02000000 + 7 + i, c) end
    emu:write16(mon + 32, 280) -- Torchic
    emu:write8(mon + 84, 5)    -- level
    emu:write16(mon + 86, 19)  -- HP
    emu:write16(mon + 88, 20)  -- max HP
    emu:write8(0x020244E9, 1)  -- gPlayerPartyCount
  elseif f == 100 then
    -- A state round trip: the frame counter in RAM comes back.
    local s = emu:saveStateBuffer()
    local before = emu:read32(0x03000000)
    emu:write32(0x03000000, before ~ 0xFFFFFFFF)
    console:log("state loaded: " .. tostring(emu:loadStateBuffer(s)) ..
      ", restored: " .. tostring(emu:read32(0x03000000) == before))
  end
end)

-- The guard: a callback that never returns is stopped and removed, and
-- the game and the other callbacks run on.
callbacks:add("frame", function()
  if emu:currentFrame() == 200 then
    console:log("hanging at frame 200")
    while true do end
  end
end)

local saves = 0
callbacks:add("reset", function()
  console:log(string.format("reset at frame %d, pc %08X", emu:currentFrame(), emu:readRegister("pc")))
end)
callbacks:add("savedataUpdated", function() saves = saves + 1 end)
callbacks:add("stop", function()
  -- Emerald's gPlayerPartyCount.
  buf:print(string.format("frames %d, party %d\n", emu:currentFrame(), emu:read8(0x020244E9)))
end)
callbacks:add("shutdown", function() console:log("shutdown, savedata updates " .. saves) end)
