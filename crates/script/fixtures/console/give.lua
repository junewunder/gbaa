-- For the script console's check (docs/tools/commands.md, gbaa-desktop's
-- script console): defines `give(level)`, to type at the console's prompt
-- as Ironmon's commands are typed, which puts a Torchic named LUA at
-- `level` in Emerald's party for mGBA's pokemon.lua to show. As
-- ../emerald/main.lua does: personality and OT ID 0, so the substructures
-- are in order and unencrypted.
function give(level)
  local mon = 0x020244EC -- gPlayerParty[0]
  for i = 0, 99 do emu:write8(mon + i, 0) end
  for i, c in ipairs({0xC6, 0xCF, 0xBB, 0xFF}) do emu:write8(mon + 7 + i, c) end
  emu:write16(mon + 32, 280)         -- Torchic
  emu:write8(mon + 84, level or 5)   -- level
  emu:write16(mon + 86, 19)          -- HP
  emu:write16(mon + 88, 20)          -- max HP
  emu:write8(0x020244E9, 1)          -- gPlayerPartyCount
  return "gave LUA, Lv " .. (level or 5)
end
console:log('give.lua: type give(5) to put a Torchic in the party')
