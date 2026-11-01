-- Gets Emerald from its title screen into the saved game, through mGBA's
-- key API: the probe's own test of keysRead and setKeys.
local presses = {[30] = "START", [150] = "A", [270] = "A", [330] = "A", [390] = "A"}
local held = 0
callbacks:add("keysRead", function()
  local f = emu:currentFrame()
  for at, key in pairs(presses) do
    if f >= at and f < at + 6 then held = 1 << C.GBA_KEY[key] break else held = 0 end
  end
  emu:setKeys(held)
end)
