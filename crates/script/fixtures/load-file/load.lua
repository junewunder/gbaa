-- scripting/ironmon's check of `emu:loadFile` (crates/desktop/tests/script_load_file.rs):
-- run headless from a copy beside `next.gba`, it changes the first ROM's
-- save, loads the next ROM as Ironmon's new runs do (loadFile, then
-- reset), and writes the next ROM's save.
callbacks:add("frame", function()
  local f = emu:currentFrame()
  if f == 2 then
    console:log(string.format("%s save %02X", emu:getGameTitle(), emu:read8(0x0E000000)))
    emu:write8(0x0E000000, 0x22)
  elseif f == 3 then
    console:log("loaded: " .. tostring(emu:loadFile(script.dir .. "/next.gba")))
    emu:reset()
  elseif f == 4 then
    console:log(string.format("%s save %02X", emu:getGameTitle(), emu:read8(0x0E000000)))
    emu:write8(0x0E000000, 0x33)
  end
end)
