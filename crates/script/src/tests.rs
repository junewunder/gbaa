use super::*;
use runtime::platform::Headless;
use std::io::Write;
use std::path::Path;

/// A ROM of zeros (an endless `andeq r0, r0, r0`) with a header.
fn rom() -> &'static [u8] {
    let mut r = vec![0u8; 0x200];
    r[0xA0..0xAC].copy_from_slice(b"TESTGAME\0\0\0\0");
    r[0xAC..0xB0].copy_from_slice(b"BTST");
    Box::leak(r.into_boxed_slice())
}

fn host() -> (Host<Headless>, Box<Machine<Headless>>) {
    host_for(rom())
}

fn host_for(rom: &'static [u8]) -> (Host<Headless>, Box<Machine<Headless>>) {
    let hooks = Hooks {
        run_frame: Box::new(|m| m.run_frame()),
        reset: Box::new(move |m| *m = Machine::new(rom, Headless)),
        load_state: Box::new(|m, b| m.load_state(&b).map_err(|e| e.to_string())),
        screenshot: Box::new(|_, _| Ok(())),
        save_slot: Box::new(|_, _| Err("no slots".into())),
        load_slot: Box::new(|_, _| Err("no slots".into())),
    };
    (
        Host::new(rom, hooks).unwrap(),
        Box::new(Machine::new(rom, Headless)),
    )
}

fn script(src: &str) -> tempfile::Script {
    tempfile::Script::new(src)
}

