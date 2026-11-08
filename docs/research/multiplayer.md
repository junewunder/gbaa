# Multiplayer: a server, planned beyond the GBA

Research for the unit research/multiplayer: what multiplayer means for the
recompiled games, how emulators and fan projects do it, what our timing
model allows, and a server architecture with a console-neutral core and a
per-console adapter, so that a later recompiled GB, NDS or GameCube fits.
Researched 2026-10-23 on quack, from the pokeemerald decomp
(`$GBAA_DATA/src/pokeemerald`), our runtime, the Emerald hacks on quack and
the web (sources at the end).

## Summary

- **Recommendation: every peer runs every console in the session.** Only
  inputs cross the network; the link cable between the consoles is
  emulated inside each peer's process, exactly and deterministically. This
  is how Dolphin's integrated GBA does GameCube–GBA link over netplay, and
  it fits us better than anyone: our save states are exact and fast (a run
  from a state draws what the original run drew, and `own-state-check` and
  `rewind-check` prove it), and recompiled Emerald runs a frame in about
  0.66 ms on quack in the overworld and in battles (25 times real time;
  see "Latency over the internet"), so a peer can run four of them.
- **The rule: your press reaches your console on the next frame.** Zero
  input delay, every game, turn-based ones too: instant response is what
  a GBA feels like and what this project keeps. Other players may
  teleport. Input delay is never the tool for a bad connection or a slow
  machine (june, 2026-10-27, after feeling lockstep's 3 frames: "it felt
  awful"). So netplay is **rollback with delay 0** (Slippi's kind:
  inputs alone cross the wire), the only netplay: lockstep and the QUIC
  relay were removed, not parked (june, 2026-10-28;
  multiplayer/remove-lockstep-quic). Where rollback's re-runs cost more than a frame
  has, the fix is speed (recompiled code for netplay, no drawing on
  re-run frames), not delay. Build it, play it, find what's bad, fix
  that. The older lockstep reasoning below is kept as history.
- **Which games netplay covers:** every game that links in multiplayer
  mode, with nothing per game. The cable assumes no lag, so its bytes
  can't cross the internet; inputs can. Normal mode, single-pak MultiBoot,
  UART, JOY bus and the wireless adapter aren't emulated yet. See "The
  cable over the internet: modes, timing, games" (2026-11-08).
- **The wireless adapter is the exception.** Its games talk to the adapter
  in packets, with radio-sized delays built in, and the Union Room gathers
  more players than one peer should simulate. For wireless the recommended
  shape is the opposite: each player runs only their own console, and the
  adapter is emulated as a device whose "radio" is the server. Later work.
- **A relay server first, on a machine june runs.** Clients make only
  outbound connections, so there is no NAT traversal to do. Rooms with a
  short code are the matchmaking. Plain UDP for transport (QUIC, the
  `quinn` crate, at first; removed 2026-10-28). Direct peer-to-peer is a later latency optimisation, not a need.
  A prototype (`crates/relay`, 2026-10-24) showed the relay needs to know
  nothing about the game or the console: see "Relay prototype".
- **Layers:** a console-neutral session core (`netplay`, std), a server
  (`gbaa-server`), a per-console adapter (for the GBA: a link hub in
  `runtime`, no_std, that runs several `Machine`s against one serial bus),
  and the front end (desktop: host, join, show your own seat). A later
  presence layer (other players on your overworld) sits beside the core,
  not in it.
- **Emerald Rogue already has a multiplayer mode** ("Multiplayer
  Adventures", through a companion program, Rogue Assistant, "currently
  only supported when playing using mGBA on Windows PC", says the ROM's
  own text). It doesn't use the link cable at all: each player runs their
  own game, and the assistant copies parts of a multiplayer blob in RAM
  between them over ENet. `gbaa-desktop --rogue ADDR` is that assistant,
  built in, and speaks its protocol, so a gbaa player can play with an
  mGBA player (see "Rogue Assistant").
- **Decisions for june:** what to build first, and where the server runs
  (asked with this unit; see "Questions for june").

## What multiplayer means here

### The link port, as the hardware has it

The GBA's serial port (`SIOCNT`, `RCNT` and friends at `0x0400_0120`–`0x0400_015A`)
has these modes (GBATEK):

