-- scripting/input-keys' check (crates/desktop/tests/script_input.rs): run
-- headless with --script-keys holding Shift and `!` (as mGBA's window
-- gives Shift+1) and then A. A hotkey as PokeLua's Gen 3 scripts read
-- theirs, `input:isKeyActive` each frame, and the `key` events.
local was = {}
callbacks:add("frame", function()
  local now = {
    shift = input:isKeyActive(C.KEY.SHIFT),
    bang = input:isKeyActive("!") and input:isKeyActive(33),
    a = input:isKeyActive("A"),
  }
  for _, k in ipairs({"shift", "bang", "a"}) do
    if now[k] ~= was[k] then
      console:log(string.format("frame %d: %s %s", emu:currentFrame(), k,
        now[k] and "held" or "up"))
    end
  end
  if now.shift and now.bang and not (was.shift and was.bang) then
    local keys = input:activeKeys()
    table.sort(keys)
    console:log("hotkey Shift+! in frame " .. emu:currentFrame() .. ", keys "
      .. table.concat(keys, " "))
  end
  was = now
end)
callbacks:add("key", function(e)
  console:log(string.format("key %d: %X state %d mods %d", e.seq, e.key, e.state,
    e.modifiers))
end)
