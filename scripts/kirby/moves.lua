-- Kirby & the Amazing Mirror (USA): drives player 1's Kirby through a fixed list of moves and logs
-- its state every frame (research/kirby-controller-probe, docs/research/kirby-controller.md).
-- Run by `just kirby-assets`. The moves start once Kirby first stands (stateFn = KirbyStand), so
-- the menu presses before it only have to reach the first room.
--
-- Each console line: frame, move label, then struct Kirby fields of gKirbys[0] (the katam decomp's
-- layout): st stateFn (+0x78), x y position (+0x40, +0x44; 1/256 px, y down), vx vy speed (+0x50,
-- +0x52; 1/256 px a frame, vy up), anim animationIndex (+0xD4), keys held as the game read them
-- (movementState, +0x118) and newly pressed (+0x11A), fl flags (+0x08; bit 0 facing left), c the
-- state's counter (+0x04), box the body hitbox (+0x38: left, top, half width, half height), spr
-- the sprite's animId/variant (+0x1C, +0x2A) and hb the animation's own hitbox slot 0 (+0x30:
-- index, left, top, right, bottom; index -1 when the animation sets none).

local K = 0x02020EE0
local STAND = 0x0803FF65
local A, B, R, L, D = 1, 2, 16, 32, 128

-- {label, frames, keys held}
local moves = {
  {"rest", 300, 0},
  {"walk-right", 60, R}, {"rest", 30, 0},
  {"dash-right", 3, R}, {"dash-gap", 3, 0}, {"dash-right", 45, R}, {"rest", 40, 0},
  {"walk-left", 40, L}, {"rest", 30, 0},
  {"tap-jump", 3, A}, {"rest", 60, 0},
  {"hold-jump", 30, A}, {"rest", 60, 0},
  {"float-jump", 3, A}, {"float-gap", 8, 0},
  {"float-1", 3, A}, {"float-gap", 8, 0}, {"float-2", 3, A}, {"float-gap", 8, 0},
  {"float-3", 3, A}, {"float-gap", 8, 0}, {"float-4", 3, A}, {"float-gap", 8, 0},
  {"float-hold", 40, A}, {"float-drift", 30, 0},
  {"puff-b", 3, B}, {"rest", 80, 0},
  {"crouch", 30, D}, {"rest", 30, 0},
  {"slide", 3, D}, {"slide", 3, D | A}, {"slide-hold", 20, D}, {"rest", 50, 0},
  {"inhale", 50, B}, {"rest", 60, 0},
  {"run-jump", 20, L}, {"run-jump", 3, L | A}, {"run-jump", 20, L}, {"rest", 60, 0},
}

local f, start, label = 0, nil, "-"

local function at(t)
  for _, m in ipairs(moves) do
    if t < m[2] then return m[1], m[3] end
    t = t - m[2]
  end
  return "end", 0
end

local function s8(v) if v >= 0x80 then return v - 0x100 end return v end
local function s16(v) if v >= 0x8000 then return v - 0x10000 end return v end
local function s32(v) if v >= 0x80000000 then return v - 0x100000000 end return v end

callbacks:add("keysRead", function()
  if start then
    local keys
    label, keys = at(f - start)
    emu:setKeys(keys)
  end
end)

callbacks:add("frame", function()
  f = f + 1
  local st = emu:read32(K + 0x78)
  if not start and st == STAND then start = f + 1 end
  console:log(string.format(
    "%d %s st=%08x x=%d y=%d vx=%d vy=%d anim=%d keys=%04x new=%04x fl=%08x c=%d box=%d,%d,%d,%d spr=%d/%d hb=%d:%d,%d,%d,%d",
    f, label, st, s32(emu:read32(K + 0x40)), s32(emu:read32(K + 0x44)),
    s16(emu:read16(K + 0x50)), s16(emu:read16(K + 0x52)), emu:read16(K + 0xD4),
    emu:read16(K + 0x118), emu:read16(K + 0x11A), emu:read32(K + 0x08), s16(emu:read16(K + 0x04)),
    s8(emu:read8(K + 0x38)), s8(emu:read8(K + 0x39)), s8(emu:read8(K + 0x3A)), s8(emu:read8(K + 0x3B)),
    emu:read16(K + 0x1C), emu:read8(K + 0x2A), s32(emu:read32(K + 0x30)),
    s8(emu:read8(K + 0x34)), s8(emu:read8(K + 0x35)), s8(emu:read8(K + 0x36)), s8(emu:read8(K + 0x37))))
end)
