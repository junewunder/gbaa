# Could compiled code stop depending on ROM addresses?

research/code-without-rom-addresses, 2026-10-31. For june, who read
moonbeam's mod view (instant timing, all readable) and wants the ROM
addresses gone from lines like

```rust
call!(m, pc, f_08002065 @ 0x0800_2065 -> 0x0800_bc3f, 6, { regs!(m; 0 = 0x815_5728, 14 = 0x800_bc3f); });
```

Research only. Redone from scratch the same day: an earlier session's
draft was set aside, and everything below was read, run or measured for
this version. Its census tool (`crates/interp/examples/returns.rs`) is
still in the tree; this doc rests on a different experiment (below).

## Summary

- **The addresses can leave the code's text. They can't leave the game's
  state, and that isn't our design's doing: the games use them.** Emerald
  compares function pointers hundreds of times
  (`gMain.callback2 == CB2_Overworld`, finding a task by its function) and
  splits them into halves to store in task data
  (`(u32)StartTeleportFieldEffect >> 16`); Kirby picks object behaviour
  by comparing state-function pointers (`kg1->obj.unk78 ==
  sub_080DB7DC`). A ROM code address is a value in these games' RAM, the
  same as an HP number. Every recompiler surveyed keeps them for function
  pointers, including the two that drop return addresses.
- **Return addresses are the one kind that could, in principle, be
  something else, and almost no game code looks at them.** Eight games
  run twice side by side, the second with a scrambled token in lr in place
  of every return address (translated back only when the game jumps to
  it): in 90 seconds of each, 6.9 million calls got a token and seven of
  the eight games did nothing different. WarioWare did: a hand-written ARM
  loop in IWRAM calls a helper that does `sub lr, lr, #24` and returns
  there, a short way back into the loop. That's a game doing arithmetic
  on its return address, and any scheme without real addresses breaks it.
- **Our runtime needs them too, and changing that gains no speed.**
  Kirby's compiled calls came back somewhere other than their return site
  41,382 times in 90 seconds (the callee's tail ran in the interpreter:
  Kirby's RAM code, uncovered code); a build with plain host returns
  stopped running the game within its first frames. Restores (save
  states, rewind, netplay rollback) and the interpreter resume only
  through real addresses. A build with the return check made as cheap as
  it can be ran no faster than today's (same frame hash, times within
  noise).
- **Recommendation:** keep real addresses in guest state; take the
  numbers out of the text by naming them, the way an assembler's labels
  do. The two units already filed (`call!` writes lr and takes the
  callee's key itself; a mod's own calls need no address) are the first
  steps, and a `#[gba]` proc macro for mods (june's other thread) fits on
  top of them.

## What the addresses in a line of code are

Read from `crates/codegen/src/emit.rs` and `crates/runtime/src/dispatch.rs`.

| In the text | What it is | Who reads the value |
|---|---|---|
| `f_08002065` | the callee's Rust name | the compiler only |
| `@ 0x0800_2065` | the callee's entry key, passed as its block argument and written to `pc` before the tick | the dispatcher, if the tick yields there (an IRQ that can't run in place, the frame end without a guest stack); snapshots, as `cpu.pc` |
| `-> 0x0800_bc3f`, `14 = 0x800_bc3f` | the return site, written to lr | the callee, which pushes lr into guest RAM and returns through it; then `settle` compares where it came back to with the site |
| `tick!(m, pc = …)` | the block's key | an IRQ taken in place returns to it (`yield_at`); a resume starts from it |

Two facts about the runtime decide the rest:

