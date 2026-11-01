-- scripting/storage's check (crates/desktop/tests/script_storage.rs): run
-- headless twice with the same config folder. The first run finds the
-- bucket empty and fills it; the second must read it all back.
local b = storage:getBucket("gbaa_storage_check")
if b.runs == nil then
  b.runs = 1
  b.saved = {name = "gbaa", list = {1, 2, 3}, half = 0.5, yes = true}
  b.saved.list[4] = 4 -- in place: the stored table itself
  console:log("run 1: wrote the bucket")
else
  b.runs = b.runs + 1
  local s = b.saved
  console:log(string.format("run %d: %s %s %s %s %s", b.runs, s.name,
    table.concat(s.list, ","), s.half, s.yes, math.type(b.runs)))
end
