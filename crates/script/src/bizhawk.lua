-- BizHawk's Lua dialect (2.x), as far as drawn panels need it, over the
-- host's own: `gui.draw*` on a canvas layer per script, the size of the
-- picture and `client.SetGameExtraPadding`'s padding (so a right or left
-- pad is a sidebar; above and below the picture the canvas clips), the
-- `memory` and `mainmemory` reads and writes, `emu.frameadvance` (the
-- script's main chunk runs as a coroutine, `__run`, resumed each frame),
-- `event.onframeend`, `input.getmouse` (src/input.rs), `bit` and
-- `bizstring`. `forms`
-- (WinForms dialogs) is out of scope: its calls warn and give nil.
local G, stubbed = _G, __stubbed

-- Run a script's main chunk; if it yields (`emu.frameadvance`), resume it
-- each frame until it ends.
function __run(f)
  local co = coroutine.create(f)
  local ok, err = coroutine.resume(co)
  if not ok then error(err, 0) end
  if coroutine.status(co) ~= "suspended" then return end
  local id
  id = callbacks:add("frame", function()
    local ok, err = coroutine.resume(co)
    if coroutine.status(co) == "dead" then callbacks:remove(id) end
    if not ok then error(err, 0) end
  end)
end

-- Each script's drawing surface: its padding, layer and painter.
local surfaces = {}
local function surface()
  local s = surfaces[__script]
  if not s then
    s = {pad = {0, 0, 0, 0}, fg = 0xFFFFFFFF, bg = nil, textbg = nil}
    surfaces[__script] = s
  end
  if not s.layer then
    local l, t, r, b = table.unpack(s.pad)
    s.layer = canvas:newLayer(l + 240 + r, t + 160 + b)
    if not s.layer then error("gui: no canvas layer left", 3) end
    s.layer:setPosition(-l, -t)
    s.image = s.layer.image
    s.painter = image.newPainter(s.image)
    s.painter:setBlend(true)
    __builtinFont(s.painter)
  end
  return s
end
local function clear(s)
  if s and s.layer and s.dirty then
    local p = s.painter
    p:setBlend(false)
    p:setFill(true)
    p:setFillColor(0)
    p:setStrokeWidth(0)
    p:drawRectangle(0, 0, s.image.width, s.image.height)
    p:setBlend(true)
    s.dirty = false
  end
end
-- Show what the script drew this frame; the next frame starts clear, as
-- BizHawk's surface does.
local function commit()
  local s = surfaces[__script]
  if s and s.layer then
    s.layer:update()
    s.dirty = true
  end
end
local function nextFrame()
  local s = surfaces[__script]
  commit()
  coroutine.yield()
  clear(s)
end

local function int(n) return math.floor(tonumber(n) or 0) end

-- .NET's named colours, those scripts use.
local NAMES = {
  black = 0xFF000000, white = 0xFFFFFFFF, red = 0xFFFF0000, lime = 0xFF00FF00,
  green = 0xFF008000, blue = 0xFF0000FF, yellow = 0xFFFFFF00, cyan = 0xFF00FFFF,
  aqua = 0xFF00FFFF, magenta = 0xFFFF00FF, fuchsia = 0xFFFF00FF, gray = 0xFF808080,
  grey = 0xFF808080, darkgray = 0xFFA9A9A9, lightgray = 0xFFD3D3D3, silver = 0xFFC0C0C0,
  orange = 0xFFFFA500, purple = 0xFF800080, pink = 0xFFFFC0CB, brown = 0xFFA52A2A,
  navy = 0xFF000080, maroon = 0xFF800000, olive = 0xFF808000, teal = 0xFF008080,
  gold = 0xFFFFD700, violet = 0xFFEE82EE, indigo = 0xFF4B0082, darkblue = 0xFF00008B,
  darkred = 0xFF8B0000, darkgreen = 0xFF006400, lightblue = 0xFFADD8E6,
  lightgreen = 0xFF90EE90, transparent = 0x00FFFFFF,
}
local function color(c, default)
  if c == nil then return default end
  if type(c) == "number" then return int(c) & 0xFFFFFFFF end
  if type(c) == "string" then
    local hex = c:match("^#(%x+)$")
    if hex and #hex == 6 then return 0xFF000000 | tonumber(hex, 16) end
    if hex and #hex == 8 then return tonumber(hex, 16) end
    return NAMES[c:lower()] or default
  end
  return default
end

local function fill(s, x, y, w, h, c)
  if not c or c >> 24 == 0 or w <= 0 or h <= 0 then return end
  local p = s.painter
  p:setStrokeWidth(0)
  p:setFill(true)
  p:setFillColor(c)
  p:drawRectangle(x, y, w, h)
end
local function line(s, x1, y1, x2, y2, c)
  if not c or c >> 24 == 0 then return end
  local p = s.painter
  p:setStrokeWidth(1)
  p:setStrokeColor(c)
  p:drawLine(x1, y1, x2, y2)
end

-- An image file, by path, at a size: loaded once. A relative path is the
-- script's folder's first, as BizHawk runs a script there.
local images = {}
local function scriptDir()
  for i = #__scripts, 1, -1 do
    if __scripts[i].id == __script then return __scripts[i].env.script.dir end
  end
end
local function loadImage(path, w, h)
  local key = tostring(__script) .. "\0" .. path .. "\0" .. tostring(w) .. "x" .. tostring(h)
  local img = images[key]
  if img ~= nil then return img or nil end
  local dir = scriptDir()
  local ok, src = false, nil
  if dir and not path:match("^/") then ok, src = pcall(image.load, dir .. "/" .. path) end
  if not (ok and src) then ok, src = pcall(image.load, path) end
  src = ok and src or nil
  if src and w and h and (w ~= src.width or h ~= src.height) and w > 0 and h > 0 then
    local scaled = image.new(w, h)
    for y = 0, h - 1 do
      local sy = y * src.height // h
      for x = 0, w - 1 do
        scaled:setPixel(x, y, src:getPixel(x * src.width // w, sy))
      end
    end
    src = scaled
  end
  images[key] = src or false
  return src
end

local function text(x, y, msg, fg, bg, size, halign, valign)
  local s = surface()
  local p = s.painter
  msg = tostring(msg)
  -- BizHawk's sizes are pixels; the painter's points at 100 dpi.
  p:setFontSize((size or 12) * 0.72)
  local box = p:textBoxSize(msg)
  local w, h = int(box.width), int(box.height)
  local ax, ay = 0x01, 0x10
  if halign == "center" or halign == "middle" then x = x - w // 2
  elseif halign == "right" then x = x - w end
  if valign == "middle" or valign == "center" then y = y - h // 2
  elseif valign == "bottom" then y = y - h end
  fill(s, x, y, w + 1, h, color(bg, s.textbg))
  p:setStrokeWidth(0)
  p:setFillColor(color(fg, s.fg))
  p:drawText(msg, x, y, ax | ay)
end

gui = stubbed({
  drawRectangle = function(x, y, w, h, lc, bg)
    local s = surface()
    x, y, w, h = int(x), int(y), int(w), int(h)
    if w < 0 then x, w = x + w, -w end
    if h < 0 then y, h = y + h, -h end
    -- GDI+'s: the outline is w+1 by h+1, the fill inside it.
    fill(s, x + 1, y + 1, w - 1, h - 1, color(bg, s.bg))
    local c = color(lc, s.fg)
    fill(s, x, y, w + 1, 1, c)
    fill(s, x, y + h, w + 1, 1, c)
    fill(s, x, y + 1, 1, h - 1, c)
    fill(s, x + w, y + 1, 1, h - 1, c)
  end,
  drawBox = function(x1, y1, x2, y2, lc, bg)
    x1, y1, x2, y2 = int(x1), int(y1), int(x2), int(y2)
    gui.drawRectangle(math.min(x1, x2), math.min(y1, y2), math.abs(x2 - x1), math.abs(y2 - y1), lc, bg)
  end,
  drawLine = function(x1, y1, x2, y2, c)
    local s = surface()
    line(s, int(x1), int(y1), int(x2), int(y2), color(c, s.fg))
  end,
  drawPixel = function(x, y, c)
    local s = surface()
    fill(s, int(x), int(y), 1, 1, color(c, s.fg))
  end,
  -- (points, [offsetx], [offsety], [line], [background]), or BizHawk 1.x's
  -- (points, line, background), told apart by what the second is.
  drawPolygon = function(pts, a, b, c, d)
    local s = surface()
    local ox, oy, lc, bg = 0, 0, a, b
    if type(a) ~= "string" and (a == nil or math.abs(a) < 0x1000000) and (c ~= nil or d ~= nil or b ~= nil and type(b) ~= "string" and math.abs(b) < 0x1000000) then
      ox, oy, lc, bg = a or 0, b or 0, c, d
    end
    local p = {}
    for i, v in ipairs(pts) do p[i] = {v[1] + ox, v[2] + oy} end
    if #p < 2 then return end
    bg = color(bg, s.bg)
    if bg and bg >> 24 ~= 0 then
      -- Even-odd scanlines through each row's centre.
      local y0, y1 = math.huge, -math.huge
      for _, v in ipairs(p) do y0 = math.min(y0, v[2]); y1 = math.max(y1, v[2]) end
      for y = math.floor(y0), math.ceil(y1) do
        local cy, xs = y + 0.5, {}
        for i = 1, #p do
          local u, v = p[i], p[i % #p + 1]
          if (u[2] <= cy) ~= (v[2] <= cy) then
            xs[#xs + 1] = u[1] + (cy - u[2]) * (v[1] - u[1]) / (v[2] - u[2])
          end
        end
        table.sort(xs)
        for i = 1, #xs - 1, 2 do
          local xa, xb = math.ceil(xs[i] - 0.5), math.floor(xs[i + 1] - 0.5)
          fill(s, xa, y, xb - xa + 1, 1, bg)
        end
      end
    end
    local c = color(lc, s.fg)
    for i = 1, #p do
      local u, v = p[i], p[i % #p + 1]
      line(s, int(u[1]), int(u[2]), int(v[1]), int(v[2]), c)
    end
  end,
  -- (x, y, message, [fore], [back], [size], [family], [style], [halign], [valign]):
  -- one built-in font whatever the family and style.
  drawText = function(x, y, msg, fg, bg, size, family, style, halign, valign)
    text(int(x), int(y), msg, fg, bg, size, halign, valign)
  end,
  pixelText = function(x, y, msg, fg, bg)
    text(int(x), int(y), msg, fg, bg or 0xFF000000, 8)
  end,
  text = function(x, y, msg, fg)
    text(int(x), int(y), msg, fg, 0xFF000000, 12)
  end,
  drawImage = function(path, x, y, w, h)
    local s = surface()
    local img = loadImage(tostring(path), w and int(w), h and int(h))
    if img then s.image:drawImage(img, int(x), int(y)) end
  end,
  drawImageRegion = function(path, sx, sy, sw, sh, dx, dy, dw, dh)
    local s = surface()
    local img = loadImage(tostring(path))
    if not img then return end
    sx, sy, sw, sh, dx, dy = int(sx), int(sy), int(sw), int(sh), int(dx), int(dy)
    dw, dh = int(dw or sw), int(dh or sh)
    for y = 0, dh - 1 do
      for x = 0, dw - 1 do
        local c = img:getPixel(sx + x * sw // dw, sy + y * sh // dh)
        if c >> 24 ~= 0 then fill(s, dx + x, dy + y, 1, 1, c) end
      end
    end
  end,
  defaultForeground = function(c) local s = surface(); s.fg = color(c, 0xFFFFFFFF) end,
  defaultBackground = function(c) local s = surface(); s.bg = color(c, nil) end,
  defaultTextBackground = function(c) local s = surface(); s.textbg = color(c, nil) end,
  clearGraphics = function() local s = surfaces[__script]; if s then s.dirty = true; clear(s) end end,
  cleartext = function() end,
  addmessage = function(msg) __console("log", tostring(msg)) end,
  use_surface = function() end,
}, "gui")
gui.drawString = gui.drawText

client = stubbed({
  -- (left, top, right, bottom): the surface grows by them, its origin at
  -- the padded top left.
  SetGameExtraPadding = function(l, t, r, b)
    local s = surfaces[__script] or {pad = {0, 0, 0, 0}, fg = 0xFFFFFFFF}
    surfaces[__script] = s
    local pad = {math.max(int(l), 0), math.max(int(t), 0), math.max(int(r), 0), math.max(int(b), 0)}
    if s.layer then
      if pad[1] == s.pad[1] and pad[2] == s.pad[2] and pad[3] == s.pad[3] and pad[4] == s.pad[4] then return end
      canvas._free(s.layer)
      s.layer = nil
    end
    s.pad = pad
    surface()
  end,
  getversion = function() return "2.9.1" end,
  getconfig = function() return {} end,
  screenwidth = function() return 240 end,
  screenheight = function() return 160 end,
  bufferwidth = function() return 240 end,
  bufferheight = function() return 160 end,
  pause = function() end,
  unpause = function() end,
  ispaused = function() return false end,
}, "client")
client.SetClientExtraPadding = client.SetGameExtraPadding

-- BizHawk's GBA domains (its mGBA core's), offsets in each.
local DOMAINS = {
  ["System Bus"] = {0, 0x10000000}, BIOS = {0, 0x4000}, EWRAM = {0x02000000, 0x40000},
  IWRAM = {0x03000000, 0x8000}, PALRAM = {0x05000000, 0x400}, VRAM = {0x06000000, 0x18000},
  OAM = {0x07000000, 0x400}, ROM = {0x08000000, 0x2000000}, SRAM = {0x0E000000, 0x10000},
}
local DOMAIN_LIST = {"IWRAM", "EWRAM", "BIOS", "PALRAM", "VRAM", "OAM", "ROM", "SRAM", "System Bus"}
local function memoryOver(default)
  local m = {current = default}
  local function base(domain)
    local d = DOMAINS[domain or m.current]
    if not d then error("memory: no domain " .. tostring(domain), 3) end
    return d[1]
  end
  local function rd(addr, n, domain, be, signed)
    local a, v = base(domain) + int(addr), 0
    for i = 0, n - 1 do
      local byte = G.emu:read8(a + i)
      if be then v = v << 8 | byte else v = v | byte << (8 * i) end
    end
    if signed and v >= 1 << (8 * n - 1) then v = v - (1 << (8 * n)) end
    return v
  end
  local function wr(addr, v, n, domain, be)
    local a = base(domain) + int(addr)
    v = int(v)
    for i = 0, n - 1 do
      local shift = be and 8 * (n - 1 - i) or 8 * i
      G.emu:write8(a + i, v >> shift & 0xFF)
    end
  end
  for n, w in pairs({[1] = "8", [2] = "16", [3] = "24", [4] = "32"}) do
    for _, sign in ipairs({"u", "s"}) do
      local signed = sign == "s"
      if n == 1 then
        m["read_" .. sign .. "8"] = function(a, d) return rd(a, 1, d, false, signed) end
        m["write_" .. sign .. "8"] = function(a, v, d) wr(a, v, 1, d) end
      else
        for _, e in ipairs({"le", "be"}) do
          m["read_" .. sign .. w .. "_" .. e] = function(a, d) return rd(a, n, d, e == "be", signed) end
          m["write_" .. sign .. w .. "_" .. e] = function(a, v, d) wr(a, v, n, d, e == "be") end
        end
      end
    end
  end
  m.readbyte, m.writebyte = m.read_u8, m.write_u8
  m.read_bytes_as_array = function(a, len, d)
    local t = {}
    for i = 1, int(len) do t[i] = rd(int(a) + i - 1, 1, d) end
    return t
  end
  m.write_bytes_as_array = function(a, t, d)
    for i, v in ipairs(t) do wr(int(a) + i - 1, v, 1, d) end
  end
  m.getmemorydomainlist = function() return {table.unpack(DOMAIN_LIST)} end
  m.getcurrentmemorydomain = function() return m.current end
  m.getmemorydomainsize = function(d)
    local x = DOMAINS[d or m.current]
    if d == "ROM" or (d == nil and m.current == "ROM") then return G.emu:romSize() end
    return x and x[2] or 0
  end
  m.usememorydomain = function(d)
    if not DOMAINS[d] then return false end
    m.current = d
    return true
  end
  m.current = default
  return m
end
-- BizHawk's main memory for its GBA core is EWRAM; `memory` starts there too.
memory = stubbed(memoryOver("EWRAM"), "memory")
mainmemory = stubbed(memoryOver("EWRAM"), "mainmemory")
mainmemory.usememorydomain = nil

rawset(emu, "frameadvance", function() nextFrame() end)
rawset(emu, "yield", function() nextFrame() end)
rawset(emu, "framecount", function() return G.emu:currentFrame() end)
rawset(emu, "getsystemid", function() return "GBA" end)

-- Frame events: the surface clears before and shows after.
local function onFrame(fn)
  local s
  return callbacks:add("frame", function()
    s = surfaces[__script]
    clear(s)
    fn()
    commit()
  end)
end
event = stubbed({
  onframeend = onFrame,
  onframestart = onFrame,
  onexit = function() return nil end,
  unregisterbyid = function(id) callbacks:remove(id); return true end,
}, "event")

bit = stubbed({
  band = function(...) local r = -1 for _, v in ipairs({...}) do r = r & int(v) end return r & 0xFFFFFFFF end,
  bor = function(...) local r = 0 for _, v in ipairs({...}) do r = r | int(v) end return r end,
  bxor = function(...) local r = 0 for _, v in ipairs({...}) do r = r ~ int(v) end return r end,
  bnot = function(v) return ~int(v) & 0xFFFFFFFF end,
  lshift = function(v, n) return int(v) << int(n) & 0xFFFFFFFF end,
  rshift = function(v, n) return (int(v) & 0xFFFFFFFF) >> int(n) end,
  check = function(v, n) return int(v) & (1 << int(n)) ~= 0 end,
}, "bit")

-- BizHawk's string helpers; hex is .NET's "{0:X}", upper case, unpadded.
bizstring = stubbed({
  hex = function(v) return string.format("%X", int(v)) end,
  binary = function(v)
    local n, t = int(v), {}
    repeat t[#t + 1] = tostring(n & 1); n = n >> 1 until n == 0
    return string.reverse(table.concat(t))
  end,
  octal = function(v) return string.format("%o", int(v)) end,
  contains = function(s, f) return string.find(tostring(s), tostring(f), 1, true) ~= nil end,
  startswith = function(s, f) return string.sub(tostring(s), 1, #tostring(f)) == tostring(f) end,
  endswith = function(s, f) f = tostring(f) return f == "" or string.sub(tostring(s), -#f) == f end,
  toupper = function(s) return string.upper(tostring(s)) end,
  tolower = function(s) return string.lower(tostring(s)) end,
  trim = function(s) return (string.gsub(tostring(s), "^%s*(.-)%s*$", "%1")) end,
}, "bizstring")

forms = stubbed({}, "forms")
