-- A BizHawk-style panel beside the picture (scripting/bizhawk-gui), run
-- with `--script` on Emerald: a grey box right of the game with text,
-- the frame count, the game code, a line, a triangle and a rectangle.
client.SetGameExtraPadding(0, 0, 150, 0)
while true do
  gui.drawRectangle(245, 5, 139, 150, 0xFFAAAAAA, 0xFF222222)
  gui.drawText(250, 10, "gbaa BizHawk shim", "white", nil, 12)
  gui.drawText(250, 30, string.format("frame %d", emu.framecount()), "cyan", nil, 10)
  gui.drawText(250, 45, string.format("code %08x", memory.read_u32_be(0xAC, "ROM")), "yellow", nil, 10)
  gui.drawLine(250, 65, 375, 65, "orange")
  gui.drawPolygon({{255, 75}, {255, 95}, {275, 85}}, "lime", "green")
  gui.drawRectangle(290, 75, 20, 20, "red", 0x800000FF)
  emu.frameadvance()
end