1. **The machine's state is always complete; the host stack is a cache.**
   At each tick and call, codegen writes Rust locals back into the
   machine (`regs!`), and the guest stack is guest RAM. So a restore can
   throw the host stack away (`desktop/src/coro.rs`: "Dropping a suspended
   guest (a state load, a rewind) discards its frames without running
   them") and start a new guest from `cpu.pc`.
2. **The interpreter is how execution resumes anywhere.** After a
   restore, the dispatcher runs the interpreter from `cpu.pc` until it
   reaches a compiled entry, and every function that was open is left
   through the return addresses on the guest stack (`run_inner`'s
   fallback). For that, `cpu.pc` and every stacked return address must
   be real ROM addresses the interpreter can fetch from.

## Who needs real addresses in the state

- **The game itself**, for function pointers (above). Return addresses
  are the only kind the games don't inspect (the experiment below).
- **Save states, rewind and netplay's rollback**: restores land wherever
  the snapshot was taken and get back out through stacked return
  addresses. The netplay-after-restore unit (done 2026-10-31) depends on
  this: it made never-returning functions print with every block a key
  so a restore can land anywhere in them.
- **Save state files across builds and from mGBA**: our state's header
  checks the ROM's CRC, not the build (`runtime/src/snap.rs`), so a state
  outlives a rebuild, and `--state` loads mGBA's states, whose stacks
  hold mGBA's real addresses.
- **The interpreter fallback**, which runs code nothing compiled
  (Kirby's RAM code, 33.5 points of its cycles in the timing run below)
  and calls into and returns out of compiled code with real `bl`s.
- **The byte-for-byte checks** (`same-states`, `own-state-check`,
  `rewind-check`, the mGBA comparisons): a token in a stack slot would be
  a difference on every frame.

## Do the games read their return addresses?

`cargo run --release -p interp --example lr_numbers -- ROM --frames N
--press KEY@F-G/E… [--skip ADDR]… [--self-test]`
(`crates/interp/examples/lr_numbers.rs`) simulates the scheme june asked
about. Two machines run the ROM in lockstep, one instruction at a time,
with instant timing. In the second, every call (an instruction that
leaves lr at the instruction after it: `bl`, or a jump with lr set) gets
a token in lr instead of its return address: a value outside the GBA's
memory map with the address's bits scrambled. When the second machine
jumps to a token it is translated back before the next instruction, so
both always run the same code at the same addresses. What must stay
equal: after every instruction the clock, pc and CPU mode; at every frame
end all of RAM, VRAM, OAM, palette, the I/O registers and the drawn
frame (a RAM word may differ only by holding part of a token). Code that
adds to a return address and jumps there, loads through it, compares it
and branches, or stores something computed from it, makes them differ,
and the run stops and names the call.

A register or flag holding a value computed from a token changes nothing
by itself, so those are counted and shown, not stopped on: agbcc writes
a halfword field of a stack struct with a word-wide read, mask and write,
carrying the struct's uninitialised other half (a stale return address)
through a register before it is overwritten.

**Controls.** `--self-test` puts a few Thumb instructions in IWRAM that
call moonbeam's own `__gnu_thumb1_case_uqi` (libgcc's switch helper: it
reads a byte table at its return address and returns past it), and the
tool must catch it: it stops at the helper's `bx lr`, which lands
somewhere else. With that call left its real return address
(`--skip`), nothing differs. moonbeam is the only one of the eight with
these helpers, and the routes below never reached its callers (deep in
its bundled JavaScript engine and C library).

Each game ran 5,400 frames (90 seconds) from reset with Start every 2 s
from frame 300, A every half second from 400, B every 0.75 s from 2500,
and right, up and left held in turn from 2000.

| Game | Instructions | Calls given a token | Result |
|---|---:|---:|---|
| Kirby & the Amazing Mirror | 370 M | 825,308 | no difference |
| Pokémon Emerald | 947 M | 499,076 | no difference (73,357 register-only, the stale-stack carry above) |
| moonbeam | 770 M | 1,275,092 | no difference |
| The Minish Cap | 212 M | 842,002 | no difference |
| Metroid: Zero Mission | 229 M | 705,797 | no difference |
| WarioWare, Inc. | 798 M | 1,430,870 | **returns to `lr - 24` at frame 40**; with that helper and its twin skipped (1,653 calls), no difference |
| Kirby: Nightmare in Dream Land | 781 M | 499,271 | no difference |
| Emerald Rogue | 124 M | 797,925 | no difference (5,088 register-only, the same carry) |

WarioWare's code, copied from ROM `0x0800_07e4` to IWRAM:

```
3000444:  bleq  0x3000468         ; a shortcut back into the loop
3000468:  orr   sl, r5, #0x81000000
300046c:  sub   lr, lr, #24       ; return 24 bytes before the return site
3000470:  b     0x3000480
3000474:  add   r6, r6, r5, lsl #1
3000478:  orr   sl, r5, #0x80000000
300047c:  sub   lr, lr, #20       ; the twin, 20 bytes back
...
3000490:  bx    lr
```

Today's build compiles that `bleq` as a jump that writes lr, and the
helper's `bx lr` goes through the dispatcher to whatever lr holds, so it
works because lr is a real address.

A static pass over each build's compiled instructions found lr named as
an operand in hundreds of places per game, nearly all ARM code using it
as a scratch register once the return address is pushed (Metroid's
IWRAM routine loads a table base into it); telling those from a read of
the return address takes data flow, which is what the dynamic tool
gives.

Limits: dynamic runs over boot, menus and early play; code the routes
didn't reach isn't covered, and moonbeam's switch helpers show there is
such code. The return addresses the IRQ entry makes aren't tokened.

**Correction.** An earlier version of this experiment (the same day)
moved return addresses to the ROM's other mirror instead of a token and
reported all eight games identical. A review found it tested only about
half the calls and counted any two values one mirror apart as equal,
which hid additions, masks and loads through lr; the switch helper would
have passed it. The token version above replaced it, with the controls
added to show it catches what the old one couldn't.

## What returning natively would gain

Measured on Kirby's play build (structured, instant timing, recompiled
2026-10-31), headless, 5,400 frames with the same presses; `perf` isn't
allowed on quack, so builds were timed against each other, interleaved.

