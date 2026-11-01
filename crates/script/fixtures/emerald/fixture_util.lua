-- Loaded by main.lua with `require`, from beside it (as mcp-mgba loads its
-- json.lua).
local M = {}

function M.hex(s)
  return (s:gsub(".", function(c) return string.format("%02X", c:byte()) end))
end

return M
