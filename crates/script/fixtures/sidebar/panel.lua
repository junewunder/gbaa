-- A tracker-style panel right of the game picture (the canvas's sidebar,
-- crates/script/src/canvas.rs): 80 pixels wide, the picture's height, a
-- dark ground with a light border and a red bar, at the game's scale.
local w, h = 80, canvas:screenHeight()
panel = canvas:newLayer(w, h)
panel:setPosition(canvas:screenWidth(), 0)
local p = image.newPainter(panel.image)
p:setFill(true)
p:setFillColor(0xFF203040)
p:setStrokeWidth(1)
p:setStrokeColor(0xFFE0E0E0)
p:drawRectangle(0, 0, w, h)
p:setStrokeWidth(0)
p:setFillColor(0xFFE03030)
p:drawRectangle(8, 8, w - 16, 12)
panel:update()
console:log("panel " .. w .. "x" .. h .. " at " .. panel.x)