/// Scripts written to a fresh temporary folder.
mod tempfile {
    pub struct Script {
        pub dir: std::path::PathBuf,
        pub path: std::path::PathBuf,
    }
    impl Script {
        pub fn new(src: &str) -> Script {
            use std::sync::atomic::{AtomicU32, Ordering};
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "gbaa-script-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("main.lua");
            std::fs::write(&path, src).unwrap();
            Script { dir, path }
        }
    }
    impl Drop for Script {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

#[test]
fn reads_memory_header_and_callbacks_in_order() {
    let (h, mut m) = host();
    m.bus.write32(0x0200_0010, 0x1234_5678);
    let s = script(
        r#"
        console:log(emu:getGameTitle() .. " " .. emu:getGameCode())
        console:log(string.format("%08x %04x", emu:read32(0x02000010), emu.memory.wram:read16(0x12)))
        console:log(emu.memory.wram:base() .. " " .. emu.memory.wram:size() .. " " .. emu.memory.wram:name())
        callbacks:add("frame", function() console:log("a" .. emu:currentFrame()) end)
        local id = callbacks:add("frame", function() console:log("b") end)
        callbacks:add("frame", function() console:log("c") callbacks:remove(id) end)
        local b = console:createBuffer("Out")
        b:print("hello") b:clear() b:print("there")
        "#,
    );
    h.load(&mut m, &s.path, Access::Sandboxed);
    h.frame(&mut m, 1);
    h.frame(&mut m, 2);
    assert_eq!(
        h.console(),
        [
            "log: TESTGAME AGB-BTST",
            "log: 12345678 1234",
            "log: 33554432 262144 wram",
            "log: a1",
            "log: b",
            "log: c",
            "log: a2",
            "log: c",
        ]
    );
    assert_eq!(h.buffers(), [("Out".to_string(), "there".to_string())]);
}

#[test]
fn a_missing_name_warns_once_and_returns_nil() {
    let (h, mut m) = host();
    let s = script("for i = 1, 3 do assert(emu:setRotationCallbacks(nil) == nil) end");
    h.load(&mut m, &s.path, Access::Sandboxed);
    assert_eq!(
        h.console(),
        ["warn: emu.setRotationCallbacks is not supported by gbaa"]
    );
}

#[test]
fn keys_set_in_keys_read_are_pressed() {
    let (h, mut m) = host();
    let s = script(
        r#"callbacks:add("keysRead", function()
            if emu:getKey(C.GBA_KEY.B) == 1 then emu:addKey(C.GBA_KEY.START) end
        end)"#,
    );
    h.load(&mut m, &s.path, Access::Sandboxed);
    assert_eq!(h.keys_read(&mut m, 0), None);
    assert_eq!(h.keys_read(&mut m, 0b10), Some(0b1010));
}

#[test]
fn input_counts_presses_and_fires_key_events() {
    use input::{Event, State};
    let (h, mut m) = host();
    let s = script(
        r#"
        assert(C.KEY.SHIFT == 0x800032 and C.INPUT_STATE.HELD == 2 and C.KMOD.SHIFT == 3)
        callbacks:add("key", function(e)
          console:log(string.format("%d %d %x %d %d", e.seq, e.type, e.key, e.state, e.modifiers))
        end)
        callbacks:add("mouseMove", function(e) console:log("move " .. e.x .. " " .. e.y) end)
        callbacks:add("frame", function()
          local k = input:activeKeys()
          table.sort(k)
          console:log(string.format("%s %s %s %s seq %d [%s]", input:isKeyActive("!"),
            input:isKeyActive(33), input:isKeyActive(C.KEY.SHIFT), input:isKeyActive("!!"),
            input.seq, table.concat(k, " ")))
        end)
        "#,
    );
    h.load(&mut m, &s.path, Access::Sandboxed);
    let key = |key, state| Event::Key {
        key,
        state,
        mods: 0,
    };
    h.input(&mut m, key(input::key("SHIFT").unwrap(), State::Down));
    h.input(
        &mut m,
        Event::Key {
            key: 33,
            state: State::Down,
            mods: 3,
        },
    );
    h.input(&mut m, key(33, State::Down));
    h.frame(&mut m, 1);
    // The second release of `!` lets it go; the first fires nothing.
    h.input(&mut m, key(33, State::Up));
    h.input(&mut m, Event::MouseMove { x: 12, y: -3 });
    h.frame(&mut m, 2);
    h.input(&mut m, key(33, State::Up));
    h.input(&mut m, Event::ClearKeys);
    h.frame(&mut m, 3);
    assert_eq!(
        h.console(),
        [
            "log: 0 1 800032 1 0",
            "log: 1 1 21 1 3",
            "log: 2 1 21 2 0",
            "log: true true true false seq 3 [33 8388658]",
            "log: move 12 -3",
            "log: true true true false seq 4 [33 8388658]",
            "log: 4 1 21 0 0",
            "log: 5 1 800032 0 0",
            "log: false false false false seq 6 []",
        ]
    );
}

#[test]
fn a_runaway_callback_is_stopped_and_removed() {
    let (h, mut m) = host();
    let s = script(
        r#"callbacks:add("frame", function() while true do pcall(function() end) end end)
        callbacks:add("frame", function() console:log("still here") end)"#,
    );
    h.load(&mut m, &s.path, Access::Sandboxed);
    let t = std::time::Instant::now();
    h.frame(&mut m, 1);
    assert!(t.elapsed().as_millis() < 1000, "took {:?}", t.elapsed());
    h.frame(&mut m, 2);
    let c = h.console();
    assert!(c[0].contains("time budget"), "{c:?}");
    assert_eq!(
        &c[1..],
        [
            "error: frame callback removed",
            "log: still here",
            "log: still here"
        ]
    );
}

#[test]
fn require_and_dofile_find_files_beside_the_script() {
    let (h, mut m) = host();
    let s = script(
        r#"local helper = require("helper")
        local dir = debug.getinfo(1, "S").source:match("^@(.*[/\\])")
        dofile(dir .. "other.lua")
        console:log(helper.name .. " " .. other)"#,
    );
    let mut f = std::fs::File::create(s.dir.join("helper.lua")).unwrap();
    writeln!(f, "return {{name = 'helper'}}").unwrap();
    std::fs::write(s.dir.join("other.lua"), "other = 'other'").unwrap();
    h.load(&mut m, &s.path, Access::Sandboxed);
    assert_eq!(h.console(), ["log: helper other"]);
}

#[test]
fn a_state_buffer_round_trips_and_reset_fires() {
    let (h, mut m) = host();
    let s = script(
        r#"callbacks:add("reset", function() console:log("reset") end)
        emu:write8(0x03000000, 7)
        local st = emu:saveStateBuffer()
        emu:write8(0x03000000, 9)
        console:log(tostring(emu:loadStateBuffer(st)) .. " " .. emu:read8(0x03000000))
        emu:reset()
        console:log(emu:read8(0x03000000))
        console:log(string.format("%08x", string.unpack(">I4", emu:checksum(C.CHECKSUM.CRC32))))"#,
    );
    h.load(&mut m, &s.path, Access::Sandboxed);
    let crc = format!("log: {:08x}", runtime::state::crc32(m.bus.rom));
    assert_eq!(h.console(), ["log: true 7", "log: reset", "log: 0", &crc]);
}

/// `fixtures/sandbox/` copied into a fresh folder, with the files outside
/// it that its calls try to reach.
#[cfg(unix)]
fn sandbox_fixture() -> tempfile::Script {
    let s = script("");
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/sandbox");
    let dir = s.dir.join("script");
    std::fs::create_dir(&dir).unwrap();
    for f in ["main.lua", "helper.lua"] {
        std::fs::copy(fixtures.join(f), dir.join(f)).unwrap();
    }
    std::fs::write(s.dir.join("outside.txt"), "secret").unwrap();
    std::fs::write(s.dir.join("outside.lua"), "return 'outside'").unwrap();
    std::os::unix::fs::symlink(&s.dir, dir.join("link")).unwrap();
    s
}

#[cfg(unix)]
#[test]
fn a_sandboxed_script_gets_an_error_for_each_blocked_call() {
    let (h, mut m) = host();
    let s = sandbox_fixture();
    h.load(&mut m, &s.dir.join("script/main.lua"), Access::Sandboxed);
    let sandboxed = "the script is sandboxed (give it full access to use it)";
    let not_allowed = |name: &str| format!("{name} is not allowed: {sandboxed}");
    let outside = |name: &str, p: &str| {
        let dir = s.dir.join("script");
        format!(
            "{name}: {}/{p} is outside the script's folder ({})",
            dir.display(),
            sandboxed.replace(" (", "; ").trim_end_matches(')')
        )
    };
    let mut want = vec![];
    let mut blocked = |what: &str, why: String| want.push(format!("log: blocked: {what}: {why}"));
    for f in [
        "os.execute",
        "os.exit",
        "os.getenv",
        "os.tmpname",
        "io.popen",
    ] {
        blocked(f, not_allowed(f));
    }
    blocked("io.open outside", outside("io.open", "../outside.txt"));
    blocked("io.open by ..", outside("io.open", "sub/../../outside.txt"));
    blocked("io.open by a link", outside("io.open", "link/outside.txt"));
    // A number is a filename to Lua, here in the crate's folder, not the script's.
    let by_number = |name: &str, n: &str| {
        format!(
            "{name}: {n} is outside the script's folder ({})",
            sandboxed.replace(" (", "; ").trim_end_matches(')')
        )
    };
    blocked("io.open by a number", by_number("io.open", "123"));
    blocked("os.remove by a number", by_number("os.remove", "1"));
    blocked(
        "emu:saveStateFile by a number",
        by_number("emu:saveStateFile", "7"),
    );
    blocked("io.lines outside", outside("io.lines", "../outside.txt"));
    blocked("io.output outside", outside("io.output", "../outside.txt"));
    blocked("os.remove outside", outside("os.remove", "../outside.txt"));
    blocked("os.rename out", outside("os.rename", "../moved.lua"));
    blocked("dofile outside", outside("loadfile", "../outside.txt"));
    blocked("loadfile outside", outside("loadfile", "../outside.lua"));
    blocked(
        "require outside",
        "module 'outside' not found beside the script".into(),
    );
    blocked("package.loadlib", not_allowed("package.loadlib"));
    for f in ["debug.sethook", "debug.getupvalue", "debug.getregistry"] {
        blocked(f, not_allowed(f));
    }
    blocked(
        "load bytecode",
        "attempt to load a binary chunk (mode is 't')".into(),
    );
    blocked(
        "emu:saveStateFile outside",
        outside("emu:saveStateFile", "../x.state"),
    );
    blocked(
        "emu:loadStateFile outside",
        outside("emu:loadStateFile", "../outside.txt"),
    );
    blocked(
        "emu:screenshot outside",
        outside("emu:screenshot", "../x.png"),
    );
    blocked("the real emu", "attempt to index a boolean value".into());
    for a in [
        "io.open and write inside",
        "os.rename and os.remove inside",
        "require beside",
        "dofile beside",
        "require a standard library",
        "load source",
        "states and screenshots inside",
        "os.time, os.clock, os.date, debug.traceback",
    ] {
        want.push(format!("log: allowed: {a}"));
    }
    let got = h.console();
    for (g, w) in got.iter().zip(&want) {
        assert_eq!(g, w);
    }
    assert_eq!(got.len(), want.len(), "{got:#?}");
    assert_eq!(
        std::fs::read_to_string(s.dir.join("outside.txt")).unwrap(),
        "secret"
    );
    assert!(!s.dir.join("x.state").exists());
    assert!(!std::path::Path::new("123").exists() && !std::path::Path::new("7").exists());
}

#[test]
fn full_access_has_everything_as_mgba() {
    let (h, mut m) = host();
    let s = script(
        r#"local f = assert(io.open(os.tmpname(), "w")) f:close()
        console:log(type(os.execute) .. " " .. type(io.popen) .. " " .. type(debug.sethook)
          .. " " .. type(package.loadlib) .. " " .. type(os.getenv("PATH")))"#,
    );
    h.load(&mut m, &s.path, Access::Full);
    assert_eq!(
        h.console(),
        ["log: function function function function string"]
    );
}

#[test]
fn each_script_has_its_own_globals() {
    let (h, mut m) = host();
    let a = script("shared = 'a' io = nil");
    let b = script("console:log(tostring(shared) .. ' ' .. type(io.open))");
    h.load(&mut m, &a.path, Access::Sandboxed);
    h.load(&mut m, &b.path, Access::Full);
    assert_eq!(h.console(), ["log: nil function"]);
}

#[test]
fn the_prompt_calls_what_scripts_define() {
    let (h, mut m) = host();
    // Sandboxed, its env holds a copy of every global: only what it set
    // itself is found through it.
    let s = script(
        r#"
        count = 0
        function HELP() console:log("help " .. count) count = count + 1 end
        "#,
    );
    h.load(&mut m, &s.path, Access::Sandboxed);
    h.eval(&mut m, "HELP()");
    h.eval(&mut m, "count, emu:getGameCode()");
    h.eval(&mut m, "x = 5");
    h.eval(&mut m, "x * 2");
    h.eval(&mut m, "nope(");
    h.eval(&mut m, "io.open ~= nil");
    let c = h.console();
    assert_eq!(
        c[..9],
        [
            "> HELP()",
            "log: help 0",
            "> count, emu:getGameCode()",
            "log: 1\tAGB-BTST",
            "> x = 5",
            "> x * 2",
            "log: 10",
            "> nope(",
            c[8].as_str(),
        ]
    );
    assert!(
        c[8].starts_with("error: ") && c[8].contains("prompt"),
        "{c:?}"
    );
    assert_eq!(c[9..], ["> io.open ~= nil", "log: true"]);
}

#[test]
fn unloading_drops_a_scripts_callbacks_and_buffers() {
    let (h, mut m) = host();
    let a = script(
        r#"
        local b = console:createBuffer("A")
        callbacks:add("frame", function()
            b:print("a")
            -- Added from its callback: still script a's.
            if emu:currentFrame() == 1 then
                callbacks:add("frame", function() console:log("a late") end)
            end
        end)
        "#,
    );
    let b = script(
        r#"console:createBuffer("B"):print("b") callbacks:add("frame", function() console:log("b") end)"#,
    );
    let ia = h.load(&mut m, &a.path, Access::Sandboxed);
    let ib = h.load(&mut m, &b.path, Access::Sandboxed);
    assert_ne!(ia, ib);
    h.eval(
        &mut m,
        r#"callbacks:add("frame", function() console:log("prompt") end)"#,
    );
    h.frame(&mut m, 1);
    h.frame(&mut m, 2);
    assert_eq!(h.buffers().len(), 2);
    h.unload(ia);
    assert_eq!(h.scripts(), [(ib, b.path.clone())]);
    assert_eq!(h.buffers(), [("B".to_string(), "b".to_string())]);
    let before = h.console().len();
    h.frame(&mut m, 3);
    assert_eq!(h.console()[before..], ["log: b", "log: prompt"]);
}

/// A loopback port nothing listens on (just now).
fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

/// Like mGBA's `res/scripts/socketserver.lua`: accept from a `received`
/// callback, log what each client sends and answer it.
const ECHO_SERVER: &str = r#"
local server, err = socket.bind(nil, PORT)
assert(server, err)
assert(server:listen())
local clients = {}
server:add("received", function()
  local c, err = server:accept()
  if err then console:error("accept: " .. err) return end
  clients[#clients + 1] = c
  c:add("received", function()
    while true do
      local p, err = c:receive(1024)
      if p then
        console:log("got " .. p:match("^(.-)%s*$"))
        c:send("echo " .. p)
      else
        if err ~= socket.ERRORS.AGAIN then console:log("closed: " .. err) c:close() end
        return
      end
    end
  end)
  console:log("connected")
end)
console:log("listening")
"#;

#[test]
fn a_sandboxed_socket_server_answers_a_loopback_client() {
    use std::io::{BufRead, BufReader, Write};
    let (h, mut m) = host();
    let port = free_port();
    let s = script(&ECHO_SERVER.replace("PORT", &port.to_string()));
    h.load(&mut m, &s.path, Access::Sandboxed);
    let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    h.frame(&mut m, 1);
    c.write_all(b"hello\n").unwrap();
    // Each frame polls; the data may take one to arrive.
    for n in 2..50 {
        h.frame(&mut m, n);
        if h.console().len() >= 3 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let mut line = String::new();
    BufReader::new(&c).read_line(&mut line).unwrap();
    assert_eq!(line, "echo hello\n");
    drop(c);
    for n in 50..100 {
        h.frame(&mut m, n);
        if h.console().len() >= 4 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        h.console(),
        [
            "log: listening",
            "log: connected",
            "log: got hello",
            "log: closed: disconnected"
        ]
    );
}

#[test]
fn a_script_connects_out_and_sends() {
    use std::io::Read;
    let (h, mut m) = host();
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let s = script(&format!(
        r#"
        local s = assert(socket.connect("localhost", {port}))
        console:log(tostring(s:send("ping", 2)))
        local p, err = s:receive(10)
        console:log(tostring(p) .. " " .. err .. " " .. tostring(err == socket.ERRORS.AGAIN))
        console:log(tostring(s:hasdata()))
        s:close()"#
    ));
    h.load(&mut m, &s.path, Access::Sandboxed);
    let (mut c, _) = l.accept().unwrap();
    let mut got = String::new();
    c.read_to_string(&mut got).unwrap();
    assert_eq!(got, "ing");
    assert_eq!(
        h.console(),
        ["log: 4", "log: nil temporary failure true", "log: false"]
    );
}

#[test]
fn sandboxed_sockets_reach_loopback_only() {
    let (h, mut m) = host();
    let port = free_port();
    let src = format!(
        r#"
        console:log(tostring(select(2, socket.connect("10.255.255.1", 80))))
        console:log(tostring(select(2, socket.bind("10.255.255.1", 80))))
        console:log(tostring(_socket.create():connect("10.255.255.1", 80)))
        console:log(tostring(select(2, socket.connect("127.0.0.1", {port}))))
        console:log(tostring(C.SOCKERR.DENIED) .. " " .. socket.ERRORS.DENIED)
        console:log(tostring(__prompt) .. " " .. tostring(getmetatable(socket)))"#
    );
    let s = script(&src);
    h.load(&mut m, &s.path, Access::Sandboxed);
    assert_eq!(
        h.console(),
        [
            "log: access denied",
            "log: access denied",
            "log: 4",
            "log: connection refused",
            "log: 4 access denied",
            "log: nil nil",
        ]
    );
}

/// A sandboxed script's `socket` has method tables of its own: swapping
/// one, or a socket's metatable, can't reach a full-access script's sockets.
#[test]
fn a_sandboxed_script_cant_swap_a_full_scripts_socket_methods() {
    use std::io::{BufRead, BufReader, Write};
    let (h, mut m) = host();
    let port = free_port();
    let full = script(&ECHO_SERVER.replace("PORT", &port.to_string()));
    h.load(&mut m, &full.path, Access::Full);
    let evil = script(
        r#"
        local hijack = function(self) console:log("hijacked") end
        for _, k in ipairs({"poll", "receive", "send", "accept", "_hook", "_dispatch"}) do
          socket._tcpMT.__index[k] = hijack
          socket._mt.__index[k] = hijack
        end
        socket._wrap = hijack
        console:log(tostring(getmetatable(_socket.create())))
        console:log(tostring(pcall(function() getmetatable(_socket.create()).__index = hijack end)))"#,
    );
    h.load(&mut m, &evil.path, Access::Sandboxed);
    let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    h.frame(&mut m, 1);
    c.write_all(b"hello\n").unwrap();
    for n in 2..50 {
        h.frame(&mut m, n);
        if h.console().len() >= 5 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let mut line = String::new();
    BufReader::new(&c).read_line(&mut line).unwrap();
    assert_eq!(line, "echo hello\n");
    let console = h.console();
    assert!(
        !console.iter().any(|l| l.contains("hijacked")),
        "{console:?}"
    );
    assert_eq!(console[..2], ["log: listening", "log: false"]);
    assert_eq!(console[3..], ["log: connected", "log: got hello"]);
}

#[test]
fn unloading_closes_a_scripts_sockets() {
    let (h, mut m) = host();
    let port = free_port();
    let s = script(&ECHO_SERVER.replace("PORT", &port.to_string()));
    let id = h.load(&mut m, &s.path, Access::Full);
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_ok());
    h.unload(id);
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
}

#[test]
fn a_bucket_persists_across_hosts() {
    let folder = script("");
    let dir = folder.dir.join("storage");
    let write = script(
        r#"
        local b = storage:getBucket("test.bucket")
        b.n = 42
        b.f = 1.5
        b.s = "hi\n"
        b.list = {1, 2, {x = true}}
        b.t = {name = "x"}
        b.t.added = 7
        local t = b.t
        t.later = "yes"
        console:log(tostring(b.flush == nil) .. " " .. b.n .. " " .. b.t.added)
        for _, bad in ipairs({".x", "a/b", "a-b", "a b", string.rep("a", 65)}) do
          console:log(tostring(storage:getBucket(bad)))
        end
        -- mGBA's check passes an empty name (as ".json") and dots after the first.
        console:log(tostring(storage:getBucket(string.rep("a", 64)) ~= nil and storage:getBucket("") ~= nil))
        local function try(f) local ok, e = pcall(f) console:log(tostring(ok) .. " " .. tostring(e)) end
        try(function() b.f = print end)
        try(function() b.f = {[2] = 1} end)
        local loop = {} loop.me = loop
        try(function() b.f = loop end)
        local q = storage:getBucket("quiet")
        q:enableAutoFlush(false)
        q.x = 1
        q:enableAutoFlush(false)
        "#,
    );
    {
        let (h, mut m) = host();
        h.set_storage(Some(dir.clone()));
        h.load(&mut m, &write.path, Access::Sandboxed);
        let c = h.console();
        assert_eq!(c[0], "log: false 42 7", "{c:?}");
        assert_eq!(&c[1..6], ["log: nil"; 5]);
        assert_eq!(c[6], "log: true");
        assert!(
            c[7].starts_with("log: false") && c[7].contains("a function"),
            "{c:?}"
        );
        assert!(
            c[8].starts_with("log: false") && c[8].contains("1..n"),
            "{c:?}"
        );
        assert!(
            c[9].starts_with("log: false") && c[9].contains("itself"),
            "{c:?}"
        );
        // Nothing yet: flushes come every FLUSH_EVERY frames, or at the end.
        assert!(!dir.join("test.bucket.json").exists());
        h.frame(&mut m, storage::FLUSH_EVERY);
        assert!(dir.join("test.bucket.json").exists());
        // Autoflush off: not on the timer, but at the end, being dirty.
        assert!(!dir.join("quiet.json").exists());
    }
    assert!(dir.join("quiet.json").exists());
    let read = script(
        r#"
        local b = storage:getBucket("test.bucket")
        console:log(string.format("%d %s %q %d %d %s %s %s %s", b.n, math.type(b.f) .. b.f,
          b.s, #b.list, b.list[2], tostring(b.list[3].x), b.t.name, b.t.added, b.t.later))
        console:log(tostring(storage:getBucket("quiet").x) .. " " .. tostring(b.missing))
        b.n = nil
        console:log(tostring(b:reload()) .. " " .. b.n)
        b.n = 1
        console:log(tostring(b:flush()))
        b.n = 2
        console:log(tostring(b:reload()) .. " " .. b.n)
        console:log(tostring(storage:getBucket("nofile"):reload()))
        "#,
    );
    let (h, mut m) = host();
    h.set_storage(Some(dir.clone()));
    h.load(&mut m, &read.path, Access::Sandboxed);
    assert_eq!(
        h.console(),
        [
            "log: 42 float1.5 \"hi\\\n\" 3 2 true x 7 yes",
            "log: 1 nil",
            "log: true 42",
            "log: true",
            "log: true 1",
            "log: false",
        ]
    );
}

#[test]
fn without_a_folder_buckets_live_in_memory() {
    let (h, mut m) = host();
    let s = script(
        r#"
        local b = storage:getBucket("mem")
        b.x = 3
        console:log(tostring(b:flush()) .. " " .. storage:getBucket("mem").x)
        "#,
    );
    h.load(&mut m, &s.path, Access::Full);
    assert_eq!(h.console(), ["log: false 3"]);
}

/// `rom()` running: r0 = EWRAM, r1 = 5; `strb r1, [r0]` at +8 (a
/// breakpoint), `ldr r2, [r0]`, `str r1, [r0, #4]` twice, then `b .`.
fn watch_rom() -> &'static [u8] {
    let mut r = rom().to_vec();
    let code: [u32; 7] = [
        0xE3A0_0402,
        0xE3A0_1005,
        0xE5C0_1000,
        0xE590_2000,
        0xE580_1004,
        0xE580_1004,
        0xEAFF_FFFE,
    ];
    for (i, w) in code.iter().enumerate() {
        r[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
    }
    Box::leak(r.into_boxed_slice())
}

#[test]
fn breakpoints_and_watchpoints_fire_in_the_interpreter() {
    let (h, mut m) = host_for(watch_rom());
    let s = script(
        r#"
        local function show(what)
          return function(i)
            local t = {}
            for _, k in ipairs({"address", "width", "segment", "oldValue", "newValue", "accessType"}) do
              if i[k] then t[#t + 1] = k .. "=" .. string.format("%x", i[k]) end
            end
            console:log(what .. " " .. table.concat(t, " ") .. " r1=" .. emu:readRegister("r1"))
          end
        end
        bp = emu:setBreakpoint(show("break"), 0x08000008)
        emu:setWatchpoint(show("write"), 0x02000000, C.WATCHPOINT_TYPE.WRITE)
        emu:setRangeWatchpoint(show("read"), 0x02000000, 0x02000008, C.WATCHPOINT_TYPE.READ)
        emu:setWatchpoint(show("change"), 0x02000005, C.WATCHPOINT_TYPE.WRITE_CHANGE)
        local never = emu:setBreakpoint(show("never"), 0x0800000C)
        console:log(tostring(emu:clearBreakpoint(never)) .. " " .. tostring(emu:clearBreakpoint(never)))
        "#,
    );
    h.load(&mut m, &s.path, Access::Sandboxed);
    assert!(h.has_points());
    h.run_frame_debug(&mut m);
    assert_eq!(
        h.console(),
        [
            "log: true false",
            "log: break address=8000008 segment=0 r1=5",
            "log: write address=2000000 width=1 segment=0 oldValue=0 newValue=5 accessType=1 r1=5",
            "log: read address=2000000 width=4 segment=0 oldValue=5 accessType=2 r1=5",
            "log: change address=2000004 width=4 segment=0 oldValue=0 newValue=5 accessType=1 r1=5",
        ]
    );
    // Unloading clears its points; the frames run compiled again.
    h.unload(1);
    assert!(!h.has_points());
}

#[test]
fn painters_draw_as_mgba_does() {
    assert_eq!(canvas::mix(0x80FF_0000, 0xFF00_00FF), 0xFF80_007F);
    let (h, mut m) = host();
    let s = script(
        r#"
        local function px(img, ...)
          local t = {}
          for _, xy in ipairs({...}) do t[#t + 1] = string.format("%08x", img:getPixel(xy[1], xy[2])) end
          console:log(table.concat(t, " "))
        end
        local img = image.new(8, 8)
        local p = image.newPainter(img)
        p:setFill(true) p:setFillColor(0xFF00FF00)
        p:setStrokeWidth(1) p:setStrokeColor(0xFFFF0000)
        p:drawRectangle(1, 1, 4, 4)
        px(img, {1, 1}, {2, 2}, {4, 4}, {5, 5}, {0, 0})
        p:drawLine(0, 7, 7.9, 7)
        px(img, {0, 7}, {7, 7})
        local c = image.new(5, 5)
        local q = image.newPainter(c)
        q:setFill(true) q:setFillColor(0xFF0000FF)
        q:drawCircle(0, 0, 5)
        px(c, {2, 2}, {0, 2}, {0, 0})
        local bg = image.new(2, 1)
        bg:setPixel(0, 0, 0xFF0000FF)
        local b = image.newPainter(bg)
        b:setFill(true) b:setBlend(true) b:setFillColor(0x80FF0000)
        b:drawRectangle(0, 0, 1, 1)
        local mask = image.new(1, 1)
        mask:setPixel(0, 0, 0xFF808080)
        b:setFillColor(0xFFFFFFFF)
        b:drawMask(mask, 1, 0)
        px(bg, {0, 0}, {1, 0})
        img:drawImage(bg, 6, 0, 0.5)
        px(img, {6, 0}, {7, 0}, {-1, 0})
        console:log(img.width .. "x" .. img.height .. " " .. tostring(image.new(10000, 1)))
        "#,
    );
    h.load(&mut m, &s.path, Access::Sandboxed);
    assert_eq!(
        h.console(),
        [
            "log: ffff0000 ff00ff00 ffff0000 00000000 00000000",
            "log: ffff0000 ffff0000",
            "log: ff0000ff ff0000ff 00000000",
            "log: ff80007f ff808080",
            "log: 7f80007f 7f808080 00000000",
            "log: 8x8 nil",
        ]
    );
}

#[test]
fn a_layer_shows_what_it_held_at_the_update_after_layer_update() {
    let (h, mut m) = host();
    assert_eq!(h.overlay(), None);
    let s = script(
        r#"
        layer = canvas:newLayer(2, 2)
        layer:setPosition(3, 4)
        layer.image:setPixel(1, 1, 0xFFFFFFFF)
        layer:update()
        console:log(canvas:screenWidth() .. "x" .. canvas:screenHeight() .. " " .. layer.x .. "," .. layer.y)
        console:log(script.path:sub(-8) .. " " .. tostring(script.dir ~= nil))
        "#,
    );
    let id = h.load(&mut m, &s.path, Access::Sandboxed);
    assert_eq!(h.console(), ["log: 240x160 3,4", "log: main.lua true"]);
    let at = |x: usize, y: usize| y * 240 + x;
    // Made blank at 0, 0; changed only at the next frame's update.
    assert!(h.overlay().unwrap().px.iter().all(|&c| c == 0));
    h.frame(&mut m, 1);
    let o = h.overlay().unwrap();
    assert_eq!((o.width, o.left), (240, 0));
    assert_eq!(o.px[at(4, 5)], 0xFFFF_FFFF);
    assert_eq!(o.px.iter().filter(|&&c| c != 0).count(), 1);
    h.eval(&mut m, "layer:setPosition(0, 0)");
    assert_eq!(h.overlay().unwrap().px[at(4, 5)], 0xFFFF_FFFF);
    h.frame(&mut m, 2);
    assert_eq!(h.overlay().unwrap().px[at(1, 1)], 0xFFFF_FFFF);
    // A handle kept past the unload reaches no layer.
    h.eval(&mut m, "keep = layer");
    h.unload(id);
    assert_eq!(h.overlay(), None);
    h.eval(&mut m, "keep:update()");
    assert!(h.console().last().unwrap().contains("unloaded"));
}

#[test]
fn a_layer_past_the_picture_makes_a_sidebar() {
    let (h, mut m) = host();
    let s = script(
        r#"
        right = canvas:newLayer(100, 200)
        right:setPosition(230, -10)
        right.image:setPixel(99, 10, 0xFFFF0000)
        right:update()
        "#,
    );
    h.load(&mut m, &s.path, Access::Sandboxed);
    h.frame(&mut m, 1);
    let o = h.overlay().unwrap();
    // 90 past the right edge; the rows past the picture's are clipped.
    assert_eq!(
        (o.width, o.left, o.right(), o.px.len()),
        (330, 0, 90, 330 * 160)
    );
    assert_eq!(o.px[329], 0xFFFF_0000);
    // One left too, and one far off the picture's rows adds nothing.
    h.eval(
        &mut m,
        r#"
        left = canvas:newLayer(20, 20)
        left:setPosition(-20, 50)
        left.image:setPixel(0, 0, 0xFF00FF00)
        left:update()
        below = canvas:newLayer(10, 10)
        below:setPosition(-500, 160)
        below:update()
        "#,
    );
    h.frame(&mut m, 2);
    let o = h.overlay().unwrap();
    assert_eq!((o.width, o.left, o.right()), (350, 20, 90));
    assert_eq!(o.px[50 * 350], 0xFF00_FF00);
    assert_eq!(o.px[349], 0xFFFF_0000);
    // A sidebar is at most the picture's width.
    h.eval(&mut m, "left:setPosition(-5000, 0) left:update()");
    h.frame(&mut m, 3);
    assert_eq!(h.overlay().unwrap().left, canvas::SIDEBAR_MAX);
}

#[test]
fn a_sandboxed_script_loads_images_and_fonts_only_from_its_folder() {
    let (h, mut m) = host();
    let src = r#"
        local p = image.newPainter(image.new(1, 1))
        for _, f in ipairs({
          function() return image.load("/etc/hostname") end,
          function() return p:loadFont("/etc/hostname") end,
          function() return image.new(1, 1):save("/tmp/gbaa-canvas-test.png") end,
          function() return image.load(script.dir .. "/missing.png") end,
        }) do
          local ok, err = pcall(f)
          console:log(tostring(ok) .. " " .. tostring(tostring(err):match("outside the script's folder") ~= nil))
        end
    "#;
    let s = script(src);
    h.load(&mut m, &s.path, Access::Sandboxed);
    assert_eq!(
        h.console(),
        [
            "log: false true",
            "log: false true",
            "log: false true",
            "log: true false"
        ]
    );
    let (h, mut m) = host();
    h.load(&mut m, &s.path, Access::Full);
    assert_eq!(h.console(), ["log: true false"; 4]);
}

#[test]
fn load_state_slot_keeps_the_cartridge_save_unless_asked() {
    let rom = rom();
    let slots = Rc::new(RefCell::new(std::collections::HashMap::new()));
    let (save, load) = (slots.clone(), slots.clone());
    let hooks = Hooks {
        run_frame: Box::new(|m| m.run_frame()),
        reset: Box::new(move |m| *m = Machine::new(rom, Headless)),
        load_state: Box::new(|m, b| m.load_state(&b).map_err(|e| e.to_string())),
        screenshot: Box::new(|_, _| Ok(())),
        save_slot: Box::new(move |m, n| {
            save.borrow_mut().insert(n, save_state(m));
            Ok(())
        }),
        load_slot: Box::new(move |m, n| {
            let b = load.borrow().get(&n).cloned().ok_or("empty")?;
            m.load_state(&b).map_err(|e| e.to_string())
        }),
    };
    let h = Host::new(rom, hooks).unwrap();
    let mut m = Box::new(Machine::new(rom, Headless));
    m.bus.save.data[0] = 1;
    h.eval(&mut m, "emu:saveStateSlot(3)");
    m.bus.save.data[0] = 2;
    h.eval(&mut m, "emu:loadStateSlot(3)");
    assert_eq!(m.bus.save.data[0], 2, "the default keeps the save");
    h.eval(&mut m, "emu:loadStateSlot(3, C.SAVESTATE.ALL)");
    assert_eq!(m.bus.save.data[0], 1, "SAVEDATA puts the state's back");
    h.eval(&mut m, "emu:loadStateSlot(4)");
    let console = h.console();
    let said: Vec<&str> = console
        .iter()
        .skip(1)
        .step_by(2)
        .map(String::as_str)
        .collect();
    assert_eq!(
        said,
        ["log: true"; 3]
            .into_iter()
            .chain(["log: false"])
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_bizhawk_script_draws_beside_the_picture_a_frame_at_a_time() {
    let (h, mut m) = host();
    m.bus.write32(0x0200_0010, 0x1234_5678);
    let s = script(
        r#"
        client.SetGameExtraPadding(0, 10, 100, 0)
        gui.defaultTextBackground(0)
        local img = image.new(2, 2)
        img:setPixel(0, 0, 0xFF00FF00) img:setPixel(1, 1, 0xFF00FF00)
        img:save(script.dir .. "/dot.png")
        local n = 0
        while true do
          n = n + 1
          gui.drawRectangle(245, 20, 10, 10, 0xFFFF0000, 0xFF0000FF)
          gui.drawLine(240, 40, 249, 40, "white")
          gui.drawPolygon({{260, 20}, {260, 30}, {270, 25}}, "yellow", "yellow")
          gui.drawImage("dot.png", 300, 20)
          gui.drawImage("dot.png", 300, 40, 4, 4)
          gui.drawText(250, 60, "Hi", "white", 0xFF000000, 12)
          if n == 1 then gui.drawPixel(5, 15, 0xFFFFFFFF) end
          console.log(string.format("%d %08x %04x %02x %s", n, memory.read_u32_le(0x10),
            memory.read_u16_be(0x10, "EWRAM"), mainmemory.read_u8(0x11), tostring(input.getmouse().Left)))
          emu.frameadvance()
        end
        "#,
    );
    h.load(&mut m, &s.path, Access::Sandboxed);
    assert_eq!(h.console(), ["log: 1 12345678 7856 56 false"]);
    // The padded surface's origin is its top left: 10 rows above the
    // picture (clipped), 100 columns right of it (a sidebar).
    h.frame(&mut m, 1);
    let o = h.overlay().unwrap();
    assert_eq!((o.width, o.left, o.right()), (340, 0, 100));
    let at = |x: usize, y: usize| y * 340 + x;
    assert_eq!(o.px[at(5, 5)], 0xFFFF_FFFF);
    h.input(
        &mut m,
        input::Event::MouseButton {
            button: 0,
            down: true,
        },
    );
    h.frame(&mut m, 2);
    let o = h.overlay().unwrap();
    // Each frame starts clear: the first frame's pixel is gone.
    assert_eq!(o.px[at(5, 5)], 0);
    // GDI+'s rectangle: an 11 by 11 outline, filled inside.
    assert_eq!(o.px[at(245, 10)], 0xFFFF_0000);
    assert_eq!(o.px[at(255, 20)], 0xFFFF_0000);
    assert_eq!(o.px[at(250, 15)], 0xFF00_00FF);
    assert_eq!(o.px[at(256, 15)], 0);
    assert_eq!(o.px[at(244, 30)], 0xFFFF_FFFF);
    assert_eq!(o.px[at(263, 15)], 0xFFFF_FF00);
    assert_eq!(o.px[at(300, 10)], 0xFF00_FF00);
    assert_eq!(o.px[at(301, 10)], 0);
    assert_eq!(o.px[at(301, 11)], 0xFF00_FF00);
    // Drawn at 4 by 4: each pixel doubled.
    assert_eq!(o.px[at(301, 31)], 0xFF00_FF00);
    assert_eq!(o.px[at(302, 32)], 0xFF00_FF00);
    assert_eq!(o.px[at(302, 30)], 0);
    let text = (250..270)
        .flat_map(|x| (50..65).map(move |y| (x, y)))
        .filter(|&(x, y)| o.px[at(x, y)] >> 16 & 0xFF > 0x80)
        .count();
    // On its black background.
    assert_eq!(o.px[at(250, 50)], 0xFF00_0000);
    assert!(text > 10, "{text} light pixels of text");
    assert_eq!(
        h.console()[1..],
        [
            "log: 2 12345678 7856 56 false",
            "log: 3 12345678 7856 56 true"
        ]
    );
}

#[test]
fn bizstring_formats_as_bizhawk_does() {
    let (h, mut m) = host();
    let s = script(
        r#"
        console.log(bizstring.hex(0xbeef) .. " " .. bizstring.binary(5) .. " "
          .. tostring(bizstring.contains("a.b", ".")) .. " " .. bizstring.trim("  x "))
        "#,
    );
    h.load(&mut m, &s.path, Access::Sandboxed);
    assert_eq!(h.console(), ["log: BEEF 101 true x"]);
}

#[test]
fn a_gif_loads_as_its_first_frame() {
    let s = script("");
    let path = s.dir.join("a.gif");
    let mut out = vec![];
    {
        let mut e = gif::Encoder::new(&mut out, 2, 1, &[0, 0, 0, 255, 0, 0]).unwrap();
        let mut f = gif::Frame::from_indexed_pixels(2, 1, vec![0, 1], Some(0));
        f.palette = None;
        e.write_frame(&f).unwrap();
    }
    std::fs::write(&path, out).unwrap();
    let img = canvas::Image::load(&path).unwrap();
    assert_eq!((img.width, img.height), (2, 1));
    assert_eq!(img.px, [0, 0xFFFF_0000]);
}
