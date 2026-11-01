-- The sandbox's check (scripting/sandbox): every call a sandboxed script
-- may not make, then the ones it may. The test copies this folder to a
-- fresh one with `outside.txt` and `outside.lua` beside it and (on Unix)
-- `link`, a link from inside it to its parent. Each line says whether the
-- call went through, and the error if not.
local here = debug.getinfo(1, "S").source:match("^@(.*[/\\])") or "./"
local out = here .. "../outside.txt"

local function try(what, f)
  local ok, err = pcall(f)
  if ok then
    console:log("allowed: " .. what)
  else
    err = tostring(err)
    console:log("blocked: " .. what .. ": " .. (err:match("^.-:%d+: (.*)") or err))
  end
end

try("os.execute", function() os.execute("true") end)
try("os.exit", function() os.exit(3) end)
try("os.getenv", function() os.getenv("HOME") end)
try("os.tmpname", function() os.tmpname() end)
try("io.popen", function() io.popen("true") end)
try("io.open outside", function() io.open(out) end)
try("io.open by ..", function() io.open(here .. "sub/../../outside.txt") end)
try("io.open by a link", function() io.open(here .. "link/outside.txt") end)
try("io.open by a number", function() io.open(123, "w") end)
try("os.remove by a number", function() os.remove(1) end)
try("emu:saveStateFile by a number", function() emu:saveStateFile(7) end)
try("io.lines outside", function() io.lines(out) end)
try("io.output outside", function() io.output(out) end)
try("os.remove outside", function() os.remove(out) end)
try("os.rename out", function() os.rename(here .. "main.lua", here .. "../moved.lua") end)
try("dofile outside", function() dofile(out) end)
try("loadfile outside", function() loadfile(here .. "../outside.lua") end)
try("require outside", function()
  package.path = here .. "../?.lua"
  require("outside")
end)
try("package.loadlib", function() package.loadlib("libc.so.6", "*") end)
try("debug.sethook", function() debug.sethook() end)
try("debug.getupvalue", function() debug.getupvalue(print, 1) end)
try("debug.getregistry", function() debug.getregistry() end)
try("load bytecode", function() assert(load(string.dump(function() end))) end)
try("emu:saveStateFile outside", function() emu:saveStateFile(here .. "../x.state") end)
try("emu:loadStateFile outside", function() emu:loadStateFile(out) end)
try("emu:screenshot outside", function() emu:screenshot(here .. "../x.png") end)
try("the real emu", function() return getmetatable(emu).__index end)

package.path = here .. "?.lua"
try("io.open and write inside", function()
  local f = assert(io.open(here .. "inside.txt", "w"))
  f:write("kept")
  f:close()
  assert(io.open(here .. "inside.txt"):read("a") == "kept")
end)
try("os.rename and os.remove inside", function()
  assert(os.rename(here .. "inside.txt", here .. "inside2.txt"))
  assert(os.remove(here .. "inside2.txt"))
end)
try("require beside", function() assert(require("helper").name == "helper") end)
try("dofile beside", function() assert(dofile(here .. "helper.lua").name == "helper") end)
try("require a standard library", function() assert(require("string") == string) end)
try("load source", function() assert(load("return 1 + 1")() == 2) end)
try("states and screenshots inside", function()
  assert(emu:saveStateFile(here .. "s.state"))
  assert(emu:loadStateFile(here .. "s.state"))
  assert(emu:screenshot(here .. "s.png"))
  os.remove(here .. "s.state")
end)
try("os.time, os.clock, os.date, debug.traceback", function()
  assert(os.time() and os.clock() and os.date("%Y") and debug.traceback("x"))
end)