- **Today's build:** 9.9, 10.3, 9.9, 10.2 s; frame hash
  `39407640b97baeb0`.
- **The return check inlined** (`pc = if a == ret { a } else {
  settle(…) }` at all 17,452 call sites, so the common case is one
  compare in place of an out-of-line call): 10.3, 10.2, 9.8, 10.6 s; the
  same hash. No measurable difference.
- **Plain host returns** (`settle`, the stack-pointer save and the
  yield check deleted, `pc` set to the return site): the game stopped
  running its code within the first frames and sat halted for the rest of
  the run (1.5 billion halted cycles, a different final frame). The
  build's counts explain why: 41,382 calls in the correct run came back
  elsewhere first and were settled in place, because their callee's tail
  ran in the interpreter.

So the check is needed while the interpreter runs any code (and for
WarioWare's helper), and it already costs nothing measurable. What a native return can't remove
either: the lr write and the push and pop of it, which are the guest's
own instructions storing into guest RAM.

## How others do it

Read in their sources (commits in `docs/research/static-recompilers.md`)
and papers:

- **N64Recomp drops return addresses entirely.** `jal` is a host call
  that doesn't write `$ra` (`src/recompilation.cpp`, `cpu_jal`), so the
  game's `sw $ra` stores a stale value; `jr $ra` is `return`. It has no
  save states, and its `longjmp` support is commented out
  (`N64ModernRuntime librecomp/src/ultra_translation.cpp`). Function
  pointers still go through `LOOKUP_FUNC(vram)`: a guest address mapped
  to a host function at run time.
- **XenonRecomp makes lr optional** (`skip_lr`), "assuming the game does
  not utilize exceptions"; `blr` is `return`; indirect calls go through
  `PPC_LOOKUP_FUNC`, keyed by guest address. No save states.
- **PS2Recomp writes `$ra`** on every `jal` and `jalr`
  (`control_flow_emitter.cpp`, `emitStaticJump`).
- **gbarecomp (arm-recomp-core), a GBA recompiler, is ours:** `bl`
  writes the real `R[14]`, pushes the return on a host-side return
  stack, makes a host call, and checks `R[15]` against the return site
  after it, falling back to its dispatcher (`profiles/armv4t_gba/
  arm_codegen.cpp`, `emit_direct_branch`). Its interpreter bridge stops
  at the top return address, and entries restored from a snapshot are
  kept as "hostless" hints (`src/armv4t/runtime_arm.cpp`).
- **rev.ng's isolated translation (Gussoni et al., BAR 2019) is ours
  too:** two stacks, the emulated one evolving "exactly how it would have
  evolved in the original program", the host one holding nothing vital
  so "unwinding through it and discarding its content does not corrupt
  the semantic of the original program"; after every call a "bad return
  address" check against the fall-through, with a fallback to the
  dispatcher.
- **Huang, Chen, Baron and Franz, "No Bit Left Behind" (arXiv, 2026):**
  "Because the return address lives in emulated stack memory that the
  callee may have written to, we cannot lower ret as a native LLVM
  return; the indirect dispatcher is the only correct lowering."
- **Dynamic translators reached the same rule.** Bruening's DynamoRIO
  work calls it transparency: placing code-cache addresses on the
  application stack is a tempting shortcut, but "every shortcut like this
  violates some program's dependencies" (thesis defense, slides 39-40).

So the tools that drop return addresses are the ones with no save
states, no interpreter and no exceptions; every tool with resumable
execution keeps them, and none drops code addresses as function pointers.

## A mixed scheme

Native returns where it's provably safe would need, at a call site: the
callee and everything it reaches compiled (no interpreter, no overlay);
no restore while it is open; lr only saved and returned through. The
second fails nearly everywhere (rollback restores every frame in
netplay; the main loop is always open), and the gain measured above is
nothing. Not worth a second call protocol.

## What to do instead: names in the text

The state keeps addresses; the text can refer to them by name, with one
generated table binding names to addresses.

1. **`call!` writes lr itself and takes the callee's key from the callee**
   (filed: `mod/call-writes-lr`). `f_08002065 @ 0x0800_2065 -> 0x0800_bc3f`
   becomes `f_08002065 -> 0x0800_bc3f`, and `14 = …` goes from `regs!`.
   Done 2026-10-31: the key comes from the prelude's `keys` module
   (`codegen::emit::keys_module`).
2. **Calls a mod adds get no address** (filed: `mod/mod-call-form`): lr
   gets a reserved key outside the GBA map that `call!` accepts as "came
   back"; a restore that has to resume there is counted and reported like
   a mod resume.
   Done 2026-10-31: `call!(m, pc, f)`; a record in guest RAM below the
   callee's stack lets a restore inside the call go on where the call's
   block began (`docs/mod-view-design.md`, "A call a mod adds").
3. **Return sites and resume keys named**, so `-> 0x0800_bc3f` reads
   `-> after_sprite_update_2` and `tick!(m, pc = loop_top, …)`, bound to
   addresses in the generated table. Byte-identical state. Its cost is
   names: thousands of sites a game, which need to be ones a reader can
   follow. Not filed; june decides whether 1 and 2 are enough.
4. **A `#[gba]` proc macro for mod code** (june's thread "Rust proc
   macros for GBA guest calls"): mod code written as plain Rust calls,
   rewritten to `call!`. It works on top of 1 and 2: a proc macro sees
   only syntax, so it needs the callee's key from a const (1) and a
   return key that isn't a ROM address (2). Making a mod function
   resumable after a restore mid-call is a state machine (what `async`
   compiles to), a much bigger project; the reserved key reports that
   case instead.

## What would change this

- More games like WarioWare: each is a function native returns would
  get wrong, though only where the interpreter or a dispatcher fallback
  doesn't catch it, as `settle` does today.
- Dropping the interpreter fallback entirely (every byte of code
  compiled, RAM code included) and giving up save states across builds,
  mGBA states and byte-for-byte checks would make native returns
  possible. It would still gain nothing measurable, and function pointers
  would keep their addresses.
- A target where the check does cost (an ESP32 build, with little
  headroom) would be worth measuring the same way before deciding.

## A different question: does a build need the ROM?

Yes, and this doesn't change it. A build carries its ROM
(`include_bytes!`, `crates/desktop/src/lib.rs`) for the data the code
reads (graphics, music, tables) and for the code the interpreter runs.
Taking the ROM file away is the asset extraction group's direction.