| mode | what | who uses it in our targets |
|------|------|-----------------------------|
| Multiplayer (16-bit, 2–4 GBAs) | the master clocks a transfer; every console's `SIOMLT_SEND` halfword reaches every console's `SIOMULTI0-3` | Emerald's link cable: trades, battles, Record Mixing, Berry Blender, contests, multi battles |
| Normal 8/32-bit | one side clocks, a shift register, up to 2 MHz | the wireless adapter (32-bit, Emerald's `librfu_*`), e-Reader, GBA–GBA MultiBoot |
| UART | RS-232-like, with FIFO and flow control | rarely used by games |
| JOY bus | GameCube link (Colosseum, XD, Pokémon Box) | Emerald's `libgcnmultiboot.s` |
| General purpose | the four pins as GPIO | not for linking |

On its own a machine stores serial registers as written and never
completes a transfer (`runtime/src/io.rs`, "Serial and joybus, stored as
is"): a game that tries to link sees no partner, which is what it sees on
a real GBA with no cable. On a link hub (`runtime::link`, 2026-10-25)
multiplayer-mode transfers complete on every machine; `just emerald-link`
takes two Emeralds into the Trade Center that way, and `gbaa-desktop ROM
--link B.sav` plays two in one window (2026-10-25), where `just
emerald-trade` trades a Pokémon between the two saves.

### What Emerald does on the cable (`src/link.c`)

- Multiplayer mode at 115200 bps, serial IRQ on (`EnableSerial`).
- The master starts a transfer at each V-blank (`LinkVSync`); each serial
  IRQ reads every player's halfword, writes the next one, and the master's
  timer 3 (`-197` at 64-cycle ticks: 12,608 cycles, 0.75 ms) starts the
  next. Eight transfers a frame make one command: `CMD_LENGTH` 8 halfwords
  per player per frame, queued (`QUEUE_CAPACITY` 50) both ways.
- Lag is counted in frames: the master flags `LAG_MASTER` if fewer than 9
  serial IRQs came in a frame; a slave flags `LAG_SLAVE` after 10
  V-blanks without one. So the protocol is frame-synchronous, and every
  console must see each transfer at the same moment (it is one wire).
- Above it, block transfers (`SendBlock`) carry trade data, battle
  commands and the like. Battles stay in sync because both consoles feed
  the same exchanged commands into the same code.

### Wireless (`link_rfu_2.c`, `librfu_*.c`, `union_room*.c`)

The wireless adapter is a small computer on the link port. The game talks
to it over 32-bit normal mode with a command protocol (librfu's "STWI":
command, length, payload; the adapter answers or interrupts), at 115200 or
57600 bps clocks. The adapter does the radio: discovery, parent and child
slots (up to 5 consoles a group), packet delivery. Emerald's Union Room,
its chat, wireless trades and battles, Record Corner and wireless Berry
Crush all go through it. No emulator we found emulates it completely:
mGBA lists it as planned; VBA-M's support is partial. afska's
`gba-link-connection` documents the adapter's commands most completely, and
the decomp's `librfu` is the game side of the same protocol.

### Beyond the original

- **Spectating:** a spectator receives the inputs and runs the session
  too; free in the recommended design.
- **Linking across the internet** with a friend (the point of all this).
- **Linking two different games:** Emerald with Rogue, Emerald with a
  FireRed we might recompile later. Each peer then needs both ROMs.
- **Shared overworld** (seeing other players walk around your Hoenn, as
  PokeMMO and the 2026 browser project "Pokémon Emerald Online" do):
  nothing in the game supports it; it means reading each player's position
  from RAM and drawing the others into each game. Per game, and a layer of
  its own (below).
- **Rogue's Multiplayer Adventures:** co-op in Emerald Rogue through Rogue
  Assistant on mGBA. It reaches the game through mGBA's Lua scripting
  (memory reads and writes only); the desktop stands in for both (see
  "Rogue Assistant").

## How others do it

| project | approach | what we take |
|---------|----------|--------------|
| mGBA | local link only: several cores in one process, a lockstep that syncs the cores at transfer events. Network link has been planned for years (issue 2379: "pretty involved", a rewrite "planned for 0.12, but that may slip"); the maintainer notes the tight timing makes naive networking time out | the local lockstep is the right shape for our link hub |
| VBA-M | link over TCP (port 54970, per Dolphin's forum); cable, and wireless only in part (not checked here); transfers wait on the network | proof that a networked cable needs the timing hidden, not just forwarded |
| Dolphin, integrated GBA (2021) | a stripped mGBA core runs inside Dolphin, clock-synced to the GameCube; with netplay every peer runs every GBA, so savestates, input recordings and netplay all just work | our recommended design, generalised |
| Dolphin netplay, simple64 / Project64 netplay | lockstep with a fixed input delay; inputs exchanged, through a central server in simple64's case (from memory, not checked here) | lockstep first; a relay server; hashes |
| RetroArch netplay | GGPO-style rollback over UDP; needs a core that saves state fast and deterministically; states checked against each other | rollback if we ever need it |
| GGRS (Rust, GGPO reimplementation) + Matchbox (WebRTC) | rollback sessions, spectators, a `SyncTestSession` that forces rollbacks to find nondeterminism | the sync test idea now; the crate if rollback comes |
| AfterPlay's online link, mGBA forum proposals | run every core on a host and stream video to the others | rejected: latency and video quality (mGBA's maintainer says the same) |
| PokeMMO, Pokémon Emerald Online (2026) | a server-authoritative MMO: the client uses the game's data, the server owns the world; Emerald Online checks the ROM is clean US Emerald | only the presence idea; the rest is a different project |
| hardware (GB-Link, RetroSync) | a real cable or wireless adapter bridged to the internet | nothing to take; shows the demand |

## Timing: our cycle counting and the cable

Decision 11 counts estimated cycles at block granularity: generated code
ticks at every block exit (blocks of up to 256 instructions), I/O writes
are sync points, and IRQs are taken at block starts. Within one machine
that is fine for the link; the question is how two machines see one wire.

- **Across the internet, the wire can't be shared at transfer
  granularity.** Emerald does 8 transfers a frame, each needing every
  console's halfword; forwarding them means 8 network round trips per
  16.7 ms frame. That is what makes mGBA's and VBA-M's networked links
  time out, and why the recommended design keeps the wire inside one
  process.
- **Inside one process it is easy.** The link hub runs its machines in
  turns, each up to a shared deadline: the next transfer's end, or a
  scanline (1,232 cycles) when no transfer is in flight. A transfer starts
  on the master's `SIOCNT` write (already a sync point), takes its
  duration at the chosen baud rate, and completes on every machine at the
  same cycle, raising their serial IRQs. Emerald spaces transfers 12,608
  cycles apart, ten lines; block-granular ticks land a transfer's IRQ a
  block late at worst, much as a real game sees jitter from its own IRQ
  handlers. The order of turns is fixed, so the result is deterministic,
  which is what netplay needs: every peer computes the same frames.
- **Determinism has edges.** Decision 11's "IRQs land at slightly different
  instructions in the interpreter and compiled code" means a peer running
  a console interpreted and another running it recompiled will desync.
  Every peer must run the same build of every console in the session:
  the session's handshake compares a build ID (codegen version, ROM CRC,
  runtime version), not just the ROM.
- **The real-time clock** (Emerald's RTC, `runtime::gpio`) must be part of
  the session's inputs: the host's clock at session start, then advanced
  by emulated time, never read from each peer's wall clock.

### Latency over the internet

- Typical round trips: same city 10–30 ms, same continent 40–90 ms,
  across the Atlantic 90–150 ms. Through a relay, add the detour.
- Lockstep with input delay D frames runs smoothly while the one-way
  delay plus jitter stays under D frames: D = 3 (50 ms) covers the same
  region; D = 6 (100 ms) the same continent. In a menu-driven trade or a
  turn-based battle, 100 ms after a press is barely noticed. When an input
  is late, every peer pauses until it arrives (a stutter, not a desync).
- Rollback would hide the delay instead, at the cost of re-running up to
  D frames of every console in one frame, plus a state load per console
  and a state save per console per frame. The numbers below make it
  possible on quack and not comfortable on the Deck, and our targets
  don't need it: **lockstep with input delay, as decided.** (Superseded
  2026-10-27: rollback at delay 0, see the Summary and "Rollback".)

**Measured** (quack, 2026-10-25, multiplayer/measure-frame-cost): the
recompiled Emerald (USA) build, headless, each frame's wall time from
`gbaa-desktop --frame-times` (two runs each; quack had two lanes
working, load ~4 on 24 cores, so the tails carry other work's noise):

| scene | frames | mean | p50 | p99 | max |
|-------|-------:|-----:|----:|----:|----:|
| intro and title, from reset | 3,600 | 0.55–0.87 ms | 0.57–0.87 | 0.85–1.43 | 3.9–8.1 (frames 176–189, the intro's heavy scene; the first frame) |
| overworld, Oldale PC 2F, standing | 3,600 | 0.68–0.77 | 0.64–0.66 | 1.0–1.4 | 1.8–3.6 (frame 1) |
| overworld, Oldale Town, walking | 3,600 | 0.66–0.67 | 0.64–0.65 | 0.85–0.89 | 1.7–2.1 (frame 1) |
| wild battle (Mudkip and Poochyena, Route 101), A every 30 frames | 1,600 | 0.66 | 0.64 | 0.90–0.93 | 1.3–1.5 |

Battles cost no more than the overworld: Emerald's frame is bounded by
waiting for V-blank, not by the scene. 97.8% of guest cycles ran
compiled (2.2% in the interpreter fallback).

State, from `--state-bench 1000` in the battle (the overworld's is the
same size):

| | bytes | save | load |
|---|---:|---:|---:|
| our whole state (header, CRC-32s; what a file or the network carries) | 642,720 | 2.1 ms (the first 43 ms) | 3.6 ms |
| the bare payload (what rewind keeps) | 642,696 | 0.16–0.19 ms | 0.32 ms |
| compressed: gzip -6 / zstd -3 / zstd -19 | 30–45 K / 28–37 K / 24–30 K | | |

The whole state took 28 ms until the CRC-32 of the 16 MB ROM was kept
per machine rather than recomputed on every save and load
(multiplayer/rom-crc-once): only the first save or load pays it now.
What's left over the payload is mostly the payload's own CRC-32. A session
start sends each console's state once, ~30–40 KB with zstd: a
fraction of a second on any link.

**The Deck** is not measured yet (multiplayer/deck-frame-cost, waiting
on june's OK to use the Deck). Its Zen 2 cores (up to 3.5 GHz) against
quack's Zen 5 (up to 5.1 GHz) suggest 2–2.5× slower a frame: about
1.5 ms mean, 2–2.5 ms at p99.

**Settled from these numbers:**

- **Lockstep fits with room to spare.** Each peer runs every console: 2
  Emeralds cost ~1.3 ms of a 16.7 ms frame on quack (~3 ms estimated on
  the Deck), 4 Emeralds ~2.6 ms (~6 ms). A multi battle's four consoles
  fit on the Deck.
- **Input delay: default D = 3 frames (50 ms), adapted between 2 and 8
  from the measured round trip** (D ≥ ceil((RTT/2 + jitter) / 16.7 ms)
  + 1, the +1 for the frame being run when an input arrives). 3 covers
  the same region, 5–6 the same continent, 8 across an ocean through
  the relay. The emulation's own cost adds nothing to D: a frame runs
  in well under a frame's time.
- **Rollback doesn't fit the Deck and isn't needed** (superseded: this
  sized rollback at D = 6; at D = 0 a rollback goes back about the
  one-way trip, 2-3 frames, and recompiled Emerald's 2 consoles cost
  ~1.3 ms a frame, room for ~10 frames run again). For 2 Emeralds and
  D = 6 a rollback frame re-runs 12 console-frames: 12 × 0.66 ≈ 8 ms
  mean on quack, but at p99 12 × 0.9–1.4 ≈ 11–17 ms, plus 2 loads and 2
  saves a frame (~1 ms); on the Deck 2–2.5× that, past the 16.7 ms
  budget. It stays out unless a real-time game asks for it, and then
  with a smaller D or one console per peer.
- **Desync checks are cheap:** hashing the 642 KB payload, or a smaller
  digest of it, every few seconds costs well under a millisecond.

**To reproduce:** `just emerald-saves` for `alice.sav`; then, on a copy
of it (`--save COPY`), the recompiled build (`just build-recompiled
out/pokemon-emerald-version`) from reset with
`--input 'start@300,a@900,a@1100,down@1410+48,left@1460+160,down@1800+20,right@1820+80,down@1900+150,right@2100+100,down@2200+60,left@2400+16,down@2420+40,right@2640+48,down@2688+300,left@3040+96'`
plus up and down in turn, each held 32 frames, every 40 frames from
3150 (41 presses). PC 2F is frame 1400, Oldale Town 2640, the battle
3640 (`--state-out S --state-at N` keeps each; the battle's state is
byte-identical whether reached in one run or in steps). The town walk
is right and left in turn, held 32 every 40 frames from the town's
state; the battle presses A every 30 frames from its state.

## Architecture

### Options

| | how | for | against |
|---|---|---|---|
| **A. Shared simulation** (recommended for the cable) | every peer runs every console; inputs exchanged; the link inside the process | generic: any cable game, any console, no per-game code; exact; spectating, replays and desync recovery fall out | each peer needs every ROM in the session; CPU grows with players; a session starts from exchanged states (a few hundred KB each) |
| B. Delayed virtual cable | each peer runs only its own console; each transfer gets the others' halfwords from a fixed number of frames ago | one console per peer; join any time | changes what the game sees (everyone no longer sees one transfer at the same moment); works only for protocols that queue, like Emerald's; per-game risk; can't be tested exactly |
| C. Hook the game's link API | replace `LinkMain1` or `SendBlock` with networked versions, using symbols | the most latency-tolerant | per game, needs symbols (Rogue and Imperium have none, though they share the decomp's code); a static recompiler makes it possible, not cheap |
| D. Stream video | one host runs everything | no determinism needed | latency, bandwidth, video quality; the remote player plays a video |
| E. Authoritative MMO | the server owns the game world | a real MMO | a different game, reimplemented; only its presence idea is in scope |
| **F. Networked wireless adapter** (recommended for wireless) | each peer runs its own console and an emulated adapter; adapter packets go through the server | many players (Union Room); the adapter's protocol already expects radio delays | the adapter is a whole device to emulate; its timeouts under internet latency are unmeasured |

### Recommended: the layers

```
 front end (desktop)      host / join a room, show your seat, spectate
 ───────────────────────────────────────────────────────────────────
 netplay (std, console-neutral)
   session: seats, frame clock, input delay, input exchange,
            state transfer at start, hash checks, resync, spectators
   transport: QUIC (quinn) to the relay; in-memory for tests
 ───────────────────────────────────────────────────────────────────
 console adapter (per console)      trait Console
   GBA: runtime::link hub (no_std) — N Machines, one serial bus,
        multiplayer and normal modes; later the wireless adapter device
 ───────────────────────────────────────────────────────────────────
 gbaa-server (std, console-neutral): rooms, codes, relay of opaque payloads
```

**`netplay` core.** Knows nothing about the GBA. The console side is a
trait, roughly:

```rust
trait Console {
    type Input: Copy + Eq;          // GBA: 10 key bits per seat
    fn seats(&self) -> usize;
    fn step(&mut self, inputs: &[Self::Input]); // one frame, every seat
    fn save(&mut self, out: &mut Vec<u8>);
    fn load(&mut self, s: &[u8]) -> Result<(), Error>;
    fn hash(&self) -> u64;          // cheap; for desync checks
    fn build_id(&self) -> [u8; 32]; // what every peer must agree on
}
```

A session: the host creates a room; peers join and send their build ID
and their own console's state (their save included); the host fixes the
seat order and starts at frame 0 with every state loaded on every peer.
Each frame, each peer sends its input for frame N + D and steps once it
has every seat's input for N. Every 60 frames peers exchange a hash; on a
mismatch each seat's owner is the authority for its console and resends
its state. When the session ends, each peer keeps only its own console's
result: its cartridge save, written as now (`desktop::sav_file`). The
other consoles' states are discarded.

**GBA adapter: the link hub.** In `runtime` (no_std, so a future embedded
target keeps it): a `Link` that owns N `Machine`s and a serial-bus device,
runs them in turns to a shared deadline, and completes transfers on every
machine at the same cycle. It is also useful without any network: two
games linked on one screen, and the only way to test the cable
deterministically. Multiplayer mode first (Emerald's cable), then normal
32-bit (the wireless adapter's port, e-Reader, MultiBoot), UART last if a
game needs it. The `Console` impl for a GBA session lives with the front
end (std), wrapping the hub.

Cost per peer is one console per seat: 4 Emeralds at about 0.66 ms each
(p99 under 1.5 ms) on quack, an estimated 1.5 ms each on the Deck, well
inside a frame.

**Server.** `gbaa-server`, a small tokio + quinn program: rooms with a
6-character code, seats plus spectators, passing build IDs between
seats for the clients to check (and so the ROM check, which the build ID
contains; ROMs and saves pass between players only as session state,
never stored), and relaying
input and state messages. Relaying every message costs a detour but needs
no NAT traversal: everyone connects out. Hole-punched direct connections
(QUIC over UDP makes that possible later) are an optimisation for peers
far from the server. Accounts, if ever, are a later question; a room code
shared with a friend is enough to start.

**Protocol.** Two layers, after the relay prototype. The relay's own
messages are console- and game-neutral (see "Relay prototype"):
`Create{proto, seats, build_id}`, `Join{proto, code, build_id}`, `Lock`,
`Send{to, payload}` up; `Welcome{code, seat, seats, peers}`,
`PeerJoined{seat, build_id}`, `PeerLeft{seat}`, `From{seat, payload}`,
`Error` down. The session's messages ride inside `payload`, opaque to the
server: `Start{frame0, rtc, seats}`, `State{seat, state}`, `Input{frame,
bits}` (each repeated in the next few packets so a lost one costs
nothing), `Hash{frame, h}`, `Resync{seat, state}`. On QUIC, reliable
ordered streams carry everything but inputs, which go as datagrams.
`postcard` for the encoding.

**Wireless, later (option F).** An emulated adapter per console, speaking
the STWI command protocol to the game, with the server as the radio:
broadcast, parent and child slots, packets. Each peer runs only its own
console, so a Union Room can hold many players. Needs the device emulated
first locally (two machines in one hub, adapter to adapter), then the
radio moved onto the network, then a measurement of how much latency
librfu's timeouts allow.

**Presence, later.** A shared overworld is not a link feature. Each game
reports where its player is (map, coordinates, facing, sprite) by reading
RAM at frame ends through per-game symbols; the server fans that out to
everyone in the same "world"; each game draws the others as extra object
events or sprites. Non-deterministic and loss-tolerant, so it doesn't use
the session core. Its per-game hooks are the same kind of thing as Lua
scripting's memory API: design it after that research settles.

### Other consoles

The core and the server don't change. A new console brings an adapter:

- **GB / GBC:** the link is an 8-bit shift register (internal or external
  clock, 8 KHz or faster). Same hub idea; Pokémon Red/Blue/Gold trades and
  battles are frame-synchronous like Emerald's.
- **NDS:** the wireless (Nintendo's Wi-Fi local play, Pokémon's Union Room
  successors) is a much bigger device than the GBA adapter; WFC online
  play is a server protocol (like Wiimmfi's). Shared simulation for local
  wireless, a protocol server for WFC.
- **GameCube:** input-sync netplay as Dolphin does, with GBAs as more
  seats in the same session; the JOY bus to a GBA is one more link-hub
  device.

## Relay prototype

Done 2026-10-24 (multiplayer/relay-prototype), before the real server,
to test june's ask that the relay know nothing about the game, so one
server serves any game on any console. `crates/relay`: std only, TCP with
length-prefixed frames, a thread per client (transport and async are the
relay-server unit's; the question here was what the server must know).
`gbaa-relay [ADDR]` served; `gbaa-relay host ADDR [SEATS]` and
`gbaa-relay join ADDR CODE` played a toy lockstep game through it. (The
same crate is now the real server, `gbaa-server`, over QUIC: see "Relay
server".)

**The toy clients** stand in for the emulator: every peer runs the whole
toy game (a 64-bit state stepped each frame by every seat's input), sends
its input 3 frames ahead, and swaps state hashes every 60 frames, which is
the session core's shape. `cargo test -p relay`: two clients play 600
frames and four play 300, with random per-frame jitter, and each ends
on the same state as an offline run; a build-ID mismatch is caught by
the clients; full, locked and missing rooms are refused; a 400 KB state
(a GBA save state's size) relays whole; a leaver is announced. Run as
three processes (server, host, joiner), both clients ended on the same
state.

**Answer: the server needed to know nothing about the game or the
console.** What it holds per room: a code, a seat count, each seat's
build ID as bytes it never reads (kept only to hand to later joiners), a
lock flag, and which seat may lock (the lowest occupied one). Its only
numbers are its own: a relay protocol version, at most 16 seats a room,
and a 4 MiB message cap (sized so a whole GBA state fits; a limit, not
knowledge, and a setting for the real server). It has no clock and no
idea of frames.

**What moved to the client** to get there, against the first design
above:

- **The build-ID check.** The server passes each seat's ID to the others
  (`Welcome`, `PeerJoined`); each client compares and leaves on a
  mismatch, so a wrong joiner holds a seat for one round trip. A server
  check of opaque byte equality would also be game-neutral, but keep it
  in the client: a session of two different games (Emerald with Rogue)
  has a different ID per seat, and only the clients know which
  combinations go together.
- **The console field** in `Hello` is gone: what console and game a peer
  runs is part of its build ID.
- **The seat count** is the creator's (a GBA cable room asks for 2 to
  4), not the server's.
- **Start** is the host's payload to everyone; the server only learns
  "no more joiners" (`Lock`). Its "started" is that flag.
- **Frames, inputs, input delay, hashes, resync** are payloads. Resync
  needs only `Send{to: seat}`, a message to one seat.
- **Who is authoritative** when a peer leaves is the clients' decision
  on `PeerLeft`; the server tracks no host beyond who may lock.

**Not prototyped**, and still game-neutral when built: spectators (a
seat kind that doesn't count toward the seats and receives everything),
rejoining after a dropped connection (a per-seat token from `Welcome`),
QUIC datagrams for inputs (the repeat-in-the-next-packets is the
client's), and a server-side room expiry. None needs to look inside a
payload.

## Session core

Built 2026-10-25 (multiplayer/session-core), `crates/netplay`, std,
depending on `runtime` only for its `gba` module:

- **`Console`** as sketched above, with `save(seat)` (one seat's state,
  for its owner to send), `load(states)` (every seat's at once, which
  also restarts whatever links them) and `set_clock` (the host's clock at
  the start; the console advances it by emulated time). Inputs are fixed
  size on the wire (`Input`: `u16` for a GBA).
- **`Transport`**: `send(to, Reliable | Unreliable, bytes)` and `recv`,
  the seat a message is from given by the transport (the relay's
  `From{seat}`). `sim` is an in-memory one with seeded latency, jitter and
  loss (a lost reliable message is resent a round trip later, kept in
  order); `sim::play` runs peers on it, each on its own 60 Hz clock.
- **Messages** (`msg.rs`): `State{frame, build, data}`, `Start{delay,
  clock}`, `Input{first, inputs, have}`, `Hash{frame, hash}`. No
  `Resync` message is needed: at every 60th frame F each peer sends its
  hash, and before running past F + 60 it must hold every seat's hash for
  F; every peer sees the same hashes, so all decide alike, and on a
  mismatch each sends its own seat's state at F + 60 and loads everyone's
  there. The build-ID check rides in `State`. Inputs repeat from the
  first frame any other seat lacks (`have` says), so loss needs no resend
  logic.
- **GBA** (`netplay::gba::Gba`): N machines on a `runtime::link` hub in
  the interpreter; a seat's state is its machine's whole save state. The
  hub's own state isn't in it, so `load` plugs in a new cable: exact at
  the start (where a straight run starts), and the same on every peer at
  a resync.
- **Checked:** `cargo test -p netplay` (a toy console of 2 to 4 linked
  seats and a tiny key-folding ROM on two GBAs, under 40 ms + 0-30 ms
  jitter and 25% loss, match a straight run's hashes; two deliberate
  desyncs end in a resync at the same frame on every peer and agreeing
  hashes after; a build mismatch is refused). `just emerald-netplay`:
  two Emeralds walk into the Trade Center as a session under 40 ms, +30
  ms, 10% loss with delay 3: 70 of 70 hashes and the end state equal the
  straight run's, 83 s of simulated time for 70 s of frames.
- **Not yet:** adaptive input delay (fixed from `Start`), spectators, a
  peer leaving, and recompiled consoles (the interpreter only, as the
  build ID says). The relay as a `Transport`: "Relay server". Two
  processes over UDP: see "Variable-ping simulator".

## Variable-ping simulator

Built 2026-10-25 (multiplayer/ping-sim), the owner's way to test netplay
locally: two peers, each its own process, over real UDP sockets through
a proxy that follows a TOML profile (`crates/netplay/profiles/`), and
`just ping-sim PROFILE` checks them against a straight run.

- **`netplay::udp`**, a `Transport` over UDP: an unreliable message is a
  datagram; a reliable one goes in 1200-byte chunks (under any MTU),
  numbered per destination, 64 in flight, each resent after an RFC 6298
  timeout until an ack covers it (cumulative, plus a bitmap of the 64
  after: a selective ack), and put back in order. Each timeout doubles
  the timeout, to 1 s, until a chunk sent once measures the round trip
  again (multiplayer/udp-resend-backoff, below). Each
  datagram carries its sender's seat, so a peer can sit behind a proxy
  (or the relay). The real transport is still QUIC, the relay-server
  unit's; this one is for tests and is enough for two peers on a LAN.
- **`netplay::proxy`**: a socket per side, each learning its peer's
  address from what arrives. Delay = latency + a sine wander + a spike
  while one lasts (exponential gaps) + uniform jitter; each direction
  keeps its order except the `reorder` share, held back so later ones
  overtake; plus loss and duplicates. Seeded, but real time, so runs
  differ in detail.
- **Checked** (2026-10-25, quack, debug build): calm (20 ms + 5), wander
  (60 ± 40 ms over 4 s, + 20, 2% lost), spiky (40 ms, 250 ms spikes of
  400 ms every 2 s, 5% lost) and rough (30 + 40 ms, 20% lost, 15%
  reordered, 5% duplicated) each give a straight run's hashes and end on
  both peers, toy console and two linked GBAs alike. At delay 3, calm
  plays 360 frames in 6.0 s, real time; rough in 8.2 s; the GBA's
  400 KB states cross rough in a few hundred resent chunks.

## Relay server

**Removed 2026-10-28** (multiplayer/remove-lockstep-quic, june's
decision): the QUIC server, its certificate, client and toy lockstep
game are gone, and `gbaa-server` is the plain-UDP rooms alone (see
"Rollback", Transport) on one port, 7374, keeping nothing on disk. Its
check is `just room-sim` (the toy game by rollback through a local
server's rooms, or `--server ADDR` a remote one). What follows is the
QUIC server as it was, kept as history.

Built 2026-10-25 (multiplayer/relay-server), `crates/relay` in place of
the TCP prototype, its rooms and wire messages kept: `gbaa-server`,
tokio + quinn.

- **Connection:** QUIC over UDP. Each client opens one bidirectional
  stream (requests up, events down, each a `u32` length and the
  message); a `Send` may instead go as a datagram, which the server
  passes on as a `From` datagram (on the stream if the receiver can't
  take one that size). Idle connections end after 30 s; clients send a
  keep-alive every 5 s. BBR congestion control (Cubic collapsed under the
  rough profile's random loss), and a 4 KiB datagram queue so held-back
  inputs are dropped oldest first instead of queued for seconds.
- **TLS without a domain:** the server makes a self-signed certificate
  on its first run (`--cert-dir`, default `$GBAA_DATA/gbaa-server/`,
  kept so restarts keep it, the key owner-only) and clients
  pin it: they trust that certificate alone, at whatever address. A
  player needs the address and the cert file. Accounts and a CA-signed
  name stay later questions.
- **Rooms:** as the prototype, plus spectators: `Watch{proto, code,
  build_id}` gives a member number from the seat count up (up to 16),
  allowed after `Lock`, announced with `PeerJoined`/`PeerLeft` like a
  seat, hearing everything sent to everyone. A room goes when its last
  member leaves. Relay protocol version 2 (the `Watch` message). Still
  nothing about the game.
- **Client:** `relay::Client`, blocking, its connection on a thread of
  its own (so a game loop needs no async). `netplay::relay::Relay` is the
  session's `Transport` over it: `host` creates the room, waits for the
  seats to fill and locks it; `join` takes a seat by code. Reliable
  messages ride the stream; inputs go as datagrams, at most one per 4 ms
  with a newer one replacing a waiting one (the session sends its inputs
  every 1 ms pump, harmless on bare UDP, a queue through congestion
  control). What spectators send is dropped there until sessions have
  spectators.
- **Checked** (2026-10-25, quack, debug build): `gbaa-pingsim relay
  PROFILE`, two peer processes through a local server, each peer's path
  to it through its own proxy following the profile, so the peers see
  two hops of it: calm 360 frames in 6.2 s (real time); wander 13.8 s,
  spiky 13.8 s, rough 13.2 s (two hops' delay, against delay 3); rough
  with two linked GBAs, 240 frames and their 400 KB states, 13.1 s. All
  match the straight run's hashes and end, no resync. `cargo test -p
  relay`: toy sessions of 2 and 4 seats, refusals, a 400 KB message,
  spectators, datagrams and a 20 KB one that falls back to the stream.
- **Not yet:** rejoining after a dropped connection, a room expiry
  beyond "empty", NAT hole punching.

### Versions (multiplayer/server-version-cutoff)

Built 2026-10-31, june's ask (2026-10-30): a friend on a stale binary is
told to update, not let in to desync. `netplay::VERSION`
(crates/netplay/src/lib.rs) is a number every client sends after its
build ID in the rooms' `H` and `J` (a `u32`, little-endian; a build from
before versions sends none and counts as 0). `gbaa-server --min-version
N` refuses an older one with an `E`: "this game is too old for the
server (version 1, needs 2): get the new build", which a pre-version
client prints as any refusal, and the desktop window's netplay page
shows as "The server said no: …", not a timeout. Without the flag every
version is let in (the room-sim's and tests' in-process servers). The
server compares the number and nothing more; the build ID still keeps
two different builds out of one room.

**When to raise it:** by hand, by one, in the commit of any change that
can split peers (two builds stepping a console, the link or rollback
differently) or changes the wire (the session's messages, `udp`'s
datagrams, the rooms'). Then raise `--min-version` in
deploy/fly/Dockerfile to match (a netplay test fails until they agree)
and redeploy fly once the new build is with the friends.

## Desktop netplay

Built 2026-10-25 (multiplayer/desktop-netplay), `crates/desktop/src/room.rs`:
`gbaa-desktop ROM --server ADDR --host` or `--join CODE`, two seats
(2 to 4 since 2026-10-31, `--players N`: "Four players" below), the
host the cable's parent. Each peer opens its own cartridge save into its
seat's machine, the session swaps the states and runs both GBAs; the
window draws its seat's PPU, writes its seat's save as it changes and
drops the other console at the end. The window's consoles read the
host's wall clock (sent in `Start`), headless the fixed clock.

- **A save's change flag is console state.** `Save::dirty` is in the
  snapshot, so the save writer clearing it on its own seat alone made the
  peers' hashes differ at every in-game save (six resyncs in the trade).
  Every peer now clears every seat's flag each frame, after its writer
  has looked.
- **Checked** (2026-10-25, quack): `just emerald-netplay-trade` (calm 121
  s, rough 265 s for 7600 frames; same end hash on both peers, no
  resync), and two windows in a private `cu session` through a real
  `gbaa-server`, each showing its own seat through the trade at 60 fps.
- **Not yet:** sound (`netplay::gba`'s machines drop it), more than two
  seats (since 2026-10-31: up to four), spectating, a room-code prompt in the menu (the code is a
  flag), and leaving: a player who closes the window ends the other's
  session ("player N left").

## Rollback

Built 2026-10-27 (multiplayer/rollback-design), beside lockstep, the
default since the same day, and the only netplay since 2026-10-28
(multiplayer/remove-lockstep-quic: lockstep's `Session`,
`netplay::relay` and the QUIC server removed; the sim tests, the ping
simulator and `emerald-netplay` ported to rollback):
`crates/netplay/src/rollback.rs`. Slippi's kind: inputs alone cross the wire, every peer runs
every console, and a peer never waits for another's input. It runs each
frame at once, guessing a missing input as that seat's last one, keeps a
snapshot (`netplay::Snapshot`: each machine's payload, the cable's hub
state and the bits of `SIOCNT` it owns) before every guessed frame, and
when a real input differs from its guess it loads the snapshot before
that frame and runs to where it was again, the speakers quiet. The other
player is seen to jump; the game doesn't stop.

- **Input delay** (default 0; `--delay` 0-30 for experiments): this
  seat's input is for frame N + D, and a round trip under twice it needs
  no rollback.
- **Window**: 300 frames (5 s) past the last frame every input is in
  for; past it the peer waits (the other player gone, near enough). It
  was Slippi's 7 (117 ms), which froze the game at any late input
  longer than that; june asked why stop at all. The window bounds only
  memory (a snapshot a frame, ~1.3 MB for two GBAs: ~390 MB at 300) and
  how many frames one rollback may run again: a peer silent for a
  second costs a 60-frame re-run when its inputs land, a hitch of
  60 x 6.8 ms in the interpreter, which the playtests should show.
  A game that itself waits on the cable (a battle's turn, the trade
  screen) shows its own "waiting" instead, as on hardware.
- **Time sync**: each peer sees how far ahead it runs from the input
  counts both sides report (`Msg::Input::have`; no new message), and a
  peer ahead by a frame or more holds one back (`Status::Skipped`, at
  most one in 12).
- **Hashes**: every 60th frame's consoles are hashed once every input
  before it is in, from its snapshot, and compared; a mismatch is a
  `desync N` line. No resync yet: none seen.
- **Transport: plain UDP** through `gbaa-server`'s rooms
  (`relay::rooms`, the server's one port, 7374; until 2026-10-28 one
  port above the QUIC server's 7373): the server seats the players by
  room code and passes datagrams on, nothing else. No certificate.
  `netplay::udp::Udp::host`/`join`.
- **Saves**: a save's change flag is console state, so `Gba::step` now
  moves it to `Gba::take_changed` itself every frame (a frame run again
  must compute what it did the first time).
- **Checked**: unit tests (toy consoles at delay 0 through 40-70 ms with
  25% loss, three seats, linked GBAs) end on a straight run's hashes;
  Kirby & the Amazing Mirror's multiplayer (two Kirbys in Rainbow Route,
  about 8 cable transfers a frame) through `gbaa-pingsim desktop`, every
  run's two peers on one end hash, the same whatever the network did.

Kirby, two consoles a peer, on quack (interpreter): a frame of both
costs 6.8 ms mean (p99 13 ms), a snapshot 0.35 ms, a restore 0.7 ms
(the payload is 643 KB a console). So a 16.7 ms frame has room to run
about one frame again; a deep rollback makes that frame late (a hitch),
not a freeze. Fat LTO gave about 20% more in one measurement, not
switched on.

Measured, 3600 frames (a minute) paced at 60 fps, both peers on quack,
the joiner's path through the profile (`just rollback-feel PROFILE ROM
--headless --paced ...`; the lockstep row from before it was removed). With few input changes (a scripted walk):

| profile (one way) | mode | frozen (times, total) | frame p99 |
|---|---|---|---|
| wander (60 ± 40 ms, 2% loss) | lockstep, delay 3 | ~1190 times, ~40 s | 8.8 ms |
| wander | rollback, delay 0 | 18-21, 0.5-0.7 s | 10-13 ms |
| wander | rollback, delay 1 | 3-4, under 0.1 s | 9.7 ms |
| wander | rollback, delay 2 | 0-1, 0.05 s | 9.3 ms |

With inputs changing every 9 frames for each player (harder than play):

| profile | delay | rollbacks a minute, frames run again | frame p99 | frozen |
|---|---|---|---|---|
| calm (20 ms) | 0 | ~300, ~590 | 23 ms | never |
| calm | 2 | 0 | 9 ms | never |
| wander | 0 | ~300, ~1290 (deepest 7) | 50 ms | 0.6 s |
| wander | 2 | ~280, ~740 | 35 ms | only at the start |

(The start's own wait, while each side's state arrives, isn't play; it
shows as one freeze before frame 20.)

**Recommendation (superseded 2026-10-27 by delay 0 as the default):
offer both, rollback for play.** Lockstep freezes at
every late input on a wandering connection; rollback never did once
playing. Delay 0 feels instant and holds up on a near connection; on a
far one its re-runs cost more than quack has, and delay 1-2 (Slippi's
default 2) trades 17-33 ms of input lag for smooth frames. The Deck
(estimated 2-2.5x slower, docs above) has no room for re-runs at all in
the interpreter: a recompiled build for linked play is the way to more
headroom there. (2026-10-28: about 1.4x, "On recompiled code" below.)

### On recompiled code

Since 2026-10-28 (multiplayer/recompiled-netplay) a recompiled game's
binary runs a session's consoles on its code: `netplay::gba::Gba::with_code`,
on the desktop's own platform type (generated code is built for
`desktop::Desktop`, which hands the consoles a `netplay::gba::Pad` when
it is a netplay seat, `netplay::gba::Seat`). The build ID says `gbaa rc`
and a hash of the code's shape (every key, overlay guard, interpreted
block start and folded ROM range, and since 2026-10-31 its `--timing`)
instead of `gbaa gba interp`, so a peer on another build, or interpreting,
is refused. Byte 29 is the interpreter's timing (`runtime::Timing`), so an
instant peer and an exact one are refused too. Snapshots and
restores are the machine payload as before.

At first each turn of the cable ended at the hub's deadline by unwinding
compiled code to the dispatcher, and that was the catch: the cable ends
a machine's turn every scanline, and a **structured** build resumed past
a function's entry runs in the interpreter to the next key. Kirby's
structured build ran 23% of its cycles compiled in netplay (72% alone in
a window) and was slower than the interpreter, 5.6 ms a frame against
4.8, so for a day a structured build played interpreted. A **v1** build
(`gbaa-recompile --no-structured`: every block start a key) resumes in
compiled code: 71% compiled, the same share as alone.

Since 2026-10-29 (multiplayer/netplay-guest-stacks) each console runs on
a guest stack of its own, as the window's (`crates/desktop/src/coro.rs`,
`Guests`, given to the session as `netplay::gba::Turns`): it switches out
at the hub's deadline as at a frame end, and goes on in compiled code at
the next turn and the next frame. Between turns the machine still holds
the whole state a snapshot needs (`cpu.pc` and the registers are written
at every switch). A rollback's restore ends the guests (their frames are
discarded, not run), and new ones start from each `cpu.pc`; a function
they resume past its entry runs block-timed (`Nest::timed_resumes`: the
`--interp-blocks` interpreter's way, each inner block's instructions then
one tick with their cost), so it computes exactly what the compiled
function would have. Proof of that: Kirby's structured build ends a
session on the same hash as its v1 build (`a9e16c05…` at delay 0,
`00fdb075…` at delay 2, below), unwinding or not. The old way, a
per-instruction resume, ended on another hash than the guest that ran
on, so guests kept across a restore that way would split the peers.

After a restore, though, a function that never returns (the main
loop) ran block-timed for good, so under rollback Kirby's structured
build was 48% compiled. Since 2026-10-31
(multiplayer/netplay-compiled-after-restore) codegen prints such a
function v1 (`V1::NeverReturns` in `crates/codegen/src/lower.rs`: every
exit goes to one of its blocks, a call, or a `swi` back to one), so a
restore resumes it at a key. In Kirby that's `GameLoop`, `AgbMain` and
the ROM's entry; `GameLoop` alone had held 99% of the block-timed
cycles (296M of 297M in a wander delay 0 session). Kirby's structured
build now runs as much compiled under rollback as v1 and about as fast,
on v1's hash (the 2026-10-31 table below). What's left of either
build's time is its own interpreter fallback: about 29% of its cycles,
most of them code it copies to RAM that codegen doesn't cover.

Kirby two-player, both peers on quack, 2026-10-31, the same command as
the 2026-10-29 table below; other sessions were building beside some
runs, so the quietest runs are given (the noisier ones were up to 1.3
ms slower, either build). Exact is `gbaa-recompile --timing exact`;
instant is a game's default, run after the interpreter fallback took
the build's timing too (the interpreter's instant timing, PR 74):

| build | timing | profile, delay | compiled | frame p50, p99 | end hash |
|---|---|---|---|---|---|
| structured | exact | wander, 0 | 70.7% | 3.6 ms, 26.5-27 ms | `a9e16c05…` |
| v1 | exact | wander, 0 | 70.7% | 3.5-3.7 ms, 27-28 ms | `a9e16c05…` |
| structured | exact | wander, 2 | 70.7% | 3.6-3.7 ms, 20-20.5 ms | `00fdb075…` |
| v1 | exact | wander, 2 | 70.8% | 3.4-3.5 ms, 20-20.5 ms | `00fdb075…` |
| structured | instant | wander, 0 | 69.4% | 3.4-3.6 ms, 25-26 ms | `41fb45c8…` |
| v1 | instant | wander, 0 | 69.4% | 3.5 ms, 25-27 ms | `2c3e0981…` |
| structured | instant | wander, 2 | 69.5% | 3.5-3.6 ms, 19.5-20 ms | `db772d4f…` |
| v1 | instant | wander, 2 | 69.5% | 3.8-3.9 ms, 19.5-20 ms | `fd7c21f0…` |

Every run's two peers ended on one hash, and each build on the same
hash every time it ran. Exact, structured and v1 end on the same hash,
the 2026-10-29 table's. Instant, they don't: a structured build and its
v1 build tick on different clocks there. That splits no session, since
the build ID refuses a peer on another build.

Before that, Kirby two-player, both peers on quack, 2026-10-29 (lane 1 running judge
jobs beside it, so ±0.3 ms): `just rollback-feel PROFILE ROM --exe BIN
--headless --paced --frames 3600 --delay D --input1
'M,a@1820,a@1950,a@2100,right@2200-3600/18+9' --input2
'M,left@2200-3600/18+9'`, M the menu presses below
(`start@400,start@700,a@800,a@950,down@1050,a@1100`); "unwinding" is
the session without `Guests`. Frame p50 and p99 of each peer:

| build | profile, delay | turns | compiled | frame p50, p99 | end hash |
|---|---|---|---|---|---|
| structured | wander, 0 | unwinding | 24% | 5.7-5.8 ms, 39 ms | `f39c168b…` |
| structured | wander, 0 | guests | 48% | 4.5-4.6 ms, 30-31 ms | `a9e16c05…` |
| v1 | wander, 0 | unwinding | 71% | 3.4-3.5 ms, 25-26 ms | `a9e16c05…` |
| structured | wander, 2 | guests | 48% | 4.3-4.4 ms, 23 ms | `00fdb075…` |
| v1 | wander, 2 | unwinding | 71% | 3.5-3.6 ms, 19 ms | `00fdb075…` |
| structured | calm, 2 (no rollbacks) | guests | 71% | 3.3 ms, 7 ms | `00fdb075…` |

Every run's two peers ended on one hash. The earlier runs, 2026-10-28:

Kirby two-player, both peers on quack (lane 2 busy beside it), 3600
frames paced at 60 fps through `gbaa-pingsim desktop` (`just
rollback-feel PROFILE rollback ROM --exe BIN --headless --paced --frames
3600 --delay D --input1 S --input2 S`), the menu presses above then from
frame 2200 player 1 `right` and player 2 `left` every 9 frames; one
binary, `--interp` for the interpreter. Frame p50 and p99 of each peer:

| profile | delay | interpreter | v1 recompiled | frames run again (interp, v1) |
|---|---|---|---|---|
| calm | 2 | 4.8 ms, 10-11 ms | 3.4-3.6 ms, 9-11 ms | ~0, ~0-210 |
| wander | 0 | 5.3-5.5 ms, 45 ms | 4.2-4.3 ms, 36 ms | ~1450, ~1540 (deepest 8-9) |
| wander | 2 | 5.2-5.3 ms, 33 ms | 3.6 ms, 24 ms | ~855, ~915 |
| rough | 0 | 4.9-5.0 ms, 39 ms | 3.5 ms, 27 ms | ~1260-1830, ~1365 |
| rough | 2 | 4.9-5.0 ms, 26 ms | 3.4-3.6 ms, 19-20 ms | ~740, ~935 |

(Structured, calm at delay 2: 5.6 ms, 8.7 ms, 23% compiled.) No run froze
once playing, and in every run the two peers ended on one hash; the same
delay ended on the same hash in every profile (delay 0: `0627f71e…`
recompiled, `fb3b1227…` interpreted; delay 2: `87e31911…`, `8ff8fff0…`).
A frame run again costs about a frame, so a 16.7 ms frame has room for
about 2.5 more in the interpreter and about 4 recompiled: **about 1.4x
the re-run headroom**, and a third off the p99 under rollback. One
console alone: 1.5-1.7 ms a frame v1, 2.6-2.9 ms interpreted (menus and
play alike). Not measured yet on june's Mac (no Mac among the factory's
machines; multiplayer/recompiled-netplay-mac).

### What made Kirby laggy (multiplayer/lag-report)

june's first session over fly (2026-10-27, delay 2, june's desktop and a
friend's Mac on the interpreter build): "it works, and it's laggy as
hell". The window now reports on it (2026-10-28): the menu shows the end
lines so far at the screen's foot, and on exit they go to stdout and to
`$GBAA_DATA/netplay/<unix seconds>-<room>-p<seat>.txt`, so a playtest
leaves numbers on both machines. Beside the old two lines, the first
says how many frames ran over 16.7 ms, and a third (`netplay link:`)
gives the round trip (smoothed, least, most; measured on the reliable
hash chunks' acks, so it overstates by up to a frame of the peer's
loop) and how far this peer runs ahead of the other (time sync's smoothed
lead: now, average, range).

Kirby at delay 0, quack, interpreter, 5400 frames paced, both players
moving and jumping (`--input1 'start@400,start@700,a@800,a@950,down@1050,
a@1100,a@1820,a@1950,a@2100,right@2400-5400/90+40,a@2410-5400/50+8'`,
`--input2 '…,left@2420-5400/70+30,a@2430-5400/45+10,b@2500-5400/120+5'`):

| profile | round trip | rollbacks, re-run | frames over 16.7 ms | p50 / p99 / max | frozen | lead |
|---|---|---|---|---|---|---|
| calm | 60 ms (42-94) | 193, 400 (deepest 3) | 36-43 (0.7%) | 5.4 / 16.4 / 55 ms | never | ±1 frame |
| wander | 163 ms (65-271) | 192, 995 (deepest 11) | 180-233 (3-4%) | 6.0 / 38 / 75 ms | never | -3 to +3 |

So with rollback the lag is the CPU, not the wire: no freezes and time
sync holds the peers within a few frames, but each rollback re-runs its
depth (about 5 frames on wander) at 6-7 ms a frame, and that frame
comes out 2-3 frames late: a hitch a player feels every second or so
on a far line, more on a slower machine (the Mac's interpreter build).
At delay 2 that same session added 33 ms of input lag on top. Speed is
the fix (june, 2026-10-27: zero input delay): recompiled
netplay (multiplayer/recompiled-netplay), and the session with june on
it (multiplayer/recompiled-netplay-mac) should bring both sides'
report files.

### Resends backed off (multiplayer/udp-resend-backoff)

A 5-minute Kirby soak through fly (2026-10-27, both peers on quack,
delay 2) resent 143k and 171k datagrams and counted 508k waits, against
52-386 resent in each 90-second run before it. Two weaknesses in
`netplay::udp` could do that, and both are fixed (2026-10-29):

- **No backoff.** A timed-out chunk was resent every timeout, and by
  Karn's rule a resent chunk's ack measures nothing, so a timeout under
  the real round trip (its floor is 20 ms) never grew. Now each timeout
  doubles it, to 1 s, until a chunk sent once is acked. A peer that
  stops answering after the timeout reached its floor: 64 resends of one
  chunk in 1.3 s before, 6 after (`a_silent_peer_backs_the_resends_off`).
- **Go-back-N.** The ack was cumulative only, so one chunk lost from a
  64-chunk window had all 64 resent. Now the ack carries a bitmap of the
  64 chunks after the first missing one. A 400 KB state (341 chunks)
  through a proxy with 20 + 30 ms and a tenth lost: 393 resent for 140
  lost before, 85 for 81 after
  (`a_big_message_over_a_lossy_path_resends_about_what_was_lost`).

The soak's own trigger wasn't reproduced. Ten minutes (36000 frames,
paced, headless, delay 0, the inputs above extended to frame 36000) on
quack, the old and new builds side by side through fly (one pair per
room, 2026-10-29 00:10), and the old build through the proxy's wander
and rough profiles; freezing the joiner (SIGSTOP) for 10 s three times
in a 90-second calm run didn't trigger it either (26 resent old, 27
new: nothing reliable is in flight while play waits). Every run ended
on one hash. Each peer's end line:

```
old, wander: seat 1: 77194 datagrams, 492 resent, 2065 waits; seat 2: 77266, 440, 2105 waits
old, rough:  seat 1: 78348 datagrams, 1216 resent, 4221 waits; seat 2: 78499, 1212, 4604 waits
old, fly:    netplay: room GPAWTN, seat 1, 36000 frames, end 7e0c09082e340e25, 34690 transfers, 624 waits, 76443 datagrams, 84 resent, 2554 rollbacks, 5035 frames run again, deepest 12, 0 stalls, 51 held back
             netplay: room GPAWTN, seat 2, 36000 frames, end 7e0c09082e340e25, 34690 transfers, 543 waits, 76483 datagrams, 156 resent, 1955 rollbacks, 4211 frames run again, deepest 10, 0 stalls, 77 held back
new, fly:    netplay: room Y9ZGBT, seat 1, 36000 frames, end 7e0c09082e340e25, 34690 transfers, 554 waits, 76366 datagrams, 51 resent, 2548 rollbacks, 4912 frames run again, deepest 16, 0 stalls, 59 held back
             netplay: room Y9ZGBT, seat 2, 36000 frames, end 7e0c09082e340e25, 34690 transfers, 559 waits, 76383 datagrams, 86 resent, 1954 rollbacks, 4179 frames run again, deepest 10, 0 stalls, 76 held back
```

So over fly the new build resends 5-9 a minute, under the 90-second
runs' 35-257, and the old one, 8-16, did too on this night's path. Both
fly pairs' round trip was about 52 ms (least 32-35, most 139-217). If
a playtest's report file shows thousands resent again, the cause is
still open: note what the session did (a menu open, a slow start, the
machine swapping).

### Four players (multiplayer/four-seats)

From june's Kirby playtest (2026-10-29): a third player joining was told
the room was full, as rooms had two seats. Since 2026-10-31 a room has 2
to 4: the host picks how many (`gbaa-desktop --players N`, the menu's
Players row), joiners take the next free seat in the order they arrive,
and the session starts once every seat is taken (one more is told the
room is full). The server had always taken 2-4 (`relay::rooms`), and
the session core and `runtime::link` were already N-seat; the desktop
was what fixed two. The room's build ID leaves out the seat count (byte
28), which a joiner learns from the room before it makes its consoles.
`runtime::link`'s `four_players_share_one_bus` test runs four machines
on one cable: every player gets all four halfwords at one cycle, with
its own ID.

Checked on quack, 2026-10-31: `gbaa-pingsim room --players 3|4`, toy
and GBA consoles on wander, every seat on the straight run's hashes;
three and then four windows of the recompiled Kirby in a private `cu
session` through a local `gbaa-server`, each showing its own seat (P1
to P4 on Kirby's cable screen, then its own Kirby's colour in Rainbow
Route), all ending on one hash (three: `929be1bd…` at 3000 frames;
four: `2749087a…` at 3600).

Brought onto the public main on 2026-11-05, over peer-leaves and the
desync report, and checked again on quack (interpreted): `just room-sim
wander --players 4 --console gba` and `--players 3` pass on every seat;
three and four Kirby windows in a private `cu session` (calm, delay 2),
titled player 1 to 4, the corner mark `Connected · 3/3` and `4/4`, each
window on its own Kirby in Rainbow Route, ending on one hash (three:
`27f2caeb…` at 3000; four: `245b958b…` at 3600). With peer-leaves, a
headless four-peer run first ended apart: a peer that finished early
said goodbye while a slower one (four interpreted consoles run behind
real time) was 200 frames back, and that one unplugged its cable. A
peer at `--frames`' end now waits (up to 30 s, `room::CATCH_UP`) until
every seat has run as far before it lingers and leaves; headless, four
peers then ended on `c894de98…` (calm, delay 2, 3000 frames) and three
on `71a8ed2d…` (wander, delay 0). With three or four players, one leaving
unplugs everyone's cable: each remaining peer plays on alone.

Frame cost, four Kirbys a peer, quack, 3600 frames paced through `just
rollback-feel PROFILE ROM --exe BIN [--interp] --headless --paced
--players 4` with the menu presses above on all four, then from 2200
`right`/`left` every 18, 18, 30 and 24 frames on players 1 to 4. All four
peers ran on quack at once (16 consoles, three proxies), so each peer
shares the machine's cores and caches with three others, as a real
session's wouldn't: these are upper bounds. The structured build, its
default instant timing:

| players | build | profile, delay | frame p50 | frame p99 | frames over 16.7 ms | end hash |
|---|---|---|---|---|---|---|
| 2 | recompiled | calm, 2 | 3.5-3.8 ms | 6.7-9.2 ms | 1 | `db772d4f…` |
| 2 | interpreted | calm, 2 | 6.8-7.1 ms | 10.8-10.9 ms | 2 | `b67bf1e1…` |
| 2 | recompiled | wander, 0 | 3.7 ms | 27 ms | 94-100 | `41fb45c8…` |
| 4 | recompiled | calm, 2 | 7.0-7.3 ms | 19-26 ms | 79-145 | `47aeef69…` |
| 4 | interpreted | calm, 2 | 17.9-20.8 ms | 35-62 ms | 2160-3400 | `9d6fb426…` |
| 4 | recompiled | wander, 0 | 7.1-7.3 ms | 69-81 ms | 290-360 | `531a181f…` |
| 4 | interpreted | wander, 0 | 14.5-18.4 ms | 150-166 ms | 860-2430 | `d5a8ecef…` |

Every run's peers ended on one hash, and each setting on the same hash
every time it ran; none froze once playing. A frame of four Kirbys costs
about twice two, so the **re-run headroom** (frames a 16.7 ms frame has
room to run again) falls from about 3.5 to about 1.3 recompiled, and
the interpreter has none: at four it runs behind real time on quack
even with no rollbacks (most frames over budget). With four, a late
input from any of three others means a rollback, so at delay 0 on
wander each peer ran about 2100-2500 frames again a minute (deepest 14
to 107 frames), against about 830 for two; the p99 is a hitch of 4-5
frames. On the Deck (2-2.5x slower than quack, "Latency over the
internet" above) four recompiled Kirbys would take about 14-18 ms a
frame: about one frame's budget, no re-runs. An estimate, not measured
there: june decided on 2026-11-01 to leave the Deck's runs for later
("May a lane SSH onto your Deck to time four Kirbys' netplay": leave the
Deck for later), so they are their own unit (multiplayer/four-kirbys-deck),
run whenever june next says yes.

### The other players' sound (multiplayer/other-seat-sound)

From june's Kirby playtest (2026-10-29, through fly): her friend couldn't
hear june's Kirby. The window plays its own seat's console, as your own
GBA would, and rollback runs frames again quietly (`Pad::quiet`), so
either Kirby doesn't sound the other Kirbys, or rollback drops them.
**It's the game: Kirby & the Amazing Mirror's console plays only its own
Kirby's sounds**, even with the other Kirby on screen; rollback drops
none.

Measured 2026-11-08 on quack by `crates/netplay/tests/seat_sound.rs`
(ignored: it needs the ROM and its save; `cargo test --release -p
netplay --test seat_sound -- --ignored --nocapture`, about 3 minutes):
two GBAs on a cable in the interpreter, both on the same save, the menu
presses of the runs above, then from frame 2400 seat 1 jumps in place (A
for 10 frames every 60) or stands still, seat 0 idle throughout:

- **Straight** (no rollback, as two GBAs on a real cable): seat 1's own
  sound differs on 258 of the 1200 frames (each jump, 9 frames, and its
  landing, 3-4), and seat 0's picture differs (on 1180 frames; 598 pixels
  20 frames in, the yellow Kirby in the air beside the red one), but seat
  0's sound is the same on every frame, sample for sample.
- **Rollback** (60 ± 40 ms one way, 2% loss; 52 rollbacks, 293 frames run
  again on seat 0's peer): what seat 0's speaker heard is the straight
  run's seat 0 sound on all 3600 frames, and the end hash is the straight
  run's.

So there's nothing to make play: the other player's jumps make no sound
on your GBA either, and the netplay window is as faithful as a cable.
Hearing the others would be a mod (mixing in another seat's sound, or
the game's), not a fix. Rollback can still cut the start of a sound in a
game where your console does sound the other players: a sound started on
a frame run again plays from where the re-run left it.

## Playing a friend (the play gate)

What june needs for multiplayer/play-gate, from what's built (2026-10-25):

1. **The server**, on fly.io (june's choice, 2026-10-26): the app
   `gbaa-server`, its rooms on UDP 7374 at 37.16.3.41 (the dedicated
   IPv4 fly needs for public UDP, billed; `just fly-setup` made it).
   `just fly-up` before playing, `just fly-down` after; `just
   fly-deploy` after a server change. It keeps no files, so the 1 GB
   volume `gbaa_data` that held the QUIC certificate isn't needed any
   more (still billed until june runs `fly volumes destroy`). `just
   room-sim calm --server 37.16.3.41` checks it end to end with the toy
   game first. docs/tools/commands.md, "The server on fly.io".
2. **The friend's build**: the same commit's `gbaa-desktop` (Linux:
   `cargo build --release -p desktop`; Windows: `just windows-exe`'s
   `out/windows/`, untested for netplay), and their own clean US Emerald:
   the room refuses a peer whose ROM CRC, size or snapshot version
   differs from the host's.
3. **Play** (by rollback, the only netplay): june `gbaa-desktop ROM
   --server 37.16.3.41 --host` reads the room code from the title bar;
   the friend `gbaa-desktop ROM --server 37.16.3.41 --join CODE`. The
   port defaults to the server's, 7374, which deploy/fly/fly.toml opens. Each brings
   their own save (default beside the ROM). Emerald: both walk into a
   Pokémon Center's cable club, 2F: Trade Center for the trade,
   Colosseum for the battle. Kirby: both pick a file, Start Game, Multi
   Player. `--rollback` still parses and changes nothing.
4. **Known gaps:** the menu pauses both, closing the window ends the
   session for both. A desync is printed in the terminal (`desync N`)
   and not resynced; worth noting.

## Rogue Assistant

Read 2026-10-28 from
[pokeemerald-rogue-assistant](https://github.com/Pokabbie/pokeemerald-rogue-assistant)
(`RogueAssistantCpp/Src/Behaviours/MultiplayerBehaviour.cpp`,
`CommonBehaviour.cpp`, `ObservedGameMemory.cpp`, `Inc/GameData.h`) and
the game's side, pokeemerald-rogue's `src/rogue_multiplayer.c` and
`data/maps/Rogue_Interior_PokeConnect/scripts.pory`. Built as
`crates/desktop/src/rogue.rs` (`gbaa-desktop ROM --rogue ADDR`).

**How it reaches the game.** `RogueAssistant_mGBA.lua` loads a native
module (Windows only) and gives it nothing but mGBA's `emu:readRange`
and `emu:write8/16/32`, once a frame. No link cable, no sockets in Lua,
no debugger: the module reads guest memory, writes guest memory, and
does its own networking.

**The header.** The ROM's GF header carries, at `0x08000178`, the word
20012, a pointer to a `RogueAssistantHeader`, and 30035. The header is
two bytes (version, debug), two of padding, then 34 words: the compat
version (3 in our `Emerald Rogue.gba`, what the current assistant wants;
the pre-2.2.1 copy has 2), the assistant's confirm word's size and offset
in `gRogueAssistantState`, then the multiplayer blob's layout, then home
box fields, then six pointers, of which two matter here: the assistant
state's address and the address of the blob's pointer. Ours:

| field | value |
|---|---|
| blob size | 1440 bytes |
| handshake | 48 bytes at 1388; its state byte at +42, the player ID at +43 |
| game state (the host's) | 868 bytes at 520 |
| player profiles | 2 × 40 bytes at 0 |
| player states | 2 × 220 bytes at 80 |
| request / current state | the bytes at 1436 / 1437 (bit 0 active, bit 1 host) |
| assistant state | `0x02038270`, confirm word 2 bytes at +54 |
| blob pointer | at `0x02038e90` (null with no session) |

**The game's side.** Talking to the PokeConnect clerk in the hub (after
the game has seen an assistant and saved) asks Host or Join. Host
(`RogueMP_OpenHost`) points the blob pointer at the blob and sets the
request to active+host; Join (`RogueMP_OpenClient`) writes the player's
profile and versions into the handshake, its state "send to host", and
sets the request to active. The game waits (B cancels) until its current
state turns active. Every frame the game counts in the confirm word; the
assistant keeps writing it to 0, and a count that runs on means "no
assistant".

**The assistant's side**, each frame:

1. Write 0 to the confirm word. Read the blob pointer; null means no
   session (close it). Else read the blob.
2. A nonzero request opens a session: host listens on a UDP port (30025
   unless the player types another), client connects to an address,
   both ENet with 4 channels (0 handshake, 1 game state, 2 player state,
   3 profiles). A request that later changes closes it.
3. Host: on opening, write current = request (the game is now hosting).
   When a client's handshake arrives, write it into the host's game and
   stop reading the network until the game answers (state "send to
   client", the seat in the player ID byte, or refusal for a different
   flavour, save version or revised mode); send that answer back, and
   note the client's seat.
4. Client: once ENet connects and the game's handshake says "send to
   host", send it (reliable). When the answer comes, write it into the
   game, take the seat, and write current = request.
5. Connected. Host: when the profiles change, broadcast them (reliable);
   the game state 5 times a second (reliable); all player states 30
   times a second (unreliable). Client: its own player state 30 times a
   second (unreliable). Each side writes what it receives into its game:
   the host only the sender's seat of the player states, a client all
   but its own.

So the game does everything else: positions, followers, trades and the
host's run seed travel in those blobs, and each game draws the other
players from them. Two players today (`NET_PLAYER_CAPACITY`); the host
always gives seat 1.

**Ours.** `rogue::Assistant` does the above on the desktop's frame loop,
reading and writing `m.bus` after each frame, at frame counts (every 12th
and 2nd frame) where the assistant uses wall-clock timers. `--rogue ADDR`
gives the address once, for both roles; which role is the game's choice
in its menu. Tests: `rogue::tests::two_games_see_each_other` runs a
host and a client over localhost UDP against a stand-in for the game's
side (`rogue_multiplayer.c`'s handshake and player-state writes);
`rom_header_reads` the header. It goes over ENet directly, not through
gbaa-server: a host must be reachable (port forwarding), as with the
assistant. Carrying it through gbaa-server's rooms would remove that and
is a later step; it would lose mGBA interop for that path only.

**Wire compatibility with mGBA + Rogue Assistant: yes, by construction.**
The assistant links ENet 1.3.17 (`External/ENet`). `rusty_enet` 0.4 is
ENet 1.3.18 transpiled to Rust (commit 2662c0d). Between v1.3.17 and that
commit `include/enet/protocol.h`, which defines every command on the
wire, is unchanged; the changes are send scheduling, `ENET_SOCKOPT_TTL`,
a checksum table (neither side uses checksums or compression) and MTU
negotiation, where 1.3.18 takes the smaller of the two MTUs (default
1392 against 1400), which a 1.3.17 peer accepts. The payloads are the
raw blob spans, no framing of the assistant's own, and the channel
numbers, sizes and packet flags above match. Not yet tried against a real
mGBA with the assistant (Windows only): a run against C ENet 1.3.17
built here would be the cheap check.

## The cable over the internet: modes, timing, games (multiplayer/link-cable-survey)

june's question (2026-10-29): can netplay stand in for any game's link
cable, and does the cable assume no lag? Short answers, worked out on
quack on 2026-11-08:

- **The cable assumes no lag.** A transfer finishes in microseconds,
  and the games' protocols are built on that: Emerald's parent flags a
  link error in the very frame it gets fewer than 9 of its serial
  interrupts back. Sending the cable's bytes over the internet can't
  work. What netplay sends instead is button presses, and every peer
  runs every console, so each peer has the whole cable in its own
  process, with no lag (see Summary).
- **So netplay covers any game that uses a mode the link hub emulates,
  and nothing else is per game.** Today that is multiplayer mode, the
  mode of almost every GBA-to-GBA cable game. Normal mode, UART, JOY
  bus and the wireless adapter aren't emulated: a game that uses one of
  them sees no partner, just as with no cable plugged in.
- **Tried through two local peers** (`gbaa-pingsim desktop`, headless,
  paced): Emerald's trade, Kirby & the Amazing Mirror's multiplayer and
  Nightmare in Dream Land's Quick Draw all work, with both seats on one
  end hash and no desyncs.

### Each mode, and how fast it is

A frame is 280,896 cycles (16.74 ms). Transfer times are for one
transfer, from GBATEK and mGBA's `src/gba/sio.c`; the "link error"
column is what a game does when transfers stop coming.

| mode | how a transfer works | one transfer takes | when the game calls it a link error | `runtime::link` |
|------|---------------------|-------------------|-------------------------------------|-----------------|
| Multiplayer (16-bit, 2–4 GBAs) | the parent starts it; every console's halfword reaches every console at once | 3,194 cycles (190 µs) for 2 players at 115,200 bps, 11,863 (707 µs) for 4; 8.5 ms for 4 at 9,600 bps | Emerald (`src/link.c`, `LinkVSync`): the parent in the frame its V-blank sees fewer than 9 serial IRQs (`LAG_MASTER`), a child after 10 V-blanks with none (`LAG_SLAVE`), a player-data exchange after 600 frames. Not measured in the others | **yes** (2026-10-25; 2–4 players, 2026-10-31) |
| Normal 8-bit / 32-bit | one side clocks a shift register, the other is clocked | internal clock 256 kHz: 524 cycles (8-bit), 2,097 (32-bit); 2 MHz: 67 and 268 | the hardware waits forever for an outside clock; any timeout is the game's | no: no partner |
| MultiBoot (single-pak) | BIOS `MultiBoot` (SWI 0x25) sends a program of up to 256 KB to children that have no cartridge, over multiplayer mode at 115,200 bps or normal 32-bit mode; a child boots it from RAM | in multiplayer mode a halfword per transfer, about 190 µs, so 256 KB would take about 25 s; real single-pak programs are smaller and send in a few seconds (from memory) | the BIOS's handshake retries, and the game's own screen times out | no: our HLE `MultiBoot` always fails (`runtime/src/bios.rs`), and a seat can't start without a cartridge. Games that send their own program (Nightmare in Dream Land) work if both seats have the cartridge |
| UART | RS-232-style bytes, a 4-byte FIFO, CTS flow control | 10 bits a byte: 87 µs at 115,200 bps | the game's | no: no partner |
| JOY bus | a GameCube polls; the GBA's hardware answers reset, status, read and write commands | a few hundred µs per command, clocked by the GameCube (from memory, not checked) | the GameCube game's | no; needs a GameCube on the other end, which we don't run |
| General purpose | the four pins as plain I/O | — | — | not linking |
| Wireless adapter | a small computer on the port; the game sends it commands over normal 32-bit mode (librfu's "STWI"), and the adapter does the radio | each command is a few 32-bit transfers at 2 MHz; the adapter answers within a frame or so (afska's `gba-link-connection` docs) | librfu's per-command timeouts, then the game's (not measured) | no: no adapter answers, so games say none is attached |

Why every peer runs every console, in one line each:

- **Forwarding the cable's bytes needs a round trip per transfer.**
  Emerald does 8 a frame, each needing every console's halfword. Even a
  20 ms ping is more than a whole frame per transfer, and Emerald's
  parent gives up the first frame it falls short. mGBA's maintainer says
  the same of naive network link, and it is why VBA-M's network link
  times out.
- **Inputs are a few bytes a frame and can arrive late.** Rollback
  re-runs the frames a late press changed (see "Rollback"), so each
  console sees a cable with no lag. What the network's lag costs is the
  other players' Kirbys and trainers jumping into place, never a link
  error.
- **The same hub serves every multiplayer-mode game.** Nothing in the
  hub or the netplay session knows which game it runs: Nightmare in Dream
  Land's Quick Draw worked through two peers on its first try. Most
  link games use multiplayer mode, so netplay is a generic replacement
  for those. Each other mode needs its own emulation in the hub, once,
  and then it works for every game that uses that mode.
- **Not a fit for the wireless adapter.** Its Union Room gathers more
  players than one peer should run (see Summary), so the wireless
  adapter would have each player run only their own console, with the
  server as the adapter's radio.

### Popular link games, by mode

From memory and from each game's manual text; not checked against the
ROMs unless the game is in the table of games tried below.

| game | mode | what it needs that we lack |
|------|------|----------------------------|
| Pokémon Ruby, Sapphire, FireRed, LeafGreen, Emerald | multiplayer mode (trades, battles, Record Mixing, Berry Blender, contests); FireRed, LeafGreen and Emerald also the wireless adapter (Union Room); JOY bus to Colosseum, XD and Box | the adapter; JOY bus means a GameCube |
| The Legend of Zelda: Four Swords (with A Link to the Past) | multiplayer mode, 2–4, every player needs a cartridge | nothing, in principle (not tried: no ROM) |
| Mario Kart: Super Circuit | multiplayer mode with every player's cartridge (all tracks); single-pak MultiBoot (fewer tracks) | the multi-pak game: nothing (not tried); single-pak: MultiBoot |
| Bomberman Tournament, Bomberman Max 2 | multiplayer mode, multi-pak; Tournament also single-pak | single-pak: MultiBoot |
| Super Mario Advance 1–4 (Mario Bros. Classic), many puzzle games (Puyo Pop, Tetris Worlds) | single-pak MultiBoot, 2–4 | MultiBoot |
| Kirby & the Amazing Mirror | multiplayer mode, 2–4, every player needs a cartridge | nothing: works |
| Kirby: Nightmare in Dream Land (sub-games) | multiplayer mode; the parent sends the sub-game ("Transmitting…") | nothing when every seat has the cartridge: works |
| Final Fantasy Crystal Chronicles, Four Swords Adventures, Metroid Fusion with Prime, Wind Waker's Tingle Tuner | JOY bus, a GBA as a GameCube controller or screen | a GameCube; out of scope |

### Games tried through two local peers

On quack, 2026-11-08, through `gbaa-pingsim desktop PROFILE ROM --
--headless --paced`, both peers interpreted on one machine (loaded:
other lanes' jobs, load average 10–18). The serial mode is what
`SIOCNT` read each frame (an mGBA Lua script, run unlinked: `--script`
isn't for `--link`).

| game | mode seen | run | result |
|------|-----------|-----|--------|
| Pokémon Emerald | multiplayer, 115,200 bps (`src/link.c`) | `just emerald-netplay-trade calm`, then `wander` twice: alice.sav and bob.sav through the cable club to a trade, 8,400 frames | **works.** calm: both end `35d6b1be…`, 43,960 transfers, the trade in both saves. wander: once the same pass, once the peers split at frame 1,119 (a 3.4 s stall on one, then each played on alone), on a loaded machine; filed as its own unit |
| Kirby & the Amazing Mirror | multiplayer, 115,200 bps (`SIOCNT` 0x6003 at the cable screen) | wander, 3,000 frames, the menu presses from "Four players", then left and right | **works.** both end `47204b2e…`, 18,186 transfers, no desync, p50 7.0 ms a frame |
| Kirby: Nightmare in Dream Land | multiplayer, 115,200 bps (`SIOCNT` 0x6003 at "Transmitting…") | wander, 7,000 frames: `start@300,a@1000,a@1300,down@1500,a@1600,a@1900,down@2100,a@2200` on both (file 1, Sub-Games, Quick Draw, Multiplayer), `a@3700` on player 1 ("OK?"), then A presses through the duel | **works.** both end `24d71dde…`, 17,937 transfers, no desync; the duel's screen shows both Kirbys on each seat. p50 23 ms a frame, interpreted, on the loaded machine: slower than real time. On one local hub (`--link`) the same presses reach Quick Draw's title |
| Emerald Rogue, Emerald Imperium | Emerald's cable code (hacks of it) | not tried: they need saves standing at a cable club. Rogue's own multiplayer goes through Rogue Assistant, not the cable (see "Rogue Assistant") | — |
| Minish Cap, Metroid Zero Mission, WarioWare | no GBA-to-GBA link play | — | — |

Gaps filed as units, each with what it changes:

- **The wireless adapter** (multiplayer/wireless-adapter-local): Emerald's
  Union Room, first with both consoles in one process. This is the gap
  that matters for june's games. It also needs normal 32-bit mode on the
  hub.
- **Single-pak MultiBoot** (multiplayer/single-pak-multiboot): a seat
  with no cartridge that boots the program the parent sends (the BIOS's
  `MultiBoot` over the hub). For Mario Kart's and Bomberman's single-pak
  modes and the Mario Bros. Classic games. None of june's ROMs need it.
- **Emerald's netplay trade splitting on wander under load**
  (multiplayer/wander-split-under-load): the one failure above.

Not filed: plain normal-mode GBA-to-GBA play (we found no popular game
that uses it, and the adapter unit covers its 32-bit form), UART (no
games), and JOY bus (it needs a recompiled GameCube on the other end).

## Risks

- **Determinism.** Anything that differs between peers desyncs: the build,
  the RTC, uninitialised state, a host-dependent float in the sound mixer
  (sound isn't part of the hash, but anything that feeds back into the
  game is). Mitigation: a sync test (run one session twice with forced
  state reloads and compare hashes) in the check, as GGRS's
  `SyncTestSession` does, and the build ID in the handshake.
- **Each peer needs every ROM, recompiled.** Linking Emerald with Rogue
  means both peers have both games built. Our games are separate binaries
  today; a session of two different games needs one binary with both, or
  a runtime that loads a recompiled game as a library. A design question
  for the first two-game unit.
- **Saves cross the network.** Your save goes to the other peers for the
  session; fine between friends, worth saying in the UI.
- **The hub's timing may not be fine enough** for a game that sends
  faster than once a block: the fix is decision 11's own "split blocks
  after constant I/O stores", only for the link hub's machines.
- **The wireless adapter** is undocumented hardware with timeouts we
  haven't measured; Union Room could turn out to need option C-style
  hooks after all.
- **Test saves.** The cable club needs a save past the first Pokémon
  Center. None is on quack for Emerald, so `just emerald-saves` crafts
  two (docs/tools/commands.md): the game's own new-game save blocks,
  patched. A crafted save lacks story flags a played one has; anything
  past the cable club may need more set.
- **Rogue co-op is only as good as Rogue's own netcode**: each player's
  game is its own, and the host's state reaches clients 5 times a second
  (see "Rogue Assistant"). Nothing of ours to fix there; it is the mod.

## Plan of units

In order; a new group, "Multiplayer", after june's answers. Each is under
the 3-hour cap.

1. **Measure frame cost and save size for netplay** (quack and the Deck):
   overworld and battle frame times, state save/load times, payload sizes;
   settles the input-delay and rollback budget. Heavy. Done on quack
   (2026-10-25, "Latency over the internet"); the Deck is its own unit,
   waiting on june's OK to use it.
2. **Emerald saves at the cable club, for link tests**: two saves (two
   trainers, a Pokémon each) standing in a Pokémon Center, from a TAS or
   june's save; kept outside git like ROMs.
3. **A link hub: two GBAs on one serial bus**, multiplayer mode
   (`runtime`, no_std). Check: two Emeralds from those saves reach
   `LINK_STATE_CONN_ESTABLISHED` (symbols) and finish the cable club's
   handshake, identically twice. Heavy.
4. **Two linked GBAs in one window, local**: the desktop runs a hub, two
   pads or a split keyboard. Check: a trade between the two saves
   completes (a short input script). Done (2026-10-25, `--link`,
   `just emerald-trade`).
5. **The netplay session core, over an in-memory transport**: the
   `Console` trait, lockstep with input delay, start from states, hashes,
   resync; the GBA hub's `Console` impl. Check: a sync test with injected
   latency, jitter and loss gives the same hashes as a straight run.
   Done (2026-10-25, `crates/netplay`, `just emerald-netplay`; see
   "Session core").
6. **The relay server: rooms, codes, QUIC**. Check: two headless clients
   play a session through a local server under injected latency. Done
   (2026-10-25, `gbaa-server`; see "Relay server"); QUIC removed
   2026-10-28, its check now `just room-sim`.
7. **Desktop: host and join a room, play your seat**. Check: two windows
   on one machine through the server. Done (2026-10-25, `--server`,
   `just emerald-netplay-trade`; see "Desktop netplay").
8. **june trades and battles with a friend over the internet**: the play
   gate (recorded by june).
9. Later, each its own research or unit when wanted: the wireless adapter
   locally, then networked (Union Room); rollback; spectating in the UI;
   shared overworld presence (after the Lua research); what Rogue
   Assistant talks to; two different games in one session.

## Questions for june

Asked with this unit (`just ask`), each blocking the units it changes:

- **What first:** cable trades and battles with a friend (the plan above;
  default), or the Union Room over the wireless adapter (option F first,
  more players, harder), or a shared overworld (presence first; no trades
  until later).
- **Where the server runs:** a machine of june's that's always on
  (default; which one, and reachable how: a port forward or a tunnel), or
  no server at all with one player hosting (the host forwards a port), or
  a small rented VPS.

## Sources

- pokeemerald decomp: `src/link.c` (cable), `src/link_rfu_2.c`,
  `src/librfu_*.c`, `src/union_room*.c` (wireless), `include/link.h`.
- GBATEK, serial communication sections (problemkaputt.de/gbatek.htm).
- mGBA network link issue: github.com/mgba-emu/mgba/issues/2379.
- Dolphin's integrated GBA: dolphin-emu.org/blog/2021/07/21/integrated-gba/.
- RetroArch netplay and run-ahead: docs.libretro.com/guides/runahead/,
  emulation.gametechwiki.com/index.php/RetroArch.
- GGRS: github.com/gschup/ggrs; docs.rs/crate/ggrs.
- afska's gba-link-connection (cable and wireless adapter docs):
  github.com/afska/gba-link-connection.
- Pokémon Emerald Online (2026):
  generationamiga.com/2026/08/06/pokemon-emerald-online-turns-the-gba-classic-into-a-browser-mmo/.
- VBA-M's link port: forums.dolphin-emu.org (VBA-M and Dolphin link threads).
- Emerald Rogue's own text (decoded from the ROM on quack, 2026-10-23):
  "Multiplayer Adventures", "Rogue Assistant", "only supported when
  playing using mGBA on Windows PC".
