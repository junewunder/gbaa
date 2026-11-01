-- scripting/state-slots' check (crates/desktop/tests/script_slots.rs): run
-- headless, it saves slot 7 as PokeLua's Shift+number hotkeys do, changes
-- the game, and loads the slot back as its number hotkeys do.
callbacks:add("frame", function()
  local f = emu:currentFrame()
  if f == 2 then
    emu:write32(0x02000000, 0x12345678)
    console:log("saved 7: " .. tostring(emu:saveStateSlot(7)))
    console:log("saved -1: " .. tostring(emu:saveStateSlot(-1)))
  elseif f == 3 then
    emu:write32(0x02000000, 0xDEADBEEF)
    console:log("loaded 8: " .. tostring(emu:loadStateSlot(8)))
  elseif f == 4 then
    console:log("loaded 7: " .. tostring(emu:loadStateSlot(7)))
    console:log(string.format("wram %08X", emu:read32(0x02000000)))
  end
end)
